# Release builds

## Continuous integration

The test workflow checks the headless engine on macOS, Windows, and Linux, and builds a native macOS app. The release workflow tests and packages four native targets:

| Runner | Target | Artifacts |
| --- | --- | --- |
| macOS 15, Apple Silicon | aarch64-apple-darwin | DMG |
| macOS 15, Intel | x86_64-apple-darwin | DMG |
| Windows Server 2022 | x86_64-pc-windows-msvc | NSIS EXE, MSI |
| Ubuntu 22.04 | x86_64-unknown-linux-gnu | AppImage, DEB |

Version tags trigger release builds. The workflow can also be dispatched manually for a selected branch. Manual runs upload build artifacts without creating a release. Branch pushes run the test workflow; they do not duplicate the tag's release jobs. Artifacts expire after 14 days.

Every package job runs formatting checks, all-target tests, and Clippy before packaging. Tauri CLI is pinned to 2.12.4. Package names include the Cargo version and platform, with a SHA-256 manifest for each platform.

## Create a release

1. Set the version in `Cargo.toml`, update `Cargo.lock`, and commit the reviewed changes. Tauri reads the version from Cargo; do not duplicate it in the app configuration.
2. Push a matching tag, such as `v0.1.0`. A mismatched tag fails validation.
3. Wait for all four package jobs. The workflow verifies checksums and creates a **draft** GitHub release containing all installers and `SHA256SUMS.txt`.
4. Download and test installers on the target operating systems. Review release notes, signatures, and checksums, then publish the draft manually.

Tags containing a hyphen create prerelease drafts. Re-running a tag may replace assets in its draft; the workflow refuses to overwrite an already published release. No automatic updater is configured.

## Signing

Branch and manual builds use ad-hoc macOS signing. Release tags can use these repository secrets for Developer ID signing and notarization:

- `APPLE_CERTIFICATE`: Base64-encoded Developer ID Application certificate in PKCS#12 format.
- `APPLE_CERTIFICATE_PASSWORD`: Certificate password.
- `APPLE_SIGNING_IDENTITY`: Developer ID Application signing identity.
- `APPLE_ID`: Apple account used for notarization.
- `APPLE_PASSWORD`: App-specific password for that account.
- `APPLE_TEAM_ID`: Apple Developer team ID.

If `APPLE_CERTIFICATE` is supplied, signing identity and notarization credentials are required. Without it, the build remains ad-hoc signed and is not notarized. Credentials are exposed only to tag package jobs. Protect release tags and limit repository write access before configuring signing secrets.

Windows installers are unsigned by default. Configure Windows code signing before public distribution if trusted publisher identification is required. The installer bundles the Visual C++ runtime and WebView2 bootstrapper; installing WebView2 requires internet access if the runtime is absent.

See the official [macOS signing guide](https://v2.tauri.app/distribute/sign/macos/) and [Windows installer guide](https://v2.tauri.app/distribute/windows-installer/).

## Local packaging

Install the platform prerequisites and pinned CLI:

```sh
cargo install tauri-cli --version 2.12.4 --locked
```

Build on the target operating system:

```sh
# macOS: CI=true skips Finder-driven DMG layout, useful in automation.
CI=true cargo tauri build --ci --bundles app,dmg -- --locked

# Windows
cargo tauri build --ci --bundles nsis,msi -- --locked

# Linux
cargo tauri build --ci --bundles appimage,deb -- --locked
```

Outputs are under `target/release/bundle/`. An explicit `--target` places them under `target/<target>/release/bundle/`. Start from a clean bundle directory when collecting installers: the collector rejects missing, duplicate, or unexpected target artifacts and never overwrites its destination.

```sh
cargo run --release --no-default-features --locked --example release_artifacts -- \
  target/release/bundle dist macos-arm64
```

Supported labels: `macos-arm64`, `macos-x64`, `windows-x64`, and `linux-x64`. Collection streams package copies and hashes, stages output before publishing it, and creates `SHA256SUMS-<label>.txt`.
