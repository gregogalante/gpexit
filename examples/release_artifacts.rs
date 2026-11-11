//! Collect a target's native installers and publish checksums without overwriting files.
use anyhow::{bail, Context, Result};
use gpexit_core::files::file_hash;
use std::{
    env, fs,
    io::{self, Write},
    path::{Component, Path, PathBuf},
};
use tempfile::NamedTempFile;
use walkdir::WalkDir;

fn targets(label: &str) -> Result<&'static [&'static str]> {
    match label {
        "macos-arm64" | "macos-x64" => Ok(&["dmg"]),
        "windows-x64" => Ok(&["exe", "msi"]),
        "linux-x64" => Ok(&["AppImage", "deb"]),
        _ => bail!("Unsupported release label: {label}"),
    }
}

fn resolved_destination(path: &Path) -> Result<PathBuf> {
    let mut absolute = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::ParentDir => {
                absolute.pop();
            }
            Component::CurDir => {}
            _ => absolute.push(component),
        }
    }
    let mut existing = absolute.as_path();
    let mut remaining = Vec::new();
    while !existing.exists() {
        if existing.is_symlink() {
            bail!("Invalid destination symlink: {}", existing.display());
        }
        remaining.push(
            existing
                .file_name()
                .context("Invalid distribution directory")?
                .to_owned(),
        );
        existing = existing
            .parent()
            .context("Invalid distribution directory")?;
    }
    let mut resolved = existing.canonicalize()?;
    for name in remaining.into_iter().rev() {
        resolved.push(name);
    }
    Ok(resolved)
}

fn destination_name(label: &str, extension: &str) -> String {
    let setup = if extension == "exe" { "-setup" } else { "" };
    format!(
        "GPExit_{}_{label}{setup}.{extension}",
        env!("CARGO_PKG_VERSION")
    )
}

