# sss — Simple Static Server

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

Create a file and publish it, using the project ID returned above:

```sh
mkdir -p public
echo hello > public/index.html
sss sync ./public --project a1b2c3
```

Fetch the published page:

```sh
curl http://localhost:8080/s/a1b2c3/
```

```text
hello
```

You can also open the URL in a browser. Run the same sync command whenever you want to publish an update.

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

## Listing and Deleting Projects

List projects on the selected server:

```sh
sss list
```

Use a project's ID from the JSON response with `--project`.

Delete a project, including all its files, versions, and access settings:

```sh
sss delete --project a1b2c3
```

To delete only selected files, specify their paths:

```sh
sss delete old.html assets/old.css --project a1b2c3
```

File deletion publishes a new version; previous versions remain available. Whole-project deletion cannot be undone through sss.

When authentication is enabled, listing uses the server's shared credentials and deletion requires project editing access. Supply credentials with `--basic_auth` or `SSS_BASIC_AUTH`.

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

## Inspect and Diagnose

```sh
sss info --project a1b2c3
sss doctor
sss doctor --project a1b2c3
sss diff ./public --project a1b2c3
```

`info` returns the project URL, current version, current file count and bytes, total stored bytes across versions, viewing/editing policies, and `expires_at`. It requires editing access and never returns passwords or hashes. `expires_at` is a Unix timestamp in seconds, or `null` for no expiration.

`doctor` reports the selected upstream, client/server versions, reachability, and authentication checks as JSON. Without a project it checks shared administration access; with `--project` it checks editing access to that project. It makes no changes and exits nonzero when a check fails.

`diff` compares local files with the current published snapshot without publishing. Its JSON includes added, modified, and deleted paths and unified text patches. Binary/non-UTF-8 files and file pairs larger than 1 MiB report sizes with `content_omitted: true`. It uses editing credentials even when viewing has a separate password. If the published revision changes during comparison, retry the command.

## Watch a Directory

```sh
sss sync ./public --project a1b2c3 --watch
```

Publish once, then watch for local changes until Ctrl-C. Changes are polled every 250 ms and published after 750 ms of quiet, grouping bursts into one version. Existing exclusions and `.sssignore` apply. Identical content does not create a version. Each publication prints one JSON object on its own line; diagnostics go to stderr. `--watch` cannot be combined with `--dry-run`.

An unreadable or missing source directory is not published; watching resumes when it can be read. A server error or revision conflict stops the watcher so you can inspect the problem before retrying. Watch keeps the local directory authoritative when publishing a new batch; coordinate with other editors of the same project.

## Project Expiration

Choose a default lifetime for newly created projects on the server:

```sh
sss serve --default-expires-in 7d
# Or set SSS_DEFAULT_EXPIRES_IN=7d in the server environment.
```

Override it when creating a project, or change it with the same `config` command used for authentication:

```sh
sss new --name preview --expires-in 1d
sss config --project a1b2c3 --expires-in 30d
sss config --project a1b2c3 --expires-in none
sss config --project a1b2c3 --name hello --basic_auth_view none --expires-in 7d
```

Durations are positive whole numbers with `s`, `m`, `h`, `d`, or `w`; `none` disables expiration. JSON configuration accepts `"expires_in": "7d"` or `"expires_in": "none"`; CLI flags override JSON values.

The lifetime starts at creation or when explicitly changed. Uploads, syncs, and rollbacks do not extend it. Omitting the setting on `new` uses the server default (which is `none` unless configured); omitting it on `config` preserves the existing deadline. Changing the server default never changes existing projects. Updating project settings requires shared server authentication when configured.

**Expiration deletes the entire project and all its versions.** At the deadline, its API and static URLs return 404 and it disappears from `list`. Cleanup runs on startup and every 30 seconds; failed filesystem cleanup is retried. Expired projects cannot be revived by extending their deadline. Use `info` or `list` to inspect deadlines before they expire.

## Authentication (Optional)

By default, no authentication is required. Anyone who can reach the server can create, edit, and delete projects.

To protect the server, set one username and password:

```sh
export SSS_BASIC_AUTH='user:example-password'
sss serve
```

This shared credential protects project creation, global listing, project settings changes, and, by default, all project viewing and editing. It can administer every project. If `SSS_BASIC_AUTH` is unset, server-wide operations require no authentication and projects inherit that default.

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

Creating projects, listing all projects, and changing project settings use the server's shared authentication. If it is unset, those operations remain unauthenticated even when a project has its own password. Set `SSS_BASIC_AUTH` on the server if you need to protect those operations.

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
