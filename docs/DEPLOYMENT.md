# Deployment

Run the release binary under systemd or in a container. The server uses HTTP; terminate HTTPS outside sss. Keep `/var/lib/sss` persistent.

## systemd

Install `packaging/sss.service` and create the `sss` service user. Put runtime configuration in `/etc/sss/sss.env`:

```sh
SSS_LISTEN=127.0.0.1
SSS_PORT=8080
SSS_PUBLIC_URL=https://sss.example
```

Optionally add `SSS_BASIC_AUTH=user:password`; restrict the environment file to its administrator. The unit sets `--data-dir /var/lib/sss` and runs as the `sss` user. Do not expose or commit credentials.

## Default Project Lifetime

Set `SSS_DEFAULT_EXPIRES_IN=7d` or `--default-expires-in 7d` to give new projects a default idle lifetime. The default is `none`. Projects can override it on creation and administrators can change or remove expiration with `sss config --expires-in`. The deadline is the lifetime after the latest successful publish or rollback. Setting `expires_in` restarts it. Name or authentication changes, reads, and deleting an old version do not renew it. Existing projects are unaffected by server default changes.

Set `SSS_DEFAULT_ON_EXPIRE=retain` or `--default-on-expire retain` to keep expired content in server storage. The product default is `delete`. For example:

```sh
sss serve --default-expires-in 7d --default-on-expire retain
sss config --project ID --on-expire retain
```

The server default applies only to new projects and does not set or change a lifetime. `new` and `config` accept `--on-expire delete` or `--on-expire retain`; JSON uses `on_expire`. CLI flags override JSON. Omission or `null` on creation inherits the server default; omission or `null` on config preserves the policy. Changing only the policy leaves the exact deadline unchanged. Disabling and re-enabling expiration preserves the policy.

At the deadline, both policies make all current, image, fixed-version, raw file, and project API requests unavailable and omit the project from listing. Shared administrator credentials do not bypass expiration. Cleanup deletes only expired projects with `on_expire=delete`, on startup and every 30 seconds. Retained content keeps its files, rows, and deadline and consumes disk. No API browses or revives expired projects. Project summaries expose `on_expire`; `/api/status` and `doctor` without a project expose `default_on_expire`.

## Docker

```sh
docker build -t sss .
docker run --rm -p 127.0.0.1:8080:8080 -v sss-data:/var/lib/sss sss
```

The image binds to `0.0.0.0:8080` inside the container. Set `SSS_PUBLIC_URL` to the external origin; configure shared authentication with `SSS_BASIC_AUTH` if needed.

## Storage

SQLite records project policy, current version, the next version number, and retained snapshots. Version files live below `projects/<id>/<number>/`. A data-directory lock prevents two servers from sharing a store. Startup removes uncommitted snapshot directories while keeping all retained versions.

Back up the entire data directory while the service is stopped. Restore it with its original permissions. Delete obsolete versions to reclaim disk, or configure expiration with `on_expire=delete`. Current versions cannot be deleted. Expiration with `retain` does not reclaim storage.

New stores use SQLite `user_version=3`. This build atomically upgrades a format 2 store on first startup by adding the required `projects.on_expire` column. Every existing project gets the historical `delete` policy, independently of the configured server default. The conversion preserves all existing project fields, versions, lifetimes, exact deadlines, and files. It runs once. Other unsupported formats, including the API-key prototype, are rejected without migration.

Binaries without format 3 support refuse the upgraded store before cleanup. Keep a stopped-server backup of the complete format 2 store if you may need to return to an older binary. A binary downgrade alone cannot open format 3.

## Updating

Use `sss update` as the owner of the binary, or replace it with a verified release artifact, then restart the service. A successful binary update does not restart the server. Rebuild and replace a container image instead of self-updating inside a container.

## Release Contract

`.github/workflows/release.yml` builds macOS arm64/x86_64 and Linux arm64/x86_64 GNU and musl targets. Tag `v<VERSION>` must match Cargo.toml. Each archive contains `sss` and is named `sss-v<VERSION>-<target>.tar.gz`. A signed `sss-v<VERSION>-checksums.txt` manifest and its `.sigstore.json` bundle bind archive digests to the repository's release workflow. The updater pins the repository identity and exact tag.

The attestation trust root is Sigstore public-good, matching public GitHub release attestations. Publication requires that provenance to be available; private-repository attestation support is not a substitute. Test a published release-to-release update before calling a rollout complete.
