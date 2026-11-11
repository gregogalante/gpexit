use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Folders {
    pub inputs: PathBuf,
    pub gallery: PathBuf,
    pub exports: PathBuf,
}

impl Folders {
    pub fn validate(&self) -> Result<Self> {
        let canonical = |p: &Path| -> Result<PathBuf> {
            if !p.is_dir() {
                bail!("Directory not found: {}", p.display());
            }
            p.canonicalize()
                .with_context(|| format!("Cannot access {}", p.display()))
        };
        let value = Self {
            inputs: canonical(&self.inputs)?,
            gallery: canonical(&self.gallery)?,
            exports: canonical(&self.exports)?,
        };
        let paths = [&value.inputs, &value.gallery, &value.exports];
        for (i, a) in paths.iter().enumerate() {
            for b in paths.iter().skip(i + 1) {
                if a.starts_with(b) || b.starts_with(a) {
                    bail!("Select three separate directories; directories cannot contain one another.");
                }
            }
        }
        Ok(value)
    }

    pub fn trash(&self) -> PathBuf {
        self.gallery.join(".gpexit-trash")
    }
}

pub fn load(path: &Path) -> Result<Option<Folders>> {
    if !path.exists() {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&fs::read(path)?)?))
}

pub fn save(path: &Path, folders: &Folders) -> Result<()> {
    let folders = folders.validate()?;
    let parent = path.parent().context("Invalid settings path")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer_pretty(&mut temporary, &folders)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|e| e.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persists_paths_and_rejects_nested_roots() -> Result<()> {
        let root = tempfile::tempdir()?;
        let folders = Folders {
            inputs: root.path().join("inputs"),
            gallery: root.path().join("gallery"),
            exports: root.path().join("exports"),
        };
        for p in [&folders.inputs, &folders.gallery, &folders.exports] {
            fs::create_dir(p)?;
        }
        let path = root.path().join("settings/folders.json");
        save(&path, &folders)?;
        assert_eq!(load(&path)?.unwrap(), folders.validate()?);
        let mut bad = folders.clone();
        bad.exports = bad.gallery.clone();
        assert!(bad.validate().is_err());
        Ok(())
    }
}
