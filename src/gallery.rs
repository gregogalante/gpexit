use crate::{config::Folders, dates, files};
use anyhow::{bail, Context, Result};
use chrono::{Datelike, NaiveDate};
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};
use walkdir::WalkDir;

fn connection(gallery: &Path) -> Result<Connection> {
    let connection = Connection::open(gallery.join(".gpexit-dates.sqlite3"))?;
    connection.execute_batch("CREATE TABLE IF NOT EXISTS decisions (checksum TEXT PRIMARY KEY, date TEXT NOT NULL); CREATE TABLE IF NOT EXISTS kept (path TEXT PRIMARY KEY, size INTEGER, modified INTEGER);")?;
    Ok(connection)
}

fn relative(root: &Path, path: &Path) -> Result<String> {
    Ok(path
        .strip_prefix(root)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn parse_date(value: &str) -> Result<NaiveDate> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d")?;
    if !dates::valid_date(date) || date.format("%Y-%m-%d").to_string() != value {
        bail!("Date must be between 1900 and next year, in YYYY-MM-DD format");
    }
    Ok(date)
}

fn valid_checksum(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut temporary =
        tempfile::NamedTempFile::new_in(path.parent().context("Invalid metadata path")?)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

fn read_metadata(path: &Path) -> Result<Value> {
    let path = files::sidecar(path);
    if path.is_symlink() {
        bail!("Metadata cannot be a symbolic link");
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

pub fn load_decisions(gallery: &Path) -> Result<HashMap<String, String>> {
    if !gallery.join(".gpexit-dates.sqlite3").exists() {
        return Ok(HashMap::new());
    }
    let connection = connection(gallery)?;
    let mut statement = connection.prepare("SELECT checksum, date FROM decisions")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut values = HashMap::new();
    for row in rows {
        let (checksum, date) = row?;
        if !valid_checksum(&checksum) {
            bail!("Invalid checksum in date database");
        }
        parse_date(&date)?;
        values.insert(checksum, date);
    }
    Ok(values)
}

pub fn set_decisions(gallery: &Path, values: &HashMap<String, String>) -> Result<()> {
    if values.is_empty() {
        return Ok(());
    }
    for (checksum, date) in values {
        if !valid_checksum(checksum) {
            bail!("Invalid date manifest checksum");
        }
        parse_date(date)?;
    }
    let mut connection = connection(gallery)?;
    let transaction = connection.transaction()?;
    for (checksum, date) in values {
        transaction.execute(
            "INSERT OR IGNORE INTO decisions VALUES (?1, ?2)",
            params![checksum, date],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

pub fn update_sidecar_date(media: &Path, date: NaiveDate) -> Result<()> {
    if !dates::valid_date(date) {
        bail!("Date is outside the supported range");
    }
    let sidecar = files::sidecar(media);
    let mut metadata = if sidecar.exists() {
        read_metadata(media)?
    } else {
        json!({"title": media.file_name().context("Invalid media path")?.to_string_lossy()})
    };
    let date_string = date.format("%Y-%m-%d").to_string();
    if metadata.get("gpexitDate").and_then(Value::as_str) == Some(date_string.as_str())
        && dates::google_date(&metadata) == Some(date)
    {
        return Ok(());
    }
    let clock = dates::google_timestamp(&metadata)
        .map(|date| date.time())
        .unwrap_or_default();
    let corrected = date.and_time(clock).and_utc();
    let object = metadata
        .as_object_mut()
        .context("Google metadata must be a JSON object")?;
    let original = object.get("photoTakenTime").cloned().unwrap_or(Value::Null);
    object
        .entry("gpexitOriginalPhotoTakenTime")
        .or_insert(original);
    object.insert(
        "photoTakenTime".to_string(),
        json!({
            "timestamp": corrected.timestamp().to_string(),
            "formatted": corrected.format("%Y-%m-%d %H:%M:%S UTC").to_string()
        }),
    );
    object.insert("gpexitDate".to_string(), Value::String(date_string));
    let mut bytes = serde_json::to_vec_pretty(&metadata)?;
    bytes.push(b'\n');
    atomic_write(&sidecar, &bytes)
}

pub fn correct_date(folders: &Folders, relative_path: &str, value: &str) -> Result<String> {
    let date = parse_date(value)?;
    let media = files::media_path(&folders.gallery, relative_path)?;
    let checksum = files::file_hash(&media)?;
    let kind = files::media_kind(&media.to_string_lossy()).context("Unsupported media")?;
    let folder = files::safe_directory(
        &folders.gallery,
        Path::new(&format!("{:04}/{:02}/{kind}", date.year(), date.month())),
    )?;
    // Detect collisions before changing either the JSON or the database.
    if media.parent() != Some(folder.as_path()) {
        let destination = files::destination_for(
            &folder,
            media
                .file_name()
                .and_then(|name| name.to_str())
                .context("Invalid filename")?,
            &checksum,
        )?;
        if files::sidecar(&destination).exists() || files::sidecar(&destination).is_symlink() {
            bail!("Metadata collision");
        }
    }
    let original_json = if files::sidecar(&media).exists() {
        Some(fs::read(files::sidecar(&media))?)
    } else {
        None
    };
    let mut connection = connection(&folders.gallery)?;
    let transaction = connection.transaction()?;
    transaction.execute(
        "INSERT OR REPLACE INTO decisions VALUES (?1, ?2)",
        params![checksum, value],
    )?;
    update_sidecar_date(&media, date)?;
    let moved = match files::move_existing(&media, &folder, &checksum) {
        Ok(path) => path,
        Err(error) => {
            restore_json(&media, original_json.as_deref())?;
            return Err(error);
        }
    };
    if let Err(error) = transaction.commit() {
        if moved != media {
            fs::rename(&moved, &media).context("Cannot roll back corrected media")?;
            fs::rename(files::sidecar(&moved), files::sidecar(&media))
                .context("Cannot roll back corrected metadata")?;
        }
        restore_json(&media, original_json.as_deref())?;
        return Err(error.into());
    }
    relative(&folders.gallery, &moved)
}

fn restore_json(media: &Path, original: Option<&[u8]>) -> Result<()> {
    if let Some(bytes) = original {
        atomic_write(&files::sidecar(media), bytes)
    } else {
        fs::remove_file(files::sidecar(media))?;
        Ok(())
    }
}

pub fn conflicts(folders: &Folders) -> Result<Vec<Value>> {
    let decisions = load_decisions(&folders.gallery)?;
    let mut results = Vec::new();
    for path in files::media_files(&folders.gallery)? {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .context("Invalid media filename")?;
        let Some(name_date) = dates::filename_date(name) else {
            continue;
        };
        let Ok(metadata) = read_metadata(&path) else {
            continue;
        };
        let Some(google_date) = dates::google_date(&metadata) else {
            continue;
        };
        if name_date == google_date
            || metadata.get("gpexitDate").and_then(Value::as_str)
                == Some(google_date.format("%Y-%m-%d").to_string().as_str())
        {
            continue;
        }
        if !decisions.is_empty() && decisions.contains_key(&files::file_hash(&path)?) {
            continue;
        }
        let relative_path = relative(&folders.gallery, &path)?;
        let (year, month) = dates::archive_date_parts(&relative_path);
        let kind = files::media_kind(name);
        let exif = if kind == Some("photos") {
            dates::embedded_date(&path)
        } else {
            None
        };
        let current = if year.as_deref() == Some(google_date.format("%Y").to_string().as_str())
            && month.as_deref() == Some(google_date.format("%m").to_string().as_str())
        {
            Some(google_date)
        } else {
            None
        };
        results.push(
            json!({"path": relative_path, "filename_date": name_date, "google_date": google_date,
                            "exif_date": exif, "current_date": current, "kind": kind}),
        );
    }
    Ok(results)
}

fn file_identity(path: &Path) -> Result<(i64, i64)> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
    Ok((metadata.len().try_into()?, modified.try_into()?))
}

fn kept_files(gallery: &Path) -> Result<HashMap<String, (i64, i64)>> {
    if !gallery.join(".gpexit-dates.sqlite3").exists() {
        return Ok(HashMap::new());
    }
    let connection = connection(gallery)?;
    let mut statement = connection.prepare("SELECT path, size, modified FROM kept")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            (row.get::<_, i64>(1)?, row.get::<_, i64>(2)?),
        ))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

pub fn list_gallery(folders: &Folders) -> Result<Vec<Value>> {
    let kept = kept_files(&folders.gallery)?;
    let mut result = Vec::new();
    for path in files::media_files(&folders.gallery)? {
        let relative_path = relative(&folders.gallery, &path)?;
        let identity = file_identity(&path)?;
        let (year, month) = dates::archive_date_parts(&relative_path);
        result.push(json!({"path": relative_path, "kind": files::media_kind(&path.to_string_lossy()),
                           "size": identity.0, "kept": kept.get(&relative_path) == Some(&identity), "year": year, "month": month}));
    }
    Ok(result)
}

pub fn keep_file(folders: &Folders, relative_path: &str) -> Result<()> {
    let media = files::media_path(&folders.gallery, relative_path)?;
    let relative_path = relative(&folders.gallery, &media)?;
    let (size, modified) = file_identity(&media)?;
    connection(&folders.gallery)?.execute(
        "INSERT OR REPLACE INTO kept VALUES (?1, ?2, ?3)",
        params![relative_path, size, modified],
    )?;
    Ok(())
}

fn valid_trash_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn trash_record(folders: &Folders, id: &str) -> Result<(PathBuf, PathBuf, Value)> {
    if !valid_trash_id(id) || folders.trash().is_symlink() {
        bail!("Invalid trash entry");
    }
    let folder = folders.trash().join(id);
    if folder.is_symlink() {
        bail!("Invalid trash entry");
    }
    let record: Value = serde_json::from_slice(&fs::read(folder.join("entry.json"))?)?;
    let path = files::media_path(
        &folder,
        record
            .get("path")
            .and_then(Value::as_str)
            .context("Invalid trash record")?,
    )?;
    Ok((folder, path, record))
}

pub fn list_trash(folders: &Folders) -> Result<Vec<Value>> {
    if !folders.trash().exists() {
        return Ok(Vec::new());
    }
    if folders.trash().is_symlink() {
        bail!("Trash cannot be a symbolic link");
    }
    let mut items = Vec::new();
    for entry in fs::read_dir(folders.trash())? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        let Ok((_, media, record)) = trash_record(folders, &id) else {
            continue;
        };
        items.push(json!({"id": id, "path": record["path"], "size": media.metadata()?.len()}));
    }
    items.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Ok(items)
}

pub fn trashed_hashes(folders: &Folders) -> Result<HashSet<String>> {
    let mut checksums = HashSet::new();
    for item in list_trash(folders)? {
        let (_, _, record) =
            trash_record(folders, item["id"].as_str().context("Invalid trash ID")?)?;
        if let Some(checksum) = record
            .get("checksum")
            .and_then(Value::as_str)
            .filter(|value| valid_checksum(value))
        {
            checksums.insert(checksum.to_string());
        }
    }
    Ok(checksums)
}

pub fn trash_file(folders: &Folders, relative_path: &str) -> Result<String> {
    trash_file_hashed(folders, relative_path, files::file_hash)
}

pub fn trash_file_checked(
    folders: &Folders,
    relative_path: &str,
    context: &crate::task::TaskContext,
) -> Result<String> {
    trash_file_hashed(folders, relative_path, |path| {
        let mut last_report = std::time::Instant::now();
        files::file_hash_checked(path, |done, total| {
            context.check_cancelled()?;
            if last_report.elapsed() >= std::time::Duration::from_secs(5) {
                context.log(format!(
                    "Checking {relative_path}: {:.0}%",
                    done as f64 * 100.0 / total.max(1) as f64
                ));
                last_report = std::time::Instant::now();
            }
            Ok(())
        })
    })
}

fn trash_file_hashed(
    folders: &Folders,
    relative_path: &str,
    hash: impl FnOnce(&Path) -> Result<String>,
) -> Result<String> {
    let media = files::media_path(&folders.gallery, relative_path)?;
    if folders.trash().is_symlink() {
        bail!("Trash cannot be a symbolic link");
    }
    let relative_path = relative(&folders.gallery, &media)?;
    let checksum = hash(&media)?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    let folder = folders.trash().join(&id);
    let destination = folder.join(&relative_path);
    fs::create_dir_all(destination.parent().context("Invalid trash destination")?)?;
    let result = (|| -> Result<()> {
        atomic_write(
            &folder.join("entry.json"),
            &serde_json::to_vec(&json!({"path": relative_path, "checksum": checksum}))?,
        )?;
        fs::rename(&media, &destination)?;
        let sidecar = files::sidecar(&media);
        if sidecar.exists() || sidecar.is_symlink() {
            if let Err(error) = fs::rename(sidecar, files::sidecar(&destination)) {
                fs::rename(&destination, &media).context("Cannot roll back trash move")?;
                return Err(error.into());
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        if !destination.exists() {
            fs::remove_dir_all(folder)?;
        }
        return Err(error);
    }
    Ok(id)
}

fn restore_target(gallery: &Path, relative_path: &str) -> Result<PathBuf> {
    let path = Path::new(relative_path);
    if path
        .components()
        .any(|part| !matches!(part, Component::Normal(_)))
        || path
            .components()
            .any(|part| part.as_os_str() == ".gpexit-trash")
    {
        bail!("Invalid restore destination");
    }
    let mut target = gallery.canonicalize()?;
    for part in path.components() {
        target.push(part.as_os_str());
        if target.is_symlink() {
            bail!("Restore destination cannot contain symbolic links");
        }
    }
    Ok(target)
}

pub fn restore_file(folders: &Folders, id: &str) -> Result<String> {
    let (folder, media, record) = trash_record(folders, id)?;
    let relative_path = record["path"].as_str().context("Invalid trash path")?;
    let destination = restore_target(&folders.gallery, relative_path)?;
    let old_json = files::sidecar(&media);
    let new_json = files::sidecar(&destination);
    if destination.exists()
        || destination.is_symlink()
        || new_json.exists()
        || new_json.is_symlink()
    {
        bail!("File already exists in gallery; restore would overwrite it");
    }
    fs::create_dir_all(
        destination
            .parent()
            .context("Invalid restore destination")?,
    )?;
    fs::rename(&media, &destination)?;
    if old_json.exists() || old_json.is_symlink() {
        if let Err(error) = fs::rename(old_json, new_json) {
            fs::rename(&destination, &media).context("Cannot roll back restored media")?;
            return Err(error.into());
        }
    }
    fs::remove_dir_all(folder)?;
    Ok(relative_path.to_string())
}

fn folder_usage(folder: &Path, exclude_trash: bool) -> Result<Value> {
    if !folder.exists() || folder.is_symlink() {
        return Ok(json!({"bytes": 0, "files": 0}));
    }
    let mut bytes = 0_u64;
    let mut files = 0_u64;
    for entry in WalkDir::new(folder)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !(entry.file_type().is_symlink()
                || exclude_trash && entry.depth() == 1 && entry.file_name() == ".gpexit-trash")
        })
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error)
                if error
                    .io_error()
                    .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
            {
                continue
            }
            Err(error) => return Err(error.into()),
        };
        if entry.file_type().is_file() {
            match entry.metadata() {
                Ok(metadata) => {
                    bytes += metadata.len();
                    files += 1;
                }
                Err(error)
                    if error
                        .io_error()
                        .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                {
                    continue
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    Ok(json!({"bytes": bytes, "files": files}))
}

pub fn storage_stats(folders: &Folders) -> Result<Value> {
    Ok(
        json!({"inputs": folder_usage(&folders.inputs, false)?, "gallery": folder_usage(&folders.gallery, true)?,
              "exports": folder_usage(&folders.exports, false)?, "trash": folder_usage(&folders.trash(), false)?,
              "disk": {"free": fs2::available_space(&folders.gallery)?, "total": fs2::total_space(&folders.gallery)?}}),
    )
}
