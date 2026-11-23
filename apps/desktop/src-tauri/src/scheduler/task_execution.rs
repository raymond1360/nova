//! Dedicated Windows fixture only. Models never receive filesystem or process capabilities.
//! State/HTTP admission belongs to Store and relay; this module owns bounded local facts/I/O.
use super::{
    contract::*,
    store::{Guard, Store},
    Result,
};
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{self, File, OpenOptions},
    io::{BufRead, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

const NOTICE: &str = "NON_ATOMIC_IN_PLACE_V1";
pub(crate) const MATERIAL: &str = "DEEPSEEK_TASK_REVIEW_V1";
fn bytes<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|_| "ENCODING")
}
fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|_| "TASK_FILE_IO")
}
fn sql<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|_| "TASK_DATABASE_REJECTED")
}

/// Pin every ancestor, not just the leaf. Guard checks reparse points and denies delete sharing.
use crate::execution_safety::{parents,ExclusiveTarget};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskRequest {
    pub fixture_root: PathBuf,
    pub task_id: String,
    pub target: String,
    pub old_text: String,
    pub new_text: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TaskPlan {
    pub request: TaskRequest,
    pub fixture_identity: String,
    pub database_identity: String,
    pub task: Value,
    pub root_identity: String,
    pub target_identity: String,
    pub head: String,
    pub tree: String,
    pub branch: String,
    pub snapshot: String,
    pub policy: Value,
}
impl TaskPlan {
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(hash(&bytes(self)?))
    }
    fn validate(&self) -> Result<()> {
        let r = &self.request;
        if !id(&r.task_id)
            || !relative(&r.target)
            || r.target.starts_with('.')
            || r.target.split('/').any(|x| x.starts_with('.'))
            || self.snapshot.is_empty()
            || self.snapshot.len() > 8192
            || self.snapshot.contains('\0')
            || r.old_text.is_empty()
            || r.old_text.len() > 8192
            || r.new_text.len() > 16384
            || r.new_text == r.old_text
            || !hex(&self.head, 40)
            || !hex(&self.tree, 40)
            || !self.branch.starts_with("refs/heads/")
            || self.task["state"] != "confirmed"
            || self.policy != policy()
        {
            return Err("TASK_PLAN_REJECTED");
        }
        // Proposal's existing strict text grammar remains unchanged.
        Proposal {
            schema: "nova.relay.proposal/1".into(),
            target_path: r.target.clone(),
            old_text: r.old_text.clone(),
            new_text: r.new_text.clone(),
            rationale: String::new(),
        }
        .validate_target(&r.target)?;
        let c = replace_once(&self.snapshot, &r.old_text, &r.new_text)?;
        if c.len() > 65536 {
            return Err("BOUND");
        }
        super::store::safe_location(&r.fixture_root)?;
        Ok(())
    }
    pub(crate) fn core(&self, delivery: String) -> Result<ApiPlan> {
        self.validate()?;
        Ok(ApiPlan {
            schema: "nova.scheduler.api-plan/1".into(),
            mode: "REAL_API".into(),
            package_id: format!("task-{}", self.request.task_id),
            registration_revision: 0,
            input_base_sha: self.head.clone(),
            input_delivery_identity: delivery,
            target_path: self.request.target.clone(),
            base_sha256: hash(self.snapshot.as_bytes()),
            goal: String::from_utf8(bytes(&self.task)?).map_err(|_| "ENCODING")?,
            constraints: vec![String::from_utf8(bytes(self)?).map_err(|_| "ENCODING")?, "DEVELOP_PROPOSAL_EXPLICIT_V1".into()],
            examples: vec![],
            protocol: "DEEPSEEK_CHAT_JSON_V1".into(),
            model_develop: "deepseek-flash".into(),
            model_review: "deepseek-flash".into(),
            limits: ApiLimits {
                stage_ms: 45000,
                total_ms: 120000,
                input_tokens: 8192,
                output_tokens: 4096,
            },
            pm_decision_ref: "TASK_LOCAL_EXPLICIT_APPROVAL".into(),
            plan_request_sha256: self.digest()?,
        })
    }
    // Absence retains the frozen legacy request encoding; new plans carry an explicit prompt version.
    fn historical_core(&self,stored:&ApiPlan)->Result<ApiPlan>{
        let mut expected=self.core(stored.input_delivery_identity.clone())?;
        if stored.constraints.len()==1 {expected.constraints.pop();}
        Ok(expected)
    }
    fn repo(&self) -> PathBuf {
        self.request.fixture_root.join("project")
    }
    fn target(&self) -> PathBuf {
        self.repo().join(&self.request.target)
    }
    pub(crate) fn store_root(&self) -> PathBuf {
        self.request
            .fixture_root
            .join(format!("task-ledger-{}", self.request.task_id))
    }
}
fn policy() -> Value {
    json!({"schema":"nova.task-start/1","source":"TOOL_GIT_READ","backend":"DEEPSEEK_DIRECT",
    "endpoint":"https://api.deepseek.com/chat/completions","models":["deepseek-flash","deepseek-flash"],"thinking":"disabled","stream":false,"response_format":"json_object",
    "max_tokens":4096,"max_requests":2,"repairs":0,"retries":0,"request_bytes":65536,"response_bytes":131072,"stage_ms":45000,"total_ms":120000,
    "proxy":"EXPLICIT_DIRECT","no_money_limit":true,"write_policy":NOTICE,"project_commands":[],"automatic_restore":false,"approval_ttl_seconds":900})
}
fn replace_once(base: &str, old: &str, new: &str) -> Result<String> {
    if old.is_empty() {
        return Err("TASK_REPLACEMENT_REJECTED");
    }
    let hits: Vec<_> = base
        .as_bytes()
        .windows(old.len())
        .enumerate()
        .filter(|(_, x)| *x == old.as_bytes())
        .map(|(i, _)| i)
        .collect();
    if hits.len() != 1 {
        return Err("TASK_REPLACEMENT_REJECTED");
    }
    let i = hits[0];
    Ok(format!("{}{}{}", &base[..i], new, &base[i + old.len()..]))
}

