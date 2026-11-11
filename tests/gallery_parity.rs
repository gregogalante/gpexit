use anyhow::Result;
use chrono::NaiveDate;
use gpexit_core::{config::Folders, dates, files, gallery};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

struct Fixture {
    _root: tempfile::TempDir,
    folders: Folders,
    relative: String,
    original: Value,
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
        let relative = "2014/12/photos/IMG_20140607_154612.jpg".to_string();
        let media = folders.gallery.join(&relative);
        fs::create_dir_all(media.parent().unwrap())?;
        fs::write(&media, b"original photo bytes")?;
        let original = json!({"title": "IMG_20140607_154612.jpg", "description": "unchanged", "photoTakenTime": {"timestamp": "1419793427", "formatted": "original Google text"}, "geoData": {"latitude": 0}});
        fs::write(files::sidecar(&media), serde_json::to_vec(&original)?)?;
        Ok(Self {
            _root: root,
            folders,
            relative,
            original,
        })
    }
    fn media(&self) -> PathBuf {
        self.folders.gallery.join(&self.relative)
    }
}

fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

#[test]
fn common_filename_dates_and_google_priority() {
    let expected = NaiveDate::from_ymd_opt(2014, 6, 7);
    for name in [
        "IMG_20140607_154612.jpg",
        "VID_20140607_154612.mp4",
        "PXL_20140607_154612123.jpg",
        "IMG-20140607-WA0001.jpg",
        "Screenshot_2014-06-07-15-46-12.png",
        "20140607154612123.jpg",
        "2014.06.07.jpg",
        "2014_06_07.jpg",
    ] {
        assert_eq!(dates::filename_date(name), expected, "{name}");
    }
    for name in [
        "IMG_20210230.jpg",
        "IMG_20211301.jpg",
        "photo_2020.jpg",
        "120140607.jpg",
        "2014060712345.jpg",
    ] {
        assert_eq!(dates::filename_date(name), None, "{name}");
    }
    let google = json!({"photoTakenTime": {"timestamp": "1419793427"}});
    assert_eq!(
        dates::choose_date("IMG_20140607_154612.jpg", &google),
        NaiveDate::from_ymd_opt(2014, 12, 28)
    );
    assert_eq!(
        dates::choose_date("IMG_20140607_154612.jpg", &Value::Null),
        expected
    );
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"photoTakenTime": null}),
        json!({"photoTakenTime": {"timestamp": "bad"}}),
        json!({"photoTakenTime": {"timestamp": "-1"}}),
    ] {
        assert_eq!(dates::google_date(&value), None);
    }
    assert_eq!(
        dates::archive_date_parts("2014/06/photos/file.jpg"),
        (Some("2014".into()), Some("06".into()))
    );
    assert_eq!(
        dates::archive_date_parts("2014/foto/file.jpg"),
        (Some("2014".into()), Some("Unknown month".into()))
    );
    assert_eq!(
        dates::archive_date_parts("Unknown year/Unknown month/videos/file.mp4"),
        (Some("Unknown year".into()), Some("Unknown month".into()))
    );
}

#[test]
fn embedded_exif_capture_date_is_read() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("capture.tiff");
    let mut bytes = b"II\x2a\0\x08\0\0\0".to_vec();
    bytes.extend_from_slice(&[1, 0, 0x69, 0x87, 4, 0, 1, 0, 0, 0, 26, 0, 0, 0, 0, 0, 0, 0]);
    bytes.extend_from_slice(&[1, 0, 3, 0x90, 2, 0, 20, 0, 0, 0, 44, 0, 0, 0, 0, 0, 0, 0]);
    bytes.extend_from_slice(b"2014:06:07 15:46:12\0");
    fs::write(&path, bytes)?;
    assert_eq!(
        dates::embedded_date(&path),
        NaiveDate::from_ymd_opt(2014, 6, 7)
    );
    Ok(())
}

#[test]
fn manual_corrections_are_portable_and_preserve_first_original_date() -> Result<()> {
    let fixture = Fixture::new()?;
    assert_eq!(gallery::conflicts(&fixture.folders)?.len(), 1);
    let corrected = gallery::correct_date(&fixture.folders, &fixture.relative, "2013-06-07")?;
    assert_eq!(corrected, "2013/06/photos/IMG_20140607_154612.jpg");
    let corrected = gallery::correct_date(&fixture.folders, &corrected, "2012-03-04")?;
    let path = fixture.folders.gallery.join(&corrected);
    let metadata = read_json(&files::sidecar(&path))?;
    assert_eq!(
        metadata["gpexitOriginalPhotoTakenTime"],
        fixture.original["photoTakenTime"]
    );
    assert_eq!(metadata["description"], fixture.original["description"]);
    assert_eq!(metadata["geoData"], fixture.original["geoData"]);
    assert_eq!(metadata["gpexitDate"], "2012-03-04");
    assert_eq!(
        dates::google_date(&metadata),
        NaiveDate::from_ymd_opt(2012, 3, 4)
    );
    assert_eq!(
        metadata["photoTakenTime"]["formatted"],
        "2012-03-04 19:03:47 UTC"
    );
    assert_eq!(fs::read(&path)?, b"original photo bytes");
    assert_eq!(
        gallery::load_decisions(&fixture.folders.gallery)?.get(&files::file_hash(&path)?),
        Some(&"2012-03-04".to_string())
    );
    fs::remove_file(fixture.folders.gallery.join(".gpexit-dates.sqlite3"))?;
    assert!(gallery::conflicts(&fixture.folders)?.is_empty());
    assert_eq!(
        dates::choose_date(path.file_name().unwrap().to_str().unwrap(), &metadata),
        NaiveDate::from_ymd_opt(2012, 3, 4)
    );
    Ok(())
}

