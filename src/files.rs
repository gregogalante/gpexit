use anyhow::{bail, Context, Result};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::{
    fs,
    fs::File,
    io::Read,
    path::{Component, Path, PathBuf},
    sync::LazyLock,
};
use walkdir::WalkDir;

pub fn media_kind(name: &str) -> Option<&'static str> {
    let extension = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    match extension.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" | "avif" | "bmp" | "tif"
        | "tiff" | "dng" | "raw" | "arw" | "cr2" | "cr3" | "nef" | "orf" | "raf" | "rw2" => {
            Some("photos")
        }
        "mp4" | "mov" | "avi" | "m4v" | "mpg" | "mpeg" | "3gp" | "3g2" | "mkv" | "webm" | "mts"
        | "m2ts" | "wmv" => Some("videos"),
        _ => None,
    }
}

pub fn file_hash(path: &Path) -> Result<String> {
    file_hash_checked(path, |_, _| Ok(()))
}

pub fn file_hash_checked(
    path: &Path,
    mut progress: impl FnMut(u64, u64) -> Result<()>,
) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("Cannot read {}", path.display()))?;
    let before = file.metadata()?;
    progress(0, before.len())?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut done = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        done += read as u64;
        progress(done, before.len())?;
    }
    let after = fs::metadata(path)?;
    if before.len() != after.len() || before.modified()? != after.modified()? {
        bail!("File changed while being checked: {}", path.display());
    }
    Ok(hex::encode(digest.finalize()))
}

pub fn safe_filename(name: &str) -> String {
    static RESERVED: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i)^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)").unwrap());
    let sanitized: String = name
        .chars()
        .map(|ch| {
            if ch < ' ' || "<>:\"\\|?*/".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect();
    let mut sanitized = sanitized.trim_end_matches([' ', '.']).to_string();
    if RESERVED.is_match(&sanitized) {
        sanitized.insert(0, '_');
    }
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

pub fn sidecar(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".json");
    PathBuf::from(name)
}

pub fn safe_directory(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut destination = root.to_path_buf();
    for part in relative.components() {
        if !matches!(part, Component::Normal(_)) {
            bail!("Invalid gallery directory");
        }
        destination.push(part.as_os_str());
        if destination.is_symlink() {
            bail!("Gallery destinations cannot contain symbolic links");
        }
    }
    Ok(destination)
}

pub fn destination_for(folder: &Path, name: &str, checksum: &str) -> Result<PathBuf> {
    if Path::new(name).components().count() != 1 || matches!(name, "." | ".." | "") {
        bail!("Invalid destination filename");
    }
    fs::create_dir_all(folder)?;
    let mut destination = folder.join(name);
    if destination.exists() || destination.is_symlink() {
        let path = Path::new(name);
        let stem = path
            .file_stem()
            .and_then(|part| part.to_str())
            .context("Invalid filename")?;
        let suffix = path
            .extension()
            .and_then(|part| part.to_str())
            .map(|ext| format!(".{ext}"))
            .unwrap_or_default();
        destination = folder.join(format!(
            "{stem}_{}{suffix}",
            &checksum[..checksum.len().min(16)]
        ));
        if destination.exists() || destination.is_symlink() {
            bail!("Filename collision: {}", destination.display());
        }
    }
    Ok(destination)
}

pub fn move_existing(path: &Path, folder: &Path, checksum: &str) -> Result<PathBuf> {
    if path.parent() == Some(folder) {
        return Ok(path.to_path_buf());
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("Invalid media filename")?;
    let destination = destination_for(folder, name, checksum)?;
    let old_json = sidecar(path);
    let new_json = sidecar(&destination);
    if new_json.exists() || new_json.is_symlink() {
        bail!("Metadata collision: {}", new_json.display());
    }
    fs::rename(path, &destination)?;
    if old_json.exists() || old_json.is_symlink() {
        if let Err(error) = fs::rename(&old_json, &new_json) {
            fs::rename(&destination, path).context("Cannot roll back media move")?;
            return Err(error.into());
        }
    }
    Ok(destination)
}

fn excluded(component: &str) -> bool {
    matches!(component, ".gpexit-trash" | "archives" | "archivi")
}

pub fn media_path(gallery: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.as_os_str().is_empty()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        bail!("File not found in gallery");
    }
    if relative
        .components()
        .any(|part| excluded(&part.as_os_str().to_string_lossy()))
    {
        bail!("File is outside the gallery media folders");
    }
    let root = gallery.canonicalize()?;
    let mut candidate = gallery.to_path_buf();
    for part in relative.components() {
        candidate.push(part.as_os_str());
        if candidate.is_symlink() {
            bail!("Symbolic links are not gallery media");
        }
    }
    let canonical = candidate
        .canonicalize()
        .context("File not found in gallery")?;
    if !canonical.starts_with(&root)
        || !canonical.is_file()
        || media_kind(&canonical.to_string_lossy()).is_none()
    {
        bail!("File not found in gallery");
    }
    Ok(candidate)
}

pub fn media_files(gallery: &Path) -> Result<Vec<PathBuf>> {
    if !gallery.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in WalkDir::new(gallery)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| {
            !entry.file_type().is_symlink()
                && (entry.depth() == 0 || !excluded(&entry.file_name().to_string_lossy()))
        })
    {
        let entry = entry?;
        if entry.file_type().is_file() && media_kind(&entry.file_name().to_string_lossy()).is_some()
        {
            result.push(entry.into_path());
        }
    }
    result.sort();
    Ok(result)
}
