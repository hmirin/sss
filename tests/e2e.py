#!/usr/bin/env python3
"""Disposable black-box acceptance tests. No production services or credentials."""
import base64
import json
import os
from pathlib import Path
import socket
import sqlite3
import subprocess
import sys
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen

BINARY = str(Path(sys.argv[1]).resolve())


def exercise(shared):
    with tempfile.TemporaryDirectory(prefix='sss-e2e-') as tmp:
        root = Path(tmp)
        work = root / 'work'
        work.mkdir()
        data = root / 'data'
        env = {k: v for k, v in os.environ.items() if not k.startswith('SSS_')}
        env['HOME'] = str(root)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        upstream = f'http://127.0.0.1:{port}'
        process = None
        log = open(root / 'server.log', 'w+')

        def cli(*args, credential=shared, ok=True, stdin=None, extra_env=None):
            e = dict(env, SSS_UPSTREAM=upstream)
            if credential is not None:
                e['SSS_BASIC_AUTH'] = credential
            e.update(extra_env or {})
            result = subprocess.run([BINARY, *args], cwd=work, env=e, input=stdin,
                                    text=True, capture_output=True, timeout=30)
            assert (result.returncode == 0) == ok, (args, result.stderr)
            return json.loads(result.stdout) if ok else result.stderr

        def request(path, method='GET', body=None, credential=None):
            headers = {}
            if credential:
                headers['Authorization'] = 'Basic ' + base64.b64encode(credential.encode()).decode()
            if body is not None:
                headers['Content-Type'] = 'application/json'
            try:
                response = urlopen(Request(upstream + path, method=method, headers=headers,
                    data=None if body is None else json.dumps(body).encode()), timeout=10)
            except HTTPError as err:
                response = err
            return response.status, response.read()

        def start():
            nonlocal process
            e = dict(env, SSS_LISTEN='127.0.0.1', SSS_PORT=str(port),
                     SSS_DATA_DIR=str(data), SSS_PUBLIC_URL=upstream)
            if shared:
                e['SSS_BASIC_AUTH'] = shared
            process = subprocess.Popen([BINARY, 'serve'], env=e, stdout=log, stderr=log)
            for _ in range(150):
                try:
                    if request('/health')[0] == 200:
                        return
                except OSError:
                    pass
                if process.poll() is not None:
                    log.seek(0)
                    raise AssertionError(log.read())
                time.sleep(.05)
            raise AssertionError('server readiness timed out')

        def stop():
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=10)

        try:
            start()
            if shared:
                assert request('/api/projects', 'POST', {})[0] == 401
                assert request('/api/projects')[0] == 401
                assert 'SSS_BASIC_AUTH' in cli('new', credential=None, ok=False)
            # Repeated creation in one directory, JSON by default, no binding file.
            first = cli('new', '--name', 'hello')
            p = first['id']
            other = cli('new', '--name', 'other')['id']
            assert p != other and first['url'] == f'{upstream}/s/{p}/'
            assert not (work / '.sss.json').exists()
            assert len(cli('list')['projects']) == 2
            cli('sync', '.', ok=False)  # No implicit last-used project.
            # Upstream flag overrides environment; Basic flag overrides bad environment.
            cli('--upstream', upstream, 'list', extra_env={'SSS_UPSTREAM': 'http://127.0.0.1:1'})
            if shared:
                cli('list', '--basic_auth', shared, credential='wrong:password')
            (work / 'index.html').write_text('one')
            (work / 'assets').mkdir()
            (work / 'assets/style.css').write_text('body{}')
            v1 = cli('sync', '.', '--project', p)
            assert v1['version'] == 1
            assert v1['version_url'] == f'{upstream}/s/{p}/versions/1/'
            for path in ['', 'versions/1/', 'versions/1/assets/style.css']:
                assert request(f'/s/{p}/{path}', credential=shared)[0] == 200
                if shared:
                    assert request(f'/s/{p}/{path}')[0] == 401
            (work / 'index.html').write_text('two')
            v2 = cli('upload', 'index.html', '--project', p)
            assert v2['version'] == 2
            cli('sync', 'index.html', '--project', p, ok=False)
            assert request(f'/s/{p}/assets/style.css', credential=shared)[1] == b'body{}'
            assert request(f'/s/{p}/versions/1/', credential=shared)[1] == b'one'
            assert request(f'/s/{p}/', credential=shared)[1] == b'two'
            # Dry-run distinguishes additions, modifications and deletions.
            (work / 'assets/style.css').unlink()
            (work / 'index.html').write_text('three')
            (work / 'new.txt').write_text('new')
            (work / '.env').write_text('SECRET=fixture')
            (work / '.sssignore').write_text('ignored.txt\n')
            (work / 'ignored.txt').write_text('ignored')
            preview = cli('sync', '.', '--project', p, '--dry-run')
            assert preview == {'project': p, 'dry_run': True, 'added': ['new.txt'],
                               'modified': ['index.html'], 'deleted': ['assets/style.css']}
            assert request(f'/s/{p}/', credential=shared)[1] == b'two'
            v3 = cli('sync', '.', '--project', p)
            assert v3['version'] == 3
            for name in ['.env', 'ignored.txt', 'assets/style.css']:
                assert request(f'/s/{p}/{name}', credential=shared)[0] == 404
            assert cli('sync', '.', '--project', p)['unchanged']
            assert cli('versions', '--project', p)['versions'] == [1, 2, 3]
            # Reject traversal, reserved version paths and stale writes atomically.
            status, payload = request(f'/api/projects/{p}/manifest', credential=shared)
            manifest = json.loads(payload)
            for path in ['../escape', '/absolute', 'x/../../escape', 'x\\y', 'versions/1/index.html']:
                body = {'mode': 'upload', 'base_revision': manifest['revision'],
                        'files': {'index.html': base64.b64encode(b'bad').decode(), path: ''}}
                assert request(f'/api/projects/{p}/versions', 'POST', body, shared)[0] == 400
            assert request(f'/api/projects/{p}/versions', 'POST',
                {'mode': 'upload', 'base_revision': 'stale', 'files': {}}, shared)[0] == 409
            (work / 'link').symlink_to(root / 'server.log')
            cli('sync', '.', '--project', p, ok=False)
            (work / 'link').unlink()
            # Rollback changes concurrency token, preserves snapshots; deleted numbers never reused.
            rollback = cli('rollback', '1', '--project', p)
            assert rollback['revision'] != manifest['revision']
            assert request(f'/s/{p}/', credential=shared)[1] == b'one'
            cli('delete-version', '1', '--project', p, ok=False)
            cli('delete-version', '2', '--project', p)
            assert request(f'/s/{p}/versions/2/', credential=shared)[0] == 404
            assert cli('sync', '.', '--project', p)['version'] == 4
            stop()
            start()
            assert cli('versions', '--project', p) == {'project': p, 'current': 4, 'versions': [1, 3, 4]}
            assert request(f'/s/{p}/versions/1/', credential=shared)[1] == b'one'
            # Per-project credentials and overrides (also when no shared credential exists).
            config = {'name': 'protected', 'auth': {'view': {'mode': 'basic', 'username': 'reader',
                'password': 'view-password'}, 'write': {'mode': 'basic', 'username': 'writer', 'password': 'write:password'}}}
            private = cli('new', '--config', '-', stdin=json.dumps(config))['id']
            cli('sync', '.', '--project', private, credential=None, ok=False)
            cli('sync', '.', '--project', private, credential='reader:view-password', ok=False)
            cli('sync', '.', '--project', private, credential='writer:write:password')
            assert request(f'/s/{private}/')[0] == 401
            assert request(f'/s/{private}/versions/1/', credential='reader:view-password')[0] == 200
            assert request(f'/s/{private}/versions/1/', credential='writer:write:password')[0] == 401
            if shared:
                cli('new', credential='writer:write:password', ok=False)
                cli('sync', '.', '--project', other, credential='writer:write:password', ok=False)
                cli('config', '--project', private, '--basic_auth_view', 'none', credential='writer:write:password', ok=False)
                assert request(f'/s/{private}/', credential=shared)[0] == 200
            cli('config', '--project', private, '--basic_auth_view', 'reader:new-password')
            assert request(f'/s/{private}/versions/1/', credential='reader:view-password')[0] == 401
            assert request(f'/s/{private}/versions/1/', credential='reader:new-password')[0] == 200
            cli('config', '--project', private, '--basic_auth_view', 'none', '--basic_auth_write', 'none')
            assert request(f'/s/{private}/')[0] == 200
            cli('sync', '.', '--project', private, credential=None)
            cli('config', '--project', private, '--basic_auth_view', 'inherit', '--basic_auth_write', 'inherit')
            assert request(f'/s/{private}/')[0] == (401 if shared else 200)
            # Flags override JSON settings; no cleartext credentials in database.
            public = cli('new', '--config', '-', '--basic_auth_view', 'none', '--basic_auth_write', 'none', stdin=json.dumps(config))['id']
            cli('sync', '.', '--project', public, credential=None)
            assert request(f'/s/{public}/')[0] == 200
            db = sqlite3.connect(data / 'sss.db')
            stored = '\n'.join(r[0] for r in db.execute('SELECT auth FROM projects'))
            for secret in ['view-password', 'write:password', 'new-password']:
                assert secret not in stored
            db.close()
            cli('delete', 'index.html', '--project', p)
            assert request(f'/s/{p}/', credential=shared)[0] == 404
            assert request(f'/s/{p}/versions/1/', credential=shared)[1] == b'one'
            for project in [p, other, private, public]:
                cli('delete', '--project', project)
            assert cli('list')['projects'] == []
        finally:
            stop()
            log.close()


