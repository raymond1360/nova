//! Owned-file and SQLite adapter. Never opens the application's daily database.
use super::{
    contract::*,
    policy::{self, Candidate},
    Result,
};
use crate::{
    db::Database,
    models::{Actor, CreateProjectRequest, CreateTaskRequest, TaskState},
    storage::tasks::is_legal_transition,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

pub struct Guard {
    file: File,
    pub path: PathBuf,
    pub identity: String,
    tool_links: bool,
}
fn normal(s: &str) -> String {
    s.trim_start_matches("\\\\?\\")
        .replace('/', "\\")
        .to_lowercase()
}
pub fn absolute(path: &Path) -> Result<()> {
    let s = path.to_str().ok_or("PATH_REJECTED")?;
    if !path.is_absolute()
        || s.starts_with("\\\\")
        || s.contains('\0')
        || s.chars().filter(|c| *c == ':').count() != 1
        || s.split(['\\', '/'])
            .skip(1)
            .any(|p| p == ".." || p == "." || p.ends_with(['.', ' ']))
    {
        return Err("PATH_REJECTED");
    }
    Ok(())
}
fn components(path: &Path) -> Result<()> {
    absolute(path)?;
    let mut part = PathBuf::new();
    for c in path.components() {
        part.push(c);
        let m = fs::symlink_metadata(&part).map_err(|_| "PATH_MISSING")?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err("LINK_REJECTED");
            }
        }
        if m.file_type().is_symlink() {
            return Err("LINK_REJECTED");
        }
    }
    Ok(())
}
#[cfg(windows)]
fn facts(file: &File) -> Result<(String, PathBuf, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION,
    };
    let mut i: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let h = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    if unsafe { GetFileInformationByHandle(h, &mut i) } == 0 || i.dwFileAttributes & 0x400 != 0 {
        return Err("FILE_IDENTITY_UNKNOWN");
    }
    let mut buffer = vec![0u16; 32768];
    let n = unsafe { GetFinalPathNameByHandleW(h, buffer.as_mut_ptr(), buffer.len() as u32, 0) }
        as usize;
    if n == 0 || n >= buffer.len() {
        return Err("FILE_IDENTITY_UNKNOWN");
    }
    let p = String::from_utf16(&buffer[..n]).map_err(|_| "PATH_REJECTED")?;
    if p.starts_with("\\\\?\\UNC\\") {
        return Err("PATH_REJECTED");
    }
    Ok((
        format!(
            "{:08x}:{:08x}{:08x}",
            i.dwVolumeSerialNumber, i.nFileIndexHigh, i.nFileIndexLow
        ),
        PathBuf::from(p.trim_start_matches("\\\\?\\")),
        i.nNumberOfLinks,
    ))
}
#[cfg(not(windows))]
fn facts(_: &File) -> Result<(String, PathBuf, u32)> {
    Err("WINDOWS_REQUIRED")
}
impl Guard {
    pub fn open(path: &Path, db: bool) -> Result<Self> {
        Self::open_policy(path, db, false)
    }
    /// Installed tools may legitimately be hardlinked (Git for Windows). This
    /// exception never applies to repositories, reports or owned databases.
    pub fn tool(path: &Path) -> Result<Self> {
        Self::open_policy(path, false, true)
    }
    fn open_policy(path: &Path, db: bool, tool_links: bool) -> Result<Self> {
        components(path)?;
        let mut opts = OpenOptions::new();
        opts.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(if db { 3 } else { 1 })
                .custom_flags(0x00200000 | 0x02000000);
        }
        let file = opts.open(path).map_err(|_| "FILE_OPEN_REJECTED")?;
        let (identity, final_path, links) = facts(&file)?;
        if normal(path.to_str().ok_or("PATH_REJECTED")?)
            != normal(final_path.to_str().ok_or("PATH_REJECTED")?)
            || (!tool_links && !file.metadata().map_err(|_| "FILE_STAT")?.is_dir() && links != 1)
        {
            return Err("ALIAS_REJECTED");
        }
        Ok(Self {
            file,
            path: final_path,
            identity,
            tool_links,
        })
    }
    pub fn verify(&self) -> Result<()> {
        let (id, path, _) = facts(&self.file)?;
        let other = Guard::open_policy(&self.path, true, self.tool_links)?;
        if id != self.identity || path != self.path || other.identity != id {
            return Err("FILE_CHANGED");
        }
        Ok(())
    }
    pub fn bytes(&mut self, limit: usize) -> Result<Vec<u8>> {
        let before = self.file.metadata().map_err(|_| "FILE_STAT")?;
        if !before.is_file() || before.len() > limit as u64 {
            return Err("FILE_BOUND");
        }
        let mut b = vec![];
        (&mut self.file)
            .take(limit as u64 + 1)
            .read_to_end(&mut b)
            .map_err(|_| "FILE_READ")?;
        let after = self.file.metadata().map_err(|_| "FILE_STAT")?;
        self.verify()?;
        if b.len() as u64 != before.len()
            || after.len() != before.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err("FILE_CHANGED");
        }
        Ok(b)
    }
}
pub fn read_file(path: &Path, limit: usize) -> Result<(Vec<u8>, String)> {
    let mut g = Guard::open(path, false)?;
    let b = g.bytes(limit)?;
    Ok((b, g.identity))
}
pub fn inside(path: &Path, root: &Path) -> bool {
    let p = normal(&path.to_string_lossy());
    let r = normal(&root.to_string_lossy());
    p == r || p.starts_with(&(r + "\\"))
}
pub fn safe_location(path: &Path) -> Result<()> {
    absolute(path)?;
    let s = normal(&path.to_string_lossy());
    if s.contains("\\appdata\\")
        || s.split('\\')
            .any(|p| matches!(p, ".ssh" | ".aws" | "credentials"))
        || matches!(
            path.file_name().and_then(|s| s.to_str()),
            Some("auth.json" | ".npmrc" | ".env")
        )
    {
        return Err("SENSITIVE_PATH_REJECTED");
    }
    Ok(())
}
fn new_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "CREATE_NEW_REJECTED")?;
    f.write_all(bytes).map_err(|_| "WRITE_FAILED")?;
    f.sync_all().map_err(|_| "FLUSH_FAILED")
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Owner {
    magic: String,
    db_id: String,
    root: String,
    root_identity: String,
    db_identity: String,
}

