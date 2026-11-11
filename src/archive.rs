//! Streaming archive tasks shared by the native desktop interface.
use anyhow::{anyhow, bail, Context, Result};
use chrono::{Datelike, NaiveDate, Utc};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};
use tempfile::{NamedTempFile, TempDir};
use zip::{write::FileOptions, AesMode, CompressionMethod, ZipArchive, ZipWriter};

use crate::{config::Folders, dates, files, gallery, task::TaskContext};

const CHUNK: usize = 1024 * 1024;
const MAX_JSON: u64 = 4 * CHUNK as u64;
type Metadata = HashMap<(String, String), (Value, Vec<u8>)>;

#[derive(Clone)]
struct Member {
    index: usize,
    path: String,
    size: u64,
    directory: bool,
}

struct Extracted {
    member: usize,
    checksum: String,
    temporary: NamedTempFile,
}

#[derive(Default)]
struct Counts {
    imported: u64,
    duplicates: u64,
    unknown_date: u64,
    unrecognized: u64,
    resumed: u64,
    trashed: u64,
}

impl Counts {
    fn value(&self) -> Value {
        json!({"imported": self.imported, "duplicates": self.duplicates,
            "unknown date": self.unknown_date, "unrecognized": self.unrecognized,
            "resumed": self.resumed, "trashed": self.trashed})
    }
}

fn open_zip(path: &Path) -> Result<ZipArchive<File>> {
    ZipArchive::new(File::open(path).with_context(|| format!("Cannot open {}", path.display()))?)
        .with_context(|| format!("Invalid ZIP: {}", path.display()))
}

fn members(archive: &mut ZipArchive<File>) -> Result<Vec<Member>> {
    (0..archive.len())
        .map(|index| {
            let member = archive.by_index_raw(index)?;
            Ok(Member {
                index,
                path: member.name()?.into_owned(),
                size: member.size(),
                directory: member.is_dir(),
            })
        })
        .collect()
}

fn basename(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}
fn dirname(path: &str) -> &str {
    path.rsplit_once('/').map_or(".", |(parent, _)| parent)
}
fn metadata_key(path: &str, name: &str) -> (String, String) {
    (dirname(path).to_lowercase(), name.to_lowercase())
}

fn read_json(archive: &mut ZipArchive<File>, member: &Member, password: &str) -> Result<Vec<u8>> {
    if member.size > MAX_JSON {
        bail!("Metadata is too large: {}", member.path);
    }
    let mut source = archive
        .by_index_decrypt(member.index, password.as_bytes())
        .with_context(|| format!("Cannot open {}; check ZIP password", member.path))?;
    let mut raw = Vec::with_capacity(member.size as usize);
    (&mut source)
        .take(MAX_JSON + 1)
        .read_to_end(&mut raw)
        .with_context(|| {
            format!(
                "Cannot decrypt or read {}; check password and ZIP integrity",
                member.path
            )
        })?;
    if raw.len() as u64 > MAX_JSON {
        bail!("Metadata is too large: {}", member.path);
    }
    Ok(raw)
}

fn valid_year(value: &str) -> bool {
    value.len() == 4
        && value.bytes().all(|b| b.is_ascii_digit())
        && value
            .parse::<i32>()
            .is_ok_and(|year| (1900..=2099).contains(&year))
}

fn manifest(path: &str) -> bool {
    let parts: Vec<_> = path.split('/').collect();
    parts.len() == 2
        && parts[1] == ".gpexit-dates.json"
        && (valid_year(parts[0]) || parts[0] == "Unknown year")
}

fn manifest_decisions(value: Value) -> Result<HashMap<String, String>> {
    let object = value
        .as_object()
        .context("Expected checksum-to-date mapping")?;
    let mut decisions = HashMap::new();
    for (checksum, value) in object {
        if checksum.len() != 64
            || !checksum
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            bail!("Invalid checksum in date manifest");
        }
        let value = value.as_str().context("Invalid date in date manifest")?;
        let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .context("Invalid date in date manifest")?;
        if date.format("%Y-%m-%d").to_string() != value
            || !(1900..=Utc::now().year() + 1).contains(&date.year())
        {
            bail!("Date outside supported range in date manifest");
        }
        decisions.insert(checksum.clone(), value.to_owned());
    }
    Ok(decisions)
}

