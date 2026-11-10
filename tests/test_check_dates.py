import csv
import io
import json
import tempfile
import unittest
from pathlib import Path

from check_dates import check_dates, google_date
from gpexit import filename_date


class CheckDatesTests(unittest.TestCase):
    def test_common_filename_formats(self):
        for name in ('IMG_20140607_154612.jpg', 'VID_20140607_154612.mp4',
                     'PXL_20140607_154612123.jpg', 'Screenshot_2014-06-07-15-46-12.png',
                     'IMG-20140607-WA0001.jpg'):
            with self.subTest(name=name):
                self.assertEqual(filename_date(name).isoformat(), '2014-06-07')

    def test_report_conflicts_and_unavailable_metadata(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            folder = output / '2014' / '12' / 'photos'
            folder.mkdir(parents=True)
            for name in ('IMG_20140607_154612.jpg', 'IMG_20141228_000000.jpg',
                         'IMG_20140608_000000.jpg', 'IMG_20140609_000000.jpg', 'undated.jpg'):
                (folder / name).touch()
            metadata = {'photoTakenTime': {'timestamp': '1419793427'}}
            for name in ('IMG_20140607_154612.jpg', 'IMG_20141228_000000.jpg'):
                (folder / (name + '.json')).write_text(json.dumps(metadata), encoding='utf-8')
            (folder / 'IMG_20140609_000000.jpg.json').write_text('{}', encoding='utf-8')
            report = io.StringIO(newline='')
            self.assertEqual(check_dates(output, csv.writer(report)), (2, 1, 1, 1))
            rows = list(csv.reader(io.StringIO(report.getvalue())))
            self.assertEqual(rows[1], ['2014/12/photos/IMG_20140607_154612.jpg', '2014-06-07', '2014-12-28'])
            self.assertEqual(len(rows), 2)

    def test_invalid_google_metadata(self):
        for value in (None, [], {}, {'photoTakenTime': None}, {'photoTakenTime': {'timestamp': 'bad'}}):
            with self.subTest(value=value):
                self.assertIsNone(google_date(value))


if __name__ == '__main__':
    unittest.main()
