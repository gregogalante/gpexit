use anyhow::Result;
use gpexit_core::{config::Folders, files, service::Engine};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

struct Fixture {
    root: tempfile::TempDir,
    folders: Folders,
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
        Ok(Self {
            root,
            folders: folders.validate()?,
        })
    }
    fn engine(&self) -> Result<Arc<Engine>> {
        Engine::new(
            self.root.path().join("settings/folders.json"),
            self.root.path().join("cache"),
        )
    }
    fn configured(&self) -> Result<Arc<Engine>> {
        let engine = self.engine()?;
        let state = engine.request(
            "POST",
            "/api/settings",
            serde_json::to_value(&self.folders)?,
        )?;
        assert_eq!(state["configured"], true);
        Ok(engine)
    }
    fn photo(&self, relative: &str) -> Result<PathBuf> {
        let path = self.folders.gallery.join(relative);
        fs::create_dir_all(path.parent().unwrap())?;
        fs::write(&path, b"unaltered test photo")?;
        fs::write(
            files::sidecar(&path),
            serde_json::to_vec(&json!({
                "title":path.file_name().unwrap().to_string_lossy(),
                "photoTakenTime":{"timestamp":"1419793427","formatted":"original"}
            }))?,
        )?;
        Ok(path)
    }
}

fn get(engine: &Arc<Engine>, path: &str) -> Result<Value> {
    engine.request("GET", path, Value::Null)
}

#[test]
fn settings_are_required_before_reading_or_modifying_data() -> Result<()> {
    let fixture = Fixture::new()?;
    let engine = fixture.engine()?;
    assert_eq!(get(&engine, "/api/settings")?["configured"], false);
    let state = get(&engine, "/api/state")?;
    assert_eq!(state["configured"], false);
    assert_eq!(state["inputs"], json!([]));
    assert_eq!(state["exports"], json!([]));
    for (method, path, body) in [
        ("GET", "/api/gallery", Value::Null),
        ("GET", "/api/storage", Value::Null),
        ("POST", "/api/gallery/keep", json!({"path":"photo.jpg"})),
        ("POST", "/api/tasks", json!({"action":"export"})),
    ] {
        assert!(
            engine.request(method, path, body).is_err(),
            "{method} {path}"
        );
    }
    assert!(!engine.tasks.running());
    Ok(())
}

#[test]
fn configured_folders_persist_after_reopening_engine() -> Result<()> {
    let fixture = Fixture::new()?;
    let engine = fixture.configured()?;
    assert_eq!(engine.configured_folders()?, fixture.folders);
    drop(engine);
    let reopened = fixture.engine()?;
    assert_eq!(get(&reopened, "/api/settings")?["configured"], true);
    assert_eq!(reopened.configured_folders()?, fixture.folders);
    Ok(())
}

#[test]
fn overlapping_directories_are_rejected_without_persisting_settings() -> Result<()> {
    let fixture = Fixture::new()?;
    let engine = fixture.engine()?;
    let mut folders = fixture.folders.clone();
    folders.exports = folders.gallery.clone();
    assert!(engine
        .request("POST", "/api/settings", serde_json::to_value(folders)?)
        .is_err());
    let nested = fixture.folders.gallery.join("nested");
    fs::create_dir(&nested)?;
    let mut folders = fixture.folders.clone();
    folders.inputs = nested;
    assert!(engine
        .request("POST", "/api/settings", serde_json::to_value(folders)?)
        .is_err());
    assert_eq!(get(&engine, "/api/settings")?["configured"], false);
    assert!(!fixture.root.path().join("settings/folders.json").exists());
    Ok(())
}

#[cfg(unix)]
#[test]
fn aliased_directories_are_rejected_after_symlink_resolution() -> Result<()> {
    let fixture = Fixture::new()?;
    let engine = fixture.engine()?;
    let alias = fixture.root.path().join("gallery-alias");
    std::os::unix::fs::symlink(&fixture.folders.gallery, &alias)?;
    let mut folders = fixture.folders.clone();
    folders.exports = alias;
    assert!(engine
        .request("POST", "/api/settings", serde_json::to_value(folders)?)
        .is_err());
    assert_eq!(get(&engine, "/api/settings")?["configured"], false);
    Ok(())
}

