# HTTP API

Management routes require `Authorization: Bearer <key>` and JSON request bodies. Errors normally use `{"error":"message"}`; framework-level malformed requests may use other bodies. `401` means missing, invalid, or expired credentials; `403` means insufficient scope; `404` means missing resource; `409` means a stale publication revision. Use HTTPS outside loopback/trusted tunnels. Admission is limited to two concurrent requests; excess requests receive `503` and should be retried with backoff.

## Routes

| Method and path | Authorization | Operation |
|---|---|---|
| `GET /health` | None | `{"status":"ok"}` |
| `POST /api/projects` | Admin | Create project |
| `GET /api/projects/{id}/manifest` | Admin or project read access | Current revision and SHA-256 manifest |
| `POST /api/projects/{id}/files` | Admin or matching project operation | Atomic upload, mirror, or file deletion |
| `PATCH /api/projects/{id}` | Admin or project `config` | Set/remove browsing password |
| `DELETE /api/projects/{id}` | Admin or project `delete` | Delete project and scoped keys |
| `POST /api/keys` | Admin | Create scoped API key |
| `GET /api/keys` | Admin | List key metadata |
| `DELETE /api/keys/{keyID}` | Admin | Revoke key |
| `GET /s/{id}/` | Project Basic auth if enabled | Serve `index.html` |
| `GET /s/{id}/{path}` | Project Basic auth if enabled | Serve file; trailing slash maps to `index.html` |

## Projects and files

Create with `{}` or `{"basic_password":"..."}`. Response fields: `project`, `url`, `revision`.

Manifest response:

```json
{"revision":"opaque-revision","files":{"index.html":"sha256-hex"}}
```

Send an update using that exact revision:

```json
{
  "mode": "upload",
  "base_revision": "opaque-revision",
  "files": {"index.html": "PGgxPkhlbGxvPC9oMT4="}
}
```

Values in `files` are standard Base64 file bytes. Modes:

- `upload`: add/replace the supplied paths and preserve other files.
- `sync`: include `keep`, the complete desired list of paths; existing files outside that list are removed. Supply changed/new bytes in `files`.
- `delete`: supply `delete`, a list of paths; do not supply file contents or `keep`.

`base_revision` is required for all updates. On `409`, fetch a fresh manifest and reconsider/retry the update. Success returns `project`, `revision`, file count `files`, decoded total `bytes`, and `url`.

Patch a project with `{"basic_password":"..."}` to enable/change Basic auth, or `{"basic_password":null}` to remove it. Passwords use the username `sss` and are stored as salted Argon2 hashes.

## Keys

```json
{"project":"6a91bd02c7ef","scopes":["upload","sync"],"expires_in":86400}
```

Creation returns `id`, `key`, `project`, `scopes`, and `expires` (Unix seconds or null). The raw key is returned once; storage holds its SHA-256 hash. Listing returns `{"keys":[...]}` metadata; `scopes` is a comma-separated string in list entries. A project scope does not authorize another project or key administration. Manifest reading is allowed for upload, sync, or delete keys. Administrator keys are created locally with `sss admin-key`.

## Serving and security model

Static responses use guessed MIME types, `Cache-Control: no-store`, `X-Content-Type-Options: nosniff`, a no-referrer policy, and a CSP disabling workers and cross-origin framing. There is no server-side application execution or encryption feature.

All projects share one origin. A publishing principal can publish active JavaScript; Basic passwords are not browser-origin isolation. Deploy separate server origins for mutually untrusted authors. Bind management access to a trusted network and distribute narrowly scoped keys. Never embed management keys in served JavaScript.