struct Fixture {
    _pins: Vec<Guard>,
    _lock: Option<File>,
    db: Connection,
    root: PathBuf,
    identity: String,
    db_identity: String,
}
impl Fixture {
    fn open(root: &Path) -> Result<Self> {
        Self::open_mode(root, true)
    }
    fn open_mode(root: &Path, lock_owner: bool) -> Result<Self> {
        super::store::safe_location(root)?;
        let mut pins = parents(&root.join("product.sqlite3"))?;
        let rg = Guard::open(root, true)?;
        if rg.path != root {
            return Err("TASK_FIXTURE_REJECTED");
        }
        let (marker, _) = super::store::read_file(&root.join("S1-ISOLATED-FIXTURE.txt"), 128)?;
        if marker != b"NOVA_S1_ISOLATED_ONLY\n" {
            return Err("TASK_FIXTURE_REJECTED");
        }
        let path = root.join("product.sqlite3");
        let dg = Guard::open(&path, false)?; // denies DB writers, not only cooperative new entrypoints
        for suffix in ["-wal", "-shm"] {
            let p = root.join(format!("product.sqlite3{suffix}"));
            if p.exists() {
                pins.push(Guard::open(&p, false)?);
            }
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true).create(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0).custom_flags(0x00200000);
        }
        let lock = if lock_owner {
            Some(
                opts.open(root.join("S1-OWNER.lock"))
                    .map_err(|_| "TASK_FIXTURE_BUSY")?,
            )
        } else {
            None
        };
        let db = sql(Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY,
        ))?;
        sql(db.execute_batch("PRAGMA query_only=ON; BEGIN;"))?;
        // v13 only adds API-only receipt/config tables; the frozen v12 Task contract is unchanged.
        if !matches!(sql(db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)))?, 12 | 13 | 14) {
            return Err("TASK_DATABASE_REJECTED");
        }
        let identity = rg.identity.clone();
        let db_identity = dg.identity.clone();
        pins.push(rg);
        pins.push(dg);
        Ok(Self {
            _pins: pins,
            _lock: lock,
            db,
            root: root.into(),
            identity,
            db_identity,
        })
    }
    fn task(&self, id: &str) -> Result<Value> {
        use crate::storage::workspace::Fields;
        let row=sql(self.db.query_row("SELECT t.project_id,t.state,t.title,t.goal,t.in_scope,t.not_in_scope,t.done_criteria,t.ai_kind,p.local_path,b.id,b.draft_id,b.revision,b.content_hash,b.selected_ai,d.conversation_id,d.origin_task_id,d.status,d.current_revision FROM tasks t JOIN nova_projects p ON p.id=t.project_id JOIN requirement_task_bindings b ON b.task_id=t.id AND b.project_id=t.project_id JOIN requirement_drafts d ON d.id=b.draft_id AND d.project_id=b.project_id WHERE t.id=?1",[id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,Fields{title_raw:r.get(2)?,goal_raw:r.get(3)?,in_scope_raw:r.get(4)?,not_in_scope_raw:r.get(5)?,done_criteria_raw:r.get(6)?,selected_ai:r.get(7)?},r.get::<_,String>(8)?,r.get::<_,String>(9)?,r.get::<_,String>(10)?,r.get::<_,i64>(11)?,r.get::<_,String>(12)?,r.get::<_,String>(13)?,r.get::<_,String>(14)?,r.get::<_,Option<String>>(15)?,r.get::<_,String>(16)?,r.get::<_,i64>(17)?))))?;
        let (
            project,
            state,
            fields,
            path,
            binding,
            draft,
            rev,
            h,
            ai,
            conversation,
            origin,
            status,
            current,
        ) = row;
        let (revision_fields,revision_hash,questions)=sql(self.db.query_row("SELECT title_raw,goal_raw,in_scope_raw,not_in_scope_raw,done_criteria_raw,selected_ai,content_hash,blocking_questions_json FROM requirement_revisions WHERE draft_id=?1 AND revision=?2 AND project_id=?3",rusqlite::params![draft,rev,project],|r|Ok((Fields{title_raw:r.get(0)?,goal_raw:r.get(1)?,in_scope_raw:r.get(2)?,not_in_scope_raw:r.get(3)?,done_criteria_raw:r.get(4)?,selected_ai:r.get(5)?},r.get::<_,String>(6)?,r.get::<_,String>(7)?))))?;
        if state != "confirmed"
            || status != "bound"
            || current != rev
            || fields != revision_fields
            || fields.selected_ai.as_deref() != Some(&ai)
            || hash(&bytes(&(1, &draft, rev, &fields))?) != h
            || revision_hash != h
            || questions != "[]"
            || Path::new(&path) != self.root.join("project")
        {
            return Err("TASK_BINDING_REJECTED");
        }
        let mut q=sql(self.db.prepare("SELECT field,item_index,item_text_redacted FROM confirmed_requirement_items WHERE task_id=?1 AND project_id=?2 ORDER BY field,item_index"))?;
        let frozen: Vec<(String, i64, String)> = sql(q
            .query_map(rusqlite::params![id, project], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            }))?
        .collect::<rusqlite::Result<_>>()
        .map_err(|_| "TASK_DATABASE_REJECTED")?;
        let mut expected = Vec::new();
        for (name, value) in [
            ("goal", &fields.goal_raw),
            ("in_scope", &fields.in_scope_raw),
            ("not_in_scope", &fields.not_in_scope_raw),
            ("done_criteria", &fields.done_criteria_raw),
        ] {
            if value.trim().is_empty() {
                return Err("TASK_BINDING_REJECTED");
            }
            for (i, line) in crate::storage::requirement_items::split_requirement_field(value)
                .into_iter()
                .enumerate()
            {
                expected.push((
                    name.to_string(),
                    i as i64,
                    crate::security::redact_sensitive_text(&line),
                ));
            }
        }
        expected.sort();
        if frozen.is_empty() || expected != frozen {
            return Err("TASK_BINDING_REJECTED");
        }
        let full = json!({"task_id":id,"project_id":project,"state":state,"fields":fields,"binding_id":binding,"draft_id":draft,"revision":rev,"content_hash":h,
            "selected_ai":ai,"conversation_id":conversation,"origin_task_id":origin,"frozen_requirements":frozen});
        if bytes(&full)?.len() > 16384 {
            return Err("BOUND");
        }
        static_scan(&full)?;
        Ok(full)
    }
}
fn static_scan(v: &Value) -> Result<()> {
    // This does not read a credential slot; known active credentials are scanned after approval.
    match v {
        Value::String(s) => {
            if crate::security::redact_sensitive_text(s) != *s {
                return Err("SECRET_ECHO");
            }
        }
        Value::Array(a) => {
            for x in a {
                static_scan(x)?;
            }
        }
        Value::Object(m) => {
            for (k, x) in m {
                static_scan(&json!(k))?;
                static_scan(x)?;
            }
        }
        _ => (),
    }
    Ok(())
}

/// Deliberately narrow fixture config grammar, not a general Git config parser.
/// Reject includes without following them. Keep the inspected file pinned until Git exits.
fn fs_metadata(p:&Path)->std::io::Result<std::fs::Metadata>{std::fs::symlink_metadata(p)}
fn git_config_guard(root: &Path) -> Result<Guard> {
    let reject = "TASK_GIT_CONFIG_UNSUPPORTED";
    // No Git discovery/indirection: this entry only accepts a standalone .git directory.
    let metadata=root.join(".git");
    let directory=Guard::open(&metadata,true).map_err(|_|reject)?;
    if !directory.path.is_dir() {return Err(reject);}
    for name in ["commondir", "gitdir", "config.worktree"] {
        match fs_metadata(&metadata.join(name)) {
            Ok(_) => return Err(reject),
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => (),
            Err(_) => return Err(reject),
        }
    }
    if root.join(".git/config.worktree").exists() {
        return Err(reject);
    }
    let mut g = Guard::open(&root.join(".git/config"), false).map_err(|_| reject)?;
    let raw = g.bytes(16384).map_err(|_| reject)?;
    let text = std::str::from_utf8(&raw).map_err(|_| reject)?;
    let mut core = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if line == "[core]" {
            core = true;
            continue;
        }
        let (key, value) = line.split_once('=').ok_or(reject)?;
        if !core || !matches!((key.trim(), value.trim()),
            ("repositoryformatversion", "0") | ("bare", "false")
            | ("filemode", "false" | "true") | ("logallrefupdates", "true")
            | ("symlinks", "false") | ("ignorecase", "true" | "false")
            | ("autocrlf", "false")) {
            return Err(reject);
        }
    }
    Ok(g)
}

/// Fixed argv plus pinned allowlisted local config; no repository-derived commands.
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    let _config = git_config_guard(root)?;
    let tool = Path::new(r"C:\Program Files\Git\cmd\git.exe");
    let _pin = Guard::tool(tool)?;
    let mut c = Command::new(tool);
    c.current_dir(root).env_clear();
    if let Some(v) = std::env::var_os("SystemRoot") {
        c.env("SystemRoot", v);
    }
    c.env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_TERMINAL_PROMPT", "0");
    c.args([
        "--no-pager",
        "--no-optional-locks",
        "--literal-pathspecs",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.hooksPath=NUL",
    ]);
    c.args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    let mut child = c.spawn().map_err(|_| "TASK_GIT_REJECTED")?;
    let stdout = child.stdout.take().ok_or("TASK_GIT_REJECTED")?;
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        stdout.take(262145).read_to_end(&mut b).map(|_| b)
    });
    let end = Instant::now() + Duration::from_secs(5);
    let result = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Ok(s.success()),
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(10)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break Err("TASK_GIT_REJECTED");
            }
        }
    };
    let b = reader
        .join()
        .map_err(|_| "TASK_GIT_REJECTED")?
        .map_err(|_| "TASK_GIT_REJECTED")?;
    if !result? || b.len() > 262144 {
        return Err("TASK_GIT_REJECTED");
    }
    Ok(b)
}
fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    String::from_utf8(git(root, args)?)
        .map(|s| s.trim_end_matches(['\r', '\n']).into())
        .map_err(|_| "TASK_GIT_REJECTED")
}
fn git_facts(repo: &Path, target: &str, changed: bool) -> Result<(String, String, String)> {
    if !repo.join(".git").is_dir() || repo.join(".gitmodules").exists() {
        return Err("TASK_GIT_REJECTED");
    }
    let _g = Guard::open(&repo.join(".git"), true)?;
    // worktrees and submodules use a .git indirection file and are deliberately unsupported.
    let root = git_text(repo, &["rev-parse", "--show-toplevel"])?;
    if Guard::open(Path::new(&root), true)?.path != repo {
        return Err("TASK_GIT_REJECTED");
    }
    let branch = git_text(repo, &["symbolic-ref", "HEAD"])?;
    let head = git_text(repo, &["rev-parse", "--verify", "HEAD"])?;
    let tree = git_text(repo, &["rev-parse", "--verify", "HEAD^{tree}"])?;
    let tracked = git(repo, &["ls-files", "--stage", "-z", "--", target])?;
    let tracked = std::str::from_utf8(&tracked).map_err(|_| "TASK_GIT_REJECTED")?;
    if !tracked.starts_with("100644 ")
        || !tracked.ends_with(&format!(" 0\t{target}\0"))
        || tracked.matches('\0').count() != 1
    {
        return Err("TASK_GIT_REJECTED");
    }
    let state = git(
        repo,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ],
    )?;
    let expected = if changed {
        format!(" M {target}\0").into_bytes()
    } else {
        vec![]
    };
    if state != expected {
        return Err("TASK_SCOPE_CHANGED");
    }
    if changed
        && git(
            repo,
            &[
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--name-only",
                "-z",
                "HEAD",
                "--",
            ],
        )? != format!("{target}\0").as_bytes()
    {
        return Err("TASK_SCOPE_CHANGED");
    }
    Ok((head, tree, branch))
}

