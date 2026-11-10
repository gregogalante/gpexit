# GPExit — Google Photos Exit

GPExit builds a local, content-deduplicated photo and video gallery from Google Photos Takeout ZIP files. Its web interface runs on your computer and provides import tasks, annual ZIP exports, and visual review of conflicting dates.

## Requirements and installation

Python 3.10 or newer is required on macOS, Linux, or Windows.

```sh
python -m venv .venv
source .venv/bin/activate
python -m pip install -r requirements.txt
```

On Windows PowerShell, activate with `.venv\Scripts\Activate.ps1` instead.

## Web interface

Place your Takeout ZIP files in `inputs`, then run from the project directory:

```sh
python web_app.py
```

Open <http://127.0.0.1:8765>. Stop the server with Ctrl+C; stopping also terminates its active task. Completed imported files remain available for resume. Use `--port 8766` to change the port or `--root /path/to/archive` to select another data directory.

The interface provides:

- **Inputs and import:** List ZIP files and sizes, select inputs, supply an optional password, and import into the gallery. If inputs use different passwords, import them in separate batches.
- **Gallery and dates:** Browse photos and videos, mark files to keep, or move them with their JSON into a recoverable trash folder. Filter pending or all media by year and month, and restore removed files from the trash tab. Scan for conflicts between filename dates and Google dates, preview photos or videos, inspect available EXIF dates, and select a source or enter a custom date. Confirming the Google date marks the conflict as reviewed.
- **ZIP export:** Create or replace annual ZIP files, optionally protected by an AES-256 password entered twice for confirmation. Leave both fields empty to export without encryption.
- **Activity:** Follow task status and recent log output. A failed task displays its error; retry an import to resume completed work.

Gallery pages reuse an in-memory snapshot, invalidated after gallery mutations and completed tasks. Use the gallery refresh button after editing files outside GPExit. JPEG previews use decoder downsampling and at most two simultaneous conversions to limit memory.

The overview shows file counts and logical byte sizes for inputs, gallery, exports, and trash, plus free disk space. Sizes include metadata and databases, and exclude symbolic links. Refresh statistics manually or after a task completes; log polling does not repeatedly scan storage.

Only one task or gallery mutation runs at a time. Do not run the CLI or another web instance against the same gallery concurrently. The server binds exclusively to loopback and is intended for local use.

```text
inputs/       Original ZIP files
gallery/      Photos, videos, Google JSON, and local databases
exports/      Annual ZIP files
trash/        Recoverable removed media and metadata
```

Media is organized as `gallery/YYYY/MM/photos` or `gallery/YYYY/MM/videos`. Unknown dates use `Unknown year/Unknown month`. Inputs are never modified. Existing annual exports are replaced atomically when a new export succeeds; annual ZIP files are built one at a time.

## Gallery review and trash

"Keep" records a review decision in the local gallery database. Reviewed files are hidden from the pending filter and remain available under all media. Changing a file makes it pending again.

Moving to trash preserves media and JSON outside the gallery. New exports exclude these files, and import skips matching contents while their trash record exists. Restore refuses to overwrite an existing file. There is no permanent-delete action in the UI. Existing ZIP exports still contain their previous contents until regenerated. Back up `trash` as well if you want to preserve recoverable removals and their import exclusions.

## Date review and corrections

Dates normally follow this priority: Google `photoTakenTime`, a complete date in the filename, then original EXIF date for supported images. Previously exported annual archives provide their stored year and month as a fallback. ZIP filesystem dates and embedded video metadata are not used.

Filename recognition includes `IMG_20140607_154612`, `VID_20140607_154612`, `PXL_20210405_112233123`, `IMG-20140607-WA0001`, and `Screenshot_2021-12-03-14-22-00`. Dates must be in year-month-day order. Comparisons use calendar dates; Google timestamps are interpreted in UTC, so timezone differences can cause a one-day discrepancy. A filename date does not guarantee the capture date.

Review lists files with recognizable filename dates and valid Google dates that disagree. Files with missing or invalid metadata cannot be compared. Previews depend on browser and Pillow format support; unsupported formats display an unavailable-preview message.

A correction moves media and its Google JSON into the selected year/month folder. It updates `photoTakenTime` in the accompanying JSON and preserves its first original value in `gpexitOriginalPhotoTakenTime`. The chosen date is also stored in `gpexitDate`, so copying media and JSON preserves the correction without SQLite or an export manifest. The existing UTC time of day is retained; when unavailable, midnight UTC is used. Image content and EXIF remain unchanged. Manual decisions are also stored by SHA-256 content hash in `gallery/.gpexit-dates.sqlite3` and take precedence on subsequent imports. Reviewed files disappear from the review list. Annual exports include a `.gpexit-dates.json` manifest so rebuilding a gallery from those exports preserves decisions. Existing local decisions take precedence over imported manifests. Keep the entire gallery, including hidden files, when backing it up.

To produce a raw CSV conflict report, independently of review decisions:

```sh
python check_dates.py --output gallery > date-conflicts.csv
```

The CSV contains relative paths, filename dates, and Google dates. A summary goes to standard error; invalid metadata produces a nonzero exit status. This command only reads files.

## Command line

The CLI uses the same import and export functions:

```sh
python gpexit.py
python gpexit.py inputs/first.zip inputs/second.zip --output gallery --exports exports
python gpexit.py --workers 2
```

It prompts for input passwords and whether to create annual ZIP files. By default, it uses up to four worker processes. Identical SHA-256 contents are deduplicated; distinct files with the same name receive a hash suffix. Matching Google JSON is copied alongside media. Original album folders are not recreated.

Import progress is stored in `gallery/.gpexit-progress.sqlite3`. Each run hashes input ZIP contents and existing gallery files to validate resume state. This reads substantial data on large archives. Processing uses up to one temporary media file per worker; exporting needs space for both old and new annual ZIP files until replacement succeeds. Keep original exports until you have verified the gallery.

Passwords are held only for the task and are not saved. AES-encrypted ZIP files require a compatible reader, such as 7-Zip.

## Development

```sh
python -m unittest discover -s tests
```

`gpexit.py` is the CLI entry point; `archive_tasks.py` owns import, deduplication, checkpoints, and ZIP export. `dates.py` parses source dates. `gallery.py` handles date review and manual corrections. `gallery_review.py` handles browsing, keep decisions, and recoverable removal; `storage.py` measures directory sizes. `web_app.py` serves the UI in `web/index.html` and serializes mutations; `web_worker.py` isolates long-running tasks.

Personal data directories, ZIP files, and SQLite databases are ignored by Git. Add custom data directories to `.gitignore` before publishing.
