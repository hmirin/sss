use crate::{hash, valid_path, AuthArgs, Cli, Command, KeyCommand};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};
#[derive(Serialize, Deserialize)]
struct Config {
    upstream: String,
    project: String,
}
struct Client {
    http: reqwest::Client,
    upstream: String,
    key: String,
}
impl Client {
    async fn call(&self, method: Method, path: &str, body: Option<Value>) -> Result<Value> {
        let mut req = self
            .http
            .request(method, format!("{}{path}", self.upstream))
            .bearer_auth(&self.key);
        if let Some(b) = body {
            req = req.json(&b)
        }
        let resp = req.send().await.context("request failed")?;
        let status = resp.status();
        let body = resp.text().await?;
        if !status.is_success() {
            bail!("server returned {status}: {body}")
        };
        Ok(serde_json::from_str(&body)?)
    }
}
fn password(a: &AuthArgs) -> Result<Option<String>> {
    if a.no_basic_auth {
        return Ok(None);
    }
    if a.password_stdin {
        let mut s = String::new();
        std::io::stdin().take(4098).read_to_string(&mut s)?;
        return Ok(Some(s.trim_end_matches(['\r', '\n']).to_owned()));
    }
    if let Some(p) = &a.basic_auth {
        if p.is_empty() {
            return Ok(Some(rpassword::prompt_password("Viewing password: ")?));
        }
        return Ok(Some(p.clone()));
    }
    Ok(None)
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
fn print(value: &Value, json_output: bool) {
    if json_output {
        println!("{value}")
    } else if value.get("dry_run").and_then(Value::as_bool) == Some(true) {
        println!("{}", serde_json::to_string_pretty(value).unwrap());
    } else if let Some(url) = value.get("url").and_then(Value::as_str) {
        println!("{url}")
    } else {
        println!("{}", serde_json::to_string_pretty(value).unwrap())
    }
}
pub async fn run(cli: &Cli) -> Result<()> {
    let cfg = if Path::new(".sss.json").exists() {
        Some(serde_json::from_slice::<Config>(&fs::read(".sss.json")?)?)
    } else {
        None
    };
    let upstream = cli
        .upstream
        .as_deref()
        .or(cfg.as_ref().map(|c| c.upstream.as_str()))
        .context("specify --upstream or create .sss.json with sss new")?;
    let url = url::Url::parse(upstream)?;
    if url.host_str().is_none()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        bail!("upstream must be an origin URL")
    };
    let loopback = url.host_str().is_some_and(|h| {
        h == "localhost"
            || h == "[::1]"
            || h.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if url.scheme() != "https" && !(url.scheme() == "http" && (loopback || cli.allow_http)) {
        bail!("HTTPS required (except loopback; --allow-http permits an explicit test origin)")
    }
    let client = Client {
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(120))
            .build()?,
        upstream: upstream.trim_end_matches('/').into(),
        key: std::env::var("SSS_API_KEY").context("set SSS_API_KEY")?,
    };
    let project = cli
        .project
        .as_deref()
        .or(cfg.as_ref().map(|c| c.project.as_str()));
    if let Some(p) = project {
        if p.is_empty() || !p.bytes().all(|c| c.is_ascii_alphanumeric()) {
            bail!("invalid project ID")
        }
    }
    let p = || project.context("specify --project or create .sss.json with sss new");
    let result=match &cli.command{
  Command::New(args)=>{if cfg.is_some(){bail!(".sss.json already exists; use another directory for a new project")};let result=client.call(Method::POST,"/api/projects",Some(json!({"basic_password":password(args)?}))).await?;let config=Config{upstream:client.upstream.clone(),project:result["project"].as_str().context("missing project ID")?.into()};let mut f=fs::OpenOptions::new().write(true).create_new(true).open(".sss.json")?;use std::io::Write;f.write_all(serde_json::to_string_pretty(&config)?.as_bytes())?;result},
  Command::Upload{files}=>{let files=collect(&std::env::current_dir()?,files,false)?;publish(&client,p()?,files,false,false).await?},
  Command::Sync{dir,dry_run}=>{let files=collect(dir,&[],true)?;publish(&client,p()?,files,true,*dry_run).await?},
  Command::Delete{files}=>{let p=p()?;if files.is_empty(){client.call(Method::DELETE,&format!("/api/projects/{p}"),None).await?}else{for f in files{if !valid_path(f){bail!("unsafe path")}}let m=client.call(Method::GET,&format!("/api/projects/{p}/manifest"),None).await?;client.call(Method::POST,&format!("/api/projects/{p}/files"),Some(json!({"mode":"delete","delete":files,"base_revision":m["revision"]}))).await?}},
  Command::Config(args)=>{if args.basic_auth.is_none() && !args.password_stdin && !args.no_basic_auth{bail!("specify --basic-auth, --password-stdin, or --no-basic-auth")};client.call(Method::PATCH,&format!("/api/projects/{}",p()?),Some(json!({"basic_password":password(args)?}))).await?},
  Command::Keys{command}=>match command{
   KeyCommand::Create{scope,expires_in}=>client.call(Method::POST,"/api/keys",Some(json!({"project":p()?,"scopes":scope.split(',').collect::<Vec<_>>(),"expires_in":expires_in}))).await?,
   KeyCommand::List=>client.call(Method::GET,"/api/keys",None).await?,
   KeyCommand::Revoke{key_id}=>{if key_id.is_empty()||!key_id.bytes().all(|c|c.is_ascii_alphanumeric()){bail!("invalid key ID")};client.call(Method::DELETE,&format!("/api/keys/{key_id}"),None).await?}
  },
  _=>unreachable!()
 };
    print(&result, cli.json);
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
    if dry || (changed.is_empty() && removed.is_empty()) {
        return Ok(
            json!({"project":p,"dry_run":dry,"changed":changed.keys().collect::<Vec<_>>(),"deleted":removed,"revision":m["revision"],"url":format!("{}/s/{p}/",c.upstream)}),
        );
    }
    let mut body =
        json!({"mode":if sync{"sync"}else{"upload"},"files":changed,"base_revision":m["revision"]});
    if sync {
        body["keep"] = json!(local.keys().collect::<Vec<_>>())
    };
    c.call(
        Method::POST,
        &format!("/api/projects/{p}/files"),
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
