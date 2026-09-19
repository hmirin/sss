# sss — Simple Serve System

Publish static files to a stable URL. Update them from any machine. Keep a URL for every version.

One binary contains both the server and CLI. Start without configuration or authentication, then add Basic authentication where you need it.

- Multiple projects on one server, each with its own URL.
- File uploads and directory sync, with atomic publication of each update.
- Numbered versions, fixed version URLs, and rollback.
- Optional Basic authentication for administration, viewing, and editing.
- Local file storage and SQLite metadata. No database service or Docker required.

## Quick Start

Start a server:

```sh
sss serve
```

In another terminal, create a project:

```sh
sss new --name hello
```

```json
{
  "id": "a1b2c3",
  "url": "http://localhost:8080/s/a1b2c3/"
}
```

Publish a directory:

```sh
sss sync ./public --project a1b2c3
```

Open the returned project URL. Run the same sync command whenever you want to publish an update.

By default, the CLI connects to `http://localhost:8080`. To use another server, set an environment variable:

```sh
export SSS_UPSTREAM=https://somewhere
sss new --name hello
sss sync ./public --project a1b2c3
```

Or pass the server to individual commands:

```sh
sss new --upstream https://somewhere --name hello
sss sync ./public --upstream https://somewhere --project a1b2c3
```

`--upstream` takes precedence over `SSS_UPSTREAM`. No login or client setup is required.

## Projects and Publishing

Create projects freely and select the target explicitly with `--project`:

```sh
sss new --name another-site
sss sync ./site-a --project a1b2c3
sss sync ./site-b --project d4e5f6
```

`sync` makes the project match the directory, including removing files that are no longer present. Preview the changes with `--dry-run`. Use `upload` to add or replace files without removing other files:

```sh
sss sync ./public --project a1b2c3 --dry-run
sss upload index.html assets/style.css --project a1b2c3
```

Updates compare file hashes, transfer changed files, and switch the published version only after the complete file set is ready. Concurrent changes are checked so a stale update cannot silently overwrite a newer one.

Use relative asset paths such as `assets/style.css`, not `/assets/style.css`, so pages work under both project and version URLs. Upload only the files you intend to serve. Hidden files, credentials, and common build dependency directories are excluded by default; `.sssignore` adds custom exclusions.

sss serves static HTML, CSS, JavaScript, and other files. It does not run application backends.

## Versions

Every published update creates a new version, numbered `1`, `2`, `3`, and so on:

```text
/s/a1b2c3/              Current version
/s/a1b2c3/versions/1/   Version 1
/s/a1b2c3/versions/2/   Version 2
```

Share the project URL to show the current version, or a version URL to show a specific snapshot. Assets are part of the snapshot, so relative links within a version stay on that version.

Rollback changes which version the project URL serves. It does not rewrite an existing version or reuse a version number. Old versions can be deleted, but the current version cannot be deleted until another version is selected. A deleted version URL no longer serves its content.

All versions use the project's current authentication settings. Changing a password also changes access to older versions.

## Authentication (Optional)

By default, no authentication is required. Anyone who can reach the server can create, edit, and delete projects.

Set Basic authentication on the server when needed. Each variable is optional:

```sh
export SSS_BASIC_AUTH_ADMIN='admin:example-admin-password'
export SSS_BASIC_AUTH_VIEW='viewer:example-view-password'
export SSS_BASIC_AUTH_WRITE='editor:example-write-password'

sss serve
```

| Variable | Purpose |
|---|---|
| `SSS_BASIC_AUTH_ADMIN` | Protect project creation, global listing, authentication changes, and administrative operations. Admin credentials can manage any project. |
| `SSS_BASIC_AUTH_VIEW` | Default credentials for viewing projects and their assets. |
| `SSS_BASIC_AUTH_WRITE` | Default credentials for editing projects, managing versions, and deleting projects. |

An unset variable leaves its corresponding operations unauthenticated. Set all the boundaries you want to protect; setting admin authentication alone does not protect viewing or editing.

### Per-project settings

Projects inherit the server's viewing and editing settings by default. Override either setting with JSON:

```json
{
  "name": "hello",
  "auth": {
    "view": { "mode": "none" },
    "write": {
      "mode": "basic",
      "username": "editor",
      "password": "example-project-password"
    }
  }
}
```

```sh
sss new --config project.json
```

| Mode | Behavior |
|---|---|
| `inherit` | Use the corresponding server environment variable. This is the default when a setting is omitted. |
| `none` | Explicitly disable authentication for this project operation, even if a server default is set. |
| `basic` | Use the project's specified username and password. |

Both viewing and editing support all three modes. Authentication settings are changed through the admin boundary; project editing credentials do not grant permission to change authentication settings.

Keep configuration files containing passwords out of repositories and published directories. Use `--config -` to read JSON from standard input instead of a file. Server-side passwords are stored as hashes.

### Authenticating CLI requests

Pass credentials only when the server or project requires them:

```sh
sss new --name hello --basic_auth admin:example-admin-password
sss sync ./public --project a1b2c3 \
  --basic_auth editor:example-project-password
```

For repeated use or agents, supply credentials through the environment:

```sh
export SSS_BASIC_AUTH='editor:example-project-password'
sss sync ./public --project a1b2c3
```

The CLI uses `--basic_auth`, then `SSS_BASIC_AUTH`, or sends no credentials when neither is set. It does not require a login step or automatically save credentials. An authentication failure explains how to supply them.

`--basic_auth` authenticates a request; the project's JSON config defines its authentication rules. Credentials never belong in a URL or a project binding file. Prefer environment injection over command-line arguments when shell history or process listings could expose passwords.

## Hosting

The server listens on `127.0.0.1:8080` by default. Choose an explicit listen address, persistent data directory, and external URL for deployment:

```sh
sss serve \
  --listen 127.0.0.1:8080 \
  --data-dir /var/lib/sss \
  --public-url https://sss.example
```

sss serves HTTP. Use Tailscale Serve, Caddy, or another reverse proxy to provide HTTPS. Certificate provisioning and renewal belong to the hosting setup. Use HTTPS when transmitting credentials over a network.

One hostname serves every project under `/s/{id}/`; creating a project does not require another DNS record or certificate. For example, an Incus container exposed through Tailscale Serve can serve projects at:

```text
https://sss-oracle.example.ts.net/s/a1b2c3/
https://sss-oracle.example.ts.net/s/d4e5f6/
```

Network access rules and Basic authentication are independent. A Tailnet-only deployment remains reachable only by devices allowed by its network policy, even when sss authentication is disabled.

Run the binary directly under a service manager such as systemd. Docker is optional. Keep the data directory persistent and back it up, including both SQLite metadata and file storage.

Projects share a browser origin. Use sss for trusted publishers, not for isolating mutually untrusted tenants.

## Build and Check

```sh
cargo build --release --locked
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
python3 tests/e2e.py target/release/sss
```

The resulting `target/release/sss` binary contains both the server and CLI. The integration test starts a disposable loopback server and requires Python 3.
