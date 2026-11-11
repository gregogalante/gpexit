//! Derived, conservative cleanup suggestions. Original media is never modified here.
use crate::{config::Folders, dates, files, gallery, preview, task::TaskContext};
use anyhow::{bail, Context, Result};
use chrono::NaiveDateTime;
use image::imageops::FilterType;
use regex::Regex;
use rusqlite::{params, Connection};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::UNIX_EPOCH,
};

const ALGORITHM_VERSION: i64 = 1;
const MIN_TEXTURE: f64 = 25.0;
const BUCKET_LIMIT: usize = 128;
const CANDIDATE_LIMIT: usize = 1024;
const SIMILAR_GROUP_LIMIT: usize = 24;

pub struct CleanupResults {
    pub items: Vec<Value>,
    pub summary: Value,
}

#[derive(Clone)]
struct Media {
    path: PathBuf,
    relative: String,
    kind: String,
    size: u64,
    modified: i64,
    fingerprint: String,
}

#[derive(Clone)]
struct Analysis {
    size: u64,
    modified: i64,
    version: i64,
    hash: Option<u64>,
    sharpness: Option<f64>,
    texture: Option<f64>,
    error: Option<String>,
}

fn identity(path: &Path) -> Result<(u64, i64)> {
    let metadata = fs::metadata(path)?;
    if !metadata.is_file() || path.is_symlink() {
        bail!("Not a regular gallery file");
    }
    let modified: i64 = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_nanos()
        .try_into()?;
    Ok((metadata.len(), modified))
}

fn fingerprint_identity(path: &Path, size: u64, modified: i64) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(path.canonicalize()?.as_os_str().as_encoded_bytes());
    hash.update([0]);
    hash.update(size.to_le_bytes());
    hash.update(modified.to_le_bytes());
    Ok(hex::encode(hash.finalize()))
}

pub fn fingerprint(path: &Path) -> Result<String> {
    let (size, modified) = identity(path)?;
    fingerprint_identity(path, size, modified)
}

fn eligible(folders: &Folders) -> Result<(Vec<Media>, usize)> {
    let mut result = Vec::new();
    let mut kept = 0;
    for row in gallery::list_gallery(folders)? {
        if row["kept"] == true {
            kept += 1;
            continue;
        }
        let relative = row["path"]
            .as_str()
            .context("Invalid gallery path")?
            .to_string();
        let path = files::media_path(&folders.gallery, &relative)?;
        let (size, modified) = identity(&path)?;
        let fingerprint = fingerprint_identity(&path, size, modified)?;
        result.push(Media {
            path,
            relative,
            kind: row["kind"]
                .as_str()
                .context("Invalid media kind")?
                .to_string(),
            size,
            modified,
            fingerprint,
        });
    }
    Ok((result, kept))
}

fn database(folders: &Folders) -> Result<Connection> {
    let path = folders.gallery.join(".gpexit-cleanup.sqlite3");
    if path.is_symlink() {
        bail!("Cleanup cache cannot be a symbolic link");
    }
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; CREATE TABLE IF NOT EXISTS analysis (path TEXT PRIMARY KEY, size INTEGER NOT NULL, modified INTEGER NOT NULL, version INTEGER NOT NULL, hash TEXT, sharpness REAL, texture REAL, error TEXT);")?;
    Ok(connection)
}

fn load_analysis(folders: &Folders) -> Result<HashMap<String, Analysis>> {
    if !folders.gallery.join(".gpexit-cleanup.sqlite3").exists() {
        return Ok(HashMap::new());
    }
    let connection = database(folders)?;
    let mut statement = connection
        .prepare("SELECT path,size,modified,version,hash,sharpness,texture,error FROM analysis")?;
    let rows = statement.query_map([], |row| {
        let hash: Option<String> = row.get(4)?;
        Ok((
            row.get::<_, String>(0)?,
            Analysis {
                size: row.get(1)?,
                modified: row.get(2)?,
                version: row.get(3)?,
                hash: hash.and_then(|value| u64::from_str_radix(&value, 16).ok()),
                sharpness: row.get(5)?,
                texture: row.get(6)?,
                error: row.get(7)?,
            },
        ))
    })?;
    Ok(rows.collect::<std::result::Result<_, _>>()?)
}

fn current<'a>(media: &Media, rows: &'a HashMap<String, Analysis>) -> Option<&'a Analysis> {
    rows.get(&media.relative).filter(|row| {
        row.size == media.size && row.modified == media.modified && row.version == ALGORITHM_VERSION
    })
}

