use anyhow::{Context, Result};
use gpexit_core::{config::Folders, files, service::Engine};
use image::{Rgb, RgbImage};
use serde_json::{json, Value};
use std::{
    fs::{self, File, FileTimes},
    path::PathBuf,
    sync::{mpsc, Arc},
    time::Duration,
};

struct Fixture {
    engine: Arc<Engine>,
    folders: Folders,
    _root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Result<Self> {
        let root = tempfile::tempdir()?;
        let folders = Folders {
            inputs: root.path().join("inputs"),
            gallery: root.path().join("gallery"),
            exports: root.path().join("exports"),
        };
        for path in [&folders.inputs, &folders.gallery, &folders.exports] {
            fs::create_dir(path)?;
        }
        let engine = Engine::new(root.path().join("settings.json"), root.path().join("cache"))?;
        engine.request("POST", "/api/settings", serde_json::to_value(&folders)?)?;
        Ok(Self {
            engine,
            folders: folders.validate()?,
            _root: root,
        })
    }

    fn media(&self, name: &str, content: &[u8]) -> Result<PathBuf> {
        let path = self.folders.gallery.join("2020/01/photos").join(name);
        fs::create_dir_all(path.parent().context("Invalid fixture path")?)?;
        fs::write(&path, content)?;
        Ok(path)
    }

    fn image(&self, name: &str, textured: bool) -> Result<PathBuf> {
        let path = self.media(name, b"")?;
        let image = RgbImage::from_fn(144, 128, |x, y| {
            let level = if textured && ((x / 16 + y / 16) % 2 == 0) {
                230
            } else {
                20
            };
            Rgb([level, level, level])
        });
        image.save(&path)?;
        Ok(path)
    }

    fn get(&self, path: &str) -> Result<Value> {
        self.engine.request("GET", path, Value::Null)
    }

    fn post(&self, path: &str, body: Value) -> Result<Value> {
        self.engine.request("POST", path, body)
    }

    fn selections(&self) -> Result<Vec<Value>> {
        let response = self.get("/api/cleanup?mode=large&min_bytes=0&refresh=1")?;
        Ok(response["items"]
            .as_array()
            .context("Missing cleanup groups")?
            .iter()
            .flat_map(|group| group["items"].as_array().unwrap().iter())
            .map(|item| json!({"path":item["path"], "fingerprint":item["fingerprint"]}))
            .collect())
    }

    fn wait_success(&self) -> Result<Value> {
        self.engine.tasks.wait();
        let status = self.engine.tasks.status();
        assert_eq!(status.status, "Completato", "{:?}", status.log);
        let serialized = serde_json::to_value(status)?;
        serialized
            .get("result")
            .cloned()
            .filter(|value| !value.is_null())
            .context("Task result must remain available to the interface")
    }
}

#[test]
fn large_filter_orders_pages_and_refreshes_cached_snapshots() -> Result<()> {
    let fixture = Fixture::new()?;
    for index in 0..26 {
        fixture.media(&format!("photo_{index:02}.jpg"), &vec![b'x'; index + 1])?;
    }
    let first = fixture.get("/api/cleanup?mode=large&min_bytes=0")?;
    assert_eq!(first["total"], 26);
    assert_eq!(first["pages"], 2);
    assert_eq!(first["items"].as_array().unwrap().len(), 24);
    assert_eq!(first["items"][0]["size"], 26);
    assert_eq!(
        first["items"][0]["items"][0]["path"],
        "2020/01/photos/photo_25.jpg"
    );
    let second = fixture.get("/api/cleanup?mode=large&min_bytes=0&page=2")?;
    assert_eq!(second["items"].as_array().unwrap().len(), 2);
    assert_eq!(second["items"][0]["size"], 2);
    assert_eq!(second["items"][1]["size"], 1);
    let filtered = fixture.get("/api/cleanup?mode=large&min_bytes=25")?;
    assert_eq!(filtered["total"], 2);
    fixture.media("new.jpg", &[b'z'; 100])?;
    assert_eq!(
        fixture.get("/api/cleanup?mode=large&min_bytes=0")?["total"],
        26
    );
    let refreshed = fixture.get("/api/cleanup?mode=large&min_bytes=0&refresh=1")?;
    assert_eq!(refreshed["total"], 27);
    assert_eq!(refreshed["items"][0]["size"], 100);
    assert_eq!(refreshed["summary"]["eligible"], 27);
    assert_eq!(refreshed["summary"]["groups"], 27);
    assert_eq!(
        fixture.get("/api/cleanup?mode=large&min_bytes=0&page=999")?["page"],
        2
    );
    Ok(())
}

