use crate::{
    auth::{Access, Rule, StoredAccess, StoredRule},
    hash, id, valid_path,
};
use anyhow::{bail, Context, Result};
use axum::{
    extract::{DefaultBodyLimit, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD, Engine};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;
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
    admin: Option<StoredRule>,
    default_lifetime: Option<i64>,
}
type Shared = Arc<Mutex<Store>>;
struct ApiError(StatusCode, String);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut r = (self.0, Json(json!({"error":self.1}))).into_response();
        if self.0 == StatusCode::UNAUTHORIZED {
            r.headers_mut().insert(
                "www-authenticate",
                "Basic realm=\"sss\", charset=\"UTF-8\"".parse().unwrap(),
            );
        }
        r.headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
        r
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
fn open(root: &FsPath, url: &str, credential: Option<&str>) -> Result<Store> {
    fs::create_dir_all(root.join("projects"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    let db = Connection::open(root.join("sss.db"))?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='projects')",
        [],
        |r| r.get(0),
    )?;
    if (exists && version != 2) || (!exists && version != 0 && version != 2) {
        bail!("unsupported data format; use a new data directory (existing data was not migrated)")
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;
        CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY,name TEXT NOT NULL,revision TEXT NOT NULL,current INTEGER NOT NULL DEFAULT 0,next INTEGER NOT NULL DEFAULT 1,auth TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS versions(project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,number INTEGER NOT NULL,PRIMARY KEY(project,number)); PRAGMA user_version=2;")?;
    db.execute_batch("CREATE TABLE IF NOT EXISTS expirations(project TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE, expires_at INTEGER NOT NULL); CREATE INDEX IF NOT EXISTS expiration_time ON expirations(expires_at);")?;
    let admin = credential
        .map(|v| {
            let (u, p) = crate::auth::split(v)?;
            Rule::Basic {
                username: u.into(),
                password: p.into(),
            }
            .store()
        })
        .transpose()?;
    Ok(Store {
        db,
        root: root.into(),
        public_url: url.trim_end_matches('/').into(),
        admin,
        default_lifetime: None,
    })
}
struct Project {
    name: String,
    revision: String,
    current: u64,
    next: u64,
    access: StoredAccess,
    expires_at: Option<i64>,
}
fn project(s: &Store, p: &str) -> std::result::Result<Project, ApiError> {
    let expires_at: Option<i64> =
        s.db.query_row(
            "SELECT expires_at FROM expirations WHERE project=?1",
            [p],
            |r| r.get(0),
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    if expires_at.is_some_and(|t| t <= crate::expiry::now()) {
        return Err(missing());
    }
    let row =
        s.db.query_row(
            "SELECT name,revision,current,next,auth FROM projects WHERE id=?1",
            [p],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, u64>(3)?,
                    r.get::<_, String>(4)?,
                ))
            },
        )
        .optional()
        .map_err(anyhow::Error::from)?
        .ok_or_else(missing)?;
    Ok(Project {
        expires_at,
        name: row.0,
        revision: row.1,
        current: row.2,
        next: row.3,
        access: serde_json::from_str(&row.4).map_err(anyhow::Error::from)?,
    })
}
fn auth(
    s: &Store,
    h: &HeaderMap,
    p: Option<&str>,
    scope: &str,
) -> std::result::Result<(), ApiError> {
    let credential = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("basic"))
        .and_then(|(_, v)| STANDARD.decode(v).ok())
        .and_then(|v| String::from_utf8(v).ok());
    if s.admin
        .as_ref()
        .is_some_and(|a| a.matches(credential.as_deref()))
    {
        return Ok(());
    }
    let rule = if scope == "admin" {
        StoredRule::Inherit
    } else {
        let row = project(s, p.ok_or_else(missing)?)?;
        if scope == "view" {
            row.access.view
        } else {
            row.access.write
        }
    };
    let allowed = match rule {
        StoredRule::None => true,
        StoredRule::Inherit => s.admin.is_none(),
        ref r => r.matches(credential.as_deref()),
    };
    if allowed {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::UNAUTHORIZED,
            "authentication required; supply --basic_auth or SSS_BASIC_AUTH".into(),
        ))
    }
}
#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct NewProject {
    #[serde(default)]
    name: String,
    #[serde(default)]
    auth: Access,
    expires_in: Option<String>,
}
async fn new_project(
    State(state): State<Shared>,
    h: HeaderMap,
    Json(body): Json<NewProject>,
) -> ApiResult {
    let mut s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    let lifetime = match &body.expires_in {
        Some(v) => crate::expiry::duration(v).map_err(|_| bad("invalid expires_in"))?,
        None => s.default_lifetime,
    };
    let expires_at = crate::expiry::deadline(lifetime).map_err(|_| bad("expiration too large"))?;
    if body.name.len() > 256 {
        return Err(bad("name too long"));
    }
    let access = body
        .auth
        .store()
        .map_err(|_| bad("invalid authentication settings"))?;
    let p = id(6);
    let rev = id(12);
    let tx = s.db.transaction().map_err(anyhow::Error::from)?;
    tx.execute(
        "INSERT INTO projects(id,name,revision,auth) VALUES(?1,?2,?3,?4)",
        params![
            p,
            body.name,
            rev,
            serde_json::to_string(&access).map_err(anyhow::Error::from)?
        ],
    )
    .map_err(anyhow::Error::from)?;
    if let Some(t) = expires_at {
        tx.execute(
            "INSERT INTO expirations(project,expires_at) VALUES(?1,?2)",
            params![p, t],
        )
        .map_err(anyhow::Error::from)?;
    }
    tx.commit().map_err(anyhow::Error::from)?;
    Ok(Json(
        json!({"id":p,"url":format!("{}/s/{p}/",s.public_url),"expires_at":expires_at}),
    ))
}
async fn list_projects(State(state): State<Shared>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    let mut q =
        s.db.prepare("SELECT id,name,current,(SELECT expires_at FROM expirations WHERE project=id) FROM projects WHERE id NOT IN (SELECT project FROM expirations WHERE expires_at <= unixepoch()) ORDER BY id")
            .map_err(anyhow::Error::from)?;
    let rows=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"name":r.get::<_,String>(1)?,"version":r.get::<_,u64>(2)?,"expires_at":r.get::<_,Option<i64>>(3)?}))).map_err(anyhow::Error::from)?.collect::<std::result::Result<Vec<_>,_>>().map_err(anyhow::Error::from)?;
    Ok(Json(json!({"projects":rows})))
}
fn summary(s: &Store, p: &str, row: &Project) -> Value {
    json!({"project":p,"expires_at":row.expires_at,"name":row.name,"revision":row.revision,"version":row.current,"url":format!("{}/s/{p}/",s.public_url),"version_url":if row.current==0 {Value::Null}else{json!(format!("{}/s/{p}/versions/{}/",s.public_url,row.current))}})
}
async fn manifest(State(state): State<Shared>, Path(p): Path<String>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    let row = project(&s, &p)?;
    let mut value = summary(&s, &p, &row);
    value["files"] = json!(if row.current == 0 {
        BTreeMap::new()
    } else {
        files(
            &s.root
                .join("projects")
                .join(&p)
                .join(row.current.to_string()),
        )?
    });
    Ok(Json(value))
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
    let mut s = state.lock().unwrap();
    auth(&s, &h, Some(&p), &body.mode)?;
    let row = project(&s, &p)?;
    let old = row.revision.clone();
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
    let old_root = s
        .root
        .join("projects")
        .join(&p)
        .join(row.current.to_string());
    let mut desired = if row.current == 0 {
        BTreeMap::new()
    } else {
        files(&old_root)?
    };
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
    let version = row.next;
    let root = s.root.join("projects").join(&p).join(version.to_string());
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
        let tx = s.db.transaction()?;
        tx.execute(
            "INSERT INTO versions(project,number) VALUES(?1,?2)",
            params![p, version],
        )?;
        tx.execute(
            "UPDATE projects SET revision=?1,current=?2,next=?3 WHERE id=?4",
            params![rev, version, version + 1, p],
        )?;
        tx.commit()?;
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&root);
        return Err(e.into());
    }
    Ok(Json(
        json!({"project":p,"revision":rev,"version":version,"version_url":format!("{}/s/{p}/versions/{version}/",s.public_url),"files":desired.len(),"bytes":total,"url":format!("{}/s/{p}/",s.public_url)}),
    ))
}