fn metrics(path: &Path) -> Result<(u64, f64, f64)> {
    let image = image::open(path).context("Cannot decode cleanup thumbnail")?;
    let small = image.resize_exact(9, 8, FilterType::Triangle).to_luma8();
    let mut hash = 0_u64;
    for y in 0..8 {
        for x in 0..8 {
            hash <<= 1;
            if small.get_pixel(x, y)[0] > small.get_pixel(x + 1, y)[0] {
                hash |= 1;
            }
        }
    }
    let gray = image.thumbnail(512, 512).to_luma8();
    let pixels = gray.as_raw();
    if pixels.is_empty() {
        bail!("Empty cleanup image");
    }
    let count = pixels.len() as f64;
    let mean = pixels.iter().map(|&v| f64::from(v)).sum::<f64>() / count;
    let texture =
        (pixels.iter().map(|&v| f64::from(v).powi(2)).sum::<f64>() / count - mean.powi(2)).max(0.0);
    let (width, height) = gray.dimensions();
    let mut sum = 0.0;
    let mut squares = 0.0;
    let mut samples = 0_u64;
    for y in 1..height.saturating_sub(1) {
        for x in 1..width.saturating_sub(1) {
            let pixel = |x, y| f64::from(gray.get_pixel(x, y)[0]);
            let laplacian = pixel(x - 1, y) + pixel(x + 1, y) + pixel(x, y - 1) + pixel(x, y + 1)
                - 4.0 * pixel(x, y);
            sum += laplacian;
            squares += laplacian * laplacian;
            samples += 1;
        }
    }
    let sharpness = if samples == 0 {
        0.0
    } else {
        (squares / samples as f64 - (sum / samples as f64).powi(2)).max(0.0)
    };
    Ok((hash, sharpness, texture))
}

pub fn analyze(folders: &Folders, cache: &Path, ctx: &TaskContext) -> Result<Value> {
    ctx.check_cancelled()?;
    let (media, kept) = eligible(folders)?;
    let photos: Vec<_> = media
        .into_iter()
        .filter(|media| media.kind == "photos")
        .collect();
    let existing = load_analysis(folders)?;
    let connection = database(folders)?;
    let mut analyzed = 0;
    let mut unsupported = 0;
    let mut cache_hits = 0;
    let mut changed = 0;
    ctx.progress(0, photos.len() as u64);
    ctx.log(format!(
        "Cleanup analysis: {} photos; {kept} kept files excluded",
        photos.len()
    ));
    for (index, media) in photos.iter().enumerate() {
        ctx.check_cancelled()?;
        if let Some(row) = current(media, &existing) {
            cache_hits += 1;
            if row.error.is_some() {
                unsupported += 1;
            } else {
                analyzed += 1;
            }
        } else {
            let result =
                preview::cached_thumbnail(&media.path, cache).and_then(|path| metrics(&path));
            ctx.check_cancelled()?;
            if identity(&media.path)? != (media.size, media.modified) {
                changed += 1;
                ctx.log(format!(
                    "Changed during analysis; retry later: {}",
                    media.relative
                ));
            } else {
                let (hash, sharpness, texture, error) = match result {
                    Ok((hash, sharpness, texture)) => {
                        analyzed += 1;
                        (
                            Some(format!("{hash:016x}")),
                            Some(sharpness),
                            Some(texture),
                            None,
                        )
                    }
                    Err(error) => {
                        unsupported += 1;
                        (None, None, None, Some(format!("{error:#}")))
                    }
                };
                connection.execute(
                    "INSERT OR REPLACE INTO analysis VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        media.relative,
                        media.size,
                        media.modified,
                        ALGORITHM_VERSION,
                        hash,
                        sharpness,
                        texture,
                        error
                    ],
                )?;
            }
        }
        ctx.progress((index + 1) as u64, photos.len() as u64);
        if (index + 1) % 25 == 0 || index + 1 == photos.len() {
            ctx.log(format!(
                "Cleanup analysis: {}/{}; {cache_hits} cached; {unsupported} unavailable",
                index + 1,
                photos.len()
            ));
        }
    }
    ctx.check_cancelled()?;
    preview::prune_cache(cache, 512 * 1024 * 1024)?;
    Ok(
        json!({"eligible":photos.len(),"analyzed":analyzed,"pending":changed,"unsupported":unsupported,"cache_hits":cache_hits,"kept_excluded":kept,"algorithm_version":ALGORITHM_VERSION}),
    )
}

fn item(media: &Media, score: Option<f64>) -> Value {
    let mut value = json!({"path":media.relative,"kind":media.kind,"size":media.size,"fingerprint":media.fingerprint});
    if let Some(score) = score {
        value["score"] = json!(score);
    }
    value
}

fn group(reason: &str, items: Vec<Value>) -> Value {
    let mut hash = Sha256::new();
    hash.update(reason.as_bytes());
    let mut size = 0_u64;
    for item in &items {
        hash.update([0]);
        hash.update(item["fingerprint"].as_str().unwrap_or_default().as_bytes());
        size = size.saturating_add(item["size"].as_u64().unwrap_or(0));
    }
    json!({"id":hex::encode(hash.finalize()),"reason":reason,"size":size,"items":items})
}

