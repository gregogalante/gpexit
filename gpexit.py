#!/usr/bin/env python3
"""Command-line entry point for GPExit archive tasks."""
import argparse
import getpass
from pathlib import Path
import sqlite3
import sys

import pyzipper

# Preserve the original Python API while command-line and web clients share tasks.
from archive_tasks import (available_cpus, configure_output, extract_member,
                           import_archives, media_kind, safe_filename,
                           worker_count, write_archives, ask_yes)
from dates import choose_date, choose_year, embedded_date, filename_date, filename_year


def main():
    configure_output()
    parser = argparse.ArgumentParser(description='GPExit - Google Photos Exit: incremental photo and video archive')
    parser.add_argument('zip', nargs='*', type=Path, help='ZIP files to import (default: inputs/*.zip)')
    parser.add_argument('--workers', type=int, default=worker_count(available_cpus()), help='Parallel workers (default: available CPUs, up to 4)')
    parser.add_argument('--output', type=Path, default=Path('gallery'), help='Gallery directory (default: gallery)')
    parser.add_argument('--exports', type=Path, default=Path('exports'), help='Annual ZIP directory (default: exports)')
    args = parser.parse_args()
    if args.workers < 1:
        parser.error('--workers must be at least 1')
    paths = args.zip or sorted(Path('inputs').glob('*.zip'))
    if not paths:
        parser.error('No ZIP files found.')
    for path in paths:
        if not path.is_file():
            parser.error(f'File not found: {path}')
    args.output.mkdir(parents=True, exist_ok=True)
    counts = import_archives(paths, args.output, args.workers)
    print(f'✅ Import complete: {counts["imported"]} new, {counts["duplicates"]} duplicates, {counts["resumed"]} previously completed, {counts["unknown date"]} without a year, {counts["unrecognized"]} unsupported.')
    if ask_yes('Create or update yearly ZIP files?'):
        password = None
        if ask_yes('Protect ZIP files with an AES-256 password?'):
            while True:
                password = getpass.getpass('New password: ')
                confirmation = getpass.getpass('Repeat password: ')
                if password and password == confirmation:
                    password = password.encode('utf-8')
                    break
                print('Password is empty or does not match; try again.')
        write_archives(args.output, password, args.exports)
    print(f'✅ Archive available at: {args.output.resolve()}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, EOFError, KeyboardInterrupt, pyzipper.BadZipFile, sqlite3.Error) as exc:
        print(f'🛑 Operation interrupted: {exc}', file=sys.stderr)
        sys.exit(1)
