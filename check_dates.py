#!/usr/bin/env python3
"""Report conflicts between filename dates and Google Photos metadata."""
import argparse
import csv
import json
from pathlib import Path
import sys

from archive_tasks import configure_output, media_kind
from dates import filename_date, google_date


def check_dates(output, writer):
    checked = conflicts = missing = invalid = 0
    writer.writerow(('file', 'filename_date', 'google_date'))
    for path in sorted(output.rglob('*')):
        if not path.is_file() or not media_kind(path.name):
            continue
        if path.relative_to(output).parts[0] in ('archives', 'archivi'):
            continue
        name_date = filename_date(path.name)
        if name_date is None:
            continue
        sidecar = path.with_name(path.name + '.json')
        try:
            metadata = json.loads(sidecar.read_text(encoding='utf-8'))
        except FileNotFoundError:
            missing += 1
            continue
        except (OSError, UnicodeError, ValueError) as exc:
            print(f'Cannot read {sidecar}: {exc}', file=sys.stderr)
            invalid += 1
            continue
        date = google_date(metadata)
        if date is None:
            invalid += 1
            continue
        checked += 1
        if name_date != date:
            conflicts += 1
            writer.writerow((str(path.relative_to(output)), name_date.isoformat(), date.isoformat()))
    return checked, conflicts, missing, invalid


def main():
    configure_output()
    parser = argparse.ArgumentParser(description='Report filename dates that differ from Google Photos dates as CSV.')
    parser.add_argument('--output', type=Path, default=Path('gallery'), help='GPExit output directory (default: gallery)')
    args = parser.parse_args()
    if not args.output.is_dir():
        parser.error(f'Output directory not found: {args.output}')
    checked, conflicts, missing, invalid = check_dates(args.output, csv.writer(sys.stdout))
    print(f'Compared: {checked}; conflicts: {conflicts}; missing JSON: {missing}; invalid or missing Google date: {invalid}', file=sys.stderr)
    if invalid:
        return 1
    return 0


if __name__ == '__main__':
    sys.exit(main())