#[test]
fn invalid_metadata_and_collisions_do_not_partially_correct_media() -> Result<()> {
    let fixture = Fixture::new()?;
    let original_json = fs::read(files::sidecar(&fixture.media()))?;
    let destination = fixture
        .folders
        .gallery
        .join("2013/06/photos/IMG_20140607_154612.jpg");
    fs::create_dir_all(destination.parent().unwrap())?;
    fs::write(files::sidecar(&destination), b"existing metadata")?;
    assert!(gallery::correct_date(&fixture.folders, &fixture.relative, "2013-06-07").is_err());
    assert!(fixture.media().is_file());
    assert_eq!(fs::read(files::sidecar(&fixture.media()))?, original_json);
    assert_eq!(
        fs::read(files::sidecar(&destination))?,
        b"existing metadata"
    );
    assert!(gallery::load_decisions(&fixture.folders.gallery)?.is_empty());
    fs::remove_file(files::sidecar(&destination))?;
    fs::write(files::sidecar(&fixture.media()), b"[]")?;
    assert!(gallery::correct_date(&fixture.folders, &fixture.relative, "2013-06-07").is_err());
    assert!(fixture.media().is_file());
    assert_eq!(fs::read(files::sidecar(&fixture.media()))?, b"[]");
    assert!(gallery::load_decisions(&fixture.folders.gallery)?.is_empty());
    assert!(gallery::correct_date(&fixture.folders, &fixture.relative, "1800-01-01").is_err());
    assert!(gallery::correct_date(&fixture.folders, &fixture.relative, "2014-02-30").is_err());
    Ok(())
}

#[test]
fn imported_decisions_validate_atomically_and_local_values_win() -> Result<()> {
    let fixture = Fixture::new()?;
    let checksum = files::file_hash(&fixture.media())?;
    gallery::set_decisions(
        &fixture.folders.gallery,
        &HashMap::from([(checksum.clone(), "2013-06-07".to_string())]),
    )?;
    gallery::set_decisions(
        &fixture.folders.gallery,
        &HashMap::from([(checksum.clone(), "2012-03-04".to_string())]),
    )?;
    assert_eq!(
        gallery::load_decisions(&fixture.folders.gallery)?[&checksum],
        "2013-06-07"
    );
    assert!(gallery::set_decisions(
        &fixture.folders.gallery,
        &HashMap::from([("not-a-checksum".to_string(), "2013-06-07".to_string())])
    )
    .is_err());
    assert!(gallery::set_decisions(
        &fixture.folders.gallery,
        &HashMap::from([("f".repeat(64), "2014-02-30".to_string())])
    )
    .is_err());
    assert_eq!(gallery::load_decisions(&fixture.folders.gallery)?.len(), 1);
    Ok(())
}

#[test]
fn keep_decisions_persist_and_file_changes_require_review() -> Result<()> {
    let fixture = Fixture::new()?;
    assert_eq!(gallery::list_gallery(&fixture.folders)?[0]["kept"], false);
    gallery::keep_file(&fixture.folders, &fixture.relative)?;
    assert_eq!(gallery::list_gallery(&fixture.folders)?[0]["kept"], true);
    fs::write(fixture.media(), b"different photo bytes")?;
    let row = &gallery::list_gallery(&fixture.folders)?[0];
    assert_eq!(row["kept"], false);
    assert_eq!(row["year"], "2014");
    assert_eq!(row["month"], "12");
    Ok(())
}

#[test]
fn trash_and_restore_preserve_media_json_and_import_exclusion() -> Result<()> {
    let fixture = Fixture::new()?;
    let checksum = files::file_hash(&fixture.media())?;
    let metadata = fs::read(files::sidecar(&fixture.media()))?;
    let id = gallery::trash_file(&fixture.folders, &fixture.relative)?;
    assert!(!fixture.media().exists());
    assert!(!files::sidecar(&fixture.media()).exists());
    assert!(files::media_files(&fixture.folders.gallery)?.is_empty());
    assert!(gallery::list_gallery(&fixture.folders)?.is_empty());
    assert_eq!(
        gallery::list_trash(&fixture.folders)?[0]["path"],
        fixture.relative
    );
    assert!(gallery::trashed_hashes(&fixture.folders)?.contains(&checksum));
    let stats = gallery::storage_stats(&fixture.folders)?;
    assert_eq!(stats["gallery"]["files"], 0);
    assert_eq!(stats["trash"]["files"], 3);
    let restored = gallery::restore_file(&fixture.folders, &id)?;
    assert_eq!(restored, fixture.relative);
    assert_eq!(fs::read(fixture.media())?, b"original photo bytes");
    assert_eq!(fs::read(files::sidecar(&fixture.media()))?, metadata);
    assert!(gallery::trashed_hashes(&fixture.folders)?.is_empty());
    assert!(gallery::list_trash(&fixture.folders)?.is_empty());
    Ok(())
}

