//! Bounded cleanup benchmark using read-only copies of up to 64 real JPEG files.
use anyhow::{Context, Result};
use gpexit_core::{cleanup, config::Folders, files, task::TaskContext};
use serde_json::json;
use std::{fs, path::PathBuf, time::Instant};

fn main() -> Result<()> {
    let source = PathBuf::from(
        std::env::args()
            .nth(1)
            .context("Usage: cleanup_probe <gallery-directory>")?,
    );
    let source = source
        .canonicalize()
        .with_context(|| format!("Cannot open source gallery: {}", source.display()))?;
    anyhow::ensure!(source.is_dir(), "Gallery directory not found");
    let temporary = tempfile::tempdir()?;
    let folders = Folders {
        inputs: temporary.path().join("inputs"),
        gallery: temporary.path().join("gallery"),
        exports: temporary.path().join("exports"),
    };
    for folder in [&folders.inputs, &folders.gallery, &folders.exports] {
        fs::create_dir(folder)?;
    }
    let started = Instant::now();
    let mut photos = 0;
    let mut bytes = 0;
    for path in files::media_files(&source)? {
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if !extension.eq_ignore_ascii_case("jpg") && !extension.eq_ignore_ascii_case("jpeg") {
            continue;
        }
        let size = path.metadata()?.len();
        if size > 16 * 1024 * 1024 {
            continue;
        }
        let destination = folders.gallery.join(path.strip_prefix(&source)?);
        fs::create_dir_all(destination.parent().context("Invalid sample destination")?)?;
        fs::copy(&path, &destination)?;
        let metadata = files::sidecar(&path);
        if metadata.is_file() && !metadata.is_symlink() {
            fs::copy(metadata, files::sidecar(&destination))?;
        }
        bytes += size;
        photos += 1;
        if photos >= 64 {
            break;
        }
    }
    anyhow::ensure!(photos > 0, "No eligible JPEG photos found");
    let copy_seconds = started.elapsed().as_secs_f64();
    let cache = temporary.path().join("cache");
    let started = Instant::now();
    let first = cleanup::analyze(&folders, &cache, &TaskContext::default())?;
    let first_seconds = started.elapsed().as_secs_f64();
    let started = Instant::now();
    let cached = cleanup::analyze(&folders, &cache, &TaskContext::default())?;
    let cached_seconds = started.elapsed().as_secs_f64();
    let mut views = serde_json::Map::new();
    for mode in ["large", "bursts", "similar", "quality"] {
        let started = Instant::now();
        let results = cleanup::results(&folders, mode, 1024 * 1024, 3, 3, 100.0)?;
        views.insert(mode.into(), json!({"seconds":started.elapsed().as_secs_f64(),"groups":results.items.len(),"summary":results.summary}));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "photos":photos,"media_bytes":bytes,"sample_limit":64,"maximum_file_bytes":16*1024*1024,
            "copy_seconds":copy_seconds,"first_analysis_seconds":first_seconds,"first_analysis":first,
            "cached_analysis_seconds":cached_seconds,"cached_analysis":cached,"views":views,
            "source_modified":false,"temporary_sample_removed_on_exit":true
        }))?
    );
    Ok(())
}
