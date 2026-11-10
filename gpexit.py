#!/usr/bin/env python3
"""Build a cumulative, content-deduplicated Google Photos archive."""
import argparse
from collections import deque
from concurrent.futures import ProcessPoolExecutor, TimeoutError
import datetime as dt
import getpass
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import sqlite3
import time
import sys
import tempfile

import pyzipper
from PIL import Image

PHOTO = {'.jpg', '.jpeg', '.png', '.gif', '.webp', '.heic', '.heif', '.avif', '.bmp', '.tif', '.tiff', '.dng', '.raw', '.arw', '.cr2', '.cr3', '.nef', '.orf', '.raf', '.rw2'}
VIDEO = {'.mp4', '.mov', '.avi', '.m4v', '.mpg', '.mpeg', '.3gp', '.3g2', '.mkv', '.webm', '.mts', '.m2ts', '.wmv'}
CHUNK = 1024 * 1024


def media_kind(name):
    suffix = Path(name).suffix.lower()
    return 'photos' if suffix in PHOTO else 'videos' if suffix in VIDEO else None


def filename_date(name):
    for match in re.finditer(r'(?<!\d)((?:19|20)\d{2})[-_.]?(\d{2})[-_.]?(\d{2})(?=(?:\d{6}(?:\d{3})?)?(?!\d))', name):
        try:
            return dt.date(*map(int, match.groups()))
        except ValueError:
            continue
    return None


def filename_year(name):
    date = filename_date(name)
    return str(date.year) if date else None


def choose_date(filename, metadata, embedded_date=None):
    try:
        timestamp = int(metadata.get('photoTakenTime', {}).get('timestamp', 0))
        if timestamp > 0:
            date = dt.datetime.fromtimestamp(timestamp, dt.timezone.utc).date()
            if 1900 <= date.year <= dt.datetime.now(dt.timezone.utc).year + 1:
                return date
    except (ValueError, TypeError, OSError, OverflowError, AttributeError):
        pass
    return filename_date(filename) or embedded_date


def choose_year(filename, metadata, embedded_year=None):
    date = choose_date(filename, metadata)
    return str(date.year) if date else embedded_year


def embedded_date(path):
    try:
        with Image.open(path) as image:
            exif = image.getexif()
            date = exif.get(36867) or exif.get(36868)
            if not date:
                date = exif.get_ifd(34665).get(36867)
            if date and re.match(r'^(19|20)\d{2}:\d{2}:\d{2}', str(date)):
                return dt.datetime.strptime(str(date)[:10], '%Y:%m:%d').date()
    except (OSError, ValueError, TypeError, AttributeError, KeyError, SyntaxError):
        pass
    return None


def file_hash(path, label=None):
    digest = hashlib.sha256()
    total = path.stat().st_size
    size = 0
    last_update = time.monotonic()
    with path.open('rb') as source:
        for chunk in iter(lambda: source.read(CHUNK), b''):
            digest.update(chunk)
            size += len(chunk)
            if label and time.monotonic() - last_update >= 5:
                print(f'  ⏳ {label}: {size / max(total, 1):.0%} checked', flush=True)
                last_update = time.monotonic()
    return digest.hexdigest()


def fingerprint_inputs(paths):
    fingerprints = []
    for number, path in enumerate(paths, 1):
        before = path.stat()
        print(f'🔍 Checking input {number}/{len(paths)}: {path.name}', flush=True)
        digest = file_hash(path, path.name)
        after = path.stat()
        if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
            raise RuntimeError(f'Input changed while being checked: {path}')
        fingerprints.append((str(path.resolve()), digest))
    return hashlib.sha256(json.dumps(fingerprints).encode()).hexdigest()


def open_checkpoint(output):
    database = sqlite3.connect(output / '.gpexit-progress.sqlite3')
    database.execute('CREATE TABLE IF NOT EXISTS completed (session TEXT NOT NULL, archive TEXT NOT NULL, member INTEGER NOT NULL, checksum TEXT NOT NULL, PRIMARY KEY (session, archive, member))')
    database.commit()
    return database