fn collect_metadata(
    paths: &[PathBuf],
    password: &str,
    ctx: &TaskContext,
) -> Result<(Metadata, HashMap<String, String>)> {
    let mut records = Metadata::new();
    let mut direct = Metadata::new();
    let mut decisions = HashMap::new();
    for path in paths {
        ctx.check_cancelled()?;
        ctx.log(format!("Reading metadata: {}", path.display()));
        let mut archive = open_zip(path)?;
        for member in members(&mut archive)? {
            ctx.check_cancelled()?;
            if member.directory || !member.path.to_lowercase().ends_with(".json") {
                continue;
            }
            let is_manifest = manifest(&member.path);
            if member.size > MAX_JSON {
                if is_manifest {
                    bail!("Date manifest is too large: {}", member.path);
                }
                continue;
            }
            let raw = read_json(&mut archive, &member, password)?;
            let value: Value = match serde_json::from_slice(&raw) {
                Ok(value) => value,
                Err(error) if is_manifest => return Err(error).context("Invalid date manifest"),
                Err(_) => continue,
            };
            if is_manifest {
                for (checksum, date) in manifest_decisions(value)? {
                    decisions.entry(checksum).or_insert(date);
                }
                continue;
            }
            if !value.is_object() {
                continue;
            }
            if let Some(title) = value.get("title").and_then(Value::as_str) {
                if !title.is_empty() && !title.contains(['/', '\\']) {
                    records
                        .entry(metadata_key(&member.path, title))
                        .or_insert_with(|| (value.clone(), raw.clone()));
                }
            }
            let name = basename(&member.path);
            let media_name = &name[..name.len() - 5];
            if files::media_kind(media_name).is_some() {
                direct
                    .entry(metadata_key(&member.path, media_name))
                    .or_insert((value, raw));
            }
        }
    }
    records.extend(direct);
    Ok((records, decisions))
}

fn hash_file(path: &Path, ctx: &TaskContext) -> Result<String> {
    let mut source = File::open(path)?;
    let total = source.metadata()?.len();
    let mut done = 0u64;
    let mut last_report = Instant::now();
    let mut digest = Sha256::new();
    let mut buffer = vec![0; CHUNK];
    loop {
        ctx.check_cancelled()?;
        let size = source.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        digest.update(&buffer[..size]);
        done += size as u64;
        if last_report.elapsed() >= Duration::from_secs(5) {
            ctx.log(format!(
                "Checking {}: {:.0}%",
                path.display(),
                done as f64 * 100.0 / total.max(1) as f64
            ));
            last_report = Instant::now();
        }
    }
    Ok(hex::encode(digest.finalize()))
}

fn fingerprint(paths: &[PathBuf], ctx: &TaskContext) -> Result<String> {
    let mut values = Vec::new();
    for (number, path) in paths.iter().enumerate() {
        ctx.check_cancelled()?;
        let before = fs::metadata(path)?;
        ctx.log(format!(
            "Checking input {}/{}: {}",
            number + 1,
            paths.len(),
            path.display()
        ));
        let checksum = hash_file(path, ctx)?;
        let after = fs::metadata(path)?;
        if before.len() != after.len() || before.modified()? != after.modified()? {
            bail!("Input changed while being checked: {}", path.display());
        }
        values.push((
            path.canonicalize()?.to_string_lossy().into_owned(),
            checksum,
        ));
    }
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&values)?)))
}

