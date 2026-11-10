import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile


class ConsoleEncodingTests(unittest.TestCase):
    def test_import_with_cp1252_console_and_unicode_filename(self):
        script = Path(__file__).resolve().parents[1] / 'gpexit.py'
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / '照片.zip'
            output = root / 'output'
            with zipfile.ZipFile(archive, 'w') as source:
                source.writestr('album/IMG_20200102.jpg', b'photo')
            env = os.environ.copy()
            env['PYTHONIOENCODING'] = 'cp1252'
            result = subprocess.run(
                [sys.executable, str(script), str(archive), '--workers', '1', '--output', str(output)],
                input='n\n', capture_output=True, encoding='cp1252', env=env, timeout=30,
            )
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertEqual((output / '2020/01/photos/IMG_20200102.jpg').read_bytes(), b'photo')


if __name__ == '__main__':
    unittest.main()
