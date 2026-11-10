"""Inspect gallery dates and persist manual decisions independently of Google metadata."""
import datetime as dt
from contextlib import contextmanager
import json
import os
from pathlib import Path
import sqlite3
import tempfile

from dates import embedded_date, filename_date, google_date


@contextmanager
def database(output):
    connection = sqlite3.connect(output / '.gpexit-dates.sqlite3')
    try:
        with connection:
            connection.execute('CREATE TABLE IF NOT EXISTS decisions (checksum TEXT PRIMARY KEY, date TEXT NOT NULL)')
            yield connection
    finally:
        connection.close()


def load_overrides(output):
    if not (output / '.gpexit-dates.sqlite3').exists():
        return {}
    with database(output) as connection:
        return dict(connection.execute('SELECT checksum, date FROM decisions'))


def media_path(output, relative):
    from archive_tasks import media_kind
    path = (output / relative).resolve()
    if not path.is_relative_to(output.resolve()) or not path.is_file() or not media_kind(path.name):
        raise ValueError('File not found in gallery.')
    if path.relative_to(output.resolve()).parts[0] in ('archives', 'archivi'):
        raise ValueError('File is outside the gallery media folders.')
    return path


def conflicts(output):
    from archive_tasks import archive_date_parts, file_hash, media_kind
    decisions = load_overrides(output)
    for path in sorted(output.rglob('*')):
        if not path.is_file() or not media_kind(path.name) or path.is_symlink():
            continue
        if path.relative_to(output).parts[0] in ('archives', 'archivi'):
            continue
        name_date = filename_date(path.name)
        if name_date is None:
            continue
        sidecar = path.with_name(path.name + '.json')
        try:
            metadata = json.loads(sidecar.read_text(encoding='utf-8'))
        except (OSError, UnicodeError, ValueError):
            continue
        date = google_date(metadata)
        if date is None or date == name_date:
            continue
        if metadata.get('gpexitDate') == date.isoformat():
            continue
        if decisions and file_hash(path) in decisions:
            continue
        year, month = archive_date_parts(path.relative_to(output).as_posix())
        exif = embedded_date(path) if media_kind(path.name) == 'photos' else None
        yield {'path': path.relative_to(output).as_posix(), 'filename_date': name_date.isoformat(),
               'google_date': date.isoformat(), 'exif_date': exif.isoformat() if exif else None,
               'current_date': date.isoformat() if (year, month) == (str(date.year), f'{date.month:02d}') else None,
               'kind': media_kind(path.name)}


def update_sidecar_date(path, value):
    """Keep a portable decision and the first original Google timestamp."""
    date = dt.date.fromisoformat(value)
    sidecar = path.with_name(path.name + '.json')
    metadata = json.loads(sidecar.read_text(encoding='utf-8')) if sidecar.exists() else {'title': path.name}
    if not isinstance(metadata, dict):
        raise ValueError('Google metadata must be a JSON object.')
    if metadata.get('gpexitDate') == value and google_date(metadata) == date:
        return
    original = metadata.get('photoTakenTime')
    clock = dt.time()
    if google_date(metadata):
        clock = dt.datetime.fromtimestamp(int(original['timestamp']), dt.timezone.utc).time()
    corrected = dt.datetime.combine(date, clock, dt.timezone.utc)
    metadata.setdefault('gpexitOriginalPhotoTakenTime', original)
    metadata['photoTakenTime'] = {'timestamp': str(int(corrected.timestamp())),
                                'formatted': corrected.strftime('%Y-%m-%d %H:%M:%S UTC')}
    metadata['gpexitDate'] = date.isoformat()
    fd, name = tempfile.mkstemp(prefix='.metadata-', dir=path.parent)
    temp = Path(name)
    try:
        with os.fdopen(fd, 'w', encoding='utf-8') as stream:
            json.dump(metadata, stream, ensure_ascii=False, indent=2)
            stream.write('\n')
        os.replace(temp, sidecar)
    finally:
        temp.unlink(missing_ok=True)


def correct_date(output, relative, value):
    from archive_tasks import file_hash, media_kind, move_existing
    date = dt.date.fromisoformat(value)
    if not 1900 <= date.year <= dt.date.today().year + 1:
        raise ValueError('Date must be between 1900 and next year.')
    path = media_path(output, relative)
    checksum = file_hash(path)
    with database(output) as connection:
        connection.execute('INSERT OR REPLACE INTO decisions VALUES (?, ?)', (checksum, date.isoformat()))
        update_sidecar_date(path, date.isoformat())
        destination = move_existing(path, output / str(date.year) / f'{date.month:02d}' / media_kind(path.name), checksum)
    return destination.relative_to(output).as_posix()
