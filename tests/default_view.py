"""Disposable acceptance coverage for new-project viewing defaults."""
import base64
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.error import HTTPError
from urllib.request import Request, urlopen


def exercise_default_view(binary):
    with tempfile.TemporaryDirectory(prefix='sss-default-view-') as tmp:
        root = Path(tmp)
        public = root / 'public'
        public.mkdir()
        (public / 'index.html').write_text('default-view-content')
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        origin = f'http://127.0.0.1:{port}'
        env = {k: v for k, v in os.environ.items() if not k.startswith('SSS_')}
        env.update(HOME=str(root), SSS_UPSTREAM=origin)
        admin = 'admin:disposable-secret'
        reader = 'reader:disposable-view-secret'
        server = None

        def request(path, auth=None, method='GET', body=None):
            headers = {}
            if auth:
                headers['Authorization'] = 'Basic ' + base64.b64encode(auth.encode()).decode()
            if body is not None:
                headers['Content-Type'] = 'application/json'
            try:
                response = urlopen(Request(origin + path, headers=headers, method=method,
                    data=None if body is None else json.dumps(body).encode()), timeout=5)
            except HTTPError as error:
                response = error
            with response:
                return response.status, response.read(), response.headers

        def cli(*args, auth=admin, ok=True):
            cli_env = dict(env)
            if auth is not None:
                cli_env['SSS_BASIC_AUTH'] = auth
            result = subprocess.run([binary, *args], env=cli_env, capture_output=True,
                                    text=True, timeout=20)
            assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
            return json.loads(result.stdout) if result.stdout else result.stderr

        def start(*args, default_env=None):
            nonlocal server
            server_env = dict(env, SSS_BASIC_AUTH=admin)
            if default_env is not None:
                server_env['SSS_DEFAULT_VIEW_AUTH'] = default_env
            server = subprocess.Popen([binary, 'serve', '--port', str(port), '--data-dir',
                str(root / 'data'), *args], env=server_env, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                assert server.poll() is None, 'server exited before becoming ready'
                try:
                    if request('/health')[0] == 200:
                        return
                except OSError:
                    pass
                time.sleep(.05)
            raise AssertionError('server did not become ready')

        def stop():
            if server and server.poll() is None:
                server.terminate()
                server.wait(timeout=10)

        def publish(*args):
            project = cli('new', *args)['id']
            cli('sync', str(public), '--project', project)
            return project

        def info(project):
            return cli('info', '--project', project)['auth']

        def status():
            return cli('doctor')['checks']['server_admin_access']['response']['default_view_auth']

        try:
            start()
            assert status() == 'inherit'
            inherited = publish()
            explicit_none = publish('--basic_auth_view', 'none')
            assert request(f'/s/{inherited}/')[0] == 401
            assert request(f'/s/{explicit_none}/')[0] == 200
            stop()

            start(default_env='none')
            assert status() == 'none'
            assert request(f'/s/{inherited}/')[0] == 401
            assert request(f'/s/{explicit_none}/')[0] == 200
            public_project = publish()
            assert info(public_project)['view']['mode'] == 'none'
            assert info(public_project)['write']['mode'] == 'inherit'
            assert request(f'/s/{public_project}/')[0] == 200
            assert request(f'/s/{public_project}/versions/1/')[0] == 200
            private_project = publish('--basic_auth_view', 'inherit')
            assert request(f'/s/{private_project}/')[0] == 401
            basic_project = publish('--basic_auth_view', reader)
            assert request(f'/s/{basic_project}/')[0] == 401
            assert request(f'/s/{basic_project}/', 'reader:wrong')[0] == 401
            code, body, headers = request(f'/s/{basic_project}/', reader)
            assert code == 200 and body == b'default-view-content'
            assert 'Basic' in request(f'/s/{basic_project}/')[2]['WWW-Authenticate']
            assert request(f'/s/{basic_project}/versions/1/index.html', reader)[0] == 200
            code, body, _ = request('/api/projects', admin, 'POST',
                {'auth': {'write': {'mode': 'inherit'}}})
            assert code == 200
            write_only = json.loads(body)['id']
            assert info(write_only)['view']['mode'] == 'none'
            assert info(write_only)['write']['mode'] == 'inherit'
            for auth in (None, reader):
                assert request('/api/projects', auth)[0] == 401
                assert request('/api/projects', auth, 'POST', {})[0] == 401
                for project in (public_project, basic_project):
                    assert request(f'/api/projects/{project}/manifest', auth)[0] == 401
                    assert request(f'/api/projects/{project}', auth, 'PATCH', {'name': 'denied'})[0] == 401
                    assert request(f'/api/projects/{project}', auth, 'DELETE')[0] == 401
                    cli('sync', str(public), '--project', project, auth=auth, ok=False)
            cli('sync', str(public), '--project', public_project)
            stop()

            start('--default-view-auth', 'inherit', default_env='none')
            assert status() == 'inherit'
            assert request(f'/s/{public_project}/')[0] == 200
            assert request(f'/s/{basic_project}/', reader)[0] == 200
            assert info(public_project)['write']['mode'] == 'inherit'
            new_inherited = publish()
            assert request(f'/s/{new_inherited}/')[0] == 401
            stop()
            for args, invalid_env in ((['--default-view-auth', 'basic'], None), ([], 'basic')):
                error_env = dict(env)
                if invalid_env is not None:
                    error_env['SSS_DEFAULT_VIEW_AUTH'] = invalid_env
                result = subprocess.run([binary, 'serve', *args], env=error_env,
                    capture_output=True, text=True, timeout=10)
                assert result.returncode != 0 and 'invalid value' in result.stderr, result.stderr
        finally:
            stop()
    print('PASS: default-view creation policy, explicit overrides, persisted rules, separate viewer/admin access')
