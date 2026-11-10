"""Measure local storage used by GPExit folders."""
import os
from pathlib import Path
import shutil


def folder_usage(folder):
    size = files = 0
    if folder.is_symlink():
        return {'bytes': size, 'files': files}
    for directory, folders, names in os.walk(folder, followlinks=False):
        parent = Path(directory)
        folders[:] = [name for name in folders if not (parent / name).is_symlink()]
        for name in names:
            path = parent / name
            try:
                if not path.is_symlink() and path.is_file():
                    size += path.stat().st_size
                    files += 1
            except FileNotFoundError:
                # A task may move or remove a file while storage is being measured.
                continue
    return {'bytes': size, 'files': files}


def storage_stats(root):
    result = {name: folder_usage(root / name) for name in ('inputs', 'gallery', 'exports', 'trash')}
    disk = shutil.disk_usage(root)
    result['disk'] = {'free': disk.free, 'total': disk.total}
    return result
