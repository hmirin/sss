# sss — Simple Serve System

Publish static files to a stable URL and update them from any machine. One Rust binary runs both the client and server. Run the server directly in a Linux container with systemd; Docker is optional.

```sh
export SSS_API_KEY=... # Obtain an administrator or scoped key securely.
sss new --upstream https://preview.example
# https://preview.example/s/6a91bd02c7ef/
sss upload index.html assets/style.css
sss sync . --dry-run
sss sync .
```

- Stable project URLs and local `.sss.json` project bindings.
- Additive uploads, directory mirroring, and individual or whole-project deletion.
- SHA-256 comparison and revision checks; complete file sets publish together.
- Optional Basic authentication for every project asset.
- Project-scoped API keys with operation scopes, expiry, and revocation.
- SQLite metadata and local file storage. No database service or Docker required.

Use relative asset URLs: `assets/style.css`, not `/assets/style.css`. This serves static HTML, CSS, JavaScript, and other files; it does not run application backends. Encryption and continuous file watching are not part of the initial CLI.

## Build and check

```sh
cargo build --release --locked
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
python3 tests/e2e.py target/release/sss
```

The black-box test starts a disposable loopback server and checks publishing, authentication, key boundaries, restart persistence, and deletion. It needs only Python 3 and the compiled binary, and works on macOS and Linux.

## Run a server

```sh
# Capture this once into a private credential store, never a repository.
sss admin-key --data-dir /var/lib/sss
sss serve --listen 127.0.0.1:8080 \
  --data-dir /var/lib/sss --public-url https://preview.example
```

Put an HTTPS reverse proxy or Tailscale Serve in front of the loopback listener. API keys authorize management; optional Basic authentication controls browsing. Treat publishers as trusted: projects share a browser origin and are not isolated web tenants.

See [CLI reference](docs/CLI.md), [HTTP API](docs/API.md), and [deployment](docs/DEPLOYMENT.md).