fn burst_timestamp(path: &Path) -> Option<(&'static str, i64)> {
    let metadata = fs::read(files::sidecar(path))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let manual_date = metadata
        .as_ref()
        .and_then(|value| value["gpexitDate"].as_str())
        .and_then(|value| chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .filter(|date| dates::valid_date(*date));
    static TIMESTAMP: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"((?:19|20)\d{2})[-_.]?(\d{2})[-_.]?(\d{2})[T_ .-]?(\d{2})[-_.:]?(\d{2})[-_.:]?(\d{2})").unwrap()
    });
    let name = path.file_name()?.to_str()?;
    for capture in TIMESTAMP.captures_iter(name) {
        let found = capture.get(0)?;
        if name[..found.start()]
            .chars()
            .next_back()
            .is_some_and(|character| character.is_ascii_digit())
        {
            continue;
        }
        let following = name[found.end()..]
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        if !matches!(following, 0 | 3) {
            continue;
        }
        let date = chrono::NaiveDate::from_ymd_opt(
            capture[1].parse().ok()?,
            capture[2].parse().ok()?,
            capture[3].parse().ok()?,
        );
        let Some(time) = date.and_then(|date| {
            date.and_hms_opt(
                capture[4].parse().ok()?,
                capture[5].parse().ok()?,
                capture[6].parse().ok()?,
            )
        }) else {
            continue;
        };
        let time = manual_date
            .map(|date| date.and_time(time.time()))
            .unwrap_or(time);
        if dates::valid_date(time.date()) {
            return Some(("filename-local", time.and_utc().timestamp()));
        }
    }
    if let Some(metadata) = metadata {
        if let Some(timestamp) = dates::google_timestamp(&metadata) {
            return Some(("google-utc", timestamp.timestamp()));
        }
    }
    // EXIF has camera-local clock semantics, separate from Google UTC timestamps.
    let file = fs::File::open(path).ok()?;
    let exif = exif::Reader::new()
        .read_from_container(&mut std::io::BufReader::new(file))
        .ok()?;
    for tag in [exif::Tag::DateTimeOriginal, exif::Tag::DateTimeDigitized] {
        if let Some(field) = exif.get_field(tag, exif::In::PRIMARY) {
            if let exif::Value::Ascii(values) = &field.value {
                for value in values {
                    let Some(time) = std::str::from_utf8(value)
                        .ok()
                        .and_then(|text| text.get(..19))
                        .and_then(|text| {
                            NaiveDateTime::parse_from_str(text, "%Y:%m:%d %H:%M:%S").ok()
                        })
                    else {
                        continue;
                    };
                    let time = manual_date
                        .map(|date| date.and_time(time.time()))
                        .unwrap_or(time);
                    if dates::valid_date(time.date()) {
                        return Some(("exif-local", time.and_utc().timestamp()));
                    }
                }
            }
        }
    }
    None
}

fn bursts(media: &[Media], gap: u64) -> Vec<Value> {
    let mut timed: Vec<_> = media
        .iter()
        .filter_map(|media| {
            burst_timestamp(&media.path).map(|(source, time)| (source, time, media))
        })
        .collect();
    timed.sort_by(|a, b| (a.0, a.1, &a.2.relative).cmp(&(b.0, b.1, &b.2.relative)));
    let mut result = Vec::new();
    let mut active: Vec<(&str, i64, &Media)> = Vec::new();
    for entry in timed {
        let separate = active.last().is_some_and(|last| {
            last.0 != entry.0
                || entry.1.saturating_sub(last.1) as u64 > gap
                || entry.1.saturating_sub(active[0].1) > 60
                || active.len() >= 12
        });
        if separate {
            if active.len() >= 2 {
                result.push(group(
                    "burst",
                    active
                        .iter()
                        .map(|(_, _, media)| item(media, None))
                        .collect(),
                ));
            }
            active.clear();
        }
        active.push(entry);
    }
    if active.len() >= 2 {
        result.push(group(
            "burst",
            active
                .iter()
                .map(|(_, _, media)| item(media, None))
                .collect(),
        ));
    }
    result
}

fn partition_keys(hash: u64, distance: u32) -> Vec<(u32, u64)> {
    let parts = distance + 1;
    let mut keys = Vec::with_capacity(parts as usize);
    let mut start = 0;
    for part in 0..parts {
        let width = 64 / parts + u32::from(part < 64 % parts);
        let mask = if width == 64 {
            u64::MAX
        } else {
            (1_u64 << width) - 1
        };
        keys.push((part, (hash >> start) & mask));
        start += width;
    }
    keys
}