fn index_gallery(
    output: &Path,
    overrides: &HashMap<String, String>,
    ctx: &TaskContext,
) -> Result<HashMap<String, PathBuf>> {
    let paths = files::media_files(output)?;
    let mut index = HashMap::new();
    for (number, path) in paths.iter().enumerate() {
        ctx.check_cancelled()?;
        ctx.progress(number as u64, paths.len() as u64);
        let checksum = hash_file(path, ctx)?;
        let relative = path
            .strip_prefix(output)?
            .to_string_lossy()
            .replace('\\', "/");
        let (year, month) = dates::archive_date_parts(&relative);
        let mut destination = path.clone();
        if let Some(mut year) = year {
            let mut month = month.unwrap_or_else(|| "Unknown month".into());
            let metadata = fs::read(files::sidecar(path))
                .ok()
                .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
                .unwrap_or_else(|| json!({}));
            let manual = overrides.get(&checksum);
            let date = if let Some(value) = manual {
                Some(NaiveDate::parse_from_str(value, "%Y-%m-%d")?)
            } else {
                dates::choose_date(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .as_ref(),
                    &metadata,
                )
                .or_else(|| {
                    if files::media_kind(path.to_string_lossy().as_ref()) == Some("photos") {
                        dates::embedded_date(path)
                    } else {
                        None
                    }
                })
            };
            if let Some(date) = date {
                if manual.is_some() || year == "Unknown year" || date.year().to_string() == year {
                    year = date.year().to_string();
                    month = format!("{:02}", date.month());
                }
                if manual.is_some() {
                    gallery::update_sidecar_date(path, date)?;
                }
            }
            let kind =
                files::media_kind(path.to_string_lossy().as_ref()).context("Unknown media kind")?;
            let folder =
                files::safe_directory(output, &PathBuf::from(year).join(month).join(kind))?;
            destination = files::move_existing(path, &folder, &checksum)?;
        }
        index.entry(checksum).or_insert(destination);
    }
    Ok(index)
}

fn checkpoint(output: &Path) -> Result<Connection> {
    let database = Connection::open(output.join(".gpexit-progress.sqlite3"))?;
    // Progress is reconstructible from hashed media; avoid a disk sync for every member.
    database.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
    database.execute_batch("CREATE TABLE IF NOT EXISTS completed (session TEXT NOT NULL, archive TEXT NOT NULL, member INTEGER NOT NULL, checksum TEXT NOT NULL, PRIMARY KEY (session, archive, member));")?;
    Ok(database)
}

fn save_checkpoint(
    database: &Connection,
    session: &str,
    archive: &str,
    member: usize,
    checksum: &str,
) -> Result<()> {
    database.execute(
        "INSERT OR REPLACE INTO completed VALUES (?1, ?2, ?3, ?4)",
        params![session, archive, member as i64, checksum],
    )?;
    Ok(())
}

fn save_sidecar(destination: &Path, raw: Option<&[u8]>) -> Result<()> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let path = files::sidecar(destination);
    if path.exists() {
        return Ok(());
    }
    let mut temporary = NamedTempFile::new_in(path.parent().context("Invalid metadata path")?)?;
    temporary.write_all(raw)?;
    match temporary.persist_noclobber(&path) {
        Ok(_) => Ok(()),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.error.into()),
    }
}

fn extract(
    archive: &mut ZipArchive<File>,
    member: usize,
    password: &str,
    staging: &Path,
    ctx: &TaskContext,
) -> Result<Extracted> {
    ctx.check_cancelled()?;
    let mut source = archive
        .by_index_decrypt(member, password.as_bytes())
        .context("Cannot open ZIP member; check ZIP password")?;
    let total = source.size();
    let name = source.name()?.into_owned();
    let mut done = 0u64;
    let mut last_report = Instant::now();
    let mut temporary = NamedTempFile::new_in(staging)?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; CHUNK];
    loop {
        ctx.check_cancelled()?;
        let size = source
            .read(&mut buffer)
            .context("Cannot decrypt or read media; check password and ZIP integrity")?;
        if size == 0 {
            break;
        }
        digest.update(&buffer[..size]);
        temporary.write_all(&buffer[..size])?;
        done += size as u64;
        if last_report.elapsed() >= Duration::from_secs(5) {
            ctx.log(format!(
                "Extracting {name}: {:.0}%",
                done as f64 * 100.0 / total.max(1) as f64
            ));
            last_report = Instant::now();
        }
    }
    Ok(Extracted {
        member,
        checksum: hex::encode(digest.finalize()),
        temporary,
    })
}

