# HTTP API

Requests and responses use JSON. Basic authentication uses the `Authorization` header; rejected requests return 401 with a Basic challenge. No Bearer tokens or API keys are used.

Shared server credentials authorize all operations. Without shared credentials, server-wide operations are open. Project operations use the project's editing policy; static content uses its viewing policy. Policies are `inherit`, `none`, or `basic` with `username` and `password`. Passwords and hashes are never returned.

| Method | Path | Body / result | Authentication |
|---|---|---|---|
| GET | `/health` | `status`, `version` | None |
| POST | `/api/projects` | Optional `name`, `auth`, `expires_in`; returns `id`, `url`, `expires_at` | Shared |
| GET | `/api/projects` | `projects` array with `id`, `name`, `version`, `expires_at` | Shared |
| GET | `/api/projects/{id}` | Metadata, current file count/bytes, total storage_bytes, auth modes, expires_at | Edit |
| GET | `/api/projects/{id}/manifest` | Current metadata and file hashes | Edit |
| PATCH | `/api/projects/{id}` | Optional `name`, `auth.view`, `auth.write`, `expires_in` | Shared |
| GET | `/api/status` | Server version, authentication_required, default_expires_in_seconds | Shared |
| GET | `/api/projects/{id}/versions/{n}/files/{path}` | Raw snapshot file bytes for diff | Edit |
| POST | `/api/projects/{id}/versions` | Publish a snapshot; see below | Edit |
| GET | `/api/projects/{id}/versions` | `project`, `current`, `versions` (integers) | Edit |
| PUT | `/api/projects/{id}/current` | `version`, `base_revision` | Edit |
| DELETE | `/api/projects/{id}/versions/{n}` | Delete a non-current version | Edit |
| DELETE | `/api/projects/{id}` | Delete project and all snapshots | Edit |

A new project has no published version (`current` / `version` is 0). Its site returns 404 until first publication.

## Publish a Snapshot

Read the manifest to obtain its `revision` concurrency token and `files` mapping from relative paths to SHA-256 hashes. Post:

```json
{
  "mode": "upload",
  "base_revision": "revision-from-manifest",
  "files": {"index.html": "aGVsbG8="}
}
```

File values are base64-encoded bytes. `upload` preserves existing files. `sync` also requires `keep`, an array naming the entire desired file set, and removes other files. `delete` accepts a `delete` array and no uploaded files or keep list. Snapshot publication returns `project`, `revision`, `version`, `version_url`, `url`, file count, and byte count.

A stale revision returns 409. Rollback also changes this token. Validate the whole update before publishing; rejected uploads never alter the active snapshot. The CLI skips no-op uploads; a direct successful publication request creates a version.

## Static Content

`/s/{id}/` serves the current snapshot; `/s/{id}/versions/{n}/` serves a retained snapshot. Trailing-slash paths resolve to `index.html`. Version numbers are canonical integers without leading zeros. The top-level `versions/` namespace cannot be uploaded.

All snapshots use current project viewing authentication, including missing asset requests. Responses include guessed MIME types, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, a no-referrer policy, and a CSP disabling workers and cross-origin framing. Projects share an origin.

## Expiration and Settings

`expires_in` is a positive integer followed by `s`, `m`, `h`, `d`, or `w`, or the string `none`. Omission on POST inherits the configured server default; omission on PATCH preserves the current deadline. Explicit values start a new lifetime from request processing time, not the last publication. Responses expose `expires_at` as Unix seconds or null. Server defaults apply only on creation. Existing projects without a deadline remain unlimited.

PATCH validates and applies name, authentication and expiration together. For example: `{"auth":{"view":{"mode":"none"}},"expires_in":"7d"}`. Config changes require shared administration access, not project editing credentials. Authentication summaries expose configured modes, effective authentication_required, and a custom username only; never passwords or hashes.

Expired projects return 404 from all project/file endpoints, including fixed version URLs, and are excluded from listing. Files and metadata are removed on startup or the next 30-second cleanup pass; failed deletions are retried. Expiration cannot be reversed after the deadline.
