use anyhow::Result;
use gpexit_core::{
    cleanup,
    config::Folders,
    files, gallery,
    task::{TaskContext, TaskManager},
};
use image::{GrayImage, Luma};
use serde_json::{json, Value};
use std::{
    fs,
    path::PathBuf,
    time::{Duration, Instant},
};

struct Fixture {
    _root: tempfile::TempDir,
    folders: Folders,
    cache: PathBuf,
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
        let cache = root.path().join("cache");
        Ok(Self {
            _root: root,
            folders: folders.validate()?,
            cache,
        })
    }
    fn path(&self, name: &str) -> Result<PathBuf> {
        let path = self.folders.gallery.join("2020/01/photos").join(name);
        fs::create_dir_all(path.parent().unwrap())?;
        Ok(path)
    }
    fn image(&self, name: &str, image: &GrayImage) -> Result<String> {
        let path = self.path(name)?;
        image.save(&path)?;
        Ok(path
            .strip_prefix(&self.folders.gallery)?
            .to_string_lossy()
            .replace('\\', "/"))
    }
    fn raw(&self, name: &str, bytes: &[u8]) -> Result<String> {
        let path = self.path(name)?;
        fs::write(&path, bytes)?;
        Ok(path
            .strip_prefix(&self.folders.gallery)?
            .to_string_lossy()
            .replace('\\', "/"))
    }
    fn analyze(&self) -> Result<Value> {
        cleanup::analyze(&self.folders, &self.cache, &TaskContext::default())
    }
    fn results(&self, mode: &str) -> Result<cleanup::CleanupResults> {
        cleanup::results(&self.folders, mode, 0, 3, 3, f64::MAX)
    }
}

fn checker() -> GrayImage {
    GrayImage::from_fn(256, 256, |x, y| {
        Luma([if (x / 32 + y / 32) % 2 == 0 { 30 } else { 220 }])
    })
}

#[test]
fn sharpness_orders_blurred_image_first_and_suggestions_never_modify_originals() -> Result<()> {
    let fixture = Fixture::new()?;
    let sharp = checker();
    let blurred = image::imageops::blur(&sharp, 6.0);
    let sharp_path = fixture.image("sharp.png", &sharp)?;
    let blur_path = fixture.image("blurred.png", &blurred)?;
    let before_sharp = fs::read(fixture.folders.gallery.join(&sharp_path))?;
    let before_blurred = fs::read(fixture.folders.gallery.join(&blur_path))?;
    let analyzed = fixture.analyze()?;
    assert_eq!(analyzed["analyzed"], 2);
    let results = fixture.results("quality")?;
    assert_eq!(results.items.len(), 2);
    assert_eq!(results.items[0]["items"][0]["path"], blur_path);
    let blur_score = results.items[0]["items"][0]["score"].as_f64().unwrap();
    let sharp_score = results.items[1]["items"][0]["score"].as_f64().unwrap();
    assert!(
        sharp_score > blur_score * 2.0,
        "sharp={sharp_score}; blurred={blur_score}"
    );
    let thresholded = cleanup::results(
        &fixture.folders,
        "quality",
        0,
        3,
        3,
        (blur_score + sharp_score) / 2.0,
    )?;
    assert_eq!(thresholded.items.len(), 1);
    assert_eq!(
        fs::read(fixture.folders.gallery.join(sharp_path))?,
        before_sharp
    );
    assert_eq!(
        fs::read(fixture.folders.gallery.join(blur_path))?,
        before_blurred
    );
    Ok(())
}

#[test]
fn similar_images_group_by_perceptual_hash_and_flat_images_are_excluded() -> Result<()> {
    let fixture = Fixture::new()?;
    let source = checker();
    fixture.image("first.png", &source)?;
    fixture.image("duplicate.png", &source)?;
    let brighter = GrayImage::from_fn(256, 256, |x, y| {
        Luma([source.get_pixel(x, y)[0].saturating_add(10)])
    });
    fixture.image("brighter.png", &brighter)?;
    fixture.image(
        "flat-dark.png",
        &GrayImage::from_pixel(256, 256, Luma([30])),
    )?;
    fixture.image(
        "flat-light.png",
        &GrayImage::from_pixel(256, 256, Luma([220])),
    )?;
    assert_eq!(fixture.results("similar")?.summary["pending"], 5);
    assert!(fixture.results("similar")?.items.is_empty());
    fixture.analyze()?;
    let results = fixture.results("similar")?;
    assert_eq!(results.summary["analyzed"], 5);
    assert_eq!(results.items.len(), 1);
    let members = results.items[0]["items"].as_array().unwrap();
    assert_eq!(members.len(), 3);
    assert!(members
        .iter()
        .all(|member| !member["path"].as_str().unwrap().contains("flat-")));
    for member in members {
        assert!(member["score"].as_f64().unwrap() <= 3.0);
    }
    Ok(())
}