#[test]
fn gallery_lock_prevents_a_second_engine_from_operating() -> Result<()> {
    let fixture = Fixture::new()?;
    let first = fixture.configured()?;
    let second = fixture.engine()?;
    let state = get(&second, "/api/settings")?;
    assert_eq!(state["configured"], false);
    assert!(state["error"].as_str().unwrap().contains("already open"));
    assert!(get(&second, "/api/gallery").is_err());
    drop(second);
    drop(first);
    let reopened = fixture.engine()?;
    assert_eq!(get(&reopened, "/api/settings")?["configured"], true);
    Ok(())
}

#[test]
fn state_lists_case_insensitive_regular_zip_files_only() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::write(fixture.folders.inputs.join("first.ZIP"), b"zip-one")?;
    fs::write(fixture.folders.inputs.join("second.zip"), b"zip-two")?;
    fs::write(fixture.folders.inputs.join("notes.txt"), b"notes")?;
    fs::create_dir(fixture.folders.inputs.join("directory.zip"))?;
    fs::write(fixture.folders.exports.join("2014.ZiP"), b"export")?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.folders.inputs.join("first.ZIP"),
        fixture.folders.inputs.join("linked.zip"),
    )?;
    let engine = fixture.configured()?;
    let state = get(&engine, "/api/state")?;
    let names: Vec<_> = state["inputs"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["first.ZIP", "second.zip"]);
    assert_eq!(state["inputs"][0]["size"], 7);
    assert_eq!(state["exports"][0]["name"], "2014.ZiP");
    Ok(())
}

#[test]
fn gallery_filters_pagination_and_force_refresh_use_consistent_snapshots() -> Result<()> {
    let fixture = Fixture::new()?;
    for index in 0..25 {
        fixture.photo(&format!("2014/06/photos/photo_{index:02}.jpg"))?;
    }
    fixture.photo("2014/12/photos/december.jpg")?;
    fixture.photo("2015/06/photos/next-year.jpg")?;
    let engine = fixture.configured()?;
    let second = get(&engine, "/api/gallery?review=all&year=2014&month=06&page=2")?;
    assert_eq!(second["total"], 25);
    assert_eq!(second["pages"], 2);
    assert_eq!(second["items"].as_array().unwrap().len(), 1);
    assert_eq!(second["items"][0]["path"], "2014/06/photos/photo_24.jpg");
    assert_eq!(
        get(&engine, "/api/gallery?year=2014&month=06&page=99")?["page"],
        2
    );
    assert_eq!(get(&engine, "/api/gallery?year=2015&month=12")?["total"], 0);
    assert_eq!(get(&engine, "/api/gallery?year=2015&month=12")?["pages"], 1);
    let options = get(&engine, "/api/gallery/options")?;
    assert_eq!(options["years"], json!(["2014", "2015"]));
    assert_eq!(options["months"], json!(["06", "12"]));
    assert!(get(&engine, "/api/gallery?review=invalid").is_err());
    fixture.photo("2016/01/photos/new-file.jpg")?;
    assert_eq!(get(&engine, "/api/gallery?review=all")?["total"], 27);
    assert_eq!(
        get(&engine, "/api/gallery?review=all&refresh=1")?["total"],
        28
    );
    assert_eq!(
        get(&engine, "/api/gallery/options")?["years"],
        json!(["2014", "2015", "2016"])
    );
    Ok(())
}

#[test]
fn keep_trash_restore_invalidate_gallery_snapshots() -> Result<()> {
    let fixture = Fixture::new()?;
    let relative = "2014/12/photos/IMG_20140607_154612.jpg";
    let original = fixture.photo(relative)?;
    let original_json = fs::read(files::sidecar(&original))?;
    let engine = fixture.configured()?;
    assert_eq!(get(&engine, "/api/gallery")?["total"], 1);
    engine.request("POST", "/api/gallery/keep", json!({"path":relative}))?;
    assert_eq!(get(&engine, "/api/gallery")?["total"], 0);
    assert_eq!(
        get(&engine, "/api/gallery?review=all")?["items"][0]["kept"],
        true
    );
    let trashed = engine.request("POST", "/api/gallery/trash", json!({"path":relative}))?;
    assert_eq!(get(&engine, "/api/gallery?review=all")?["total"], 0);
    assert_eq!(get(&engine, "/api/trash")?["total"], 1);
    assert!(!original.exists());
    engine.request("POST", "/api/gallery/restore", json!({"id":trashed["id"]}))?;
    assert_eq!(get(&engine, "/api/gallery?review=all")?["total"], 1);
    assert_eq!(get(&engine, "/api/trash")?["total"], 0);
    assert_eq!(fs::read(original)?, b"unaltered test photo");
    assert_eq!(
        fs::read(files::sidecar(&fixture.folders.gallery.join(relative)))?,
        original_json
    );
    Ok(())
}

