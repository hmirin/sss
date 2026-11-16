# Releasing sss

`Cargo.toml` contains the package version. Create a `v<version>` tag on `main` after CI passes. The tag starts `.github/workflows/release.yml`, which builds and attests the six GitHub Release archives, publishes the GitHub Release, then publishes `simple-static-server` to crates.io.

The crates.io job uses Trusted Publishing. Configure the trusted publisher on the `simple-static-server` crate with these values:

| Field | Value |
| --- | --- |
| Provider | GitHub Actions |
| GitHub owner | `hmirin` |
| Repository | `sss` |
| Workflow filename | `release.yml` |
| Environment | `crates-io` |

The GitHub `crates-io` environment must allow only `v*` tags. The publish job requests `id-token: write`, exchanges the GitHub identity for a temporary crates.io token, and revokes it when the job ends. Do not add a long-lived crates.io token to GitHub Actions secrets.

The first crates.io version, `0.1.3`, was published manually. Trusted Publishing applies to later versions after the crate setting and GitHub environment are in place. Check both release channels and install the published crate with `cargo install simple-static-server --locked` before calling a release complete.