#[test]
fn keep_removes_suggestions_and_forged_selection_cannot_trash_kept_file() -> Result<()> {
    let fixture = Fixture::new()?;
    let kept = fixture.media("keep.jpg", b"keep this image")?;
    let other = fixture.media("other.jpg", b"another image")?;
    let selections = fixture.selections()?;
    fixture.post(
        "/api/gallery/keep",
        json!({"path":"2020/01/photos/keep.jpg"}),
    )?;
    let response = fixture.get("/api/cleanup?mode=large&min_bytes=0")?;
    assert_eq!(response["total"], 1);
    assert_eq!(response["summary"]["kept_excluded"], 1);
    assert_eq!(
        response["items"][0]["items"][0]["path"],
        "2020/01/photos/other.jpg"
    );
    assert!(fixture
        .post("/api/cleanup/trash", json!({"items":selections}))
        .is_err());
    assert!(kept.exists());
    assert!(other.exists());
    assert!(!fixture.engine.tasks.running());
    assert_eq!(fixture.get("/api/trash")?["total"], 0);
    Ok(())
}

#[test]
fn stale_same_length_file_rejects_entire_batch_without_partial_moves() -> Result<()> {
    let fixture = Fixture::new()?;
    let first = fixture.media("a.jpg", b"first original bytes")?;
    let second = fixture.media("b.jpg", b"other original bytes")?;
    let selected = fixture.selections()?;
    let before = fs::metadata(&second)?;
    fs::write(&second, vec![b'z'; before.len() as usize])?;
    File::options()
        .write(true)
        .open(&second)?
        .set_times(FileTimes::new().set_modified(before.modified()? + Duration::from_secs(2)))?;
    assert_eq!(fs::metadata(&second)?.len(), before.len());
    assert!(fixture
        .post("/api/cleanup/trash", json!({"items":selected}))
        .is_err());
    assert_eq!(fs::read(first)?, b"first original bytes");
    assert!(second.exists());
    assert_eq!(fixture.get("/api/trash")?["total"], 0);
    assert!(!fixture.engine.tasks.running());
    Ok(())
}

#[test]
fn invalid_bulk_selections_are_rejected_before_any_move() -> Result<()> {
    let fixture = Fixture::new()?;
    let photo = fixture.media("photo.jpg", b"preserved")?;
    let selected = fixture.selections()?;
    let valid = selected[0].clone();
    let invalid = [
        json!({"items":[]}),
        json!({"items":[valid.clone(),valid.clone()]}),
        json!({"items":[{"path":"../outside.jpg","fingerprint":"forged"}]}),
        json!({"items":[{"path":"2020/01/photos/photo.jpg","fingerprint":"forged"}]}),
        json!({"items":[{"path":"2020/01/photos/photo.jpg","fingerprint":42}]}),
        json!({"items":vec![valid;501]}),
        json!({"items":"invalid"}),
    ];
    for body in invalid {
        assert!(
            fixture.post("/api/cleanup/trash", body.clone()).is_err(),
            "{body}"
        );
        assert_eq!(fs::read(&photo)?, b"preserved");
        assert!(!fixture.engine.tasks.running());
        assert_eq!(fixture.get("/api/trash")?["total"], 0);
    }
    Ok(())
}

#[test]
fn valid_batch_exposes_task_result_and_restores_original_media_and_json() -> Result<()> {
    let fixture = Fixture::new()?;
    let first = fixture.media("a.jpg", b"first photo bytes")?;
    let second = fixture.media("b.jpg", b"second photo bytes")?;
    let metadata = b"{\"title\":\"a.jpg\",\"description\":\"original metadata\"}";
    fs::write(files::sidecar(&first), metadata)?;
    let selected = fixture.selections()?;
    let response = fixture.post("/api/cleanup/trash", json!({"items":selected}))?;
    assert_eq!(response["started"], true);
    let result = fixture.wait_success()?;
    assert_eq!(result["cancelled"], false);
    assert_eq!(result["failed"], json!([]));
    assert_eq!(result["moved"].as_array().unwrap().len(), 2);
    assert!(!first.exists());
    assert!(!second.exists());
    assert!(!files::sidecar(&first).exists());
    assert_eq!(
        fixture.get("/api/cleanup?mode=large&min_bytes=0")?["total"],
        0
    );
    assert_eq!(fixture.get("/api/trash")?["total"], 2);
    for item in result["moved"].as_array().unwrap() {
        let restored = fixture.post("/api/gallery/restore", json!({"id":item["id"]}))?;
        assert_eq!(restored["path"], item["path"]);
    }
    assert_eq!(fs::read(&first)?, b"first photo bytes");
    assert_eq!(fs::read(&second)?, b"second photo bytes");
    assert_eq!(fs::read(files::sidecar(&first))?, metadata);
    assert_eq!(fixture.get("/api/trash")?["total"], 0);
    Ok(())
}

