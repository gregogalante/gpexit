use anyhow::Result;
use gpexit_core::{
    archive::{export_archives, import_archives},
    config::Folders,
    files, gallery,
    task::{TaskContext, TaskManager},
};
use serde_json::{json, Value};
use std::{
    fs::{self, File},
    io::Write,
    path::PathBuf,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use zip::{write::FileOptions, AesMode, CompressionMethod, ZipArchive, ZipWriter};

struct Fixture {
    _root: TempDir,
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
            _root: root,
            folders,
        })
    }
    fn zip(&self, name: &str, entries: &[(&str, &[u8])], password: &str) -> Result<PathBuf> {
        let path = self.folders.inputs.join(name);
        let mut archive = ZipWriter::new(File::create(&path)?);
        let mut options =
            FileOptions::<()>::default().compression_method(CompressionMethod::Deflated);
        if !password.is_empty() {
            options = options.with_aes_encryption(AesMode::Aes256, password);
        }
        for (name, content) in entries {
            archive.start_file(*name, options)?;
            archive.write_all(content)?;
        }
        archive.finish()?;
        Ok(path)
    }
    fn import(&self, paths: &[PathBuf], password: &str, workers: usize) -> Result<Value> {
        import_archives(
            &self.folders,
            paths,
            password,
            workers,
            &TaskContext::default(),
        )
    }
}

fn google(title: &str, timestamp: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"title": title, "photoTakenTime": {"timestamp": timestamp}}))
        .unwrap()
}

#[test]
fn cross_zip_metadata_direct_sidecar_priority_and_resume() -> Result<()> {
    let fixture = Fixture::new()?;
    let title = google("IMG_20140607_154612.jpg", "1419793427");
    let direct = google("unrelated title.jpg", "1546300800");
    let first = fixture.zip(
        "photos.zip",
        &[("album/IMG_20140607_154612.jpg", b"photo")],
        "",
    )?;
    let second = fixture.zip(
        "metadata.zip",
        &[
            ("album/photo.supplemental-metadata.json", &title),
            ("album/IMG_20140607_154612.jpg.json", &direct),
        ],
        "",
    )?;
    let counts = fixture.import(&[first.clone(), second.clone()], "", 4)?;
    assert_eq!(counts["imported"], 1);
    let destination = fixture
        .folders
        .gallery
        .join("2019/01/photos/IMG_20140607_154612.jpg");
    assert_eq!(fs::read(&destination)?, b"photo");
    assert_eq!(fs::read(files::sidecar(&destination))?, direct);
    let resumed = fixture.import(&[first, second], "", 2)?;
    assert_eq!(resumed["resumed"], 1);
    assert_eq!(resumed["imported"], 0);
    Ok(())
}

#[test]
fn duplicate_content_collision_and_missing_date_upgrade() -> Result<()> {
    let fixture = Fixture::new()?;
    let first = fixture.zip(
        "first.zip",
        &[
            ("album/no-date.jpg", b"same"),
            ("album/IMG_20200102.jpg", b"first"),
        ],
        "",
    )?;
    fixture.import(&[first], "", 1)?;
    let second = fixture.zip(
        "second.zip",
        &[
            ("album/IMG_20200203.jpg", b"same"),
            ("album/IMG_20200102.jpg", b"other"),
        ],
        "",
    )?;
    let counts = fixture.import(&[second], "", 4)?;
    assert_eq!(counts["duplicates"], 1);
    assert_eq!(counts["imported"], 1);
    assert_eq!(
        fs::read(fixture.folders.gallery.join("2020/02/photos/no-date.jpg"))?,
        b"same"
    );
    assert!(!fixture
        .folders
        .gallery
        .join("Unknown year/Unknown month/photos/no-date.jpg")
        .exists());
    let names: Vec<_> = fs::read_dir(fixture.folders.gallery.join("2020/01/photos"))?
        .collect::<std::io::Result<Vec<_>>>()?;
    assert_eq!(names.len(), 2);
    assert!(names.iter().any(|entry| entry
        .file_name()
        .to_string_lossy()
        .starts_with("IMG_20200102_")));
    Ok(())
}

