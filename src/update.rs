use anyhow::{bail, Context, Result};
use attestation_verify::{
    Bundle, CheckpointOriginPolicy, GithubPolicy, RefPolicy, RepositoryIdentity, SignerPolicy,
    SourcePolicy, TrustStore, Verifier, WorkflowPath, WorkflowRevisionPolicy,
};
use serde::Deserialize;
use serde_json::json;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::Duration,
};
const REPO: &str = "hmirin/sss";
const WORKFLOW: &str = ".github/workflows/release.yml";
const SKILL: &str = include_str!("../skill.md");
#[derive(Deserialize)]
struct Asset {
    name: String,
    url: String,
}
#[derive(Deserialize)]
struct Release {
    tag_name: String,
    assets: Vec<Asset>,
    draft: bool,
    prerelease: bool,
}
struct Github {
    client: reqwest::Client,
    token: Option<String>,
}
impl Github {
    async fn download(&self, url: &str, limit: usize, asset: bool) -> Result<Vec<u8>> {
        let parsed = url::Url::parse(url)?;
        if parsed.scheme() != "https" || parsed.host_str() != Some("api.github.com") {
            bail!("unexpected GitHub API asset URL")
        }
        let mut request = self.client.get(url).header(
            "accept",
            if asset {
                "application/octet-stream"
            } else {
                "application/vnd.github+json"
            },
        );
        if let Some(token) = &self.token {
            request = request.bearer_auth(token)
        }
        let mut response = request
            .send()
            .await?
            .error_for_status()
            .context("GitHub release unavailable; private repositories require GH_TOKEN")?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            if bytes.len() + chunk.len() > limit {
                bail!("release download exceeds size limit")
            };
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
    async fn asset(&self, release: &Release, name: &str, limit: usize) -> Result<Vec<u8>> {
        let asset = release
            .assets
            .iter()
            .find(|a| a.name == name)
            .with_context(|| format!("release missing {name}"))?;
        self.download(&asset.url, limit, true).await
    }
}
pub async fn run(check: bool) -> Result<()> {
    let github = Github {
        client: reqwest::Client::builder()
            .user_agent(concat!("sss/", env!("CARGO_PKG_VERSION")))
            .https_only(true)
            .timeout(Duration::from_secs(120))
            .build()?,
        token: std::env::var("GH_TOKEN").ok(),
    };
    let release: Release = serde_json::from_slice(
        &github
            .download(
                &format!("https://api.github.com/repos/{REPO}/releases/latest"),
                2 * 1024 * 1024,
                false,
            )
            .await?,
    )?;
    let version = semver::Version::parse(
        release
            .tag_name
            .strip_prefix('v')
            .context("release tag must start with v")?,
    )?;
    if release.draft || release.prerelease || !version.pre.is_empty() {
        bail!("expected a stable release")
    }
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION"))?;
    if check || version <= current {
        println!(
            "{}",
            json!({"updated":false,"update_available":version>current,"current_version":current.to_string(),"latest_version":version.to_string()})
        );
        return Ok(());
    }
    let target = target()?;
    let manifest_name = format!("sss-{}-checksums.txt", release.tag_name);
    let archive_name = format!("sss-{}-{target}.tar.gz", release.tag_name);
    let manifest = github.asset(&release, &manifest_name, 1024 * 1024).await?;
    let bundle = github
        .asset(
            &release,
            &format!("{manifest_name}.sigstore.json"),
            4 * 1024 * 1024,
        )
        .await?;
    verify_manifest(&release.tag_name, &manifest, &bundle)?;
    let digest = manifest_digest(std::str::from_utf8(&manifest)?, &archive_name)?;
    let archive = github
        .asset(&release, &archive_name, 128 * 1024 * 1024)
        .await?;
    let binary = verified_binary(&archive, &digest)?;
    let exe = std::env::current_exe()?.canonicalize()?;
    install_binary(&binary, &version, &exe, skill_paths())?;
    Ok(())
}
fn install_binary(
    binary: &[u8],
    version: &semver::Version,
    exe: &Path,
    skills: Vec<PathBuf>,
) -> Result<()> {
    let parent = exe.parent().context("executable has no parent")?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(parent.join(".sss-update.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock).context("another update is running")?;
    let current = env!("CARGO_PKG_VERSION");
    let mut candidate =
        tempfile::NamedTempFile::new_in(exe.parent().context("executable has no parent")?)?;
    candidate.write_all(binary)?;
    candidate.as_file().sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        candidate
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))?;
    }
    let candidate = candidate.into_temp_path();
    let output = std::process::Command::new(&candidate)
        .arg("--version")
        .output()?;
    if !output.status.success()
        || String::from_utf8(output.stdout)?.trim() != format!("sss {version}")
    {
        bail!("release binary version mismatch")
    }
    let skill = std::process::Command::new(&candidate)
        .arg("--skill")
        .output()?;
    if !skill.status.success() {
        bail!("release binary could not emit its skill")
    }
    let skill = String::from_utf8(skill.stdout)?;
    if !skill.starts_with("---\n") || !skill.lines().any(|l| l == "name: sss") {
        bail!("release skill is malformed")
    }
    // Prepare writes before replacing the running executable. Modified skills remain untouched.
    let mut prepared = Vec::new();
    let mut preserved = Vec::new();
    for path in skills {
        if !path.exists() {
            continue;
        }
        if fs::read_to_string(&path)? != SKILL {
            preserved.push(path);
            continue;
        }
        let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
        file.write_all(skill.as_bytes())?;
        file.as_file().sync_all()?;
        prepared.push((path, file));
    }
    candidate
        .persist(exe)
        .map_err(|e| e.error)
        .context("could not replace sss binary")?;
    let mut refreshed = Vec::new();
    let mut failed = Vec::new();
    for (path, file) in prepared {
        match file.persist(&path) {
            Ok(_) => refreshed.push(path),
            Err(_) => failed.push(path),
        }
    }
    println!(
        "{}",
        json!({"updated":true,"previous_version":current,"version":version.to_string(),"binary":exe,"attestation_verified":true,"skills_updated":refreshed,"skills_preserved":preserved,"skills_failed":failed,"server_restart_required":true})
    );
    if !failed.is_empty() {
        bail!(
            "binary updated; some skills could not be refreshed; regenerate them with sss --skill"
        )
    }
    Ok(())
}
fn skill_paths() -> Vec<PathBuf> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let codex = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".codex"));
    let mut paths = vec![
        codex.join("skills/sss/SKILL.md"),
        home.join(".claude/skills/sss/SKILL.md"),
        home.join(".agents/skills/sss/SKILL.md"),
    ];
    paths.sort();
    paths.dedup();
    paths
}
fn target() -> Result<String> {
    let arch = std::env::consts::ARCH;
    if !matches!(arch, "aarch64" | "x86_64") {
        bail!("no release target for this CPU")
    }
    match std::env::consts::OS {
        "macos" => Ok(format!("{arch}-apple-darwin")),
        "linux" => Ok(format!(
            "{arch}-unknown-linux-{}",
            if cfg!(target_env = "musl") {
                "musl"
            } else {
                "gnu"
            }
        )),
        _ => bail!("self-update supports macOS and Linux"),
    }
}
fn verify_manifest(tag: &str, manifest: &[u8], bundle: &[u8]) -> Result<()> {
    let repository = RepositoryIdentity::parse(REPO)?;
    let tag_ref = format!("refs/tags/{tag}");
    let policy = GithubPolicy::builder()
        .source(SourcePolicy {
            repository: repository
                .clone()
                .with_owner_id(1284876)
                .with_repository_id(1373982914),
            git_ref: RefPolicy::Exact(tag_ref.clone()),
            commit: None,
        })
        .signer(SignerPolicy {
            repository,
            path: WorkflowPath::new(WORKFLOW)?,
            revision: WorkflowRevisionPolicy::Ref(tag_ref),
        })
        .build()?;
    let trust = TrustStore::embedded_public_good()?;
    let log = trust
        .tlogs
        .iter()
        .find(|l| {
            l.base_url == "https://rekor.sigstore.dev"
                && l.public_key.key_details == "PKIX_ECDSA_P256_SHA_256"
        })
        .context("missing public-good log")?;
    let checkpoint = CheckpointOriginPolicy::builder()
        .allow_origin(log, "rekor.sigstore.dev - 1193050959916656506")?
        .build()?;
    let verifier = Verifier::builder()
        .trust_store(trust)
        .github_policy(policy)
        .checkpoint_origin_policy(checkpoint)
        .build()?;
    verifier
        .verify_bytes(manifest, &Bundle::from_json(bundle)?)
        .context("release checksum attestation did not verify")?;
    Ok(())
}
fn manifest_digest(manifest: &str, name: &str) -> Result<String> {
    let matches: Vec<_> = manifest
        .lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() == 2
                && fields[1].trim_start_matches('*') == name
                && fields[0].len() == 64
                && fields[0].bytes().all(|b| b.is_ascii_hexdigit())
            {
                Some(fields[0].to_ascii_lowercase())
            } else {
                None
            }
        })
        .collect();
    if matches.len() != 1 {
        bail!("checksum manifest must contain exactly one digest for {name}")
    }
    Ok(matches[0].clone())
}
fn verified_binary(archive: &[u8], digest: &str) -> Result<Vec<u8>> {
    if crate::hash(archive) != digest {
        bail!("release archive checksum mismatch")
    }
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let mut binary = None;
    for entry in tar.entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() != Path::new("sss") {
            continue;
        }
        if !entry.header().entry_type().is_file()
            || binary.is_some()
            || entry.size() > 256 * 1024 * 1024
        {
            bail!("invalid release binary entry")
        }
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes)?;
        binary = Some(bytes);
    }
    binary.context("archive does not contain sss")
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_untrusted_manifest() {
        assert!(verify_manifest("v0.2.0", b"fake", b"{}").is_err())
    }
    #[test]
    fn checksum_is_unambiguous() {
        let d = "a".repeat(64);
        assert_eq!(manifest_digest(&format!("{d}  x"), "x").unwrap(), d);
        assert!(manifest_digest(&format!("{d} x\n{d} x"), "x").is_err());
        assert!(manifest_digest("bad x", "x").is_err());
    }
    fn archive(link: bool) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        let mut header = tar::Header::new_gnu();
        header.set_size(if link { 0 } else { 3 });
        header.set_mode(0o755);
        if link {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("/tmp/other").unwrap();
        }
        header.set_cksum();
        tar.append_data(
            &mut header,
            "sss",
            if link { &b""[..] } else { &b"abc"[..] },
        )
        .unwrap();
        tar.into_inner().unwrap().finish().unwrap()
    }
    #[test]
    fn verifies_archive_before_extracting() {
        let a = archive(false);
        assert_eq!(verified_binary(&a, &crate::hash(&a)).unwrap(), b"abc");
        assert!(verified_binary(&a, &"0".repeat(64)).is_err());
        let a = archive(true);
        assert!(verified_binary(&a, &crate::hash(&a)).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn replacement_refreshes_only_unmodified_skills() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("sss");
        fs::write(&exe, b"old binary").unwrap();
        let skill = dir.path().join("SKILL.md");
        fs::write(&skill, SKILL).unwrap();
        let custom = dir.path().join("custom.md");
        fs::write(&custom, "custom skill").unwrap();
        let script=b"#!/bin/sh\ncase \"$1\" in\n--version) echo 'sss 9.0.0';;\n--skill) printf '%s\\n' '---' 'name: sss' 'description: Updated skill' '---';;\nesac\n";
        install_binary(
            script,
            &semver::Version::parse("9.0.0").unwrap(),
            &exe,
            vec![skill.clone(), custom.clone()],
        )
        .unwrap();
        assert_eq!(fs::read(&exe).unwrap(), script);
        assert!(fs::read_to_string(skill).unwrap().contains("Updated skill"));
        assert_eq!(fs::read_to_string(custom).unwrap(), "custom skill");
    }
    #[cfg(unix)]
    #[test]
    fn wrong_candidate_preserves_installation() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("sss");
        fs::write(&exe, b"old binary").unwrap();
        let script = b"#!/bin/sh\necho 'sss 0.0.0'\n";
        assert!(install_binary(
            script,
            &semver::Version::parse("9.0.0").unwrap(),
            &exe,
            vec![]
        )
        .is_err());
        assert_eq!(fs::read(exe).unwrap(), b"old binary");
    }
}
