import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import zipfile

from archive_tasks import import_archives, write_archives
from gallery_review import (gallery_items, keep_file, kept_files, restore_file,
                            trash_file, trash_items, trash_record, trashed_checksums)


class GalleryReviewTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.gallery = self.root / 'gallery'
        self.relative = '2014/06/photos/IMG_20140607_154612.jpg'
        self.photo = self.gallery / self.relative
        self.photo.parent.mkdir(parents=True)
        self.content = b'original photo content'
        self.photo.write_bytes(self.content)
        self.sidecar = self.photo.with_name(self.photo.name + '.json')
        self.metadata = b'{"title": "IMG_20140607_154612.jpg", "description": "original"}'
        self.sidecar.write_bytes(self.metadata)

    def test_kept_decision_persists_and_changed_file_needs_review(self):
        self.assertEqual([item['path'] for item in gallery_items(self.gallery)], [self.relative])
        keep_file(self.gallery, self.relative)
        self.assertEqual(list(gallery_items(self.gallery)), [])
        # Reopening via a new Path instance must read the persisted decision.
        reopened = Path(str(self.gallery))
        self.assertIn(self.relative, kept_files(reopened))
        items = list(gallery_items(reopened, 'all'))
        self.assertEqual(len(items), 1)
        self.assertTrue(items[0]['kept'])
        self.photo.write_bytes(b'changed photo with a different size')
        pending = list(gallery_items(reopened))
        self.assertEqual([item['path'] for item in pending], [self.relative])
        self.assertFalse(pending[0]['kept'])
        with self.assertRaises(ValueError):
            list(gallery_items(reopened, 'unknown-filter'))

    def test_trash_and_restore_preserve_media_and_json_bytes(self):
        entry = trash_file(self.gallery, self.relative)
        self.assertFalse(self.photo.exists())
        self.assertFalse(self.sidecar.exists())
        self.assertEqual(list(gallery_items(self.gallery, 'all')), [])
        self.assertEqual(list(trash_items(self.root)), [
            {'id': entry, 'path': self.relative, 'size': len(self.content)}])
        folder, trashed, record = trash_record(self.root, entry)
        self.assertEqual(trashed.read_bytes(), self.content)
        self.assertEqual(trashed.with_name(trashed.name + '.json').read_bytes(), self.metadata)
        checksum = hashlib.sha256(self.content).hexdigest()
        self.assertEqual(record['checksum'], checksum)
        self.assertEqual(trashed_checksums(Path(str(self.root))), {checksum})
        self.assertEqual(restore_file(self.gallery, entry), self.relative)
        self.assertEqual(self.photo.read_bytes(), self.content)
        self.assertEqual(self.sidecar.read_bytes(), self.metadata)
        self.assertFalse(folder.exists())
        self.assertEqual(list(trash_items(self.root)), [])
        self.assertEqual(trashed_checksums(self.root), set())

    def test_restore_collisions_never_overwrite_media_or_sidecar(self):
        entry = trash_file(self.gallery, self.relative)
        self.photo.write_bytes(b'existing media')
        with self.assertRaisesRegex(ValueError, 'overwrite'):
            restore_file(self.gallery, entry)
        self.assertEqual(self.photo.read_bytes(), b'existing media')
        _, trashed, _ = trash_record(self.root, entry)
        self.assertEqual(trashed.read_bytes(), self.content)
        self.photo.unlink()
        self.sidecar.write_bytes(b'existing metadata')
        with self.assertRaisesRegex(ValueError, 'overwrite'):
            restore_file(self.gallery, entry)
        self.assertEqual(self.sidecar.read_bytes(), b'existing metadata')
        self.assertEqual(trashed.read_bytes(), self.content)
        self.assertEqual(trashed.with_name(trashed.name + '.json').read_bytes(), self.metadata)

    def test_paths_and_trash_ids_cannot_escape_storage(self):
        outside = self.root / 'outside.jpg'
        outside.write_bytes(b'outside')
        for relative in ('../outside.jpg', str(outside)):
            for operation in (keep_file, trash_file):
                with self.subTest(operation=operation.__name__, relative=relative):
                    with self.assertRaises(ValueError):
                        operation(self.gallery, relative)
        for entry in ('../gallery', 'invalid', 'a' * 31, 'a' * 33):
            with self.subTest(entry=entry), self.assertRaises(ValueError):
                restore_file(self.gallery, entry)
        entry = 'a' * 32
        malicious = self.root / 'trash' / entry
        malicious.mkdir(parents=True)
        (malicious / 'entry.json').write_text(json.dumps({'path': '../../outside.jpg'}))
        with self.assertRaises(ValueError):
            restore_file(self.gallery, entry)
        self.assertEqual(outside.read_bytes(), b'outside')
        self.assertTrue(self.photo.exists())

    def test_trashed_media_is_not_reimported_and_is_excluded_from_exports(self):
        source = self.root / 'input.zip'
        with zipfile.ZipFile(source, 'w') as archive:
            archive.writestr('album/' + self.photo.name, self.content)
            archive.writestr('album/' + self.sidecar.name, self.metadata)
        entry = trash_file(self.gallery, self.relative)
        import_archives([source], self.gallery, workers=1, password='')
        self.assertFalse(self.photo.exists())
        self.assertFalse(self.sidecar.exists())
        self.assertEqual([item['id'] for item in trash_items(self.root)], [entry])
        remaining = self.photo.parent / 'IMG_20140608.jpg'
        remaining.write_bytes(b'kept media')
        exports = self.root / 'exports'
        write_archives(self.gallery, None, exports)
        with zipfile.ZipFile(exports / '2014.zip') as archive:
            self.assertEqual(archive.namelist(), ['2014/06/photos/IMG_20140608.jpg'])
        self.assertEqual(trashed_checksums(self.root), {hashlib.sha256(self.content).hexdigest()})


if __name__ == '__main__':
    unittest.main()