struct ImportState<'a> {
    output: &'a Path,
    metadata: &'a Metadata,
    overrides: &'a HashMap<String, String>,
    discarded: &'a HashSet<String>,
    index: &'a mut HashMap<String, PathBuf>,
    counts: &'a mut Counts,
    database: &'a Connection,
    session: &'a str,
    archive: &'a str,
    ctx: &'a TaskContext,
}

impl ImportState<'_> {
    fn commit(&mut self, extracted: Extracted, member: &Member) -> Result<()> {
        self.ctx.check_cancelled()?;
        let checksum = &extracted.checksum;
        let name = basename(&member.path);
        if self.discarded.contains(checksum) {
            self.counts.trashed += 1;
            save_checkpoint(
                self.database,
                self.session,
                self.archive,
                member.index,
                checksum,
            )?;
            return Ok(());
        }
        let kind = files::media_kind(name).context("Unknown media kind")?;
        let record = self.metadata.get(&metadata_key(&member.path, name));
        let empty = json!({});
        let metadata = record.map_or(&empty, |(metadata, _)| metadata);
        let raw = record.map(|(_, raw)| raw.as_slice());
        let manual = self
            .overrides
            .get(checksum)
            .map(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d"))
            .transpose()?;
        let (archived_year, archived_month) = dates::archive_date_parts(&member.path);
        let mut date = manual.or_else(|| dates::choose_date(name, metadata));
        if date.is_none()
            && kind == "photos"
            && archived_month
                .as_deref()
                .is_none_or(|month| month == "Unknown month")
        {
            date = dates::embedded_date(extracted.temporary.path());
        }
        let year = date
            .map(|value| value.year().to_string())
            .or(archived_year)
            .unwrap_or_else(|| "Unknown year".into());
        let month = date
            .map(|value| format!("{:02}", value.month()))
            .or(archived_month)
            .unwrap_or_else(|| "Unknown month".into());
        if let Some(existing) = self.index.get(checksum).cloned() {
            let relative = existing
                .strip_prefix(self.output)?
                .to_string_lossy()
                .replace('\\', "/");
            let (existing_year, existing_month) = dates::archive_date_parts(&relative);
            let improve = (existing_year.as_deref() == Some("Unknown year")
                && year != "Unknown year")
                || (existing_year.as_deref() == Some(&year)
                    && existing_month.as_deref() == Some("Unknown month")
                    && month != "Unknown month");
            let destination = if improve {
                files::move_existing(
                    &existing,
                    &files::safe_directory(
                        self.output,
                        &PathBuf::from(&year).join(&month).join(kind),
                    )?,
                    checksum,
                )?
            } else {
                existing
            };
            save_sidecar(&destination, raw)?;
            if let Some(date) = manual {
                gallery::update_sidecar_date(&destination, date)?;
            }
            self.index.insert(checksum.clone(), destination);
            self.counts.duplicates += 1;
        } else {
            let destination = files::destination_for(
                &files::safe_directory(self.output, &PathBuf::from(&year).join(&month).join(kind))?,
                &files::safe_filename(name),
                checksum,
            )?;
            extracted
                .temporary
                .persist_noclobber(&destination)
                .map_err(|error| error.error)?;
            save_sidecar(&destination, raw)?;
            if let Some(date) = manual {
                gallery::update_sidecar_date(&destination, date)?;
            }
            self.index.insert(checksum.clone(), destination);
            self.counts.imported += 1;
            self.counts.unknown_date += u64::from(year == "Unknown year");
        }
        save_checkpoint(
            self.database,
            self.session,
            self.archive,
            member.index,
            checksum,
        )?;
        Ok(())
    }
}

