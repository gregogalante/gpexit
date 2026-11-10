import json
import io
from PIL import Image
from http.client import HTTPConnection
from pathlib import Path
import tempfile
import threading
import unittest
from unittest.mock import patch

from web_app import Application, Handler, ThreadingHTTPServer


class GalleryHTTPTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = Path(self.temporary.name)
        self.app = Application(self.root)
        self.paths = ['2014/06/photos/one.jpg', '2014/12/photos/two.jpg', '2015/06/videos/three.mp4']
        for relative in self.paths:
            path = self.app.gallery / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(relative.encode())
        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.app = self.app
        self.thread = threading.Thread(target=self.server.serve_forever)
        self.thread.start()
        self.connection = HTTPConnection('127.0.0.1', self.server.server_port, timeout=5)

    def tearDown(self):
        self.connection.close()
        self.server.shutdown()
        self.thread.join()
        self.server.server_close()
        self.app.close()
        self.temporary.cleanup()

    def request(self, path, body=None):
        headers = {'X-GPExit-Token': self.app.token, 'Content-Type': 'application/json'}
        self.connection.request('GET' if body is None else 'POST', path,
                                None if body is None else json.dumps(body), headers)
        response = self.connection.getresponse()
        data = json.loads(response.read())
        self.assertEqual(response.status, 200, data)
        return data

    def test_combined_filters_and_cached_pagination(self):
        from web_app import gallery_items
        with patch('web_app.gallery_items', wraps=gallery_items) as listing:
            options = self.request('/api/gallery/options')
            self.assertEqual(options, {'years': ['2014', '2015'], 'months': ['06', '12']})
            result = self.request('/api/gallery?year=2014&month=06&review=all&page=999')
            self.assertEqual(result['total'], 1)
            self.assertEqual(result['page'], 1)
            self.assertEqual(result['items'][0]['path'], self.paths[0])
            self.assertEqual(self.request('/api/gallery?year=2015&month=12')['total'], 0)
            self.assertEqual(listing.call_count, 1)
            self.request('/api/gallery?refresh=1')
            self.assertEqual(listing.call_count, 2)

    def test_large_rotated_jpeg_preview_keeps_orientation_and_bounds(self):
        path = self.app.gallery / 'portrait.jpg'
        with Image.new('RGB', (4000, 3000), 'blue') as source:
            exif = Image.Exif()
            exif[274] = 6
            source.save(path, 'JPEG', exif=exif)
        self.connection.request('GET', '/api/media?path=portrait.jpg')
        response = self.connection.getresponse()
        self.assertEqual(response.status, 200)
        with Image.open(io.BytesIO(response.read())) as preview:
            self.assertEqual(preview.size, (525, 700))
        with Image.open(path) as original:
            self.assertEqual(original.size, (4000, 3000))
            self.assertEqual(original.getexif()[274], 6)

    def test_keep_trash_restore_and_statistics(self):
        self.request('/api/gallery')
        self.request('/api/gallery/keep', {'path': self.paths[0]})
        self.assertEqual(self.request('/api/gallery')['total'], 2)
        self.assertEqual(self.request('/api/gallery?review=all')['total'], 3)
        removed = self.request('/api/gallery/trash', {'path': self.paths[1]})
        self.assertEqual(self.request('/api/gallery?review=all')['total'], 2)
        trashed = self.request('/api/trash')
        self.assertEqual(trashed['items'][0]['id'], removed['id'])
        statistics = self.request('/api/storage')
        self.assertGreater(statistics['trash']['bytes'], 0)
        self.assertGreater(statistics['gallery']['bytes'], 0)
        self.assertEqual(statistics['inputs']['bytes'], 0)
        self.request('/api/gallery/restore', {'id': removed['id']})
        self.assertEqual(self.request('/api/gallery?review=all')['total'], 3)
        self.assertEqual(self.request('/api/trash')['total'], 0)


if __name__ == '__main__':
    unittest.main()
