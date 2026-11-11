# GPExit desktop

GPExit is a standalone Rust/Tauri application. The archive engine, database operations, date parsing, image previews, filesystem operations, and task management run in Rust. The interface is bundled HTML/CSS/JavaScript in the operating system WebView. No separate runtime, browser tab, or HTTP server is required.

## Run

Open `GPExit.app` on macOS. On first launch, use the native folder pickers to select three existing directories:

- **Inputs:** Original Takeout ZIP files, including encrypted ZIP files.
- **Gallery:** Extracted photos, videos, JSON metadata, and local review databases.
- **Exports:** Generated annual ZIP files.

The directories must be distinct and cannot contain one another. Selection does not move existing files. Paths are saved under the operating system application configuration directory and restored on subsequent launches. Missing or unavailable paths can be replaced in Settings. Passwords are not saved in configuration or logs.

The native gallery lock prevents two desktop instances from modifying the same gallery. Do not run other archive tools against the gallery concurrently.

## Features

- Select input ZIP files and import with one to four streaming workers. AES and traditional ZipCrypto inputs are supported. Files are deduplicated by SHA-256, associated with Google JSON across selected ZIPs, organized by year/month/type, and checkpointed for resume.
- Review all or pending media, filter by year and month, mark files to keep, move media and JSON to recoverable trash, and restore without overwriting existing files.
- Compare filename, Google, and original EXIF dates. Correct the date, move media and JSON together, update `photoTakenTime`, preserve its first original value in `gpexitOriginalPhotoTakenTime`, and record `gpexitDate` for portable corrections. Export a CSV using the native save dialog.
- Export annual ZIP64 archives, optionally protected with AES-256. Two matching password fields are required for encrypted export. Original input ZIP files remain unchanged.
- View directory sizes, file counts, free space on the gallery volume, task progress, and the latest 500 log lines. Logs are held in memory for the current application session.
- Cancel a running task. Closing the application requests cancellation and waits for task cleanup. Completed imported files remain available for resume; an incomplete annual export does not replace the previous ZIP.

Image thumbnails are generated lazily and cached outside the gallery. JPEG decoding is downsampled before resizing; preview generation is serialized to limit memory. Cached previews are limited to approximately 512 MiB and pruned by age. JPEG, PNG, GIF, WebP, TIFF, and BMP previews are supported. Other media can still be archived and reviewed by metadata when no decoder is available. Video playback depends on the platform WebView's codec support, as in the web version.

Dates use this priority: manual decisions, Google `photoTakenTime`, filename date, then supported original EXIF date when appropriate. Annual archive paths provide a fallback. Google timestamps use UTC; changing a calendar date retains the existing UTC clock or uses midnight when no valid clock exists.

## Assisted cleanup

Open **Pulizia assistita** to review four types of suggestions:

- **File grandi:** Photos and videos above the selected logical file size, largest first. This view is available immediately.
- **Scatti ravvicinati:** Photos with nearby capture times. The chosen maximum gap is applied within groups of at most 12 files and a 60-second span. Filename, Google UTC, and EXIF-local clocks are kept separate. Portable manual date corrections take priority over filename calendar dates.
- **Foto simili:** Approximate visual similarity from a 64-bit difference hash, compared against each group's reference image. Adjust the distance threshold to widen or narrow suggestions.
- **Dettaglio basso:** Low estimated thumbnail sharpness, measured with Laplacian variance. Adjust the threshold to review more or fewer candidates.

Click **Analizza le foto** for the visual views, follow progress in Activity, then return to cleanup. Analysis uses existing native image decoders; no models are downloaded and no photos leave the computer. Results, including unsupported decoder failures, are cached in `gallery/.gpexit-cleanup.sqlite3` by path, size, modification time, and algorithm version. Changed files need new analysis; unchanged files reuse previous results after restarting. This cache is derived data and can be rebuilt.

