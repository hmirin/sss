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

        def cli(*args, auth=admin, ok=True, stdin=None):
            e = dict(env)
            if auth is not None:
                e['SSS_BASIC_AUTH'] = auth
            result = subprocess.run([binary, *args], env=e, input=stdin,
                                    capture_output=True, text=True, timeout=20)
            assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
            return json.loads(result.stdout) if result.stdout else result.stderr

        def get(path, auth=None, body=None, method='GET', include_headers=False):
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
            result = response.status, response.read()
            return (*result, response.headers) if include_headers else result

        def wait_for(predicate, seconds=10):
            until = time.monotonic()+seconds
            while time.monotonic() < until:
                if predicate():
                    return
                time.sleep(.1)
            raise AssertionError('condition timed out')

        def start(default_lifetime="7d", default_action=None, extra_env=None):
            nonlocal server
            args = [binary, 'serve', '--port', str(port), '--data-dir', str(data), '--basic_auth', admin]
            if default_lifetime is not None:
                args += ['--default-expires-in', default_lifetime]
            if default_action is not None:
                args += ['--default-on-expire', default_action]
            server = subprocess.Popen(args, env=dict(env, **(extra_env or {})),
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

        def stored(project=None):
            with sqlite3.connect(data/'sss.db') as db:
                rows = []
                for table, key in [('projects', 'id'), ('versions', 'project'), ('expiry', 'project')]:
                    query = f'SELECT * FROM {table}'
                    args = []
                    if project is not None:
                        query += f' WHERE {key}=?'
                        args = [project]
                    rows.append(db.execute(query+' ORDER BY 1,2', args).fetchall())
            directory = data/'projects'
            if project is not None:
                directory /= project
            content = {str(p.relative_to(directory)): p.read_bytes()
                       for p in directory.rglob('*') if p.is_file()}
            return (*rows, content)

        def removed(project):
            return not (data/'projects'/project).exists() and stored(project) == ([], [], [], {})

        def expired_denials(project, revision):
            routes = [(f'/s/{project}/{path}', 'GET', None) for path in
                      ['', 'image.png', 'versions/1/', 'versions/1/image.png',
                       'versions/2/', 'versions/2/image.png']]
            routes += [(f'/api/projects/{project}{path}', 'GET', None) for path in
                       ['', '/manifest', '/versions', '/versions/1/files/index.html',
                        '/versions/1/files/image.png', '/versions/2/files/image.png']]
            routes += [(f'/api/projects/{project}', 'PATCH',
                        {'on_expire': 'delete', 'expires_in': 'none', 'name': 'must-not-save'}),
                       (f'/api/projects/{project}/versions', 'POST',
                        {'mode': 'upload', 'base_revision': revision, 'files': {}}),
                       (f'/api/projects/{project}/current', 'PUT',
                        {'version': 1, 'base_revision': revision}),
                       (f'/api/projects/{project}/versions/1', 'DELETE', None),
                       (f'/api/projects/{project}', 'DELETE', None)]
            for credential in [None, admin, 'reader:view-secret', 'writer:write-secret']:
                for path, method, body in routes:
                    status, _ = get(path, credential, body, method)
                    assert status == 404 or (credential != admin and status == 401), (path, method, status)
            assert project not in [p['id'] for p in cli('list')['projects']]

        try:
            start()
            status = cli('doctor')
            assert status['ok'] and status['checks']['health']['response']['version']
            assert status['checks']['server_admin_access']['response']['default_expires_in_seconds'] == 604800
            assert status['checks']['server_admin_access']['response']['default_on_expire'] == 'delete'
            assert not cli('doctor', auth=None, ok=False)['ok']
            created = cli('new', '--name', 'features', '--basic_auth_view', 'reader:view-secret',
                          '--basic_auth_write', 'writer:write-secret')
            p = created['id']
            assert abs(created['expires_at']-time.time()-604800) < 3
            assert created['expires_in_seconds'] == 604800
            assert created['on_expire'] == 'delete'
            listed = next(v for v in cli('list')['projects'] if v['id'] == p)
            assert listed['expires_in_seconds'] == 604800
            assert listed['expires_at'] == created['expires_at']
            assert listed['on_expire'] == 'delete'
            assert cli('doctor', '--project', p, auth='writer:write-secret')['ok']
            info = cli('info', '--project', p, auth='writer:write-secret')
            assert info['bytes'] == info['storage_bytes'] == info['files'] == 0
            assert info['expires_in_seconds'] == 604800
            assert info['auth']['view']['mode'] == 'basic'
            assert get(f'/api/projects/{p}', 'reader:view-secret')[0] == 401
            assert 'secret' not in json.dumps(info) and 'hash' not in json.dumps(info)
            cli('config', '--project', p, '--expires-in', 'none', auth='writer:write-secret', ok=False)
            patch = cli('config', '--project', p, '--expires-in', 'none', '--name', 'renamed')
            assert patch['expires_at'] is None and patch['expires_in_seconds'] is None
            assert cli('info', '--project', p)['expires_at'] is None
            assert cli('info', '--project', p)['expires_in_seconds'] is None
            assert cli('info', '--project', p)['name'] == 'renamed'
            assert get(f'/api/projects/{p}', admin, {'name': 'must-not-save', 'expires_in': 'bad'}, 'PATCH')[0] == 400
            assert cli('info', '--project', p)['name'] == 'renamed'
            settings = root / 'config.json'
            settings.write_text(json.dumps({'expires_in': '7d'}))
            cli('config', '--project', p, '--config', str(settings), '--expires-in', 'none')
            cli('config', '--project', p, '--expires-in', '-2d', ok=False)
            assert cli('info', '--project', p)['expires_at'] is None
            patch = cli('config', '--project', p, '--expires-in', '1h')
            assert patch['expires_in_seconds'] == 3600
            configured = cli('info', '--project', p)
            expires = configured['expires_at']
            assert configured['expires_in_seconds'] == 3600
            cli('config', '--project', p, '--name', 'features-renamed')
            name_only = cli('info', '--project', p)
            assert (name_only['expires_at'], name_only['expires_in_seconds']) == (expires, 3600)
            (public/'index.html').write_text('old\n')
            (public/'space #?.txt').write_text('special old\n')
            (public/'remove.txt').write_text('remove\n')
            cli('sync', str(public), '--project', p)
            info = cli('info', '--project', p)
            assert info['files'] == 3 and info['bytes'] == 23
            assert info['expires_in_seconds'] == 3600
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
            assert cli('info', '--project', inherited)['expires_in_seconds'] == 2592000
            cli('delete', '--project', inherited)
            assert cli('info', '--project', p)['expires_in_seconds'] == 3600
            noexpiry_created = cli('new', '--expires-in', 'none')
            noexpiry = noexpiry_created['id']
            assert noexpiry_created['expires_at'] is None
            assert noexpiry_created['expires_in_seconds'] is None
            noexpiry_info = cli('info', '--project', noexpiry)
            assert noexpiry_info['expires_at'] is None
            assert noexpiry_info['expires_in_seconds'] is None
            noexpiry_listed = next(v for v in cli('list')['projects'] if v['id'] == noexpiry)
            assert noexpiry_listed['expires_at'] is None
            assert noexpiry_listed['expires_in_seconds'] is None
            idle_public = root/'idle-public'
            idle_public.mkdir()
            (idle_public/'index.html').write_text('idle first version\n')
            idle = cli('new', '--expires-in', '6s')['id']
            first_publish = cli('sync', str(idle_public), '--project', idle)
            first_deadline = first_publish['expires_at']
            assert cli('info', '--project', idle)['expires_in_seconds'] == 6
            time.sleep(3.1)
            (idle_public/'index.html').write_text('idle second version\n')
            second_publish = cli('sync', str(idle_public), '--project', idle)
            assert second_publish['expires_at'] > first_deadline
            second_deadline = second_publish['expires_at']
            before_name = cli('info', '--project', idle)
            cli('config', '--project', idle, '--name', 'idle-renamed')
            after_name = cli('info', '--project', idle)
            assert (after_name['expires_at'], after_name['expires_in_seconds']) == (
                before_name['expires_at'], before_name['expires_in_seconds'])
            wait_for(lambda: int(time.time()) > first_deadline, seconds=8)
            assert get(f'/s/{idle}/', admin)[0] == 200
            rolled_back = cli('rollback', '1', '--project', idle)
            assert rolled_back['expires_in_seconds'] == 6
            assert rolled_back['expires_at'] > second_deadline
            wait_for(lambda: int(time.time()) >= rolled_back['expires_at'], seconds=8)
            for path in [f'/s/{idle}/', f'/s/{idle}/versions/1/', f'/api/projects/{idle}',
                         f'/api/projects/{idle}/manifest', f'/api/projects/{idle}/versions/1/files/index.html']:
                assert get(path, admin)[0] == 404, path
            assert idle not in [v['id'] for v in cli('list')['projects']]
            cli('config', '--project', idle, '--expires-in', 'none', ok=False)
            def cleanup_complete():
                if (data/'projects'/idle).exists():
                    return False
                with sqlite3.connect(data/'sss.db') as db:
                    return db.execute('SELECT count(*) FROM projects WHERE id=?', [idle]).fetchone()[0] == 0
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

            start(None, extra_env={'SSS_DEFAULT_ON_EXPIRE': 'retain'})
            defaults = cli('doctor')['checks']['server_admin_access']['response']
            assert defaults['default_on_expire'] == 'retain'
            assert defaults['default_expires_in_seconds'] is None
            selected = cli('new')
            selected_id = selected['id']
            assert selected['on_expire'] == 'retain' and selected['expires_at'] is None
            assert cli('info', '--project', selected_id)['on_expire'] == 'retain'
            assert next(p for p in cli('list')['projects'] if p['id'] == selected_id)['on_expire'] == 'retain'
            for value, expected in [('delete', 'delete'), (None, 'retain')]:
                created = cli('new', '--config', '-', stdin=json.dumps({'on_expire': value}))
                assert created['on_expire'] == expected
            created = cli('new', '--config', '-', '--on-expire', 'delete',
                          stdin=json.dumps({'on_expire': 'retain'}))
            assert created['on_expire'] == 'delete'
            created = cli('new', '--config', '-', '--on-expire', 'retain',
                          stdin=json.dumps({'on_expire': 'delete'}))
            assert created['on_expire'] == 'retain'
            configured = cli('config', '--project', selected_id, '--expires-in', '1h')
            assert configured['on_expire'] == 'retain'
            deadline = configured['expires_at']
            expiry_row = stored(selected_id)[2]
            time.sleep(1.1)
            configured = cli('config', '--project', selected_id, '--on-expire', 'delete')
            assert configured['on_expire'] == 'delete' and configured['expires_at'] == deadline
            assert stored(selected_id)[2] == expiry_row
            configured = cli('config', '--project', selected_id, '--config', '-', '--on-expire', 'retain',
                             stdin=json.dumps({'on_expire': 'delete'}))
            assert configured['on_expire'] == 'retain' and configured['expires_at'] == deadline
            for body in [{'name': 'omitted'}, {'name': 'null', 'on_expire': None}]:
                configured = cli('config', '--project', selected_id, '--config', '-', stdin=json.dumps(body))
                assert configured['on_expire'] == 'retain' and configured['expires_at'] == deadline
            before = stored()
            assert get(f'/api/projects/{selected_id}', admin, {'on_expire': None}, 'PATCH')[0] == 400
            assert stored() == before
            for value in ['keep', 'Retain', '', 1, True, [], {}, {'retain': None}, {'delete': None}]:
                before = stored()
                body = {'name': 'must-not-save', 'on_expire': value, 'expires_in': 'none'}
                assert get(f'/api/projects/{selected_id}', admin, body, 'PATCH')[0] in [400, 422]
                assert get('/api/projects', admin, body, 'POST')[0] in [400, 422]
                assert stored() == before
            for args in [('new', '--on-expire', 'keep'),
                         ('config', '--project', selected_id, '--on-expire', 'keep')]:
                before = stored()
                cli(*args, ok=False)
                assert stored() == before
            configured = cli('config', '--project', selected_id, '--expires-in', 'none')
            assert configured['on_expire'] == 'retain' and configured['expires_at'] is None
            assert stored(selected_id)[2] == []
            configured = cli('config', '--project', selected_id, '--expires-in', '1h')
            assert configured['on_expire'] == 'retain' and configured['expires_at'] is not None
            stop()
            start(None, 'delete', {'SSS_DEFAULT_ON_EXPIRE': 'retain'})
            assert cli('doctor')['checks']['server_admin_access']['response']['default_on_expire'] == 'delete'
            assert cli('new')['on_expire'] == 'delete'
            assert cli('info', '--project', selected_id)['on_expire'] == 'retain'
            stop()
            before = stored()
            for args, overrides in [(['--default-on-expire', 'keep'], {}),
                                    ([], {'SSS_DEFAULT_ON_EXPIRE': 'keep'})]:
                result = subprocess.run([binary, 'serve', '--data-dir', str(data), *args],
                                        env=dict(env, **overrides), capture_output=True, timeout=5)
                assert result.returncode != 0 and stored() == before

            start(None)
            snapshots = root/'retention-public'
            snapshots.mkdir()
            image = base64.b64decode('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+jJ9kAAAAASUVORK5CYII=')
            published = {}
            for action in ['delete', 'retain']:
                created = cli('new', '--expires-in', '1h', '--on-expire', action,
                              '--basic_auth_view', 'none', '--basic_auth_write', 'writer:write-secret')
                project_id = created['id']
                for version in [1, 2]:
                    (snapshots/'index.html').write_text(f'<img src="image.png">{action} {version}')
                    (snapshots/'image.png').write_bytes(image + bytes([version]))
                    result = cli('sync', str(snapshots), '--project', project_id)
                    assert result['version'] == version
                published[action] = (project_id, result['revision'])
                for path in ['', 'image.png', 'versions/1/image.png', 'versions/2/image.png']:
                    status, _, headers = get(f'/s/{project_id}/{path}', include_headers=True)
                    assert status == 200 and headers.get('Cache-Control') == 'no-store'
                status, content, headers = get(f'/api/projects/{project_id}/versions/1/files/image.png',
                                               admin, include_headers=True)
                assert status == 200 and content == image+b'\x01'
                assert headers.get('Cache-Control') == 'no-store'
            deadline = int(time.time()) + 2
            with sqlite3.connect(data/'sss.db') as db:
                for project_id, _ in published.values():
                    db.execute('UPDATE expiry SET expires_at=? WHERE project=?', [deadline, project_id])
            expected = {action: stored(project_id) for action, (project_id, _) in published.items()}
            wait_for(lambda: int(time.time()) >= deadline, seconds=3)
            for project_id, _ in published.values():
                assert get(f'/s/{project_id}/')[0] == 404
            for action, (project_id, revision) in published.items():
                expired_denials(project_id, revision)
                assert stored(project_id) == expected[action]
                assert (data/'projects'/project_id).exists()
            deleted_id, _ = published['delete']
            retained_id, retained_revision = published['retain']
            wait_for(lambda: removed(deleted_id), seconds=35)
            assert stored(retained_id) == expected['retain']
            expired_denials(retained_id, retained_revision)
            startup_deleted = cli('new', '--expires-in', '1h', '--on-expire', 'delete')['id']
            cli('sync', str(snapshots), '--project', startup_deleted)
            stop()
            with sqlite3.connect(data/'sss.db') as db:
                db.execute('UPDATE expiry SET expires_at=? WHERE project=?',
                           [int(time.time())-1, startup_deleted])
            start(None)
            assert removed(startup_deleted)
            assert stored(retained_id) == expected['retain']
            expired_denials(retained_id, retained_revision)
            stop()

            data = root/'legacy-data'
            data.mkdir()
            auth = json.dumps({'view': {'mode': 'none'}, 'write': {'mode': 'none'}})
            deadline = int(time.time())+3600
            with sqlite3.connect(data/'sss.db') as db:
                db.executescript('''
                    CREATE TABLE projects(id TEXT PRIMARY KEY,name TEXT NOT NULL,revision TEXT NOT NULL,
                        current INTEGER NOT NULL DEFAULT 0,next INTEGER NOT NULL DEFAULT 1,auth TEXT NOT NULL);
                    CREATE TABLE versions(project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                        number INTEGER NOT NULL,PRIMARY KEY(project,number));
                    CREATE TABLE expiry(project TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,
                        lifetime INTEGER NOT NULL CHECK(lifetime>0),expires_at INTEGER NOT NULL);
                    CREATE INDEX expiry_time ON expiry(expires_at);
                    PRAGMA user_version=2;
                ''')
                for project_id in ['0123456789ab', 'abcdef012345']:
                    db.execute('INSERT INTO projects VALUES(?,?,?,?,?,?)',
                               [project_id, project_id, 'original-revision', 3, 4, auth])
                    for version in [1, 3]:
                        db.execute('INSERT INTO versions VALUES(?,?)', [project_id, version])
                        directory = data/'projects'/project_id/str(version)
                        directory.mkdir(parents=True)
                        (directory/'index.html').write_text(f'{project_id} version {version}')
                        (directory/'image.png').write_bytes(image+bytes([version]))
                db.execute('INSERT INTO expiry VALUES(?,?,?)', ['0123456789ab', 3600, deadline])
            before = stored()
            start(None, 'retain')
            after = stored()
            assert [row[:-1] for row in after[0]] == before[0]
            assert all(row[-1] == 'delete' for row in after[0])
            assert after[1:] == before[1:]
            with sqlite3.connect(data/'sss.db') as db:
                assert db.execute('PRAGMA user_version').fetchone()[0] == 3
                assert db.execute('PRAGMA integrity_check').fetchone()[0] == 'ok'
                column = next(c for c in db.execute('PRAGMA table_info(projects)') if c[1] == 'on_expire')
                assert column[2] == 'TEXT' and column[3] == 1
            for project_id in ['0123456789ab', 'abcdef012345']:
                info = cli('info', '--project', project_id)
                assert info['on_expire'] == 'delete'
                assert info['expires_at'] == (deadline if project_id == '0123456789ab' else None)
                assert get(f'/s/{project_id}/versions/1/image.png')[1] == image+b'\x01'
                assert get(f'/s/{project_id}/image.png')[1] == image+b'\x03'
            stop()
            start(None, 'retain')
            assert stored() == after
            stop()
            with sqlite3.connect(data/'sss.db') as db:
                db.execute('PRAGMA user_version=1')
            before = stored()
            result = subprocess.run([binary, 'serve', '--data-dir', str(data)],
                                    env=env, text=True, capture_output=True, timeout=5)
            assert result.returncode != 0 and 'unsupported data format' in result.stderr
            assert stored() == before
            with sqlite3.connect(data/'sss.db') as db:
                assert db.execute('PRAGMA user_version').fetchone()[0] == 1

            data = root/'failed-upgrade-data'
            data.mkdir()
            with sqlite3.connect(data/'sss.db') as db:
                db.executescript('''
                    CREATE TABLE projects(id TEXT PRIMARY KEY,name TEXT NOT NULL,revision TEXT NOT NULL,
                        current INTEGER NOT NULL DEFAULT 0,next INTEGER NOT NULL DEFAULT 1,auth TEXT NOT NULL);
                    CREATE TABLE versions(project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
                        number INTEGER NOT NULL,PRIMARY KEY(project,number));
                    CREATE TABLE expiry(project TEXT PRIMARY KEY,lifetime INTEGER NOT NULL);
                    PRAGMA user_version=2;
                ''')
                db.execute('INSERT INTO projects VALUES(?,?,?,?,?,?)', ['atomic', 'atomic', 'revision', 1, 2, auth])
                db.execute('INSERT INTO versions VALUES(?,?)', ['atomic', 1])
            directory = data/'projects'/'atomic'/'1'
            directory.mkdir(parents=True)
            (directory/'image.png').write_bytes(image)
            before = stored()
            result = subprocess.run([binary, 'serve', '--data-dir', str(data)],
                                    env=env, text=True, capture_output=True, timeout=5)
            assert result.returncode != 0 and 'no such column' in result.stderr
            assert stored() == before
            with sqlite3.connect(data/'sss.db') as db:
                assert db.execute('PRAGMA user_version').fetchone()[0] == 2
                assert all(c[1] != 'on_expire' for c in db.execute('PRAGMA table_info(projects)'))
        finally:
            if watcher and watcher.poll() is None:
                watcher.kill()
                watcher.wait()
            stop()
    print('PASS: info/doctor, authenticated text/binary diff, debounced watch, expiration defaults/override/enforcement/cleanup/restart, retention defaults/precedence/null/rejection/deadlines, retained bytes/rows across recurring and startup cleanup, format 2 upgrade preservation, failed-upgrade atomic rollback and format refusal')