#[test]
fn active_task_blocks_cleanup_and_gallery_mutations() -> Result<()> {
    let fixture = Fixture::new()?;
    let photo = fixture.media("photo.jpg", b"original")?;
    let selected = fixture.selections()?;
    let (release, wait) = mpsc::channel();
    fixture.engine.tasks.start(move |_| {
        wait.recv_timeout(Duration::from_secs(5))?;
        Ok(json!({"released":true}))
    })?;
    for (method, path, body) in [
        ("GET", "/api/cleanup?mode=large&min_bytes=0", Value::Null),
        ("POST", "/api/cleanup/trash", json!({"items":selected})),
        (
            "POST",
            "/api/gallery/keep",
            json!({"path":"2020/01/photos/photo.jpg"}),
        ),
        ("POST", "/api/tasks", json!({"action":"analyze"})),
    ] {
        assert!(
            fixture.engine.request(method, path, body).is_err(),
            "{method} {path}"
        );
    }
    release.send(())?;
    fixture.engine.tasks.wait();
    assert_eq!(fs::read(photo)?, b"original");
    assert_eq!(fixture.get("/api/trash")?["total"], 0);
    Ok(())
}

#[test]
fn analysis_task_reports_native_image_progress_and_persistent_cache_hits() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.image("checker_a.png", true)?;
    fixture.image("checker_b.png", true)?;
    fixture.image("flat.png", false)?;
    fixture.image("keep.png", true)?;
    fixture.post(
        "/api/gallery/keep",
        json!({"path":"2020/01/photos/keep.png"}),
    )?;
    let before = fixture.get("/api/cleanup?mode=similar")?;
    assert_eq!(before["summary"]["pending"], 3);
    fixture.post("/api/tasks", json!({"action":"analyze"}))?;
    let first = fixture.wait_success()?;
    assert_eq!(first["eligible"], 3);
    assert_eq!(first["analyzed"], 3);
    assert_eq!(first["unsupported"], 0);
    assert_eq!(first["cache_hits"], 0);
    assert_eq!(first["kept_excluded"], 1);
    let status = fixture.engine.tasks.status();
    assert_eq!(status.done, 3);
    assert_eq!(status.total, 3);
    let after = fixture.get("/api/cleanup?mode=similar&distance=0")?;
    assert_eq!(after["summary"]["pending"], 0);
    assert_eq!(after["summary"]["analyzed"], 3);
    assert_eq!(after["total"], 1);
    assert_eq!(after["items"][0]["items"].as_array().unwrap().len(), 2);
    let quality = fixture.get("/api/cleanup?mode=quality&quality_threshold=0")?;
    assert!(quality["items"]
        .as_array()
        .unwrap()
        .iter()
        .any(|group| { group["items"][0]["path"] == "2020/01/photos/flat.png" }));
    assert!(fixture
        .folders
        .gallery
        .join(".gpexit-cleanup.sqlite3")
        .exists());
    fixture.post("/api/tasks", json!({"action":"analyze"}))?;
    let cached = fixture.wait_success()?;
    assert_eq!(cached["cache_hits"], 3);
    assert_eq!(cached["analyzed"], 3);
    let Fixture {
        engine,
        _root: root,
        ..
    } = fixture;
    drop(engine);
    let reopened = Engine::new(root.path().join("settings.json"), root.path().join("cache"))?;
    reopened.request("POST", "/api/tasks", json!({"action":"analyze"}))?;
    reopened.tasks.wait();
    let status = serde_json::to_value(reopened.tasks.status())?;
    assert_eq!(status["status"], "Completato");
    assert_eq!(status["result"]["cache_hits"], 3);
    Ok(())
}

#[test]
fn cleanup_rejects_unknown_and_out_of_range_filters() -> Result<()> {
    let fixture = Fixture::new()?;
    for path in [
        "/api/cleanup?mode=unknown",
        "/api/cleanup?min_bytes=-1",
        "/api/cleanup?gap_seconds=0",
        "/api/cleanup?gap_seconds=61",
        "/api/cleanup?distance=11",
        "/api/cleanup?quality_threshold=NaN",
        "/api/cleanup?quality_threshold=-1",
        "/api/cleanup?quality_threshold=5001",
        "/api/cleanup?page=not-a-number",
    ] {
        assert!(fixture.get(path).is_err(), "{path}");
    }
    assert!(!fixture.engine.tasks.running());
    Ok(())
}