pub fn prepare(request: TaskRequest) -> Result<TaskPlan> {
    let f = Fixture::open(&request.fixture_root)?;
    collect(&f, request)
}
fn collect(f: &Fixture, request: TaskRequest) -> Result<TaskPlan> {
    if !id(&request.task_id)
        || !relative(&request.target)
        || request.target.split('/').any(|p| p.starts_with('.'))
    {
        return Err("TASK_PATH_CHANGED");
    }
    let repo = f.root.join("project");
    super::store::safe_location(&repo.join(&request.target))?;
    crate::runtime::path_guard::validate_directory_boundary(&repo, &f.root)
        .map_err(|_| "TASK_PATH_CHANGED")?;
    let pins = parents(&repo.join(&request.target))?;
    let rg = Guard::open(&repo, true)?;
    let mut fg = Guard::open(&repo.join(&request.target), false)?;
    let snapshot = String::from_utf8(fg.bytes(8192)?).map_err(|_| "TASK_ENCODING_REJECTED")?;
    let (head, tree, branch) = git_facts(&repo, &request.target, false)?;
    let p = TaskPlan {
        task: f.task(&request.task_id)?,
        fixture_identity: f.identity.clone(),
        database_identity: f.db_identity.clone(),
        root_identity: rg.identity.clone(),
        target_identity: fg.identity.clone(),
        head,
        tree,
        branch,
        snapshot,
        request,
        policy: policy(),
    };
    p.validate()?;
    static_scan(&serde_json::to_value(&p).map_err(|_| "ENCODING")?)?;
    for g in &pins {
        g.verify()?;
    }
    Ok(p)
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskRecord {
    pub plan: TaskPlan,
    pub digest: String,
    pub core: ApiPlan,
    pub approval_id: String,
    pub expires_utc: String,
}
impl TaskRecord {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.plan.digest()? != self.digest
            || uuid::Uuid::parse_str(&self.approval_id).is_err()
            || bytes(&self.plan.historical_core(&self.core)?)?
                != bytes(&self.core)?
            || chrono::DateTime::parse_from_rfc3339(&self.expires_utc).is_err()
        {
            return Err("TASK_APPROVAL_REJECTED");
        }
        Ok(())
    }
}
pub fn approve_dialog<R: std::io::BufRead, W: Write>(
    request: TaskRequest,
    input: &mut R,
    out: &mut W,
) -> Result<Value> {
    approve_with(
        request,
        input,
        out,
        super::deepseek_transport::scan_task_approval,
    )
}
fn approve_with<R: std::io::BufRead, W: Write>(
    request: TaskRequest,
    input: &mut R,
    out: &mut W,
    scan: impl FnOnce(&TaskPlan) -> Result<()>,
) -> Result<Value> {
    let f = Fixture::open(&request.fixture_root)?;
    let p = collect(&f, request)?;
    let digest = p.digest()?;
    io(writeln!(out,"DeepSeek 产品任务一次批准 / one-file task approval\n{}\nSnapshot (full):\n{}\nSnapshot SHA256: {}\nPlan digest: {}\nDEVELOP: full frozen requirement + snapshot + allowed replacement. REVIEW: same full requirement, original, Proposal, candidate and diff.\n最多两次串行请求；零整改/重试；独立上下文；无金额上限；每阶段45秒/总120秒；输出4096 tokens；请求65536/响应131072 bytes。\n独占句柄原位写入并flush，不保证原子可见性；中断可能留下部分内容。项目外备份仅供另获授权后人工恢复，不自动恢复/重发。不提交、不发布、不执行shell、不完成Task。\nEXPLICIT_DIRECT 不使用代理；本机若要求组织代理，不应批准。发送后取消仍可能执行或计费。\n输入同意外发、费用、直连及非原子写入风险的固定文字：ALLOW DIRECT SEND AND NONATOMIC WRITE\n再输入上述 digest。",serde_json::to_string_pretty(&p).map_err(|_|"ENCODING")?,p.snapshot,hash(p.snapshot.as_bytes()),digest))?;
    let mut ack = String::new();
    io((&mut *input).take(4097).read_line(&mut ack))?;
    if ack.trim_end_matches(['\r', '\n']) != "ALLOW DIRECT SEND AND NONATOMIC WRITE" {
        return Err("TASK_APPROVAL_REJECTED");
    }
    let mut confirm = String::new();
    io((&mut *input).take(4097).read_line(&mut confirm))?;
    if confirm.trim_end_matches(['\r', '\n']) != digest {
        return Err("TASK_APPROVAL_REJECTED");
    }
    // Approved object stays in memory. Credential scan occurs only after explicit approval.
    scan(&p)?;
    initialize(&f, p)
}
fn initialize(f: &Fixture, p: TaskPlan) -> Result<Value> {
    if collect(f, p.request.clone())? != p {
        return Err("TASK_PLAN_CHANGED");
    }
    let ledger = p.store_root();
    if ledger.exists() {
        let s = Store::task_open(&ledger)?;
        if record(&s)?.digest != p.digest()? {
            return Err("TASK_ALREADY_ACCEPTED");
        }
        return status_store(&s);
    }
    let gate = f.root.join(format!("task-{}.accepted", p.request.task_id));
    let mut marker = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&gate)
        .map_err(|_| "TASK_ALREADY_ACCEPTED")?;
    io(marker.write_all(p.digest()?.as_bytes()))?;
    io(marker.sync_all())?;
    drop(marker);
    let _accepted_pin = Guard::open(&gate, false)?;
    let s = Store::task_init(&ledger, &p)?;
    status_store(&s)
}

/// Caller is the shared relay under the fixture ownership lease. Read-only revalidation only.
pub(crate) fn verify_plan(p: &TaskPlan) -> Result<()> {
    let f = Fixture::open_mode(&p.request.fixture_root, false)?;
    if collect(&f, p.request.clone())? != *p {
        return Err("TASK_PLAN_CHANGED");
    }
    Ok(())
}

