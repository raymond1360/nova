use super::*;
use crate::models::CreateProjectRequest;
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
    thread,
};

fn dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!("nova-workspace-{}", new_id()));
    std::fs::create_dir_all(&p).unwrap();
    p
}
pub(crate) fn project(db: &Database) -> String {
    db.create_project(&CreateProjectRequest {
        name: "fixture".into(),
        local_path: dir().to_string_lossy().into(),
        source_type: "local".into(),
        source_url: None,
        purpose: None,
        long_term_goal: None,
        user_preferences: None,
        prohibitions: None,
    })
    .unwrap()
    .id
}
pub(crate) fn req(p: &str, operation: Operation) -> Request {
    Request {
        project_id: p.into(),
        client_request_id: Some(new_id()),
        operation,
    }
}
pub(crate) fn prepared(db: &Database, p: &str) -> (String, String, Value) {
    let c = db.workspace(&req(p, Operation::Open {}), &[]).unwrap()["conversation_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let value = db
        .workspace(
            &req(
                p,
                Operation::Create {
                    conversation_id: c.clone(),
                    origin_task_id: None,
                },
            ),
            &[],
        )
        .unwrap();
    let d = value["draft_id"].as_str().unwrap().to_owned();
    let fields = Fields {
        title_raw: " title ".into(),
        selected_ai: Some("codex_cli".into()),
        goal_raw: "  one\r\none\r\n\n  原文  ".into(),
        in_scope_raw: "x\nx".into(),
        not_in_scope_raw: "不改其它".into(),
        done_criteria_raw: "精确\n检查".into(),
    };
    let value = db
        .workspace(
            &req(
                p,
                Operation::Save {
                    conversation_id: c.clone(),
                    draft_id: d.clone(),
                    expected_revision: 1,
                    fields,
                },
            ),
            &[],
        )
        .unwrap();
    (c, d, value)
}
pub(crate) fn confirm(p: &str, c: &str, d: &str, v: &Value) -> Request {
    req(
        p,
        Operation::Confirm {
            conversation_id: c.into(),
            draft_id: d.into(),
            expected_revision: v["revision"].as_i64().unwrap(),
            content_hash: v["content_hash"].as_str().unwrap().into(),
            selected_ai: "codex_cli".into(),
        },
    )
}
fn memory() -> Database {
    let db = Database::memory().unwrap();
    db.migrate().unwrap();
    db
}
fn count(db: &Database, table: &str) -> i64 {
    db.conn()
        .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

#[test]
fn exact_text_reopen_confirm_and_lost_response_replay_without_start() {
    let path = dir().join("fixture.sqlite3");
    let db = Database::open(&path).unwrap();
    db.migrate().unwrap();
    let p = project(&db);
    let (c, d, v) = prepared(&db, &p);
    drop(db);
    let db = Database::open(&path).unwrap();
    db.migrate().unwrap();
    let reloaded = db
        .workspace(
            &req(
                &p,
                Operation::Get {
                    conversation_id: c.clone(),
                    draft_id: d.clone(),
                },
            ),
            &[],
        )
        .unwrap();
    assert_eq!(reloaded["fields"], v["fields"]);
    assert_eq!(reloaded["content_hash"], v["content_hash"]);
    let request = confirm(&p, &c, &d, &v);
    let first = db.workspace(&request, &[]).unwrap();
    let replay = db.workspace(&request, &[]).unwrap();
    assert_eq!(first["task_id"], replay["task_id"]);
    assert_eq!(replay["replayed"], true);
    let again = db.workspace(&confirm(&p, &c, &d, &v), &[]).unwrap();
    assert_eq!(first["task_id"], again["task_id"]);
    assert_eq!(count(&db, "tasks"), 1);
    assert_eq!(count(&db, "task_actions"), 0);
    assert_eq!(count(&db, "user_acceptance"), 0);
    let task = db.task(first["task_id"].as_str().unwrap()).unwrap();
    assert_eq!(task.state, TaskState::Confirmed);
    assert_eq!(task.goal.as_deref(), v["fields"]["goal_raw"].as_str());
    assert_eq!(task.result_generation, 0);
    assert!(task.test_command_redacted.is_none());
    let snapshot:Vec<String>=db.conn().prepare("SELECT item_text_redacted FROM confirmed_requirement_items WHERE task_id=?1 AND field='goal' ORDER BY item_index").unwrap().query_map([task.id],|r|r.get(0)).unwrap().collect::<std::result::Result<_,_>>().unwrap();
    assert_eq!(snapshot, vec!["one", "one", "原文"]);
    // A failed start gate cannot undo the binding or authorize another task.
    assert!(db
        .transition_task(
            first["task_id"].as_str().unwrap(),
            TaskState::Completed,
            Actor::User,
            None,
            None,
            false,
            None
        )
        .is_err());
    assert_eq!(
        db.workspace(
            &req(
                &p,
                Operation::Binding {
                    conversation_id: c,
                    draft_id: d
                }
            ),
            &[]
        )
        .unwrap()["task_id"],
        first["task_id"]
    );
}
#[test]
fn save_receipt_and_message_order_identity_and_old_revision() {
    let db = memory();
    let p = project(&db);
    let (c, d, v) = prepared(&db, &p);
    let mut request = req(
        &p,
        Operation::Save {
            conversation_id: c.clone(),
            draft_id: d.clone(),
            expected_revision: 2,
            fields: serde_json::from_value(v["fields"].clone()).unwrap(),
        },
    );
    let a = db.workspace(&request, &[]).unwrap();
    assert_eq!(
        db.workspace(&request, &[]).unwrap()["revision"],
        a["revision"]
    );
    if let Operation::Save { fields, .. } = &mut request.operation {
        fields.goal_raw.push('!');
    }
    assert_eq!(
        db.workspace(&request, &[]).unwrap_err().code,
        "IDEMPOTENCY_CONFLICT"
    );
    assert_eq!(
        db.workspace(&confirm(&p, &c, &d, &v), &[])
            .unwrap_err()
            .code,
        "REVISION_CONFLICT"
    );
    let p2 = project(&db);
    assert_eq!(
        db.workspace(
            &req(
                &p2,
                Operation::Get {
                    conversation_id: c.clone(),
                    draft_id: d.clone()
                }
            ),
            &[]
        )
        .unwrap_err()
        .code,
        "IDENTITY_MISMATCH"
    );
    let append = req(
        &p,
        Operation::Append {
            conversation_id: c.clone(),
            draft_id: d.clone(),
            expected_revision: 3,
            body_raw: "  用户\r\n原文\r\n".into(),
        },
    );
    db.workspace(&append, &[]).unwrap();
    db.workspace(&append, &[]).unwrap();
    let list = db
        .workspace(
            &req(
                &p,
                Operation::List {
                    conversation_id: c,
                    after_seq: 0,
                },
            ),
            &[],
        )
        .unwrap();
    let messages = list["messages"].as_array().unwrap();
    assert_eq!(messages.last().unwrap()["body_raw"], "  用户\r\n原文\r\n");
    for (i, m) in messages.iter().enumerate() {
        assert_eq!(m["seq"], i + 1);
    }
    assert_eq!(count(&db, "task_actions"), 0);
    assert_eq!(count(&db, "tasks"), 0);
}
#[test]
fn two_connections_save_conflict_and_concurrent_confirm_unique() {
    let path = dir().join("fixture.sqlite3");
    let db = Database::open(&path).unwrap();
    db.migrate().unwrap();
    let p = project(&db);
    let (c, d, v) = prepared(&db, &p);
    let barrier = Arc::new(Barrier::new(2));
    let jobs: Vec<_> = (0..2)
        .map(|i| {
            let path = path.clone();
            let b = barrier.clone();
            let (p, c, d, v) = (p.clone(), c.clone(), d.clone(), v.clone());
            thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                db.migrate().unwrap();
                let mut fields: Fields = serde_json::from_value(v["fields"].clone()).unwrap();
                fields.title_raw = format!("edit {i}");
                b.wait();
                db.workspace(
                    &req(
                        &p,
                        Operation::Save {
                            conversation_id: c,
                            draft_id: d,
                            expected_revision: 2,
                            fields,
                        },
                    ),
                    &[],
                )
            })
        })
        .collect();
    let values: Vec<_> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
    assert_eq!(values.iter().filter(|v| v.is_ok()).count(), 1);
    assert_eq!(
        values.iter().find_map(|v| v.as_ref().err()).unwrap().code,
        "REVISION_CONFLICT"
    );
    let latest = db
        .workspace(
            &req(
                &p,
                Operation::Get {
                    conversation_id: c.clone(),
                    draft_id: d.clone(),
                },
            ),
            &[],
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let jobs: Vec<_> = (0..2)
        .map(|_| {
            let path = path.clone();
            let b = barrier.clone();
            let request = confirm(&p, &c, &d, &latest);
            thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                db.migrate().unwrap();
                b.wait();
                db.workspace(&request, &[]).unwrap()
            })
        })
        .collect();
    let results: Vec<_> = jobs.into_iter().map(|j| j.join().unwrap()).collect();
    assert_eq!(results[0]["task_id"], results[1]["task_id"]);
    assert_eq!(count(&db, "tasks"), 1);
    assert_eq!(count(&db, "requirement_task_bindings"), 1);
}
#[test]
fn each_confirmation_write_failure_rolls_back_all_writes() {
    let stages = [
        ("tasks", "INSERT", ""),
        ("task_state_events", "INSERT", "WHEN NEW.new_state='draft'"),
        (
            "task_state_events",
            "INSERT",
            "WHEN NEW.new_state='awaiting_confirmation'",
        ),
        ("confirmed_requirement_items", "INSERT", ""),
        (
            "task_state_events",
            "INSERT",
            "WHEN NEW.new_state='confirmed'",
        ),
        ("tasks", "UPDATE", "WHEN NEW.state='confirmed'"),
        ("requirement_task_bindings", "INSERT", ""),
        ("requirement_drafts", "UPDATE", ""),
        ("requirement_messages", "INSERT", ""),
        ("requirement_conversations", "UPDATE", ""),
        ("requirement_mutation_receipts", "INSERT", ""),
    ];
    for (table, operation, when) in stages {
        let db = memory();
        let p = project(&db);
        let (c, d, v) = prepared(&db, &p);
        let before = (
            count(&db, "requirement_messages"),
            count(&db, "requirement_mutation_receipts"),
        );
        db.conn().execute_batch(&format!("CREATE TEMP TRIGGER fail_confirm BEFORE {operation} ON {table} {when} BEGIN SELECT RAISE(ABORT,'injected'); END;")).unwrap();
        assert!(
            db.workspace(&confirm(&p, &c, &d, &v), &[]).is_err(),
            "{table}/{operation}/{when}"
        );
        for table in [
            "tasks",
            "task_state_events",
            "confirmed_requirement_items",
            "requirement_task_bindings",
        ] {
            assert_eq!(count(&db, table), 0, "{table}");
        }
        assert_eq!(
            before,
            (
                count(&db, "requirement_messages"),
                count(&db, "requirement_mutation_receipts")
            )
        );
        assert_eq!(db.workspace_read(&d).unwrap()["status"], "editable");
        assert!(db.conn().is_autocommit());
    }
}
#[test]
fn next_draft_preserves_execution_and_origin_and_activity_gates() {
    let db = memory();
    let p = project(&db);
    let (c, d, v) = prepared(&db, &p);
    let bound = db.workspace(&confirm(&p, &c, &d, &v), &[]).unwrap();
    let id = bound["task_id"].as_str().unwrap();
    db.transition_task(id, TaskState::Queued, Actor::User, None, None, false, None)
        .unwrap();
    let before = serde_json::to_value(db.task(id).unwrap()).unwrap();
    let next = db
        .workspace(
            &req(
                &p,
                Operation::Create {
                    conversation_id: c.clone(),
                    origin_task_id: Some(id.into()),
                },
            ),
            &[],
        )
        .unwrap();
    let nd = next["draft_id"].as_str().unwrap();
    let fields: Fields = serde_json::from_value(v["fields"].clone()).unwrap();
    let next = db
        .workspace(
            &req(
                &p,
                Operation::Save {
                    conversation_id: c.clone(),
                    draft_id: nd.into(),
                    expected_revision: 1,
                    fields,
                },
            ),
            &[],
        )
        .unwrap();
    assert_eq!(
        db.workspace(&confirm(&p, &c, nd, &next), &[])
            .unwrap_err()
            .code,
        "EXECUTION_IN_PROGRESS"
    );
    assert_eq!(before, serde_json::to_value(db.task(id).unwrap()).unwrap());
    assert_eq!(count(&db, "task_actions"), 0);
    let p2 = project(&db);
    let c2 = db.workspace(&req(&p2, Operation::Open {}), &[]).unwrap()["conversation_id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        db.workspace(
            &req(
                &p2,
                Operation::Create {
                    conversation_id: c2,
                    origin_task_id: Some(id.into())
                }
            ),
            &[]
        )
        .unwrap_err()
        .code,
        "IDENTITY_MISMATCH"
    );
    assert_eq!(
        db.workspace_activity(&p, &["unknown".into()])
            .unwrap_err()
            .code,
        "ACTIVITY_UNKNOWN"
    );
}
#[test]
fn bound_archived_and_incomplete_are_not_mutable_or_confirmable() {
    let db = memory();
    let p = project(&db);
    let (c, d, v) = prepared(&db, &p);
    db.workspace(&confirm(&p, &c, &d, &v), &[]).unwrap();
    assert_eq!(
        db.workspace(
            &req(
                &p,
                Operation::Save {
                    conversation_id: c.clone(),
                    draft_id: d,
                    expected_revision: 2,
                    fields: Fields::default()
                }
            ),
            &[]
        )
        .unwrap_err()
        .code,
        "DRAFT_BOUND"
    );
    let blank = db
        .workspace(
            &req(
                &p,
                Operation::Create {
                    conversation_id: c.clone(),
                    origin_task_id: None,
                },
            ),
            &[],
        )
        .unwrap();
    let d = blank["draft_id"].as_str().unwrap();
    assert!(db.workspace(&confirm(&p, &c, d, &blank), &[]).is_err());
    db.workspace(
        &req(
            &p,
            Operation::Archive {
                conversation_id: c.clone(),
                draft_id: d.into(),
                expected_revision: 1,
            },
        ),
        &[],
    )
    .unwrap();
    assert_eq!(
        db.workspace(
            &req(
                &p,
                Operation::Append {
                    conversation_id: c,
                    draft_id: d.into(),
                    expected_revision: 1,
                    body_raw: "x".into()
                }
            ),
            &[]
        )
        .unwrap_err()
        .code,
        "DRAFT_ARCHIVED"
    );
}
#[test]
fn ipc_request_roundtrip_is_strict() {
    let request = req("project", Operation::Open {});
    let raw = serde_json::to_value(&request).unwrap();
    assert!(serde_json::from_value::<Request>(raw).is_ok());
    assert!(serde_json::from_value::<Request>(
        json!({"project_id":"p","operation":{"op":"dispatch_plan"}})
    )
    .is_err());
    assert!(serde_json::from_value::<Request>(
        json!({"project_id":"p","operation":{"op":"open","state":"completed"}})
    )
    .is_err());
}
