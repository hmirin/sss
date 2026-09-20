use crate::{auth::Rule, hash, valid_path, Cli, Command, ProjectArgs};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use reqwest::Method;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};
struct Client {
    http: reqwest::Client,
    upstream: String,
    credential: Option<String>,
}
impl Client {
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.upstream));
        if let Some(v) = &self.credential {
            let (u, p) = crate::auth::split(v)?;
            req = req.basic_auth(u, Some(p));
        }
        if let Some(b) = body {
            req = req.json(&b)
        }
        let resp = req.send().await.context("request failed")?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            bail!("server returned {status}: {body}")
        }
        Ok(serde_json::from_str(&body)?)
    }
}
fn project_body(args: &ProjectArgs) -> Result<Value> {
    let mut body = if let Some(p) = &args.config {
        let mut bytes = Vec::new();
        if p == Path::new("-") {
            std::io::stdin().take(1024 * 1024).read_to_end(&mut bytes)?;
        } else {
            bytes = fs::read(p)?;
        }
        serde_json::from_slice::<Value>(&bytes)
            .map_err(|_| anyhow::anyhow!("invalid project JSON"))?
    } else {
        json!({})
    };
    if !body.is_object() {
        bail!("project config must be an object")
    }
    if let Some(value) = &args.expires_in {
        crate::expiry::duration(value)?;
        body["expires_in"] = json!(value);
    }
    if let Some(name) = &args.name {
        body["name"] = json!(name)
    }
    if args.view.is_some() || args.write.is_some() {
        if body.get("auth").is_none() {
            body["auth"] = json!({})
        }
        if !body["auth"].is_object() {
            bail!("auth must be an object")
        }
        for (key, arg) in [("view", &args.view), ("write", &args.write)] {
            if let Some(v) = arg {
                body["auth"][key] = serde_json::to_value(Rule::argument(v)?)?;
            }
        }
    }
    Ok(body)
}
fn matcher(root: &Path) -> Result<Gitignore> {
    let mut b = GitignoreBuilder::new(root);
    let path = root.join(".sssignore");
    if path.exists() {
        if let Some(e) = b.add(path) {
            return Err(e.into());
        }
    }
    Ok(b.build()?)
}
fn collect(root: &Path, inputs: &[PathBuf], directory: bool) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut result = BTreeMap::new();
    let rules = matcher(root)?;
    let mut total = 0usize;
    let mut add = |path: &Path, relative: &Path| -> Result<()> {
        let name = relative
            .to_str()
            .context("path must be UTF-8")?
            .replace('\\', "/");
        if !valid_path(&name) || rules.matched_path_or_any_parents(path, false).is_ignore() {
            if directory {
                return Ok(());
            }
            bail!("excluded path: {name}")
        };
        if fs::symlink_metadata(path)?.file_type().is_symlink() {
            bail!("symlinks are not allowed: {name}")
        };
        let limit = 50 * 1024 * 1024;
        if fs::metadata(path)?.len() > (limit - total) as u64 {
            bail!("project upload exceeds quota");
        }
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take((limit - total + 1) as u64)
            .read_to_end(&mut bytes)?;
        total += bytes.len();
        if total > 50 * 1024 * 1024 || result.len() >= 5000 {
            bail!("project upload exceeds quota")
        };
        result.insert(name, bytes);
        Ok(())
    };
    if directory {
        if !fs::symlink_metadata(root)?.is_dir() {
            bail!("sync source must be a directory")
        }
        if fs::symlink_metadata(root)?.file_type().is_symlink() {
            bail!("sync root must not be a symlink")
        };
        for e in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| {
                e.depth() == 0
                    || (!e.file_name().to_string_lossy().starts_with('.')
                        && !matches!(e.file_name().to_str(), Some("node_modules" | "target"))
                        && !rules
                            .matched_path_or_any_parents(e.path(), e.file_type().is_dir())
                            .is_ignore())
            })
        {
            let e = e?;
            if e.file_type().is_symlink() {
                bail!("symlinks are not allowed: {}", e.path().display())
            }
            if e.file_type().is_file() {
                add(e.path(), e.path().strip_prefix(root)?)?
            }
        }
    } else {
        for input in inputs {
            if input.is_absolute()
                || input.components().any(|c| {
                    matches!(
                        c,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
            {
                bail!("upload paths must be relative to the current directory")
            };
            let path = root.join(input);
            let canonical = path.canonicalize()?;
            if !canonical.starts_with(root.canonicalize()?) {
                bail!("path escapes directory")
            };
            let mut check = root.to_path_buf();
            for component in input.components() {
                check.push(component);
                if fs::symlink_metadata(&check)?.file_type().is_symlink() {
                    bail!("symlinks are not allowed")
                }
            }
            let relative = input
                .components()
                .filter(|c| !matches!(c, Component::CurDir))
                .collect::<PathBuf>();
            add(&path, &relative)?
        }
    }
    Ok(result)
}

pub async fn run(cli: &Cli) -> Result<()> {
    let url = url::Url::parse(&cli.upstream)?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("upstream must be an HTTP(S) origin URL")
    }
    if let Some(v) = &cli.basic_auth {
        crate::auth::split(v)?;
    }
    let client = Client {
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(
                if matches!(cli.command, Some(Command::Doctor)) {
                    15
                } else {
                    120
                },
            ))
            .build()?,
        upstream: cli.upstream.trim_end_matches('/').into(),
        credential: cli.basic_auth.clone(),
    };
    let project = cli.project.as_deref();
    if let Some(p) = project {
        if p.is_empty() || !p.bytes().all(|c| c.is_ascii_alphanumeric()) {
            bail!("invalid project ID")
        }
    }
    let p = || project.context("specify --project ID");
    let result = match cli.command.as_ref().unwrap() {
        Command::New(a) => {
            client
                .call(Method::POST, "/api/projects", Some(project_body(a)?))
                .await?
        }
        Command::List => client.call(Method::GET, "/api/projects", None).await?,
        Command::Info => {
            client
                .call(Method::GET, &format!("/api/projects/{}", p()?), None)
                .await?
        }
        Command::Doctor => return doctor(&client, project).await,
        Command::Diff { dir } => diff(&client, p()?, collect(dir, &[], true)?).await?,
        Command::Upload { files } => {
            publish(
                &client,
                p()?,
                collect(&std::env::current_dir()?, files, false)?,
                false,
                false,
            )
            .await?
        }
        Command::Sync {
            dir,
            dry_run,
            watch,
        } => {
            if *watch {
                return watch_directory(&client, p()?, dir).await;
            }
            publish(&client, p()?, collect(dir, &[], true)?, true, *dry_run).await?
        }
        Command::Delete { files } => {
            let p = p()?;
            if files.is_empty() {
                client
                    .call(Method::DELETE, &format!("/api/projects/{p}"), None)
                    .await?
            } else {
                for f in files {
                    if !valid_path(f) {
                        bail!("unsafe path")
                    }
                }
                let m = client
                    .call(Method::GET, &format!("/api/projects/{p}/manifest"), None)
                    .await?;
                client
                    .call(
                        Method::POST,
                        &format!("/api/projects/{p}/versions"),
                        Some(json!({"mode":"delete","delete":files,"base_revision":m["revision"]})),
                    )
                    .await?
            }
        }
        Command::Config(a) => {
            let body = project_body(a)?;
            client
                .call(
                    Method::PATCH,
                    &format!("/api/projects/{}", p()?),
                    Some(body),
                )
                .await?
        }
        Command::Versions => {
            client
                .call(
                    Method::GET,
                    &format!("/api/projects/{}/versions", p()?),
                    None,
                )
                .await?
        }
        Command::Rollback { version } => {
            let p = p()?;
            let m = client
                .call(Method::GET, &format!("/api/projects/{p}/manifest"), None)
                .await?;
            client
                .call(
                    Method::PUT,
                    &format!("/api/projects/{p}/current"),
                    Some(json!({"version":version,"base_revision":m["revision"]})),
                )
                .await?
        }
        Command::DeleteVersion { version } => {
            client
                .call(
                    Method::DELETE,
                    &format!("/api/projects/{}/versions/{version}", p()?),
                    None,
                )
                .await?
        }
        _ => unreachable!(),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
async fn publish(
    c: &Client,
    p: &str,
    local: BTreeMap<String, Vec<u8>>,
    sync: bool,
    dry: bool,
) -> Result<Value> {
    let m = c
        .call(Method::GET, &format!("/api/projects/{p}/manifest"), None)
        .await?;
    let remote: BTreeMap<String, String> = serde_json::from_value(m["files"].clone())?;
    let changed: BTreeMap<String, String> = local
        .iter()
        .filter(|(k, v)| remote.get(*k) != Some(&hash(v)))
        .map(|(k, v)| (k.clone(), STANDARD.encode(v)))
        .collect();
    let removed: Vec<_> = if sync {
        remote
            .keys()
            .filter(|k| !local.contains_key(*k))
            .cloned()
            .collect()
    } else {
        vec![]
    };
    if dry {
        let added: Vec<_> = changed
            .keys()
            .filter(|k| !remote.contains_key(*k))
            .collect();
        let modified: Vec<_> = changed.keys().filter(|k| remote.contains_key(*k)).collect();
        return Ok(
            json!({"project":p,"dry_run":true,"added":added,"modified":modified,"deleted":removed}),
        );
    }
    if changed.is_empty() && removed.is_empty() && m["version"].as_u64() != Some(0) {
        let mut result = m.clone();
        result.as_object_mut().unwrap().remove("files");
        result["unchanged"] = json!(true);
        return Ok(result);
    }
    let mut body =
        json!({"mode":if sync{"sync"}else{"upload"},"files":changed,"base_revision":m["revision"]});
    if sync {
        body["keep"] = json!(local.keys().collect::<Vec<_>>())
    };
    c.call(
        Method::POST,
        &format!("/api/projects/{p}/versions"),
        Some(body),
    )
    .await
}
async fn doctor(c: &Client, project: Option<&str>) -> Result<()> {
    let health = c.call(Method::GET, "/health", None).await;
    let mut report = json!({"upstream":c.upstream,"client_version":env!("CARGO_PKG_VERSION"),"credentials_supplied":c.credential.is_some(),"checks":{}});
    let reachable = health.is_ok();
    report["checks"]["health"] = match health {
        Ok(v) => json!({"ok":true,"response":v}),
        Err(e) => json!({"ok":false,"error":format!("{e:#}")}),
    };
    let mut authorized = false;
    if reachable {
        let path = project
            .map(|p| format!("/api/projects/{p}"))
            .unwrap_or_else(|| "/api/status".into());
        let check = c.call(Method::GET, &path, None).await;
        authorized = check.is_ok();
        report["checks"][if project.is_some() {
            "project_edit_access"
        } else {
            "server_admin_access"
        }] = match check {
            Ok(v) => json!({"ok":true,"response":v}),
            Err(e) => json!({"ok":false,"error":format!("{e:#}")}),
        };
    }
    report["ok"] = json!(reachable && authorized);
    println!("{}", serde_json::to_string_pretty(&report)?);
    if !reachable || !authorized {
        bail!("doctor found a connection or access problem; see checks");
    }
    Ok(())
}

async fn watch_directory(c: &Client, p: &str, dir: &Path) -> Result<()> {
    let initial = collect(dir, &[], true)?;
    println!("{}", publish(c, p, initial.clone(), true, false).await?);
    eprintln!(
        "watching {}; publishing after changes settle (Ctrl-C to stop)",
        dir.display()
    );
    let mut published = initial.clone();
    let mut observed = initial;
    let mut changed = std::time::Instant::now();
    loop {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>return Ok(()),
            _=tokio::time::sleep(std::time::Duration::from_millis(250))=>{}
        }
        let next = match collect(dir, &[], true) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("watch: local scan failed; nothing published: {e}");
                changed = std::time::Instant::now();
                continue;
            }
        };
        if next != observed {
            observed = next;
            changed = std::time::Instant::now();
            continue;
        }
        if observed == published || changed.elapsed() < std::time::Duration::from_millis(750) {
            continue;
        }
        // Stop on server errors or a concurrent revision conflict rather than overwriting it automatically.
        let result = tokio::select! {
            _=tokio::signal::ctrl_c()=>return Ok(()),
            result=publish(c,p,observed.clone(),true,false)=>result?
        };
        println!("{result}");
        published = observed.clone();
    }
}