/// Import with at most four streaming extraction workers and deterministic commit order.
pub fn import_archives(
    folders: &Folders,
    paths: &[PathBuf],
    password: &str,
    workers: usize,
    ctx: &TaskContext,
) -> Result<Value> {
    if paths.is_empty() {
        bail!("Select at least one ZIP file");
    }
    if workers == 0 {
        bail!("Worker count must be at least one");
    }
    fs::create_dir_all(&folders.gallery)?;
    let session = fingerprint(paths, ctx)?;
    let (metadata, imported_decisions) = collect_metadata(paths, password, ctx)?;
    gallery::set_decisions(&folders.gallery, &imported_decisions)?;
    let overrides = gallery::load_decisions(&folders.gallery)?;
    let discarded = gallery::trashed_hashes(folders)?;
    ctx.log("Checking existing gallery contents");
    let mut index = index_gallery(&folders.gallery, &overrides, ctx)?;
    let database = checkpoint(&folders.gallery)?;
    let staging = TempDir::with_prefix_in(".import-", &folders.gallery)?;
    let mut counts = Counts::default();
    for archive_path in paths {
        ctx.check_cancelled()?;
        ctx.log(format!("Importing {}", archive_path.display()));
        let archive_name = archive_path.canonicalize()?.to_string_lossy().into_owned();
        let mut archive = open_zip(archive_path)?;
        let entries = members(&mut archive)?;
        let mut statement = database.prepare(
            "SELECT member, checksum FROM completed WHERE session = ?1 AND archive = ?2",
        )?;
        let completed: HashMap<usize, String> = statement
            .query_map(params![session, archive_name], |row| {
                Ok((row.get::<_, i64>(0)? as usize, row.get(1)?))
            })?
            .collect::<std::result::Result<_, _>>()?;
        let mut jobs = Vec::new();
        for member in &entries {
            ctx.check_cancelled()?;
            if member.directory {
                continue;
            }
            let name = basename(&member.path);
            if files::media_kind(name).is_none() {
                if !name.to_lowercase().ends_with(".json") {
                    counts.unrecognized += 1;
                }
                continue;
            }
            if name.is_empty() || name == "." || name == ".." || name.contains('\\') {
                bail!("Invalid media filename: {}", member.path);
            }
            if let Some(checksum) = completed.get(&member.index) {
                let ready = index.get(checksum).is_some_and(|path| {
                    !metadata.contains_key(&metadata_key(&member.path, name))
                        || files::sidecar(path).exists()
                });
                if ready || discarded.contains(checksum) {
                    counts.resumed += 1;
                    continue;
                }
            }
            jobs.push(member.index);
        }
        let worker_count = workers.min(4).min(jobs.len());
        if worker_count == 0 {
            continue;
        }
        let mut state = ImportState {
            output: &folders.gallery,
            metadata: &metadata,
            overrides: &overrides,
            discarded: &discarded,
            index: &mut index,
            counts: &mut counts,
            database: &database,
            session: &session,
            archive: &archive_name,
            ctx,
        };
        std::thread::scope(|scope| -> Result<()> {
            let mut pipes = Vec::new();
            for _ in 0..worker_count {
                let mut reader = open_zip(archive_path)?;
                let (send_job, receive_job) = mpsc::sync_channel(1);
                let (send_result, receive_result) = mpsc::channel();
                let staging = staging.path();
                scope.spawn(move || {
                    for member in receive_job {
                        let result = extract(&mut reader, member, password, staging, ctx);
                        if send_result.send(result).is_err() {
                            break;
                        }
                    }
                });
                pipes.push((send_job, receive_result));
            }
            for (slot, member) in jobs.iter().take(worker_count).enumerate() {
                pipes[slot].0.send(*member)?;
            }
            for (number, member) in jobs.iter().enumerate() {
                ctx.check_cancelled()?;
                let slot = number % worker_count;
                let extracted = pipes[slot]
                    .1
                    .recv()
                    .map_err(|_| anyhow!("Archive extraction worker stopped"))??;
                if extracted.member != *member {
                    bail!("Archive extraction order mismatch");
                }
                state.commit(extracted, &entries[*member])?;
                ctx.progress((number + 1) as u64, jobs.len() as u64);
                if number % 25 == 0 {
                    ctx.log(format!("Imported {}/{} members", number + 1, jobs.len()));
                }
                if let Some(next) = jobs.get(number + worker_count) {
                    pipes[slot].0.send(*next)?;
                }
            }
            Ok(())
        })?;
    }
    ctx.log(format!(
        "Import complete: {} new, {} duplicates, {} resumed, {} trashed",
        counts.imported, counts.duplicates, counts.resumed, counts.trashed
    ));
    Ok(counts.value())
}

