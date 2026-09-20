---
name: sss
description: Publish and update static sites or agent-generated HTML artifacts with sss. Use for project creation, file uploads, directory sync, version management, and project access settings on an sss server.
---

# sss

Use the `sss` CLI to publish static files and return their URLs. The same binary runs the server with `sss serve`.

## Choose the Target

- Use the user's server, or the configured `SSS_UPSTREAM`. `--upstream URL` overrides it. With neither, the CLI uses `http://localhost:8080`.
- For local hosting, run `sss serve` in a separate terminal or managed process. For an existing remote server, use it directly.
- Reuse the project ID when updating a site. Create a new project for a separate site. Pass `--project` explicitly; do not infer the target from the last command.

## Create and Publish

```sh
sss new --name hello
```

Read `id` and `url` from the JSON response. Use the returned ID in subsequent commands:

```sh
sss sync ./public --project <id>
```

`sync` mirrors the directory, including remote deletions. Use `--dry-run` to inspect additions, modifications, and deletions when the target already contains files whose removal is uncertain:

```sh
sss sync ./public --project <id> --dry-run
```

To add or replace individual files while keeping the rest:

```sh
sss upload index.html assets/style.css --project <id>
```

Publish the built site directory or selected artifacts, not the source repository. Use relative asset paths such as `assets/style.css` so the page also works at version URLs. sss does not run application backends.

After publishing, use the returned `url`, `version`, and `version_url`. Check that the page and its assets load; for interactive artifacts, exercise the intended interaction. Return the current URL and, when useful, the fixed version URL to the user. Do not substitute the local filesystem path or a localhost URL for a remotely accessible URL when the user needs to view it from another device.

## Authentication

No authentication is needed by default. When required, authenticate any CLI request with `--basic_auth user:password` or `SSS_BASIC_AUTH`. The flag overrides the environment variable. There is no login step.

The server's shared credential can create and manage projects. Project-specific credentials authorize only their configured project operations. If a request fails authentication, obtain the appropriate credential through the user's approved mechanism; do not disable authentication to make the request succeed.

These creation options configure the new project's access rules; they do not authenticate the creation request:

```sh
sss new --name hello --basic_auth_view none --basic_auth_write none
```

Both options accept:

| Value | Access rule |
|---|---|
| `inherit` | Use the server's `SSS_BASIC_AUTH`; this is the default. |
| `none` | No authentication for this operation. |
| `username:password` | Project-specific Basic authentication. |

Leave these options omitted unless the requested access rules call for an override. Project settings can also be supplied with `sss new --config project.json` or `--config -` for JSON on stdin. Explicit access flags override the corresponding JSON fields.

```json
{
  "name": "hello",
  "auth": {
    "view": { "mode": "none" },
    "write": { "mode": "inherit" }
  }
}
```

The JSON form for a custom credential is `{"mode":"basic","username":"editor","password":"..."}`. Do not include real passwords in published files, committed configuration, or user-facing output. Use HTTPS when transmitting credentials over a network.

## Versions

Each published update creates a numbered version. The project URL serves the current version; `/s/{id}/versions/{n}/` serves a fixed snapshot. Use URLs returned by the server rather than constructing them from the project name.

Rollback selects an existing version as current without changing its content or reusing its number. All versions use the project's current authentication settings. Use:

```sh
sss versions --project <id>
sss rollback <version> --project <id>
sss delete-version <version> --project <id>
```

Check the target project and version before changing or deleting them; the current version cannot be deleted until another is selected.

## Update

When asked to update sss, run `sss update`. Use `sss update --check` for a read-only check. The updater verifies the signed release and replaces the binary, then refreshes registered, unmodified skills. It reports modified skills that it preserved. Read the new `sss --skill` output after updating. A running server requires a separate restart to use the new binary.

## Server Configuration

`sss serve` defaults to `127.0.0.1:8080` and stores data in `~/.sss`.

| CLI option | Environment variable |
|---|---|
| `--listen` | `SSS_LISTEN` |
| `--port` | `SSS_PORT` |
| `--data-dir` | `SSS_DATA_DIR` |
| `--public-url` | `SSS_PUBLIC_URL` |

Set `SSS_BASIC_AUTH` on the server for shared authentication. If it is unset, creation, global listing, and project settings changes are unauthenticated even if a project has its own password.

Use the deployment's external URL as `--public-url`. Preserve the data directory across restarts. Configure persistent hosting or external network access only when the task calls for it.

## Inspect, Compare, and Diagnose

- `sss info --project ID`: URLs, version, current file count/bytes, total storage_bytes, authentication modes, and expires_at (Unix seconds or null). Requires editing access; no passwords or hashes are returned.
- `sss doctor [--project ID]`: read-only JSON connectivity/version/authentication checks. Without a project, checks shared admin access; with a project, checks edit access. A failed check exits nonzero.
- `sss diff ./public --project ID`: read-only JSON changes with unified text patches. Binary, non-UTF-8, and file pairs over 1 MiB omit content and report sizes. Requires editing access. Retry if the server revision changed.
- `sss sync ./public --project ID --watch`: initial sync then poll every 250 ms and publish after 750 ms of quiet. Prints JSON lines per publication. Ctrl-C stops it. Cannot combine with --dry-run. Local scan errors pause publication; server errors and revision conflicts stop the watcher. Local content is authoritative for each new batch, so coordinate simultaneous editors.

## Expiration and Project Settings

```sh
sss serve --default-expires-in 7d
sss new --name preview --expires-in 1d
sss config --project ID --expires-in 30d
sss config --project ID --expires-in none
sss config --project ID --name renamed --basic_auth_view none --expires-in 7d
```

The server also reads `SSS_DEFAULT_EXPIRES_IN`; the default is `none`. Accept positive whole numbers followed by s/m/h/d/w, or none. JSON settings use `expires_in` alongside name and auth; CLI flags take precedence. New projects inherit the server default when omitted; config preserves the current deadline when omitted. Explicit durations start from creation/configuration time. Sync/upload/rollback do not renew them, and changing the server default does not change existing projects.

Expiration deletes the project and every version. At the deadline all project URLs become unavailable; cleanup runs on startup and every 30 seconds. Expired projects cannot be revived. Reuse the user's agreed retention policy; do not invent a finite lifetime for permanent artifacts. Check expires_at with info/list. Changes to authentication, name, and expiration use shared administration authentication.
