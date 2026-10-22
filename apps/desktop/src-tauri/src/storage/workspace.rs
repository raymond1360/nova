//! Non-AI requirement workspace. No runtime adapter, credentials or filesystem access.
use super::{new_id, now};
use crate::{
    db::Database,
    models::{Actor, CreateTaskRequest, TaskState},
};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Fields {
    pub title_raw: String,
    pub selected_ai: Option<String>,
    pub goal_raw: String,
    pub in_scope_raw: String,
    pub not_in_scope_raw: String,
    pub done_criteria_raw: String,
}
impl Default for Fields {
    fn default() -> Self {
        Self {
            title_raw: String::new(),
            selected_ai: None,
            goal_raw: String::new(),
            in_scope_raw: String::new(),
            not_in_scope_raw: String::new(),
            done_criteria_raw: String::new(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Open {},
    List {
        conversation_id: String,
        after_seq: i64,
    },
    Create {
        conversation_id: String,
        origin_task_id: Option<String>,
    },
    Get {
        conversation_id: String,
        draft_id: String,
    },
    Save {
        conversation_id: String,
        draft_id: String,
        expected_revision: i64,
        fields: Fields,
    },
    Append {
        conversation_id: String,
        draft_id: String,
        expected_revision: i64,
        body_raw: String,
    },
    Confirm {
        conversation_id: String,
        draft_id: String,
        expected_revision: i64,
        content_hash: String,
        selected_ai: String,
    },
    Binding {
        conversation_id: String,
        draft_id: String,
    },
    Archive {
        conversation_id: String,
        draft_id: String,
        expected_revision: i64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub project_id: String,
    pub client_request_id: Option<String>,
    pub operation: Operation,
}
#[derive(Debug, Serialize)]
pub struct WorkspaceError {
    pub code: String,
    pub current_revision: Option<i64>,
}
impl WorkspaceError {
    fn code(code: &str) -> Self {
        Self {
            code: code.into(),
            current_revision: None,
        }
    }
}
impl From<rusqlite::Error> for WorkspaceError {
    fn from(_: rusqlite::Error) -> Self {
        Self::code("DATABASE_ERROR")
    }
}
impl From<crate::error::NovaError> for WorkspaceError {
    fn from(_: crate::error::NovaError) -> Self {
        Self::code("TASK_GATE_REJECTED")
    }
}
type Result<T> = std::result::Result<T, WorkspaceError>;
fn hash(value: &impl Serialize) -> String {
    let bytes = serde_json::to_vec(value).expect("serializable workspace DTO");
    ring::digest::digest(&ring::digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
fn content_hash(d: &str, revision: i64, fields: &Fields) -> String {
    hash(&(1, d, revision, fields))
}
fn err(code: &str) -> WorkspaceError {
    WorkspaceError::code(code)
}

impl Database {
    /// Caller serializes against process-local start reservations before entering here.
    /// `reserved_tasks` is captured while retaining that same lock until this call returns.
    pub(crate) fn workspace(&self, request: &Request, reserved_tasks: &[String]) -> Result<Value> {
        self.project(&request.project_id)
            .map_err(|_| err("IDENTITY_MISMATCH"))?;
        let write = !matches!(
            request.operation,
            Operation::Get { .. } | Operation::List { .. } | Operation::Binding { .. }
        );
        if !write {
            return self.workspace_inner(request, reserved_tasks);
        }
        let key = request
            .client_request_id
            .as_deref()
            .ok_or_else(|| err("REQUEST_ID_REQUIRED"))?;
        uuid::Uuid::parse_str(key).map_err(|_| err("INVALID_REQUEST_ID"))?;
        let op = match request.operation {
            Operation::Open {} => "open",
            Operation::Create { .. } => "create",
            Operation::Save { .. } => "save",
            Operation::Append { .. } => "append",
            Operation::Confirm { .. } => "confirm",
            Operation::Archive { .. } => "archive",
            _ => unreachable!(),
        };
        let request_hash = hash(request);
        self.conn().execute_batch("BEGIN IMMEDIATE;")?;
        let result = (|| {
            let receipt:Option<(String,String)>=self.conn().query_row(
                "SELECT request_hash,response_refs_json FROM requirement_mutation_receipts WHERE project_id=?1 AND operation=?2 AND client_request_id=?3",
                params![request.project_id,op,key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            if let Some((existing, data)) = receipt {
                if existing != request_hash {
                    return Err(err("IDEMPOTENCY_CONFLICT"));
                }
                let mut value: Value =
                    serde_json::from_str(&data).map_err(|_| err("DATABASE_ERROR"))?;
                value["replayed"] = json!(true);
                return Ok(value);
            }
            let mut value = self.workspace_inner(request, reserved_tasks)?;
            value["replayed"] = json!(false);
            self.conn().execute(
                "INSERT INTO requirement_mutation_receipts VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![
                    request.project_id,
                    op,
                    key,
                    request_hash,
                    value
                        .get("draft_id")
                        .or_else(|| value.get("conversation_id"))
                        .and_then(Value::as_str)
                        .unwrap_or(""),
                    value.to_string(),
                    now()
                ],
            )?;
            Ok(value)
        })();
        match result {
            Ok(value) => {
                if let Err(e) = self.conn().execute_batch("COMMIT;") {
                    let _ = self.conn().execute_batch("ROLLBACK;");
                    return Err(e.into());
                }
                Ok(value)
            }
            Err(e) => {
                let _ = self.conn().execute_batch("ROLLBACK;");
                Err(e)
            }
        }
    }
    fn workspace_identity(&self, p: &str, c: &str, d: Option<&str>) -> Result<()> {
        let found: bool = self.conn().query_row(
            "SELECT EXISTS(SELECT 1 FROM requirement_conversations WHERE id=?1 AND project_id=?2)",
            params![c, p],
            |r| r.get(0),
        )?;
        if !found {
            return Err(err("IDENTITY_MISMATCH"));
        }
        if let Some(d) = d {
            let found:bool=self.conn().query_row("SELECT EXISTS(SELECT 1 FROM requirement_drafts WHERE id=?1 AND project_id=?2 AND conversation_id=?3)",params![d,p,c],|r|r.get(0))?;
            if !found {
                return Err(err("IDENTITY_MISMATCH"));
            }
        }
        Ok(())
    }
    fn workspace_head(&self, d: &str) -> Result<(i64, String)> {
        Ok(self.conn().query_row(
            "SELECT current_revision,status FROM requirement_drafts WHERE id=?1",
            [d],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?)
    }
    fn workspace_editable(&self, d: &str, expected: i64) -> Result<i64> {
        let (rev, status) = self.workspace_head(d)?;
        if status != "editable" {
            return Err(err(if status == "bound" {
                "DRAFT_BOUND"
            } else {
                "DRAFT_ARCHIVED"
            }));
        }
        if rev != expected {
            return Err(WorkspaceError {
                code: "REVISION_CONFLICT".into(),
                current_revision: Some(rev),
            });
        }
        Ok(rev)
    }
    fn workspace_read(&self, d: &str) -> Result<Value> {
        let (rev, status) = self.workspace_head(d)?;
        let (fields,h)=self.conn().query_row("SELECT title_raw,selected_ai,goal_raw,in_scope_raw,not_in_scope_raw,done_criteria_raw,content_hash FROM requirement_revisions WHERE draft_id=?1 AND revision=?2",params![d,rev],|r|Ok((Fields{title_raw:r.get(0)?,selected_ai:r.get(1)?,goal_raw:r.get(2)?,in_scope_raw:r.get(3)?,not_in_scope_raw:r.get(4)?,done_criteria_raw:r.get(5)?},r.get::<_,String>(6)?)))?;
        let origin: Option<String> = self.conn().query_row(
            "SELECT origin_task_id FROM requirement_drafts WHERE id=?1",
            [d],
            |r| r.get(0),
        )?;
        Ok(
            json!({"draft_id":d,"revision":rev,"status":status,"fields":fields,"content_hash":h,"origin_task_id":origin}),
        )
    }
    fn workspace_revision(&self, p: &str, d: &str, rev: i64, fields: &Fields) -> Result<()> {
        if fields
            .selected_ai
            .as_deref()
            .is_some_and(|a| a != "codex_cli" && a != "claude_cli")
        {
            return Err(err("INVALID_AI"));
        }
        if serde_json::to_vec(fields).unwrap().len() > 65536 {
            return Err(err("INPUT_TOO_LARGE"));
        }
        self.conn().execute("INSERT INTO requirement_revisions (draft_id,project_id,revision,base_revision,title_raw,selected_ai,goal_raw,in_scope_raw,not_in_scope_raw,done_criteria_raw,source,content_hash,created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'user',?11,?12)",
            params![d,p,rev,rev-1,fields.title_raw,fields.selected_ai,fields.goal_raw,fields.in_scope_raw,fields.not_in_scope_raw,fields.done_criteria_raw,content_hash(d,rev,fields),now()])?;
        Ok(())
    }
    fn workspace_message(
        &self,
        p: &str,
        c: &str,
        d: &str,
        rev: i64,
        kind: &str,
        body: Option<&str>,
    ) -> Result<()> {
        self.conn().execute("INSERT INTO requirement_messages (id,project_id,conversation_id,seq,draft_id,revision,kind,body_raw,created_at) SELECT ?1,?2,?3,COALESCE(MAX(seq),0)+1,?4,?5,?6,?7,?8 FROM requirement_messages WHERE conversation_id=?3",params![new_id(),p,c,d,rev,kind,body,now()])?;
        self.conn().execute(
            "UPDATE requirement_conversations SET updated_at=?1 WHERE id=?2",
            params![now(), c],
        )?;
        Ok(())
    }
    fn workspace_bound(&self, d: &str) -> Result<Value> {
        let value=self.conn().query_row("SELECT id,task_id,revision,content_hash,selected_ai FROM requirement_task_bindings WHERE draft_id=?1",[d],|r|Ok(json!({"binding_id":r.get::<_,String>(0)?,"task_id":r.get::<_,String>(1)?,"revision":r.get::<_,i64>(2)?,"content_hash":r.get::<_,String>(3)?,"selected_ai":r.get::<_,String>(4)?,"draft_id":d}))).optional()?;
        Ok(value.unwrap_or(Value::Null))
    }
    pub(crate) fn workspace_activity(&self, p: &str, reserved: &[String]) -> Result<()> {
        for id in reserved {
            let task = self.task(id).map_err(|_| err("ACTIVITY_UNKNOWN"))?;
            if task.project_id == p {
                return Err(err("EXECUTION_IN_PROGRESS"));
            }
        }
        // Unknown persisted states fail closed, including states introduced by a newer build.
        let count:i64=self.conn().query_row("SELECT COUNT(*) FROM tasks WHERE project_id=?1 AND state NOT IN ('draft','awaiting_confirmation','confirmed','preview_ready','awaiting_user_acceptance','completed','failed','cancelled','interrupted','needs_user_action')",[p],|r|r.get(0))?;
        if count > 0 {
            return Err(err("EXECUTION_IN_PROGRESS"));
        }
        Ok(())
    }
    fn workspace_inner(&self, req: &Request, reserved: &[String]) -> Result<Value> {
        let p = &req.project_id;
        match &req.operation {
            Operation::Open {} => {
                self.conn().execute("INSERT INTO requirement_conversations VALUES (?1,?2,?3,?3) ON CONFLICT(project_id) DO NOTHING",params![new_id(),p,now()])?;
                let c: String = self.conn().query_row(
                    "SELECT id FROM requirement_conversations WHERE project_id=?1",
                    [p],
                    |r| r.get(0),
                )?;
                Ok(json!({"conversation_id":c}))
            }
            Operation::List {
                conversation_id: c,
                after_seq,
            } => {
                self.workspace_identity(p, c, None)?;
                let mut stmt=self.conn().prepare("SELECT id,seq,kind,body_raw,draft_id,revision FROM requirement_messages WHERE conversation_id=?1 AND seq>?2 ORDER BY seq LIMIT 100")?;
                let messages:Vec<Value>=stmt.query_map(params![c,after_seq],|r|Ok(json!({"id":r.get::<_,String>(0)?,"seq":r.get::<_,i64>(1)?,"kind":r.get::<_,String>(2)?,"body_raw":r.get::<_,Option<String>>(3)?,"draft_id":r.get::<_,String>(4)?,"revision":r.get::<_,i64>(5)?})))?.collect::<std::result::Result<_,_>>()?;
                let mut stmt=self.conn().prepare("SELECT id FROM requirement_drafts WHERE project_id=?1 AND conversation_id=?2 ORDER BY created_at,id")?;
                let ids: Vec<String> = stmt
                    .query_map(params![p, c], |r| r.get(0))?
                    .collect::<std::result::Result<_, _>>()?;
                let drafts: Vec<Value> = ids
                    .iter()
                    .map(|d| self.workspace_read(d))
                    .collect::<Result<_>>()?;
                Ok(json!({"messages":messages,"drafts":drafts}))
            }
            Operation::Create {
                conversation_id: c,
                origin_task_id: origin,
            } => {
                self.workspace_identity(p, c, None)?;
                if let Some(t) = origin {
                    if self
                        .task(t)
                        .map_err(|_| err("IDENTITY_MISMATCH"))?
                        .project_id
                        != *p
                    {
                        return Err(err("IDENTITY_MISMATCH"));
                    }
                }
                let existing:Option<String>=self.conn().query_row("SELECT id FROM requirement_drafts WHERE project_id=?1 AND status='editable' AND origin_task_id IS ?2",params![p,origin],|r|r.get(0)).optional()?;
                if let Some(d) = existing {
                    return self.workspace_read(&d);
                }
                let d = new_id();
                self.conn().execute(
                    "INSERT INTO requirement_drafts VALUES (?1,?2,?3,?4,?5,'editable',1,?6,?6)",
                    params![
                        d,
                        p,
                        c,
                        origin,
                        if origin.is_some() { "next" } else { "initial" },
                        now()
                    ],
                )?;
                self.workspace_revision(p, &d, 1, &Fields::default())?;
                self.workspace_message(p, c, &d, 1, "draft_created", None)?;
                self.workspace_read(&d)
            }
            Operation::Get {
                conversation_id: c,
                draft_id: d,
            } => {
                self.workspace_identity(p, c, Some(d))?;
                self.workspace_read(d)
            }
            Operation::Binding {
                conversation_id: c,
                draft_id: d,
            } => {
                self.workspace_identity(p, c, Some(d))?;
                self.workspace_bound(d)
            }
            Operation::Save {
                conversation_id: c,
                draft_id: d,
                expected_revision: r,
                fields,
            } => {
                self.workspace_identity(p, c, Some(d))?;
                self.workspace_editable(d, *r)?;
                self.workspace_revision(p, d, r + 1, fields)?;
                let n=self.conn().execute("UPDATE requirement_drafts SET current_revision=?1,updated_at=?2 WHERE id=?3 AND current_revision=?4 AND status='editable'",params![r+1,now(),d,r])?;
                if n != 1 {
                    return Err(err("REVISION_CONFLICT"));
                }
                self.workspace_message(p, c, d, r + 1, "revision_saved", None)?;
                self.workspace_read(d)
            }
            Operation::Append {
                conversation_id: c,
                draft_id: d,
                expected_revision: r,
                body_raw,
            } => {
                self.workspace_identity(p, c, Some(d))?;
                self.workspace_editable(d, *r)?;
                if body_raw.len() > 65536 {
                    return Err(err("INPUT_TOO_LARGE"));
                }
                self.workspace_message(p, c, d, *r, "user", Some(body_raw))?;
                Ok(json!({"draft_id":d,"saved":true}))
            }
            Operation::Archive {
                conversation_id: c,
                draft_id: d,
                expected_revision: r,
            } => {
                self.workspace_identity(p, c, Some(d))?;
                self.workspace_editable(d, *r)?;
                let unsettled:bool=self.conn().query_row("SELECT EXISTS(SELECT 1 FROM planning_requests WHERE draft_id=?1 AND status IN ('reserved','dispatching','running','uncertain'))",[d],|r|r.get(0))?;
                if unsettled {
                    return Err(err("PLANNING_BUSY"));
                }
                self.conn().execute(
                    "UPDATE requirement_drafts SET status='archived',updated_at=?1 WHERE id=?2",
                    params![now(), d],
                )?;
                self.workspace_read(d)
            }
            Operation::Confirm {
                conversation_id: c,
                draft_id: d,
                expected_revision: r,
                content_hash: h,
                selected_ai: ai,
            } => {
                self.workspace_identity(p, c, Some(d))?;
                let binding = self.workspace_bound(d)?;
                if !binding.is_null() {
                    if binding["revision"] == *r
                        && binding["content_hash"] == *h
                        && binding["selected_ai"] == *ai
                    {
                        return Ok(binding);
                    }
                    return Err(err("DRAFT_BOUND"));
                }
                self.workspace_editable(d, *r)?;
                let current = self.workspace_read(d)?;
                let f: Fields = serde_json::from_value(current["fields"].clone())
                    .map_err(|_| err("DATABASE_ERROR"))?;
                if content_hash(d, *r, &f) != *h || f.selected_ai.as_deref() != Some(ai) {
                    return Err(err("REVISION_CONFLICT"));
                }
                if [
                    &f.title_raw,
                    &f.goal_raw,
                    &f.in_scope_raw,
                    &f.not_in_scope_raw,
                    &f.done_criteria_raw,
                ]
                .iter()
                .any(|v| v.trim().is_empty())
                {
                    return Err(err("INCOMPLETE_SCOPE"));
                }
                let questions:String=self.conn().query_row("SELECT blocking_questions_json FROM requirement_revisions WHERE draft_id=?1 AND revision=?2",params![d,r],|row|row.get(0))?;
                if questions != "[]" {
                    return Err(err("UNRESOLVED_CLARIFICATION"));
                }
                self.workspace_activity(p, reserved)?;
                let task = self.create_task_in_transaction(
                    &CreateTaskRequest {
                        project_id: p.clone(),
                        title: f.title_raw,
                        goal: Some(f.goal_raw),
                        in_scope: Some(f.in_scope_raw),
                        not_in_scope: Some(f.not_in_scope_raw),
                        done_criteria: Some(f.done_criteria_raw),
                        ai_kind: Some(ai.clone()),
                    },
                    Actor::User,
                )?;
                self.transition_task_in_transaction(
                    &task.id,
                    TaskState::AwaitingConfirmation,
                    Actor::User,
                    None,
                    None,
                    false,
                    None,
                )?;
                self.transition_task_in_transaction(
                    &task.id,
                    TaskState::Confirmed,
                    Actor::User,
                    None,
                    None,
                    false,
                    None,
                )?;
                self.conn().execute(
                    "INSERT INTO requirement_task_bindings VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![new_id(), p, d, r, h, ai, task.id, now()],
                )?;
                self.conn().execute(
                    "UPDATE requirement_drafts SET status='bound',updated_at=?1 WHERE id=?2",
                    params![now(), d],
                )?;
                self.workspace_message(p, c, d, *r, "task_bound", None)?;
                self.workspace_bound(d)
            }
        }
    }
}

#[cfg(test)]
mod tests;
