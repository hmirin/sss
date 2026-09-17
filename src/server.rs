use crate::{hash, id, now, valid_path};
use anyhow::{bail, Context, Result};
use argon2::{
    password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
};
const MAX_BYTES: usize = 50 * 1024 * 1024;
const MAX_FILES: usize = 5000;
struct Store {
    db: Connection,
    root: PathBuf,
    public_url: String,
}
type Shared = Arc<Mutex<Store>>;
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(json!({"error":self.1}))).into_response()
    }
}
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        eprintln!("server error: {e:#}");
        Self(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal server error".into(),
        )
    }
}
type ApiResult = std::result::Result<Json<Value>, ApiError>;
fn bad(s: &str) -> ApiError {
    ApiError(StatusCode::BAD_REQUEST, s.into())
}
fn missing() -> ApiError {
    ApiError(StatusCode::NOT_FOUND, "not found".into())
}
fn open(root: &FsPath, url: &str) -> Result<Store> {
    fs::create_dir_all(root.join("projects"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    let db = Connection::open(root.join("sss.db"))?;
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY,revision TEXT NOT NULL,password TEXT); CREATE TABLE IF NOT EXISTS keys(id TEXT PRIMARY KEY,hash TEXT NOT NULL UNIQUE,project TEXT,scopes TEXT NOT NULL,expires INTEGER,admin INTEGER NOT NULL DEFAULT 0);")?;
    Ok(Store {
        db,
        root: root.to_owned(),
        public_url: url.trim_end_matches('/').into(),
    })
}
pub fn admin_key(root: &FsPath, json_output: bool) -> Result<()> {
    let s = open(root, "")?;
    let token = format!("sss_{}", id(32));
    let key_id = id(8);
    s.db.execute(
        "INSERT INTO keys(id,hash,scopes,admin) VALUES(?1,?2,'*',1)",
        params![key_id, hash(token.as_bytes())],
    )?;
    if json_output {
        println!("{}", json!({"id":key_id,"key":token}));
    } else {
        println!("{token}");
    }
    Ok(())
}
fn auth(
    s: &Store,
    h: &HeaderMap,
    project: Option<&str>,
    scope: &str,
) -> std::result::Result<bool, ApiError> {
    let token = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| ApiError(StatusCode::UNAUTHORIZED, "API key required".into()))?;
    let row =
        s.db.query_row(
            "SELECT project,scopes,expires,admin FROM keys WHERE hash=?1",
            [hash(token.as_bytes())],
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<i64>>(2)?,
                    r.get::<_, bool>(3)?,
                ))
            },
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    let Some((p, scopes, expires, admin)) = row else {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "invalid API key".into()));
    };
    if expires.is_some_and(|e| e <= now()) {
        return Err(ApiError(StatusCode::UNAUTHORIZED, "expired API key".into()));
    }
    if admin {
        return Ok(true);
    }
    if scope == "admin"
        || p.as_deref() != project
        || !scopes
            .split(',')
            .any(|v| v == scope || (scope == "read" && matches!(v, "upload" | "sync" | "delete")))
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "key scope does not permit operation".into(),
        ));
    }
    Ok(false)
}
fn project(s: &Store, p: &str) -> std::result::Result<(String, Option<String>), ApiError> {
    s.db.query_row(
        "SELECT revision,password FROM projects WHERE id=?1",
        [p],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
    .map_err(anyhow::Error::from)?
    .ok_or_else(missing)
}
fn password_hash(p: &str) -> Result<String> {
    if p.is_empty() || p.len() > 4096 {
        bail!("password must contain 1..4096 bytes")
    };
    let salt = SaltString::generate(&mut rand::rngs::OsRng);
    Argon2::default()
        .hash_password(p.as_bytes(), &salt)
        .map(|x| x.to_string())
        .map_err(|e| anyhow::anyhow!("{e}"))
}
#[derive(Deserialize)]
struct NewProject {
    basic_password: Option<String>,
}
async fn new_project(
    State(state): State<Shared>,
    h: HeaderMap,
    Json(body): Json<NewProject>,
) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    let password = body
        .basic_password
        .as_deref()
        .map(password_hash)
        .transpose()
        .map_err(|_| bad("invalid password"))?;
    let p = id(6);
    let rev = id(12);
    fs::create_dir_all(s.root.join("projects").join(&p).join(&rev)).map_err(anyhow::Error::from)?;
    s.db.execute(
        "INSERT INTO projects VALUES(?1,?2,?3)",
        params![p, rev, password],
    )
    .map_err(anyhow::Error::from)?;
    Ok(Json(
        json!({"project":p,"url":format!("{}/s/{p}/",s.public_url),"revision":rev}),
    ))
}
fn files(root: &FsPath) -> Result<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    for e in walkdir::WalkDir::new(root).follow_links(false) {
        let e = e?;
        if e.file_type().is_symlink() {
            bail!("symlink in storage")
        };
        if e.file_type().is_file() {
            let path = e
                .path()
                .strip_prefix(root)?
                .to_str()
                .context("non UTF-8 path")?
                .replace('\\', "/");
            result.insert(path, hash(&fs::read(e.path())?));
        }
    }
    Ok(result)
}
async fn manifest(State(state): State<Shared>, Path(p): Path<String>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "read")?;
    let (rev, _) = project(&s, &p)?;
    Ok(Json(
        json!({"revision":rev,"files":files(&s.root.join("projects").join(&p).join(&rev))?}),
    ))
}
#[derive(Deserialize)]
struct Update {
    mode: String,
    #[serde(default)]
    files: BTreeMap<String, String>,
    keep: Option<Vec<String>>,
    delete: Option<Vec<String>>,
    base_revision: Option<String>,
}
async fn update(
    State(state): State<Shared>,
    Path(p): Path<String>,
    h: HeaderMap,
    Json(body): Json<Update>,
) -> ApiResult {
    if !matches!(body.mode.as_str(), "upload" | "sync" | "delete") {
        return Err(bad("invalid mode"));
    }
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), &body.mode)?;
    let (old, _) = project(&s, &p)?;
    if body.base_revision.as_deref() != Some(&old) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "revision changed; retry operation".into(),
        ));
    }
    if body.files.len() > MAX_FILES
        || body.keep.as_ref().is_some_and(|x| x.len() > MAX_FILES)
        || body.delete.as_ref().is_some_and(|x| x.len() > MAX_FILES)
    {
        return Err(bad("too many files"));
    }
    for path in body
        .files
        .keys()
        .chain(body.keep.iter().flatten())
        .chain(body.delete.iter().flatten())
    {
        if !valid_path(path) {
            return Err(bad("unsafe or excluded path"));
        }
    }
    if body.delete.is_some() && body.mode != "delete" {
        return Err(bad("delete only allowed in delete mode"));
    }
    if body.mode == "sync" && body.keep.is_none() {
        return Err(bad("sync requires keep manifest"));
    }
    if body.mode == "delete" && (!body.files.is_empty() || body.keep.is_some()) {
        return Err(bad("invalid delete request"));
    }
    let old_root = s.root.join("projects").join(&p).join(&old);
    let mut desired = files(&old_root)?;
    if let Some(keep) = &body.keep {
        if body.mode != "sync" {
            return Err(bad("keep only allowed in sync"));
        };
        desired.retain(|k, _| keep.contains(k));
    }
    if let Some(del) = &body.delete {
        for k in del {
            desired.remove(k);
        }
    }
    let mut decoded = BTreeMap::new();
    let mut total = 0usize;
    for (k, v) in body.files {
        let bytes = STANDARD.decode(v).map_err(|_| bad("invalid base64"))?;
        total = total
            .checked_add(bytes.len())
            .ok_or_else(|| bad("quota exceeded"))?;
        if total > MAX_BYTES {
            return Err(bad("quota exceeded"));
        };
        desired.insert(k.clone(), hash(&bytes));
        decoded.insert(k, bytes);
    }
    if let Some(keep) = &body.keep {
        if keep.iter().any(|k| !desired.contains_key(k))
            || desired.keys().any(|k| !keep.contains(k))
        {
            return Err(bad("sync manifest must match resulting files"));
        }
    }
    if desired.len() > MAX_FILES {
        return Err(bad("too many files"));
    }
    for k in desired.keys().filter(|k| !decoded.contains_key(*k)) {
        total += fs::metadata(old_root.join(k))
            .map_err(anyhow::Error::from)?
            .len() as usize;
    }
    if total > MAX_BYTES {
        return Err(bad("project quota exceeded"));
    }
    let rev = id(12);
    let root = s.root.join("projects").join(&p).join(&rev);
    let result = (|| -> Result<()> {
        fs::create_dir_all(&root)?;
        for k in desired.keys() {
            let dest = root.join(k);
            fs::create_dir_all(dest.parent().unwrap())?;
            if let Some(bytes) = decoded.get(k) {
                fs::write(dest, bytes)?
            } else {
                fs::copy(old_root.join(k), dest)?;
            }
        }
        s.db.execute(
            "UPDATE projects SET revision=?1 WHERE id=?2",
            params![rev, p],
        )?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&root);
        return Err(e.into());
    }
    // All reads also hold the store lock; the previous release is no longer in use.
    if let Err(e) = fs::remove_dir_all(old_root) {
        eprintln!("old release cleanup deferred: {e}");
    }
    Ok(Json(
        json!({"project":p,"revision":rev,"files":desired.len(),"bytes":total,"url":format!("{}/s/{p}/",s.public_url)}),
    ))
}
async fn delete_project(
    State(state): State<Shared>,
    Path(p): Path<String>,
    h: HeaderMap,
) -> ApiResult {
    let mut s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "delete")?;
    project(&s, &p)?;
    let tx = s.db.transaction().map_err(anyhow::Error::from)?;
    tx.execute("DELETE FROM keys WHERE project=?1", [&p])
        .map_err(anyhow::Error::from)?;
    tx.execute("DELETE FROM projects WHERE id=?1", [&p])
        .map_err(anyhow::Error::from)?;
    tx.commit().map_err(anyhow::Error::from)?;
    fs::remove_dir_all(s.root.join("projects").join(&p)).map_err(anyhow::Error::from)?;
    Ok(Json(json!({"deleted":p})))
}
async fn config(
    State(state): State<Shared>,
    Path(p): Path<String>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "config")?;
    project(&s, &p)?;
    let v = body
        .get("basic_password")
        .ok_or_else(|| bad("basic_password is required"))?;
    let pass = if v.is_null() {
        None
    } else {
        Some(
            password_hash(v.as_str().ok_or_else(|| bad("password must be a string"))?)
                .map_err(|_| bad("invalid password"))?,
        )
    };
    s.db.execute(
        "UPDATE projects SET password=?1 WHERE id=?2",
        params![pass, p],
    )
    .map_err(anyhow::Error::from)?;
    Ok(Json(json!({"project":p,"basic_auth":pass.is_some()})))
}
#[derive(Deserialize)]
struct KeyInput {
    project: Option<String>,
    scopes: Vec<String>,
    expires_in: Option<u64>,
}
async fn create_key(
    State(state): State<Shared>,
    h: HeaderMap,
    Json(body): Json<KeyInput>,
) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    let p = body
        .project
        .ok_or_else(|| bad("project required for scoped keys"))?;
    project(&s, &p)?;
    if body.scopes.is_empty()
        || body
            .scopes
            .iter()
            .any(|x| !matches!(x.as_str(), "upload" | "sync" | "delete" | "config"))
    {
        return Err(bad("invalid scopes"));
    }
    let expires = body
        .expires_in
        .map(|n| {
            i64::try_from(n)
                .ok()
                .and_then(|n| now().checked_add(n))
                .ok_or_else(|| bad("invalid expiry"))
        })
        .transpose()?;
    let token = format!("sss_{}", id(32));
    let key_id = id(8);
    s.db.execute(
        "INSERT INTO keys(id,hash,project,scopes,expires) VALUES(?1,?2,?3,?4,?5)",
        params![
            key_id,
            hash(token.as_bytes()),
            p,
            body.scopes.join(","),
            expires
        ],
    )
    .map_err(anyhow::Error::from)?;
    Ok(Json(
        json!({"id":key_id,"key":token,"project":p,"scopes":body.scopes,"expires":expires}),
    ))
}
#[derive(Serialize)]
struct KeyInfo {
    id: String,
    project: Option<String>,
    scopes: String,
    expires: Option<i64>,
    admin: bool,
}
async fn list_keys(State(state): State<Shared>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    let mut q =
        s.db.prepare("SELECT id,project,scopes,expires,admin FROM keys ORDER BY id")
            .map_err(anyhow::Error::from)?;
    let rows = q
        .query_map([], |r| {
            Ok(KeyInfo {
                id: r.get(0)?,
                project: r.get(1)?,
                scopes: r.get(2)?,
                expires: r.get(3)?,
                admin: r.get(4)?,
            })
        })
        .map_err(anyhow::Error::from)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(anyhow::Error::from)?;
    Ok(Json(json!({"keys":rows})))
}
async fn revoke_key(State(state): State<Shared>, Path(k): Path<String>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    let n =
        s.db.execute("DELETE FROM keys WHERE id=?1", [&k])
            .map_err(anyhow::Error::from)?;
    if n == 0 {
        return Err(missing());
    };
    Ok(Json(json!({"revoked":k})))
}
async fn static_root(State(s): State<Shared>, Path(p): Path<String>, h: HeaderMap) -> Response {
    static_file(s, p, "index.html".into(), h)
}
async fn static_asset(
    State(s): State<Shared>,
    Path((p, path)): Path<(String, String)>,
    h: HeaderMap,
) -> Response {
    static_file(s, p, path, h)
}
fn static_file(state: Shared, p: String, path: String, h: HeaderMap) -> Response {
    let s = state.lock().unwrap();
    let Ok((rev, password)) = project(&s, &p) else {
        return missing().into_response();
    };
    if let Some(password) = password {
        let good = h
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Basic "))
            .and_then(|v| STANDARD.decode(v).ok())
            .and_then(|v| String::from_utf8(v).ok())
            .and_then(|v| v.strip_prefix("sss:").map(str::to_owned))
            .is_some_and(|v| {
                PasswordHash::new(&password)
                    .is_ok_and(|ph| Argon2::default().verify_password(v.as_bytes(), &ph).is_ok())
            });
        if !good {
            return (
                StatusCode::UNAUTHORIZED,
                [
                    (
                        "www-authenticate",
                        format!("Basic realm=\"sss-{p}\", charset=\"UTF-8\""),
                    ),
                    ("cache-control", "no-store".into()),
                ],
                "Authentication required",
            )
                .into_response();
        }
    }
    let path = if path.ends_with('/') {
        format!("{path}index.html")
    } else {
        path
    };
    if !valid_path(&path) {
        return missing().into_response();
    }
    let root = s.root.join("projects").join(&p).join(rev);
    let full = root.join(&path);
    let Ok(meta) = fs::symlink_metadata(&full) else {
        return missing().into_response();
    };
    if !meta.is_file() || meta.file_type().is_symlink() {
        return missing().into_response();
    }
    let Ok(data) = fs::read(&full) else {
        return missing().into_response();
    };
    (
        [
            (
                "content-type",
                mime_guess::from_path(path)
                    .first_or_octet_stream()
                    .to_string(),
            ),
            ("cache-control", "no-store".into()),
            ("x-content-type-options", "nosniff".into()),
            (
                "content-security-policy",
                "frame-ancestors 'self'; worker-src 'none'".into(),
            ),
            ("referrer-policy", "no-referrer".into()),
        ],
        data,
    )
        .into_response()
}
pub async fn serve(listen: &str, root: &FsPath, url: &str) -> Result<()> {
    let parsed = url::Url::parse(url)?;
    if !matches!(parsed.scheme(), "http" | "https")
        || parsed.host_str().is_none()
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
    {
        bail!("public-url must be an HTTP(S) origin")
    }
    fs::create_dir_all(root)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("serve.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("another sss server is using this data directory")?;
    let store = open(root, url)?;
    // A failed publication can leave a release directory without an active pointer.
    for entry in fs::read_dir(root.join("projects"))? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() || !entry.file_type()?.is_dir() {
            bail!("unexpected entry in project storage");
        }
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| anyhow::anyhow!("invalid storage name"))?;
        let active = project(&store, &name).ok().map(|x| x.0);
        for release in fs::read_dir(entry.path())? {
            let release = release?;
            if release.file_type()?.is_symlink() || !release.file_type()?.is_dir() {
                bail!("unexpected release entry");
            }
            if active.as_deref() != release.file_name().to_str() {
                fs::remove_dir_all(release.path())?;
            }
        }
        if active.is_none() {
            fs::remove_dir(entry.path())?;
        }
    }
    let state = Arc::new(Mutex::new(store));
    let permits = Arc::new(tokio::sync::Semaphore::new(2));
    let app = Router::new()
        .route("/health", get(|| async { Json(json!({"status":"ok"})) }))
        .route("/api/projects", post(new_project))
        .route(
            "/api/projects/{p}",
            axum::routing::delete(delete_project).patch(config),
        )
        .route("/api/projects/{p}/manifest", get(manifest))
        .route("/api/projects/{p}/files", post(update))
        .route("/api/keys", post(create_key).get(list_keys))
        .route("/api/keys/{k}", axum::routing::delete(revoke_key))
        .route("/s/{p}/", get(static_root))
        .route("/s/{p}/{*path}", get(static_asset))
        .layer(DefaultBodyLimit::max(72 * 1024 * 1024))
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let permits = permits.clone();
                async move {
                    let Ok(_permit) = permits.try_acquire_owned() else {
                        return (StatusCode::SERVICE_UNAVAILABLE, "server busy; retry")
                            .into_response();
                    };
                    next.run(request).await
                }
            },
        ))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(listen).await?;
    eprintln!("sss listening on {}", listener.local_addr()?);
    axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