#[test]
fn output_changed_after_checkpoint_is_validated_by_content() -> Result<()> {
    let fixture = Fixture::new()?;
    let source = fixture.zip("source.zip", &[("album/IMG_20200102.jpg", b"original")], "")?;
    fixture.import(std::slice::from_ref(&source), "", 1)?;
    let output = fixture
        .folders
        .gallery
        .join("2020/01/photos/IMG_20200102.jpg");
    fs::write(&output, b"modified")?;
    let counts = fixture.import(&[source], "", 2)?;
    assert_eq!(counts["resumed"], 0);
    assert_eq!(counts["imported"], 1);
    let content: Vec<_> = files::media_files(&fixture.folders.gallery)?
        .iter()
        .map(fs::read)
        .collect::<std::io::Result<_>>()?;
    assert!(content.contains(&b"original".to_vec()));
    assert!(content.contains(&b"modified".to_vec()));
    Ok(())
}

#[test]
fn archived_layout_safe_names_and_unsupported_members() -> Result<()> {
    let fixture = Fixture::new()?;
    let archive = fixture.zip(
        "annual.zip",
        &[
            ("2019/07/photos/no-date.jpg", b"photo"),
            ("2018/foto/old.jpg", b"legacy"),
            ("album/CON.jpg", b"reserved"),
            ("album/notes.txt", b"unsupported"),
            ("album/irrelevant.json", b"{invalid"),
        ],
        "",
    )?;
    let counts = fixture.import(&[archive], "", 1)?;
    assert_eq!(counts["imported"], 3);
    assert_eq!(counts["unrecognized"], 1);
    assert_eq!(
        fs::read(fixture.folders.gallery.join("2019/07/photos/no-date.jpg"))?,
        b"photo"
    );
    assert_eq!(
        fs::read(
            fixture
                .folders
                .gallery
                .join("2018/Unknown month/photos/old.jpg")
        )?,
        b"legacy"
    );
    assert!(fixture
        .folders
        .gallery
        .join("Unknown year/Unknown month/photos/_CON.jpg")
        .exists());
    Ok(())
}

#[test]
fn encrypted_input_rejects_wrong_password_then_imports() -> Result<()> {
    let fixture = Fixture::new()?;
    let metadata = google("IMG_20140607.jpg", "1419793427");
    let source = fixture.zip(
        "encrypted.zip",
        &[
            ("album/IMG_20140607.jpg", b"secret photo"),
            ("album/IMG_20140607.jpg.json", &metadata),
        ],
        "secret",
    )?;
    assert!(fixture
        .import(std::slice::from_ref(&source), "wrong", 4)
        .is_err());
    assert!(fixture
        .import(std::slice::from_ref(&source), "", 1)
        .is_err());
    assert!(files::media_files(&fixture.folders.gallery)?.is_empty());
    let counts = fixture.import(&[source], "secret", 4)?;
    assert_eq!(counts["imported"], 1);
    assert_eq!(
        fs::read(
            fixture
                .folders
                .gallery
                .join("2014/12/photos/IMG_20140607.jpg")
        )?,
        b"secret photo"
    );
    assert!(!fs::read_dir(&fixture.folders.gallery)?.any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".import-")));
    Ok(())
}

#[test]
fn manual_decisions_survive_aes_export_rebuild_and_local_choices_win() -> Result<()> {
    let fixture = Fixture::new()?;
    let metadata = google("IMG_20140607.jpg", "1419793427");
    let source = fixture.zip(
        "source.zip",
        &[
            ("album/IMG_20140607.jpg", b"photo"),
            ("album/IMG_20140607.jpg.json", &metadata),
        ],
        "",
    )?;
    fixture.import(std::slice::from_ref(&source), "", 1)?;
    gallery::correct_date(
        &fixture.folders,
        "2014/12/photos/IMG_20140607.jpg",
        "2013-06-07",
    )?;
    fixture.import(&[source], "", 4)?;
    let exported = export_archives(&fixture.folders, "secret", &TaskContext::default())?;
    assert!(exported["archives"]
        .as_array()
        .unwrap()
        .contains(&json!("2013.zip")));
    let archive_path = fixture.folders.exports.join("2013.zip");
    let mut archive = ZipArchive::new(File::open(&archive_path)?)?;
    assert!(archive.by_name("2013/06/photos/IMG_20140607.jpg").is_err());
    let fresh = Fixture::new()?;
    fresh.import(std::slice::from_ref(&archive_path), "secret", 2)?;
    let media = fresh
        .folders
        .gallery
        .join("2013/06/photos/IMG_20140607.jpg");
    assert_eq!(fs::read(&media)?, b"photo");
    let sidecar: Value = serde_json::from_slice(&fs::read(files::sidecar(&media))?)?;
    assert_eq!(sidecar["gpexitDate"], "2013-06-07");
    assert_eq!(
        sidecar["gpexitOriginalPhotoTakenTime"]["timestamp"],
        "1419793427"
    );
    assert_eq!(gallery::load_decisions(&fresh.folders.gallery)?.len(), 1);
    gallery::correct_date(
        &fresh.folders,
        "2013/06/photos/IMG_20140607.jpg",
        "2012-03-04",
    )?;
    fresh.import(&[archive_path], "secret", 2)?;
    assert!(fresh
        .folders
        .gallery
        .join("2012/03/photos/IMG_20140607.jpg")
        .exists());
    Ok(())
}

