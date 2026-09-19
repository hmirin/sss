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

`new` returns `id` and `url`. Both `--basic_auth_view` and `--basic_auth_write` accept `inherit`, `none`, or `username:password`; omission means `inherit`. These set project policy. `--basic_auth` authenticates the request.

JSON config accepts `name` and `auth.view` / `auth.write`. Each auth object has `mode: inherit`, `mode: none`, or `mode: basic` with `username` and `password`. Explicit flags override the corresponding JSON fields.

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

## Change Authentication

```sh
sss config --project ID --basic_auth_view none
sss config --project ID --basic_auth_write inherit
sss config --project ID --config project-auth.json
```

The JSON object contains `auth` just as on creation. Only supplied fields change. This command requires shared server credentials when configured; project editing credentials do not authorize it. Renaming is not part of `config`.

## Server

```sh
sss serve
sss serve --listen 127.0.0.1 --port 8080 --data-dir /var/lib/sss --public-url https://sss.example
```

| Option | Environment | Default |
|---|---|---|
| `--listen` | `SSS_LISTEN` | `127.0.0.1` |
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