fn similar(media: &[Media], rows: &HashMap<String, Analysis>, distance: u32) -> Vec<Value> {
    let mut buckets: HashMap<(u32, u64), VecDeque<usize>> = HashMap::new();
    let mut representatives: Vec<(u64, Vec<Value>)> = Vec::new();
    for media in media {
        let Some(row) = current(media, rows) else {
            continue;
        };
        let Some(hash) = row.hash else { continue };
        if row.error.is_some()
            || row.texture.unwrap_or(0.0) < MIN_TEXTURE
            || !(3..=61).contains(&hash.count_ones())
        {
            continue;
        }
        let keys = partition_keys(hash, distance);
        let mut candidates = HashSet::new();
        for key in &keys {
            if let Some(bucket) = buckets.get(key) {
                for &candidate in bucket.iter().rev() {
                    candidates.insert(candidate);
                    if candidates.len() >= CANDIDATE_LIMIT {
                        break;
                    }
                }
            }
            if candidates.len() >= CANDIDATE_LIMIT {
                break;
            }
        }
        let mut candidates: Vec<_> = candidates.into_iter().collect();
        candidates.sort_unstable();
        let best = candidates
            .into_iter()
            .filter_map(|index| {
                let (anchor, members) = &representatives[index];
                let difference = (anchor ^ hash).count_ones();
                (difference <= distance && members.len() < SIMILAR_GROUP_LIMIT)
                    .then_some((difference, index))
            })
            .min();
        if let Some((difference, index)) = best {
            representatives[index]
                .1
                .push(item(media, Some(f64::from(difference))));
        } else {
            let index = representatives.len();
            representatives.push((hash, vec![item(media, Some(0.0))]));
            for key in keys {
                let bucket = buckets.entry(key).or_default();
                if bucket.len() >= BUCKET_LIMIT {
                    bucket.pop_front();
                }
                bucket.push_back(index);
            }
        }
    }
    representatives
        .into_iter()
        .filter(|(_, members)| members.len() >= 2)
        .map(|(_, members)| group("similar", members))
        .collect()
}

pub fn results(
    folders: &Folders,
    mode: &str,
    min_bytes: u64,
    gap_seconds: u64,
    distance: u32,
    quality_threshold: f64,
) -> Result<CleanupResults> {
    if !matches!(mode, "large" | "bursts" | "similar" | "quality") {
        bail!("Unknown cleanup mode");
    }
    if distance > 10 {
        bail!("Similarity distance must be between 0 and 10");
    }
    if !quality_threshold.is_finite() || quality_threshold < 0.0 {
        bail!("Quality threshold must be a nonnegative finite number");
    }
    let (all, kept) = eligible(folders)?;
    let eligible: Vec<_> = all
        .into_iter()
        .filter(|media| mode == "large" || media.kind == "photos")
        .collect();
    let rows = load_analysis(folders)?;
    let analyzed = eligible
        .iter()
        .filter(|media| {
            media.kind == "photos"
                && current(media, &rows).is_some_and(|row| {
                    row.error.is_none() && row.hash.is_some() && row.sharpness.is_some()
                })
        })
        .count();
    let unsupported = eligible
        .iter()
        .filter(|media| {
            media.kind == "photos" && current(media, &rows).is_some_and(|row| row.error.is_some())
        })
        .count();
    let pending = eligible
        .iter()
        .filter(|media| media.kind == "photos")
        .count()
        - analyzed
        - unsupported;
    let items: Vec<Value> = match mode {
        "large" => {
            let mut selected: Vec<_> = eligible
                .iter()
                .filter(|media| media.size >= min_bytes)
                .collect();
            selected.sort_by(|a, b| b.size.cmp(&a.size).then(a.relative.cmp(&b.relative)));
            selected
                .into_iter()
                .map(|media| group("large", vec![item(media, None)]))
                .collect()
        }
        "bursts" => bursts(&eligible, gap_seconds),
        "similar" => similar(&eligible, &rows, distance),
        "quality" => {
            let mut selected: Vec<_> = eligible
                .iter()
                .filter_map(|media| {
                    let row = current(media, &rows)?;
                    let score = row
                        .sharpness
                        .filter(|score| row.error.is_none() && *score <= quality_threshold)?;
                    Some((score, media))
                })
                .collect();
            selected.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.relative.cmp(&b.1.relative)));
            selected
                .into_iter()
                .map(|(score, media)| group("quality", vec![item(media, Some(score))]))
                .collect()
        }
        _ => unreachable!(),
    };
    let summary = json!({"eligible":eligible.len(),"analyzed":analyzed,"pending":pending,"unsupported":unsupported,"kept_excluded":kept,"groups":items.len(),"algorithm_version":ALGORITHM_VERSION,"approximate":true});
    Ok(CleanupResults { items, summary })
}