fn collect(bundle: &Path, distribution: &Path, label: &str) -> Result<Vec<PathBuf>> {
    let expected = targets(label)?;
    if !bundle.is_dir() || bundle.is_symlink() {
        bail!(
            "Bundle directory not found or is a symbolic link: {}",
            bundle.display()
        );
    }
    let bundle = bundle.canonicalize()?;
    let distribution = resolved_destination(distribution)?;
    if distribution.starts_with(&bundle) {
        bail!("Distribution directory must be outside the bundle directory");
    }
    if distribution.exists() && !distribution.is_dir() {
        bail!("Distribution destination is not a directory");
    }
    let mut sources = vec![None; expected.len()];
    for entry in WalkDir::new(&bundle)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| !entry.file_type().is_symlink())
    {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Some(extension) = entry.path().extension().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(index) = expected
            .iter()
            .position(|value| extension.eq_ignore_ascii_case(value))
        else {
            continue;
        };
        if sources[index].replace(entry.path().to_path_buf()).is_some() {
            bail!("Multiple .{} artifacts found for {label}", expected[index]);
        }
    }
    let sources = sources
        .into_iter()
        .zip(expected)
        .map(|(source, extension)| {
            source.with_context(|| format!("Missing .{extension} artifact for {label}"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut destinations: Vec<_> = expected
        .iter()
        .map(|extension| distribution.join(destination_name(label, extension)))
        .collect();
    destinations.push(distribution.join(format!("SHA256SUMS-{label}.txt")));
    for path in &destinations {
        if path.exists() || path.is_symlink() {
            bail!("Refusing to overwrite {}", path.display());
        }
    }
    fs::create_dir_all(&distribution)?;
    let mut staged = Vec::new();
    let mut checksums = String::new();
    for (source, destination) in sources.iter().zip(&destinations) {
        let mut temporary = NamedTempFile::new_in(&distribution)?;
        io::copy(&mut fs::File::open(source)?, &mut temporary)
            .with_context(|| format!("Cannot copy {}", source.display()))?;
        temporary.as_file().sync_all()?;
        let checksum = file_hash(temporary.path())?;
        let name = destination
            .file_name()
            .context("Invalid artifact filename")?
            .to_str()
            .context("Invalid artifact filename")?;
        checksums.push_str(&format!("{checksum}  {name}\n"));
        staged.push(temporary);
    }
    let mut manifest = NamedTempFile::new_in(&distribution)?;
    manifest.write_all(checksums.as_bytes())?;
    manifest.as_file().sync_all()?;
    staged.push(manifest);
    for (temporary, destination) in staged.into_iter().zip(&destinations) {
        temporary
            .persist_noclobber(destination)
            .map_err(|error| error.error)
            .with_context(|| format!("Cannot publish {}", destination.display()))?;
    }
    Ok(destinations)
}

fn main() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    if args.len() != 3 {
        bail!("Usage: release_artifacts BUNDLE_DIR DIST_DIR LABEL");
    }
    let label = args[2]
        .to_str()
        .context("Release label must be valid UTF-8")?;
    for path in collect(Path::new(&args[0]), Path::new(&args[1]), label)? {
        println!("{}", path.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn setup() -> Result<(tempfile::TempDir, PathBuf, PathBuf)> {
        let root = tempfile::tempdir()?;
        let bundle = root.path().join("bundle");
        let distribution = root.path().join("dist");
        fs::create_dir(&bundle)?;
        Ok((root, bundle, distribution))
    }

    #[test]
    fn requires_existing_bundle_and_supported_label() -> Result<()> {
        let (_root, bundle, distribution) = setup()?;
        assert!(collect(&bundle.join("missing"), &distribution, "macos-arm64").is_err());
        assert!(collect(&bundle, &distribution, "unknown").is_err());
        assert!(collect(&bundle, &distribution, "macos-arm64").is_err());
        assert!(!distribution.exists());
        Ok(())
    }

    #[test]
    fn validates_every_required_target_before_copying() -> Result<()> {
        let (_root, bundle, distribution) = setup()?;
        fs::write(bundle.join("setup.exe"), b"installer")?;
        assert!(collect(&bundle, &distribution, "windows-x64").is_err());
        assert!(collect(&bundle, &distribution, "linux-x64").is_err());
        assert!(!distribution.exists());
        Ok(())
    }

    #[test]
    fn rejects_duplicate_extension_before_copying() -> Result<()> {
        let (_root, bundle, distribution) = setup()?;
        fs::create_dir(bundle.join("nested"))?;
        fs::write(bundle.join("first.dmg"), b"first")?;
        fs::write(bundle.join("nested/second.DMG"), b"second")?;
        assert!(collect(&bundle, &distribution, "macos-arm64").is_err());
        assert!(!distribution.exists());
        Ok(())
    }

    #[test]
    fn refuses_existing_artifact_or_manifest_without_partial_copy() -> Result<()> {
        for existing in [
            destination_name("windows-x64", "msi"),
            "SHA256SUMS-windows-x64.txt".into(),
        ] {
            let (_root, bundle, distribution) = setup()?;
            fs::write(bundle.join("setup.exe"), b"exe")?;
            fs::write(bundle.join("setup.msi"), b"msi")?;
            fs::create_dir(&distribution)?;
            fs::write(distribution.join(existing), b"preserved")?;
            assert!(collect(&bundle, &distribution, "windows-x64").is_err());
            assert_eq!(fs::read_dir(&distribution)?.count(), 1);
            assert!(!distribution
                .join(destination_name("windows-x64", "exe"))
                .exists());
            let only = fs::read_dir(&distribution)?.next().unwrap()?.path();
            assert_eq!(fs::read(only)?, b"preserved");
        }
        Ok(())
    }

    #[test]
    fn collects_all_labels_with_exact_names_and_copy_checksums() -> Result<()> {
        for label in ["macos-arm64", "macos-x64", "windows-x64", "linux-x64"] {
            let (_root, bundle, distribution) = setup()?;
            fs::create_dir(bundle.join("nested"))?;
            fs::write(bundle.join("ignored.txt"), b"ignored")?;
            let extensions = targets(label)?;
            for extension in extensions {
                fs::write(
                    bundle.join("nested").join(format!("original.{extension}")),
                    extension.as_bytes(),
                )?;
            }
            let output = collect(&bundle, &distribution, label)?;
            assert_eq!(output.len(), extensions.len() + 1);
            let mut expected_manifest = String::new();
            for (path, extension) in output.iter().zip(extensions) {
                let name = format!(
                    "GPExit_{}_{label}{}.{extension}",
                    env!("CARGO_PKG_VERSION"),
                    if *extension == "exe" { "-setup" } else { "" }
                );
                assert_eq!(path.file_name().unwrap().to_str(), Some(name.as_str()));
                assert_eq!(fs::read(path)?, extension.as_bytes());
                expected_manifest.push_str(&format!(
                    "{}  {name}\n",
                    hex::encode(Sha256::digest(fs::read(path)?))
                ));
            }
            assert_eq!(
                fs::read_to_string(output.last().unwrap())?,
                expected_manifest
            );
            assert_eq!(fs::read_dir(&distribution)?.count(), output.len());
        }
        Ok(())
    }

    #[test]
    fn rejects_output_within_bundle_even_when_not_created_yet() -> Result<()> {
        let (_root, bundle, _distribution) = setup()?;
        fs::write(bundle.join("setup.dmg"), b"dmg")?;
        for distribution in [&bundle, &bundle.join("new/nested/dist")] {
            assert!(collect(&bundle, distribution, "macos-arm64").is_err());
        }
        assert!(!bundle.join("new").exists());
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn ignores_symlink_files_and_directories_and_rejects_output_alias() -> Result<()> {
        let (root, bundle, distribution) = setup()?;
        let external = root.path().join("external");
        fs::create_dir(&external)?;
        fs::write(external.join("setup.dmg"), b"external")?;
        std::os::unix::fs::symlink(external.join("setup.dmg"), bundle.join("linked.dmg"))?;
        std::os::unix::fs::symlink(&external, bundle.join("linked-folder"))?;
        assert!(collect(&bundle, &distribution, "macos-arm64").is_err());
        fs::write(bundle.join("real.dmg"), b"real")?;
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&bundle, &alias)?;
        assert!(collect(&bundle, &alias.join("dist"), "macos-arm64").is_err());
        Ok(())
    }
}
