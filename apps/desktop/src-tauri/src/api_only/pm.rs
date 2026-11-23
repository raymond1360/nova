//! Bounded PM decisions over the existing API task receipts; never a second file/test engine.
use super::*;
use crate::models::{Actor, CreateTaskRequest, TaskState};
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub project_id: String,
    pub goal: String,
    pub pm_config_id: String,
    pub execution_config_id: String,
    pub paths: Vec<String>,
    pub tests: Vec<FixedTest>,
    pub allow_repair: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub id: String,
    pub request: Request,
    pub root: PathBuf,
    pub root_identity: String,
    pub files: Vec<FileSnapshot>,
    pub tests: Vec<ApprovedTest>,
    pub pm: ConfigView,
    pub execution: ConfigView,
    pub expires_utc: i64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Draft {
    schema: String,
    action: String,
    goal: String,
    requirements: String,
    acceptance: String,
    files: Vec<String>,
    test_recipes: Vec<String>,
    reason: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    schema: String,
    action: String,
    reason: String,
}
fn tx<T>(db: &Database, f: impl FnOnce() -> Result<T>) -> Result<T> {
    sql(db.conn().execute_batch("BEGIN IMMEDIATE"))?;
    match f() {
        Ok(v) => {
            sql(db.conn().execute_batch("COMMIT"))?;
            Ok(v)
        }
        Err(e) => {
            let _ = db.conn().execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
pub fn prepare(db: &Database, vault: &dyn CredentialStore, request: Request) -> Result<Value> {
    safe_text(&request.goal)?;
    if request.goal.trim().is_empty()
        || request.goal.len() > 8192
        || request.paths.is_empty()
        || request.paths.len() > 3
        || request.tests.is_empty()
        || request.tests.len() > 2
    {
        return Err("API_PM_SCOPE_SPLIT_REQUIRED");
    }
    let project = db
        .project(&request.project_id)
        .map_err(|_| "API_PROJECT_MISSING")?;
    let root = Guard::open(Path::new(&project.local_path), true)?;
    safe_location(&root.path)?;
    let pm = config::load_config(db, &request.pm_config_id)?.view;
    let execution = config::load_config(db, &request.execution_config_id)?.view;
    for c in [&pm, &execution] {
        c.config.require_adapter()?;
        if !c.has_key {
            return Err("API_KEY_MISSING");
        }
    }
    let mut files = vec![];
    let mut seen = BTreeSet::new();
    for path in &request.paths {
        permitted_path(path)?;
        if !seen.insert(path.to_lowercase()) {
            return Err("API_DUPLICATE_PATH");
        }
        let (raw, identity) = read_file(&root.path.join(path), 8192)?;
        let before = String::from_utf8(raw).map_err(|_| "API_UTF8_REQUIRED")?;
        safe_text(&before)?;
        files.push(FileSnapshot {
            path: path.clone(),
            identity,
            sha256: hash(before.as_bytes()),
            before,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let tests = request
        .tests
        .iter()
        .map(|t| tool_snapshot(t, &root.path, &files))
        .collect::<Result<Vec<_>>>()?;
    let packet = Packet {
        id: uuid::Uuid::new_v4().to_string(),
        request,
        root: root.path,
        root_identity: root.identity,
        files,
        tests,
        pm,
        execution,
        expires_utc: now_utc() + 3600,
    };
    let raw = encode(&packet)?;
    if raw.len() > 49152 {
        return Err("API_CONTEXT_BOUND");
    }
    scan(
        db,
        vault,
        &packet,
        &serde_json::to_value(&packet).map_err(|_| "API_ENCODING")?,
    )?;
    sql(db.conn().execute("INSERT INTO api_pm_packets(id,project_id,packet_json,digest,phase) VALUES(?1,?2,?3,?4,'PREPARED')",rusqlite::params![packet.id,packet.request.project_id,raw,hash(raw.as_bytes())]))?;
    status(db, &packet.id)
}
fn load(db: &Database, id: &str) -> Result<Packet> {
    let (raw, digest): (String, String) = sql(db.conn().query_row(
        "SELECT packet_json,digest FROM api_pm_packets WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    ))?;
    if hash(raw.as_bytes()) != digest {
        return Err("API_PM_PACKET_TAMPERED");
    };
    decode(raw.as_bytes(), 65536)
}
fn phase(db: &Database, id: &str) -> Result<String> {
    sql(db
        .conn()
        .query_row("SELECT phase FROM api_pm_packets WHERE id=?1", [id], |r| {
            r.get(0)
        }))
}
fn advance(db: &Database, id: &str, from: &str, to: &str) -> Result<()> {
    if sql(db.conn().execute(
        "UPDATE api_pm_packets SET phase=?1,revision=revision+1 WHERE id=?2 AND phase=?3",
        rusqlite::params![to, id, from],
    ))? != 1
    {
        return Err("API_PM_NO_REPLAY");
    };
    Ok(())
}
fn current(db: &Database, p: &Packet, original: bool) -> Result<()> {
    if p.expires_utc <= now_utc() {
        return Err("API_APPROVAL_EXPIRED");
    }
    for c in [&p.pm, &p.execution] {
        if encode(&config::load_config(db, &c.config.id)?.view)? != encode(c)? {
            return Err("API_BINDING_DRIFT");
        }
    }
    let project = db
        .project(&p.request.project_id)
        .map_err(|_| "API_PROJECT_MISSING")?;
    let g = Guard::open(Path::new(&project.local_path), true)?;
    if g.path != p.root || g.identity != p.root_identity {
        return Err("API_ROOT_DRIFT");
    }
    if original {
        for f in &p.files {
            let (b, i) = read_file(&p.root.join(&f.path), 8192)?;
            if i != f.identity || hash(&b) != f.sha256 {
                return Err("API_FILE_DRIFT");
            }
        }
    }
    Ok(())
}
fn scan(db: &Database, vault: &dyn CredentialStore, p: &Packet, v: &Value) -> Result<()> {
    let raw = encode(v)?;
    for c in [&p.pm, &p.execution] {
        config::acquire(&config::load_config(db, &c.config.id)?, vault)?
            .scan_json(raw.as_bytes(), 131072)?;
    }
    Ok(())
}
fn request<P: ApiPort>(
    db: &Database,
    vault: &dyn CredentialStore,
    p: &Packet,
    api: &mut P,
    cancel: &CancellationToken,
    run: Option<&str>,
    stage: &str,
    input: Value,
) -> Result<Value> {
    current(db, p, stage == "PLAN")?;
    if cancel.is_cancelled() {
        return Err("API_CANCELLED_BEFORE_START");
    }
    scan(db, vault, p, &input)?;
    let ordinal = tx(db, || {
        let count: i64 = sql(db.conn().query_row(
            "SELECT pm_count FROM api_pm_packets WHERE id=?1",
            [&p.id],
            |r| r.get(0),
        ))?;
        if count >= 5
            || (stage == "PLAN" && count != 0)
            || (stage != "PLAN" && phase(db, &p.id)? != "RUNNING")
        {
            return Err("API_PM_REQUEST_LIMIT");
        }
        sql(db.conn().execute(
            "UPDATE api_pm_packets SET pm_count=pm_count+1,revision=revision+1 WHERE id=?1",
            [&p.id],
        ))?;
        let raw = encode(&input)?;
        sql(db.conn().execute("INSERT INTO api_pm_decisions(packet_id,ordinal,run_id,stage,input_json,input_hash,phase) VALUES(?1,?2,?3,?4,?5,?6,'STARTED')",rusqlite::params![p.id,count+1,run,stage,raw,hash(raw.as_bytes())]))?;
        Ok(count + 1)
    })?;
    let result = (|| {
        current(db, p, stage == "PLAN")?;
        let c = config::load_config(db, &p.pm.config.id)?;
        let key = config::acquire(&c, vault)?;
        let v = transport::completion(
            api,
            &p.pm.config,
            &key,
            if stage == "PLAN" {
                "PM_PLAN"
            } else {
                "PM_DECIDE"
            },
            &input,
            cancel,
        )?;
        current(db, p, stage == "PLAN")?;
        if cancel.is_cancelled() {
            return Err("API_CANCELLED_UNKNOWN");
        };
        scan(db, vault, p, &v)?;
        Ok(v)
    })();
    match result {
        Ok(v) => {
            sql(db.conn().execute("UPDATE api_pm_decisions SET phase='RECEIVED',output_json=?1 WHERE packet_id=?2 AND ordinal=?3",rusqlite::params![encode(&v)?,p.id,ordinal]))?;
            Ok(v)
        }
        Err(e) => {
            stop(db, &p.id, e)?;
            Err(e)
        }
    }
}
fn stop(db: &Database, id: &str, error: &str) -> Result<()> {
    sql(db.conn().execute("UPDATE api_pm_decisions SET disposition=COALESCE(disposition,'REJECTED'), rejection_reason=?1 WHERE packet_id=?2 AND ordinal=(SELECT pm_count FROM api_pm_packets WHERE id=?2)",rusqlite::params![error,id]))?;
    let state = if error.contains("UNKNOWN") || error.contains("TIMEOUT") {
        "UNKNOWN"
    } else {
        "STOPPED"
    };
    sql(db.conn().execute(
        "UPDATE api_pm_packets SET phase=?1,error_code=?2,revision=revision+1 WHERE id=?3",
        rusqlite::params![state, error, id],
    ))?;
    Ok(())
}
fn disposition(db: &Database, p: &Packet, accepted: bool) -> Result<()> {
    sql(db.conn().execute("UPDATE api_pm_decisions SET disposition=?1 WHERE packet_id=?2 AND ordinal=(SELECT pm_count FROM api_pm_packets WHERE id=?2)",rusqlite::params![if accepted{"ADOPTED"}else{"REJECTED"},p.id]))?;
    Ok(())
}
pub fn plan<P: ApiPort>(
    db: &Database,
    vault: &dyn CredentialStore,
    id: &str,
    digest: &str,
    explicit_network: bool,
    api: &mut P,
    cancel: &CancellationToken,
) -> Result<Value> {
    let p = load(db, id)?;
    let s = status(db, id)?;
    if !explicit_network || s["digest"] != digest {
        return Err("API_PM_PLANNING_CONSENT_REQUIRED");
    }
    current(db, &p, true)?;
    advance(db, id, "PREPARED", "PLANNING_STARTED")?;
    let result = (|| {
        let v = request(
            db,
            vault,
            &p,
            api,
            cancel,
            None,
            "PLAN",
            json!({"goal":p.request.goal,"files":p.files,"fixed_tests":p.tests,"limit":"selected files/tests only, <=3 files; clarify/split otherwise"}),
        )?;
        let d: Draft = decode(encode(&v)?.as_bytes(), 32768)?;
        let mut paths = d.files.clone();
        paths.sort();
        let selected: Vec<_> = p.files.iter().map(|f| f.path.clone()).collect();
        let recipes: Vec<_> = p
            .tests
            .iter()
            .map(|t| t.specification.recipe.clone())
            .collect();
        if d.schema != "nova.api.pm.draft/1"
            || !matches!(d.action.as_str(), "ARRANGE_DEVELOP" | "CLARIFY" | "STOP")
            || d.reason.len() > 2048
            || d.goal.trim().is_empty()
            || d.requirements.len() > 8192
            || d.acceptance.len() > 8192
            || paths != selected
            || d.test_recipes != recipes
        {
            disposition(db, &p, false)?;
            return Err("API_PM_DRAFT_REJECTED");
        }
        disposition(db, &p, true)?;
        let next = if d.action == "ARRANGE_DEVELOP"
            && !d.requirements.trim().is_empty()
            && !d.acceptance.trim().is_empty()
        {
            "DRAFT"
        } else {
            "NEEDS_USER"
        };
        sql(db.conn().execute(
            "UPDATE api_pm_packets SET draft_json=?1,phase=?2,revision=revision+1 WHERE id=?3",
            rusqlite::params![encode(&d)?, next, id],
        ))?;
        status(db, id)
    })();
    if let Err(e) = result {
        stop(db, id, e)?;
        return Err(e);
    }
    result
}
/// User accepts PM requirements; transaction creates the normal confirmed Task. No execution approval here.
pub fn confirm(db: &Database, vault: &dyn CredentialStore, id: &str) -> Result<Value> {
    let p = load(db, id)?;
    current(db, &p, true)?;
    let raw: String = sql(db.conn().query_row(
        "SELECT draft_json FROM api_pm_packets WHERE id=?1",
        [id],
        |r| r.get(0),
    ))?;
    let d: Draft = decode(raw.as_bytes(), 32768)?;
    let task = tx(db, || {
        advance(db, id, "DRAFT", "CONFIRMING")?;
        let t = db
            .create_task_in_transaction(
                &CreateTaskRequest {
                    project_id: p.request.project_id.clone(),
                    title: d.goal.chars().take(120).collect(),
                    goal: Some(d.goal.clone()),
                    in_scope: Some(d.requirements.clone()),
                    not_in_scope: Some(format!(
                        "Only {:?}; no scope expansion, no automatic completion/commit",
                        p.request.paths
                    )),
                    done_criteria: Some(d.acceptance.clone()),
                    ai_kind: None,
                },
                Actor::User,
            )
            .map_err(|_| "API_TASK_CREATE_REJECTED")?;
        for to in [TaskState::AwaitingConfirmation, TaskState::Confirmed] {
            db.transition_task_in_transaction(
                &t.id,
                to,
                Actor::User,
                Some("User confirmed PM draft"),
                None,
                false,
                None,
            )
            .map_err(|_| "API_TASK_CONFIRM_REJECTED")?;
        }
        sql(db.conn().execute(
            "UPDATE api_pm_packets SET task_id=?1 WHERE id=?2",
            rusqlite::params![t.id, id],
        ))?;
        Ok(t.id)
    })?;
    let result = (|| {
        let plan = prepare_deadline(
            db,
            vault,
            &task,
            &p.execution.config.id,
            p.request.paths.clone(),
            p.request.tests.clone(),
            p.request.allow_repair,
            None,
            None,
        )?;
        if encode(&plan.files)? != encode(&p.files)? || encode(&plan.tests)? != encode(&p.tests)? {
            return Err("API_PM_SCOPE_DRIFT");
        }
        sql(db.conn().execute("UPDATE api_pm_packets SET run_id=?1,phase='READY',revision=revision+1 WHERE id=?2 AND phase='CONFIRMING'",rusqlite::params![plan.id,id]))?;
        status(db, id)
    })();
    if let Err(e) = result {
        stop(db, id, e)?;
        return Err(e);
    }
    result
}
pub fn approve(db: &Database, id: &str, digest: &str) -> Result<Value> {
    let p = load(db, id)?;
    current(db, &p, true)?;
    tx(db, || {
        if phase(db, id)? != "READY" {
            return Err("API_PM_NO_REPLAY");
        }
        let s = status(db, id)?;
        let run = s["run_id"].as_str().ok_or("API_RUN_INVALID")?;
        let (plan, d, _, _) = load_plan(db, run)?;
        // PM scope, config and deadline are part of the separate user-visible approval digest.
        if s["execution_approval_digest"] != digest || plan.expires_utc < p.expires_utc {
            return Err("API_APPROVAL_MISMATCH");
        }
        super::approve(db, run, &d)?;
        sql(db.conn().execute("UPDATE api_pm_packets SET phase='APPROVED',approved_digest=?1,revision=revision+1 WHERE id=?2",rusqlite::params![digest,id]))?;
        Ok(())
    })?;
    status(db, id)
}
pub(super) fn managed(db: &Database, task: &str) -> Result<bool> {
    sql(db.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM api_pm_packets WHERE task_id=?1)",
        [task],
        |r| r.get(0),
    ))
}
pub(super) fn validate_bound(db: &Database, plan: &Plan) -> Result<()> {
    let id: Option<String> = sql(db
        .conn()
        .query_row(
            "SELECT id FROM api_pm_packets WHERE task_id=?1",
            [&plan.task_id],
            |r| r.get(0),
        )
        .optional())?;
    if let Some(id) = id {
        let p = load(db, &id)?;
        current(db, &p, false)?;
        if !matches!(
            phase(db, &id)?.as_str(),
            "CONFIRMING" | "READY" | "APPROVED" | "RUNNING"
        ) {
            return Err("API_PM_STOPPED");
        }
        if plan.config.config.id != p.execution.config.id {
            return Err("API_BINDING_DRIFT");
        }
        if matches!(phase(db, &id)?.as_str(), "APPROVED" | "RUNNING") {
            let s = status(db, &id)?;
            if s["approved_digest"].is_null()
                || s["approved_digest"] != s["execution_approval_digest"]
            {
                return Err("API_PM_APPROVAL_REQUIRED");
            }
        }
    }
    Ok(())
}
struct Policy<'a, P> {
    db: &'a Database,
    vault: &'a dyn CredentialStore,
    packet: &'a Packet,
    api: &'a mut P,
    cancel: &'a CancellationToken,
}
impl<P: ApiPort> ReviewPolicy for Policy<'_, P> {
    fn before_review(&mut self, p: &Plan, evidence: &Value) -> Result<()> {
        let a = decide(
            self.db,
            self.vault,
            self.packet,
            self.api,
            self.cancel,
            &p.id,
            "BEFORE_REVIEW",
            evidence,
            &["ARRANGE_REVIEW", "CLARIFY", "STOP"],
        )?;
        if a != "ARRANGE_REVIEW" {
            return Err("API_PM_HUMAN_REQUIRED");
        }
        Ok(())
    }
}
fn decide<P: ApiPort>(
    db: &Database,
    vault: &dyn CredentialStore,
    p: &Packet,
    api: &mut P,
    cancel: &CancellationToken,
    run: &str,
    stage: &str,
    evidence: &Value,
    allowed: &[&str],
) -> Result<String> {
    let (plan, _, _, _) = load_plan(db, run)?;
    let v = request(
        db,
        vault,
        p,
        api,
        cancel,
        Some(run),
        stage,
        json!({"run_id":run,"stage":stage,"confirmed_task":plan.task,"approved_tests":plan.tests,"file_scope":p.request.paths,"actual_evidence":evidence,"allowed_actions":allowed,"no_user_acceptance":true}),
    )?;
    let d: Decision = match decode(encode(&v)?.as_bytes(), 4096) {
        Ok(d) => d,
        Err(e) => {
            disposition(db, p, false)?;
            return Err(e);
        }
    };
    let valid = d.schema == "nova.api.pm.decision/1"
        && !d.reason.trim().is_empty()
        && d.reason.len() <= 2048
        && allowed.contains(&d.action.as_str());
    disposition(db, p, valid)?;
    if !valid {
        return Err("API_PM_DECISION_REJECTED");
    }
    Ok(d.action)
}
pub fn execute<P: ApiPort, E: ApiPort, T: TestPort>(
    db: &Database,
    vault: &dyn CredentialStore,
    id: &str,
    pm_api: &mut P,
    execution_api: &mut E,
    test: &mut T,
    cancel: &CancellationToken,
) -> Result<Value> {
    let p = load(db, id)?;
    current(db, &p, true)?;
    let s = status(db, id)?;
    if s["approved_digest"].is_null() || s["approved_digest"] != s["execution_approval_digest"] {
        return Err("API_PM_APPROVAL_REQUIRED");
    }
    advance(db, id, "APPROVED", "RUNNING")?;
    let result = (|| {
        let mut run = s["run_id"].as_str().ok_or("API_RUN_INVALID")?.to_string();
        for round in 0..2 {
            current(db, &p, false)?;
            let mut policy = Policy {
                db,
                vault,
                packet: &p,
                api: pm_api,
                cancel,
            };
            let receipt = execute_staged(
                db,
                &run,
                vault,
                execution_api,
                test,
                cancel,
                Some(&mut policy),
            )?;
            if receipt["phase"] == "UNKNOWN" || phase(db, id)? == "UNKNOWN" {
                return Err("API_PM_EXECUTION_UNKNOWN");
            }
            if receipt["summary"].get("error_code").is_some() {
                return Err("API_PM_EXECUTION_STOPPED");
            }
            let allowed = if receipt["phase"] == "AWAITING_USER" {
                vec!["DELIVER", "CLARIFY", "STOP"]
            } else if receipt["phase"] == "NEEDS_REMEDIATION"
                && round == 0
                && p.request.allow_repair
            {
                vec!["REPAIR", "CLARIFY", "STOP"]
            } else {
                vec!["CLARIFY", "STOP"]
            };
            let action = decide(
                db,
                vault,
                &p,
                pm_api,
                cancel,
                &run,
                "AFTER_REVIEW",
                &receipt,
                &allowed,
            )?;
            current(db, &p, false)?;
            if cancel.is_cancelled() {
                return Err("API_CANCELLED_UNKNOWN");
            }
            match action.as_str() {
                "DELIVER" => {
                    task_loop::publish(db, &run)?;
                    advance(db, id, "RUNNING", "AWAITING_USER")?;
                    return status(db, id);
                }
                "REPAIR" => {
                    let (parent, _, _, _) = load_plan(db, &run)?;
                    let child = task_loop::inherit_repair(db, vault, &run, &parent)?;
                    run = child.id;
                    sql(db.conn().execute(
                        "UPDATE api_pm_packets SET run_id=?1,revision=revision+1 WHERE id=?2",
                        rusqlite::params![run, id],
                    ))?;
                }
                _ => {
                    advance(db, id, "RUNNING", "NEEDS_USER")?;
                    return status(db, id);
                }
            }
        }
        Err("API_PM_REQUEST_LIMIT")
    })();
    if let Err(e) = result {
        stop(db, id, e)?;
        return Err(e);
    }
    result
}
pub fn cancel_pending(db: &Database, id: &str) -> Result<()> {
    let n=sql(db.conn().execute("UPDATE api_pm_packets SET phase='CANCELLED',revision=revision+1 WHERE id=?1 AND phase IN ('PREPARED','DRAFT','READY','APPROVED')",[id]))?;
    if n != 1 {
        return Err("API_PM_ACTIVE_OR_UNKNOWN");
    };
    Ok(())
}
pub fn status(db: &Database, id: &str) -> Result<Value> {
    let p = load(db, id)?;
    let (phase,revision,task,run,approved,count,draft,error):(String,i64,Option<String>,Option<String>,Option<String>,i64,Option<String>,Option<String>)=sql(db.conn().query_row("SELECT phase,revision,task_id,run_id,approved_digest,pm_count,draft_json,error_code FROM api_pm_packets WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?))))?;
    let mut stmt=sql(db.conn().prepare("SELECT ordinal,run_id,stage,input_hash,phase,output_json,disposition,rejection_reason FROM api_pm_decisions WHERE packet_id=?1 ORDER BY ordinal"))?;
    let rows = sql(stmt.query_map([id], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, Option<String>>(6)?,
            r.get::<_, Option<String>>(7)?,
        ))
    }))?;
    let mut decisions = vec![];
    for row in rows {
        let (n, run, stage, hash, phase, output, disposition, rejection_reason) = sql(row)?;
        decisions.push(json!({"ordinal":n,"run_id":run,"stage":stage,"input_hash":hash,"phase":phase,"output":output.map(|s|decode::<Value>(s.as_bytes(),65536)).transpose()?,"disposition":disposition,"rejection_reason":rejection_reason}));
    }
    let execution = run.as_deref().map(|r| super::status(db, r)).transpose()?;
    let plan = run
        .as_deref()
        .map(|r| load_plan(db, r).map(|v| v.0))
        .transpose()?;
    let approval_plan = task
        .as_deref()
        .map(|t| {
            sql(db.conn().query_row(
                "SELECT first_run FROM api_task_scopes WHERE task_id=?1",
                [t],
                |r| r.get::<_, String>(0),
            ))
        })
        .transpose()?
        .map(|r| load_plan(db, &r).map(|v| v.1))
        .transpose()?;
    let approval=hash(encode(&json!({"packet_digest":hash(encode(&p)?.as_bytes()),"plan":approval_plan,"pm_limit":5,"execution_limit":4,"total_limit":9,"deadline":p.expires_utc}))?.as_bytes());
    let mut report = task
        .as_deref()
        .map(|t| task_loop::report(db, t))
        .transpose()?;
    let execution_requests: i64 = report
        .as_ref()
        .and_then(|r| r["runs"].as_array())
        .map(|runs| {
            runs.iter()
                .map(|r| {
                    r["summary"]["develop_attempts_reserved"]
                        .as_i64()
                        .unwrap_or(0)
                        + r["summary"]["review_attempts_reserved"]
                            .as_i64()
                            .unwrap_or(0)
                })
                .sum()
        })
        .unwrap_or(0);
    if let Some(r) = report.as_mut() {
        r["roles"]["pm"] = json!({"config_id":p.pm.config.id,"independent_context":true});
        r["pm_decisions"] = json!(decisions);
        r["requests_reserved"] = json!({"pm":count,"execution":execution_requests,"total":count+execution_requests,"maximum":9});
    }
    Ok(
        json!({"id":id,"phase":phase,"revision":revision,"digest":hash(encode(&p)?.as_bytes()),"packet":p,"draft":draft.map(|s|decode::<Value>(s.as_bytes(),32768)).transpose()?,"task_id":task,"run_id":run,"approved_digest":approved,"execution_approval_digest":approval,"pm_requests_reserved":count,"pm_request_limit":5,"execution_requests_reserved":execution_requests,"overall_requests_reserved":count+execution_requests,"execution_request_limit":4,"overall_request_limit":9,"decisions":decisions,"execution":execution,"plan":plan,"report":report,"error_code":error,"liveness":"UNKNOWN unless this process holds the active cancellation slot","automatic_replay":false,"user_acceptance":"ONLY_USER"}),
    )
}
pub fn latest(db: &Database, project: &str) -> Result<Option<Value>> {
    let id: Option<String> = sql(db
        .conn()
        .query_row(
            "SELECT id FROM api_pm_packets WHERE project_id=?1 ORDER BY rowid DESC LIMIT 1",
            [project],
            |r| r.get(0),
        )
        .optional())?;
    id.map(|id| status(db, &id)).transpose()
}
