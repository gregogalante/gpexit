"""Browse media and keep recoverable removal decisions outside the gallery."""
import json
from pathlib import Path
import re
import shutil
import uuid

from gallery import database, media_path


def kept_files(output):
    if not (output / '.gpexit-dates.sqlite3').exists():
        return {}
    with database(output) as connection:
        connection.execute('CREATE TABLE IF NOT EXISTS kept (path TEXT PRIMARY KEY, size INTEGER, modified INTEGER)')
        return {path: (size, modified) for path, size, modified in connection.execute('SELECT path, size, modified FROM kept')}


def keep_file(output, relative):
    path = media_path(output, relative)
    info = path.stat()
    with database(output) as connection:
        connection.execute('CREATE TABLE IF NOT EXISTS kept (path TEXT PRIMARY KEY, size INTEGER, modified INTEGER)')
        connection.execute('INSERT OR REPLACE INTO kept VALUES (?, ?, ?)',
                           (path.relative_to(output.resolve()).as_posix(), info.st_size, info.st_mtime_ns))


def gallery_items(output, review='pending'):
    from archive_tasks import archive_date_parts, media_kind
    if review not in ('pending', 'all'):
        raise ValueError('Invalid gallery filter.')
    kept = kept_files(output)
    for path in sorted(output.rglob('*')):
        if path.is_symlink() or not path.is_file() or not media_kind(path.name):
            continue
        relative = path.relative_to(output).as_posix()
        if Path(relative).parts[0] in ('archives', 'archivi'):
            continue
        info = path.stat()
        reviewed = kept.get(relative) == (info.st_size, info.st_mtime_ns)
        if review == 'all' or not reviewed:
            year, month = archive_date_parts(relative)
            yield {'path': relative, 'kind': media_kind(path.name), 'size': info.st_size,
                   'kept': reviewed, 'year': year, 'month': month}


def trash_record(root, entry_id):
    if not re.fullmatch(r'[0-9a-f]{32}', entry_id):
        raise ValueError('Invalid trash entry.')
    folder = root / 'trash' / entry_id
    if folder.is_symlink():
        raise ValueError('Invalid trash entry.')
    record = json.loads((folder / 'entry.json').read_text(encoding='utf-8'))
    path = media_path(folder, record['path'])
    return folder, path, record


def trash_items(root):
    for folder in sorted((root / 'trash').glob('*')):
        if not folder.is_dir() or folder.is_symlink():
            continue
        try:
            _, path, record = trash_record(root, folder.name)
        except (OSError, ValueError, KeyError, TypeError):
            continue
        yield {'id': folder.name, 'path': record['path'], 'size': path.stat().st_size}


def trashed_checksums(root):
    checksums = set()
    for item in trash_items(root):
        _, _, record = trash_record(root, item['id'])
        checksum = record.get('checksum', '')
        if isinstance(checksum, str) and re.fullmatch(r'[0-9a-f]{64}', checksum):
            checksums.add(checksum)
    return checksums


def trash_file(output, relative):
    from archive_tasks import file_hash
    path = media_path(output, relative)
    relative = path.relative_to(output.resolve()).as_posix()
    entry_id = uuid.uuid4().hex
    folder = output.parent / 'trash' / entry_id
    destination = folder / relative
    destination.parent.mkdir(parents=True)
    record = {'path': relative, 'checksum': file_hash(path)}
    (folder / 'entry.json').write_text(json.dumps(record), encoding='utf-8')
    sidecar = path.with_name(path.name + '.json')
    try:
        path.rename(destination)
        if sidecar.exists():
            sidecar.rename(destination.with_name(destination.name + '.json'))
    except OSError:
        if destination.exists():
            destination.rename(path)
        shutil.rmtree(folder)
        raise
    return entry_id


def restore_file(output, entry_id):
    folder, path, record = trash_record(output.parent, entry_id)
    destination = (output / record['path']).resolve()
    if not destination.is_relative_to(output.resolve()):
        raise ValueError('Invalid restore destination.')
    sidecar = path.with_name(path.name + '.json')
    new_sidecar = destination.with_name(destination.name + '.json')
    if destination.exists() or new_sidecar.exists():
        raise ValueError('File already exists in gallery; restore would overwrite it.')
    destination.parent.mkdir(parents=True, exist_ok=True)
    path.rename(destination)
    try:
        if sidecar.exists():
            sidecar.rename(new_sidecar)
    except OSError:
        destination.rename(path)
        raise
    shutil.rmtree(folder)
    return record['path']