#[test]
fn invalid_manifests_do_not_create_media_or_decisions() -> Result<()> {
    let checksum = "a".repeat(64);
    for value in [
        json!({"bad checksum": "2014-06-07"}),
        json!({checksum.clone(): "2014-02-30"}),
        json!({checksum.clone(): "20140607"}),
        json!({checksum.clone(): {"date":"2014-06-07"}}),
        json!([]),
    ] {
        let fixture = Fixture::new()?;
        let raw = serde_json::to_vec(&value)?;
        let archive = fixture.zip(
            "invalid.zip",
            &[
                ("2014/.gpexit-dates.json", &raw),
                ("2014/06/photos/IMG_20140607.jpg", b"photo"),
            ],
            "",
        )?;
        assert!(fixture.import(&[archive], "", 1).is_err());
        assert!(gallery::load_decisions(&fixture.folders.gallery)?.is_empty());
        assert!(files::media_files(&fixture.folders.gallery)?.is_empty());
    }
    Ok(())
}

#[test]
fn trash_decisions_skip_reimport_and_never_enter_exports() -> Result<()> {
    let fixture = Fixture::new()?;
    let source = fixture.zip("source.zip", &[("IMG_20200102.jpg", b"photo")], "")?;
    fixture.import(std::slice::from_ref(&source), "", 1)?;
    let entry = gallery::trash_file(&fixture.folders, "2020/01/photos/IMG_20200102.jpg")?;
    let counts = fixture.import(&[source], "", 4)?;
    assert_eq!(counts["resumed"], 1);
    assert!(files::media_files(&fixture.folders.gallery)?.is_empty());
    export_archives(&fixture.folders, "", &TaskContext::default())?;
    let archive = ZipArchive::new(File::open(fixture.folders.exports.join("2020.zip"))?)?;
    assert_eq!(archive.len(), 0);
    assert_eq!(
        gallery::restore_file(&fixture.folders, &entry)?,
        "2020/01/photos/IMG_20200102.jpg"
    );
    assert!(fixture
        .folders
        .gallery
        .join("2020/01/photos/IMG_20200102.jpg")
        .exists());
    Ok(())
}

#[test]
fn cancellation_before_import_leaves_no_partial_media() -> Result<()> {
    let fixture = Fixture::new()?;
    let source = fixture.zip("source.zip", &[("IMG_20200102.jpg", b"photo")], "")?;
    let manager = TaskManager::default();
    let folders = fixture.folders.clone();
    manager.start(move |ctx| {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ctx.is_cancelled() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        import_archives(&folders, &[source], "", 4, &ctx)
    })?;
    manager.cancel();
    manager.wait();
    assert!(manager.status().status.starts_with("Interrotto"));
    assert!(files::media_files(&fixture.folders.gallery)?.is_empty());
    Ok(())
}

fn exif_jpeg() -> Vec<u8> {
    let mut tiff = b"II\x2a\x00\x08\x00\x00\x00".to_vec();
    tiff.extend_from_slice(&[1, 0, 0x69, 0x87, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0]);
    tiff.extend_from_slice(&[1, 0, 3, 0x90, 2, 0, 20, 0, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0]);
    tiff.extend_from_slice(b"2017:08:09 10:11:12\0");
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
    jpeg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    jpeg.extend_from_slice(b"Exif\0\0");
    jpeg.extend_from_slice(&tiff);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    jpeg
}