pub const SCHEMA: &str = r#"
CREATE TABLE scheduler_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),magic TEXT NOT NULL CHECK(magic='NOVA_SH1_OWNED_V1'),purpose TEXT NOT NULL CHECK(purpose='INTERNAL_HANDOFF_ONLY'),schema_version INTEGER NOT NULL CHECK(schema_version=1),db_id TEXT NOT NULL UNIQUE,owner_root TEXT NOT NULL,product_user_version_at_init INTEGER NOT NULL,ready INTEGER NOT NULL CHECK(ready=1));
CREATE TABLE scheduler_packages(package_id TEXT PRIMARY KEY,project_id TEXT NOT NULL REFERENCES nova_projects(id),task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id),manifest_json TEXT NOT NULL,manifest_sha256 TEXT NOT NULL,policy_id TEXT NOT NULL CHECK(policy_id='INTERNAL_MANUAL_V1'),phase TEXT NOT NULL,revision INTEGER NOT NULL CHECK(revision>=0),current_envelope_id TEXT,base_sha TEXT NOT NULL CHECK(length(base_sha)=40),base_source TEXT NOT NULL CHECK(base_source IN ('TOOL_GIT_READ','HUMAN_ATTESTED')),current_delivery_identity TEXT NOT NULL CHECK(length(current_delivery_identity)=64),auto_repair_count INTEGER NOT NULL DEFAULT 0 CHECK(auto_repair_count=0),manual_rounds_recorded INTEGER NOT NULL DEFAULT 0 CHECK(manual_rounds_recorded>=0),readiness_invalidated INTEGER NOT NULL DEFAULT 0 CHECK(readiness_invalidated IN (0,1)),registration_closed INTEGER NOT NULL DEFAULT 0 CHECK(registration_closed IN (0,1)),closure_receipt_id TEXT);
CREATE TABLE scheduler_envelopes(envelope_id TEXT PRIMARY KEY,package_id TEXT NOT NULL REFERENCES scheduler_packages(package_id),envelope_json TEXT NOT NULL,observed_head_sha TEXT NOT NULL CHECK(length(observed_head_sha)=40),observed_utc TEXT NOT NULL,delivery_identity TEXT NOT NULL CHECK(length(delivery_identity)=64));
CREATE TABLE scheduler_receipts(receipt_id TEXT PRIMARY KEY,package_id TEXT NOT NULL REFERENCES scheduler_packages(package_id),envelope_id TEXT REFERENCES scheduler_envelopes(envelope_id),mode TEXT NOT NULL CHECK(mode IN ('CURRENT','INDEX_ONLY')),report_type TEXT CHECK(report_type IN ('DEVELOPMENT','REVIEW','CI','PM_DECISION')),file_path TEXT NOT NULL,file_bytes INTEGER NOT NULL CHECK(file_bytes>=0 AND file_bytes<=2097152),file_sha256 TEXT NOT NULL CHECK(length(file_sha256)=64),metadata_json TEXT,delivery_identity TEXT,source_receipt_id TEXT REFERENCES scheduler_receipts(receipt_id),transcribed_by_json TEXT,CHECK(mode!='CURRENT' OR (delivery_identity IS NOT NULL AND length(delivery_identity)=64)),CHECK((source_receipt_id IS NULL AND transcribed_by_json IS NULL) OR (source_receipt_id IS NOT NULL AND transcribed_by_json IS NOT NULL AND source_receipt_id!=receipt_id)),UNIQUE(package_id,file_sha256,mode));
CREATE TABLE scheduler_issue_snapshots(receipt_id TEXT NOT NULL REFERENCES scheduler_receipts(receipt_id),issue_id TEXT NOT NULL,severity TEXT NOT NULL CHECK(severity IN ('BLOCKER','HIGH','MEDIUM','LOW','INFO')),blocking INTEGER NOT NULL CHECK(blocking IN (0,1)),status TEXT NOT NULL CHECK(status IN ('OPEN','CLOSED')),first_seen_sha TEXT NOT NULL CHECK(length(first_seen_sha)=40),closed_sha TEXT,CHECK((status='OPEN' AND closed_sha IS NULL) OR (status='CLOSED' AND closed_sha IS NOT NULL AND length(closed_sha)=40)),PRIMARY KEY(receipt_id,issue_id));
CREATE TABLE scheduler_events(seq INTEGER PRIMARY KEY AUTOINCREMENT,package_id TEXT NOT NULL REFERENCES scheduler_packages(package_id),revision INTEGER NOT NULL,idempotency_key TEXT NOT NULL,kind TEXT NOT NULL,actor TEXT NOT NULL CHECK(actor IN ('user','nova','system')),payload_json TEXT NOT NULL,UNIQUE(package_id,revision),UNIQUE(package_id,idempotency_key));
"#;
fn sql<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|_| "STORE_SQL_ERROR")
}
fn product<T>(r: crate::error::NovaResult<T>) -> Result<T> {
    r.map_err(|_| "PRODUCT_STORE_REJECTED")
}
fn fk(c: &Connection) -> Result<()> {
    sql(c.execute_batch("PRAGMA foreign_keys=ON;"))?;
    if sql(c.query_row("PRAGMA foreign_keys", [], |r| r.get::<_, i64>(0)))? != 1 {
        return Err("FOREIGN_KEYS_DISABLED");
    }
    Ok(())
}
fn encoded<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|_| "ENCODING")
}

