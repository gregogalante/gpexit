#!/usr/bin/env python3
"""Serve the local GPExit interface and serialize gallery operations."""
import argparse
from collections import deque
import json
import mimetypes
import os
from pathlib import Path
import secrets
import signal
import sqlite3
import shutil
import subprocess
import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from gallery import conflicts, correct_date, media_path
from gallery_review import gallery_items, keep_file, restore_file, trash_file, trash_items
from storage import storage_stats

ROOT = Path(__file__).resolve().parent


def paginate(items, page):
    pages = max(1, (len(items) + 23) // 24)
    page = min(pages, max(1, int(page)))
    return {'items': items[(page - 1) * 24:page * 24], 'page': page, 'total': len(items), 'pages': pages}


class Application:
    def __init__(self, root):
        self.root = root.resolve()
        self.gallery = self.root / 'gallery'
        self.inputs = self.root / 'inputs'
        self.exports = self.root / 'exports'
        self.token = secrets.token_urlsafe(32)
        self.lock = threading.Lock()
        self.preview_slots = threading.BoundedSemaphore(2)
        self.task = {'running': False, 'status': 'Pronto', 'log': []}
        self.process = None
        self.gallery_cache = None

    def gallery_snapshot(self, refresh=False):
        if refresh or self.gallery_cache is None:
            self.gallery_cache = list(gallery_items(self.gallery, 'all'))
        return self.gallery_cache

    def listing(self, folder):
        return [{'name': path.name, 'size': path.stat().st_size} for path in sorted(folder.glob('*.zip')) if path.is_file() and not path.is_symlink()]

    def state(self):
        return {'inputs': self.listing(self.inputs), 'exports': self.listing(self.exports),
                'gallery': str(self.gallery),
                'task': self.task, 'token': self.token}

    def start(self, request):
        with self.lock:
            if self.task['running']:
                raise ValueError('Un task è già in esecuzione.')
            action = request.get('action')
            payload = {'action': action, 'gallery': str(self.gallery), 'exports': str(self.exports), 'password': request.get('password', '')}
            if action == 'import':
                names = request.get('inputs', [])
                allowed = {item['name'] for item in self.listing(self.inputs)}
                if not names or any(name not in allowed for name in names):
                    raise ValueError('Selezionare ZIP validi dalla lista inputs.')
                payload['paths'] = [str(self.inputs / name) for name in names]
            elif action == 'export':
                if request.get('password', '') != request.get('password_confirmation', ''):
                    raise ValueError('Le password di esportazione non coincidono.')
                if not self.gallery.is_dir():
                    raise ValueError('Gallery non trovata.')
            else:
                raise ValueError('Task sconosciuto.')
            process = subprocess.Popen([sys.executable, '-u', str(ROOT / 'web_worker.py')], stdin=subprocess.PIPE,
                                       stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding='utf-8', start_new_session=True)
            self.process = process
            self.task = {'running': True, 'status': 'In esecuzione', 'log': []}
            process.stdin.write(json.dumps(payload))
            process.stdin.close()
            threading.Thread(target=self.collect, args=(process,), daemon=True).start()

    def collect(self, process):
        lines = deque(maxlen=500)
        for line in process.stdout:
            lines.append(line.rstrip())
            self.task['log'] = list(lines)
        code = process.wait()
        process.stdout.close()
        with self.lock:
            self.task.update(running=False, status='Completato' if code == 0 else f'Fallito (exit {code})')
            self.gallery_cache = None
            self.process = None

    def close(self):
        if self.process and self.process.poll() is None:
            os.killpg(self.process.pid, signal.SIGTERM)
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait()


class Handler(BaseHTTPRequestHandler):
    def handle(self):
        try:
            super().handle()
        except ConnectionError:
            # Browsers cancel media requests when seeking or leaving the page.
            pass

    def log_message(self, *args):
        pass

    def send_json(self, value, status=200):
        data = json.dumps(value).encode('utf-8')
        self.send_response(status)
        self.send_header('Content-Type', 'application/json; charset=utf-8')
        self.send_header('Content-Length', str(len(data)))
        self.send_header('Cache-Control', 'no-store')
        self.end_headers()
        self.wfile.write(data)

    def allowed_host(self):
        return self.headers.get('Host') in (f'127.0.0.1:{self.server.server_port}', f'localhost:{self.server.server_port}')

    def do_GET(self):
        if not self.allowed_host():
            self.send_error(403)
            return
        app = self.server.app
        url = urlparse(self.path)
        query = parse_qs(url.query)
        try:
            if url.path == '/api/state':
                self.send_json(app.state())
            elif url.path == '/api/storage':
                self.send_json(storage_stats(app.root))
            elif url.path in ('/api/gallery', '/api/gallery/options', '/api/trash'):
                with app.lock:
                    if app.task['running']:
                        raise ValueError('Attendere la fine del task prima di sfogliare la gallery.')
                    if url.path == '/api/trash':
                        items = list(trash_items(app.root))
                    else:
                        items = app.gallery_snapshot(query.get('refresh') == ['1'])
                        if url.path == '/api/gallery':
                            review = query.get('review', ['pending'])[0]
                            if review not in ('pending', 'all'):
                                raise ValueError('Invalid gallery filter.')
                            year, month = query.get('year', [''])[0], query.get('month', [''])[0]
                            items = [item for item in items if (review == 'all' or not item['kept'])
                                     and (not year or item['year'] == year) and (not month or item['month'] == month)]
                if url.path == '/api/gallery/options':
                    self.send_json({'years': sorted({item['year'] for item in items if item['year']}),
                                    'months': sorted({item['month'] for item in items if item['month']})})
                else:
                    self.send_json(paginate(items, query.get('page', ['1'])[0]))
            elif url.path == '/api/conflicts':
                with app.lock:
                    if app.task['running']:
                        raise ValueError('Attendere la fine del task prima di verificare le date.')
                    items = list(conflicts(app.gallery))
                self.send_json(paginate(items, query.get('page', ['1'])[0]))
            elif url.path == '/api/media':
                path = media_path(app.gallery, query.get('path', [''])[0])
                from archive_tasks import media_kind
                from PIL import Image, ImageOps
                if media_kind(path.name) == 'photos':
                    import io
                    with app.preview_slots, Image.open(path) as original:
                        # JPEG decoders can downsample before allocating full-resolution pixels.
                        original.draft('RGB', (900, 900))
                        preview = ImageOps.exif_transpose(original)
                        preview.thumbnail((900, 700))
                        data = io.BytesIO()
                        preview.convert('RGB').save(data, 'JPEG', quality=82)
                    content = data.getvalue()
                    self.send_response(200)
                    self.send_header('Content-Type', 'image/jpeg')
                    self.send_header('Content-Length', str(len(content)))
                    self.end_headers()
                    self.wfile.write(content)
                else:
                    self.send_response(200)
                    self.send_header('Content-Type', mimetypes.guess_type(path.name)[0] or 'application/octet-stream')
                    self.send_header('Content-Length', str(path.stat().st_size))
                    self.end_headers()
                    with path.open('rb') as source:
                        shutil.copyfileobj(source, self.wfile)
            elif url.path == '/':
                data = (ROOT / 'web' / 'index.html').read_bytes()
                self.send_response(200)
                self.send_header('Content-Type', 'text/html; charset=utf-8')
                self.send_header('Content-Length', str(len(data)))
                self.end_headers()
                self.wfile.write(data)
            else:
                self.send_error(404)
        except ConnectionError:
            return
        except (OSError, ValueError, sqlite3.Error) as exc:
            self.send_json({'error': str(exc)}, 400)

    def do_POST(self):
        app = self.server.app
        if not self.allowed_host() or self.headers.get('X-GPExit-Token') != app.token:
            self.send_error(403)
            return
        try:
            length = int(self.headers.get('Content-Length', '0'))
            if not 0 < length <= 65536:
                raise ValueError('Invalid request size.')
            request = json.loads(self.rfile.read(length))
            if self.path == '/api/tasks':
                app.start(request)
                self.send_json({'ok': True})
            elif self.path in ('/api/gallery/keep', '/api/gallery/trash', '/api/gallery/restore'):
                with app.lock:
                    if app.task['running']:
                        raise ValueError('Attendere la fine del task.')
                    if self.path == '/api/gallery/keep':
                        keep_file(app.gallery, request['path'])
                        result = {'ok': True}
                    elif self.path == '/api/gallery/trash':
                        result = {'id': trash_file(app.gallery, request['path'])}
                    else:
                        result = {'path': restore_file(app.gallery, request['id'])}
                    app.gallery_cache = None
                self.send_json(result)
            elif self.path in ('/api/correct', '/api/keep'):
                with app.lock:
                    if app.task['running']:
                        raise ValueError('Attendere la fine del task.')
                    value = request.get('date')
                    if self.path == '/api/keep':
                        from dates import google_date
                        path = media_path(app.gallery, request['path'])
                        date = google_date(json.loads(path.with_name(path.name + '.json').read_text(encoding='utf-8')))
                        if date is None:
                            raise ValueError('Data Google non disponibile.')
                        value = date.isoformat()
                    destination = correct_date(app.gallery, request['path'], value)
                    app.gallery_cache = None
                self.send_json({'path': destination})
            else:
                self.send_error(404)
        except ConnectionError:
            return
        except (OSError, ValueError, KeyError, TypeError, sqlite3.Error) as exc:
            self.send_json({'error': str(exc)}, 400)


def main():
    parser = argparse.ArgumentParser(description='GPExit local web interface')
    parser.add_argument('--root', type=Path, default=Path.cwd(), help='Directory containing inputs, gallery and exports')
    parser.add_argument('--port', type=int, default=8765)
    args = parser.parse_args()
    server = ThreadingHTTPServer(('127.0.0.1', args.port), Handler)
    server.app = Application(args.root)
    print(f'GPExit: http://127.0.0.1:{server.server_port} — Ctrl+C to stop', flush=True)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.app.close()
        server.server_close()


if __name__ == '__main__':
    main()
