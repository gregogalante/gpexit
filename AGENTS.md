# Agent guide

## Project

GPExit is a standalone Rust/Tauri desktop application for importing Google Photos Takeout ZIP files, reviewing and cleaning a gallery, and exporting annual archives. The UI uses local HTML/CSS/JavaScript in the system WebView. Do not add a server or a separate language runtime.

## Architecture

- `src/archive.rs`: streaming input verification, bounded extraction workers, deduplication, metadata association, checkpoints, and annual ZIP export.
- `src/dates.rs`, `src/files.rs`, `src/gallery.rs`: source dates, safe file operations, review decisions, corrections, trash, and statistics.
- `src/config.rs`: three-directory configuration and atomic persistence.
- `src/cleanup.rs`: incremental local analysis and bounded assisted-cleanup suggestions.
- `src/task.rs`: serialized tasks, progress, logs, and cooperative cancellation.
- `src/service.rs`: desktop operations and cached gallery snapshots.
- `src/preview.rs`: bounded image conversion and cached thumbnails.
- `src/main.rs`: native window, folder/save dialogs, IPC, asset scopes, and shutdown.
- `ui/`: bundled frontend assets without a Node build step.
- `tests/`: temporary-fixture integration tests.

## Data and safety

Three user-selected, non-overlapping directories are persisted in platform settings. Never preselect personal directories or migrate their files silently. Keep personal archives, galleries, databases, and build artifacts out of Git.

Manual dates take precedence over inferred dates. Preserve original media when correcting dates. Update JSON photoTakenTime and retain its first original value in gpexitOriginalPhotoTakenTime; gpexitDate makes corrections portable without SQLite. Include decisions in annual exports.

Trash lives in gallery/.gpexit-trash. Exclude it from normal traversal and export, skip its hashes during import, and keep removals recoverable. All mutations are serialized. Closing the app cancels active work and waits for cleanup. Gallery locks prevent concurrent desktop instances.

## Validation and build

Use Rust 1.91 or newer and the checked-in Cargo.lock. Standard commands:

- `cargo test --locked`
- `cargo clippy --all-targets --locked -- -D warnings`
- `cargo fmt -- --check`
- `cargo test --no-default-features --locked` for the headless engine.
- `cargo run --locked` to launch the native application.
- `scripts/bundle-macos.sh` to build the local macOS app.

Use isolated GPEXIT_CONFIG_DIR and GPEXIT_CACHE_DIR directories for native UI tests. Use temporary media and stop owned app/test processes after validation. Do not modify the real gallery to test features. Keep target/ and gen/ ignored.