pub struct Store {
    root: PathBuf,
    owner: Owner,
    _root_guard: Guard,
    _db_guard: Guard,
}
#[derive(Clone)]
pub struct Snapshot {
    pub manifest: Manifest,
    pub task_id: String,
    pub revision: i64,
    pub delivery: String,
    pub base: String,
    pub base_source: String,
    pub phase: String,
    pub invalidated: bool,
    pub closed: bool,
    pub manual_rounds: i64,
    pub current_envelope: String,
}
#[derive(Clone)]
pub struct Import {
    pub path: String,
    pub bytes: Vec<u8>,
    pub report: Option<Report>,
}
impl Store {
    pub fn init(root: &Path, m: &Manifest, e: &Envelope) -> Result<Self> {
        Self::init_stage(root, m, e, 0)
    }
    // Fixed fault points only accessible to this crate's tests; never an input/CLI flag.
    fn init_stage(root: &Path, m: &Manifest, e: &Envelope, fault: u8) -> Result<Self> {
        m.validate()?;
        safe_location(root)?;
        absolute(root)?;
        let parent = root.parent().ok_or("PATH_REJECTED")?;
        let _parent = Guard::open(parent, true)?;
        if root.exists() {
            return Err("CREATE_NEW_REJECTED");
        }
        let repo = Guard::open(Path::new(&m.repo_path), true)?;
        if repo.identity != e.root_identity {
            return Err("ROOT_MISMATCH");
        }
        if inside(root, &repo.path)
            || inside(&repo.path, root)
            || inside(root, Path::new(&e.git_dir))
            || inside(Path::new(&e.git_dir), root)
        {
            return Err("STORE_ROOT_REJECTED");
        }
        for r in &m.report_roots {
            safe_location(Path::new(r))?;
            let _ = Guard::open(Path::new(r), true)?;
        }
        fs::create_dir(root).map_err(|_| "CREATE_NEW_REJECTED")?;
        let rg = Guard::open(root, true)?;
        new_file(&root.join("handoff.sqlite3"), &[])?;
        let dg = Guard::open(&root.join("handoff.sqlite3"), true)?;
        let owner = Owner {
            magic: "NOVA_SH1_OWNED_V1".into(),
            db_id: uuid::Uuid::new_v4().to_string(),
            root: rg.path.to_string_lossy().into(),
            root_identity: rg.identity.clone(),
            db_identity: dg.identity.clone(),
        };
        new_file(&root.join("owner.json"), encoded(&owner)?.as_bytes())?;
        let db = Database::open(&dg.path).map_err(|_| "INIT_OPEN_FAILED")?;
        fk(db.conn())?;
        if fault == 1 {
            return Err("INJECTED_INIT_FAILURE");
        }
        db.migrate().map_err(|_| "INIT_MIGRATION_FAILED")?;
        if fault == 2 {
            return Err("INJECTED_INIT_FAILURE");
        }
        let project = product(db.create_project(&CreateProjectRequest {
            name: m.title.clone(),
            local_path: m.repo_path.clone(),
            source_type: "local".into(),
            source_url: None,
            purpose: Some(m.goal.clone()),
            long_term_goal: None,
            user_preferences: None,
            prohibitions: Some("Handoff registration only; no execution authorization".into()),
        }))?;
        if fault == 3 {
            return Err("INJECTED_INIT_FAILURE");
        }
        sql(db.conn().execute_batch("BEGIN IMMEDIATE;"))?;
        let result = (|| {
            sql(db.conn().execute_batch(SCHEMA))?;
            let task = product(db.create_task_in_transaction(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: m.title.clone(),
                    goal: Some(m.goal.clone()),
                    in_scope: Some(encoded(&m.allowed_paths)?),
                    not_in_scope: Some("External execution, models, CI and publication".into()),
                    done_criteria: Some(encoded(&m.acceptance_items)?),
                    ai_kind: None,
                },
                Actor::User,
            ))?;
            for (state, actor) in [
                (TaskState::AwaitingConfirmation, Actor::User),
                (TaskState::Confirmed, Actor::User),
                (TaskState::Queued, Actor::Nova),
            ] {
                step(&db, &task.id, state, actor, "INIT")?;
            }
            let v: i64 = sql(db.conn().query_row("PRAGMA user_version", [], |r| r.get(0)))?;
            if v != 12 {
                return Err("PRODUCT_SCHEMA_UNSUPPORTED");
            }
            sql(db.conn().execute("INSERT INTO scheduler_meta VALUES(1,'NOVA_SH1_OWNED_V1','INTERNAL_HANDOFF_ONLY',1,?1,?2,?3,1)",params![owner.db_id,owner.root,v]))?;
            sql(db.conn().execute("INSERT INTO scheduler_packages(package_id,project_id,task_id,manifest_json,manifest_sha256,policy_id,phase,revision,current_envelope_id,base_sha,base_source,current_delivery_identity) VALUES(?1,?2,?3,?4,?5,'INTERNAL_MANUAL_V1','HANDOFF_READY',0,?6,?7,?8,?9)",params![m.package_id,project.id,task.id,encoded(m)?,hash(encoded(m)?.as_bytes()),e.envelope_id,e.base,e.base_source,e.delivery_identity]))?;
            save_envelope(db.conn(), m, e)?;
            sql(db.conn().execute("INSERT INTO scheduler_events(package_id,revision,idempotency_key,kind,actor,payload_json) VALUES(?1,0,'INIT','INIT','user',?2)",params![m.package_id,encoded(&json!({"base_source":e.base_source,"execution_permission":"NOT_GRANTED_BY_SH1"}))?]))?;
            if fault == 4 {
                return Err("INJECTED_INIT_FAILURE");
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                sql(db.conn().execute_batch("COMMIT;"))?;
            }
            Err(e) => {
                let _ = db.conn().execute_batch("ROLLBACK;");
                return Err(e);
            }
        }
        drop(db);
        drop(dg);
        drop(rg);
        Self::open(root)
    }
    pub fn open(root: &Path) -> Result<Self> {
        safe_location(root)?;
        let rg = Guard::open(root, true)?;
        let (b, _) =
            read_file(&root.join("owner.json"), 16384).map_err(|_| "OWNERSHIP_REJECTED")?;
        let owner: Owner = json(&b).map_err(|_| "OWNERSHIP_REJECTED")?;
        let dg =
            Guard::open(&root.join("handoff.sqlite3"), true).map_err(|_| "OWNERSHIP_REJECTED")?;
        if owner.magic != "NOVA_SH1_OWNED_V1"
            || owner.root != rg.path.to_string_lossy()
            || owner.root_identity != rg.identity
            || owner.db_identity != dg.identity
        {
            return Err("OWNERSHIP_REJECTED");
        }
        let s = Self {
            root: rg.path.clone(),
            owner,
            _root_guard: rg,
            _db_guard: dg,
        };
        check_layout(&s.root)?;
        let c = s.read_connection()?;
        let mark=sql(c.query_row("SELECT magic,purpose,schema_version,db_id,owner_root,product_user_version_at_init,ready FROM scheduler_meta WHERE singleton=1",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,i64>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?,r.get::<_,i64>(6)?)))).map_err(|_|"PARTIAL_INIT")?;
        if mark
            != (
                "NOVA_SH1_OWNED_V1".into(),
                "INTERNAL_HANDOFF_ONLY".into(),
                1,
                s.owner.db_id.clone(),
                s.owner.root.clone(),
                12,
                1,
            )
            || sql(c.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)))? != 12
            || sql(
                c.query_row("SELECT COUNT(*) FROM scheduler_packages", [], |r| {
                    r.get::<_, i64>(0)
                }),
            )? != 1
        {
            return Err("STORE_VERSION_REJECTED");
        }
        let _ = s.snapshot()?;
        Ok(s)
    }
    fn read_connection(&self) -> Result<Connection> {
        self._root_guard.verify()?;
        self._db_guard.verify()?;
        let c = sql(Connection::open_with_flags(
            self.root.join("handoff.sqlite3"),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ))?;
        sql(c.busy_timeout(std::time::Duration::from_secs(5)))?;
        self._db_guard.verify()?;
        Ok(c)
    }
    fn write_connection(&self) -> Result<Database> {
        let _preflight = self.read_connection()?;
        let db = Database::open(&self.root.join("handoff.sqlite3"))
            .map_err(|_| "STORE_OPEN_REJECTED")?;
        self._db_guard.verify()?;
        fk(db.conn())?;
        Ok(db)
    }
    pub fn snapshot(&self) -> Result<Snapshot> {
        let c = self.read_connection()?;
        snapshot(&c)
    }
    pub fn audit_envelope(&self) -> Result<Envelope> {
        let c = self.read_connection()?;
        let s = snapshot(&c)?;
        let text: String = sql(c.query_row(
            "SELECT envelope_json FROM scheduler_envelopes WHERE envelope_id=?1",
            [s.current_envelope],
            |r| r.get(0),
        ))?;
        text.pipe_json()
    }
    pub fn unverified_status(&self, code: &'static str) -> Result<Value> {
        self.unverified_view(code, false)
    }
    pub fn unverified_handoff(&self, code: &'static str) -> Result<Value> {
        self.unverified_view(code, true)
    }
    fn unverified_view(&self, code: &'static str, compact: bool) -> Result<Value> {
        let c = self.read_connection()?;
        let snap = snapshot(&c)?;
        let saved: String = sql(c.query_row(
            "SELECT envelope_json FROM scheduler_envelopes WHERE envelope_id=?1",
            [snap.current_envelope],
            |r| r.get(0),
        ))?;
        let old: Envelope = saved.pipe_json()?;
        let mut v = self.view(&old, compact, Some(code))?;
        let obj = v.as_object_mut().ok_or("ENCODING")?;
        if let Some(head) = obj.remove("head") {
            obj.insert("last_observed_head".into(), head);
        }
        obj.insert("error_code".into(), json!(code));
        obj.insert("current_identity_verified".into(), json!(false));
        obj.insert("sampled_delivery_identity".into(), Value::Null);
        obj.insert("projection_is_historical_only".into(), json!(true));
        Ok(v)
    }
    pub fn import(&self, expected: &Snapshot, e: &Envelope, input: &Import) -> Result<Value> {
        let m = &expected.manifest;
        let next_revision = expected
            .revision
            .checked_add(1)
            .ok_or("REVISION_OVERFLOW")?;
        let _repo_guard = Guard::open(Path::new(&m.repo_path), true)?;
        if _repo_guard.identity != e.root_identity {
            return Err("ROOT_MISMATCH");
        }
        if input.bytes.len() > REPORT_LIMIT {
            return Err("INPUT_BOUND");
        }
        let mode = if input.report.is_some() {
            "CURRENT"
        } else {
            "INDEX_ONLY"
        };
        let digest = hash(&input.bytes);
        let rid = hash(format!("{}:{digest}:{mode}", m.package_id).as_bytes());
        let repair = input
            .report
            .as_ref()
            .is_some_and(|r| r.context.stage == "REPAIR");
        if let Some(r) = &input.report {
            r.validate()?;
            e.validate_report(m, r)?;
        }
        if !repair && mode == "CURRENT" && expected.delivery != e.delivery_identity {
            return Err("VERSION_MISMATCH");
        }
        if e.base != expected.base {
            return Err("BASELINE_CHANGED");
        }
        // Hash the original outside SQLite's write lock, retaining its read-only
        // guard until commit. The DB revision then protects the referenced row.
        let _source_guard = if let Some(r) = &input.report {
            validate_source(&self.read_connection()?, m, r)?
        } else {
            None
        };
        let db = self.write_connection()?;
        sql(db.conn().execute_batch("BEGIN IMMEDIATE;"))?;
        let result = (|| {
            let mut now = snapshot(db.conn())?;
            if now.revision != expected.revision || now.delivery != expected.delivery {
                return Err("CONCURRENT_REVISION");
            }
            if let Some((existing,d))=sql(db.conn().query_row("SELECT receipt_id,delivery_identity FROM scheduler_receipts WHERE package_id=?1 AND file_sha256=?2 AND mode=?3",params![m.package_id,digest,mode],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?))).optional())? {
                if mode=="CURRENT" && d.as_deref()!=Some(e.delivery_identity.as_str()){return Err("STALE_RECEIPT")}
                return Ok(json!({"receipt_id":existing,"idempotent":true,"registration_closed":now.closed,"source":"TOOL_FILE_HASH"}));
            }
            if now.closed {
                return Err("REGISTRATION_CLOSED");
            }
            let candidates = load_candidates(db.conn(), &now.delivery, false)?;
            let resolutions = load_candidates(db.conn(), &now.delivery, true)?;
            if let Some(r) = &input.report {
                if r.report_type == ReportType::Review {
                    // Also preserve issue IDs across deliveries, not just same-SHA candidates.
                    let mut stmt=sql(db.conn().prepare("SELECT metadata_json FROM scheduler_receipts WHERE mode='CURRENT' AND report_type='REVIEW' ORDER BY rowid"))?;
                    let previous = sql(stmt.query_map([], |row| row.get::<_, String>(0)))?;
                    for raw in previous {
                        let old: Report = json(sql(raw)?.as_bytes())?;
                        policy::validate_history(&old, r)?;
                    }
                }
                if let Some(res) = &r.context.slot_resolution {
                    validate_resolution(res, &candidates, &resolutions, &e.delivery_identity)?;
                }
                if repair {
                    let old:Envelope=sql(db.conn().query_row("SELECT envelope_json FROM scheduler_envelopes WHERE delivery_identity=?1 ORDER BY rowid DESC LIMIT 1",[&now.delivery],|r|r.get::<_,String>(0)))?.pipe_json()?;
                    if r.context.supersedes_sha.as_deref() != Some(old.head.as_str())
                        || !matches!(
                            product(db.task(&now.task_id))?.state,
                            TaskState::NeedsUserAction | TaskState::AwaitingUserAcceptance
                        )
                    {
                        return Err("REPAIR_STATE_REJECTED");
                    }
                    step(&db, &now.task_id, TaskState::SafeFixing, Actor::User, &rid)?;
                    now.manual_rounds = now.manual_rounds.checked_add(1).ok_or("ROUND_OVERFLOW")?;
                    if now.delivery == e.delivery_identity {
                        now.invalidated = true;
                        step(
                            &db,
                            &now.task_id,
                            TaskState::NeedsUserAction,
                            Actor::Nova,
                            &rid,
                        )?;
                    } else {
                        now.delivery = e.delivery_identity.clone();
                        now.invalidated = false;
                        step(&db, &now.task_id, TaskState::Verifying, Actor::Nova, &rid)?;
                    }
                }
            }
            save_envelope(db.conn(), m, e)?;
            let report_type = input
                .report
                .as_ref()
                .map(|r| encoded(&r.report_type))
                .transpose()?
                .map(|s| s.trim_matches('"').to_string());
            let metadata = input.report.as_ref().map(encoded).transpose()?;
            let src = input
                .report
                .as_ref()
                .and_then(|r| r.source_report.as_ref())
                .map(|s| s.receipt_id.as_str());
            let trans = input
                .report
                .as_ref()
                .and_then(|r| r.transcribed_by.as_ref())
                .map(encoded)
                .transpose()?;
            sql(db.conn().execute(
                "INSERT INTO scheduler_receipts VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    rid,
                    m.package_id,
                    e.envelope_id,
                    mode,
                    report_type,
                    input.path,
                    input.bytes.len() as i64,
                    digest,
                    metadata,
                    if mode == "CURRENT" {
                        Some(e.delivery_identity.as_str())
                    } else {
                        None
                    },
                    src,
                    trans
                ],
            ))?;
            #[cfg(test)]
            if IMPORT_FAILURE.with(|v| v.get()) {
                return Err("INJECTED_IMPORT_FAILURE");
            }
            if let Some(r) = &input.report {
                for i in &r.issues {
                    sql(db.conn().execute(
                        "INSERT INTO scheduler_issue_snapshots VALUES(?1,?2,?3,?4,?5,?6,?7)",
                        params![
                            rid,
                            i.id,
                            encoded(&i.severity)?.trim_matches('"'),
                            i.blocking as i64,
                            encoded(&i.status)?.trim_matches('"'),
                            i.first_seen_sha,
                            i.closed_sha
                        ],
                    ))?;
                }
                let state = product(db.task(&now.task_id))?.state;
                if r.report_type == ReportType::Development && !repair && state == TaskState::Queued
                {
                    step(&db, &now.task_id, TaskState::Running, Actor::User, &rid)?;
                    step(&db, &now.task_id, TaskState::Verifying, Actor::Nova, &rid)?;
                }
                let cs = load_candidates(db.conn(), &now.delivery, false)?;
                let rs = load_candidates(db.conn(), &now.delivery, true)?;
                let projection = policy::project(&m.required_reports, &cs, &rs, now.invalidated);
                match r.context.decision_kind.as_deref() {
                    Some("CONTINUE_MATERIALS") => {
                        if !projection.conflicts.is_empty() || !projection.blocking.is_empty() {
                            return Err("MATERIALS_STILL_BLOCKED");
                        }
                        if state == TaskState::Interrupted {
                            step(
                                &db,
                                &now.task_id,
                                TaskState::NeedsUserAction,
                                Actor::User,
                                &rid,
                            )?;
                        }
                        step(
                            &db,
                            &now.task_id,
                            if projection.has_development {
                                TaskState::Verifying
                            } else {
                                TaskState::Queued
                            },
                            Actor::User,
                            &rid,
                        )?;
                    }
                    Some("PAUSE") => {
                        step(&db, &now.task_id, TaskState::Interrupted, Actor::User, &rid)?
                    }
                    Some("CANCEL") => {
                        step(&db, &now.task_id, TaskState::Cancelled, Actor::User, &rid)?
                    }
                    Some("CLOSURE_REFERENCE") => {
                        if state != TaskState::AwaitingUserAcceptance || !projection.complete {
                            return Err("CLOSURE_REJECTED");
                        }
                        now.closed = true;
                    }
                    _ => (),
                }
                let state = product(db.task(&now.task_id))?.state;
                if state == TaskState::Verifying {
                    if !projection.conflicts.is_empty() || !projection.blocking.is_empty() {
                        step(
                            &db,
                            &now.task_id,
                            TaskState::NeedsUserAction,
                            Actor::Nova,
                            &rid,
                        )?;
                    } else if projection.complete {
                        step(
                            &db,
                            &now.task_id,
                            TaskState::PreviewReady,
                            Actor::Nova,
                            &rid,
                        )?;
                        step(
                            &db,
                            &now.task_id,
                            TaskState::AwaitingUserAcceptance,
                            Actor::Nova,
                            &rid,
                        )?;
                    }
                } else if state == TaskState::AwaitingUserAcceptance
                    && (!projection.conflicts.is_empty() || !projection.blocking.is_empty())
                {
                    step(
                        &db,
                        &now.task_id,
                        TaskState::NeedsUserAction,
                        Actor::User,
                        &rid,
                    )?;
                }
            }
            let state = product(db.task(&now.task_id))?.state;
            let phase = policy::phase(state)?;
            let n=sql(db.conn().execute("UPDATE scheduler_packages SET revision=revision+1,phase=?1,current_delivery_identity=?2,current_envelope_id=?3,manual_rounds_recorded=?4,readiness_invalidated=?5,registration_closed=?6,closure_receipt_id=?7 WHERE package_id=?8 AND revision=?9",params![phase,now.delivery,e.envelope_id,now.manual_rounds,now.invalidated as i64,now.closed as i64,if now.closed{Some(rid.as_str())}else{None},m.package_id,expected.revision]))?;
            if n != 1 {
                return Err("CONCURRENT_REVISION");
            }
            sql(db.conn().execute("INSERT INTO scheduler_events(package_id,revision,idempotency_key,kind,actor,payload_json) VALUES(?1,?2,?3,'IMPORT','user',?4)",params![m.package_id,next_revision,rid,encoded(&json!({"receipt_id":rid,"mode":mode,"from_delivery":expected.delivery,"to_delivery":now.delivery,"readiness_invalidated":now.invalidated,"manual_rounds_recorded":now.manual_rounds,"execution_permission":"NOT_GRANTED_BY_SH1"}))?]))?;
            Ok(
                json!({"receipt_id":rid,"idempotent":false,"phase":phase,"revision":next_revision,"execution_permission":"NOT_GRANTED_BY_SH1"}),
            )
        })();
        match result {
            Ok(v) => {
                sql(db.conn().execute_batch("COMMIT;"))?;
                Ok(v)
            }
            Err(e) => {
                let _ = db.conn().execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }
    pub fn status(&self, e: &Envelope) -> Result<Value> {
        self.view(e, false, None)
    }
    pub fn handoff(&self, e: &Envelope) -> Result<Value> {
        self.view(e, true, None)
    }
    fn view(&self, e: &Envelope, compact: bool, forced_hold: Option<&str>) -> Result<Value> {
        let c = self.read_connection()?;
        let s = snapshot(&c)?;
        let cs = load_candidates(&c, &s.delivery, false)?;
        let rs = load_candidates(&c, &s.delivery, true)?;
        let p = policy::project(&s.manifest.required_reports, &cs, &rs, s.invalidated);
        let mut receipts = vec![];
        let mut stmt=sql(c.prepare("SELECT receipt_id,mode,file_path,file_bytes,file_sha256,metadata_json,delivery_identity,source_receipt_id FROM scheduler_receipts ORDER BY rowid"))?;
        let rows = sql(stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
            ))
        }))?;
        let mut missing = false;
        for row in rows {
            let (rid, mode, path, bytes, digest, metadata, d, source) = sql(row)?;
            let file_state = match read_file(Path::new(&path), REPORT_LIMIT) {
                Ok((b, _)) if b.len() as i64 == bytes && hash(&b) == digest => "PRESENT",
                Ok(_) => "CHANGED",
                Err(_) => "MISSING_OR_UNREADABLE",
            };
            if file_state != "PRESENT" {
                missing = true;
            }
            receipts.push(if compact{json!({"receipt_id":rid,"mode":mode,"file_path":attributed(path,Source::ToolFileHash),"file_state":file_state,"sha256":attributed(digest,Source::ToolFileHash),"delivery_identity":d,"source_receipt_id":source})}else{json!({"receipt_id":rid,"mode":mode,"file_path":attributed(path,Source::ToolFileHash),"file_state":file_state,"bytes":attributed(bytes,Source::ToolFileHash),"sha256":attributed(digest,Source::ToolFileHash),"claims":metadata.and_then(|v|serde_json::from_str::<Value>(&v).ok()).map(|v|attributed(v,Source::HumanAttested)),"delivery_identity":d,"source_receipt_id":source})});
        }
        let stale = e.delivery_identity != s.delivery;
        let task: (String, i64) = sql(c.query_row(
            "SELECT state,result_generation FROM tasks WHERE id=?1",
            [&s.task_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        ))?;
        let root_files = if compact {
            vec![]
        } else {
            inventory(&self.root)?
        };
        let hold = forced_hold.or(if stale {
            Some("DELIVERY_IDENTITY_CHANGED")
        } else if missing {
            Some("REPORT_IDENTITY_UNKNOWN")
        } else {
            None
        });
        let handoff = policy::handoff_plan(&s.phase, &p, &cs, s.invalidated, s.closed, hold);
        if compact {
            return Ok(
                json!({"schema":"nova.scheduler.handoff/1","context":attributed(json!({"package_id":s.manifest.package_id,"title":s.manifest.title,"goal":s.manifest.goal,"allowed_paths":s.manifest.allowed_paths,"acceptance_items":s.manifest.acceptance_items,"pm_decision_ref":s.manifest.pm_decision_ref}),Source::HumanAttested),"head":attributed(&e.head,Source::ToolGitRead),"base":attributed(&e.base,Source::base(&s.base_source)?),"delivery_identity":s.delivery,"sampled_delivery_identity":e.delivery_identity,"eol_policy":attributed(&s.manifest.eol_policy,Source::HumanAttested),"scope_claim":"FINAL_CUMULATIVE_DIFF_ONLY","phase":s.phase,"projection":p,"version_stale":stale,"report_identity_problem":missing,"handoff":handoff,"receipts":receipts,"external_actions_executed":"NONE","execution_permission":"NOT_GRANTED_BY_SH1"}),
            );
        }
        Ok(
            json!({"schema":"nova.scheduler.status/1","manifest":attributed(&s.manifest,Source::HumanAttested),"package_id":attributed(&s.manifest.package_id,Source::HumanAttested),"phase":s.phase,"task_state":task.0,"result_generation":task.1,"projection":p,"head":attributed(&e.head,Source::ToolGitRead),"base":attributed(&e.base,Source::base(&s.base_source)?),"scope_claim":"FINAL_CUMULATIVE_DIFF_ONLY; DOES_NOT_PROVE_INTERMEDIATE_COMMITS_STAYED_IN_SCOPE","envelope_id":e.envelope_id,"scope_files":attributed(&e.files,Source::ToolFileHash),"head_tree":attributed(&e.tree,Source::ToolGitRead),"delivery_identity":s.delivery,"sampled_delivery_identity":e.delivery_identity,"version_stale":stale,"report_identity_problem":missing,"eol_policy":attributed(&s.manifest.eol_policy,Source::HumanAttested),"outside_scope_notice":e.outside_scope_notice,"auto_repair_count":0,"manual_rounds_recorded":s.manual_rounds,"historical_rounds":"UNKNOWN","readiness_invalidated":s.invalidated,"registration_closed":s.closed,"closure_semantics":if s.closed{"CLOSED_BY_PM_REFERENCE [HUMAN_ATTESTED]"}else{"NOT_CLOSED"},"receipts":receipts,"owned_root_entries":root_files,"external_actions_executed":"NONE","execution_permission":"NOT_GRANTED_BY_SH1","handoff":handoff}),
        )
    }
}
fn check_layout(root: &Path) -> Result<()> {
    let mut count = 0;
    for entry in fs::read_dir(root).map_err(|_| "STORE_ENUMERATION")? {
        count += 1;
        if count > 16 {
            return Err("OWNED_LAYOUT_REJECTED");
        }
        let path = entry.map_err(|_| "STORE_ENUMERATION")?.path();
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or("OWNED_LAYOUT_REJECTED")?;
        if name == "backups" {
            let g = Guard::open(&path, true)?;
            if !g.path.is_dir() {
                return Err("OWNED_LAYOUT_REJECTED");
            }
        } else if ![
            "owner.json",
            "handoff.sqlite3",
            "handoff.sqlite3-wal",
            "handoff.sqlite3-shm",
            "handoff.sqlite3-journal",
        ]
        .contains(&name)
        {
            return Err("OWNED_LAYOUT_REJECTED");
        }
    }
    Ok(())
}
fn inventory(root: &Path) -> Result<Vec<Value>> {
    check_layout(root)?;
    let mut paths = vec![];
    for entry in fs::read_dir(root).map_err(|_| "STORE_ENUMERATION")? {
        let p = entry.map_err(|_| "STORE_ENUMERATION")?.path();
        if p.file_name().and_then(|s| s.to_str()) == Some("backups") {
            for x in fs::read_dir(&p).map_err(|_| "STORE_ENUMERATION")? {
                if paths.len() >= 64 {
                    return Err("STORE_INVENTORY_BOUND");
                }
                paths.push(x.map_err(|_| "STORE_ENUMERATION")?.path());
            }
        } else {
            paths.push(p)
        }
    }
    paths.sort();
    let mut result = vec![];
    for p in paths {
        let name = p
            .strip_prefix(root)
            .map_err(|_| "OWNED_LAYOUT_REJECTED")?
            .to_string_lossy()
            .into_owned();
        match read_file(&p,8*1024*1024) {
            Ok((b,_))=>result.push(json!({"name":name,"bytes":b.len(),"sha256":hash(&b),"state":"PRESENT","source":"TOOL_FILE_HASH","semantics":"READ_BYTES_NOT_SQLITE_TRANSACTION_SNAPSHOT"})),
            Err(_)=>result.push(json!({"name":name,"bytes":null,"sha256":null,"state":"UNKNOWN"})),
        }
    }
    Ok(result)
}
trait PipeJson {
    fn pipe_json<T: serde::de::DeserializeOwned>(self) -> Result<T>;
}
impl PipeJson for String {
    fn pipe_json<T: serde::de::DeserializeOwned>(self) -> Result<T> {
        serde_json::from_str(&self).map_err(|_| "STORE_INCONSISTENT")
    }
}
fn snapshot(c: &Connection) -> Result<Snapshot> {
    let s=sql(c.query_row("SELECT manifest_json,manifest_sha256,task_id,revision,current_delivery_identity,base_sha,base_source,phase,readiness_invalidated,registration_closed,manual_rounds_recorded,current_envelope_id FROM scheduler_packages",[],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?,r.get::<_,String>(6)?,r.get::<_,String>(7)?,r.get::<_,bool>(8)?,r.get::<_,bool>(9)?,r.get::<_,i64>(10)?,r.get::<_,String>(11)?))))?;
    if hash(s.0.as_bytes()) != s.1 {
        return Err("STORE_INCONSISTENT");
    }
    let manifest: Manifest = json(s.0.as_bytes())?;
    manifest.validate()?;
    let state = sql(
        c.query_row("SELECT state FROM tasks WHERE id=?1", [&s.2], |r| {
            r.get::<_, String>(0)
        }),
    )?
    .parse::<TaskState>()
    .map_err(|_| "STORE_INCONSISTENT")?;
    if policy::phase(state)? != s.7
        || matches!(
            state,
            TaskState::PreviewReady | TaskState::SafeFixing | TaskState::Running
        )
    {
        return Err("STORE_INCONSISTENT");
    }
    Ok(Snapshot {
        manifest,
        task_id: s.2,
        revision: s.3,
        delivery: s.4,
        base: s.5,
        base_source: s.6,
        phase: s.7,
        invalidated: s.8,
        closed: s.9,
        manual_rounds: s.10,
        current_envelope: s.11,
    })
}
fn step(db: &Database, task: &str, to: TaskState, actor: Actor, correlation: &str) -> Result<()> {
    let from = product(db.task(task))?.state;
    if to == TaskState::Completed || !is_legal_transition(from, to) {
        return Err("TRANSITION_REJECTED");
    }
    product(db.transition_task_in_transaction(
        task,
        to,
        actor,
        Some("SH1 material registration only"),
        Some(correlation),
        false,
        Some("No external execution or user acceptance inferred"),
    ))?;
    Ok(())
}
fn save_envelope(c: &Connection, m: &Manifest, e: &Envelope) -> Result<()> {
    sql(c.execute(
        "INSERT OR IGNORE INTO scheduler_envelopes VALUES(?1,?2,?3,?4,?5,?6)",
        params![
            e.envelope_id,
            m.package_id,
            encoded(e)?,
            e.head,
            e.observed_utc,
            e.delivery_identity
        ],
    ))?;
    Ok(())
}
fn load_candidates(c: &Connection, delivery: &str, pm: bool) -> Result<Vec<Candidate>> {
    let mut stmt=sql(c.prepare("SELECT receipt_id,metadata_json FROM scheduler_receipts WHERE mode='CURRENT' AND delivery_identity=?1 ORDER BY rowid"))?;
    let rows = sql(stmt.query_map([delivery], |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
    }))?;
    let mut out = vec![];
    for row in rows {
        let (receipt_id, s) = sql(row)?;
        let report: Report = json(s.as_bytes())?;
        if (report.report_type == ReportType::PmDecision) == pm {
            out.push(Candidate { receipt_id, report })
        }
    }
    Ok(out)
}
fn validate_source(c: &Connection, m: &Manifest, r: &Report) -> Result<Option<Guard>> {
    if let Some(src) = &r.source_report {
        let source=sql(c.query_row("SELECT mode,source_receipt_id,file_path,file_sha256 FROM scheduler_receipts WHERE receipt_id=?1 AND package_id=?2",params![src.receipt_id,m.package_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?))).optional())?.ok_or("DERIVATION_REJECTED")?;
        if source.0 != "INDEX_ONLY" || source.1.is_some() {
            return Err("DERIVATION_REJECTED");
        }
        let mut guard = Guard::open(Path::new(&source.2), false)?;
        let bytes = guard.bytes(REPORT_LIMIT)?;
        if hash(&bytes) != source.3 {
            return Err("SOURCE_CHANGED");
        }
        // An INDEX_ONLY file containing a derivative is not an eligible original.
        if Report::parse(&bytes).is_ok_and(|x| x.source_report.is_some()) {
            return Err("DERIVATION_REJECTED");
        }
        return Ok(Some(guard));
    }
    Ok(None)
}
fn validate_resolution(
    r: &Resolution,
    cs: &[Candidate],
    rs: &[Candidate],
    delivery: &str,
) -> Result<()> {
    use std::collections::BTreeSet;
    let actual: BTreeSet<_> = cs
        .iter()
        .filter(|c| c.report.report_type == r.report_type)
        .map(|c| c.receipt_id.clone())
        .collect();
    let supplied: BTreeSet<_> = r.candidate_receipt_ids.iter().cloned().collect();
    if r.delivery_identity != delivery
        || actual != supplied
        || !actual.contains(&r.selected_receipt_id)
    {
        return Err("RESOLUTION_REJECTED");
    }
    let prior = rs.iter().rev().find(|c| {
        c.report
            .context
            .slot_resolution
            .as_ref()
            .is_some_and(|x| x.report_type == r.report_type)
    });
    if r.supersedes_resolution_receipt_id.as_deref() != prior.map(|c| c.receipt_id.as_str()) {
        return Err("RESOLUTION_CONFLICT");
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn test_init_fault(root: &Path, m: &Manifest, e: &Envelope, point: u8) -> Result<Store> {
    Store::init_stage(root, m, e, point)
}
#[cfg(test)]
thread_local! {static IMPORT_FAILURE:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};}
#[cfg(test)]
pub(super) fn test_fail_import(s: &Store, e: &Envelope, input: &Import) -> Result<Value> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            IMPORT_FAILURE.with(|v| v.set(false));
        }
    }
    IMPORT_FAILURE.with(|v| v.set(true));
    let _reset = Reset;
    s.import(&s.snapshot()?, e, input)
}
#[cfg(test)]
pub(super) fn test_constraints(s: &Store) -> Result<()> {
    let db = s.write_connection()?;
    if db.conn().execute("INSERT INTO scheduler_packages(package_id,project_id,task_id,manifest_json,manifest_sha256,policy_id,phase,revision,base_sha,base_source,current_delivery_identity) VALUES('bad','missing','missing','{}','x','INTERNAL_MANUAL_V1','HANDOFF_READY',0,'0000000000000000000000000000000000000000','TOOL_GIT_READ','0000000000000000000000000000000000000000000000000000000000000000')",[]).is_ok(){return Err("FOREIGN_KEYS_DISABLED")}
    for table in [
        "task_actions",
        "task_reviews",
        "self_checks",
        "user_acceptance",
    ] {
        let count: i64 =
            sql(db
                .conn()
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)))?;
        if count != 0 {
            return Err("PRODUCT_FACT_WRITTEN");
        }
    }
    Ok(())
}
