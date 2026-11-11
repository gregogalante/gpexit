//! A small headless harness for interoperability and performance checks.
use anyhow::{Context, Result};
use gpexit_core::{archive, config::Folders, task::TaskContext};
use std::{env, fs, path::PathBuf};
fn main() -> Result<()> {
    let args: Vec<_> = env::args().collect();
    let root = PathBuf::from(
        args.get(1)
            .context("Usage: archive_probe ROOT import|export [PASSWORD]")?,
    );
    let folders = Folders {
        inputs: root.join("inputs"),
        gallery: root.join("gallery"),
        exports: root.join("exports"),
    };
    for path in [&folders.inputs, &folders.gallery, &folders.exports] {
        fs::create_dir_all(path)?;
    }
    let password = args.get(3).map(String::as_str).unwrap_or("");
    let result = match args.get(2).map(String::as_str) {
        Some("import") => {
            let mut paths: Vec<_> = fs::read_dir(&folders.inputs)?
                .filter_map(|v| v.ok())
                .map(|v| v.path())
                .filter(|v| v.extension().is_some_and(|v| v.eq_ignore_ascii_case("zip")))
                .collect();
            paths.sort();
            archive::import_archives(&folders, &paths, password, 4, &TaskContext::default())?
        }
        Some("export") => archive::export_archives(&folders, password, &TaskContext::default())?,
        _ => anyhow::bail!("Expected import or export"),
    };
    println!("{result}");
    Ok(())
}
