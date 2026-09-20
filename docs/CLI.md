# CLI Reference

All client commands return JSON. No client configuration file or login step is required.

## Connection and Authentication

| Option | Environment | Default |
|---|---|---|
| `--upstream URL` | `SSS_UPSTREAM` | `http://localhost:8080` |
| `--basic_auth USER:PASS` | `SSS_BASIC_AUTH` | No credentials |
| `--project ID` | — | Required for project operations |

Flags override environment variables. Project selection is explicit and never follows the last-created project. Credentials are not saved. HTTP and HTTPS origins are supported; supply HTTPS when credentials cross a network.

## Create and List

```sh
sss new --name hello
sss list
sss new --name public --basic_auth_view none --basic_auth_write none
sss new --config project.json
sss new --config -
```

`new` returns `id`, `url`, and `expires_at`. Both `--basic_auth_view` and `--basic_auth_write` accept `inherit`, `none`, or `username:password`; omission means `inherit`. These set project policy. `--basic_auth` authenticates the request.

JSON config accepts `name`, `expires_in`, and `auth.view` / `auth.write`. Each auth object has `mode: inherit`, `mode: none`, or `mode: basic` with `username` and `password`. Explicit flags override the corresponding JSON fields.

Creation and listing use the server's shared authentication. An unset server credential leaves these operations open.

## Publish

```sh
sss upload index.html assets/style.css --project ID
sss sync ./public --project ID --dry-run
sss sync ./public --project ID
sss delete old.html --project ID
```

`upload` adds/replaces relative paths from the working directory; `sync` mirrors a directory including deletions. File deletion publishes a new snapshot. SHA-256 comparison avoids resending unchanged content. A no-op upload/sync returns `unchanged: true` and keeps the current version; an initial empty sync creates version 1.

Dry-run returns `added`, `modified`, and `deleted` paths without creating a version. Publication returns `project`, `version`, `revision`, `url`, and `version_url`.

`.sssignore` follows gitignore syntax. Hidden paths, `node_modules`, `target`, `.pem`, and `.key` files are excluded. Symlinks are rejected. The top-level `versions/` path is reserved. Use relative asset URLs.

Limits per snapshot: 50 MiB decoded content, 5,000 files, 1,024 bytes per relative UTF-8 path. HTTP request limit: 72 MiB. Retained versions consume disk until explicitly deleted.

## Versions and Deletion

```sh
sss versions --project ID
sss rollback 1 --project ID
sss delete-version 2 --project ID
sss delete --project ID
```

Versions are increasing integers starting at 1 and are never reused. Rollback changes the current version and concurrency token. The current version cannot be deleted. Whole-project deletion removes all versions and access settings.

Every snapshot uses current project authentication. Publication is atomic on the server; separate browser requests spanning an update can still observe different versions. Use a fixed version URL when you need a stable snapshot.

## Change Project Settings

```sh
sss config --project ID --basic_auth_view none
sss config --project ID --basic_auth_write inherit
sss config --project ID --config project-auth.json
```

The JSON object contains `auth` just as on creation. Only supplied fields change. This command requires shared server credentials when configured; project editing credentials do not authorize it. Use `--name` to rename a project and `--expires-in` to set its lifetime from now or `none` to disable expiration.

## Server

```sh
sss serve
sss serve --listen 127.0.0.1 --port 8080 --data-dir /var/lib/sss --public-url https://sss.example
```

| Option | Environment | Default |
|---|---|---|
| `--listen` | `SSS_LISTEN` | `127.0.0.1` |
| `--default-expires-in` | `SSS_DEFAULT_EXPIRES_IN` | `none` |
| `--port` | `SSS_PORT` | `8080` |
| `--data-dir` | `SSS_DATA_DIR` | `~/.sss` |
| `--public-url` | `SSS_PUBLIC_URL` | `http://localhost:<port>` |

`SSS_BASIC_AUTH` configures shared server authentication. It covers administration and inherited project access. Explicit project `none` and `basic` override inheritance. Shared credentials retain access to all projects. Only hashes of project passwords are stored; server environment credentials are hashed in memory.

## Embedded Skill and Update

```sh
sss --skill
sss update --check
sss update
```

`--skill` prints the bundled skill and requires no server. `update` fetches the latest stable GitHub release for the current target, verifies its attested checksum manifest and archive, checks the new executable, and atomically replaces the binary. It does not execute a downloaded installer script.

Unmodified registered sss skills under `~/.agents`, `~/.claude`, and `CODEX_HOME` (default `~/.codex`) are refreshed from the new binary. Modified skills are preserved and reported. Custom paths require manual refresh. `--check` makes no local changes. `GH_TOKEN` is supported for private GitHub release access and is distinct from server Basic authentication.

A server already running continues to use its old executable until restarted. Signature, download, extraction, or candidate verification failures leave the installed binary unchanged. Skill replacement failures after a binary update are reported separately.

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
