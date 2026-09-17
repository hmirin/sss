# CLI reference

`sss` is a single executable containing a client and server. `sss --help` and subcommand `--help` show the installed command contract.

## Common options and credentials

| Option | Meaning |
|---|---|
| `--upstream URL` | Management server origin; otherwise use `.sss.json`. |
| `--project ID` | Explicit project; otherwise use `.sss.json`. |
| `--json` | Machine-readable JSON on standard output. |
| `--allow-http` | Permit insecure HTTP to a non-loopback server; use only on an explicitly trusted transport. |

Client management commands read the bearer token from `SSS_API_KEY`. Do not put credentials in `.sss.json`, commit them, or upload them as site files. Administrator keys create projects and issue/revoke scoped keys. Scoped keys act only on their project and permitted operations.

## Create and publish

```sh
sss new --upstream https://preview.example
sss new --upstream https://preview.example --basic-auth
sss new --upstream https://preview.example --basic-auth --password-stdin
sss upload index.html assets/style.css
sss sync . --dry-run
sss sync ./dist
```

`new` creates a project, prints its stable `/s/{ID}/` URL, and writes `.sss.json` with its upstream and project ID. `upload` adds or replaces the selected files and preserves other remote files. Paths are relative site paths; use relative asset references in HTML as well.

`sync DIR` makes the remote project match the included local directory contents. Files absent locally are removed remotely. `--dry-run` reports the comparison without uploading or deleting. `.sssignore` supplies additional ignore patterns. Hidden files/directories (including `.git`, `.env`, `.sss.json`, and `.sssignore`), `node_modules`, `target`, `.pem`, and `.key` files are excluded. Symlinks are not uploaded.

Updates compare SHA-256 hashes, transfer changed files, and publish the prepared revision together. A stale revision is rejected instead of overwriting concurrent changes. This is an atomic server revision switch, not a browser-wide snapshot: requests straddling a deployment may see different revisions. Prefer content-hashed asset names where that matters.

## Browsing passwords

```sh
sss config --basic-auth                 # Prompt without echo
sss config --basic-auth --password-stdin
sss config --no-basic-auth
```

Basic authentication uses the fixed username `sss`. It covers HTML, assets, and missing paths within an existing protected project. Use HTTPS. An inline `--basic-auth PASSWORD` argument is supported but may expose the password in shell history or process listings; prefer the prompt or stdin.

## Delete

```sh
sss delete old.html assets/old.css
sss delete --project 6a91bd02c7ef
```

With file arguments, delete only those files. With no files, delete the entire project and its scoped keys. These are immediate mutations; there is no trash or rollback command.

## API keys

```sh
sss keys create --project 6a91bd02c7ef --scope upload,sync --expires-in 86400
sss keys list
sss keys revoke KEY_ID
```

`--scope` is a comma-separated subset of `upload,sync,delete,config`; default `upload,sync`. `--expires-in` is lifetime in seconds; omission means no expiry. A key is returned only at creation; list returns metadata, not key material. Upload/sync/delete scopes include the manifest access those operations need. `delete` permits whole-project deletion as well as file deletion. Scoped keys cannot create projects or issue keys.

## Server administration

```sh
sss admin-key --data-dir /var/lib/sss
sss admin-key --data-dir /var/lib/sss --json
sss serve --listen 127.0.0.1:8080 --data-dir /var/lib/sss \
  --public-url https://preview.example
```

`admin-key` creates an additional administrator key in the local data directory and emits its secret once. It requires filesystem access, not an existing API key. Secure the data directory and capture the token privately. `serve` uses the same directory and does not print API credentials. `--public-url` must be an HTTP(S) origin, without a path, credentials, query, or fragment. `--listen` defaults to `127.0.0.1:8080`.

Limits: 50 MiB of decoded file contents and 5,000 files per project; 72 MiB per HTTP request. Paths must be nonempty relative UTF-8 paths, at most 1,024 bytes, with no empty, `.` or `..` components, backslashes, colons, NULs, or excluded components.