def save_checkpoint(database, session, archive_path, member, checksum):
    database.execute('INSERT OR REPLACE INTO completed VALUES (?, ?, ?, ?)', (session, str(archive_path.resolve()), member, checksum))
    database.commit()


def open_member(archive, info, passwords, archive_path):
    if not info.flag_bits & 1:
        return archive.open(info)
    while True:
        password = passwords.get(archive_path, passwords.get(None))
        if password is None:
            password = getpass.getpass(f'Password for {archive_path.name}: ').encode('utf-8')
        try:
            stream = archive.open(info, pwd=password)
            passwords[archive_path] = password
            passwords[None] = password
            return stream
        except RuntimeError as exc:
            if 'password' not in str(exc).lower():
                raise
            print('Incorrect password; try again.', file=sys.stderr)
            passwords.pop(archive_path, None)
            passwords.pop(None, None)


def sidecar_key(info, data):
    path = PurePosixPath(info.filename)
    title = data.get('title')
    if not isinstance(title, str) or not title or '/' in title or '\\' in title:
        return None
    return (str(path.parent).casefold(), title.casefold())


def collect_metadata(paths, passwords):
    records = {}
    direct_records = {}
    for archive_path in paths:
        print(f'  🗂️ Reading metadata: {archive_path.name}', flush=True)
        with pyzipper.AESZipFile(archive_path) as archive:
            last_update = time.monotonic()
            json_count = 0
            for info in archive.infolist():
                if not info.filename.lower().endswith('.json') or info.file_size > 4 * CHUNK:
                    continue
                json_count += 1
                if time.monotonic() - last_update >= 5:
                    print(f'  🗂️ Read {json_count} metadata files from {archive_path.name}', flush=True)
                    last_update = time.monotonic()
                with open_member(archive, info, passwords, archive_path) as stream:
                    raw = stream.read(4 * CHUNK + 1)
                try:
                    data = json.loads(raw)
                except (ValueError, UnicodeDecodeError):
                    continue
                if isinstance(data, dict):
                    key = sidecar_key(info, data)
                    if key:
                        records.setdefault(key, (data, raw))
                    sidecar = PurePosixPath(info.filename)
                    media_name = sidecar.name[:-5]
                    if media_kind(media_name):
                        direct_records.setdefault((str(sidecar.parent).casefold(), media_name.casefold()), (data, raw))
    records.update(direct_records)
    return records


def archive_date_parts(path):
    parts = PurePosixPath(path).parts
    for index, part in enumerate(parts[:-2]):
        if re.fullmatch(r'(?:19|20)\d{2}', part) or part in ('Senza data', 'Unknown year'):
            remaining = parts[index + 1:-1]
            if remaining and remaining[0] in ('foto', 'video', 'photos', 'videos'):
                return ('Unknown year' if part == 'Senza data' else part), 'Unknown month'
            if len(remaining) >= 2 and remaining[1] in ('foto', 'video', 'photos', 'videos'):
                month = remaining[0]
                if month in {f'{number:02d}' for number in range(1, 13)} or month == 'Unknown month':
                    return ('Unknown year' if part == 'Senza data' else part), month
    return None, None


def safe_filename(name):
    name = re.sub(r'[<>:"\\|?*\x00-\x1f]', '_', name).rstrip(' .')
    if re.match(r'^(CON|PRN|AUX|NUL|COM[1-9]|LPT[1-9])(?:\.|$)', name, re.IGNORECASE):
        name = '_' + name
    return name or '_'


def destination_for(folder, name, checksum):
    folder.mkdir(parents=True, exist_ok=True)
    destination = folder / name
    if destination.exists():
        destination = folder / f'{Path(name).stem}_{checksum[:16]}{Path(name).suffix}'
        if destination.exists():
            raise FileExistsError(f'Filename collision: {destination}')
    return destination


