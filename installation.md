# Install sss

These instructions are for an agent setting up sss for its user. Confirm the intended setup, install the required binaries, register the bundled skill, and verify the result.

## Choose Where sss Runs

`sss serve` runs on the machine that hosts the sites. `sss new` and `sss sync` run on the machine that publishes files. They use the same binary and can run on different machines.

Before installing, establish which setup the user wants:

- **Local:** run the server and publishing CLI on this machine.
- **Remote server:** host sites on another machine and publish from here. Confirm the target host and access method; install sss on both machines for their respective OS and architecture.
- **Existing server:** install only the publishing CLI here. Confirm the server URL and whether authentication is required.

Reuse decisions already made in the conversation. Ask only about missing choices, grouping them into a short exchange: where to host, who should be able to reach it (local, private network, or public), and whether to enable Basic authentication. Do not assume that installing the CLI locally satisfies a request to host remotely.

When discussing authentication, check whether the server's user-facing URL will use HTTPS. Basic authentication does not encrypt credentials. For remote use, arrange and verify HTTPS before sending credentials; loopback-only local use can use HTTP. A private network alone does not establish that the URL uses HTTPS.

Explain that without shared server authentication, anyone who can reach the server can create projects and change access settings. Project-specific passwords do not protect those server-wide operations. Ask about separate viewing or editing rules only when the user needs them.

## Install the Binary

Follow these steps on each machine that needs sss.

1. Check whether `sss` is already on `PATH`. If it is, inspect `sss --version` and keep it unless the user asked for an update. Use `sss update` for an existing installation.
2. Detect the operating system and CPU architecture. Download the matching binary from the [GitHub releases](https://github.com/hmirin/sss/releases), using the latest stable release unless a version was requested. Inspect the release assets rather than constructing an asset filename. Download the matching `.sigstore.json` attestation bundle and verify the archive's provenance before extracting or running it. Use GitHub CLI with the selected release tag (shown here as `$tag`) and downloaded archive (`$archive`):

   ```sh
   gh attestation verify "$archive" \
     --bundle "$archive.sigstore.json" \
     --repo hmirin/sss \
     --cert-identity "https://github.com/hmirin/sss/.github/workflows/release.yml@refs/tags/$tag" \
     --source-ref "refs/tags/$tag"
   ```

   Also compare the archive against the release's published checksum. Stop if verification fails; do not install an unverified release.
3. Install `sss` into an existing user-writable directory on `PATH`, or use `~/.local/bin`. If necessary, add that directory to the user's shell configuration without duplicating an existing entry.

If a matching binary is unavailable, build from source with Rust and Cargo:

```sh
git clone https://github.com/hmirin/sss.git
cd sss
cargo build --release --locked
mkdir -p ~/.local/bin
install -m 755 target/release/sss ~/.local/bin/sss
```

Use the requested release tag when building a specific version. If the repository or release cannot be accessed, report the missing access rather than downloading from another source.

## Register the Agent Skill

Read the skill bundled with the installed binary:

```sh
sss --skill
```

On each machine where an agent will operate sss, find that agent's supported skill directory. A server-only machine does not need a skill installation. Create an `sss` directory inside it and save the command's output as `SKILL.md`. Use the binary's output verbatim so the skill matches the installed version. Follow the agent's normal skill discovery or reload procedure.

On an update, refresh this file from the new binary. Preserve any user-authored additions separately rather than silently overwriting them.

## Updates

```sh
sss update --check
sss update
```

The updater verifies the release's signed checksum manifest and archive before replacing the binary. It refreshes unmodified sss skills in `~/.agents/skills/sss`, `~/.claude/skills/sss`, and `${CODEX_HOME:-~/.codex}/skills/sss`. For a custom location, save `sss --skill` there again after updating. Locally edited skills are preserved and reported.

Private release downloads require `GH_TOKEN` with access to the repository. Updating the binary does not restart a running server.

## Verify

```sh
sss --version
sss --help
sss --skill
```

Confirm that `sss` resolves to the installed binary, that the skill output contains YAML frontmatter with `name: sss`, and that the saved `SKILL.md` matches that output.

Report the installed version and binary path for each machine, and the skill path where registered. Stop here if the user requested installation only. Otherwise, complete the agreed setup below.

## Complete the Setup

### Local Server

For local use, start the server:

```sh
sss serve
```

Then, in another terminal:

```sh
sss new --name hello
sss sync ./public --project <id-from-new>
```

### Remote Server

Use the agreed host and access method. Inspect existing services and data before changing them. Choose persistent storage and run `sss serve` under the host's service manager so it survives logout and restarts.

Set up or reuse the chosen HTTPS endpoint, such as Tailscale Serve or Caddy, while preserving the agreed network access scope. Keep sss on loopback when the proxy runs on the same host. Set `SSS_PUBLIC_URL` to the externally reachable URL; this setting does not configure HTTPS itself.

If requested, set `SSS_BASIC_AUTH` in the service's protected environment. Keep credentials out of repositories, published files, shell history, and reports. Verify the HTTPS certificate and endpoint before sending credentials; do not bypass certificate errors.

### Remote or Existing Server: Publishing Client

On the publishing machine, set `SSS_UPSTREAM` to the agreed URL or pass `--upstream` to individual commands:

```sh
export SSS_UPSTREAM=https://your-server
sss new --name hello
sss sync ./public --project <id-from-new>
```

Supply `SSS_BASIC_AUTH` or `--basic_auth` only when required. Installing a client for an existing server does not require changing that server's hosting or access settings.

### Verify the Working Setup

For a requested deployment, publish a small disposable site from the intended publishing machine and fetch its returned URL from the intended viewing environment. Check anonymous and authenticated requests against the agreed viewing and editing rules. Restart a newly configured service and confirm the site persists. Remove the test project afterward unless the user wants to keep it.

Report the server host, publishing machine, verified URL, access scope, and whether authentication is enabled, without including passwords. State any untested environments or remaining setup steps. Do not report a remote deployment as complete when only the local CLI has been installed.