def exercise_request_timeout():
    with tempfile.TemporaryDirectory(prefix='sss-timeout-') as tmp:
        root = Path(tmp)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        upstream = f'http://127.0.0.1:{port}'
        env = {k: v for k, v in os.environ.items() if not k.startswith('SSS_')}
        env.update(SSS_LISTEN='127.0.0.1', SSS_PORT=str(port),
                   SSS_DATA_DIR=str(root / 'data'), SSS_PUBLIC_URL=upstream,
                   SSS_REQUEST_TIMEOUT_SECONDS='5')
        with open(root / 'server.log', 'w+') as log:
            server = subprocess.Popen([BINARY, 'serve'], env=env, stdout=log, stderr=log)
            sockets = []
            try:
                for _ in range(150):
                    try:
                        if urlopen(upstream + '/health', timeout=1).status == 200:
                            break
                    except OSError:
                        pass
                    if server.poll() is not None:
                        log.seek(0)
                        raise AssertionError(log.read())
                    time.sleep(.05)
                else:
                    raise AssertionError('server readiness timed out')

                for _ in range(2):
                    sock = socket.create_connection(('127.0.0.1', port), timeout=2)
                    sockets.append(sock)
                    sock.sendall(b'POST /api/projects HTTP/1.1\r\nHost: localhost\r\n'
                                 b'Content-Type: application/json\r\nContent-Length: 100\r\n'
                                 b'Connection: keep-alive\r\n\r\n{"name":"unfinished"')
                busy = False
                busy_deadline = time.monotonic() + 3.5
                while time.monotonic() < busy_deadline:
                    try:
                        urlopen(upstream + '/health', timeout=1)
                    except HTTPError as err:
                        if err.code == 503:
                            busy = True
                            break
                        raise
                    time.sleep(.02)
                assert busy, 'stalled bodies should consume both permits initially'

                recovered = False
                recovery_deadline = time.monotonic() + 8
                while time.monotonic() < recovery_deadline:
                    try:
                        recovered = urlopen(upstream + '/health', timeout=1).status == 200
                    except HTTPError as err:
                        assert err.code == 503, err.code
                    if recovered:
                        break
                    time.sleep(.02)
                assert recovered, 'permits did not recover while stalled sockets remained open'
                sockets[0].settimeout(6)
                assert b' 408 ' in sockets[0].recv(4096).split(b'\r\n', 1)[0]
                body = json.dumps({'name': 'control'}).encode()
                response = urlopen(Request(upstream + '/api/projects', method='POST',
                                           headers={'Content-Type': 'application/json'},
                                           data=body), timeout=2)
                assert response.status == 200
            finally:
                for sock in sockets:
                    sock.close()
                server.terminate()
                server.wait(timeout=10)