No file is preselected. Select individual files or explicitly select a group, review the selected count and logical byte total, then move up to 500 files to the recoverable trash. The move runs as a cancellable task, including hashing large videos, and reports moved, failed, and interrupted work. Completed moves remain recoverable in Gallery's Trash tab. Files marked **Tieni** are excluded and protected on the backend. A stale or invalid selection is rejected before any file is moved; files changing during processing are skipped. No permanent deletion is offered.

Suggestions require human review. Visual hashes do not establish that two photos have identical content. The search bounds candidate buckets and group sizes to avoid quadratic comparisons, so it can miss matches; near-uniform/low-texture hashes are excluded from similarity groups. Low thumbnail sharpness is not proof of blur: smooth backgrounds, illustrations, noise, and resolution affect scores. Burst timing does not establish that scenes are the same. This edition does not perform face identification or semantic categorization.

## Data and portability

The desktop application uses these gallery files:

```text
gallery/
  YYYY/MM/photos/...
  YYYY/MM/videos/...
  .gpexit-dates.sqlite3
  .gpexit-progress.sqlite3
  .gpexit-desktop.lock
  .gpexit-trash/<entry-id>/...
```

Trash is internal to the selected gallery so all three chosen directories can be on independent volumes. It is excluded from browsing and new annual exports. Its records prevent reimporting the same content until restored. Back up the entire gallery, including hidden files, to retain review state and recoverable removals. Previously generated ZIP files retain their old contents until regenerated.

Photo/JSON pairs remain portable without SQLite. Annual ZIPs include the manual-date manifest. Checkpoints use a content fingerprint and validate existing gallery files before resuming. Archives and date/keep databases created by earlier GPExit editions remain readable. A legacy sibling `trash` directory is separate from `.gpexit-trash` and is not relocated automatically.

The app configuration file on macOS is `~/Library/Application Support/it.gpexit.desktop/folders.json`. Thumbnail cache is under `~/Library/Caches/it.gpexit.desktop/previews`. `GPEXIT_CONFIG_DIR` and `GPEXIT_CACHE_DIR` can override these base directories for isolated testing.

## Build

Rust 1.91 or newer and the platform Tauri prerequisites are required. On macOS, install Xcode command line tools. Frontend assets are checked in; no npm installation or frontend build is needed.

```sh
cargo run --locked
```

Build the local macOS application:

```sh
scripts/bundle-macos.sh
```

The result is `target/release/bundle/macos/GPExit.app`. The script builds for the host architecture and applies an ad-hoc local signature. It does not install the app or notarize it for distribution. The verified build is macOS Apple Silicon. Windows and Linux require their normal Tauri platform dependencies and have not been verified on those operating systems.

## Verify

```sh
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt -- --check
```

The engine can also be tested without the desktop WebView:

```sh
cargo test --no-default-features --locked
```

Integration tests use temporary synthetic archives and cover AES/ZipCrypto, deduplication, metadata association, resume, collision handling, portable date changes, trash/restore, configuration persistence, exclusive locks, IPC routing, filters, cache invalidation, native previews, and task cancellation. The `archive_probe` and `performance_probe` examples provide headless interoperability and timing harnesses for development; they are not needed by the application.

## Architecture

- `config.rs`: Three-directory configuration and atomic persistence.
- `archive.rs`: Streaming input verification, bounded extraction workers, deduplication, metadata association, checkpoints, and annual ZIP export.
- `dates.rs`, `files.rs`, `gallery.rs`: Source dates, file safety, review decisions, corrections, trash, and statistics.
- `task.rs`: Serialized task lifecycle, logs, progress, and cancellation.
- `service.rs`: Desktop operations and cached gallery snapshots.
- `preview.rs`: Bounded image conversion and thumbnail cache.
- `main.rs`: Native window, folder/save dialogs, IPC, asset scopes, and shutdown.
- `ui/`: Local interface assets.
