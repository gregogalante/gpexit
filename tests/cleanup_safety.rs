use anyhow::{bail, Result};
use gpexit_core::files;
use std::fs;

#[test]
fn checked_hash_stops_between_chunks_without_modifying_source() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("source.bin");
    fs::write(&path, vec![19u8; 2 * 1024 * 1024])?;
    let expected = files::file_hash(&path)?;
    let mut stopped_at = 0;
    assert!(files::file_hash_checked(&path, |done, _| {
        stopped_at = done;
        if done >= 1024 * 1024 {
            bail!("cancelled");
        }
        Ok(())
    })
    .is_err());
    assert_eq!(stopped_at, 1024 * 1024);
    assert_eq!(files::file_hash(&path)?, expected);
    Ok(())
}

#[cfg(unix)]
#[test]
fn cancelling_large_video_cleanup_keeps_unmoved_video() -> Result<()> {
    use gpexit_core::{config::Folders, service::Engine};
    use serde_json::{json, Value};
    use std::{
        thread,
        time::{Duration, Instant},
    };
    let root = tempfile::tempdir()?;
    let folders = Folders {
        inputs: root.path().join("inputs"),
        gallery: root.path().join("gallery"),
        exports: root.path().join("exports"),
    };
    for path in [&folders.inputs, &folders.gallery, &folders.exports] {
        fs::create_dir(path)?;
    }
    let video = folders.gallery.join("large.mp4");
    fs::File::create(&video)?.set_len(4 * 1024 * 1024 * 1024)?;
    let engine = Engine::new(root.path().join("settings.json"), root.path().join("cache"))?;
    engine.request("POST", "/api/settings", serde_json::to_value(&folders)?)?;
    let found = engine.request("GET", "/api/cleanup?mode=large&min_bytes=1", Value::Null)?;
    let item = &found["items"][0]["items"][0];
    engine.request(
        "POST",
        "/api/cleanup/trash",
        json!({"items":[{"path":item["path"],"fingerprint":item["fingerprint"]}]}),
    )?;
    let until = Instant::now() + Duration::from_secs(5);
    while engine.tasks.running()
        && !engine
            .tasks
            .status()
            .log
            .iter()
            .any(|line| line.starts_with("Trash:"))
        && Instant::now() < until
    {
        thread::sleep(Duration::from_millis(1));
    }
    engine.tasks.cancel();
    engine.tasks.wait();
    let result = engine.tasks.status().result.unwrap();
    assert_eq!(result["cancelled"], true);
    assert!(result["moved"].as_array().unwrap().is_empty());
    assert_eq!(fs::metadata(video)?.len(), 4 * 1024 * 1024 * 1024);
    assert!(!folders.trash().exists());
    Ok(())
}
