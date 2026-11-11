use crate::{files, service::Engine};
use anyhow::{bail, Context, Result};
use image::{DynamicImage, ImageBuffer, ImageFormat, ImageReader};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::BufReader,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::UNIX_EPOCH,
};

fn orientation(path: &Path) -> u32 {
    File::open(path)
        .ok()
        .and_then(|file| {
            exif::Reader::new()
                .read_from_container(&mut BufReader::new(file))
                .ok()
        })
        .and_then(|exif| {
            exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .and_then(|v| v.value.get_uint(0))
        })
        .unwrap_or(1)
}

fn jpeg_preview(path: &Path) -> Result<DynamicImage> {
    let mut decoder = jpeg_decoder::Decoder::new(BufReader::new(File::open(path)?));
    decoder.scale(900, 900)?;
    let pixels = decoder.decode()?;
    let info = decoder.info().context("Missing JPEG dimensions")?;
    let width = u32::from(info.width);
    let height = u32::from(info.height);
    Ok(match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => DynamicImage::ImageLuma8(
            ImageBuffer::from_raw(width, height, pixels).context("Invalid JPEG pixels")?,
        ),
        jpeg_decoder::PixelFormat::RGB24 => DynamicImage::ImageRgb8(
            ImageBuffer::from_raw(width, height, pixels).context("Invalid JPEG pixels")?,
        ),
        _ => ImageReader::open(path)?.with_guessed_format()?.decode()?,
    })
}

pub fn generate(path: &Path, destination: &Path) -> Result<()> {
    let reader = ImageReader::open(path)?.with_guessed_format()?;
    let format = reader
        .format()
        .context("Preview unavailable for this image format")?;
    let mut image = if format == ImageFormat::Jpeg {
        jpeg_preview(path)?
    } else {
        let mut reader = ImageReader::open(path)?.with_guessed_format()?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(256 * 1024 * 1024);
        reader.limits(limits);
        reader.decode()?
    };
    image = match orientation(path) {
        2 => image.fliph(),
        3 => image.rotate180(),
        4 => image.flipv(),
        5 => image.rotate90().fliph(),
        6 => image.rotate90(),
        7 => image.rotate270().fliph(),
        8 => image.rotate270(),
        _ => image,
    };
    let preview = image.thumbnail(900, 700).to_rgb8();
    let parent = destination
        .parent()
        .context("Invalid thumbnail destination")?;
    fs::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut temp, 82).encode_image(&preview)?;
    temp.persist(destination).map_err(|e| e.error)?;
    Ok(())
}

pub fn source(engine: &Arc<Engine>, relative: &str) -> Result<Value> {
    let folders = engine.configured_folders()?;
    let path = files::media_path(&folders.gallery, relative)?;
    let kind = files::media_kind(path.file_name().and_then(|s| s.to_str()).unwrap_or(""))
        .context("Unsupported media")?;
    if kind == "videos" {
        return Ok(json!({"path":path,"kind":kind}));
    }
    let _guard = engine
        .preview_lock
        .lock()
        .map_err(|_| anyhow::anyhow!("Preview unavailable"))?;
    let destination = cached_thumbnail(&path, &engine.cache_path)?;
    Ok(json!({"path":destination,"kind":kind}))
}

pub fn cached_thumbnail(path: &Path, cache: &Path) -> Result<PathBuf> {
    let metadata = fs::metadata(path)?;
    let modified = metadata.modified()?.duration_since(UNIX_EPOCH)?.as_nanos();
    let key = hex::encode(Sha256::digest(
        format!("v1:{}:{}:{modified}", path.display(), metadata.len()).as_bytes(),
    ));
    fs::create_dir_all(cache)?;
    let destination = cache.join(format!("{key}.jpg"));
    if !destination.is_file() {
        generate(path, &destination)?;
        // Batch directory scans when cleanup generates thousands of thumbnails.
        static GENERATED: AtomicUsize = AtomicUsize::new(0);
        if GENERATED.fetch_add(1, Ordering::Relaxed).is_multiple_of(32) {
            prune_cache(cache, 512 * 1024 * 1024)?;
        }
    }
    Ok(destination)
}

pub fn prune_cache(cache: &Path, maximum_bytes: u64) -> Result<()> {
    if !cache.is_dir() {
        return Ok(());
    }
    let mut entries: Vec<(PathBuf, fs::Metadata)> = fs::read_dir(cache)?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            if !entry.file_type().ok()?.is_file() || entry.path().extension()? != "jpg" {
                return None;
            }
            Some((entry.path(), entry.metadata().ok()?))
        })
        .collect();
    entries.sort_by_key(|(_, m)| m.modified().ok());
    let mut total: u64 = entries.iter().map(|(_, m)| m.len()).sum();
    for (path, metadata) in entries {
        if total <= maximum_bytes {
            break;
        }
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        total -= metadata.len();
    }
    Ok(())
}

pub fn validate_report_destination(path: &Path) -> Result<()> {
    if !path
        .extension()
        .is_some_and(|value| value.eq_ignore_ascii_case("csv"))
    {
        bail!("Report must have a .csv extension");
    }
    Ok(())
}