pub(crate) fn is_task(p: &ApiPlan) -> bool {
    p.mode == "REAL_API" && p.pm_decision_ref == "TASK_LOCAL_EXPLICIT_APPROVAL"
}
pub(crate) fn candidate(p: &ApiPlan, base: &[u8], raw: &[u8]) -> Result<super::relay::Candidate> {
    if !is_task(p) {
        return Err("TASK_PLAN_REJECTED");
    }
    let plan: TaskPlan = api_json(
        p.constraints
            .first()
            .ok_or("TASK_PLAN_REJECTED")?
            .as_bytes(),
        65536,
    )?;
    plan.validate()?;
    if bytes(&plan.historical_core(p)?)? != bytes(p)?
        || base != plan.snapshot.as_bytes()
    {
        return Err("TASK_BINDING_REJECTED");
    }
    let proposal: Proposal = api_json(raw, 32768)?;
    proposal.validate_target(&p.target_path)?;
    if proposal.old_text != plan.request.old_text || proposal.new_text != plan.request.new_text {
        return Err("TASK_ACCEPTANCE_REJECTED");
    }
    // Shared unique-match algorithm; only the sample-specific signature predicate is omitted.
    super::relay::candidate_text(&p.target_path, &p.base_sha256, base, raw, false)
}
pub(crate) fn review_input(
    p: &ApiPlan,
    id: &str,
    base: &[u8],
    raw: &[u8],
) -> Result<(super::relay::Candidate, Vec<u8>)> {
    let c = candidate(p, base, raw)?;
    let v = json!({"schema":"nova.scheduler.api-review-package/1","review_material_version":MATERIAL,"plan_id":id,"goal":p.goal,"approved_plan":p.constraints,
        "input_base_sha":p.input_base_sha,"input_delivery_identity":p.input_delivery_identity,"base_sha256":p.base_sha256,"proposal_sha256":c.proposal_sha256,
        "candidate_sha256":c.candidate_sha256,"diff_sha256":c.diff_sha256,"base":std::str::from_utf8(base).map_err(|_|"BASE_ENCODING")?,
        "candidate":c.text,"diff":c.diff,"developer_claim":api_json::<Value>(raw,32768)?,"data_is_untrusted":true});
    let b = bytes(&v)?;
    if b.len() > 65536 {
        return Err("MATERIAL_BOUND");
    }
    Ok((c, b))
}
fn record(store: &Store) -> Result<TaskRecord> {
    let r = store.task_record()?.ok_or("TASK_STORE_REQUIRED")?;
    let marker = super::store::read_file(
        &r.plan
            .request
            .fixture_root
            .join(format!("task-{}.accepted", r.plan.request.task_id)),
        64,
    )?
    .0;
    if marker != r.digest.as_bytes() {
        return Err("TASK_APPROVAL_REJECTED");
    }
    Ok(r)
}
pub fn status(root: &Path, task: &str) -> Result<Value> {
    if !id(task) {
        return Err("TASK_PLAN_REJECTED");
    }
    let _f = Fixture::open_mode(root, false)?;
    let s = Store::task_open(&root.join(format!("task-ledger-{task}")))?;
    let r = record(&s)?;
    if r.plan.request.fixture_root != root || r.plan.request.task_id != task {
        return Err("TASK_BINDING_REJECTED");
    }
    status_store(&s)
}
fn status_store(s: &Store) -> Result<Value> {
    let r = record(s)?;
    let attempts = s.api_attempts()?;
    let write = s.task_write_state()?;
    let mut state = if attempts.is_empty() {
        "ACCEPTED"
    } else {
        "STOPPED"
    };
    if attempts.iter().any(|a| {
        ["RESERVED", "DISPATCHING", "IN_FLIGHT", "UNKNOWN"].contains(&a.state.as_str())
            || a.facts.as_ref().is_some_and(|f| !f.unknowns.is_empty())
            || a.evidence
                .as_ref()
                .is_some_and(|e| e["state"] != "COMPLETE")
    }) {
        state = "UNKNOWN";
    }
    let mut observed = "NOT_WRITTEN";
    if let Some((intent, stored, receipt)) = &write {
        state = "UNKNOWN";
        observed = "UNVERIFIED";
        // Recovery is read-only. Never call write/apply/API on either observed outcome.
        let (_, _, run, _) = s.api_plan()?;
        let backup_ok = super::store::read_file(&run.join("original-backup.json"), 32768)
            .is_ok_and(|(b, _)| {
                hash(&b) == intent["backup_sha256"]
                    && api_json::<Value>(&b, 32768).is_ok_and(|v| {
                        v["approval_digest"] == r.digest
                            && v["original"] == r.plan.snapshot
                            && v["sha256"] == r.core.base_sha256
                    })
            });
        let review_ok = attempts.iter().any(|a| {
            a.role == RelayRole::Review
                && a.attempt_id == intent["review_attempt"]
                && a.request_sha256 == intent["review_request_sha256"]
                && a.state == "RESPONSE_RECEIVED"
                && a.facts
                    .as_ref()
                    .is_some_and(|f| f.error_code == "NONE" && f.unknowns.is_empty())
                && a.evidence
                    .as_ref()
                    .is_some_and(|e| e["state"] == "COMPLETE")
                && a.model_claim.as_ref().is_some_and(|c| {
                    c["verdict"] == "PASS" && c["candidate_sha256"] == intent["candidate_sha256"]
                })
        });
        if backup_ok
            && review_ok
            && intent["approval_digest"] == r.digest
            && intent["approval_id"] == r.approval_id
            && intent["original_sha256"] == r.core.base_sha256
            && intent["task_id"] == r.plan.request.task_id
            && intent["file_identity"] == r.plan.target_identity
        {
            if let Ok((b, identity)) = super::store::read_file(&r.plan.target(), 65536) {
                if identity == r.plan.target_identity {
                    if hash(&b) == intent["candidate_sha256"] {
                        observed = "CANDIDATE_OBSERVED";
                    } else if b == r.plan.snapshot.as_bytes() {
                        observed = "ORIGINAL_OBSERVED";
                    } else {
                        observed = "PARTIAL_OR_OTHER_OBSERVED";
                    }
                    if stored == "VERIFIED"
                        && observed == "CANDIDATE_OBSERVED"
                        && receipt.as_ref().is_some_and(|v| {
                            v["candidate_sha256"] == intent["candidate_sha256"]
                                && v["task_id"] == r.plan.request.task_id
                        })
                        && verify_after(&r.plan).is_ok()
                    {
                        state = "TECHNICALLY_VERIFIED_AWAITING_USER";
                    }
                }
            }
        }
    }
    Ok(
        json!({"schema":"nova.task-execution-status/1","task_id":r.plan.request.task_id,"recorded_task_state":"confirmed","backend":"DEEPSEEK_DIRECT","legacy_selected_ai":r.plan.task["selected_ai"],
        "recorded_task_state_source":"APPROVED_TASK_SNAPSHOT","execution_state":state,"liveness":"unknown","approval_digest":r.digest,"facts":attempts,"write":write,"observed_file":observed,
        "automatic_retry":false,"automatic_restore":false,"next_execution_authorized":false,"write_policy":NOTICE}),
    )
}
fn verify_after(p: &TaskPlan) -> Result<()> {
    let f = Fixture::open_mode(&p.request.fixture_root, false)?;
    if f.identity != p.fixture_identity
        || f.db_identity != p.database_identity
        || f.task(&p.request.task_id)? != p.task
    {
        return Err("TASK_PLAN_CHANGED");
    }
    let rg = Guard::open(&p.repo(), true)?;
    if rg.identity != p.root_identity {
        return Err("TASK_PATH_CHANGED");
    }
    if git_facts(&p.repo(), &p.request.target, true)?
        != (p.head.clone(), p.tree.clone(), p.branch.clone())
    {
        return Err("TASK_GIT_CHANGED");
    }
    Ok(())
}
pub fn run(root: &Path, task: &str, enabled: bool, cancel: &AtomicBool) -> Result<Value> {
    if !enabled {
        return Err("REAL_API_NOT_AUTHORIZED");
    }
    super::deepseek_transport::run_task(root, task, cancel)
}
pub(crate) fn run_with(
    root: &Path,
    task: &str,
    enabled: bool,
    transport: &mut dyn super::api_adapter::ApiTransport,
    secrets: &mut dyn super::api_adapter::SecretProvider,
    clock: &dyn super::api_adapter::Clock,
    cancel: &AtomicBool,
) -> Result<Value> {
    run_with_fault(root, task, enabled, transport, secrets, clock, cancel, 0)
}
fn run_with_fault(
    root: &Path,
    task: &str,
    enabled: bool,
    transport: &mut dyn super::api_adapter::ApiTransport,
    secrets: &mut dyn super::api_adapter::SecretProvider,
    clock: &dyn super::api_adapter::Clock,
    cancel: &AtomicBool,
    fault: u8,
) -> Result<Value> {
    if !enabled {
        return Err("REAL_API_NOT_AUTHORIZED");
    }
    if !id(task) {
        return Err("TASK_PLAN_REJECTED");
    }
    let f = Fixture::open(root)?;
    let s = Store::task_open(&root.join(format!("task-ledger-{task}")))?;
    let r = record(&s)?;
    if r.plan.request.fixture_root != root || r.plan.request.task_id != task {
        return Err("TASK_BINDING_REJECTED");
    }
    // Even two completed HTTP attempts never grant a recovered process permission to apply.
    if !s.api_attempts()?.is_empty() || s.task_write_state()?.is_some() {
        return status_store(&s);
    }
    if chrono::DateTime::parse_from_rfc3339(&r.expires_utc).map_err(|_| "TASK_APPROVAL_REJECTED")?
        <= chrono::Utc::now()
    {
        return Err("TASK_APPROVAL_EXPIRED");
    }
    verify_plan(&r.plan)?;
    let origin = clock.now_ms();
    let result = super::relay::run_api_with(
        &s,
        transport,
        secrets,
        &mut super::api_adapter::InjectedCounter,
        clock,
        cancel,
    );
    if result.is_err() {
        return Err("TASK_EXECUTION_STOPPED_CHECK_STATUS");
    }
    let attempts = s.api_attempts()?;
    if attempts.len() != 2
        || attempts.iter().any(|a| {
            a.state != "RESPONSE_RECEIVED"
                || a.facts.as_ref().is_none_or(|f| {
                    f.error_code != "NONE" || !f.unknowns.is_empty() || !f.local_resources_released
                })
                || a.evidence.as_ref().is_none_or(|e| e["state"] != "COMPLETE")
        })
    {
        return status_store(&s);
    }
    let review = attempts
        .iter()
        .find(|a| a.role == RelayRole::Review)
        .ok_or("TASK_REVIEW_REJECTED")?;
    let claim: ReviewClaim = api_json(
        &bytes(review.model_claim.as_ref().ok_or("TASK_REVIEW_REJECTED")?)?,
        65536,
    )?;
    if claim.verdict != "PASS"
        || !claim.unknowns.is_empty()
        || claim.issues.iter().any(|i| i.blocking)
    {
        return status_store(&s);
    }
    if cancel.load(Ordering::Acquire) || clock.now_ms().saturating_sub(origin) >= 120000 {
        return Err("TASK_WRITE_NOT_AUTHORIZED");
    }
    let develop = attempts
        .iter()
        .find(|a| a.role == RelayRole::Develop)
        .ok_or("TASK_REVIEW_REJECTED")?;
    let c = candidate(
        &r.core,
        r.plan.snapshot.as_bytes(),
        &bytes(develop.model_claim.as_ref().ok_or("TASK_REVIEW_REJECTED")?)?,
    )?;
    claim.validate(&r.core.base_sha256, &c.candidate_sha256)?;
    verify_plan(&r.plan)?;
    let mut target = ExclusiveTarget::open(
        &r.plan.target(),
        &r.plan.target_identity,
        r.plan.snapshot.as_bytes(),
    )?;
    if f.task(task)? != r.plan.task {
        return Err("TASK_PLAN_CHANGED");
    }
    let backup = json!({"schema":"nova.task-original/1","task_id":task,"approval_digest":r.digest,"file_identity":r.plan.target_identity,"original":r.plan.snapshot,"sha256":r.core.base_sha256});
    if fault == 1 {
        return Err("TASK_BACKUP_FAILED");
    }
    s.api_save("original-backup.json", &bytes(&backup)?)?;
    let (_, _, run, run_identity) = s.api_plan()?;
    let evidence_pin = Guard::open(&run, true)?;
    if evidence_pin.identity != run_identity {
        return Err("EVIDENCE_ROOT_CHANGED");
    }
    let mut backup_pin = Guard::open(&run.join("original-backup.json"), false)?;
    let backup_bytes = backup_pin.bytes(32768)?;
    if backup_bytes != bytes(&backup)? {
        return Err("TASK_BACKUP_FAILED");
    }
    if fault == 2 {
        return Err("TASK_INTENT_FAILED");
    }
    let intent = json!({"schema":"nova.task-write-intent/1","task_id":task,"approval_digest":r.digest,"approval_id":r.approval_id,"file_identity":r.plan.target_identity,
        "original_sha256":r.core.base_sha256,"candidate_sha256":c.candidate_sha256,"diff_sha256":c.diff_sha256,"review_request_sha256":review.request_sha256,"review_attempt":review.attempt_id,"backup_sha256":hash(&backup_bytes),"write_policy":NOTICE});
    s.task_intent(&intent)?;
    if cancel.load(Ordering::Acquire) || clock.now_ms().saturating_sub(origin) >= 120000 {
        s.task_write_finish("UNKNOWN", &json!({"code":"CANCELLED_BEFORE_FILE_WRITE"}))?;
        drop(target);
        return status_store(&s);
    }
    let wrote = target.write(c.text.as_bytes(), fault);
    // No reopening during mutation: seek/write/truncate/flush and byte verification use this handle.
    drop(target);
    let post = (|| {
        wrote?;
        let mut g = Guard::open(&r.plan.target(), false)?;
        if g.identity != r.plan.target_identity || g.bytes(65536)? != c.text.as_bytes() {
            return Err("TASK_WRITE_UNKNOWN");
        }
        verify_after(&r.plan)?;
        if fault == 6 {
            return Err("TASK_RECEIPT_UNKNOWN");
        }
        let receipt = json!({"task_id":task,"candidate_sha256":c.candidate_sha256,"file_identity":r.plan.target_identity,"result":"TECHNICALLY_VERIFIED_AWAITING_USER"});
        s.task_write_finish("VERIFIED", &receipt)
    })();
    if post.is_err() {
        let _ = s.task_write_finish("UNKNOWN", &json!({"code":"TASK_WRITE_UNKNOWN"}));
    }
    status_store(&s)
}

