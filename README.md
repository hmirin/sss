# sss — Simple Static Site

**Publish what your agent builds.**

A dead-simple static site server and publishing system for agent-based workflows.
View artifacts created by Codex, Claude, or other agents from any computer or mobile device.

- One binary: both the server and CLI.
- Versions: every update creates a new numbered version. Roll back to any previous version.
- Authentication: optional Basic authentication for administration, viewing, and editing.

## Quick Start

> [!TIP]
> TL;DR: Just ask your agent to:
> ```text
> Install sss by following these instructions: https://combinatrix.ai/sss/installation.md
> ```

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

> [!WARNING]
> Use relative asset paths such as `assets/style.css`, not `/assets/style.css`, so pages work under both project and version URLs.
> sss serves static HTML, CSS, JavaScript, and other files. It does not run application backends.

## Upstream Server

By default, the CLI connects to `http://localhost:8080`.
To use another server, set `SSS_UPSTREAM`:

```sh
export SSS_UPSTREAM=https://somewhere
sss new --name hello
sss sync ./public --project a1b2c3
```

Or pass `--upstream` to individual commands:

```sh
sss new --upstream https://somewhere --name hello
sss sync ./public --upstream https://somewhere --project a1b2c3
```

`--upstream` takes precedence over `SSS_UPSTREAM`. Neither is required for local use.

## Uploading Files

`sync` makes the project match the directory, including removing files that are no longer present. Preview the changes with `--dry-run`. Use `upload` to add or replace files without removing other files:

```sh
sss sync ./public --project a1b2c3 --dry-run
```

```json
{
  "project": "a1b2c3",
  "dry_run": true,
  "added": ["assets/style.css"],
  "modified": ["index.html"],
  "deleted": ["old.html"]
}
```

Publish the changes:

```sh
sss sync ./public --project a1b2c3
```

```json
{
  "project": "a1b2c3",
  "version": 2,
  "url": "http://localhost:8080/s/a1b2c3/",
  "version_url": "http://localhost:8080/s/a1b2c3/versions/2/"
}
```

You can also upload individual files without affecting the rest of the project:

```sh
sss upload index.html assets/style.css --project a1b2c3
```

```json
{
  "project": "a1b2c3",
  "version": 3,
  "url": "http://localhost:8080/s/a1b2c3/",
  "version_url": "http://localhost:8080/s/a1b2c3/versions/3/"
}
```

Updates compare file hashes, transfer changed files, and switch the published version only after the complete file set is ready. Concurrent changes are checked so a stale update cannot silently overwrite a newer one.

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

To protect the server, set one username and password:

```sh
export SSS_BASIC_AUTH='user:example-password'
sss serve
```

This shared credential protects project creation, global listing, authentication changes, and, by default, all project viewing and editing. It can administer every project. If `SSS_BASIC_AUTH` is unset, server-wide operations require no authentication and projects inherit that default.

Projects inherit this authentication by default. You can override viewing and editing access for individual projects.

> [!CAUTION]
> Use HTTPS when sending Basic authentication credentials over a network. Basic authentication does not encrypt credentials. See [Hosting](#hosting).

### Per-Project Settings

Projects inherit the server's shared Basic authentication for both viewing and editing. Set project access rules directly when creating a project:

```sh
sss new --name hello --basic_auth_view none --basic_auth_write none
```

Each option accepts `inherit`, `none`, or `username:password`:

| Value | Behavior |
|---|---|
| `inherit` | Use the server's shared authentication. This is the default when the option is omitted. |
| `none` | Allow the operation without authentication. |
| `username:password` | Set project-specific Basic authentication for the operation. |

For example, allow anyone to view the project while requiring a project-specific password for editing:

```sh
sss new --name hello \
  --basic_auth_view none \
  --basic_auth_write editor:example-project-password
```

If project creation itself requires authentication, supply that separately:

```sh
sss new --name hello \
  --basic_auth user:example-password \
  --basic_auth_view none \
  --basic_auth_write none
```

`--basic_auth` authenticates the creation request. `--basic_auth_view` and `--basic_auth_write` configure access to the new project; they do not authenticate the request.

You can also provide the same project settings as JSON:

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

Explicit `--basic_auth_view` and `--basic_auth_write` options override the corresponding JSON settings. If neither a CLI option nor a JSON setting is provided, that operation uses `inherit`.

| Mode | Behavior |
|---|---|
| `inherit` | Use the server's `SSS_BASIC_AUTH`, or no authentication if it is unset. This is the default when a setting is omitted. |
| `none` | Explicitly disable authentication for this project operation, even if a server default is set. |
| `basic` | Use the project's specified username and password. |

Both viewing and editing support all three modes. Project-specific credentials authorize only the configured operation on that project; they do not grant server-wide administration. The server's shared credential retains access to every project, including projects with overrides.

Creating projects, listing all projects, and changing authentication settings use the server's shared authentication. If it is unset, those operations remain unauthenticated even when a project has its own password. Set `SSS_BASIC_AUTH` on the server if you need to protect those operations.

Keep configuration files containing passwords out of repositories and published directories. Use `--config -` to read JSON from standard input instead of a file. Server-side passwords are stored as hashes.

### Authenticating CLI Requests

Every CLI command uses the same `--basic_auth` flag. The server decides whether the supplied credentials authorize the requested operation:

```sh
sss new --name hello --basic_auth user:example-password
sss sync ./public --project a1b2c3 \
  --basic_auth editor:example-project-password
```

For repeated use, supply the shared credentials through the environment:

```sh
export SSS_BASIC_AUTH='user:example-password'
sss new --name another-site
sss sync ./public --project a1b2c3
```

On the server, `SSS_BASIC_AUTH` defines the shared credentials. In the CLI, it supplies credentials for outgoing requests.

`--basic_auth` takes precedence over `SSS_BASIC_AUTH`. If neither is set, the CLI sends no credentials. No login step is required. This flag authenticates the request; it does not change the project's authentication settings.

## Hosting

The server listens on `127.0.0.1:8080` by default. Choose an explicit listen address, persistent data directory, and external URL for deployment:

```sh
sss serve \
  --listen 0.0.0.0 \
  --port 12345 \
  --data-dir /var/lib/sss \
  --public-url https://sss.example
```

Or configure the server through environment variables:

```sh
export SSS_LISTEN=0.0.0.0
export SSS_PORT=12345
export SSS_DATA_DIR=/var/lib/sss
export SSS_PUBLIC_URL=https://sss.example

sss serve
```

You can run sss in a container. A Dockerfile is included in the repository; mount a persistent volume for the data directory.

Projects share a browser origin. Use sss for trusted publishers, not for isolating mutually untrusted tenants.

## Misc

### Update

```sh
sss update
```

Install the latest release and refresh registered, unmodified sss skills. Use `sss update --check` to check without installing. Restart a running server after updating its binary.

### Agent Skill

Print the bundled agent skill as Markdown in SKILL.md format:

```sh
sss --skill
```

Give the output to your agent, or save it as `SKILL.md` in your agent's skill directory. It covers project creation, publishing, versions, and optional authentication. The skill is bundled with the binary so its instructions match the installed version; no running server or network access is required.

### Storage

By default, `sss serve` creates `~/.sss` and stores its configuration, project metadata, and version files there. Use `--data-dir` or `SSS_DATA_DIR` to choose another location. Back up the entire data directory to preserve projects and their versions.

## Build and Check

```sh
cargo build --release --locked
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --locked
python3 tests/e2e.py target/release/sss
```

The resulting `target/release/sss` binary contains both the server and CLI. The integration test starts a disposable loopback server and requires Python 3.
