# Deployment

The normal deployment is a Linux container with the `sss` binary, systemd, and a persistent local data directory. Docker is optional. Build natively for the target OS/architecture; a macOS binary cannot run on Linux. SQLite is bundled at build time.

## Linux / LXC with systemd

Install a release binary as `/usr/local/bin/sss`, then create the service user and private directories:

```sh
sudo useradd --system --home-dir /var/lib/sss --shell /usr/sbin/nologin sss
sudo install -d -o sss -g sss -m 0700 /var/lib/sss
sudo install -d -m 0755 /etc/sss
```

Create `/etc/sss/sss.env` (no credentials needed):

```ini
SSS_LISTEN=127.0.0.1:8080
SSS_PUBLIC_URL=https://preview.example
```

Install `packaging/sss.service` as `/etc/systemd/system/sss.service`. Bootstrap an administrator token and capture stdout directly into a private credential store; do not put the token in logs, this environment file, or the repository:

```sh
sudo -u sss /usr/local/bin/sss admin-key --data-dir /var/lib/sss
sudo systemctl daemon-reload
sudo systemctl enable --now sss
curl --fail http://127.0.0.1:8080/health
```

Place an HTTPS reverse proxy or Tailscale Serve on the same host in front of `127.0.0.1:8080`. Set `SSS_PUBLIC_URL` to the actual external origin. Preserve the `Authorization` header and allow requests up to 72 MiB. Do not expose the loopback backend directly on a public interface. Tailscale ACLs are independent of application API keys and Basic authentication.

## Optional Docker

```sh
docker build -t sss:local .
docker volume create sss-data
docker run --rm -v sss-data:/var/lib/sss sss:local \
  admin-key --data-dir /var/lib/sss
docker run -d --name sss --restart unless-stopped \
  -p 127.0.0.1:8080:8080 -v sss-data:/var/lib/sss sss:local \
  serve --listen 0.0.0.0:8080 --data-dir /var/lib/sss \
  --public-url https://preview.example
```

The image runs as UID/GID 10001. Bind-mounted data directories must be writable by that UID; the named volume example receives image directory ownership. The Dockerfile is optional packaging, not a runtime requirement.

## State and operation

Back up the entire data directory while the service is stopped, including SQLite state and project files. Keep its permissions private. Restore it to the same configured data directory. Current revisions survive restarts; completed updates remove old file revisions. There is no revision history or rollback command.

Project limits are 50 MiB and 5,000 files; total server storage depends on project count and temporary publication copies. Monitor disk usage. Updates and serving are serialized within this initial single-process server; this is intended for small trusted preview workloads, not a multi-tenant CDN.

For upgrades, replace the binary and restart the service, then check `/health` and a known project URL. Do not run multiple writers against the same data directory. The black-box test uses its own disposable data directory and has no production credentials:

```sh
python3 tests/e2e.py /usr/local/bin/sss
```