#[cfg(all(test, windows))]
mod tests {
    use super::super::api_adapter::*;
    use super::*;
    use crate::{
        db::Database,
        models::CreateProjectRequest,
        storage::workspace::{Fields, Operation, Request},
    };
    use std::io::Cursor;
    fn dir() -> PathBuf {
        let p = std::env::current_dir()
            .unwrap()
            .join("target")
            .join("s1-isolated-tests")
            .join(uuid::Uuid::new_v4().to_string());
        fs::create_dir_all(&p).unwrap();
        p
    }
    fn setup() -> TaskRequest {
        let root = dir();
        let repo = root.join("project");
        fs::create_dir(&repo).unwrap();
        fs::write(
            root.join("S1-ISOLATED-FIXTURE.txt"),
            b"NOVA_S1_ISOLATED_ONLY\n",
        )
        .unwrap();
        fs::write(repo.join("note.txt"), "Alpha Ä\r\nend\r\n").unwrap();
        // Test fixture creation only, never the product's command capability.
        for args in [
            vec!["init", "-q"],
            vec!["-c", "core.autocrlf=false", "add", "note.txt"],
            vec![
                "-c",
                "user.name=fixture",
                "-c",
                "user.email=fixture@invalid",
                "-c",
                "core.hooksPath=NUL",
                "commit",
                "-qm",
                "fixture",
            ],
        ] {
            assert!(
                std::process::Command::new(r"C:\Program Files\Git\cmd\git.exe")
                    .current_dir(&repo)
                    .args(args)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
        }
        let db = Database::open(&root.join("product.sqlite3")).unwrap();
        db.migrate().unwrap();
        let project = db
            .create_project(&CreateProjectRequest {
                name: "S1 fixture".into(),
                local_path: repo.to_string_lossy().into(),
                source_type: "local".into(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap()
            .id;
        let req = |operation| Request {
            project_id: project.clone(),
            client_request_id: Some(uuid::Uuid::new_v4().to_string()),
            operation,
        };
        let c = db.workspace(&req(Operation::Open {}), &[]).unwrap()["conversation_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let d = db
            .workspace(
                &req(Operation::Create {
                    conversation_id: c.clone(),
                    origin_task_id: None,
                }),
                &[],
            )
            .unwrap()["draft_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let fields = Fields {
            title_raw: "one actual fixture task".into(),
            selected_ai: Some("claude_cli".into()),
            goal_raw: "Replace Alpha with beta. Preserve Ä, CRLF and the second line.".into(),
            in_scope_raw: "note.txt, one replacement".into(),
            not_in_scope_raw: "No shell, no other files".into(),
            done_criteria_raw: "Exact beta Ä\\r\\nend\\r\\n bytes".into(),
        };
        let saved = db
            .workspace(
                &req(Operation::Save {
                    conversation_id: c.clone(),
                    draft_id: d.clone(),
                    expected_revision: 1,
                    fields,
                }),
                &[],
            )
            .unwrap();
        let bound = db
            .workspace(
                &req(Operation::Confirm {
                    conversation_id: c,
                    draft_id: d,
                    expected_revision: 2,
                    content_hash: saved["content_hash"].as_str().unwrap().into(),
                    selected_ai: "claude_cli".into(),
                }),
                &[],
            )
            .unwrap();
        let task = bound["task_id"].as_str().unwrap().to_owned();
        drop(db);
        TaskRequest {
            fixture_root: root,
            task_id: task,
            target: "note.txt".into(),
            old_text: "Alpha".into(),
            new_text: "beta".into(),
        }
    }
    fn accept(r: &TaskRequest) -> TaskPlan {
        let p = prepare(r.clone()).unwrap();
        let f = Fixture::open(&r.fixture_root).unwrap();
        initialize(&f, p.clone()).unwrap();
        p
    }
    #[test]
    fn task_git_commondir_clean_process_rejected_before_any_sampling() {
        for driver in ["clean","process"] {
            let r=setup();let p=accept(&r);let repo=p.repo();
            let common=r.fixture_root.join("common.git");
            fn copy_tree(from:&Path,to:&Path){
                fs::create_dir(to).unwrap();
                for entry in fs::read_dir(from).unwrap(){let entry=entry.unwrap();let path=entry.path();let dest=to.join(entry.file_name());if path.is_dir(){copy_tree(&path,&dest);}else{fs::copy(path,dest).unwrap();}}
            }
            copy_tree(&repo.join(".git"),&common);
            let marker=r.fixture_root.join("effect.txt");
            let command=format!("[filter \"probe\"]\n {driver} = echo SIDE_EFFECT > {}; cat\n",marker.to_string_lossy().replace('\\',"/"));
            fs::write(common.join("config"),command).unwrap();
            fs::write(repo.join(".git/commondir"),common.to_string_lossy().as_bytes()).unwrap();
            fs::write(repo.join(".git/info/attributes"),b"note.txt filter=probe\n").unwrap();
            fs::write(p.target(),p.snapshot.replace("Alpha","Bravo")).unwrap();
            assert_eq!(git_config_guard(&repo).err(),Some("TASK_GIT_CONFIG_UNSUPPORTED"));
            assert_eq!(git_facts(&repo,"note.txt",false).unwrap_err(),"TASK_GIT_CONFIG_UNSUPPORTED");
            assert_eq!(verify_after(&p).unwrap_err(),"TASK_GIT_CONFIG_UNSUPPORTED");
            let mut scans=0;
            assert!(approve_with(r.clone(),&mut Cursor::new("NO\n"),&mut vec![],|_|{scans+=1;Ok(())}).is_err());
            assert_eq!(scans,0);
            assert!(!marker.exists());
            assert_eq!(fs::read(p.target()).unwrap(),p.snapshot.replace("Alpha","Bravo").as_bytes());
        }
    }
    #[test]
    fn task_git_config_commands_rejected_at_all_sampling_boundaries() {
        for extra in [
            "[filter \"probe\"]\n clean = echo SIDE_EFFECT > ../effect.txt; cat\n",
            "[filter \"probe\"]\n process = echo SIDE_EFFECT > ../effect.txt\n",
            "[include]\n path = ../included.config\n",
            "[includeIf \"gitdir:**\"]\n path = ../included.config\n",
            "[core]\n fsmonitor = echo SIDE_EFFECT > ../effect.txt\n",
        ] {
            let r = setup();
            let p = accept(&r);
            let repo = p.repo();
            fs::write(repo.join(".git/info/attributes"), b"note.txt filter=probe\n").unwrap();
            fs::write(repo.join("included.config"), b"[filter \"probe\"]\n clean = echo SIDE_EFFECT > ../effect.txt; cat\n").unwrap();
            let config = repo.join(".git/config");
            let mut f = OpenOptions::new().append(true).open(config).unwrap();
            f.write_all(extra.as_bytes()).unwrap();
            drop(f);
            let before = fs::read(p.target()).unwrap();
            let dirty = p.snapshot.replace("Alpha", "Bravo");
            assert_eq!(dirty.len(), p.snapshot.len());
            fs::write(p.target(), dirty).unwrap();
            assert_eq!(git_facts(&repo, "note.txt", false).unwrap_err(), "TASK_GIT_CONFIG_UNSUPPORTED");
            assert_eq!(git_facts(&repo, "note.txt", true).unwrap_err(), "TASK_GIT_CONFIG_UNSUPPORTED");
            assert_eq!(verify_after(&p).unwrap_err(), "TASK_GIT_CONFIG_UNSUPPORTED");
            let mut scans = 0;
            assert!(approve_with(r.clone(), &mut Cursor::new("NO\n"), &mut vec![], |_| { scans += 1; Ok(()) }).is_err());
            assert_eq!(scans, 0);
            let (mut h, mut k, c, stop) = ports();
            assert!(run_with(&r.fixture_root, &r.task_id, true, &mut h, &mut k, &c, &stop).is_err());
            assert_eq!((h.calls, k.reads), (0, 0));
            assert_eq!(fs::read(p.target()).unwrap(), p.snapshot.replace("Alpha", "Bravo").as_bytes());
            assert!(!r.fixture_root.join("effect.txt").exists());
            assert_ne!(before, fs::read(p.target()).unwrap());
        }
    }
    #[test]
    fn task_git_config_guard_pins_and_rejects_unsupported_sources() {
        let r = setup();
        let repo = r.fixture_root.join("project");
        let pin = git_config_guard(&repo).unwrap();
        assert!(OpenOptions::new().write(true).open(repo.join(".git/config")).is_err());
        drop(pin);
        fs::write(repo.join(".git/config.worktree"), b"[filter \"probe\"]\n process = forbidden\n").unwrap();
        assert_eq!(git(&repo, &["rev-parse", "HEAD"]).unwrap_err(), "TASK_GIT_CONFIG_UNSUPPORTED");
    }
    struct Keys {
        reads: usize,
    }
    impl SecretProvider for Keys {
        fn acquire(&mut self, _: RelayRole, _: &str) -> Result<SecretLease> {
            self.reads += 1;
            SecretLease::real(b"fake-s1-credential-only".to_vec())
        }
    }
    struct Http {
        calls: usize,
        mode: u8,
        seen: Vec<Value>,
    }
    impl ApiTransport for Http {
        fn send(
            &mut self,
            req: &ApiRequest,
            _: &SecretLease,
            permit: &super::super::store::ApiDispatchPermit,
            _: u64,
            _: &dyn Clock,
            _: &AtomicBool,
        ) -> TransportResult {
            assert!(permit.claim_once());
            assert!(!permit.claim_once());
            self.calls += 1;
            let wire: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(wire["model"], "deepseek-flash");
            assert_eq!(wire["stream"], false);
            let input: Value =
                serde_json::from_str(wire["messages"][1]["content"].as_str().unwrap()).unwrap();
            self.seen.push(input.clone());
            if self.mode == 4 {
                return TransportResult {
                    status: None,
                    body: vec![],
                    resources_released: false,
                    error: Some("DEADLINE"),
                };
            }
            let mut content = if req.meta.role == RelayRole::Develop {
                json!({"schema":"nova.relay.proposal/1","target_path":"note.txt","old_text":"Alpha","new_text":if self.mode==1 {"wrong"}else{"beta"},"rationale":if self.mode==3 {"fake-s1-credential-only"}else{"one replacement"}})
            } else {
                json!({"schema":"nova.relay.review/1","base_sha256":input["base_sha256"],"candidate_sha256":input["candidate_sha256"],"verdict":if self.mode==2 {"CHANGES_REQUIRED"}else{"PASS"},"issues":[],"unknowns":[]})
            };
            if req.meta.role==RelayRole::Develop {
                if self.mode==6 {content.as_object_mut().unwrap().remove("new_text");}
                if self.mode==7 {content["target_path"]=json!("wrong.txt");}
            }
            let mut envelope = json!({"id":"synthetic-s1-response","created":1794730000u64,"object":"chat.completion","model":"deepseek-flash","choices":[{"index":0,"finish_reason":"stop","message":{"role":"assistant","content":content.to_string()}}],"usage":{"prompt_tokens":64,"completion_tokens":32,"total_tokens":96,"prompt_cache_hit_tokens":0,"prompt_cache_miss_tokens":64}});
            if self.mode == 5 {
                envelope.as_object_mut().unwrap().remove("usage");
            }
            TransportResult {
                status: Some(200),
                body: bytes(&envelope).unwrap(),
                resources_released: true,
                error: None,
            }
        }
    }
    fn ports() -> (Http, Keys, MonotonicClock, AtomicBool) {
        (
            Http {
                calls: 0,
                mode: 0,
                seen: vec![],
            },
            Keys { reads: 0 },
            MonotonicClock::default(),
            AtomicBool::new(false),
        )
    }
    #[test]
    fn task_execution_exclusive_handle_and_path_preimage_guards() {
        let r = setup();
        let p = prepare(r.clone()).unwrap();
        let pinned = Guard::open(&p.target(), false).unwrap();
        assert!(
            ExclusiveTarget::open(&p.target(), &p.target_identity, p.snapshot.as_bytes()).is_err()
        );
        drop(pinned);
        assert!(ExclusiveTarget::open(&p.target(), "wrong-id", p.snapshot.as_bytes()).is_err());
        assert!(ExclusiveTarget::open(&p.target(), &p.target_identity, b"wrong").is_err());
        let mut target =
            ExclusiveTarget::open(&p.target(), &p.target_identity, p.snapshot.as_bytes()).unwrap();
        assert!(File::open(p.target()).is_err());
        assert!(fs::write(p.target(), "outside").is_err());
        // Windows permits metadata-only hardlink creation despite share_mode(0).
        // Enforce the actual contract: a newly linked target is rejected before mutation.
        let alias = p.repo().join("late-alias.txt");
        fs::hard_link(p.target(), &alias).unwrap();
        assert!(target.write(b"short\r\n", 0).is_err());
        drop(target);
        assert_eq!(fs::read(p.target()).unwrap(), p.snapshot.as_bytes());
        fs::remove_file(&alias).unwrap();
        let mut target =
            ExclusiveTarget::open(&p.target(), &p.target_identity, p.snapshot.as_bytes()).unwrap();
        assert!(fs::rename(p.repo(), r.fixture_root.join("moved")).is_err());
        target.write(b"short\r\n", 0).unwrap();
        assert_eq!(target.read().unwrap(), b"short\r\n");
        drop(target);
        assert_eq!(fs::read(p.target()).unwrap(), b"short\r\n");
    }
    #[test]
    fn task_execution_approval_display_rejection_zero_initialization() {
        let r = setup();
        let p = prepare(r.clone()).unwrap();
        for input in [
            String::new(),
            format!("NO\n{}\n", p.digest().unwrap()),
            "ALLOW DIRECT SEND AND NONATOMIC WRITE\nwrong\n".into(),
        ] {
            let mut display = vec![];
            let mut scans = 0;
            assert!(
                approve_with(r.clone(), &mut Cursor::new(input), &mut display, |_| {
                    scans += 1;
                    Ok(())
                })
                .is_err()
            );
            assert_eq!(scans, 0);
            assert!(!p.store_root().exists());
            let text = String::from_utf8(display).unwrap();
            assert!(text.contains(&p.snapshot));
            assert!(text.contains("非原子"));
            assert!(text.contains("claude_cli"));
            assert!(text.contains("DEEPSEEK_DIRECT"));
            assert!(text.contains("无金额上限"));
        }
    }
    #[test]
    fn task_execution_approval_uses_displayed_object_and_single_task_ledger() {
        let r = setup();
        let p = prepare(r.clone()).unwrap();
        let digest = p.digest().unwrap();
        let input = format!("ALLOW DIRECT SEND AND NONATOMIC WRITE\n{digest}\n");
        let out = approve_with(r.clone(), &mut Cursor::new(input), &mut vec![], |shown| {
            assert_eq!(*shown, p);
            Ok(())
        })
        .unwrap();
        assert_eq!(out["execution_state"], "ACCEPTED");
        let f = Fixture::open(&r.fixture_root).unwrap();
        assert!(initialize(&f, p.clone()).is_ok());
        let mut changed = p.clone();
        changed.request.new_text = "other".into();
        assert!(initialize(&f, changed).is_err());
        assert!(Fixture::open(&r.fixture_root).is_err());
    }
    #[test]
    fn task_execution_two_contexts_exact_write_and_read_only_reopen() {
        let r = setup();
        let p = accept(&r);
        let original_db = fs::read(r.fixture_root.join("product.sqlite3")).unwrap();
        let (mut http, mut keys, clock, stop) = ports();
        let out = run_with(
            &r.fixture_root,
            &r.task_id,
            true,
            &mut http,
            &mut keys,
            &clock,
            &stop,
        )
        .unwrap();
        assert_eq!(
            out["execution_state"], "TECHNICALLY_VERIFIED_AWAITING_USER",
            "{out}"
        );
        assert_eq!(http.calls, 2);
        assert_eq!(http.seen[1]["review_material_version"], MATERIAL);
        assert!(http.seen[1]["goal"]
            .as_str()
            .unwrap()
            .contains("Preserve Ä"));
        assert_eq!(http.seen[1]["candidate"], "beta Ä\r\nend\r\n");
        assert_eq!(
            fs::read(p.target()).unwrap(),
            "beta Ä\r\nend\r\n".as_bytes()
        );
        assert_eq!(out["recorded_task_state"], "confirmed");
        assert_eq!(out["legacy_selected_ai"], "claude_cli");
        assert_eq!(out["backend"], "DEEPSEEK_DIRECT");
        assert_eq!(
            fs::read(r.fixture_root.join("product.sqlite3")).unwrap(),
            original_db
        );
        let before = keys.reads;
        run_with(
            &r.fixture_root,
            &r.task_id,
            true,
            &mut http,
            &mut keys,
            &clock,
            &stop,
        )
        .unwrap();
        assert_eq!(http.calls, 2);
        assert_eq!(keys.reads, before);
        assert_eq!(
            status(&r.fixture_root, &r.task_id).unwrap()["liveness"],
            "unknown"
        );
    }
    #[test]
    fn task_execution_backup_intent_and_write_faults_never_replay() {
        for fault in 1..=6 {
            let r = setup();
            let p = accept(&r);
            let (mut http, mut keys, clock, stop) = ports();
            let result = run_with_fault(
                &r.fixture_root,
                &r.task_id,
                true,
                &mut http,
                &mut keys,
                &clock,
                &stop,
                fault,
            );
            if fault <= 2 {
                assert!(result.is_err());
                assert_eq!(fs::read(p.target()).unwrap(), p.snapshot.as_bytes());
            } else {
                assert_eq!(result.unwrap()["execution_state"], "UNKNOWN");
            }
            let file = fs::read(p.target()).unwrap();
            let reads = keys.reads;
            run_with(
                &r.fixture_root,
                &r.task_id,
                true,
                &mut http,
                &mut keys,
                &clock,
                &stop,
            )
            .unwrap();
            assert_eq!(http.calls, 2);
            assert_eq!(keys.reads, reads);
            assert_eq!(fs::read(p.target()).unwrap(), file);
        }
    }
    #[test]
    fn task_execution_failure_cancel_secrets_and_usage_stop_without_review() {
        for mode in 1..=7 {
            let r = setup();
            let p = accept(&r);
            let (mut http, mut keys, clock, stop) = ports();
            http.mode = mode;
            let out = run_with(
                &r.fixture_root,
                &r.task_id,
                true,
                &mut http,
                &mut keys,
                &clock,
                &stop,
            )
            .unwrap();
            assert_ne!(out["execution_state"], "TECHNICALLY_VERIFIED_AWAITING_USER");
            if [1,6,7].contains(&mode) {
                let expected=match mode {1=>"PROPOSAL_DERIVED_MATERIAL",6=>"PROPOSAL_DECODE",_=>"PROPOSAL_VALIDATION"};
                assert_eq!(out["facts"][0]["facts"]["diagnostic"],expected);
                assert!(out["facts"][0]["facts"]["response_sha256"].is_null());
                assert!(out["facts"][0]["model_claim"].is_null());
            }

            assert_eq!(http.calls, if mode == 2 { 2 } else { 1 });
            assert_eq!(fs::read(p.target()).unwrap(), p.snapshot.as_bytes());
            for entry in
                fs::read_dir(r.fixture_root.join(format!("task-evidence-{}", r.task_id))).unwrap()
            {
                let path = entry.unwrap().path();
                if path.is_file() {
                    assert!(!String::from_utf8_lossy(&fs::read(path).unwrap())
                        .contains("fake-s1-credential-only"));
                }
            }
            let n = http.calls;
            run_with(
                &r.fixture_root,
                &r.task_id,
                true,
                &mut http,
                &mut keys,
                &clock,
                &stop,
            )
            .unwrap();
            assert_eq!(http.calls, n);
        }
        let r = setup();
        accept(&r);
        let (mut http, mut keys, clock, stop) = ports();
        stop.store(true, Ordering::Release);
        assert!(run_with(
            &r.fixture_root,
            &r.task_id,
            true,
            &mut http,
            &mut keys,
            &clock,
            &stop
        )
        .is_err());
        assert_eq!((http.calls, keys.reads), (0, 0));
    }
    #[test]
    fn task_execution_drift_missing_approval_and_default_closed_zero_send() {
        let r = setup();
        let (mut http, mut keys, clock, stop) = ports();
        assert!(run_with(
            &r.fixture_root,
            &r.task_id,
            false,
            &mut http,
            &mut keys,
            &clock,
            &stop
        )
        .is_err());
        assert!(run_with(
            &r.fixture_root,
            &r.task_id,
            true,
            &mut http,
            &mut keys,
            &clock,
            &stop
        )
        .is_err());
        let p = accept(&r);
        fs::write(p.target(), "drift").unwrap();
        assert!(run_with(
            &r.fixture_root,
            &r.task_id,
            true,
            &mut http,
            &mut keys,
            &clock,
            &stop
        )
        .is_err());
        assert_eq!((http.calls, keys.reads), (0, 0));
    }
    #[test]
    fn task_execution_material_tamper_paths_and_unicode_secret_rejected() {
        let r = setup();
        let p = prepare(r.clone()).unwrap();
        let core = p.core(p.digest().unwrap()).unwrap();
        // Serialization-only synthetic approved values, including quotes, slash, LF and Unicode.
        let mut quoted=p.clone();
        quoted.request.target="text/dynamic.txt".into();
        quoted.snapshot="before \"x\"\\tail\n after".into();
        quoted.request.old_text="\"x\"\\tail\n".into();
        quoted.request.new_text="\"Ä\"\\next\n".into();
        let explicit=quoted.core(quoted.digest().unwrap()).unwrap();
        let input=super::super::relay::api_develop_input(&explicit,quoted.snapshot.as_bytes()).unwrap();
        let req=super::super::deepseek_transport::reconstruct_request(RelayRole::Develop,&explicit,"synthetic",&input,None).unwrap();
        let wire:Value=serde_json::from_slice(&req.body).unwrap();
        let system=wire["messages"][0]["content"].as_str().unwrap();
        for rule in ["six required String fields","no additional fields","nova.relay.proposal/1","8192","16384","2048","nonempty","U+000D","U+0000","not the whole file","CRLF"] {assert!(system.contains(rule));}
        let example:Proposal=serde_json::from_str(system.split_once("Example: ").unwrap().1).unwrap();
        assert_eq!(example.target_path,quoted.request.target);assert_eq!(example.old_text,quoted.request.old_text);assert_eq!(example.new_text,quoted.request.new_text);
        example.validate_target(&quoted.request.target).unwrap();assert!(!system.contains(API_TARGET));
        let mut legacy=explicit.clone();legacy.constraints.pop();
        assert_eq!(bytes(&quoted.historical_core(&legacy).unwrap()).unwrap(),bytes(&legacy).unwrap());
        let old_input=super::super::relay::api_develop_input(&legacy,quoted.snapshot.as_bytes()).unwrap();
        let old_req=super::super::deepseek_transport::reconstruct_request(RelayRole::Develop,&legacy,"synthetic",&old_input,None).unwrap();
        let old:Value=serde_json::from_slice(&old_req.body).unwrap();assert!(old["messages"][0]["content"].as_str().unwrap().contains("exact source"));

        let proposal = json!({"schema":"nova.relay.proposal/1","target_path":"note.txt","old_text":"Alpha","new_text":"beta","rationale":"ok"});
        let (_, material) = review_input(
            &core,
            "plan",
            p.snapshot.as_bytes(),
            &bytes(&proposal).unwrap(),
        )
        .unwrap();
        assert!(String::from_utf8(material)
            .unwrap()
            .contains("done_criteria_raw"));
        let mut forged = core.clone();
        forged.goal = "changed".into();
        assert!(review_input(
            &forged,
            "plan",
            p.snapshot.as_bytes(),
            &bytes(&proposal).unwrap()
        )
        .is_err());
        let mut traversal = r.clone();
        traversal.target = "../outside.txt".into();
        assert!(prepare(traversal).is_err());
        fs::hard_link(p.target(), p.repo().join("alias.txt")).unwrap();
        assert!(prepare(r.clone()).is_err());
        let secret = SecretLease::real(b"fake-s1-credential-only".to_vec()).unwrap();
        let escaped = r#"{"value":"\u0066ake-s1-credential-only"}"#;
        assert!(secret.scan_json(escaped.as_bytes(), 65536).is_err());
    }
    #[test]
    fn task_execution_real_storage_failure_zero_write_and_crash_after_review_no_apply() {
        for fail in 0..2 {
            let r = setup();
            let p = accept(&r);
            if fail == 0 {
                fs::write(
                    r.fixture_root
                        .join(format!("task-evidence-{}", r.task_id))
                        .join("original-backup.json"),
                    b"occupied",
                )
                .unwrap();
            } else {
                let c = Connection::open(p.store_root().join("handoff.sqlite3")).unwrap();
                c.execute_batch("CREATE TRIGGER fail_intent BEFORE INSERT ON task_write BEGIN SELECT RAISE(FAIL,'synthetic disk boundary'); END;").unwrap();
            }
            let (mut h, mut k, c, stop) = ports();
            assert!(
                run_with(&r.fixture_root, &r.task_id, true, &mut h, &mut k, &c, &stop).is_err()
            );
            assert_eq!(fs::read(p.target()).unwrap(), p.snapshot.as_bytes());
            assert_eq!(h.calls, 2);
        }
        let r = setup();
        let p = accept(&r);
        let (mut h, mut k, c, stop) = ports();
        {
            let _lease = Fixture::open(&r.fixture_root).unwrap();
            let s = Store::task_open(&p.store_root()).unwrap();
            super::super::relay::run_api_with(&s, &mut h, &mut k, &mut InjectedCounter, &c, &stop)
                .unwrap();
            assert!(s.task_write_state().unwrap().is_none());
        }
        let reads = k.reads;
        run_with(&r.fixture_root, &r.task_id, true, &mut h, &mut k, &c, &stop).unwrap();
        assert_eq!((h.calls, k.reads), (2, reads));
        assert_eq!(fs::read(p.target()).unwrap(), p.snapshot.as_bytes());
    }
    #[test]
    fn task_execution_concurrent_scope_ownership_and_versions_are_fail_closed() {
        let r = setup();
        let p = accept(&r);
        {
            let _f = Fixture::open(&r.fixture_root).unwrap();
            let other = r.fixture_root.clone();
            assert!(std::thread::spawn(move || Fixture::open(&other).is_err())
                .join()
                .unwrap());
            assert!(OpenOptions::new()
                .write(true)
                .open(r.fixture_root.join("product.sqlite3"))
                .is_err());
        }
        let original = fs::read(p.store_root().join("handoff.sqlite3")).unwrap();
        assert!(Store::deepseek_open(&p.store_root()).is_err());
        assert!(Store::api_open(&p.store_root()).is_err());
        assert!(Store::open(&p.store_root()).is_err());
        assert_eq!(
            fs::read(p.store_root().join("handoff.sqlite3")).unwrap(),
            original
        );
        let (mut h, mut k, c, stop) = ports();
        assert!(run_with(
            &r.fixture_root.join("different-run"),
            &r.task_id,
            true,
            &mut h,
            &mut k,
            &c,
            &stop
        )
        .is_err());
        assert_eq!((h.calls, k.reads), (0, 0));
        {
            let cdb = Connection::open(p.store_root().join("handoff.sqlite3")).unwrap();
            cdb.execute("UPDATE scheduler_packages SET readiness_invalidated=1", [])
                .unwrap();
            assert!(
                run_with(&r.fixture_root, &r.task_id, true, &mut h, &mut k, &c, &stop).is_err()
            );
            assert_eq!((h.calls, k.reads), (0, 0));
            cdb.execute("UPDATE scheduler_packages SET readiness_invalidated=0", [])
                .unwrap();
        }
        let db = Connection::open(r.fixture_root.join("product.sqlite3")).unwrap();
        db.execute(
            "UPDATE confirmed_requirement_items SET item_text_redacted='tampered' WHERE task_id=?1",
            [&r.task_id],
        )
        .unwrap();
        drop(db);
        assert!(run_with(&r.fixture_root, &r.task_id, true, &mut h, &mut k, &c, &stop).is_err());
        assert_eq!((h.calls, k.reads), (0, 0));
    }
}