#[test]
fn correcting_date_invalidates_conflicts_and_gallery_and_updates_json() -> Result<()> {
    let fixture = Fixture::new()?;
    let relative = "2014/12/photos/IMG_20140607_154612.jpg";
    fixture.photo(relative)?;
    let engine = fixture.configured()?;
    assert_eq!(
        get(&engine, "/api/gallery?review=all")?["items"][0]["path"],
        relative
    );
    assert_eq!(get(&engine, "/api/conflicts")?["total"], 1);
    let result = engine.request(
        "POST",
        "/api/correct",
        json!({"path":relative,"date":"2014-06-07"}),
    )?;
    assert_eq!(result["path"], "2014/06/photos/IMG_20140607_154612.jpg");
    assert_eq!(get(&engine, "/api/conflicts")?["total"], 0);
    assert_eq!(
        get(&engine, "/api/gallery?review=all")?["items"][0]["path"],
        result["path"]
    );
    let metadata: Value = serde_json::from_slice(&fs::read(files::sidecar(
        &fixture
            .folders
            .gallery
            .join(result["path"].as_str().unwrap()),
    ))?)?;
    assert_eq!(metadata["gpexitDate"], "2014-06-07");
    assert_eq!(
        metadata["gpexitOriginalPhotoTakenTime"]["timestamp"],
        "1419793427"
    );
    assert_eq!(
        metadata["photoTakenTime"]["formatted"],
        "2014-06-07 19:03:47 UTC"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn report_quotes_commas_and_quotes_in_relative_paths() -> Result<()> {
    let fixture = Fixture::new()?;
    let relative = "2014/12/photos/IMG_20140607_154612,\"note\".jpg";
    fixture.photo(relative)?;
    let engine = fixture.configured()?;
    let destination = fixture.root.path().join("date-conflicts.csv");
    engine.save_report(&destination)?;
    let report = fs::read_to_string(destination)?;
    assert_eq!(report, "file,filename_date,google_date\n\"2014/12/photos/IMG_20140607_154612,\"\"note\"\".jpg\",\"2014-06-07\",\"2014-12-28\"\n");
    Ok(())
}

#[test]
fn export_password_confirmation_rejects_mismatches_without_starting_task() -> Result<()> {
    let fixture = Fixture::new()?;
    let engine = fixture.configured()?;
    for body in [
        json!({"action":"export","password":"secret"}),
        json!({"action":"export","password":"secret","password_confirmation":"wrong"}),
        json!({"action":"export","password":"","password_confirmation":"secret"}),
    ] {
        let error = engine.request("POST", "/api/tasks", body).unwrap_err();
        assert!(error.to_string().contains("non coincidono"));
        assert!(!engine.tasks.running());
    }
    assert_eq!(fs::read_dir(&fixture.folders.exports)?.count(), 0);
    Ok(())
}

#[test]
fn task_routes_import_and_export_real_zip_contents() -> Result<()> {
    let fixture = Fixture::new()?;
    let name = "IMG_20140607_154612.jpg";
    let relative = format!("2014/12/photos/{name}");
    let input = fixture.folders.inputs.join("takeout.zip");
    let mut archive = zip::ZipWriter::new(File::create(input)?);
    let options = zip::write::FileOptions::<()>::default();
    archive.start_file(format!("album/{name}"), options)?;
    archive.write_all(b"imported photo bytes")?;
    archive.start_file(format!("album/{name}.json"), options)?;
    archive.write_all(&serde_json::to_vec(&json!({
        "title":name, "photoTakenTime":{"timestamp":"1419793427"}
    }))?)?;
    archive.finish()?;
    let engine = fixture.configured()?;
    engine.request(
        "POST",
        "/api/tasks",
        json!({"action":"import","inputs":["takeout.zip"],"workers":2}),
    )?;
    engine.tasks.wait();
    assert_eq!(
        engine.tasks.status().status,
        "Completato",
        "{:?}",
        engine.tasks.status().log
    );
    assert_eq!(
        fs::read(fixture.folders.gallery.join(&relative))?,
        b"imported photo bytes"
    );
    assert_eq!(get(&engine, "/api/gallery?review=all")?["total"], 1);
    engine.request(
        "POST",
        "/api/tasks",
        json!({"action":"export","password":"","password_confirmation":""}),
    )?;
    engine.tasks.wait();
    assert_eq!(
        engine.tasks.status().status,
        "Completato",
        "{:?}",
        engine.tasks.status().log
    );
    let mut exported = zip::ZipArchive::new(File::open(fixture.folders.exports.join("2014.zip"))?)?;
    let mut bytes = Vec::new();
    exported.by_name(&relative)?.read_to_end(&mut bytes)?;
    assert_eq!(bytes, b"imported photo bytes");
    assert!(!engine.tasks.running());
    Ok(())
}

#[test]
fn folders_and_mutations_are_blocked_while_a_task_runs() -> Result<()> {
    let fixture = Fixture::new()?;
    let engine = fixture.configured()?;
    engine.tasks.start(|context| {
        while !context.is_cancelled() {
            std::thread::sleep(Duration::from_millis(1));
        }
        context.check_cancelled()?;
        Ok(Value::Null)
    })?;
    let settings_result = engine.request(
        "POST",
        "/api/settings",
        serde_json::to_value(&fixture.folders)?,
    );
    let task_result = engine.request("POST", "/api/tasks", json!({"action":"export"}));
    let gallery_result = get(&engine, "/api/gallery");
    let state_result = get(&engine, "/api/state");
    engine.request("POST", "/api/cancel", Value::Null)?;
    engine.tasks.wait();
    assert!(settings_result.is_err());
    assert!(task_result.is_err());
    assert!(gallery_result.is_err());
    assert_eq!(state_result?["task"]["running"], true);
    assert!(!engine.tasks.running());
    assert!(engine.tasks.status().status.starts_with("Interrotto"));
    assert_eq!(engine.configured_folders()?, fixture.folders);
    Ok(())
}

#[test]
fn invalid_saved_settings_surface_error_without_accessing_data() -> Result<()> {
    let fixture = Fixture::new()?;
    fs::create_dir(fixture.root.path().join("settings"))?;
    fs::write(
        fixture.root.path().join("settings/folders.json"),
        b"invalid json",
    )?;
    let engine = fixture.engine()?;
    let state = get(&engine, "/api/state")?;
    assert_eq!(state["configured"], false);
    assert!(state["error"]
        .as_str()
        .unwrap()
        .contains("Cannot read settings"));
    assert!(get(&engine, "/api/gallery").is_err());
    Ok(())
}

#[test]
fn failed_settings_write_keeps_previous_folders_and_gallery_lock() -> Result<()> {
    let first = Fixture::new()?;
    let second = Fixture::new()?;
    let engine = first.configured()?;
    let settings_path = first.root.path().join("settings/folders.json");
    fs::remove_file(&settings_path)?;
    fs::create_dir(&settings_path)?;
    assert!(engine
        .request(
            "POST",
            "/api/settings",
            serde_json::to_value(&second.folders)?
        )
        .is_err());
    assert_eq!(engine.configured_folders()?, first.folders);
    let contender = Engine::new(
        second.root.path().join("contender.json"),
        second.root.path().join("contender-cache"),
    )?;
    assert!(contender
        .request(
            "POST",
            "/api/settings",
            serde_json::to_value(&first.folders)?
        )
        .is_err());
    assert!(contender
        .request(
            "POST",
            "/api/settings",
            serde_json::to_value(&second.folders)?
        )
        .is_ok());
    Ok(())
}
