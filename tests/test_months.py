import datetime as dt
import json
import tempfile
import unittest
from pathlib import Path

import pyzipper

from gpexit import choose_date, filename_date, import_archives, safe_filename, write_archives


class DateTests(unittest.TestCase):
    def test_filename_dates_include_month_and_day(self):
        examples = {
            'IMG_20190312_235959.jpg': dt.date(2019, 3, 12),
            'Screenshot_2021-12-03-14-22-00.png': dt.date(2021, 12, 3),
            '20210405112233123.jpg': dt.date(2021, 4, 5),
        }
        for name, expected in examples.items():
            with self.subTest(name=name):
                self.assertEqual(filename_date(name), expected)

    def test_filename_rejects_invalid_or_incomplete_dates(self):
        for name in ('IMG_20210230.jpg', 'IMG_20211301.jpg', 'photo_2020.jpg'):
            with self.subTest(name=name):
                self.assertIsNone(filename_date(name))

    def test_date_priority(self):
        embedded = dt.date(2021, 2, 3)
        metadata = {'photoTakenTime': {'timestamp': '1546300800'}}
        self.assertEqual(choose_date('IMG_20200506.jpg', metadata, embedded), dt.date(2019, 1, 1))
        self.assertEqual(choose_date('IMG_20200506.jpg', {}, embedded), dt.date(2020, 5, 6))
        self.assertEqual(choose_date('IMG.jpg', {}, embedded), embedded)
        self.assertIsNone(choose_date('IMG.jpg', {'creationTime': {'timestamp': '1752541200'}}))


class FilenameCompatibilityTests(unittest.TestCase):
    def test_reserved_and_invalid_windows_names(self):
        self.assertEqual(safe_filename('CON.jpg'), '_CON.jpg')
        self.assertEqual(safe_filename('photo:one?.jpg'), 'photo_one_.jpg')


class MonthLayoutTests(unittest.TestCase):
    def test_legacy_archives_and_new_media_merge_into_months_and_round_trip(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            old = root / 'old.zip'
            new = root / 'new.zip'
            output = root / 'output'
            output.mkdir()
            with pyzipper.AESZipFile(old, 'w') as archive:
                archive.writestr('2019/foto/undated.jpg', b'old photo')
                archive.writestr('2019/video/IMG_20190214.mp4', b'old video')
            metadata = {'title': 'fresh.jpg', 'photoTakenTime': {'timestamp': '1551398400'}}
            with pyzipper.AESZipFile(new, 'w') as archive:
                archive.writestr('Takeout/album/fresh.jpg', b'new photo')
                archive.writestr('Takeout/album/fresh.jpg.json', json.dumps(metadata))
                archive.writestr('Takeout/other/copy.jpg', b'old photo')
                archive.writestr('Takeout/other/unknown.jpg', b'unknown photo')
            result = import_archives([old, new], output, workers=1)
            self.assertEqual((result['imported'], result['duplicates']), (4, 1))
            self.assertEqual((output / '2019/Unknown month/photos/undated.jpg').read_bytes(), b'old photo')
            self.assertEqual((output / '2019/02/videos/IMG_20190214.mp4').read_bytes(), b'old video')
            self.assertEqual((output / '2019/03/photos/fresh.jpg').read_bytes(), b'new photo')
            self.assertTrue((output / '2019/03/photos/fresh.jpg.json').is_file())
            self.assertEqual((output / 'Unknown year/Unknown month/photos/unknown.jpg').read_bytes(), b'unknown photo')
            write_archives(output, None)
            restored = root / 'restored'
            restored.mkdir()
            import_archives([output / 'archives/2019.zip'], restored, workers=1)
            self.assertEqual((restored / '2019/Unknown month/photos/undated.jpg').read_bytes(), b'old photo')
            self.assertEqual((restored / '2019/02/videos/IMG_20190214.mp4').read_bytes(), b'old video')
            self.assertEqual((restored / '2019/03/photos/fresh.jpg').read_bytes(), b'new photo')

    def test_renamed_collision_retains_metadata_after_round_trip(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'source.zip'
            with pyzipper.AESZipFile(source, 'w') as archive:
                for folder, content in [('one', b'first'), ('two', b'second')]:
                    archive.writestr(f'{folder}/same.jpg', content)
                    archive.writestr(f'{folder}/same.jpg.json', json.dumps({
                        'title': 'same.jpg', 'photoTakenTime': {'timestamp': '1546300800'},
                        'description': folder,
                    }))
            output = root / 'output'
            output.mkdir()
            import_archives([source], output, workers=1)
            write_archives(output, None)
            restored = root / 'restored'
            restored.mkdir()
            import_archives([output / 'archives/2019.zip'], restored, workers=1)
            metadata_files = list((restored / '2019/01/photos').glob('*.json'))
            self.assertEqual({json.loads(path.read_text())['description'] for path in metadata_files}, {'one', 'two'})

    def test_existing_legacy_output_is_migrated_without_losing_sidecar(self):
        with tempfile.TemporaryDirectory() as temporary:
            output = Path(temporary)
            old = output / '2019/foto'
            old.mkdir(parents=True)
            (old / 'IMG_20190612.jpg').write_bytes(b'existing')
            (old / 'IMG_20190612.jpg.json').write_text('{}')
            source = output / 'empty.zip'
            with pyzipper.AESZipFile(source, 'w'):
                pass
            import_archives([source], output, workers=1)
            migrated = output / '2019/06/photos/IMG_20190612.jpg'
            self.assertEqual(migrated.read_bytes(), b'existing')
            self.assertTrue(migrated.with_name(migrated.name + '.json').is_file())
            self.assertFalse((old / 'IMG_20190612.jpg').exists())


if __name__ == '__main__':
    unittest.main()
