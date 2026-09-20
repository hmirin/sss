"""Disposable integration coverage for diagnostics, diffs, watches and expiration."""
import base64
import json
import os
from pathlib import Path
import signal
import socket
import sqlite3
import subprocess
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen


def exercise_features(binary):
    with tempfile.TemporaryDirectory(prefix='sss-features-') as tmp:
        root = Path(tmp)
        public = root / 'public'
        public.mkdir()
        data = root / 'data'
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        origin = f'http://127.0.0.1:{port}'
        env = {k: v for k, v in os.environ.items() if not k.startswith('SSS_')}
        env.update(HOME=str(root), SSS_UPSTREAM=origin)
        admin = 'admin:test-secret'
        server = None
        watcher = None

        def cli(*args, auth=admin, ok=True):
            e = dict(env)
            if auth is not None:
                e['SSS_BASIC_AUTH'] = auth
            result = subprocess.run([binary, *args], env=e, capture_output=True, text=True, timeout=20)
            assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
            return json.loads(result.stdout) if result.stdout else result.stderr

        def get(path, auth=None, body=None, method='GET'):
            headers = {}
            if auth:
                headers['Authorization'] = 'Basic ' + base64.b64encode(auth.encode()).decode()
            try:
                if body is not None:
                    headers['Content-Type'] = 'application/json'
                response = urlopen(Request(origin+path, headers=headers, method=method,
                    data=None if body is None else json.dumps(body).encode()), timeout=5)
            except HTTPError as e:
                response = e
            return response.status, response.read()

        def wait_for(predicate, seconds=10):
            until = time.monotonic()+seconds
            while time.monotonic() < until:
                if predicate():
                    return
                time.sleep(.1)
            raise AssertionError('condition timed out')

        def start(default_lifetime="7d"):
            nonlocal server
            server = subprocess.Popen([binary, 'serve', '--port', str(port), '--data-dir', str(data),
                '--default-expires-in', default_lifetime, '--basic_auth', admin], env=env,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            def ready():
                try:
                    return get('/health')[0] == 200
                except OSError:
                    return False
            wait_for(ready)

        def stop():
            if server and server.poll() is None:
                server.terminate()
                server.wait(timeout=10)

        try:
            start()
            status = cli('doctor')
            assert status['ok'] and status['checks']['health']['response']['version']
            assert status['checks']['server_admin_access']['response']['default_expires_in_seconds'] == 604800
            assert not cli('doctor', auth=None, ok=False)['ok']
            created = cli('new', '--name', 'features', '--basic_auth_view', 'reader:view-secret',
                          '--basic_auth_write', 'writer:write-secret')
            p = created['id']
            assert abs(created['expires_at']-time.time()-604800) < 3
            assert cli('doctor', '--project', p, auth='writer:write-secret')['ok']
            info = cli('info', '--project', p, auth='writer:write-secret')
            assert info['bytes'] == info['storage_bytes'] == info['files'] == 0
            assert info['auth']['view']['mode'] == 'basic'
            assert get(f'/api/projects/{p}', 'reader:view-secret')[0] == 401
            assert 'secret' not in json.dumps(info) and 'hash' not in json.dumps(info)
            cli('config', '--project', p, '--expires-in', 'none', auth='writer:write-secret', ok=False)
            cli('config', '--project', p, '--expires-in', 'none', '--name', 'renamed')
            assert cli('info', '--project', p)['expires_at'] is None
            assert cli('info', '--project', p)['name'] == 'renamed'
            assert get(f'/api/projects/{p}', admin, {'name': 'must-not-save', 'expires_in': 'bad'}, 'PATCH')[0] == 400
            assert cli('info', '--project', p)['name'] == 'renamed'
            settings = root / 'config.json'
            settings.write_text(json.dumps({'expires_in': '7d'}))
            cli('config', '--project', p, '--config', str(settings), '--expires-in', 'none')
            cli('config', '--project', p, '--expires-in', '-2d', ok=False)
            assert cli('info', '--project', p)['expires_at'] is None
            cli('config', '--project', p, '--expires-in', '1h')
            expires = cli('info', '--project', p)['expires_at']
            (public/'index.html').write_text('old\n')
            (public/'space #?.txt').write_text('special old\n')
            (public/'remove.txt').write_text('remove\n')
            cli('sync', str(public), '--project', p)
            info = cli('info', '--project', p)
            assert info['files'] == 3 and info['bytes'] == 23
            assert info['expires_at'] == expires
            (public/'index.html').write_text('new\n')
            (public/'space #?.txt').write_text('special new\n')
            (public/'remove.txt').unlink()
            (public/'add.txt').write_text('added\n')
            (public/'binary.bin').write_bytes(b'\x00\xff')
            diff = cli('diff', str(public), '--project', p, auth='writer:write-secret')
            changes = {v['path']: v for v in diff['changes']}
            assert len(changes) == 5
            assert '-old\n+new\n' in changes['index.html']['patch']
            assert 'special old' in changes['space #?.txt']['patch']
            assert changes['binary.bin']['content_omitted']
            assert changes['remove.txt']['kind'] == 'deleted'
            assert changes['add.txt']['kind'] == 'added'
            assert cli('versions', '--project', p)['current'] == 1
            assert get(f'/s/{p}/')[0] == 401  # diff uses edit auth, not view auth
            watch_log = open(root/'watch.jsonl', 'w+')
            watcher = subprocess.Popen([binary, '--basic_auth', admin, 'sync', str(public),
                                       '--project', p, '--watch'], env=env, stdout=watch_log, stderr=watch_log)
            def current():
                return cli('versions', '--project', p)['current']
            wait_for(lambda: current() == 2)
            for n in range(4):
                (public/'index.html').write_text(f'burst {n}\n')
                time.sleep(.12)
            wait_for(lambda: current() == 3)
            time.sleep(1.2)
            assert current() == 3
            (public/'.env').write_text('never publish')
            time.sleep(1.2)
            assert current() == 3
            watcher.send_signal(signal.SIGINT)
            assert watcher.wait(timeout=5) == 0
            watch_log.close()
            assert cli('diff', str(public), '--project', p)['changes'] == []
            stop()
            start('30d')
            inherited = cli('new')['id']
            assert abs(cli('info', '--project', inherited)['expires_at'] - time.time() - 2592000) < 3
            cli('delete', '--project', inherited)
            assert cli('info', '--project', p)['expires_at'] == expires
            noexpiry = cli('new', '--expires-in', 'none')['id']
            assert cli('info', '--project', noexpiry)['expires_at'] is None
            doomed = cli('new', '--expires-in', '4s')['id']
            cli('sync', str(public), '--project', doomed)
            time.sleep(4.1)
            for path in [f'/s/{doomed}/', f'/s/{doomed}/versions/1/', f'/api/projects/{doomed}',
                         f'/api/projects/{doomed}/manifest', f'/api/projects/{doomed}/versions/1/files/index.html']:
                assert get(path, admin)[0] == 404, path
            assert doomed not in [v['id'] for v in cli('list')['projects']]
            cli('config', '--project', doomed, '--expires-in', 'none', ok=False)
            def cleanup_complete():
                if (data/'projects'/doomed).exists():
                    return False
                with sqlite3.connect(data/'sss.db') as db:
                    return db.execute('SELECT count(*) FROM projects WHERE id=?', [doomed]).fetchone()[0] == 0
            wait_for(cleanup_complete, seconds=35)
            cli('delete', '--project', p)
            cli('delete', '--project', noexpiry)
            restart_expired = cli('new', '--expires-in', '2s')['id']
            cli('sync', str(public), '--project', restart_expired)
            stop()
            time.sleep(2.1)
            start()
            assert get(f'/api/projects/{restart_expired}', admin)[0] == 404
            assert not (data/'projects'/restart_expired).exists()
            stop()
            result = cli('doctor', ok=False)
            assert not result['ok'] and not result['checks']['health']['ok']
        finally:
            if watcher and watcher.poll() is None:
                watcher.kill()
                watcher.wait()
            stop()
    print('PASS: info/doctor, authenticated text/binary diff, debounced watch, expiration defaults/override/enforcement/cleanup/restart')
