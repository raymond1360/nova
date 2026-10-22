use super::*;
use crate::{
    models::{Actor, CreateProjectRequest, CreateTaskRequest, TaskState},
    storage::workspace::{Fields, Operation},
};
use serde_json::{json, Value};
use std::sync::{mpsc, Arc};
fn request(p: &str, operation: Operation) -> Request {
    Request {
        project_id: p.into(),
        client_request_id: Some(uuid::Uuid::new_v4().to_string()),
        operation,
    }
}
fn fixture() -> (Arc<Mutex<Database>>, String, String, String, Request) {
    let db = Database::memory().unwrap();
    db.migrate().unwrap();
    let path = std::env::temp_dir().join(format!("nova-coordination-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    let p = db
        .create_project(&CreateProjectRequest {
            name: "P".into(),
            local_path: path.to_string_lossy().into(),
            source_type: "local".into(),
            source_url: None,
            purpose: None,
            long_term_goal: None,
            user_preferences: None,
            prohibitions: None,
        })
        .unwrap()
        .id;
    let old = db
        .create_task(
            &CreateTaskRequest {
                project_id: p.clone(),
                title: "old".into(),
                goal: None,
                in_scope: None,
                not_in_scope: None,
                done_criteria: None,
                ai_kind: Some("codex_cli".into()),
            },
            Actor::User,
        )
        .unwrap()
        .id;
    let c = db.workspace(&request(&p, Operation::Open {}), &[]).unwrap()["conversation_id"]
        .as_str()
        .unwrap()
        .to_string();
    let d = db
        .workspace(
            &request(
                &p,
                Operation::Create {
                    conversation_id: c.clone(),
                    origin_task_id: Some(old.clone()),
                },
            ),
            &[],
        )
        .unwrap()["draft_id"]
        .as_str()
        .unwrap()
        .to_string();
    let f = Fields {
        title_raw: "new".into(),
        selected_ai: Some("codex_cli".into()),
        goal_raw: "g".into(),
        in_scope_raw: "s".into(),
        not_in_scope_raw: "n".into(),
        done_criteria_raw: "d".into(),
    };
    let v = db
        .workspace(
            &request(
                &p,
                Operation::Save {
                    conversation_id: c.clone(),
                    draft_id: d.clone(),
                    expected_revision: 1,
                    fields: f,
                },
            ),
            &[],
        )
        .unwrap();
    let confirm = request(
        &p,
        Operation::Confirm {
            conversation_id: c.clone(),
            draft_id: d,
            expected_revision: 2,
            content_hash: v["content_hash"].as_str().unwrap().into(),
            selected_ai: "codex_cli".into(),
        },
    );
    (Arc::new(Mutex::new(db)), p, c, old, confirm)
}
#[test]
fn start_reservation_linearizes_before_confirmation_even_before_state_write() {
    let (db, _, _, old, confirm) = fixture();
    let runs = Arc::new(Mutex::new(HashMap::new()));
    let (entered_rx, entered_tx) = (mpsc::channel::<()>(), mpsc::channel::<()>());
    let (started_send, started_recv) = entered_rx;
    let (release_send, release_recv) = entered_tx;
    let d = db.clone();
    let r = runs.clone();
    let id = old.clone();
    let start = std::thread::spawn(move || {
        reserve(&r, &d, &id, CancellationToken::new(), |_| {
            started_send.send(()).unwrap();
            release_recv.recv().unwrap();
            Ok(())
        })
    });
    started_recv.recv().unwrap();
    let d = db.clone();
    let r = runs.clone();
    let confirm_thread = std::thread::spawn(move || execute(&r, &d, &confirm));
    release_send.send(()).unwrap();
    start.join().unwrap().unwrap();
    assert_eq!(
        confirm_thread.join().unwrap().unwrap_err().code,
        "EXECUTION_IN_PROGRESS"
    );
    assert_eq!(
        db.lock().unwrap().task(&old).unwrap().state,
        TaskState::Draft
    );
}
#[test]
fn confirmation_holds_coordination_lock_until_commit_then_future_start_is_allowed() {
    let (db, _, _, old, confirm) = fixture();
    let runs = Arc::new(Mutex::new(HashMap::new()));
    // Same production order, with a controlled pause before executing the transaction.
    let held = runs.lock().unwrap();
    let d = db.clone();
    let r = runs.clone();
    let id = old.clone();
    let (ready, receiver) = mpsc::channel();
    let start = std::thread::spawn(move || {
        ready.send(()).unwrap();
        reserve(&r, &d, &id, CancellationToken::new(), |_| Ok(()))
    });
    receiver.recv().unwrap();
    let value = db
        .lock()
        .unwrap()
        .workspace(&confirm, &held.keys().cloned().collect::<Vec<_>>())
        .unwrap();
    assert!(value["task_id"].is_string());
    assert!(held.is_empty());
    drop(held);
    start.join().unwrap().unwrap();
    assert!(runs.lock().unwrap().contains_key(&old));
}
#[test]
fn persisted_active_and_unknown_reservations_fail_closed_and_blocked_has_no_dispatch() {
    let (db, p, _, old, confirm) = fixture();
    let runs = Mutex::new(HashMap::new());
    db.lock()
        .unwrap()
        .conn()
        .execute("UPDATE tasks SET state='verifying' WHERE id=?1", [&old])
        .unwrap();
    assert_eq!(
        execute(&runs, &db, &confirm).unwrap_err().code,
        "EXECUTION_IN_PROGRESS"
    );
    runs.lock()
        .unwrap()
        .insert("unknown".into(), CancellationToken::new());
    assert_eq!(
        execute(&runs, &db, &confirm).unwrap_err().code,
        "ACTIVITY_UNKNOWN"
    );
    let capability = get_planning_capability();
    assert_eq!(capability["status"], "blocked");
    assert_eq!(capability["ai_calls"], 0);
    assert!(serde_json::from_value::<Request>(
        json!({"project_id":p,"operation":{"op":"request_requirement_plan"}})
    )
    .is_err());
    let value: Value = db
        .lock()
        .unwrap()
        .conn()
        .query_row("SELECT COUNT(*) FROM task_actions", [], |r| {
            Ok(json!(r.get::<_, i64>(0)?))
        })
        .unwrap();
    assert_eq!(value, 0);
}

#[test]
fn rejected_start_keeps_the_confirmed_binding_and_does_not_create_another_task() {
    let (db, _, _, _, confirm) = fixture();
    let runs = Mutex::new(HashMap::new());
    let bound = execute(&runs, &db, &confirm).unwrap();
    let id = bound["task_id"].as_str().unwrap();
    let rejected: Result<(), String> = reserve(&runs, &db, id, CancellationToken::new(), |_| {
        Err("mock startup condition failed; no CLI invocation".into())
    });
    assert!(rejected.is_err());
    assert!(runs.lock().unwrap().is_empty());
    let replay = execute(&runs, &db, &confirm).unwrap();
    assert_eq!(replay["task_id"], bound["task_id"]);
    assert_eq!(
        db.lock().unwrap().task(id).unwrap().state,
        TaskState::Confirmed
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .conn()
            .query_row("SELECT count(*) FROM requirement_task_bindings", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        1
    );
    assert_eq!(
        db.lock()
            .unwrap()
            .conn()
            .query_row("SELECT count(*) FROM task_actions", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
}
