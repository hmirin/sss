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
            .timeout(std::time::Duration::from_secs(120))
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
        Command::Sync { dir, dry_run } => {
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
            if body.get("name").is_some() {
                bail!("config changes authentication only")
            }
            let access = body
                .get("auth")
                .context("specify --basic_auth_view, --basic_auth_write, or an auth config")?
                .clone();
            client
                .call(
                    Method::PATCH,
                    &format!("/api/projects/{}/auth", p()?),
                    Some(access),
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
#[cfg(test)]
mod tests {
    use super::*;
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