def move_existing(existing, folder, checksum):
    if existing.parent == folder:
        return existing
    destination = destination_for(folder, existing.name, checksum)
    old_sidecar = existing.with_name(existing.name + '.json')
    new_sidecar = destination.with_name(destination.name + '.json')
    if old_sidecar.exists() and new_sidecar.exists():
        raise FileExistsError(f'Metadata collision: {new_sidecar}')
    os.replace(existing, destination)
    if old_sidecar.exists():
        os.replace(old_sidecar, new_sidecar)
    return destination


def index_output(output):
    index = {}
    files = [path for path in output.rglob('*') if path.is_file() and media_kind(path.name) and path.relative_to(output).parts[0] not in ('archivi', 'archives')]
    print(f'📚 Checking {len(files)} existing media files for duplicates...', flush=True)
    last_update = time.monotonic()
    for number, path in enumerate(files, 1):
        if number == 1 or number % 100 == 0 or time.monotonic() - last_update >= 5:
            print(f'  📚 Checked {number - 1}/{len(files)} existing files', flush=True)
            last_update = time.monotonic()
        checksum = file_hash(path, path.name)
        year, month = archive_date_parts(path.relative_to(output).as_posix())
        if year is not None:
            sidecar = path.with_name(path.name + '.json')
            try:
                metadata = json.loads(sidecar.read_text(encoding='utf-8')) if sidecar.exists() else {}
            except (OSError, UnicodeError, ValueError):
                metadata = {}
            date = choose_date(path.name, metadata)
            if date is None and media_kind(path.name) == 'photos':
                date = embedded_date(path)
            if date and (year == 'Unknown year' or str(date.year) == year):
                year, month = str(date.year), f'{date.month:02d}'
            path = move_existing(path, output / year / month / media_kind(path.name), checksum)
        index.setdefault(checksum, path)
    return index


def save_sidecar(destination, raw):
    if raw is None:
        return
    sidecar = destination.with_name(destination.name + '.json')
    if sidecar.exists():
        return
    fd, name = tempfile.mkstemp(prefix='.metadata-', dir=destination.parent)
    temp = Path(name)
    try:
        with os.fdopen(fd, 'wb') as output:
            output.write(raw)
        os.replace(temp, sidecar)
    finally:
        temp.unlink(missing_ok=True)


def available_cpus():
    if hasattr(os, 'process_cpu_count'):
        return os.process_cpu_count()
    if hasattr(os, 'sched_getaffinity'):
        return len(os.sched_getaffinity(0))
    return os.cpu_count()


def worker_count(cpu_count):
    return max(1, min(cpu_count or 1, 4))


_main_pid = os.getpid()
_worker_archive = None
_worker_archive_path = None


def close_worker_archive():
    global _worker_archive, _worker_archive_path
    if _worker_archive is not None:
        _worker_archive.close()
        _worker_archive = None
        _worker_archive_path = None


def extract_member(archive_path, member_index, password, staging):
    global _worker_archive, _worker_archive_path
    if _worker_archive_path != archive_path:
        close_worker_archive()
        _worker_archive = pyzipper.AESZipFile(archive_path)
        _worker_archive_path = archive_path
    info = _worker_archive.infolist()[member_index]
    fd, temp_name = tempfile.mkstemp(prefix='file-', dir=staging)
    temp = Path(temp_name)
    digest = hashlib.sha256()
    try:
        with os.fdopen(fd, 'wb') as target, _worker_archive.open(info, pwd=password) as source:
            last_update = time.monotonic()
            size = 0
            for chunk in iter(lambda: source.read(CHUNK), b''):
                if (Path(staging) / '.stop').exists():
                    raise InterruptedError('Import stopped')
                digest.update(chunk)
                target.write(chunk)
                size += len(chunk)
                if os.getpid() == _main_pid and time.monotonic() - last_update >= 5:
                    print(f'  ⏳ {info.filename}: {size / max(info.file_size, 1):.0%}', flush=True)
                    last_update = time.monotonic()
        return temp, digest.hexdigest()
    except BaseException:
        temp.unlink(missing_ok=True)
        raise


