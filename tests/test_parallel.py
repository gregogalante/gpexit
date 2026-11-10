import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pyzipper

from gpexit import import_archives


class ParallelImportTests(unittest.TestCase):
    def test_parallel_matches_sequential_and_is_idempotent(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            paths = []
            for number in range(3):
                path = root / f'{number}.zip'
                with pyzipper.AESZipFile(path, 'w', encryption=pyzipper.WZ_AES) as archive:
                    archive.setpassword(b'password')
                    archive.writestr(f'album{number}/IMG.jpg', b'same content' if number < 2 else b'different content')
                    metadata = {'title': 'IMG.jpg', 'photoTakenTime': {'timestamp': '1546300800'}}
                    archive.writestr(f'album{number}/IMG.jpg.json', json.dumps(metadata))
                paths.append(path)
            snapshots = []
            for workers in (1, 2):
                output = root / f'output-{workers}'
                output.mkdir()
                with patch('gpexit.getpass.getpass', return_value='password'):
                    first = import_archives(paths, output, workers=workers)
                    second = import_archives(paths, output, workers=workers)
                self.assertEqual((first['imported'], first['duplicates']), (2, 1))
                self.assertEqual((second['imported'], second['resumed']), (0, 3))
                self.assertFalse(list(output.rglob('.import-*')))
                snapshots.append({str(p.relative_to(output)): p.read_bytes() for p in output.rglob('*') if p.is_file() and not p.name.startswith('.gpexit-progress')})
            self.assertEqual(snapshots[0], snapshots[1])


if __name__ == '__main__':
    unittest.main()