async fn delete_project(
    State(state): State<Shared>,
    Path(p): Path<String>,
    h: HeaderMap,
) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    project(&s, &p)?;
    s.db.execute("DELETE FROM projects WHERE id=?1", [&p])
        .map_err(anyhow::Error::from)?;
    let root = s.root.join("projects").join(&p);
    if root.exists() {
        fs::remove_dir_all(root).map_err(anyhow::Error::from)?;
    }
    Ok(Json(json!({"deleted":p})))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchAccess {
    view: Option<Rule>,
    write: Option<Rule>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PatchProject {
    name: Option<String>,
    auth: Option<PatchAccess>,
    expires_in: Option<String>,
}
async fn config(
    State(state): State<Shared>,
    Path(p): Path<String>,
    h: HeaderMap,
    Json(body): Json<PatchProject>,
) -> ApiResult {
    let mut s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    let row = project(&s, &p)?;
    if body.name.is_none() && body.auth.is_none() && body.expires_in.is_none() {
        return Err(bad("specify name, auth, or expires_in"));
    }
    let mut access = row.access;
    if let Some(a) = body.auth {
        if let Some(v) = a.view {
            access.view = v.store().map_err(|_| bad("invalid view authentication"))?;
        }
        if let Some(v) = a.write {
            access.write = v.store().map_err(|_| bad("invalid write authentication"))?;
        }
    }
    let name = body.name.unwrap_or(row.name);
    if name.len() > 256 {
        return Err(bad("name too long"));
    }
    let expires_at = match &body.expires_in {
        None => row.expires_at,
        Some(v) => crate::expiry::deadline(
            crate::expiry::duration(v).map_err(|_| bad("invalid expires_in"))?,
        )
        .map_err(|_| bad("expiration too large"))?,
    };
    let tx = s.db.transaction().map_err(anyhow::Error::from)?;
    tx.execute(
        "UPDATE projects SET name=?1,auth=?2 WHERE id=?3",
        params![
            name,
            serde_json::to_string(&access).map_err(anyhow::Error::from)?,
            p
        ],
    )
    .map_err(anyhow::Error::from)?;
    tx.execute("DELETE FROM expirations WHERE project=?1", [&p])
        .map_err(anyhow::Error::from)?;
    if let Some(t) = expires_at {
        tx.execute(
            "INSERT INTO expirations(project,expires_at) VALUES(?1,?2)",
            params![p, t],
        )
        .map_err(anyhow::Error::from)?;
    }
    tx.commit().map_err(anyhow::Error::from)?;
    Ok(Json(
        json!({"project":p,"updated":true,"expires_at":expires_at}),
    ))
}
fn access_summary(rule: &StoredRule, admin: bool) -> Value {
    match rule {
        StoredRule::Inherit => json!({"mode":"inherit","authentication_required":admin}),
        StoredRule::None => json!({"mode":"none","authentication_required":false}),
        StoredRule::Basic { username, .. } => {
            json!({"mode":"basic","username":username,"authentication_required":true})
        }
    }
}
async fn info(State(state): State<Shared>, Path(p): Path<String>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    let row = project(&s, &p)?;
    let mut result = summary(&s, &p, &row);
    let mut total_bytes = 0u64;
    let mut current_bytes = 0u64;
    let mut current_files = 0u64;
    let root = s.root.join("projects").join(&p);
    if root.exists() {
        for entry in walkdir::WalkDir::new(&root).follow_links(false) {
            let e = entry.map_err(anyhow::Error::from)?;
            if e.file_type().is_file() {
                let size = e.metadata().map_err(anyhow::Error::from)?.len();
                total_bytes += size;
                if e.path().starts_with(root.join(row.current.to_string())) {
                    current_bytes += size;
                    current_files += 1;
                }
            }
        }
    }
    result["files"] = json!(current_files);
    result["bytes"] = json!(current_bytes);
    result["storage_bytes"] = json!(total_bytes);
    result["auth"] = json!({"view":access_summary(&row.access.view,s.admin.is_some()),"write":access_summary(&row.access.write,s.admin.is_some())});
    Ok(Json(result))
}
async fn status(State(state): State<Shared>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    Ok(Json(
        json!({"version":env!("CARGO_PKG_VERSION"),"authentication_required":s.admin.is_some(),"default_expires_in_seconds":s.default_lifetime}),
    ))
}
async fn snapshot_file(
    State(state): State<Shared>,
    Path((p, n, path)): Path<(String, u64, String)>,
    h: HeaderMap,
) -> Result<Response, ApiError> {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    project(&s, &p)?;
    has_version(&s, &p, n)?;
    if !valid_path(&path) {
        return Err(bad("unsafe path"));
    }
    let root = s.root.join("projects").join(&p).join(n.to_string());
    let mut full = root;
    for part in path.split('/') {
        full.push(part);
        if fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err(missing());
        }
    }
    let bytes = fs::read(full).map_err(|_| missing())?;
    Ok((
        [
            ("content-type", "application/octet-stream"),
            ("cache-control", "no-store"),
        ],
        bytes,
    )
        .into_response())
}
fn purge_expired(s: &Store) -> Result<()> {
    let mut q =
        s.db.prepare("SELECT project FROM expirations WHERE expires_at <= ?1")?;
    let ids = q
        .query_map([crate::expiry::now()], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for p in ids {
        let result = (|| -> Result<()> {
            let root = s.root.join("projects").join(&p);
            if root.exists() {
                fs::remove_dir_all(root)?;
            }
            s.db.execute("DELETE FROM projects WHERE id=?1", [&p])?;
            Ok(())
        })();
        if let Err(e) = result {
            eprintln!("expiry cleanup failed for {p}; will retry: {e}");
        }
    }
    Ok(())
}
async fn versions(State(state): State<Shared>, Path(p): Path<String>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    let row = project(&s, &p)?;
    let mut q =
        s.db.prepare("SELECT number FROM versions WHERE project=?1 ORDER BY number")
            .map_err(anyhow::Error::from)?;
    let nums = q
        .query_map([&p], |r| r.get::<_, u64>(0))
        .map_err(anyhow::Error::from)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(anyhow::Error::from)?;
    Ok(Json(
        json!({"project":p,"current":row.current,"versions":nums}),
    ))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectVersion {
    version: u64,
    base_revision: String,
}
fn has_version(s: &Store, p: &str, n: u64) -> std::result::Result<(), ApiError> {
    let exists: bool =
        s.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM versions WHERE project=?1 AND number=?2)",
            params![p, n],
            |r| r.get(0),
        )
        .map_err(anyhow::Error::from)?;
    if exists {
        Ok(())
    } else {
        Err(missing())
    }
}
async fn rollback(
    State(state): State<Shared>,
    Path(p): Path<String>,
    h: HeaderMap,
    Json(body): Json<SelectVersion>,
) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    let row = project(&s, &p)?;
    if row.revision != body.base_revision {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "revision changed; retry operation".into(),
        ));
    }
    has_version(&s, &p, body.version)?;
    s.db.execute(
        "UPDATE projects SET current=?1,revision=?2 WHERE id=?3",
        params![body.version, id(12), p],
    )
    .map_err(anyhow::Error::from)?;
    Ok(Json(summary(&s, &p, &project(&s, &p)?)))
}
async fn delete_version(
    State(state): State<Shared>,
    Path((p, n)): Path<(String, u64)>,
    h: HeaderMap,
) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    let row = project(&s, &p)?;
    has_version(&s, &p, n)?;
    if row.current == n {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "cannot delete the current version".into(),
        ));
    }
    s.db.execute(
        "DELETE FROM versions WHERE project=?1 AND number=?2",
        params![p, n],
    )
    .map_err(anyhow::Error::from)?;
    fs::remove_dir_all(s.root.join("projects").join(&p).join(n.to_string()))
        .map_err(anyhow::Error::from)?;
    Ok(Json(json!({"project":p,"deleted_version":n})))
}
async fn static_root(State(s): State<Shared>, Path(p): Path<String>, h: HeaderMap) -> Response {
    static_file(s, p, "".into(), h)
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
    if let Err(e) = auth(&s, &h, Some(&p), "view") {
        return e.into_response();
    }
    let row = match project(&s, &p) {
        Ok(r) => r,
        Err(e) => return e.into_response(),
    };
    let (n, mut path) = if let Some(rest) = path.strip_prefix("versions/") {
        let Some((version, path)) = rest.split_once('/') else {
            return missing().into_response();
        };
        let Ok(n) = version.parse::<u64>() else {
            return missing().into_response();
        };
        if n.to_string() != version || has_version(&s, &p, n).is_err() {
            return missing().into_response();
        }
        (n, path.to_owned())
    } else {
        (row.current, path)
    };
    if path.is_empty() || path.ends_with('/') {
        path.push_str("index.html")
    }
    if n == 0 || !valid_path(&path) {
        return missing().into_response();
    }
    let root = s.root.join("projects").join(&p).join(n.to_string());
    let mut full = root.clone();
    for part in path.split('/') {
        full.push(part);
        if fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink()) {
            return missing().into_response();
        }
    }
    let Ok(data) = fs::read(&full) else {
        return missing().into_response();
    };
    (
        [
            (
                "content-type",
                mime_guess::from_path(&path)
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
pub async fn serve(
    listen: std::net::SocketAddr,
    root: &FsPath,
    url: &str,
    credential: Option<&str>,
    default_expires_in: &str,
) -> Result<()> {
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
    let mut store = open(root, url, credential)?;
    store.default_lifetime = crate::expiry::duration(default_expires_in)?;
    purge_expired(&store)?;
    // Only remove uncommitted or deleted versions; retained snapshots survive restarts.
    for entry in fs::read_dir(root.join("projects"))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            bail!("unexpected entry in project storage")
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        for release in fs::read_dir(entry.path())? {
            let release = release?;
            if !release.file_type()?.is_dir() {
                bail!("unexpected version entry")
            }
            let keep = release
                .file_name()
                .to_str()
                .and_then(|n| n.parse::<u64>().ok())
                .is_some_and(|n| has_version(&store, &name, n).is_ok());
            if !keep {
                fs::remove_dir_all(release.path())?
            }
        }
        let exists: bool = store.db.query_row(
            "SELECT EXISTS(SELECT 1 FROM projects WHERE id=?1)",
            [&name],
            |r| r.get(0),
        )?;
        if !exists {
            fs::remove_dir(entry.path())?
        }
    }
    let state = Arc::new(Mutex::new(store));
    let cleanup = state.clone();
    let janitor = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = purge_expired(&cleanup.lock().unwrap()) {
                eprintln!("expiry cleanup failed: {e}");
            }
        }
    });
    let permits = Arc::new(tokio::sync::Semaphore::new(2));
    let app = Router::new()
        .route(
            "/health",
            get(|| async { Json(json!({"status":"ok","version":env!("CARGO_PKG_VERSION")})) }),
        )
        .route("/api/status", get(status))
        .route("/api/projects", post(new_project).get(list_projects))
        .route(
            "/api/projects/{p}",
            get(info).patch(config).delete(delete_project),
        )
        .route("/api/projects/{p}/manifest", get(manifest))
        .route("/api/projects/{p}/versions", get(versions).post(update))
        .route(
            "/api/projects/{p}/versions/{n}",
            axum::routing::delete(delete_version),
        )
        .route(
            "/api/projects/{p}/versions/{n}/files/{*path}",
            get(snapshot_file),
        )
        .route("/api/projects/{p}/current", axum::routing::put(rollback))
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
    janitor.abort();
    Ok(())
}