#[test]
fn unchanged_analysis_and_decoder_errors_are_cached_and_changed_files_become_pending() -> Result<()>
{
    let fixture = Fixture::new()?;
    let first = fixture.image("first.png", &checker())?;
    fixture.image("second.png", &checker())?;
    fixture.raw("unsupported.heic", b"not a supported image")?;
    let initial = fixture.analyze()?;
    assert_eq!(initial["eligible"], 3);
    assert_eq!(initial["analyzed"], 2);
    assert_eq!(initial["unsupported"], 1);
    assert_eq!(initial["cache_hits"], 0);
    let repeated = fixture.analyze()?;
    assert_eq!(repeated["cache_hits"], 3);
    assert_eq!(repeated["unsupported"], 1);
    let before = cleanup::fingerprint(&fixture.folders.gallery.join(&first))?;
    GrayImage::from_pixel(31, 33, Luma([5])).save(fixture.folders.gallery.join(&first))?;
    assert_ne!(
        cleanup::fingerprint(&fixture.folders.gallery.join(&first))?,
        before
    );
    let stale = fixture.results("quality")?;
    assert_eq!(stale.summary["pending"], 1);
    assert_eq!(stale.summary["unsupported"], 1);
    assert_eq!(stale.items.len(), 1);
    assert_eq!(fixture.analyze()?["cache_hits"], 2);
    let connection =
        rusqlite::Connection::open(fixture.folders.gallery.join(".gpexit-cleanup.sqlite3"))?;
    connection.execute("UPDATE analysis SET version = 0 WHERE path = ?1", [&first])?;
    drop(connection);
    assert_eq!(fixture.results("quality")?.summary["pending"], 1);
    assert_eq!(fixture.analyze()?["cache_hits"], 2);
    Ok(())
}

#[test]
fn all_cleanup_views_exclude_kept_media_and_include_current_fingerprints() -> Result<()> {
    let fixture = Fixture::new()?;
    let kept = fixture.image("IMG_20200101_120000.png", &checker())?;
    let remaining = fixture.image("IMG_20200101_120001.png", &checker())?;
    gallery::keep_file(&fixture.folders, &kept)?;
    let analysis = fixture.analyze()?;
    assert_eq!(analysis["eligible"], 1);
    assert_eq!(analysis["kept_excluded"], 1);
    for mode in ["large", "bursts", "similar", "quality"] {
        let results = fixture.results(mode)?;
        assert_eq!(results.summary["eligible"], 1, "{mode}");
        assert_eq!(results.summary["kept_excluded"], 1, "{mode}");
        for group in results.items {
            for member in group["items"].as_array().unwrap() {
                assert_eq!(member["path"], remaining);
                assert_eq!(
                    member["fingerprint"],
                    cleanup::fingerprint(&fixture.folders.gallery.join(&remaining))?
                );
            }
        }
    }
    Ok(())
}

#[test]
fn large_files_include_videos_in_descending_size_and_respect_threshold() -> Result<()> {
    let fixture = Fixture::new()?;
    let small = fixture.raw("small.jpg", &[1; 4])?;
    let large = fixture.raw("large.mp4", &[2; 40])?;
    let middle = fixture.raw("middle.png", &[3; 20])?;
    let results = cleanup::results(&fixture.folders, "large", 10, 3, 3, 100.0)?;
    assert_eq!(results.items.len(), 2);
    assert_eq!(results.items[0]["items"][0]["path"], large);
    assert_eq!(results.items[0]["items"][0]["kind"], "videos");
    assert_eq!(results.items[1]["items"][0]["path"], middle);
    assert!(results
        .items
        .iter()
        .all(|group| group["items"][0]["path"] != small));
    assert_eq!(results.items[0]["size"], 40);
    Ok(())
}

#[test]
fn burst_groups_are_bounded_by_count_and_time_span() -> Result<()> {
    let fixture = Fixture::new()?;
    for second in 0..14 {
        fixture.raw(&format!("IMG_20200101_1200{second:02}.jpg"), b"photo")?;
    }
    let results = fixture.results("bursts")?;
    assert_eq!(results.items.len(), 2);
    assert_eq!(results.items[0]["items"].as_array().unwrap().len(), 12);
    assert_eq!(results.items[1]["items"].as_array().unwrap().len(), 2);
    let second_fixture = Fixture::new()?;
    for second in [0, 10, 20, 30, 40, 50] {
        second_fixture.raw(&format!("IMG_20200101_1200{second:02}.jpg"), b"photo")?;
    }
    for second in [0, 10, 20] {
        second_fixture.raw(&format!("IMG_20200101_1201{second:02}.jpg"), b"photo")?;
    }
    let results = cleanup::results(&second_fixture.folders, "bursts", 0, 10, 3, 100.0)?;
    assert_eq!(results.items.len(), 2);
    assert_eq!(results.items[0]["items"].as_array().unwrap().len(), 7);
    assert_eq!(results.items[1]["items"].as_array().unwrap().len(), 2);
    Ok(())
}

