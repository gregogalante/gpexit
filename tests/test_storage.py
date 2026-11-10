from pathlib import Path
import shutil
import tempfile
import unittest

from storage import storage_stats


class StorageTests(unittest.TestCase):
    def test_counts_actual_file_bytes_recursively(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            for folder in ('inputs', 'gallery', 'exports', 'trash'):
                directory = root / folder
                directory.mkdir()
                (directory / 'first.bin').write_bytes(b'abc')
                nested = directory / 'nested'
                nested.mkdir()
                (nested / 'second.bin').write_bytes(b'12345')
                (nested / 'empty.bin').touch()
            stats = storage_stats(root)
            for folder in ('inputs', 'gallery', 'exports', 'trash'):
                self.assertEqual(stats[folder], {'bytes': 8, 'files': 3})
            disk = shutil.disk_usage(root)
            self.assertEqual(stats['disk']['total'], disk.total)
            self.assertGreater(stats['disk']['free'], 0)
            self.assertLessEqual(stats['disk']['free'], stats['disk']['total'])

    def test_missing_folders_and_symlinks_are_excluded(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            outside = root / 'outside'
            outside.mkdir()
            (outside / 'large.bin').write_bytes(b'123456789')
            gallery = root / 'gallery'
            gallery.mkdir()
            (gallery / 'real.bin').write_bytes(b'abc')
            (gallery / 'file-link.bin').symlink_to(outside / 'large.bin')
            (gallery / 'directory-link').symlink_to(outside, target_is_directory=True)
            (gallery / 'dangling-link').symlink_to(root / 'missing')
            (root / 'inputs').symlink_to(outside, target_is_directory=True)
            stats = storage_stats(root)
            self.assertEqual(stats['gallery'], {'bytes': 3, 'files': 1})
            for folder in ('inputs', 'exports', 'trash'):
                self.assertEqual(stats[folder], {'bytes': 0, 'files': 0})


if __name__ == '__main__':
    unittest.main()
