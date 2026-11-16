use crate::{
    auth::{Access, Rule, StoredAccess, StoredRule},
    expiry::ExpiryAction,
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
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
};
const MAX_BYTES: usize = 50 * 1024 * 1024;
const MAX_FILES: usize = 5000;
const DEFAULT_PROJECT_MAX_STORAGE: i64 = 500 * 1024 * 1024;
const VERSION_CHARGE: u64 = 64 * 1024;
const ENTRY_CHARGE: u64 = 4 * 1024;
const PROJECT_CHARGE: u64 = ENTRY_CHARGE;
struct Store {
    db: Connection,
    root: PathBuf,
    public_url: String,
    admin: Option<StoredRule>,
    default_lifetime: Option<i64>,
    default_on_expire: ExpiryAction,
    max_total_storage: u64,
    quota_used: u64,
    project_quota_used: BTreeMap<String, u64>,
    quota_poisoned: bool,
}
type Shared = Arc<Mutex<Store>>;
#[derive(Debug)]
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
fn storage_size(value: &str) -> Result<u64> {
    let (number, multiplier) = [
        ("GiB", 1024u64.pow(3)),
        ("MiB", 1024u64.pow(2)),
        ("KiB", 1024),
        ("B", 1),
    ]
    .into_iter()
    .find_map(|(suffix, multiplier)| value.strip_suffix(suffix).map(|n| (n, multiplier)))
    .context("storage size must end in B, KiB, MiB, or GiB")?;
    if number.is_empty() || !number.bytes().all(|c| c.is_ascii_digit()) {
        bail!("storage size must be a positive whole number")
    }
    let bytes = number
        .parse::<u64>()?
        .checked_mul(multiplier)
        .context("storage size is too large")?;
    if bytes == 0 || bytes > i64::MAX as u64 {
        bail!("storage size must be between 1 and i64::MAX bytes")
    }
    Ok(bytes)
}
fn open(root: &FsPath, url: &str, credential: Option<&str>) -> Result<Store> {
    fs::create_dir_all(root.join("projects"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    }
    let mut db = Connection::open(root.join("sss.db"))?;
    let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let exists: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='projects')",
        [],
        |r| r.get(0),
    )?;
    if (exists && !matches!(version, 2..=4)) || (!exists && version != 0) {
        bail!(
            "unsupported data format; expected format 2, 3, or 4 (existing data was not migrated)"
        )
    }
    db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;
    let tx = db.transaction()?;
    if exists && version == 2 {
        tx.execute_batch("ALTER TABLE projects ADD COLUMN max_storage INTEGER NOT NULL DEFAULT 524288000 CHECK(max_storage>0);")?;
    }
    if exists && version < 4 {
        tx.execute_batch("ALTER TABLE projects ADD COLUMN on_expire TEXT NOT NULL DEFAULT 'delete' CHECK(on_expire IN ('delete','retain'));")?;
    }
    tx.execute_batch("CREATE TABLE IF NOT EXISTS projects(id TEXT PRIMARY KEY,name TEXT NOT NULL,revision TEXT NOT NULL,current INTEGER NOT NULL DEFAULT 0,next INTEGER NOT NULL DEFAULT 1,auth TEXT NOT NULL,max_storage INTEGER NOT NULL DEFAULT 524288000 CHECK(max_storage>0),on_expire TEXT NOT NULL DEFAULT 'delete' CHECK(on_expire IN ('delete','retain')));
        CREATE TABLE IF NOT EXISTS versions(project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,number INTEGER NOT NULL,PRIMARY KEY(project,number));
        CREATE TABLE IF NOT EXISTS expiry(project TEXT PRIMARY KEY REFERENCES projects(id) ON DELETE CASCADE,lifetime INTEGER NOT NULL CHECK(lifetime>0),expires_at INTEGER NOT NULL);
        CREATE INDEX IF NOT EXISTS expiry_time ON expiry(expires_at);
        DROP TABLE IF EXISTS expirations;
        PRAGMA user_version=4;")?;
    tx.commit()?;
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
        default_on_expire: ExpiryAction::default(),
        max_total_storage: 0,
        quota_used: 0,
        project_quota_used: BTreeMap::new(),
        quota_poisoned: false,
    })
}
#[derive(Clone, Copy)]
struct Expiry {
    lifetime: i64,
    expires_at: i64,
}
struct Project {
    name: String,
    revision: String,
    current: u64,
    next: u64,
    access: StoredAccess,
    expiry: Option<Expiry>,
    max_storage: u64,
    on_expire: ExpiryAction,
}
fn expiry_for(lifetime: Option<i64>) -> Result<Option<Expiry>> {
    let Some(lifetime) = lifetime else {
        return Ok(None);
    };
    Ok(Some(Expiry {
        lifetime,
        expires_at: crate::expiry::deadline(lifetime)?,
    }))
}
fn expiry_row(r: &Row) -> rusqlite::Result<Expiry> {
    Ok(Expiry {
        lifetime: r.get(0)?,
        expires_at: r.get(1)?,
    })
}
fn renew_expiry(db: &Connection, p: &str) -> Result<Option<Expiry>> {
    Ok(db
        .query_row(
            "UPDATE expiry SET expires_at=?1+lifetime WHERE project=?2 RETURNING lifetime,expires_at",
            params![crate::expiry::now(), p],
            expiry_row,
        )
        .optional()?)
}
fn project(s: &Store, p: &str) -> std::result::Result<Project, ApiError> {
    let expiry: Option<Expiry> =
        s.db.query_row(
            "SELECT lifetime,expires_at FROM expiry WHERE project=?1",
            [p],
            expiry_row,
        )
        .optional()
        .map_err(anyhow::Error::from)?;
    if expiry.is_some_and(|e| e.expires_at <= crate::expiry::now()) {
        return Err(missing());
    }
    let row =
        s.db.query_row(
            "SELECT name,revision,current,next,auth,max_storage,on_expire FROM projects WHERE id=?1",
            [p],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, u64>(2)?,
                    r.get::<_, u64>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, u64>(5)?,
                    r.get::<_, ExpiryAction>(6)?,
                ))
            },
        )
        .optional()
        .map_err(anyhow::Error::from)?
        .ok_or_else(missing)?;
    Ok(Project {
        expiry,
        name: row.0,
        revision: row.1,
        current: row.2,
        next: row.3,
        access: serde_json::from_str(&row.4).map_err(anyhow::Error::from)?,
        max_storage: row.5,
        on_expire: row.6,
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
    max_storage: Option<String>,
    on_expire: Option<ExpiryAction>,
}
async fn new_project(
    State(state): State<Shared>,
    h: HeaderMap,
    Json(body): Json<NewProject>,
) -> ApiResult {
    let mut s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    if s.quota_poisoned {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage quota needs recovery; restart server".into(),
        ));
    }
    let next_total = s
        .quota_used
        .checked_add(PROJECT_CHARGE)
        .ok_or_else(|| bad("server storage quota exceeded"))?;
    if next_total > s.max_total_storage {
        return Err(ApiError(
            StatusCode::INSUFFICIENT_STORAGE,
            "server storage quota exceeded".into(),
        ));
    }
    let lifetime = match &body.expires_in {
        Some(v) => crate::expiry::duration(v).map_err(|_| bad("invalid expires_in"))?,
        None => s.default_lifetime,
    };
    let expiry = expiry_for(lifetime).map_err(|_| bad("expiration too large"))?;
    let on_expire = body.on_expire.unwrap_or(s.default_on_expire);
    let max_storage = body
        .max_storage
        .as_deref()
        .map(storage_size)
        .transpose()
        .map_err(|_| bad("invalid max_storage"))?
        .unwrap_or(DEFAULT_PROJECT_MAX_STORAGE as u64);
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
        "INSERT INTO projects(id,name,revision,auth,max_storage,on_expire) VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            p,
            body.name,
            rev,
            serde_json::to_string(&access).map_err(anyhow::Error::from)?,
            max_storage,
            on_expire,
        ],
    )
    .map_err(anyhow::Error::from)?;
    if let Some(expiry) = expiry {
        tx.execute(
            "INSERT INTO expiry(project,lifetime,expires_at) VALUES(?1,?2,?3)",
            params![p, expiry.lifetime, expiry.expires_at],
        )
        .map_err(anyhow::Error::from)?;
    }
    tx.commit().map_err(anyhow::Error::from)?;
    s.quota_used = next_total;
    s.project_quota_used.insert(p.clone(), PROJECT_CHARGE);
    Ok(Json(
        json!({"id":p,"url":format!("{}/s/{p}/",s.public_url),"expires_at":expiry.map(|e|e.expires_at),"expires_in_seconds":expiry.map(|e|e.lifetime),"max_storage_bytes":max_storage,"on_expire":on_expire}),
    ))
}
async fn list_projects(State(state): State<Shared>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    let mut q = s
        .db
        .prepare("SELECT id,name,current,(SELECT lifetime FROM expiry WHERE project=id),(SELECT expires_at FROM expiry WHERE project=id),on_expire FROM projects WHERE id NOT IN (SELECT project FROM expiry WHERE expires_at <= unixepoch()) ORDER BY id")
        .map_err(anyhow::Error::from)?;
    let rows = q
        .query_map([], |r| {
            Ok(json!({
                "id":r.get::<_,String>(0)?,
                "name":r.get::<_,String>(1)?,
                "version":r.get::<_,u64>(2)?,
                "expires_in_seconds":r.get::<_,Option<i64>>(3)?,
                "expires_at":r.get::<_,Option<i64>>(4)?,
                "on_expire":r.get::<_,ExpiryAction>(5)?
            }))
        })
        .map_err(anyhow::Error::from)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(anyhow::Error::from)?;
    Ok(Json(json!({"projects":rows})))
}
fn summary(s: &Store, p: &str, row: &Project) -> Value {
    json!({"project":p,"expires_at":row.expiry.map(|e|e.expires_at),"expires_in_seconds":row.expiry.map(|e|e.lifetime),"max_storage_bytes":row.max_storage,"on_expire":row.on_expire,"name":row.name,"revision":row.revision,"version":row.current,"url":format!("{}/s/{p}/",s.public_url),"version_url":if row.current==0 {Value::Null}else{json!(format!("{}/s/{p}/versions/{}/",s.public_url,row.current))}})
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
fn charged_file_size(size: u64) -> Result<u64> {
    let blocks = size
        .checked_add(ENTRY_CHARGE - 1)
        .context("stored size overflow")?
        / ENTRY_CHARGE;
    blocks
        .checked_mul(ENTRY_CHARGE)
        .context("stored size overflow")?
        .checked_add(ENTRY_CHARGE)
        .context("stored size overflow")
}
fn quota_used(root: &FsPath, version_depth: usize) -> Result<u64> {
    if !root.exists() {
        return Ok(0);
    }
    let mut total = 0u64;
    for entry in walkdir::WalkDir::new(root).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            bail!("symlink in storage")
        }
        let charge = if entry.depth() == 0 {
            0
        } else if entry.file_type().is_dir() && entry.depth() == version_depth {
            VERSION_CHARGE
        } else if entry.file_type().is_dir() {
            ENTRY_CHARGE
        } else if entry.file_type().is_file() {
            charged_file_size(entry.metadata()?.len())?
        } else {
            bail!("unexpected entry in storage")
        };
        total = total.checked_add(charge).context("stored size overflow")?;
    }
    Ok(total)
}
fn rebuild_quota(s: &mut Store) -> Result<()> {
    let mut query = s.db.prepare("SELECT id FROM projects")?;
    let ids = query
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut projects = BTreeMap::new();
    let mut total = 0u64;
    for id in ids {
        let used = PROJECT_CHARGE
            .checked_add(quota_used(&s.root.join("projects").join(&id), 1)?)
            .context("project quota overflow")?;
        total = total.checked_add(used).context("server quota overflow")?;
        projects.insert(id, used);
    }
    s.project_quota_used = projects;
    s.quota_used = total;
    s.quota_poisoned = false;
    Ok(())
}
fn snapshot_charge(
    old_root: &FsPath,
    desired: &BTreeMap<String, String>,
    decoded: &BTreeMap<String, Vec<u8>>,
) -> Result<u64> {
    let mut total = VERSION_CHARGE;
    let mut directories = BTreeSet::new();
    for path in desired.keys() {
        let size = match decoded.get(path) {
            Some(bytes) => bytes.len() as u64,
            None => fs::metadata(old_root.join(path))?.len(),
        };
        total = total
            .checked_add(charged_file_size(size)?)
            .context("snapshot charge overflow")?;
        let mut parent = FsPath::new(path).parent();
        while let Some(dir) = parent.filter(|dir| !dir.as_os_str().is_empty()) {
            directories.insert(dir.to_path_buf());
            parent = dir.parent();
        }
    }
    total = total
        .checked_add(directories.len() as u64 * ENTRY_CHARGE)
        .context("snapshot charge overflow")?;
    Ok(total)
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
    if s.quota_poisoned {
        return Err(ApiError(
            StatusCode::SERVICE_UNAVAILABLE,
            "storage quota needs recovery; restart server".into(),
        ));
    }
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
        total = total
            .checked_add(
                usize::try_from(
                    fs::metadata(old_root.join(k))
                        .map_err(anyhow::Error::from)?
                        .len(),
                )
                .map_err(anyhow::Error::from)?,
            )
            .ok_or_else(|| bad("project quota exceeded"))?;
    }
    if total > MAX_BYTES {
        return Err(bad("project quota exceeded"));
    }
    let charge = snapshot_charge(&old_root, &desired, &decoded)?;
    let next_project_bytes = s
        .project_quota_used
        .get(&p)
        .copied()
        .context("missing project quota state")?
        .checked_add(charge)
        .ok_or_else(|| bad("project storage quota exceeded"))?;
    let next_all_bytes = s
        .quota_used
        .checked_add(charge)
        .ok_or_else(|| bad("server storage quota exceeded"))?;
    if next_project_bytes > row.max_storage {
        return Err(ApiError(
            StatusCode::INSUFFICIENT_STORAGE,
            "project storage quota exceeded; delete an old version or raise max_storage".into(),
        ));
    }
    if next_all_bytes > s.max_total_storage {
        return Err(ApiError(
            StatusCode::INSUFFICIENT_STORAGE,
            "server storage quota exceeded; delete old versions or raise max-total-storage".into(),
        ));
    }
    let rev = id(12);
    let version = row.next;
    let root = s.root.join("projects").join(&p).join(version.to_string());
    let result = (|| -> Result<Option<Expiry>> {
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
        let expiry = renew_expiry(&tx, &p)?;
        tx.commit()?;
        Ok(expiry)
    })();
    let expiry = match result {
        Ok(expiry) => expiry,
        Err(e) => {
            if root.exists() && fs::remove_dir_all(&root).is_err() {
                s.quota_poisoned = true;
            }
            return Err(e.into());
        }
    };
    s.quota_used = next_all_bytes;
    s.project_quota_used.insert(p.clone(), next_project_bytes);
    Ok(Json(
        json!({"project":p,"revision":rev,"version":version,"version_url":format!("{}/s/{p}/versions/{version}/",s.public_url),"files":desired.len(),"bytes":total,"url":format!("{}/s/{p}/",s.public_url),"expires_at":expiry.map(|e|e.expires_at)}),
    ))
}

