"""Run one gallery operation in an isolated process."""
import getpass
import json
from pathlib import Path
import sys

from archive_tasks import import_archives, write_archives


def no_prompt(prompt):
    raise ValueError('Archive password missing or incorrect. Retry with the correct password in the web UI.')


def main():
    request = json.load(sys.stdin)
    getpass.getpass = no_prompt
    gallery = Path(request['gallery'])
    if request['action'] == 'import':
        gallery.mkdir(parents=True, exist_ok=True)
        counts = import_archives([Path(path) for path in request['paths']], gallery, password=request.get('password'))
        print(json.dumps(counts), flush=True)
    elif request['action'] == 'export':
        password = request.get('password')
        write_archives(gallery, password.encode('utf-8') if password else None, Path(request['exports']))
    else:
        raise ValueError('Unknown task.')


if __name__ == '__main__':
    try:
        main()
    except Exception as exc:
        print(f'Error: {exc}', file=sys.stderr, flush=True)
        sys.exit(1)
