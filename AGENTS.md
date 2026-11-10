# Agent guide

## Project

GPExit is a local Python application for importing Google Photos Takeout ZIP files, reviewing capture dates, and exporting annual archives. See README.md for installation and user commands.

## Architecture

- `archive_tasks.py`: import, deduplication, checkpoints, and ZIP export.
- `dates.py`: filename, Google, and EXIF date parsing.
- `gallery.py`: visual review data and persistent manual decisions keyed by SHA-256.
- `gallery_review.py`: browsing, keep decisions, recoverable trash, and restore.
- `storage.py`: logical file sizes and free disk capacity.
- `web_app.py`: loopback HTTP API and serialized gallery mutations.
- `web_worker.py`: subprocess entry point for long-running tasks.
- `web/index.html`: self-contained web interface without external assets.
- `gpexit.py` and `check_dates.py`: command-line entry points.

Manual decisions take precedence over inferred dates. Preserve original media when correcting dates. Update JSON photoTakenTime and retain its first original value in gpexitOriginalPhotoTakenTime; gpexitDate makes manual decisions portable without SQLite. Include decisions in annual exports. Keep personal gallery and trash files out of Git. Removed media must remain recoverable, excluded from new exports, and skipped on import until restored.

## Validation

Run `python -m unittest discover -s tests`. Use temporary directories for import/export and correction tests. Stop preview servers and subprocesses after verification.

## Local environment

Use managed Python through Conda; source `~/.zshrc` and activate `base` before running Python here. `.venv` provides the installed project dependencies.