def wait_for_result(future, info, pending, staging):
    while True:
        try:
            return future.result(timeout=5)
        except TimeoutError:
            staged = sum(path.stat().st_size for path in Path(staging).glob('file-*') if path.exists())
            print(f'  ⏳ Still working on {PurePosixPath(info.filename).name}; {len(pending) + 1} files in progress, {staged / 1048576:.1f} MiB temporarily written', flush=True)


def extracted_members(archive, archive_path, passwords, staging, workers, executor, counts, completed, index, records):
    pending = deque()
    total = sum(1 for info in archive.infolist() if not info.is_dir() and media_kind(PurePosixPath(info.filename).name))
    seen = 0
    last_update = time.monotonic()
    for member_index, info in enumerate(archive.infolist()):
        if info.is_dir():
            continue
        source_path = PurePosixPath(info.filename)
        name = source_path.name
        if not media_kind(name):
            if not name.lower().endswith('.json'):
                counts['unrecognized'] += 1
                print(f'⚠️ Skipped unsupported file: {archive_path.name}: {info.filename}', file=sys.stderr)
            continue
        seen += 1
        if seen == 1 or seen % 25 == 0 or time.monotonic() - last_update >= 5:
            print(f'  📸 Checking file {seen} of {total} in {archive_path.name} ({counts["resumed"]} already saved)', flush=True)
            last_update = time.monotonic()
        if name in ('', '.', '..') or '\\' in name:
            raise ValueError(f'Invalid filename: {info.filename!r}')
        checksum = completed.get(member_index)
        previous = index.get(checksum) if checksum else None
        metadata = records.get((str(source_path.parent).casefold(), name.casefold()))
        if previous and (metadata is None or previous.with_name(previous.name + '.json').exists()):
            counts['resumed'] += 1
            continue
        password = None
        if info.flag_bits & 1:
            with open_member(archive, info, passwords, archive_path):
                password = passwords[archive_path]
        args = (archive_path, member_index, password, staging)
        if executor is None:
            yield member_index, info, extract_member(*args)
            continue
        pending.append((member_index, info, executor.submit(extract_member, *args)))
        if len(pending) >= workers:
            member, current, future = pending.popleft()
            yield member, current, wait_for_result(future, current, pending, staging)
    while pending:
        member, current, future = pending.popleft()
        yield member, current, wait_for_result(future, current, pending, staging)