/// Create annual ZIPs atomically, preserving review decisions and optional AES-256 encryption.
pub fn export_archives(folders: &Folders, password: &str, ctx: &TaskContext) -> Result<Value> {
    fs::create_dir_all(&folders.exports)?;
    let overrides = gallery::load_decisions(&folders.gallery)?;
    let mut years: Vec<_> = fs::read_dir(&folders.gallery)?
        .filter_map(std::result::Result::ok)
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                && (valid_year(&name) || name == "Unknown year")
        })
        .collect();
    years.sort_by_key(|entry| entry.file_name());
    let mut generated = Vec::new();
    for (year_number, year) in years.iter().enumerate() {
        ctx.check_cancelled()?;
        let name = year.file_name().to_string_lossy().into_owned();
        let destination = folders.exports.join(format!("{name}.zip"));
        ctx.log(format!("Creating {}", destination.display()));
        let mut temporary = NamedTempFile::new_in(&folders.exports)?;
        let mut archive = ZipWriter::new(temporary.as_file_mut());
        let mut options = FileOptions::<()>::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(true);
        if !password.is_empty() {
            options = options.with_aes_encryption(AesMode::Aes256, password);
        }
        let mut year_decisions = HashMap::new();
        let mut entries: Vec<_> = walkdir::WalkDir::new(year.path())
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| !entry.file_type().is_symlink())
            .collect::<std::result::Result<_, _>>()?;
        entries.sort_by_key(|entry| entry.path().to_owned());
        let mut buffer = vec![0; CHUNK];
        for entry in entries {
            ctx.check_cancelled()?;
            if !entry.file_type().is_file() || entry.file_name() == ".gpexit-dates.json" {
                continue;
            }
            let relative = entry
                .path()
                .strip_prefix(&folders.gallery)?
                .to_str()
                .context("Filename is not valid UTF-8")?
                .replace('\\', "/");
            archive.start_file(relative, options)?;
            let mut source = File::open(entry.path())?;
            let track_hash = !overrides.is_empty()
                && files::media_kind(entry.path().to_string_lossy().as_ref()).is_some();
            let mut digest = Sha256::new();
            loop {
                ctx.check_cancelled()?;
                let size = source.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                archive.write_all(&buffer[..size])?;
                if track_hash {
                    digest.update(&buffer[..size]);
                }
            }
            if track_hash {
                let checksum = hex::encode(digest.finalize());
                if let Some(date) = overrides.get(&checksum) {
                    year_decisions.insert(checksum, date.clone());
                }
            }
        }
        if !year_decisions.is_empty() {
            archive.start_file(format!("{name}/.gpexit-dates.json"), options)?;
            archive.write_all(&serde_json::to_vec(&year_decisions)?)?;
        }
        archive.finish()?;
        ctx.check_cancelled()?;
        temporary.as_file().sync_all()?;
        temporary
            .persist(&destination)
            .map_err(|error| error.error)?;
        generated.push(
            destination
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        );
        ctx.progress((year_number + 1) as u64, years.len() as u64);
    }
    Ok(json!({"archives": generated}))
}
