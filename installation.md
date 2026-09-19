# Install sss

These instructions are for an agent installing sss for its user. Install the binary, register its bundled skill, and verify both.

## Install the Binary

1. Check whether `sss` is already on `PATH`. If it is, inspect `sss --version` and keep it unless the user asked for an update. Use `sss update` for an existing installation.
2. Detect the operating system and CPU architecture. Download the matching binary from the [GitHub releases](https://github.com/hmirin/sss/releases), using the latest stable release unless a version was requested. Inspect the release assets rather than constructing an asset filename. Verify the download against the release's published checksum.
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

Find the current agent's supported skill directory. Create an `sss` directory inside it and save the command's output as `SKILL.md`. Use the binary's output verbatim so the skill matches the installed version. Follow the agent's normal skill discovery or reload procedure.

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

Report the installed version, binary path, and skill path. Installation alone does not require starting a server or creating a project.

## Start Using sss

For local use, start the server:

```sh
sss serve
```

Then, in another terminal:

```sh
sss new --name hello
sss sync ./public --project <id-from-new>
```

For an existing server, set `SSS_UPSTREAM` or pass `--upstream`. Supply `SSS_BASIC_AUTH` only if authentication is required. The bundled skill covers publishing and project settings.