def import_archives(paths, output, workers=None):
    workers = worker_count(available_cpus()) if workers is None else workers
    if workers < 1:
        raise ValueError('Worker count must be at least 1.')
    print('👋 GPExit is starting. Press Ctrl+C to stop safely and resume later.', flush=True)
    print('🔐 Checking ZIP contents so completed work can be safely resumed...', flush=True)
    session = fingerprint_inputs(paths)
    passwords = {}
    print('🗂️ Reading Google Photos metadata...', flush=True)
    records = collect_metadata(paths, passwords)
    index = index_output(output)
    database = open_checkpoint(output)
    counts = {'imported': 0, 'duplicates': 0, 'unknown date': 0, 'unrecognized': 0, 'resumed': 0}
    print(f'🚀 Starting import with {workers} worker(s).', flush=True)
    with tempfile.TemporaryDirectory(prefix='.import-', dir=output) as staging:
        executor = ProcessPoolExecutor(max_workers=workers) if workers > 1 else None
        try:
            for archive_path in paths:
                print(f'📦 Importing: {archive_path.name}', flush=True)
                completed = dict(database.execute('SELECT member, checksum FROM completed WHERE session = ? AND archive = ?', (session, str(archive_path.resolve()))))
                with pyzipper.AESZipFile(archive_path) as archive:
                    entries = extracted_members(archive, archive_path, passwords, staging, workers, executor, counts, completed, index, records)
                    for member_index, info, (temp, checksum) in entries:
                        source_path = PurePosixPath(info.filename)
                        name = source_path.name
                        kind = media_kind(name)
                        metadata, raw = records.get((str(source_path.parent).casefold(), name.casefold()), ({}, None))
                        try:
                            date = choose_date(name, metadata)
                            archived_year, archived_month = archive_date_parts(info.filename)
                            if date is None and kind == 'photos' and archived_month in (None, 'Unknown month'):
                                date = embedded_date(temp)
                            year = str(date.year) if date else archived_year or 'Unknown year'
                            month = f'{date.month:02d}' if date else archived_month or 'Unknown month'
                            if checksum in index:
                                existing = index[checksum]
                                existing_year, existing_month = archive_date_parts(existing.relative_to(output).as_posix())
                                if (existing_year == 'Unknown year' and year != 'Unknown year') or (existing_year == year and existing_month == 'Unknown month' and month != 'Unknown month'):
                                    index[checksum] = move_existing(existing, output / year / month / kind, checksum)
                                save_sidecar(index[checksum], raw)
                                counts['duplicates'] += 1
                                save_checkpoint(database, session, archive_path, member_index, checksum)
                                continue
                            destination = destination_for(output / year / month / kind, safe_filename(name), checksum)
                            os.replace(temp, destination)
                            index[checksum] = destination
                            save_sidecar(destination, raw)
                            counts['imported'] += 1
                            counts['unknown date'] += year == 'Unknown year'
                            save_checkpoint(database, session, archive_path, member_index, checksum)
                        finally:
                            temp.unlink(missing_ok=True)
        except BaseException:
            print('🛑 Stopping workers; completed files are saved for the next run.', flush=True)
            (Path(staging) / '.stop').touch()
            raise
        finally:
            if executor is not None:
                executor.shutdown(wait=True, cancel_futures=True)
            else:
                close_worker_archive()
            database.close()
    return counts


def ask_yes(prompt):
    while True:
        answer = input(prompt + ' [y/n]: ').strip().lower()
        if answer in ('y', 'yes'):
            return True
        if answer in ('n', 'no'):
            return False


def write_archives(output, password):
    archive_dir = output / 'archives'
    archive_dir.mkdir(exist_ok=True)
    years = sorted(path for path in output.iterdir() if path.is_dir() and (re.fullmatch(r'(?:19|20)\d{2}', path.name) or path.name == 'Unknown year'))
    for year in years:
        destination = archive_dir / f'{year.name}.zip'
        fd, temp_name = tempfile.mkstemp(prefix='.zip-', suffix='.tmp', dir=archive_dir)
        os.close(fd)
        temp = Path(temp_name)
        print(f'📦 Creating {destination.name}…', flush=True)
        try:
            with pyzipper.AESZipFile(temp, 'w', compression=pyzipper.ZIP_STORED, allowZip64=True) as archive:
                if password:
                    archive.setpassword(password)
                    archive.setencryption(pyzipper.WZ_AES, nbits=256)
                for path in sorted(year.rglob('*')):
                    if path.is_file():
                        archive.write(path, path.relative_to(output).as_posix())
            os.replace(temp, destination)
        finally:
            temp.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description='GPExit - Google Photos Exit: incremental photo and video archive')
    parser.add_argument('zip', nargs='*', type=Path, help='ZIP files to import (default: inputs/*.zip)')
    parser.add_argument('--workers', type=int, default=worker_count(available_cpus()), help='Parallel workers (default: available CPUs, up to 4)')
    parser.add_argument('--output', type=Path, default=Path('output'), help='Cumulative output directory (default: output)')
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
        write_archives(args.output, password)
    print(f'✅ Archive available at: {args.output.resolve()}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, RuntimeError, EOFError, KeyboardInterrupt, pyzipper.BadZipFile, sqlite3.Error) as exc:
        print(f'🛑 Operation interrupted: {exc}', file=sys.stderr)
        sys.exit(1)