def exercise_storage_quota():
    with tempfile.TemporaryDirectory(prefix='sss-quota-') as tmp:
        root = Path(tmp)
        work = root / 'work'
        work.mkdir()
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        upstream = f'http://127.0.0.1:{port}'
        env = {k: v for k, v in os.environ.items() if not k.startswith('SSS_')}
        env.update(SSS_LISTEN='127.0.0.1', SSS_PORT=str(port),
                   SSS_DATA_DIR=str(root / 'data'), SSS_PUBLIC_URL=upstream,
                   SSS_UPSTREAM=upstream, SSS_MAX_TOTAL_STORAGE='256KiB')
        server = None

        def start():
            nonlocal server
            server = subprocess.Popen([BINARY, 'serve'], env=env,
                                      stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
            for _ in range(150):
                try:
                    if urlopen(upstream + '/health', timeout=1).status == 200:
                        return
                except OSError:
                    pass
                if server.poll() is not None:
                    raise AssertionError(server.stderr.read().decode())
                time.sleep(.05)
            raise AssertionError('quota server readiness timed out')

        def cli(*args, ok=True):
            result = subprocess.run([BINARY, *args], cwd=work, env=env,
                                    text=True, capture_output=True, timeout=30)
            assert (result.returncode == 0) == ok, (args, result.stderr)
            return json.loads(result.stdout) if ok else result.stderr

        def request(path, body):
            try:
                response = urlopen(Request(upstream + path, method='POST',
                                           headers={'Content-Type': 'application/json'},
                                           data=json.dumps(body).encode()), timeout=5)
            except HTTPError as err:
                response = err
            return response.status, json.loads(response.read())

        try:
            start()
            a = cli('new', '--max-storage', '200KiB')['id']
            b = cli('new', '--max-storage', '1MiB')['id']
            assert cli('info', '--project', a)['max_storage_bytes'] == 200 * 1024
            (work / 'index.html').write_bytes(b'ab')
            cli('upload', 'index.html', '--project', a)
            (work / 'index.html').write_bytes(b'cd')
            cli('upload', 'index.html', '--project', a)
            before = cli('info', '--project', a)
            assert before['storage_bytes'] == 4 and before['version'] == 2
            assert before['quota_used_bytes'] > before['storage_bytes']
            status, _ = request(f'/api/projects/{a}/versions',
                                {'mode': 'upload', 'files': {}, 'base_revision': before['revision']})
            assert status == 507
            assert cli('info', '--project', a) == before
            assert cli('versions', '--project', a)['versions'] == [1, 2]
            cli('delete-version', '1', '--project', a)
            status, published = request(f'/api/projects/{a}/versions',
                                        {'mode': 'upload', 'files': {}, 'base_revision': before['revision']})
            assert status == 200 and published['version'] == 3
            assert cli('config', '--project', a, '--max-storage', '100KiB')['max_storage_bytes'] == 100 * 1024
            assert cli('info', '--project', a)['storage_bytes'] == 4
            status, _ = request(f'/api/projects/{a}/versions',
                                {'mode': 'upload', 'files': {}, 'base_revision': published['revision']})
            assert status == 507
            cli('delete-version', '2', '--project', a)
            assert cli('info', '--project', a)['storage_bytes'] == 2

            (work / 'index.html').write_bytes(b'wxyz')
            cli('upload', 'index.html', '--project', b)
            (work / 'index.html').write_bytes(b'pq')
            cli('upload', 'index.html', '--project', b)
            before = cli('info', '--project', b)
            assert before['storage_bytes'] == 6
            assert json.loads(urlopen(upstream + '/api/status').read())['max_total_storage_bytes'] == 256 * 1024
            status, _ = request(f'/api/projects/{b}/versions',
                                {'mode': 'upload', 'files': {}, 'base_revision': before['revision']})
            assert status == 507
            empty = cli('new')['id']
            empty_revision = cli('info', '--project', empty)['revision']
            status, _ = request(f'/api/projects/{empty}/versions',
                                {'mode': 'sync', 'files': {}, 'keep': [], 'base_revision': empty_revision})
            assert status == 507
            for _ in range(7):
                cli('new')
            assert '507' in cli('new', ok=False)
            (work / 'index.html').write_bytes(b'r')
            assert '507' in cli('upload', 'index.html', '--project', b, ok=False)
            assert cli('info', '--project', b) == before
            cli('delete-version', '1', '--project', b)
            cli('upload', 'index.html', '--project', b)
            assert cli('info', '--project', b)['storage_bytes'] == 3
            quota_before_restart = json.loads(urlopen(upstream + '/api/status').read())['quota_used_bytes']

            server.terminate()
            server.wait(timeout=10)
            server.stderr.close()
            start()
            assert cli('info', '--project', a)['max_storage_bytes'] == 100 * 1024
            assert cli('info', '--project', b)['storage_bytes'] == 3
            assert json.loads(urlopen(upstream + '/api/status').read())['quota_used_bytes'] == quota_before_restart
            assert cli('new', '--max-storage', '0B', ok=False)
        finally:
            if server and server.poll() is None:
                server.terminate()
                server.wait(timeout=10)
            if server and server.stderr:
                server.stderr.close()


if __name__ == '__main__':
    exercise(None)
    exercise('admin:shared-password')
    exercise_request_timeout()
    exercise_storage_quota()
    from features import exercise_features
    exercise_features(BINARY)
    skill = subprocess.check_output([BINARY, '--skill'], text=True)
    assert skill.startswith('---\nname: sss\n')
    assert 'SSS_API_KEY' not in skill
    print('PASS: anonymous/shared/project auth, CLI precedence, multi-project workflow, version snapshots/rollback/deletion/restart, sync/dry-run/exclusions, atomic rejection, embedded skill')