async fn delete_project(
    State(state): State<Shared>,
    Path(p): Path<String>,
    h: HeaderMap,
) -> ApiResult {
    let mut s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    project(&s, &p)?;
    s.db.execute("DELETE FROM projects WHERE id=?1", [&p])
        .map_err(anyhow::Error::from)?;
    let root = s.root.join("projects").join(&p);
    if root.exists() {
        fs::remove_dir_all(root).map_err(anyhow::Error::from)?;
    }
    if let Some(used) = s.project_quota_used.remove(&p) {
        s.quota_used = s.quota_used.saturating_sub(used);
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
    max_storage: Option<String>,
    on_expire: Option<ExpiryAction>,
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
    if body.name.is_none()
        && body.auth.is_none()
        && body.expires_in.is_none()
        && body.max_storage.is_none()
        && body.on_expire.is_none()
    {
        return Err(bad(
            "specify name, auth, expires_in, max_storage, or on_expire",
        ));
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
    let max_storage = body
        .max_storage
        .as_deref()
        .map(storage_size)
        .transpose()
        .map_err(|_| bad("invalid max_storage"))?
        .unwrap_or(row.max_storage);
    if name.len() > 256 {
        return Err(bad("name too long"));
    }
    let update_expiry = body.expires_in.is_some();
    let on_expire = body.on_expire.unwrap_or(row.on_expire);
    let expiry = match body.expires_in {
        None => row.expiry,
        Some(v) => expiry_for(crate::expiry::duration(&v).map_err(|_| bad("invalid expires_in"))?)
            .map_err(|_| bad("expiration too large"))?,
    };
    let tx = s.db.transaction().map_err(anyhow::Error::from)?;
    tx.execute(
        "UPDATE projects SET name=?1,auth=?2,max_storage=?3,on_expire=?4 WHERE id=?5",
        params![
            name,
            serde_json::to_string(&access).map_err(anyhow::Error::from)?,
            max_storage,
            on_expire,
            p,
        ],
    )
    .map_err(anyhow::Error::from)?;
    if update_expiry {
        tx.execute("DELETE FROM expiry WHERE project=?1", [&p])
            .map_err(anyhow::Error::from)?;
        if let Some(expiry) = expiry {
            tx.execute(
                "INSERT INTO expiry(project,lifetime,expires_at) VALUES(?1,?2,?3)",
                params![p, expiry.lifetime, expiry.expires_at],
            )
            .map_err(anyhow::Error::from)?;
        }
    }
    tx.commit().map_err(anyhow::Error::from)?;
    Ok(Json(
        json!({"project":p,"updated":true,"expires_at":expiry.map(|e|e.expires_at),"expires_in_seconds":expiry.map(|e|e.lifetime),"max_storage_bytes":max_storage,"on_expire":on_expire}),
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
    result["quota_used_bytes"] = json!(s
        .project_quota_used
        .get(&p)
        .copied()
        .context("missing project quota state")?);
    result["auth"] = json!({"view":access_summary(&row.access.view,s.admin.is_some()),"write":access_summary(&row.access.write,s.admin.is_some())});
    Ok(Json(result))
}
async fn status(State(state): State<Shared>, h: HeaderMap) -> ApiResult {
    let s = state.lock().unwrap();
    auth(&s, &h, None, "admin")?;
    Ok(Json(
        json!({"version":env!("CARGO_PKG_VERSION"),"authentication_required":s.admin.is_some(),"default_expires_in_seconds":s.default_lifetime,"max_total_storage_bytes":s.max_total_storage,"quota_used_bytes":s.quota_used,"default_on_expire":s.default_on_expire}),
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
fn purge_expired(s: &mut Store) -> Result<()> {
    let mut q =
        s.db.prepare("SELECT project FROM expiry JOIN projects ON projects.id=expiry.project WHERE expires_at <= ?1 AND on_expire=?2")?;
    let ids = q
        .query_map(params![crate::expiry::now(), ExpiryAction::Delete], |r| {
            r.get::<_, String>(0)
        })?
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
        } else if let Some(used) = s.project_quota_used.remove(&p) {
            s.quota_used = s.quota_used.saturating_sub(used);
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
    let mut s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    let row = project(&s, &p)?;
    if row.revision != body.base_revision {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "revision changed; retry operation".into(),
        ));
    }
    has_version(&s, &p, body.version)?;
    let tx = s.db.transaction().map_err(anyhow::Error::from)?;
    tx.execute(
        "UPDATE projects SET current=?1,revision=?2 WHERE id=?3",
        params![body.version, id(12), p],
    )
    .map_err(anyhow::Error::from)?;
    renew_expiry(&tx, &p)?;
    tx.commit().map_err(anyhow::Error::from)?;
    Ok(Json(summary(&s, &p, &project(&s, &p)?)))
}
async fn delete_version(
    State(state): State<Shared>,
    Path((p, n)): Path<(String, u64)>,
    h: HeaderMap,
) -> ApiResult {
    let mut s = state.lock().unwrap();
    auth(&s, &h, Some(&p), "write")?;
    let row = project(&s, &p)?;
    has_version(&s, &p, n)?;
    if row.current == n {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "cannot delete the current version".into(),
        ));
    }
    let path = s.root.join("projects").join(&p).join(n.to_string());
    let charge = VERSION_CHARGE
        .checked_add(quota_used(&path, usize::MAX)?)
        .context("version charge overflow")?;
    s.db.execute(
        "DELETE FROM versions WHERE project=?1 AND number=?2",
        params![p, n],
    )
    .map_err(anyhow::Error::from)?;
    fs::remove_dir_all(path).map_err(anyhow::Error::from)?;
    s.quota_used = s.quota_used.saturating_sub(charge);
    if let Some(used) = s.project_quota_used.get_mut(&p) {
        *used = used.saturating_sub(charge);
    }
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
pub struct ServeOptions<'a> {
    pub default_expires_in: &'a str,
    pub default_on_expire: ExpiryAction,
    pub request_timeout: std::time::Duration,
    pub max_total_storage: &'a str,
}

pub async fn serve(
    listen: std::net::SocketAddr,
    root: &FsPath,
    url: &str,
    credential: Option<&str>,
    options: ServeOptions<'_>,
) -> Result<()> {
    let ServeOptions {
        default_expires_in,
        default_on_expire,
        request_timeout,
        max_total_storage,
    } = options;
    if request_timeout.is_zero() {
        bail!("request timeout must be positive")
    }
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
    store.default_on_expire = default_on_expire;
    store.max_total_storage = storage_size(max_total_storage)?;
    purge_expired(&mut store)?;
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
    rebuild_quota(&mut store)?;
    let state = Arc::new(Mutex::new(store));
    let cleanup = state.clone();
    let janitor = tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            if let Err(e) = purge_expired(&mut cleanup.lock().unwrap()) {
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
                    match tokio::time::timeout(request_timeout, next.run(request)).await {
                        Ok(response) => response,
                        Err(_) => {
                            (StatusCode::REQUEST_TIMEOUT, "request timed out").into_response()
                        }
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_sizes_reject_zero_and_overflow() {
        assert_eq!(storage_size("500MiB").unwrap(), 500 * 1024 * 1024);
        assert_eq!(storage_size("10GiB").unwrap(), 10 * 1024 * 1024 * 1024);
        for value in ["0B", "-1B", "1MB", "1.5MiB", "9223372036854775808B"] {
            assert!(storage_size(value).is_err(), "{value}");
        }
    }

    #[test]
    fn upgrades_existing_projects_without_deleting_versions() {
        let dir = tempfile::tempdir().unwrap();
        let db = Connection::open(dir.path().join("sss.db")).unwrap();
        db.execute_batch("CREATE TABLE projects(id TEXT PRIMARY KEY,name TEXT NOT NULL,revision TEXT NOT NULL,current INTEGER NOT NULL DEFAULT 0,next INTEGER NOT NULL DEFAULT 1,auth TEXT NOT NULL); CREATE TABLE versions(project TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,number INTEGER NOT NULL,PRIMARY KEY(project,number)); PRAGMA user_version=2;").unwrap();
        db.execute(
            "INSERT INTO projects(id,name,revision,current,next,auth) VALUES('abc','existing','rev',1,2,?1)",
            [serde_json::to_string(&Access::default().store().unwrap()).unwrap()],
        )
        .unwrap();
        db.execute("INSERT INTO versions(project,number) VALUES('abc',1)", [])
            .unwrap();
        drop(db);

        let store = open(dir.path(), "http://localhost:8080", None).unwrap();
        assert_eq!(
            project(&store, "abc").unwrap().max_storage,
            500 * 1024 * 1024
        );
        assert_eq!(
            store
                .db
                .query_row("SELECT count(*) FROM versions", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            store
                .db
                .query_row("PRAGMA user_version", [], |r| r.get::<_, u64>(0))
                .unwrap(),
            4
        );
    }
}