#[test]
fn exif_fallback_preserves_known_archive_month() -> Result<()> {
    let fixture = Fixture::new()?;
    let jpeg = exif_jpeg();
    let mut second = jpeg.clone();
    // Different content prevents deduplication from hiding archive-month behavior.
    second.extend_from_slice(b"different content");
    let source = fixture.zip(
        "exif.zip",
        &[
            ("album/camera.jpg", &jpeg),
            ("2018/03/photos/camera.jpg", &second),
        ],
        "",
    )?;
    let counts = fixture.import(&[source], "", 4)?;
    assert_eq!(counts["imported"], 2);
    assert_eq!(
        fs::read(fixture.folders.gallery.join("2017/08/photos/camera.jpg"))?,
        jpeg
    );
    assert_eq!(
        fs::read(fixture.folders.gallery.join("2018/03/photos/camera.jpg"))?,
        second
    );
    Ok(())
}

#[test]
fn encrypted_media_without_json_fails_cleanly_in_worker() -> Result<()> {
    let fixture = Fixture::new()?;
    let source = fixture.zip(
        "media-only.zip",
        &[("IMG_20200102.jpg", b"secret photo")],
        "secret",
    )?;
    assert!(fixture
        .import(std::slice::from_ref(&source), "wrong", 4)
        .is_err());
    assert!(files::media_files(&fixture.folders.gallery)?.is_empty());
    assert!(!fs::read_dir(&fixture.folders.gallery)?.any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".import-")));
    assert_eq!(fixture.import(&[source], "secret", 4)?["imported"], 1);
    Ok(())
}

#[test]
fn changed_input_content_changes_checkpoint_session() -> Result<()> {
    let fixture = Fixture::new()?;
    let source = fixture.zip("source.zip", &[("IMG_20200102.jpg", b"first")], "")?;
    fixture.import(std::slice::from_ref(&source), "", 1)?;
    fixture.zip("source.zip", &[("IMG_20200102.jpg", b"other")], "")?;
    let counts = fixture.import(&[source], "", 4)?;
    assert_eq!(counts["resumed"], 0);
    assert_eq!(counts["imported"], 1);
    assert_eq!(files::media_files(&fixture.folders.gallery)?.len(), 2);
    Ok(())
}

#[test]
fn traditional_zipcrypto_input_is_supported_without_external_runtime() -> Result<()> {
    let fixture = Fixture::new()?;
    // Fixed synthetic archive produced by Info-ZIP; no external tool runs in this test.
    let bytes = hex::decode(concat!(
        "504b0304140009000800afa96a5d7ff62a11290000001e00000010001c00",
        "494d475f32303230303130322e6a70675554090003e97af36ae97af36a75",
        "780b000104f501000004000000009d62e94682145944a341028263667dfc",
        "04d0148319f9714bc45cb741b48a0f87116e4c28d0cf2d6bfc504b07087f",
        "f62a11290000001e000000504b01021e03140009000800afa96a5d7ff62a",
        "11290000001e000000100018000000000001000000a48100000000494d47",
        "5f32303230303130322e6a70675554050003e97af36a75780b000104f501",
        "00000400000000504b0506000000000100010056000000830000000000"
    ))?;
    let source = fixture.folders.inputs.join("zipcrypto.zip");
    fs::write(&source, bytes)?;
    assert!(fixture
        .import(std::slice::from_ref(&source), "wrong", 2)
        .is_err());
    assert_eq!(fixture.import(&[source], "secret", 4)?["imported"], 1);
    assert_eq!(
        fs::read(
            fixture
                .folders
                .gallery
                .join("2020/01/photos/IMG_20200102.jpg")
        )?,
        b"traditional encryption fixture"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn import_rejects_symlinked_gallery_destination() -> Result<()> {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new()?;
    let outside = tempfile::tempdir()?;
    symlink(outside.path(), fixture.folders.gallery.join("2014"))?;
    let source = fixture.zip(
        "symlink.zip",
        &[("IMG_20140607.jpg", b"original source media")],
        "",
    )?;
    assert!(fixture.import(&[source], "", 4).is_err());
    assert_eq!(fs::read_dir(outside.path())?.count(), 0);
    Ok(())
}