#[test]
fn restore_refuses_media_and_metadata_collisions() -> Result<()> {
    let fixture = Fixture::new()?;
    let id = gallery::trash_file(&fixture.folders, &fixture.relative)?;
    fs::write(fixture.media(), b"new file")?;
    assert!(gallery::restore_file(&fixture.folders, &id).is_err());
    assert_eq!(fs::read(fixture.media())?, b"new file");
    assert_eq!(gallery::list_trash(&fixture.folders)?.len(), 1);
    fs::remove_file(fixture.media())?;
    fs::write(files::sidecar(&fixture.media()), b"new metadata")?;
    assert!(gallery::restore_file(&fixture.folders, &id).is_err());
    assert_eq!(fs::read(files::sidecar(&fixture.media()))?, b"new metadata");
    assert_eq!(gallery::list_trash(&fixture.folders)?.len(), 1);
    Ok(())
}

#[test]
fn gallery_paths_reject_traversal_and_trash_tampering() -> Result<()> {
    let fixture = Fixture::new()?;
    for path in [
        "../outside.jpg",
        "/outside.jpg",
        ".gpexit-trash/photo.jpg",
        "archives/photo.jpg",
    ] {
        assert!(files::media_path(&fixture.folders.gallery, path).is_err());
    }
    for id in ["../outside", "", "/tmp/photo", "x", "f/../../g"] {
        assert!(gallery::restore_file(&fixture.folders, id).is_err());
    }
    let id = gallery::trash_file(&fixture.folders, &fixture.relative)?;
    fs::write(
        fixture.folders.trash().join(&id).join("entry.json"),
        br#"{"path":"../../outside.jpg"}"#,
    )?;
    assert!(gallery::restore_file(&fixture.folders, &id).is_err());
    assert!(gallery::list_trash(&fixture.folders)?.is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn media_and_storage_skip_symbolic_links() -> Result<()> {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new()?;
    let outside = fixture.folders.inputs.join("outside.jpg");
    fs::write(&outside, b"outside")?;
    symlink(&outside, fixture.folders.gallery.join("link.jpg"))?;
    symlink(
        &fixture.folders.inputs,
        fixture.folders.gallery.join("linked-inputs"),
    )?;
    assert!(files::media_path(&fixture.folders.gallery, "link.jpg").is_err());
    assert!(files::media_path(&fixture.folders.gallery, "linked-inputs/outside.jpg").is_err());
    assert_eq!(files::media_files(&fixture.folders.gallery)?.len(), 1);
    let stats = gallery::storage_stats(&fixture.folders)?;
    assert_eq!(stats["inputs"]["bytes"], 7);
    assert_eq!(stats["gallery"]["files"], 2);
    assert_eq!(stats["exports"]["files"], 0);
    assert!(stats["disk"]["free"].as_u64().unwrap() <= stats["disk"]["total"].as_u64().unwrap());
    symlink(&fixture.folders.inputs, fixture.folders.trash())?;
    assert!(gallery::trash_file(&fixture.folders, &fixture.relative).is_err());
    assert!(fixture.media().exists());
    Ok(())
}

#[test]
fn filenames_are_portable_and_collisions_do_not_overwrite() -> Result<()> {
    assert_eq!(files::safe_filename("CON.jpg"), "_CON.jpg");
    assert_eq!(files::safe_filename("bad:name?.jpg "), "bad_name_.jpg");
    assert_eq!(files::safe_filename("..."), "_");
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("photo.jpg"), b"first")?;
    let checksum = "a".repeat(64);
    let destination = files::destination_for(root.path(), "photo.jpg", &checksum)?;
    assert_eq!(
        destination.file_name().unwrap(),
        "photo_aaaaaaaaaaaaaaaa.jpg"
    );
    fs::write(&destination, b"second")?;
    assert!(files::destination_for(root.path(), "photo.jpg", &checksum).is_err());
    assert_eq!(fs::read(root.path().join("photo.jpg"))?, b"first");
    assert_eq!(fs::read(destination)?, b"second");
    Ok(())
}

#[cfg(unix)]
#[test]
fn correction_rejects_symlinked_destination_before_changing_json() -> Result<()> {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new()?;
    let outside = fixture._root.path().join("outside");
    fs::create_dir(&outside)?;
    symlink(&outside, fixture.folders.gallery.join("2014/06"))?;
    let original = fs::read(files::sidecar(&fixture.media()))?;
    assert!(gallery::correct_date(&fixture.folders, &fixture.relative, "2014-06-07").is_err());
    assert_eq!(fs::read(files::sidecar(&fixture.media()))?, original);
    assert!(fixture.media().is_file());
    assert_eq!(fs::read_dir(&outside)?.count(), 0);
    Ok(())
}