#[test]
fn burst_timestamps_reject_invalid_names_and_do_not_mix_local_and_utc_sources() -> Result<()> {
    let fixture = Fixture::new()?;
    fixture.raw("IMG_20200101_120000.jpg", b"camera local")?;
    fixture.raw("IMG_20200101_256199.jpg", b"invalid time")?;
    fixture.raw("IMG_20200230_120001.jpg", b"invalid calendar date")?;
    fixture.raw("unrelated.jpg", b"no timestamp")?;
    for (name, timestamp) in [
        ("google-one.jpg", "1577880001"),
        ("google-two.jpg", "1577880002"),
    ] {
        let relative = fixture.raw(name, b"Google UTC")?;
        fs::write(
            files::sidecar(&fixture.folders.gallery.join(relative)),
            serde_json::to_vec(&json!({"photoTakenTime":{"timestamp":timestamp}}))?,
        )?;
    }
    let relative = fixture.raw("invalid-google.jpg", b"bad metadata")?;
    fs::write(
        files::sidecar(&fixture.folders.gallery.join(relative)),
        br#"{"photoTakenTime":{"timestamp":"NaN"}}"#,
    )?;
    let result = fixture.results("bursts")?;
    assert_eq!(result.items.len(), 1);
    let members = result.items[0]["items"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert!(members
        .iter()
        .all(|member| member["path"].as_str().unwrap().contains("google-")));
    Ok(())
}

#[test]
fn cancellation_preserves_completed_rows_and_resume_reuses_them() -> Result<()> {
    let fixture = Fixture::new()?;
    for index in 0..20 {
        fixture.image(&format!("photo-{index:02}.png"), &checker())?;
    }
    let tasks = TaskManager::default();
    let folders = fixture.folders.clone();
    let cache = fixture.cache.clone();
    tasks.start(move |context| cleanup::analyze(&folders, &cache, &context))?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while tasks.running() && tasks.status().done == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1));
    }
    tasks.cancel();
    tasks.wait();
    assert!(
        tasks.status().status.starts_with("Interrotto"),
        "{}",
        tasks.status().status
    );
    let connection =
        rusqlite::Connection::open(fixture.folders.gallery.join(".gpexit-cleanup.sqlite3"))?;
    let completed: i64 = connection.query_row(
        "SELECT count(*) FROM analysis WHERE version = 1 AND error IS NULL AND hash IS NOT NULL",
        [],
        |row| row.get(0),
    )?;
    assert!((1..20).contains(&completed), "completed={completed}");
    drop(connection);
    let resumed = fixture.analyze()?;
    assert_eq!(resumed["analyzed"], 20);
    assert_eq!(resumed["cache_hits"], completed);
    Ok(())
}

#[test]
fn invalid_cleanup_parameters_are_rejected() -> Result<()> {
    let fixture = Fixture::new()?;
    assert!(cleanup::results(&fixture.folders, "unknown", 0, 1, 1, 1.0).is_err());
    assert!(cleanup::results(&fixture.folders, "similar", 0, 1, 11, 1.0).is_err());
    assert!(cleanup::results(&fixture.folders, "quality", 0, 1, 1, f64::NAN).is_err());
    assert!(cleanup::results(&fixture.folders, "quality", 0, 1, 1, -1.0).is_err());
    Ok(())
}

#[test]
fn burst_calendar_respects_portable_manual_date_corrections() -> Result<()> {
    let fixture = Fixture::new()?;
    let one = fixture.raw("IMG_20200101_120000.jpg", b"one")?;
    let two = fixture.raw("IMG_20200101_120003.jpg", b"two")?;
    fixture.raw("IMG_20210101_120002.jpg", b"three")?;
    for relative in [one, two] {
        fs::write(
            files::sidecar(&fixture.folders.gallery.join(relative)),
            serde_json::to_vec(&json!({"gpexitDate":"2021-01-01"}))?,
        )?;
    }
    let result = fixture.results("bursts")?;
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0]["items"].as_array().unwrap().len(), 3);
    Ok(())
}
