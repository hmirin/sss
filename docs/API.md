# HTTP API

Requests and responses use JSON. Basic authentication uses the `Authorization` header; rejected requests return 401 with a Basic challenge. No Bearer tokens or API keys are used.

Shared server credentials authorize all operations. Without shared credentials, server-wide operations are open. Project operations use the project's editing policy; static content uses its viewing policy. Policies are `inherit`, `none`, or `basic` with `username` and `password`. Passwords and hashes are never returned.

| Method | Path | Body / result | Authentication |
|---|---|---|---|
| GET | `/health` | `{"status":"ok"}` | None |
| POST | `/api/projects` | Optional `name`, `auth`; returns `id`, `url` | Shared |
| GET | `/api/projects` | `projects` array with `id`, `name`, `version` | Shared |
| GET | `/api/projects/{id}` | Current metadata and file manifest | Edit |
| GET | `/api/projects/{id}/manifest` | Same metadata and manifest | Edit |
| PATCH | `/api/projects/{id}/auth` | Optional `view`, `write` policies | Shared |
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
