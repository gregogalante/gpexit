import hashlib
import io
import json
from http.client import HTTPConnection
from pathlib import Path
import tempfile
import threading
import time
import unittest
from unittest.mock import Mock, patch
from types import SimpleNamespace
import zipfile

from PIL import Image
import pyzipper

from archive_tasks import import_archives, index_output, write_archives
from gallery import conflicts, correct_date, load_overrides, media_path
from dates import google_date
from web_app import Application, Handler, ThreadingHTTPServer


class WebTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.app = Application(self.root)
        self.addCleanup(self.app.close)
        self.app.inputs.mkdir()
        image = io.BytesIO()
        Image.new('RGB', (30, 20), 'blue').save(image, 'JPEG')
        self.photo = image.getvalue()
        self.name = 'IMG_20140607_154612.jpg'
        self.relative = f'2014/12/photos/{self.name}'
        self.metadata = json.dumps({'title': self.name, 'photoTakenTime': {'timestamp': '1419793427'}})
        self.archive = self.app.inputs / 'takeout.zip'
        with zipfile.ZipFile(self.archive, 'w') as archive:
            archive.writestr(f'album/{self.name}', self.photo)
            archive.writestr(f'album/{self.name}.json', self.metadata)

    def import_gallery(self):
        self.app.gallery.mkdir(exist_ok=True)
        import_archives([self.archive], self.app.gallery, workers=1, password='')

    def wait_task(self):
        deadline = time.monotonic() + 15
        while self.app.task['running'] and time.monotonic() < deadline:
            time.sleep(0.02)
        self.assertFalse(self.app.task['running'], 'Task did not finish within 15 seconds.')
        self.assertEqual(self.app.task['status'], 'Completato', '\n'.join(self.app.task['log']))

    def test_import_and_external_export_tasks(self):
        self.assertEqual(self.app.state()['inputs'][0]['name'], 'takeout.zip')
        self.app.start({'action': 'import', 'inputs': ['takeout.zip']})
        self.wait_task()
        self.assertEqual((self.app.gallery / self.relative).read_bytes(), self.photo)
        self.app.start({'action': 'export'})
        self.wait_task()
        exported = self.app.exports / '2014.zip'
        self.assertTrue(exported.is_file())
        self.assertFalse((self.app.gallery / 'archives').exists())
        with zipfile.ZipFile(exported) as archive:
            self.assertEqual(archive.read(self.relative), self.photo)

    def test_export_password_requires_matching_confirmation(self):
        self.import_gallery()
        for request in ({'password': 'secret'}, {'password': 'secret', 'password_confirmation': 'wrong'},
                        {'password': '', 'password_confirmation': 'secret'}):
            with self.subTest(request=request), self.assertRaisesRegex(ValueError, 'non coincidono'):
                self.app.start({'action': 'export', **request})
            self.assertFalse(self.app.task['running'])
            self.assertFalse(self.app.exports.exists())
        self.app.start({'action': 'export', 'password': 'secret', 'password_confirmation': 'secret'})
        self.wait_task()
        with pyzipper.AESZipFile(self.app.exports / '2014.zip') as archive:
            archive.setpassword(b'secret')
            self.assertEqual(archive.read(self.relative), self.photo)

    def test_cancelled_media_request_does_not_send_second_response(self):
        self.app.gallery.mkdir()
        video = self.app.gallery / 'video.mp4'
        video.write_bytes(b'video')
        handler = Handler.__new__(Handler)
        handler.server = SimpleNamespace(app=self.app, server_port=8765)
        handler.headers = {'Host': '127.0.0.1:8765'}
        handler.path = '/api/media?path=video.mp4'
        handler.send_response = Mock()
        handler.send_header = Mock()
        handler.end_headers = Mock()
        handler.send_json = Mock()
        handler.wfile = io.BytesIO()
        for error in (BrokenPipeError, ConnectionResetError, ConnectionAbortedError):
            with self.subTest(error=error), patch('web_app.shutil.copyfileobj', side_effect=error):
                handler.do_GET()
            handler.send_json.assert_not_called()
        with patch('web_app.BaseHTTPRequestHandler.handle', side_effect=BrokenPipeError):
            handler.handle()

    def test_manual_date_survives_index_resume_and_reimport(self):
        self.import_gallery()
        self.assertEqual(len(list(conflicts(self.app.gallery))), 1)
        destination = correct_date(self.app.gallery, self.relative, '2013-06-07')
        corrected = self.app.gallery / destination
        self.assertEqual(destination, f'2013/06/photos/{self.name}')
        self.assertEqual(corrected.read_bytes(), self.photo)
        self.assert_corrected_metadata(corrected.with_name(self.name + '.json'), '2013-06-07')
        self.assertEqual(list(load_overrides(self.app.gallery).values()), ['2013-06-07'])
        self.assertEqual(list(conflicts(self.app.gallery)), [])
        index_output(self.app.gallery)
        self.assertTrue(corrected.exists())
        import_archives([self.archive], self.app.gallery, workers=1, password='')
        self.assertTrue(corrected.exists())
        corrected.unlink()
        corrected.with_name(self.name + '.json').unlink()
        import_archives([self.archive], self.app.gallery, workers=1, password='')
        self.assertEqual(corrected.read_bytes(), self.photo)
        self.assert_corrected_metadata(corrected.with_name(self.name + '.json'), '2013-06-07')

    def assert_corrected_metadata(self, sidecar, date):
        metadata = json.loads(sidecar.read_text(encoding='utf-8'))
        self.assertEqual(google_date(metadata).isoformat(), date)
        self.assertEqual(metadata['gpexitDate'], date)
        self.assertEqual(metadata['gpexitOriginalPhotoTakenTime'], json.loads(self.metadata)['photoTakenTime'])
        self.assertEqual(metadata['title'], self.name)
        self.assertIn('19:03:47 UTC', metadata['photoTakenTime']['formatted'])

    def test_json_preserves_correction_without_databases_or_manifest(self):
        self.import_gallery()
        destination = correct_date(self.app.gallery, self.relative, '2013-06-07')
        destination = correct_date(self.app.gallery, destination, '2012-03-04')
        corrected_json = self.app.gallery / (destination + '.json')
        self.assert_corrected_metadata(corrected_json, '2012-03-04')
        backup = self.root / 'portable.zip'
        with zipfile.ZipFile(backup, 'w') as archive:
            archive.write(self.app.gallery / destination, destination)
            archive.write(corrected_json, destination + '.json')
        restored = self.root / 'portable-gallery'
        restored.mkdir()
        import_archives([backup], restored, workers=1, password='')
        self.assertEqual(load_overrides(restored), {})
        self.assertEqual((restored / destination).read_bytes(), self.photo)
        self.assert_corrected_metadata(restored / (destination + '.json'), '2012-03-04')
        self.assertEqual(list(conflicts(restored)), [])
        import_archives([self.archive], restored, workers=1, password='')
        self.assertTrue((restored / destination).exists())
        self.assert_corrected_metadata(restored / (destination + '.json'), '2012-03-04')

    def test_rejects_paths_outside_inputs_and_gallery(self):
        self.import_gallery()
        outside = self.root / 'outside.jpg'
        outside.write_bytes(self.photo)
        for value in ('../outside.jpg', str(outside)):
            with self.subTest(path=value), self.assertRaises(ValueError):
                media_path(self.app.gallery, value)
        link = self.app.gallery / 'link.jpg'
        link.symlink_to(outside)
        with self.assertRaises(ValueError):
            media_path(self.app.gallery, 'link.jpg')
        with self.assertRaises(ValueError):
            self.app.start({'action': 'import', 'inputs': ['../outside.zip']})
        self.assertFalse(self.app.task['running'])

    def test_manual_decisions_survive_encrypted_export_and_fresh_restore(self):
        self.import_gallery()
        destination = correct_date(self.app.gallery, self.relative, '2013-06-07')
        write_archives(self.app.gallery, b'secret', self.app.exports)
        exported = self.app.exports / '2013.zip'
        checksum = hashlib.sha256(self.photo).hexdigest()
        with pyzipper.AESZipFile(exported) as archive:
            archive.setpassword(b'secret')
            self.assertEqual(json.loads(archive.read('2013/.gpexit-dates.json')), {checksum: '2013-06-07'})
        restored = self.root / 'restored'
        restored.mkdir()
        import_archives([exported], restored, workers=1, password='secret')
        self.assertEqual((restored / destination).read_bytes(), self.photo)
        self.assert_corrected_metadata(restored / (destination + '.json'), '2013-06-07')
        self.assertEqual(load_overrides(restored), {checksum: '2013-06-07'})
        self.assertEqual(list(conflicts(restored)), [])
        local = correct_date(restored, destination, '2012-03-04')
        import_archives([exported], restored, workers=1, password='secret')
        self.assertTrue((restored / local).exists())
        self.assertEqual(load_overrides(restored), {checksum: '2012-03-04'})

    def test_invalid_export_manifest_is_rejected_before_persisting(self):
        checksum = hashlib.sha256(self.photo).hexdigest()
        for values in ({'not-a-hash': '2014-06-07'}, {checksum: '2014-02-30'},
                       {checksum: '20140607'}, {checksum: {'date': '2014-06-07'}}, []):
            with self.subTest(values=values):
                source = self.root / 'invalid.zip'
                with zipfile.ZipFile(source, 'w') as archive:
                    archive.writestr('2014/.gpexit-dates.json', json.dumps(values))
                self.app.gallery.mkdir(exist_ok=True)
                with self.assertRaisesRegex(ValueError, 'Invalid date manifest'):
                    import_archives([source], self.app.gallery, workers=1, password='')
                self.assertEqual(load_overrides(self.app.gallery), {})

    def test_http_token_host_protection_and_preview(self):
        self.import_gallery()
        server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        server.app = self.app
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        connection = HTTPConnection('127.0.0.1', server.server_port, timeout=5)
        try:
            connection.request('GET', '/api/state')
            response = connection.getresponse()
            self.assertEqual(response.status, 200)
            token = json.loads(response.read())['token']
            connection.request('POST', '/api/correct', json.dumps({'path': self.relative, 'date': '2014-06-07'}),
                               {'Content-Type': 'application/json'})
            response = connection.getresponse()
            self.assertEqual(response.status, 403)
            response.read()
            self.assertTrue((self.app.gallery / self.relative).exists())
            connection.request('GET', '/api/state', headers={'Host': 'attacker.example'})
            response = connection.getresponse()
            self.assertEqual(response.status, 403)
            response.read()
            connection.request('GET', '/api/media?path=' + self.relative)
            response = connection.getresponse()
            self.assertEqual(response.status, 200)
            self.assertEqual(response.getheader('Content-Type'), 'image/jpeg')
            with Image.open(io.BytesIO(response.read())) as preview:
                self.assertEqual(preview.size, (30, 20))
            connection.request('POST', '/api/correct', json.dumps({'path': self.relative, 'date': '2014-06-07'}),
                               {'Content-Type': 'application/json', 'X-GPExit-Token': token})
            response = connection.getresponse()
            self.assertEqual(response.status, 200)
            destination = json.loads(response.read())['path']
            self.assertEqual(destination, f'2014/06/photos/{self.name}')
            self.assertTrue((self.app.gallery / destination).exists())
        finally:
            connection.close()
            server.shutdown()
            thread.join(timeout=5)
            server.server_close()


if __name__ == '__main__':
    unittest.main()
