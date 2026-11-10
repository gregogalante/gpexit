"""Read capture dates from filenames, Google metadata and EXIF."""
import datetime as dt
import re

from PIL import Image


def filename_date(name):
    for match in re.finditer(r'(?<!\d)((?:19|20)\d{2})[-_.]?(\d{2})[-_.]?(\d{2})(?=(?:\d{6}(?:\d{3})?)?(?!\d))', name):
        try:
            return dt.date(*map(int, match.groups()))
        except ValueError:
            continue
    return None


def filename_year(name):
    date = filename_date(name)
    return str(date.year) if date else None


def google_date(metadata):
    try:
        timestamp = int(metadata.get('photoTakenTime', {}).get('timestamp', 0))
        if timestamp > 0:
            date = dt.datetime.fromtimestamp(timestamp, dt.timezone.utc).date()
            if 1900 <= date.year <= dt.datetime.now(dt.timezone.utc).year + 1:
                return date
    except (ValueError, TypeError, OSError, OverflowError, AttributeError):
        pass
    return None


def choose_date(filename, metadata, embedded_date=None):
    return google_date(metadata) or filename_date(filename) or embedded_date


def choose_year(filename, metadata, embedded_year=None):
    date = choose_date(filename, metadata)
    return str(date.year) if date else embedded_year


def embedded_date(path):
    try:
        with Image.open(path) as image:
            exif = image.getexif()
            date = exif.get(36867) or exif.get(36868)
            if not date:
                date = exif.get_ifd(34665).get(36867)
            if date and re.match(r'^(19|20)\d{2}:\d{2}:\d{2}', str(date)):
                return dt.datetime.strptime(str(date)[:10], '%Y:%m:%d').date()
    except (OSError, ValueError, TypeError, AttributeError, KeyError, SyntaxError):
        pass
    return None


