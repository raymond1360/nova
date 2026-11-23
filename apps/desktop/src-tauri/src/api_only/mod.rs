//! API-only product execution receipt. Task state remains owned by the existing Database.
pub mod commands;
pub mod config;
pub mod recipes;
pub mod task_loop;
pub mod transport;
use crate::{credential::CredentialStore, db::Database, execution_safety::*};
pub use config::{Config, ConfigView, Protocol, Provider};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
pub use transport::{ApiPort, DeepSeekHttp};
pub type Result<T> = std::result::Result<T, &'static str>;
pub const API_ONLY: bool = true;
fn encode<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|_| "API_ENCODING")
}
fn decode<T: serde::de::DeserializeOwned>(b: &[u8], cap: usize) -> Result<T> {
    api_json(b, cap)
}
fn sql<T>(r: rusqlite::Result<T>) -> Result<T> {
    r.map_err(|_| "API_DATABASE_ERROR")
}
fn safe_text(s: &str) -> Result<()> {
    if s.contains('\0') || crate::security::redact_sensitive_text(s) != s {
        return Err("API_SENSITIVE_MATERIAL");
    };
    Ok(())
}
fn permitted_path(s: &str) -> Result<()> {
    if !relative(s)
        || s.split('/').any(|p| {
            p.starts_with('.')
                || matches!(
                    p.to_ascii_lowercase().as_str(),
                    "credentials"
                        | "auth.json"
                        | "id_rsa"
                        | "id_ed25519"
                        | "secrets.json"
                        | "secrets.toml"
                )
                || p.to_ascii_lowercase().ends_with(".pem")
                || p.to_ascii_lowercase().ends_with(".key")
                || p.to_ascii_lowercase().ends_with(".pfx")
                || p.to_ascii_lowercase().ends_with(".p12")
        })
    {
        return Err("API_PATH_REJECTED");
    };
    Ok(())
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedTest {
    #[serde(default)]
    pub recipe: String,
    pub program: PathBuf,
    pub file: String,
    pub contains: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedTest {
    #[serde(default)]
    pub cwd: PathBuf,
    #[serde(default)]
    pub environment: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub argv: Vec<String>,
    #[serde(default)]
    pub trust_notice: String,
    pub specification: FixedTest,
    pub program_identity: String,
    pub program_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileSnapshot {
    pub path: String,
    pub identity: String,
    pub before: String,
    pub sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    #[serde(default)]
    pub approval_kind: String,
    #[serde(default)]
    pub allow_repair: bool,
    #[serde(default)]
    pub parent_run: Option<String>,
    #[serde(default)]
    pub prior_issues: Value,
    pub schema: String,
    pub id: String,
    pub task_id: String,
    pub project_id: String,
    pub root: PathBuf,
    pub root_identity: String,
    pub task: Value,
    pub task_digest: String,
    pub config: ConfigView,
    pub files: Vec<FileSnapshot>,
    pub tests: Vec<ApprovedTest>,
    pub expires_utc: i64,
    pub notice: String,
}
fn task_snapshot(db: &Database, id: &str) -> Result<Value> {
    let task = db.task(id).map_err(|_| "API_TASK_MISSING")?;
    if crate::storage::tasks::is_legacy_frozen(&task) {
        return Err("API_LEGACY_TASK_FROZEN");
    }
    if task.state != crate::models::TaskState::Confirmed
        && !(matches!(
            task.state,
            crate::models::TaskState::NeedsUserAction
                | crate::models::TaskState::AwaitingUserAcceptance
        ) && sql(db.conn().query_row(
            "SELECT count(*) FROM api_task_scopes WHERE task_id=?1",
            [id],
            |r| r.get::<_, i64>(0),
        ))? == 1)
    {
        return Err("API_CONFIRMED_TASK_REQUIRED");
    }
    let binding: Option<(String, i64, String)> = sql(db
        .conn()
        .query_row(
            "SELECT draft_id,revision,content_hash FROM requirement_task_bindings WHERE task_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional())?;
    let items = db
        .list_confirmed_requirement_items(id)
        .map_err(|_| "API_REQUIREMENT_READ_FAILED")?;
    let v = json!({"task":task,"binding":binding,"requirements":items});
    let raw = encode(&v)?;
    if raw.len() > 16384 {
        return Err("API_CONTEXT_BOUND");
    };
    safe_text(&raw)?;
    Ok(v)
}
fn tool_snapshot(t: &FixedTest, root: &Path, files: &[FileSnapshot]) -> Result<ApprovedTest> {
    if !files.iter().any(|f| f.path == t.file)
        || t.contains.is_empty()
        || t.contains.len() > 2048
        || !matches!(
            t.program
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str(),
            "python.exe" | "python3.exe" | "node.exe"
        )
    {
        return Err("API_FIXED_TEST_REJECTED");
    }
    safe_text(&t.contains)?;
    let (argv, environment, trust_notice, cwd) = recipes::freeze(t, root)?;
    let mut g = Guard::tool(&t.program)?;
    if inside(&g.path, root) {
        return Err("API_TEST_TOOL_INSIDE_PROJECT");
    }
    let size = std::fs::metadata(&g.path)
        .map_err(|_| "API_TEST_TOOL_INVALID")?
        .len();
    let digest = g.sha256_stream(size, Instant::now() + Duration::from_secs(5))?;
    Ok(ApprovedTest {
        cwd,
        argv,
        environment,
        trust_notice,
        specification: FixedTest {
            program: g.path.clone(),
            file: t.file.clone(),
            contains: t.contains.clone(),
            recipe: t.recipe.clone(),
        },
        program_identity: g.identity,
        program_sha256: digest,
    })
}
pub fn prepare(
    db: &Database,
    vault: &dyn CredentialStore,
    task_id: &str,
    config_id: &str,
    paths: Vec<String>,
    tests: Vec<FixedTest>,
) -> Result<Plan> {
    prepare_scoped(db, vault, task_id, config_id, paths, tests, false, None)
}

#[allow(clippy::too_many_arguments)]
pub fn prepare_scoped(
    db: &Database,
    vault: &dyn CredentialStore,
    task_id: &str,
    config_id: &str,
    paths: Vec<String>,
    tests: Vec<FixedTest>,
    allow_repair: bool,
    parent: Option<&str>,
) -> Result<Plan> {
    prepare_deadline(
        db,
        vault,
        task_id,
        config_id,
        paths,
        tests,
        allow_repair,
        parent,
        None,
    )
}
#[allow(clippy::too_many_arguments)]
fn prepare_deadline(
    db: &Database,
    vault: &dyn CredentialStore,
    task_id: &str,
    config_id: &str,
    paths: Vec<String>,
    tests: Vec<FixedTest>,
    allow_repair: bool,
    parent: Option<&str>,
    inherited: Option<i64>,
) -> Result<Plan> {
    let expires = inherited.unwrap_or_else(|| now_utc() + 3600);
    if expires <= now_utc() {
        return Err("API_APPROVAL_EXPIRED");
    }
    let c = config::load_config(db, config_id)?;
    c.view.config.require_adapter()?;
    if !c.view.has_key {
        return Err("API_KEY_MISSING");
    }
    if paths.is_empty() || paths.len() > 3 || tests.is_empty() || tests.len() > 2 {
        return Err("API_SCOPE_BOUND");
    }
    let original_paths = paths.clone();
    let task = task_snapshot(db, task_id)?;
    let project_id = task["task"]["project_id"]
        .as_str()
        .ok_or("API_TASK_INVALID")?
        .to_string();
    let project = db.project(&project_id).map_err(|_| "API_PROJECT_MISSING")?;
    let root = Guard::open(Path::new(&project.local_path), true)?;
    safe_location(&root.path)?;
    let mut seen = BTreeSet::new();
    let mut files = vec![];
    for path in paths {
        permitted_path(&path)?;
        if !seen.insert(path.to_ascii_lowercase()) {
            return Err("API_DUPLICATE_PATH");
        }
        let (raw, identity) = read_file(
            &root.path.join(&path),
            if parent.is_some() { 32768 } else { 8192 },
        )?;
        let before = String::from_utf8(raw).map_err(|_| "API_UTF8_REQUIRED")?;
        safe_text(&before)?;
        files.push(FileSnapshot {
            path,
            identity,
            sha256: hash(before.as_bytes()),
            before,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let tests = tests
        .iter()
        .map(|t| tool_snapshot(t, &root.path, &files))
        .collect::<Result<Vec<_>>>()?;
    let prior_issues = task_loop::repair_context(
        db,
        task_id,
        parent,
        &c.view,
        &original_paths,
        &tests,
        allow_repair,
    )?;
    let plan=Plan{approval_kind:if inherited.is_some(){"INHERITED"}else{"EXPLICIT"}.into(),allow_repair,parent_run:parent.map(str::to_string),prior_issues,schema:"nova.api.task/1".into(),id:uuid::Uuid::new_v4().to_string(),task_id:task_id.into(),project_id:project_id.clone(),root:root.path,root_identity:root.identity,task_digest:hash(encode(&task)?.as_bytes()),task,config:c.view.clone(),files,tests,expires_utc:expires,notice:"At most DEVELOP 1 + independent REVIEW 1, no automatic network retries. Approved files only; non-atomic in-place writes may be partial. Only the displayed fixed test recipe/argv/environment is approved; never model-generated shell. API requests expose this packet and incur provider charges. No automatic restore/completion/commit. A task-level one-repair grant permits at most 4 model requests total, same configuration/files/tests only; no additional tools or budget. Project unittest executes trusted project code and dependencies with user permissions, may write/spawn/access network; not a sandbox.".into()};
    let raw = encode(&plan)?;
    config::acquire(&c, vault)?.scan_json(raw.as_bytes(), 65536)?;
    if raw.len() > 49152 {
        return Err("API_CONTEXT_BOUND");
    }
    task_loop::insert_plan(db, &plan, &raw, parent)?;
    Ok(plan)
}
fn load_plan(db: &Database, id: &str) -> Result<(Plan, String, String, Value)> {
    let (raw, digest, phase, summary): (String, String, String, String) =
        sql(db.conn().query_row(
            "SELECT plan_json,digest,phase,summary_json FROM api_task_runs WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        ))?;
    if hash(raw.as_bytes()) != digest {
        return Err("API_PLAN_TAMPERED");
    };
    Ok((
        decode(raw.as_bytes(), 65536)?,
        digest,
        phase,
        decode(summary.as_bytes(), 2 * 1024 * 1024)?,
    ))
}
fn validate_root(db: &Database, p: &Plan) -> Result<()> {
    let project = db
        .project(&p.project_id)
        .map_err(|_| "API_PROJECT_MISSING")?;
    let root = Guard::open(Path::new(&project.local_path), true)?;
    if root.path != p.root || root.identity != p.root_identity {
        return Err("API_PROJECT_ROOT_DRIFT");
    };
    Ok(())
}
fn validate_current(db: &Database, p: &Plan) -> Result<config::StoredConfig> {
    if !matches!(p.approval_kind.as_str(), "EXPLICIT" | "INHERITED") {
        return Err("API_LEGACY_PLAN_REQUIRES_NEW_APPROVAL");
    }
    if p.approval_kind == "INHERITED" {
        let parent = p.parent_run.as_deref().ok_or("API_APPROVAL_MISMATCH")?;
        let (original, _, _, _) = load_plan(db, parent)?;
        if !original.allow_repair
            || original.parent_run.is_some()
            || p.expires_utc > original.expires_utc
            || original.expires_utc <= now_utc()
        {
            return Err("API_APPROVAL_EXPIRED");
        }
    }
    validate_root(db, p)?;
    if p.schema != "nova.api.task/1" || p.expires_utc <= now_utc() {
        return Err("API_APPROVAL_EXPIRED");
    }
    if p.tests.iter().any(|t| {
        t.argv.is_empty()
            || (t.specification.recipe == "python_unittest"
                && !t.argv.iter().any(|a| a == recipes::UNIT_WRAPPER))
    }) {
        return Err("API_LEGACY_PLAN_REQUIRES_NEW_APPROVAL");
    }
    let c = config::load_config(db, &p.config.config.id)?;
    if encode(&c.view)? != encode(&p.config)?
        || hash(encode(&task_snapshot(db, &p.task_id)?)?.as_bytes()) != p.task_digest
    {
        return Err("API_BINDING_DRIFT");
    }
    let root = Guard::open(&p.root, true)?;
    if root.path != p.root || root.identity != p.root_identity {
        return Err("API_ROOT_DRIFT");
    };
    c.view.config.require_adapter()?;
    Ok(c)
}
fn verify_files(p: &Plan, after: Option<&[Change]>) -> Result<()> {
    for f in &p.files {
        let (raw, id) = read_file(&p.root.join(&f.path), 65536)?;
        let expected = after
            .and_then(|a| a.iter().find(|c| c.path == f.path))
            .map_or(f.sha256.as_str(), |c| c.candidate.candidate_sha256.as_str());
        if id != f.identity || hash(&raw) != expected {
            return Err("API_FILE_DRIFT");
        }
    }
    Ok(())
}
fn cas(db: &Database, id: &str, from: &str, to: &str, summary: &Value) -> Result<()> {
    let raw = encode(summary)?;
    if raw.len() > 262144 {
        return Err("API_EVIDENCE_BOUND");
    }
    if sql(db.conn().execute("UPDATE api_task_runs SET phase=?1,revision=revision+1,summary_json=?2 WHERE id=?3 AND phase=?4",rusqlite::params![to,raw,id,from]))?!=1{return Err("API_RUN_ALREADY_STARTED")};
    Ok(())
}
pub fn approve(db: &Database, id: &str, digest: &str) -> Result<Value> {
    let (p, d, phase, _) = load_plan(db, id)?;
    if d != digest || phase != "PREPARED" {
        return Err("API_APPROVAL_MISMATCH");
    };
    validate_current(db, &p)?;
    verify_files(&p, None)?;
    cas(
        db,
        id,
        "PREPARED",
        "APPROVED",
        &json!({"approved_digest":d,"tests":"NOT RUN","review":"NOT RUN"}),
    )?;
    status(db, id)
}
pub fn status(db: &Database, id: &str) -> Result<Value> {
    let (p, d, phase, summary) = load_plan(db, id)?;
    Ok(
        json!({"run_id":id,"task_id":p.task_id,"digest":d,"phase":phase,"summary":summary,"task_completed":phase=="ACCEPTED","process_liveness":"UNKNOWN","automatic_retry":false}),
    )
}
#[derive(Clone, Serialize, Deserialize)]
struct Change {
    path: String,
    candidate: Candidate,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edits {
    schema: String,
    edits: Vec<Proposal>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TestEvidence {
    #[serde(default)]
    pub counts: Option<recipes::Counts>,
    pub file: String,
    pub status: String,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}
pub trait TestPort {
    fn run(
        &mut self,
        test: &ApprovedTest,
        root: &Path,
        cancel: &CancellationToken,
    ) -> Result<TestEvidence>;
}
pub struct FixedTestRunner;
// Immutable, read-only verifier: no project imports, eval/exec or user/model shell text.
const CHECKER:&str="import pathlib,sys\ntry:\n b=pathlib.Path(sys.argv[1]).read_bytes(); ok=sys.argv[2].encode('utf-8') in b; print('CONTENT_ASSERT_PASS' if ok else 'CONTENT_ASSERT_FAIL'); sys.exit(0 if ok else 1)\nexcept Exception:\n print('CONTENT_ASSERT_ERROR'); sys.exit(2)";
impl TestPort for FixedTestRunner {
    fn run(
        &mut self,
        t: &ApprovedTest,
        root: &Path,
        cancel: &CancellationToken,
    ) -> Result<TestEvidence> {
        let mut r = crate::runtime::types::ExecutionRequest::new(
            t.specification.program.to_string_lossy(),
            t.argv.clone(),
            t.cwd.clone(),
            Some(root.into()),
        );
        r.environment = Some(t.environment.clone());
        r.timeout = Duration::from_secs(10);
        r.max_output_bytes = 4096;
        let e = crate::runtime::process::execute_blocking(r, cancel.clone());
        let status = match &e.outcome {
            crate::runtime::types::ExecutionOutcome::Success
                if !e.stdout_truncated && !e.stderr_truncated =>
            {
                "PASS"
            }
            crate::runtime::types::ExecutionOutcome::Failed {
                kind: crate::runtime::types::ExecutionErrorKind::NonZeroExit,
                ..
            } => "FAIL",
            _ => "UNKNOWN",
        };
        let counts = if t.specification.recipe == "python_unittest" {
            recipes::counts(&e.stdout)
        } else {
            None
        };
        let status = if status == "PASS"
            && t.specification.recipe == "python_unittest"
            && !counts.as_ref().is_some_and(|c| c.passed())
        {
            "UNKNOWN"
        } else {
            status
        };
        Ok(TestEvidence {
            counts,
            file: t.specification.file.clone(),
            status: status.into(),
            exit_code: e.exit_code,
            stdout: e.stdout,
            stderr: e.stderr,
        })
    }
}
pub fn execute<P: ApiPort, T: TestPort>(
    db: &Database,
    id: &str,
    vault: &dyn CredentialStore,
    api: &mut P,
    test: &mut T,
    cancel: &CancellationToken,
) -> Result<Value> {
    let (p, _, phase, mut summary) = load_plan(db, id)?;
    if phase != "APPROVED" {
        return Err("API_RUN_ALREADY_STARTED");
    }
    let c = validate_current(db, &p)?;
    verify_files(&p, None)?;
    if cancel.is_cancelled() {
        return Err("API_CANCELLED_BEFORE_START");
    }
    cas(db, id, "APPROVED", "DEVELOP_STARTED", &summary)?;
    let started = Instant::now();
    let mut stage = "DEVELOP_STARTED";
    let result = (|| -> Result<()> {
        let key = config::acquire(&c, vault)?;
        key.scan_json(encode(&p)?.as_bytes(), 65536)?;
        let input = json!({"task":p.task,"files":p.files,"fixed_tests":p.tests,"scope":p.notice,"prior_issues":p.prior_issues});
        summary["develop_attempts_reserved"] = json!(1);
        cas(db, id, stage, stage, &summary)?;
        validate_current(db, &p)?;
        let proposed =
            transport::completion(api, &p.config.config, &key, "DEVELOP", &input, cancel)?;
        let edits: Edits = decode(
            &serde_json::to_vec(&proposed).map_err(|_| "API_ENCODING")?,
            65536,
        )?;
        if edits.schema != "nova.api.edits/1"
            || edits.edits.is_empty()
            || edits.edits.len() > p.files.len()
        {
            return Err("API_PROPOSAL_INVALID");
        }
        let mut seen = BTreeSet::new();
        let mut changes = vec![];
        for proposal in edits.edits {
            let file = p
                .files
                .iter()
                .find(|f| f.path == proposal.target_path)
                .ok_or("API_PATH_REJECTED")?;
            if !seen.insert(proposal.target_path.clone()) {
                return Err("API_DUPLICATE_EDIT");
            }
            let raw = serde_json::to_vec(&proposal).map_err(|_| "API_ENCODING")?;
            let candidate = candidate_text(
                &file.path,
                &file.sha256,
                file.before.as_bytes(),
                &raw,
                false,
            )?;
            changes.push(Change {
                path: file.path.clone(),
                candidate,
            });
        }
        validate_current(db, &p)?;
        verify_files(&p, None)?;
        if cancel.is_cancelled() {
            return Err("API_CANCELLED_UNKNOWN");
        }
        let intent = json!({"changes":changes,"backup":"original bytes retained in immutable plan.files.before","write_semantics":"NON_ATOMIC_IN_PLACE","tests":"NOT RUN","review":"NOT RUN","develop_attempts_reserved":1,"review_attempts_reserved":0});
        cas(db, id, stage, "WRITE_INTENT", &intent)?;
        stage = "WRITE_INTENT";
        summary = intent;
        let mut targets = vec![];
        for change in &changes {
            let f = p
                .files
                .iter()
                .find(|f| f.path == change.path)
                .ok_or("API_PATH_REJECTED")?;
            targets.push(ExclusiveTarget::open(
                &p.root.join(&f.path),
                &f.identity,
                f.before.as_bytes(),
            )?);
        }
        validate_current(db, &p)?;
        for (target, change) in targets.iter_mut().zip(&changes) {
            validate_current(db, &p)?;
            target.write(change.candidate.text.as_bytes(), 0)?;
        }
        drop(targets);
        verify_files(&p, Some(&changes))?;
        summary["write_verified"] = json!(true);
        cas(db, id, stage, "TEST_STARTED", &summary)?;
        stage = "TEST_STARTED";
        let mut tests = vec![];
        for approved in &p.tests {
            if cancel.is_cancelled() || started.elapsed() > Duration::from_secs(180) {
                return Err("API_CANCELLED_UNKNOWN");
            }
            let actual = tool_snapshot(&approved.specification, &p.root, &p.files)?;
            if encode(&actual)? != encode(approved)? {
                return Err("API_TEST_TOOL_DRIFT");
            }
            // Pin the approved executable across spawn/exit; a hash snapshot alone permits a path swap.
            let mut tool_guard = Guard::tool(&approved.specification.program)?;
            let tool_size = std::fs::metadata(&tool_guard.path)
                .map_err(|_| "API_TEST_TOOL_INVALID")?
                .len();
            if tool_guard.identity != approved.program_identity
                || tool_guard.sha256_stream(tool_size, Instant::now() + Duration::from_secs(5))?
                    != approved.program_sha256
            {
                return Err("API_TEST_TOOL_DRIFT");
            }
            let _parents = parents(&p.root.join(&approved.specification.file))?;
            let mut test_file = Guard::open(&p.root.join(&approved.specification.file), false)?;
            if !p
                .files
                .iter()
                .any(|f| f.path == approved.specification.file && f.identity == test_file.identity)
            {
                return Err("API_FILE_DRIFT");
            }
            let test_bytes = test_file.bytes(65536)?;
            let expected = changes
                .iter()
                .find(|c| c.path == approved.specification.file)
                .map_or_else(
                    || {
                        p.files
                            .iter()
                            .find(|f| f.path == approved.specification.file)
                            .unwrap()
                            .sha256
                            .as_str()
                    },
                    |c| c.candidate.candidate_sha256.as_str(),
                );
            if hash(&test_bytes) != expected {
                return Err("API_FILE_DRIFT");
            }
            validate_current(db, &p)?;
            let ev = test.run(approved, &p.root, cancel)?;
            key.scan_text(ev.stdout.as_bytes())?;
            key.scan_text(ev.stderr.as_bytes())?;
            if ev.stdout.len() + ev.stderr.len() > 8192 {
                return Err("API_TEST_OUTPUT_BOUND");
            }
            tests.push(ev);
            summary["tests"] = serde_json::to_value(&tests).map_err(|_| "API_ENCODING")?;
            // Persist each test receipt before any subsequent test/review.
            cas(db, id, stage, stage, &summary)?;
            if tests.last().unwrap().status == "UNKNOWN" {
                return Err("API_TEST_RESULT_UNKNOWN");
            }
        }
        validate_current(db, &p)?;
        verify_files(&p, Some(&changes))?;
        let base_hash = hash(
            encode(
                &p.files
                    .iter()
                    .map(|f| json!({"path":f.path,"sha256":f.sha256}))
                    .collect::<Vec<_>>(),
            )?
            .as_bytes(),
        );
        let after:Vec<_>=p.files.iter().map(|f|json!({"path":f.path,"sha256":changes.iter().find(|c|c.path==f.path).map_or(&f.sha256,|c|&c.candidate.candidate_sha256)})).collect();
        let after_hash = hash(encode(&after)?.as_bytes());
        summary["review_attempts_reserved"] = json!(1);
        cas(db, id, stage, "REVIEW_STARTED", &summary)?;
        stage = "REVIEW_STARTED";
        // Fresh system+user context: no developer assistant messages or tools carried over.
        let review_input = json!({"task":p.task,"base_files":p.files,"actual_changes":changes,"actual_tests":tests,"base_sha256":base_hash,"candidate_sha256":after_hash});
        if cancel.is_cancelled() || started.elapsed() > Duration::from_secs(240) {
            return Err("API_CANCELLED_UNKNOWN");
        }
        validate_current(db, &p)?;
        let raw =
            transport::completion(api, &p.config.config, &key, "REVIEW", &review_input, cancel)?;
        let claim: ReviewClaim = decode(
            &serde_json::to_vec(&raw).map_err(|_| "API_ENCODING")?,
            65536,
        )?;
        claim.validate(&base_hash, &after_hash)?;
        validate_current(db, &p)?;
        verify_files(&p, Some(&changes))?;
        summary["review"] = serde_json::to_value(&claim).map_err(|_| "API_ENCODING")?;
        summary["review_response_validated"] = json!(true);
        let project_proof = tests
            .iter()
            .zip(&p.tests)
            .all(|(e, t)| recipes::valid_evidence(t, e));
        let passed = project_proof
            && tests
                .iter()
                .all(|t| t.status == "PASS" && t.exit_code == Some(0))
            && claim.verdict == "PASS"
            && claim.issues.is_empty()
            && claim.unknowns.is_empty();
        let next = if passed {
            "AWAITING_USER"
        } else {
            "NEEDS_REMEDIATION"
        };
        cas(db, id, stage, next, &summary)?;
        stage = next;
        Ok(())
    })();
    if let Err(error) = result {
        let unknown = matches!(
            error,
            "API_RESPONSE_UNKNOWN"
                | "API_TIMEOUT_UNKNOWN"
                | "API_CANCELLED_UNKNOWN"
                | "API_TEST_RESULT_UNKNOWN"
        ) || stage == "WRITE_INTENT";
        let next = if unknown {
            "UNKNOWN"
        } else if matches!(stage, "TEST_STARTED" | "REVIEW_STARTED") {
            "NEEDS_REMEDIATION"
        } else {
            "FAILED"
        };
        summary["error_code"] = json!(error);
        summary["stopped_phase"] = json!(stage);
        cas(db, id, stage, next, &summary)?;
    }
    status(db, id)
}
#[cfg(test)]
mod tests;

fn now_utc() -> i64 {
    #[cfg(test)]
    {
        if let Some(t) = TEST_CLOCK.with(|v| v.get()) {
            return t;
        }
    }
    chrono::Utc::now().timestamp()
}
#[cfg(test)]
thread_local! {static TEST_CLOCK:std::cell::Cell<Option<i64>>=const{std::cell::Cell::new(None)};}
