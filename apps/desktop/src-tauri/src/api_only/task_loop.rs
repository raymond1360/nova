//! Single-task repair linkage and existing Task acceptance bridge. No second Task state machine.
use super::*;
use crate::models::{Actor, TaskState};
pub fn repair_context(
    db: &Database,
    task: &str,
    parent: Option<&str>,
    c: &ConfigView,
    _paths: &[String],
    _tests: &[ApprovedTest],
    _allow: bool,
) -> Result<Value> {
    if let Some(id) = parent {
        let (p, _, phase, s) = load_plan(db, id)?;
        if p.task_id != task || p.parent_run.is_some() {
            return Err("API_REPAIR_LIMIT");
        }
        let used:i64=sql(db.conn().query_row("SELECT count(*) FROM api_task_scopes WHERE task_id=?1 AND first_run=?2 AND repair_run IS NULL",rusqlite::params![task,id],|r|r.get(0)))?;
        if used != 1 {
            return Err("API_REPAIR_LIMIT");
        }
        if !matches!(phase.as_str(), "NEEDS_REMEDIATION" | "USER_REJECTED") {
            return Err("API_REPAIR_NOT_ALLOWED");
        }
        // A manual approval may change configuration/scope, but never inherit the former grant.
        let _ = c;
        let v = json!({"parent_run":id,"review":s["review"],"tests":s["tests"],"user_feedback":s["user_feedback"],"error_code":s["error_code"]});
        if encode(&v)?.len() > 32768 {
            return Err("API_REPAIR_CONTEXT_BOUND");
        };
        Ok(v)
    } else {
        let n: i64 = sql(db.conn().query_row(
            "SELECT count(*) FROM api_task_scopes WHERE task_id=?1",
            [task],
            |r| r.get(0),
        ))?;
        if n != 0 {
            return Err("API_TASK_ALREADY_RESERVED");
        };
        Ok(Value::Null)
    }
}
pub fn insert_plan(db: &Database, p: &Plan, raw: &str, parent: Option<&str>) -> Result<()> {
    sql(db.conn().execute_batch("BEGIN IMMEDIATE"))?;
    let result = (|| {
        sql(db.conn().execute("INSERT INTO api_task_runs(id,task_id,project_id,plan_json,digest,phase,revision,summary_json) VALUES(?1,?2,?3,?4,?5,'PREPARED',0,'{}')",rusqlite::params![p.id,p.task_id,p.project_id,raw,hash(raw.as_bytes())]))?;
        if let Some(parent) = parent {
            if sql(db.conn().execute("UPDATE api_task_scopes SET repair_run=?1 WHERE task_id=?2 AND first_run=?3 AND repair_run IS NULL AND EXISTS(SELECT 1 FROM api_task_runs WHERE id=?3 AND phase IN ('NEEDS_REMEDIATION','USER_REJECTED'))",rusqlite::params![p.id,p.task_id,parent]))?!=1{return Err("API_REPAIR_LIMIT")}
        } else {
            sql(db.conn().execute(
                "INSERT INTO api_task_scopes(task_id,first_run) VALUES(?1,?2)",
                rusqlite::params![p.task_id, p.id],
            ))?;
        }
        Ok(())
    })();
    match result {
        Ok(()) => sql(db.conn().execute_batch("COMMIT")),
        Err(e) => {
            let _ = db.conn().execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
pub fn repair(
    db: &Database,
    vault: &dyn CredentialStore,
    parent: &str,
    paths: Vec<String>,
    tests: Vec<FixedTest>,
    config_id: &str,
) -> Result<Plan> {
    let (p, _, _, _) = load_plan(db, parent)?;
    prepare_scoped(
        db,
        vault,
        &p.task_id,
        config_id,
        paths,
        tests,
        false,
        Some(parent),
    )
}
fn inherited_files(db: &Database, id: &str, p: &Plan) -> Result<Vec<(String, String, String)>> {
    let (_, _, _, s) = load_plan(db, id)?;
    let changes: Vec<Change> = decode(
        &serde_json::to_vec(&s["changes"]).map_err(|_| "API_ENCODING")?,
        131072,
    )?;
    verify_files(p, Some(&changes))?;
    Ok(p.files
        .iter()
        .map(|f| {
            (
                f.path.clone(),
                f.identity.clone(),
                changes
                    .iter()
                    .find(|c| c.path == f.path)
                    .map_or(&f.sha256, |c| &c.candidate.candidate_sha256)
                    .clone(),
            )
        })
        .collect())
}
pub(super) fn inherit_repair(
    db: &Database,
    vault: &dyn CredentialStore,
    id: &str,
    p: &Plan,
) -> Result<Plan> {
    let (_, _, phase, _) = load_plan(db, id)?;
    if !p.allow_repair || p.parent_run.is_some() || p.expires_utc <= now_utc() {
        return Err("API_NEW_APPROVAL_REQUIRED");
    }
    if phase == "USER_REJECTED" {
        let t = db.task(&p.task_id).map_err(|_| "API_TASK_MISSING")?;
        if t.state != TaskState::NeedsUserAction
            || normalized(task_snapshot_for_gate(db, &t)?) != normalized(p.task.clone())
        {
            return Err("API_BINDING_DRIFT");
        }
    } else {
        validate_current(db, p)?;
    }
    validate_root(db, p)?;
    let expected = inherited_files(db, id, p)?;
    let next = prepare_deadline(
        db,
        vault,
        &p.task_id,
        &p.config.config.id,
        p.files.iter().map(|f| f.path.clone()).collect(),
        p.tests.iter().map(|t| t.specification.clone()).collect(),
        false,
        Some(id),
        Some(p.expires_utc),
    )?;
    if next.files.len() != p.files.len()
        || next.files.iter().any(|f| {
            !expected
                .iter()
                .any(|e| e.0 == f.path && e.1 == f.identity && e.2 == f.sha256)
        })
        || encode(&next.config)? != encode(&p.config)?
        || encode(&next.tests)? != encode(&p.tests)?
        || next.root_identity != p.root_identity
        || p.expires_utc <= now_utc()
    {
        return Err("API_NEW_APPROVAL_REQUIRED");
    }
    approve(db, &next.id, &hash(encode(&next)?.as_bytes()))?;
    Ok(next)
}
pub fn execute_loop<P: ApiPort, T: TestPort>(
    db: &Database,
    id: &str,
    vault: &dyn CredentialStore,
    api: &mut P,
    test: &mut T,
    cancel: &CancellationToken,
) -> Result<Value> {
    let (initial, _, phase, _) = load_plan(db, id)?;
    if cancel.is_cancelled() {
        return Err("API_CANCELLED_BEFORE_START");
    }
    let active = if phase == "USER_REJECTED" {
        inherit_repair(db, vault, id, &initial)?
    } else {
        initial
    };
    let mut result = execute(db, &active.id, vault, api, test, cancel)?;
    if result["phase"] == "NEEDS_REMEDIATION"
        && active.allow_repair
        && active.parent_run.is_none()
        && !cancel.is_cancelled()
        && result["summary"].get("error_code").is_none()
    {
        let next = inherit_repair(db, vault, &active.id, &active)?;
        result = execute(db, &next.id, vault, api, test, cancel)?;
    }
    if result["phase"] == "AWAITING_USER" {
        let result_id = result["run_id"].as_str().ok_or("API_RUN_INVALID")?;
        if let Err(error) = publish(db, result_id) {
            let (_, _, phase, mut summary) = load_plan(db, result_id)?;
            summary["error_code"] = json!(error);
            summary["publication_requires_manual_check"] = json!(true);
            cas(db, result_id, &phase, "UNKNOWN", &summary)?;
        }
        result = status(db, result_id)?;
    }
    Ok(result)
}
fn normalized(mut v: Value) -> Value {
    if let Some(t) = v["task"].as_object_mut() {
        for k in ["state", "updated_at", "result_generation"] {
            t.remove(k);
        }
    }
    v
}
pub fn passing_evidence(db: &Database, task: &crate::models::Task) -> Result<bool> {
    let id: Option<String> = sql(db
        .conn()
        .query_row(
            "SELECT coalesce(repair_run,first_run) FROM api_task_scopes WHERE task_id=?1",
            [&task.id],
            |r| r.get(0),
        )
        .optional())?;
    let Some(id) = id else { return Ok(false) };
    let (p, _, phase, s) = load_plan(db, &id)?;
    if p.approval_kind.is_empty() {
        return Ok(false);
    }
    validate_root(db, &p)?;
    if phase != "AWAITING_USER" || s["published_generation"] != task.result_generation {
        return Ok(false);
    }
    let mut current = task_snapshot_for_gate(db, task)?;
    let old = normalized(p.task.clone());
    current = normalized(current);
    if current != old {
        return Ok(false);
    }
    let changes: Vec<Change> = decode(
        &serde_json::to_vec(&s["changes"]).map_err(|_| "API_ENCODING")?,
        131072,
    )?;
    verify_files(&p, Some(&changes))?;
    let base = hash(
        encode(
            &p.files
                .iter()
                .map(|f| json!({"path":f.path,"sha256":f.sha256}))
                .collect::<Vec<_>>(),
        )?
        .as_bytes(),
    );
    let after=hash(encode(&p.files.iter().map(|f|json!({"path":f.path,"sha256":changes.iter().find(|c|c.path==f.path).map_or(&f.sha256,|c|&c.candidate.candidate_sha256)})).collect::<Vec<_>>())?.as_bytes());
    let claim: ReviewClaim = decode(
        &serde_json::to_vec(&s["review"]).map_err(|_| "API_ENCODING")?,
        65536,
    )?;
    claim.validate(&base, &after)?;
    let c = config::load_config(db, &p.config.config.id)?;
    if encode(&c.view)? != encode(&p.config)? {
        return Ok(false);
    }
    let tests: Vec<TestEvidence> = decode(
        &serde_json::to_vec(&s["tests"]).map_err(|_| "API_ENCODING")?,
        32768,
    )?;
    Ok(!tests.is_empty()
        && tests.len() == p.tests.len()
        && tests
            .iter()
            .zip(&p.tests)
            .all(|(e, t)| recipes::valid_evidence(t, e))
        && tests
            .iter()
            .all(|t| t.status == "PASS" && t.exit_code == Some(0))
        && s["review"]["verdict"] == "PASS"
        && s["review"]["issues"] == json!([])
        && s["review"]["unknowns"] == json!([])
        && s["write_verified"] == true)
}
fn task_snapshot_for_gate(db: &Database, t: &crate::models::Task) -> Result<Value> {
    let binding: Option<(String, i64, String)> = sql(db
        .conn()
        .query_row(
            "SELECT draft_id,revision,content_hash FROM requirement_task_bindings WHERE task_id=?1",
            [&t.id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional())?;
    Ok(
        json!({"task":t,"binding":binding,"requirements":db.list_confirmed_requirement_items(&t.id).map_err(|_|"API_REQUIREMENT_READ_FAILED")?}),
    )
}
fn publish(db: &Database, id: &str) -> Result<()> {
    let (p, _, phase, mut s) = load_plan(db, id)?;
    if phase != "AWAITING_USER" {
        return Err("API_ACCEPTANCE_NOT_READY");
    };
    validate_current(db, &p)?;
    sql(db.conn().execute_batch("BEGIN IMMEDIATE"))?;
    let result = (|| {
        validate_current(db, &p)?;
        let state = db.task(&p.task_id).map_err(|_| "API_TASK_MISSING")?.state;
        let route = match state {
            TaskState::Confirmed => vec![
                TaskState::Queued,
                TaskState::Running,
                TaskState::Verifying,
                TaskState::PreviewReady,
                TaskState::AwaitingUserAcceptance,
            ],
            TaskState::NeedsUserAction => vec![
                TaskState::Running,
                TaskState::Verifying,
                TaskState::PreviewReady,
                TaskState::AwaitingUserAcceptance,
            ],
            TaskState::AwaitingUserAcceptance => vec![],
            _ => return Err("API_TASK_STATE_DRIFT"),
        };
        for to in route {
            db.transition_task_in_transaction(
                &p.task_id,
                to,
                Actor::Nova,
                Some("API result receipt; no CLI executed"),
                Some(id),
                false,
                None,
            )
            .map_err(|_| "API_TASK_PUBLISH_REJECTED")?;
        }
        let t = db.task(&p.task_id).map_err(|_| "API_TASK_MISSING")?;
        s["published_generation"] = json!(t.result_generation);
        cas(db, id, "AWAITING_USER", "AWAITING_USER", &s)?;
        Ok(())
    })();
    match result {
        Ok(()) => sql(db.conn().execute_batch("COMMIT")),
        Err(e) => {
            let _ = db.conn().execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
pub(super) fn user_decision(db: &Database, id: &str, accepted: bool, note: &str) -> Result<Value> {
    safe_text(note)?;
    if note.len() > 4096 {
        return Err("API_FEEDBACK_BOUND");
    };
    let (p, _, phase, mut s) = load_plan(db, id)?;
    if phase != "AWAITING_USER" {
        return Err("API_ACCEPTANCE_NOT_READY");
    }
    sql(db.conn().execute_batch("BEGIN IMMEDIATE"))?;
    let result = (|| {
        let t = db.task(&p.task_id).map_err(|_| "API_TASK_MISSING")?;
        if accepted && !passing_evidence(db, &t)? {
            return Err("API_ACCEPTANCE_EVIDENCE_REJECTED");
        }
        db.record_user_acceptance(&p.project_id, &p.task_id, accepted, Some(note))
            .map_err(|_| "API_ACCEPTANCE_REJECTED")?;
        db.transition_task_in_transaction(
            &p.task_id,
            if accepted {
                TaskState::Completed
            } else {
                TaskState::NeedsUserAction
            },
            Actor::User,
            Some(note),
            Some(id),
            false,
            None,
        )
        .map_err(|_| "API_COMPLETION_GATE_REJECTED")?;
        s["user_feedback"] = json!(note);
        cas(
            db,
            id,
            "AWAITING_USER",
            if accepted {
                "ACCEPTED"
            } else {
                "USER_REJECTED"
            },
            &s,
        )?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            sql(db.conn().execute_batch("COMMIT"))?;
            status(db, id)
        }
        Err(e) => {
            let _ = db.conn().execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
pub fn report(db: &Database, task: &str) -> Result<Value> {
    let mut stmt = sql(db
        .conn()
        .prepare("SELECT id FROM api_task_runs WHERE task_id=?1 ORDER BY rowid"))?;
    let ids = sql(stmt.query_map([task], |r| r.get::<_, String>(0)))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| "API_DATABASE_ERROR")?;
    let runs = ids
        .iter()
        .map(|id| status(db, id))
        .collect::<Result<Vec<_>>>()?;
    let task = db.task(task).map_err(|_| "API_TASK_MISSING")?;
    let latest = runs.last().cloned().unwrap_or(Value::Null);
    let config_id = ids
        .last()
        .map(|id| load_plan(db, id).map(|p| p.0.config.config.id))
        .transpose()?;
    Ok(
        json!({"schema":"nova.api.pm-report/1","task":task,"task_goal":task.goal,"changes":latest["summary"]["changes"],"actual_tests":latest["summary"]["tests"],"self_check":latest["summary"]["review"],"blockers":{"phase":latest["phase"],"error":latest["summary"]["error_code"]},"plans":ids.iter().map(|id|load_plan(db,id).map(|v|v.0)).collect::<Result<Vec<_>>>()?,"runs":runs,"repair_count":ids.len().saturating_sub(1),"roles":{"developer":{"config_id":config_id},"reviewer":{"config_id":config_id,"independent_context":true},"pm":null},"decision_required":if task.state==TaskState::Completed{"NONE"}else{"USER_ACCEPTANCE_OR_PM_REMEDIATION"}}),
    )
}

/// UI feedback is persistent and may enter a repair context: scan the selected vault key first.
pub fn user_decision_with_vault(
    db: &Database,
    vault: &dyn CredentialStore,
    id: &str,
    accepted: bool,
    note: &str,
) -> Result<Value> {
    let (p, _, _, _) = load_plan(db, id)?;
    let c = config::load_config(db, &p.config.config.id)?;
    if c.view.has_key {
        config::acquire(&c, vault)?.scan_text(note.as_bytes())?;
    }
    user_decision(db, id, accepted, note)
}

/// Pending plans can be abandoned without sending; their Task scope/evidence remain immutable.
pub fn cancel_pending(db: &Database, task: &str) -> Result<()> {
    let id: String = sql(db.conn().query_row(
        "SELECT coalesce(repair_run,first_run) FROM api_task_scopes WHERE task_id=?1",
        [task],
        |r| r.get(0),
    ))?;
    let (_, _, phase, mut summary) = load_plan(db, &id)?;
    if !matches!(phase.as_str(), "PREPARED" | "APPROVED") {
        return Err("API_CANCEL_REQUIRES_RUNNING_TOKEN_OR_MANUAL_CHECK");
    }
    summary["cancelled_before_send"] = json!(true);
    cas(db, &id, &phase, "CANCELLED", &summary)
}
