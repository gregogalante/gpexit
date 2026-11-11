# GPExit

GPExit is a desktop app for taking control of your Google Photos archive. Import Takeout ZIP files, browse your photos, correct capture dates, review cleanup suggestions, and create encrypted annual backups. All processing happens locally on your computer.

## Get started

Install the package for your system from [Releases](https://github.com/gregogalante/gpexit/releases):

| System | Package |
| --- | --- |
| macOS, Apple Silicon | `macos-arm64.dmg` |
| macOS, Intel | `macos-x64.dmg` |
| Windows, x64 | `windows-x64-setup.exe` or `windows-x64.msi` |
| Linux, x64 | `linux-x64.AppImage` or `linux-x64.deb` |

On first launch, select three existing folders using the native folder pickers:

- **Inputs:** Your Google Photos Takeout ZIP files.
- **Gallery:** Your organized photos, videos, metadata, and review state.
- **Exports:** Your generated annual ZIP backups.

Folders must be distinct and cannot contain one another. Selecting a folder does not move its contents. GPExit remembers the paths after restarting; change them in Settings when needed. Passwords are never saved in settings or logs.

## Import and organize

Select input ZIP files, enter a password when required, and start importing. GPExit supports AES and traditional ZipCrypto archives, associates Google JSON metadata across selected ZIP files, and organizes media by year, month, and type. SHA-256 deduplication avoids storing identical media twice. Streaming extraction uses one to four workers and checkpoints completed work for resume.

The sidebar provides access to inputs, gallery, date review, assisted cleanup, exports, settings, and activity. Folder statistics show logical file sizes, file counts, and available gallery-volume space. Activity shows progress and the latest 500 log lines for the current session. Tasks can be cancelled; closing GPExit requests cancellation and waits for cleanup.

## Browse and correct dates

Filter the gallery by year, month, and review status. Inspect a photo, mark it to keep, or move it with its metadata to recoverable trash. Restore files from Trash without overwriting existing files.

Date review compares filename timestamps, Google metadata, and supported original EXIF dates. Manual corrections move the media and JSON together, update `photoTakenTime`, preserve its first original value in `gpexitOriginalPhotoTakenTime`, and record `gpexitDate`. These corrections remain portable without the SQLite database. Export the review as CSV through the native save dialog.

Manual decisions take priority over inferred dates. Google timestamps use UTC; choosing a new calendar date preserves the existing UTC clock, or uses midnight when no valid clock exists.

JPEG, PNG, GIF, WebP, TIFF, and BMP have cached image previews. Other formats remain available for archive operations and metadata review. Video playback depends on system WebView codecs. Thumbnails are generated on demand with bounded decoding and an approximately 512 MiB cache outside the gallery.

## Assisted cleanup

GPExit offers four local review tools:

- **Large files:** Photos and videos above a chosen size, largest first.
- **Nearby shots:** Photos captured close together, grouped with bounded time spans.
- **Similar photos:** Approximate visual matches using a difference hash.
- **Low detail:** Photos with low estimated thumbnail sharpness.

Run photo analysis to populate visual suggestions. No models are downloaded and no photos are uploaded. Analysis results are cached incrementally; unchanged photos reuse their results after restarting.

Suggestions need human review: similar photos may contain different moments, and low detail does not prove blur. No files are preselected. Explicitly select files or groups and review the count and size before moving them to recoverable trash. Files marked to keep are protected. Bulk operations support cancellation and reject stale selections. GPExit does not permanently delete media.

## Export and back up

Create annual ZIP64 archives, optionally encrypted with AES-256. Encrypted export requires entering the password twice. Input archives remain untouched. Interrupted exports do not replace previously completed archives.

Back up the **entire gallery, including hidden files**, to preserve review state and recoverable removals:

```text
gallery/
  YYYY/MM/photos/...
  YYYY/MM/videos/...
  .gpexit-dates.sqlite3
  .gpexit-progress.sqlite3
  .gpexit-cleanup.sqlite3
  .gpexit-trash/...
```

Trash is excluded from browsing and new exports. Existing exported ZIP files retain their previous contents until regenerated. Photo/JSON pairs carry date corrections independently of SQLite; annual exports also include the manual-date manifest. Cleanup analysis is derived data and can be rebuilt.

GPExit locks the selected gallery to prevent concurrent app instances from modifying it. Avoid modifying gallery contents with another archive tool while a task is running.

## Build and develop

Requires Rust 1.91 or newer and [Tauri platform prerequisites](https://v2.tauri.app/start/prerequisites/). The interface is bundled with the app; no Node installation, frontend build, HTTP server, or separate runtime is needed.

```sh
cargo run --locked
```

For a local macOS app:

```sh
cargo install tauri-cli --version 2.12.4 --locked
scripts/bundle-macos.sh
```

Output: `target/release/bundle/macos/GPExit.app`. Other platform packages and release signing are covered in [Release builds](docs/releasing.md).

Run verification:

```sh
cargo test --locked --all-targets
cargo clippy --all-targets --locked -- -D warnings
cargo fmt -- --check
```

The engine can also be tested without the native WebView:

```sh
cargo test --no-default-features --locked --all-targets
```

Integration tests use temporary synthetic archives and cover encryption, deduplication, metadata association, resume, date corrections, trash and restore, persistence, locks, previews, cleanup, IPC, and cancellation. Development probes in `examples/` support interoperability and performance checks.

On macOS, settings are stored in `~/Library/Application Support/it.gpexit.desktop/folders.json`; previews are under `~/Library/Caches/it.gpexit.desktop/previews`. `GPEXIT_CONFIG_DIR` and `GPEXIT_CACHE_DIR` override these locations for isolated tests. Other systems use their platform application directories.

See [AGENTS.md](AGENTS.md) for source structure and development conventions.
