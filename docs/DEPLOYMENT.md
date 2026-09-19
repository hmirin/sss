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

## Docker

```sh
docker build -t sss .
docker run --rm -p 127.0.0.1:8080:8080 -v sss-data:/var/lib/sss sss
```

The image binds to `0.0.0.0:8080` inside the container. Set `SSS_PUBLIC_URL` to the external origin; configure shared authentication with `SSS_BASIC_AUTH` if needed.

## Storage

SQLite records project policy, current version, the next version number, and retained snapshots. Version files live below `projects/<id>/<number>/`. A data-directory lock prevents two servers from sharing a store. Startup removes uncommitted snapshot directories while keeping all retained versions.

Back up the entire data directory while the service is stopped. Restore it with its original permissions. Retention is explicit: delete obsolete versions to reclaim disk. Current versions cannot be deleted.

The API-key prototype uses a different data format. This version requires a fresh directory; it refuses the old format without migrating or deleting its contents.

## Updating

Use `sss update` as the owner of the binary, or replace it with a verified release artifact, then restart the service. A successful binary update does not restart the server. Rebuild and replace a container image instead of self-updating inside a container.

## Release Contract

`.github/workflows/release.yml` builds macOS arm64/x86_64 and Linux arm64/x86_64 GNU and musl targets. Tag `v<VERSION>` must match Cargo.toml. Each archive contains `sss` and is named `sss-v<VERSION>-<target>.tar.gz`. A signed `sss-v<VERSION>-checksums.txt` manifest and its `.sigstore.json` bundle bind archive digests to the repository's release workflow. The updater pins the repository identity and exact tag.

The attestation trust root is Sigstore public-good, matching public GitHub release attestations. Publication requires that provenance to be available; private-repository attestation support is not a substitute. Test a published release-to-release update before calling a rollout complete.
