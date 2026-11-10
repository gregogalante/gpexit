import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

import pyzipper

from gpexit import extract_member, import_archives


class ResumeTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / 'input.zip'
        self.output = self.root / 'output'
        self.output.mkdir()

    def make_archive(self, items):
        with pyzipper.AESZipFile(self.source, 'w') as archive:
            for name, content in items:
                archive.writestr(f'album/{name}', content)

    def test_interrupted_import_resumes_at_first_unfinished_member(self):
        self.make_archive([('IMG_20200102.jpg', b'first'), ('IMG_20200203.jpg', b'second')])
        def interrupt_second(*args):
            if args[1] == 1:
                raise KeyboardInterrupt()
            return extract_member(*args)

        with patch('gpexit.extract_member', side_effect=interrupt_second):
            with self.assertRaises(KeyboardInterrupt):
                import_archives([self.source], self.output, workers=1)
        first = self.output / '2020/01/photos/IMG_20200102.jpg'
        self.assertEqual(first.read_bytes(), b'first')
        with patch('gpexit.extract_member', wraps=extract_member) as extraction:
            import_archives([self.source], self.output, workers=1)
        self.assertEqual([call.args[1] for call in extraction.call_args_list], [1])
        self.assertEqual((self.output / '2020/02/photos/IMG_20200203.jpg').read_bytes(), b'second')
        self.assertFalse(list(self.output.glob('.import-*')))

    def test_completed_entries_are_skipped_when_inputs_are_unchanged(self):
        self.make_archive([('IMG_20200102.jpg', b'first'), ('IMG_20200203.jpg', b'second')])
        import_archives([self.source], self.output, workers=1)
        with patch('gpexit.extract_member', wraps=extract_member) as extraction:
            import_archives([self.source], self.output, workers=1)
        extraction.assert_not_called()

    def test_missing_output_is_reimported(self):
        self.make_archive([('IMG_20200102.jpg', b'first'), ('IMG_20200203.jpg', b'second')])
        import_archives([self.source], self.output, workers=1)
        missing = self.output / '2020/01/photos/IMG_20200102.jpg'
        missing.unlink()
        with patch('gpexit.extract_member', wraps=extract_member) as extraction:
            import_archives([self.source], self.output, workers=1)
        self.assertEqual([call.args[1] for call in extraction.call_args_list], [0])
        self.assertEqual(missing.read_bytes(), b'first')

    def test_changed_input_is_reprocessed_even_if_size_and_mtime_match(self):
        self.make_archive([('IMG_20200102.jpg', b'first')])
        import_archives([self.source], self.output, workers=1)
        old_stat = self.source.stat()
        self.make_archive([('IMG_20200102.jpg', b'other')])
        self.assertEqual(self.source.stat().st_size, old_stat.st_size)
        os.utime(self.source, ns=(old_stat.st_atime_ns, old_stat.st_mtime_ns))
        with patch('gpexit.extract_member', wraps=extract_member) as extraction:
            import_archives([self.source], self.output, workers=1)
        self.assertEqual(extraction.call_count, 1)
        photos = self.output / '2020/01/photos'
        self.assertEqual({path.read_bytes() for path in photos.glob('*.jpg')}, {b'first', b'other'})


if __name__ == '__main__':
    unittest.main()
