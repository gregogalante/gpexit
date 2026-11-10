import unittest

from gpexit import choose_year, filename_year, media_kind


class FilenameYearTests(unittest.TestCase):
    def test_compact_camera_and_whatsapp_dates(self):
        self.assertEqual(filename_year('IMG_20190312_235959.jpg'), '2019')
        self.assertEqual(filename_year('IMG-20200131-WA0001.jpg'), '2020')

    def test_separated_dates(self):
        self.assertEqual(filename_year('Screenshot_2021-12-03-14-22-00.png'), '2021')

    def test_rejects_invalid_dates_and_unrelated_digits(self):
        for name in ('IMG_20210230.jpg', 'IMG_20211301.jpg', 'photo_2020.jpg'):
            with self.subTest(name=name):
                self.assertIsNone(filename_year(name))


class MediaKindTests(unittest.TestCase):
    def test_supported_formats_ignore_extension_case(self):
        self.assertEqual(media_kind('IMG_1234.JPG'), 'photos')
        self.assertEqual(media_kind('IMG_1234.HEIC'), 'photos')
        self.assertEqual(media_kind('clip.MP4'), 'videos')
        self.assertEqual(media_kind('clip.MOV'), 'videos')

    def test_metadata_is_not_media(self):
        self.assertIsNone(media_kind('IMG_1234.jpg.supplemental-metadata.json'))


class ChooseYearTests(unittest.TestCase):
    def test_google_taken_date_takes_precedence(self):
        metadata = {'photoTakenTime': {'timestamp': '1546300800'}}
        self.assertEqual(choose_year('IMG_20200101.jpg', metadata, '2021'), '2019')

    def test_filename_precedes_embedded_date(self):
        self.assertEqual(choose_year('IMG_20200101.jpg', {}, '2021'), '2020')

    def test_embedded_date_is_fallback(self):
        self.assertEqual(choose_year('IMG.jpg', {}, '2018'), '2018')

    def test_export_creation_time_is_not_used(self):
        metadata = {'creationTime': {'timestamp': '1752541200'}}
        self.assertIsNone(choose_year('IMG.jpg', metadata))

    def test_missing_date(self):
        self.assertIsNone(choose_year('IMG.jpg', {}))


if __name__ == '__main__':
    unittest.main()

class ArchiveIntegrationTests(unittest.TestCase):
    def test_cumulative_deduplication_and_encrypted_archive_round_trip(self):
        import json
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        import pyzipper
        from gpexit import import_archives, write_archives

        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            source = root / 'takeout.zip'
            output = root / 'output'
            output.mkdir()
            metadata = {'title': 'IMG.jpg', 'photoTakenTime': {'timestamp': '1546300800'}}
            with pyzipper.AESZipFile(source, 'w', encryption=pyzipper.WZ_AES) as archive:
                archive.setpassword(b'secret')
                archive.writestr('Takeout/album/IMG.jpg', b'first image')
                archive.writestr('Takeout/album/IMG.jpg.supplemental-metadata.json', json.dumps(metadata))
                archive.writestr('Takeout/other/duplicate.jpg', b'first image')
                archive.writestr('Takeout/another/IMG.jpg', b'different image')
            with patch('gpexit.getpass.getpass', return_value='secret'):
                first = import_archives([source], output)
                second = import_archives([source], output)
            self.assertEqual((first['imported'], first['duplicates']), (2, 1))
            self.assertEqual((second['imported'], second['resumed']), (0, 3))
            self.assertEqual((output / '2019/01/photos/IMG.jpg').read_bytes(), b'first image')
            self.assertTrue((output / '2019/01/photos/IMG.jpg.json').exists())
            self.assertEqual((output / 'Unknown year/Unknown month/photos/IMG.jpg').read_bytes(), b'different image')
            write_archives(output, b'archive-password')
            restored = root / 'restored'
            restored.mkdir()
            annual = output / 'archives/2019.zip'
            with patch('gpexit.getpass.getpass', return_value='archive-password'):
                import_archives([annual], restored)
            self.assertEqual((restored / '2019/01/photos/IMG.jpg').read_bytes(), b'first image')

class TimestampFilenameTests(unittest.TestCase):
    def test_compact_timestamp(self):
        self.assertEqual(filename_year('20210405112233.jpg'), '2021')
        self.assertEqual(filename_year('20210405112233123.jpg'), '2021')

class AdditionalIntegrationTests(unittest.TestCase):
    def test_zipcrypto_and_later_date_and_name_collision(self):
        import json
        import subprocess
        import shutil
        import tempfile
        from pathlib import Path
        from unittest.mock import patch
        import pyzipper
        from gpexit import import_archives

        if not shutil.which('zip'):
            self.skipTest('zip command is not installed')
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            output = root / 'output'
            output.mkdir()
            media = root / 'original.jpg'
            media.write_bytes(b'same image')
            encrypted = root / 'encrypted.zip'
            subprocess.run(['zip', '-j', '-P', 'test-password', str(encrypted), str(media)], check=True, capture_output=True)
            with patch('gpexit.getpass.getpass', return_value='test-password'):
                import_archives([encrypted], output)
            self.assertTrue((output / 'Unknown year/Unknown month/photos/original.jpg').exists())
            later = root / 'later.zip'
            with pyzipper.AESZipFile(later, 'w') as archive:
                archive.writestr('album/original.jpg', b'same image')
                archive.writestr('album/original.jpg.json', json.dumps({'title': 'original.jpg', 'photoTakenTime': {'timestamp': '1546300800'}}))
                archive.writestr('other/original.jpg', b'other image')
                archive.writestr('other/original.jpg.json', json.dumps({'title': 'original.jpg', 'photoTakenTime': {'timestamp': '1546300800'}}))
            import_archives([later], output)
            self.assertFalse((output / 'Unknown year/Unknown month/photos/original.jpg').exists())
            files = list((output / '2019/01/photos').glob('*.jpg'))
            self.assertEqual({p.read_bytes() for p in files}, {b'same image', b'other image'})