fn text_patch(path: &str, old: &[u8], new: &[u8]) -> Option<String> {
    if old.len() + new.len() > 1024 * 1024 || old.contains(&0) || new.contains(&0) {
        return None;
    }
    let before = std::str::from_utf8(old).ok()?;
    let after = std::str::from_utf8(new).ok()?;
    let a: Vec<_> = before.split_inclusive('\n').collect();
    let b: Vec<_> = after.split_inclusive('\n').collect();
    let prefix = a.iter().zip(&b).take_while(|(a, b)| a == b).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let start = prefix.saturating_sub(3);
    let ae = (a.len() - suffix + 3).min(a.len());
    let be = (b.len() - suffix + 3).min(b.len());
    let mut patch = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{},{} +{},{} @@\n",
        if ae == start { 0 } else { start + 1 },
        ae - start,
        if be == start { 0 } else { start + 1 },
        be - start
    );
    let mut line = |sign: char, value: &str| {
        patch.push(sign);
        patch.push_str(value);
        if !value.ends_with('\n') {
            patch.push_str("\n\\ No newline at end of file\n");
        }
    };
    for v in &a[start..prefix] {
        line(' ', v);
    }
    for v in &a[prefix..a.len() - suffix] {
        line('-', v);
    }
    for v in &b[prefix..b.len() - suffix] {
        line('+', v);
    }
    for v in &a[a.len() - suffix..ae] {
        line(' ', v);
    }
    Some(patch)
}
async fn diff(c: &Client, p: &str, local: BTreeMap<String, Vec<u8>>) -> Result<Value> {
    let m = c
        .call(Method::GET, &format!("/api/projects/{p}/manifest"), None)
        .await?;
    let remote: BTreeMap<String, String> = serde_json::from_value(m["files"].clone())?;
    let version = m["version"].as_u64().context("invalid version")?;
    let names: std::collections::BTreeSet<_> = local.keys().chain(remote.keys()).collect();
    let mut changes = Vec::new();
    for name in names {
        if local
            .get(name)
            .is_some_and(|bytes| remote.get(name) == Some(&hash(bytes)))
        {
            continue;
        }
        let old = if remote.contains_key(name) {
            let mut url = url::Url::parse(&format!(
                "{}/api/projects/{p}/versions/{version}/files/",
                c.upstream
            ))?;
            url.path_segments_mut()
                .unwrap()
                .pop_if_empty()
                .extend(name.split('/'));
            let mut req = c.http.get(url);
            if let Some(v) = &c.credential {
                let (u, p) = crate::auth::split(v)?;
                req = req.basic_auth(u, Some(p));
            }
            let bytes = req
                .send()
                .await
                .context("fetching published file")?
                .error_for_status()?
                .bytes()
                .await?
                .to_vec();
            if remote.get(name) != Some(&hash(&bytes)) {
                bail!("published file changed; retry diff");
            }
            bytes
        } else {
            Vec::new()
        };
        let new = local.get(name).map(Vec::as_slice).unwrap_or_default();
        let kind = if !remote.contains_key(name) {
            "added"
        } else if !local.contains_key(name) {
            "deleted"
        } else {
            "modified"
        };
        let patch = text_patch(name, &old, new);
        changes.push(json!({"path":name,"kind":kind,"before_bytes":old.len(),"after_bytes":new.len(),"patch":patch,"content_omitted":patch.is_none()}));
    }
    let latest = c
        .call(Method::GET, &format!("/api/projects/{p}/manifest"), None)
        .await?;
    if latest["revision"] != m["revision"] {
        bail!("revision changed; retry diff");
    }
    Ok(json!({"project":p,"version":version,"revision":m["revision"],"changes":changes}))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn diff_context_and_missing_newline() {
        let patch = text_patch("index.html", b"keep\nold\ntail\n", b"keep\nnew\ntail\n").unwrap();
        assert!(patch.contains("@@ -1,3 +1,3 @@\n keep\n-old\n+new\n tail\n"));
        let patch = text_patch("a", b"old", b"new").unwrap();
        assert_eq!(patch.matches("No newline at end of file").count(), 2);
        assert!(text_patch("a", b"\0", b"new").is_none());
    }
    #[test]
    fn rejects_paths() {
        for p in [
            "../a",
            "/a",
            "a//b",
            ".env",
            "a/.git/config",
            "x\\y",
            "x.key",
        ] {
            assert!(!valid_path(p), "{p}")
        }
        assert!(valid_path("assets/app.js"));
    }
    #[test]
    fn ignores_secrets_and_patterns() {
        let t = tempfile::tempdir().unwrap();
        fs::write(t.path().join("index.html"), "ok").unwrap();
        fs::write(t.path().join(".env"), "secret").unwrap();
        fs::write(t.path().join("secret.txt"), "secret").unwrap();
        fs::write(t.path().join(".sssignore"), "secret.txt\n").unwrap();
        let f = collect(t.path(), &[], true).unwrap();
        assert_eq!(f.len(), 1);
        assert!(f.contains_key("index.html"));
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinks() {
        let t = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink("/etc/passwd", t.path().join("link")).unwrap();
        assert!(collect(t.path(), &[], true).is_err());
    }
}
