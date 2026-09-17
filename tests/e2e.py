#!/usr/bin/env python3
"""Disposable black-box integration checks; Python standard library only."""
import argparse, base64, json, os, socket, subprocess, tempfile, time
from pathlib import Path
from urllib.request import Request, urlopen
from urllib.error import HTTPError


def main():
    p = argparse.ArgumentParser()
    p.add_argument('binary', type=Path)
    binary = str(p.parse_args().binary.resolve())
    with tempfile.TemporaryDirectory(prefix='sss-e2e-') as tmp:
        root = Path(tmp)
        work = root / 'work'
        work.mkdir()
        data = root / 'data'
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        upstream = f'http://127.0.0.1:{port}'
        env = dict(os.environ)
        env.pop('SSS_API_KEY', None)
        def cli(*args, token=None, ok=True, stdin=None, parsed=False):
            e = dict(env)
            if token is not None:
                e['SSS_API_KEY'] = token
            r = subprocess.run([binary, *args], cwd=work, env=e, input=stdin, text=True, capture_output=True)
            assert (r.returncode == 0) == ok, (args, r.returncode, r.stderr)
            return json.loads(r.stdout) if parsed else r.stdout.strip()
        admin = cli('admin-key', '--data-dir', str(data))
        process = None
        log = open(root / 'server.log', 'w+')
        def request(path, method='GET', body=None, token=None, basic=None):
            headers = {}
            if token:
                headers['Authorization'] = 'Bearer ' + token
            if basic:
                headers['Authorization'] = 'Basic ' + base64.b64encode(basic.encode()).decode()
            payload = None
            if body is not None:
                payload = json.dumps(body).encode()
                headers['Content-Type'] = 'application/json'
            try:
                r = urlopen(Request(upstream + path, data=payload, headers=headers, method=method), timeout=5)
            except HTTPError as err:
                r = err
            return r.status, r.read()
        def api(path, method='GET', body=None, token=admin):
            status, payload = request(path, method, body, token)
            assert 200 <= status < 300, (path, status, payload)
            return json.loads(payload) if payload else {}
        def start():
            nonlocal process
            process = subprocess.Popen([binary, 'serve', '--listen', f'127.0.0.1:{port}', '--data-dir', str(data), '--public-url', upstream], stdout=log, stderr=log)
            for _ in range(100):
                try:
                    if request('/health')[0] == 200:
                        return
                except OSError:
                    pass
                if process.poll() is not None:
                    log.seek(0)
                    raise AssertionError('server exited: ' + log.read())
                time.sleep(.05)
            raise AssertionError('server start timeout')
        def stop():
            if process and process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
        def run(*args, **kw):
            return cli('--upstream', upstream, '--allow-http', *args, token=kw.pop('token', admin), **kw)
        def remote(path, content=None, status=200, basic=None):
            actual, body = request(f'/s/{project}/' + path, basic=basic)
            assert actual == status, (path, actual, status)
            if content is not None:
                assert body == content.encode(), (path, body)
        try:
            start()
            assert request('/api/projects', 'POST', {})[0] in (401, 403)
            assert request('/api/projects', 'POST', {}, 'invalid')[0] in (401, 403)
            created = run('--json', 'new', parsed=True)
            config = json.loads((work / '.sss.json').read_text())
            project = config.get('project') or config.get('project_id') or created.get('id')
            assert project, (created, config)
            assert admin not in json.dumps(config)
            (work / 'index.html').write_text('version one')
            (work / 'assets').mkdir()
            (work / 'assets/style.css').write_text('body{}')
            run('upload', 'index.html', 'assets/style.css')
            remote('index.html', 'version one')
            remote('assets/style.css', 'body{}')
            (work / 'index.html').write_text('version two')
            run('upload', 'index.html')
            remote('index.html', 'version two')
            remote('assets/style.css', 'body{}')
            for name, content in [('.env', 'DUMMY_SECRET=never-upload'), ('.sssignore', 'ignored.txt\n'), ('ignored.txt', 'ignored')]:
                (work / name).write_text(content)
            (work / '.git').mkdir()
            (work / '.git/config').write_text('dummy')
            (work / 'assets/style.css').unlink()
            (work / 'index.html').write_text('version three')
            run('sync', '.', '--dry-run')
            remote('index.html', 'version two')
            remote('assets/style.css', 'body{}')
            run('sync', '.')
            remote('index.html', 'version three')
            stable = api(f'/api/projects/{project}/manifest')
            run('sync', '.')
            assert stable == api(f'/api/projects/{project}/manifest')
            (root / 'outside.txt').write_text('outside')
            (work / 'escape.txt').symlink_to(root / 'outside.txt')
            run('upload', 'escape.txt', ok=False)
            run('sync', '.', ok=False)
            (work / 'escape.txt').unlink()
            (work / 'escape-dir').symlink_to(root, target_is_directory=True)
            run('upload', 'escape-dir/outside.txt', ok=False)
            (work / 'escape-dir').unlink()
            assert stable == api(f'/api/projects/{project}/manifest')
            for name in ['assets/style.css', '.env', '.sss.json', '.sssignore', '.git/config', 'ignored.txt']:
                remote(name, status=404)
            (work / 'assets/style.css').write_text('protected{}')
            run('upload', 'assets/style.css')
            run('config', '--basic-auth', '--password-stdin', stdin='test-password\n')
            remote('index.html', status=401)
            remote('assets/style.css', status=401)
            remote('assets/style.css', 'protected{}', basic='sss:test-password')
            remote('index.html', 'version three', basic='sss:test-password')
            remote('index.html', status=401, basic='sss:wrong')
            remote('not-found.css', status=401)
            run('config', '--no-basic-auth')
            manifest = api(f'/api/projects/{project}/manifest')
            for path in ['../escape', '/absolute', 'a/../../escape', 'a\\..\\escape']:
                status, _ = request(f'/api/projects/{project}/files', 'POST', {'mode':'upload', 'files':{'index.html':base64.b64encode(b'partial-write').decode(), path:base64.b64encode(b'bad').decode()}, 'base_revision':manifest['revision']}, admin)
                assert status == 400, (path, status)
            assert manifest['files'] == api(f'/api/projects/{project}/manifest')['files']
            status, _ = request(f'/api/projects/{project}/files', 'POST', {'mode':'upload', 'files':{'index.html':base64.b64encode(b'bad').decode()}, 'base_revision':'stale-revision'}, admin)
            assert status == 409, status
            remote('index.html', 'version three')
            upload_key = run('--json', 'keys', 'create', '--project', project, '--scope', 'upload', parsed=True)
            current = api(f'/api/projects/{project}/manifest')
            status, _ = request(f'/api/projects/{project}/files', 'POST', {'mode':'upload', 'files':{}, 'delete':['index.html'], 'base_revision':current['revision']}, upload_key['key'])
            assert status == 400, status
            remote('index.html', 'version three')
            assert current == api(f'/api/projects/{project}/manifest')
            key = run('--json', 'keys', 'create', '--project', project, '--scope', 'upload,sync', parsed=True)
            token, key_id = key.get('token') or key.get('key'), key.get('id') or key.get('key_id')
            assert token and key_id, key
            (work / 'index.html').write_text('scoped upload')
            run('upload', 'index.html', token=token)
            remote('index.html', 'scoped upload')
            assert request('/api/projects', 'POST', {}, token)[0] in (401, 403)
            other = api('/api/projects', 'POST', {})
            other_id = other.get('id') or other.get('project')
            assert request(f'/api/projects/{other_id}/manifest', token=token)[0] in (401, 403)
            assert request(f'/api/projects/{project}', 'DELETE', token=token)[0] in (401, 403)
            assert token not in json.dumps(run('--json', 'keys', 'list', parsed=True))
            run('keys', 'revoke', key_id)
            run('upload', 'index.html', token=token, ok=False)
            expired = run('--json', 'keys', 'create', '--project', project, '--scope', 'upload', '--expires-in', '1', parsed=True)
            time.sleep(1.2)
            run('upload', 'index.html', token=expired.get('token') or expired.get('key'), ok=False)
            stop()
            start()
            remote('index.html', 'scoped upload')
            run('upload', 'index.html', token=token, ok=False)
            run('delete', 'index.html')
            remote('index.html', status=404)
            run('delete', '--project', project)
            assert request(f'/api/projects/{project}/manifest', token=admin)[0] == 404
            api(f'/api/projects/{other_id}', 'DELETE')
            print('PASS: lifecycle, nested upload, overwrite, mirror/dry-run/exclusions, auth, key isolation/revoke/expiry, traversal, symlinks, no-op revisions, batch rejection, stale revision, persistence, deletion')
        finally:
            stop()
            log.close()

if __name__ == '__main__':
    main()
