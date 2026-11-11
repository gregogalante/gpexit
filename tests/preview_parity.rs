use anyhow::Result;
use gpexit_core::{config::Folders, preview, service::Engine};
use image::{DynamicImage, GenericImageView, RgbImage};
use serde_json::json;
use std::fs;

#[test]
fn native_preview_applies_exif_orientation_and_keeps_original() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("rotated.jpg");
    let mut jpeg = Vec::new();
    let image =
        DynamicImage::ImageRgb8(RgbImage::from_pixel(1800, 1200, image::Rgb([60, 100, 70])));
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg).encode_image(&image)?;
    let tiff = b"II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut encoded = vec![0xff, 0xd8, 0xff, 0xe1];
    encoded.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    encoded.extend_from_slice(b"Exif\0\0");
    encoded.extend_from_slice(tiff);
    encoded.extend_from_slice(&jpeg[2..]);
    fs::write(&source, &encoded)?;
    let destination = root.path().join("preview.jpg");
    preview::generate(&source, &destination)?;
    let dimensions = image::open(destination)?.dimensions();
    assert!(dimensions.0 <= 900 && dimensions.1 <= 700);
    assert!(dimensions.1 > dimensions.0);
    assert_eq!(fs::read(source)?, encoded);
    Ok(())
}

#[test]
fn preview_cache_reuses_files_invalidates_changed_media_and_rejects_escape() -> Result<()> {
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
    let source = folders.gallery.join("sample.png");
    RgbImage::from_pixel(20, 10, image::Rgb([1, 2, 3])).save(&source)?;
    let first = preview::source(&engine, "sample.png")?;
    assert_eq!(preview::source(&engine, "sample.png")?, first);
    RgbImage::from_pixel(30, 10, image::Rgb([9, 8, 7])).save(&source)?;
    assert_ne!(
        preview::source(&engine, "sample.png")?["path"],
        first["path"]
    );
    assert!(preview::source(&engine, "../outside.png").is_err());
    engine.request("POST", "/api/gallery/trash", json!({"path":"sample.png"}))?;
    assert!(preview::source(&engine, "sample.png").is_err());
    Ok(())
}

#[test]
fn cache_cleanup_never_removes_other_file_types() -> Result<()> {
    let root = tempfile::tempdir()?;
    fs::write(root.path().join("one.jpg"), [1; 20])?;
    fs::write(root.path().join("two.jpg"), [2; 20])?;
    fs::write(root.path().join("keep.json"), b"unchanged")?;
    preview::prune_cache(root.path(), 0)?;
    assert!(!root.path().join("one.jpg").exists());
    assert!(!root.path().join("two.jpg").exists());
    assert_eq!(fs::read(root.path().join("keep.json"))?, b"unchanged");
    Ok(())
}
