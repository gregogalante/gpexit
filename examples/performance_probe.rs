//! Read-only timing of a real archive. No media or JSON is changed.
use anyhow::{Context, Result};
use gpexit_core::{config::Folders, gallery};
use serde_json::{json, Value};
use std::{env, path::PathBuf, time::Instant};
fn main() -> Result<()> {
    let root = PathBuf::from(
        env::args()
            .nth(1)
            .context("Usage: performance_probe ARCHIVE_ROOT")?,
    );
    let folders = Folders {
        inputs: root.join("inputs"),
        gallery: root.join("gallery"),
        exports: root.join("exports"),
    };
    let mut runs = Vec::new();
    for _ in 0..3 {
        let start = Instant::now();
        let items = gallery::list_gallery(&folders)?;
        let listing = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let conflicts = gallery::conflicts(&folders)?;
        let scan = start.elapsed().as_secs_f64();
        let start = Instant::now();
        let storage = gallery::storage_stats(&folders)?;
        runs.push(json!({"media":items.len(),"conflicts":conflicts.len(),"gallery_seconds":listing,"dates_seconds":scan,"storage_seconds":start.elapsed().as_secs_f64(),"gallery_bytes":storage["gallery"]["bytes"]}));
    }
    let value: Value = json!({"runs":runs});
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
