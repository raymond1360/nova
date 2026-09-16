//! Concurrency tests exercising the parts of the storage layer that can
//! genuinely be touched by more than one OS-level connection at once — most
//! plausibly two Nova processes pointed at the same app-data directory
//! (the user double-launching the app), since within a single process every
//! Tauri command already serializes on `AppState.db: Arc<Mutex<Database>>`.
//! These use real file-backed SQLite databases and real OS threads, never
//! the user's actual database.

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Barrier};
use std::thread;

use crate::db::Database;
use crate::models::{ActionStatus, Actor, CreateProjectRequest, CreateTaskRequest, TaskState};

fn temp_dir(name: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "nova-concurrency-test-{name}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn open_migrated(path: &std::path::Path) -> Database {
    let db = Database::open(path).unwrap();
    db.migrate().unwrap();
    db
}

#[test]
fn two_threads_transitioning_the_same_task_concurrently_only_one_wins() {
    let harness_dir = temp_dir("cas-transition");
    let project_dir = temp_dir("cas-transition-project");
    let db_path = harness_dir.join("nova.sqlite3");

    let setup = open_migrated(&db_path);
    let project = setup
        .create_project(&CreateProjectRequest {
            name: "P".to_string(),
            local_path: project_dir.to_string_lossy().into_owned(),
            source_type: "local".to_string(),
            source_url: None,
            purpose: None,
            long_term_goal: None,
            user_preferences: None,
            prohibitions: None,
        })
        .unwrap();
    let task = setup
        .create_task(
            &CreateTaskRequest {
                project_id: project.id.clone(),
                title: "T".to_string(),
                goal: None,
                in_scope: None,
                not_in_scope: None,
                done_criteria: None,
                ai_kind: None,
            },
            Actor::User,
        )
        .unwrap();
    setup
        .transition_task(
            &task.id,
            TaskState::AwaitingConfirmation,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
    setup
        .transition_task(
            &task.id,
            TaskState::Confirmed,
            Actor::User,
            None,
            None,
            false,
            None,
        )
        .unwrap();
    setup
        .transition_task(
            &task.id,
            TaskState::Queued,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
    setup
        .transition_task(
            &task.id,
            TaskState::Running,
            Actor::System,
            None,
            None,
            false,
            None,
        )
        .unwrap();
    drop(setup);

    let barrier = Arc::new(Barrier::new(2));
    let task_id = task.id.clone();

    let handles: Vec<_> = [(); 2]
        .into_iter()
        .map(|_| {
            let path = db_path.clone();
            let barrier = Arc::clone(&barrier);
            let task_id = task_id.clone();
            thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                barrier.wait();
                db.transition_task(
                    &task_id,
                    TaskState::Verifying,
                    Actor::ConnectedAi,
                    None,
                    None,
                    false,
                    None,
                )
            })
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    let succeeded = results.iter().filter(|r| r.is_ok()).count();
    let failed = results.iter().filter(|r| r.is_err()).count();

    assert_eq!(
        succeeded, 1,
        "exactly one of two concurrent identical transitions must win: {results:?}"
    );
    assert_eq!(
        failed, 1,
        "the loser must get a clean error, not a panic or a silent second transition"
    );

    let verify_conn = open_migrated(&db_path);
    let final_task = verify_conn.task(&task_id).unwrap();
    assert_eq!(final_task.state, TaskState::Verifying);

    let events = verify_conn.list_task_state_events(&task_id).unwrap();
    let verifying_events = events
        .iter()
        .filter(|e| e.new_state == TaskState::Verifying)
        .count();
    assert_eq!(
        verifying_events, 1,
        "a concurrently-lost transition must never also write a duplicate task_state_events row"
    );

    let _ = fs::remove_dir_all(&harness_dir);
    let _ = fs::remove_dir_all(&project_dir);
}

#[test]
fn concurrent_planning_of_the_same_idempotency_key_never_creates_two_ledger_rows() {
    let harness_dir = temp_dir("cas-idempotency");
    let project_dir = temp_dir("cas-idempotency-project");
    let db_path = harness_dir.join("nova.sqlite3");

    let setup = open_migrated(&db_path);
    let project = setup
        .create_project(&CreateProjectRequest {
            name: "P".to_string(),
            local_path: project_dir.to_string_lossy().into_owned(),
            source_type: "local".to_string(),
            source_url: None,
            purpose: None,
            long_term_goal: None,
            user_preferences: None,
            prohibitions: None,
        })
        .unwrap();
    let task = setup
        .create_task(
            &CreateTaskRequest {
                project_id: project.id.clone(),
                title: "T".to_string(),
                goal: None,
                in_scope: None,
                not_in_scope: None,
                done_criteria: None,
                ai_kind: None,
            },
            Actor::User,
        )
        .unwrap();
    drop(setup);

    let barrier = Arc::new(Barrier::new(5));
    let task_id = task.id.clone();
    let handles: Vec<_> = (0..5)
        .map(|_| {
            let path = db_path.clone();
            let barrier = Arc::clone(&barrier);
            let task_id = task_id.clone();
            thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                barrier.wait();
                db.plan_action(&task_id, "git_commit", "shared-key")
            })
        })
        .collect();

    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    // Every call must return Ok (plan_action is defined as idempotent —
    // "already planned" is success, not an error) — the correctness
    // property under test is that they all converge on the same row.
    for result in &results {
        assert!(
            result.is_ok(),
            "plan_action must never panic or hard-fail under a benign race: {results:?}"
        );
    }

    let verify_conn = open_migrated(&db_path);
    let count: i64 = verify_conn
        .conn()
        .query_row(
            "SELECT COUNT(*) FROM task_actions WHERE task_id = ?1 AND idempotency_key = 'shared-key'",
            rusqlite::params![task_id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        count, 1,
        "five concurrent plans of the same key must leave exactly one ledger row"
    );

    let _ = fs::remove_dir_all(&harness_dir);
    let _ = fs::remove_dir_all(&project_dir);
}

#[test]
fn concurrent_writers_under_contention_never_panic_and_never_report_raw_busy_as_success() {
    let harness_dir = temp_dir("busy-contention");
    let project_dir = temp_dir("busy-contention-project");
    let db_path = harness_dir.join("nova.sqlite3");

    let setup = open_migrated(&db_path);
    let project = setup
        .create_project(&CreateProjectRequest {
            name: "P".to_string(),
            local_path: project_dir.to_string_lossy().into_owned(),
            source_type: "local".to_string(),
            source_url: None,
            purpose: None,
            long_term_goal: None,
            user_preferences: None,
            prohibitions: None,
        })
        .unwrap();
    drop(setup);

    let barrier = Arc::new(Barrier::new(8));
    let handles: Vec<_> = (0..8)
        .map(|i| {
            let path = db_path.clone();
            let barrier = Arc::clone(&barrier);
            let project_id = project.id.clone();
            thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                barrier.wait();
                db.create_task(
                    &CreateTaskRequest {
                        project_id,
                        title: format!("concurrent task {i}"),
                        goal: None,
                        in_scope: None,
                        not_in_scope: None,
                        done_criteria: None,
                        ai_kind: None,
                    },
                    Actor::User,
                )
            })
        })
        .collect();

    // The point of this test: none of these `.join()` calls may return
    // `Err` (a panic inside the thread) — `busy_timeout` (see
    // `Database::open`) must make SQLite's write-lock contention resolve
    // via an internal wait/retry, not a raw `SQLITE_BUSY` bubbling up as an
    // uncaught panic.
    let results: Vec<_> = handles.into_iter().map(|h| h.join()).collect();
    for result in &results {
        assert!(
            result.is_ok(),
            "a thread panicked under write contention instead of returning a clean Result"
        );
    }
    let ok_count = results
        .into_iter()
        .filter(|joined| joined.as_ref().unwrap().is_ok())
        .count();
    assert_eq!(
        ok_count, 8,
        "busy_timeout should let all 8 well-formed, non-conflicting writes eventually succeed"
    );

    let verify_conn = open_migrated(&db_path);
    let tasks = verify_conn.list_tasks_for_project(&project.id).unwrap();
    assert_eq!(tasks.len(), 8);

    let _ = fs::remove_dir_all(&harness_dir);
    let _ = fs::remove_dir_all(&project_dir);
}

#[test]
fn concurrent_last_active_project_switches_converge_to_exactly_one_project() {
    let harness_dir = temp_dir("last-active-race");
    let dir_a = temp_dir("last-active-race-a");
    let dir_b = temp_dir("last-active-race-b");
    let db_path = harness_dir.join("nova.sqlite3");

    let setup = open_migrated(&db_path);
    let project_a = setup
        .create_project(&CreateProjectRequest {
            name: "A".to_string(),
            local_path: dir_a.to_string_lossy().into_owned(),
            source_type: "local".to_string(),
            source_url: None,
            purpose: None,
            long_term_goal: None,
            user_preferences: None,
            prohibitions: None,
        })
        .unwrap();
    let project_b = setup
        .create_project(&CreateProjectRequest {
            name: "B".to_string(),
            local_path: dir_b.to_string_lossy().into_owned(),
            source_type: "local".to_string(),
            source_url: None,
            purpose: None,
            long_term_goal: None,
            user_preferences: None,
            prohibitions: None,
        })
        .unwrap();
    drop(setup);

    let barrier = Arc::new(Barrier::new(2));
    let ids = [project_a.id.clone(), project_b.id.clone()];
    let handles: Vec<_> = ids
        .into_iter()
        .map(|id| {
            let path = db_path.clone();
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let db = Database::open(&path).unwrap();
                barrier.wait();
                db.set_last_active_project(&id)
            })
        })
        .collect();

    for handle in handles {
        handle.join().unwrap().unwrap();
    }

    let verify_conn = open_migrated(&db_path);
    let active_first = verify_conn.last_active_project().unwrap().unwrap();
    let active_second = verify_conn.last_active_project().unwrap().unwrap();
    assert_eq!(
        active_first.id, active_second.id,
        "repeated reads right after a race must agree with each other"
    );
    assert!(
        active_first.id == project_a.id || active_first.id == project_b.id,
        "the winner must be one of the two real projects, never a corrupted/blank value"
    );

    let _ = fs::remove_dir_all(&harness_dir);
    let _ = fs::remove_dir_all(&dir_a);
    let _ = fs::remove_dir_all(&dir_b);
}

#[test]
fn startup_recovery_scan_runs_safely_alongside_a_concurrent_plain_read() {
    let harness_dir = temp_dir("recovery-vs-read");
    let project_dir = temp_dir("recovery-vs-read-project");
    let db_path = harness_dir.join("nova.sqlite3");

    let setup = open_migrated(&db_path);
    let project = setup
        .create_project(&CreateProjectRequest {
            name: "P".to_string(),
            local_path: project_dir.to_string_lossy().into_owned(),
            source_type: "local".to_string(),
            source_url: None,
            purpose: None,
            long_term_goal: None,
            user_preferences: None,
            prohibitions: None,
        })
        .unwrap();
    setup.set_last_active_project(&project.id).unwrap();
    let task = setup
        .create_task(
            &CreateTaskRequest {
                project_id: project.id.clone(),
                title: "T".to_string(),
                goal: None,
                in_scope: None,
                not_in_scope: None,
                done_criteria: None,
                ai_kind: None,
            },
            Actor::User,
        )
        .unwrap();
    for state in [
        TaskState::AwaitingConfirmation,
        TaskState::Confirmed,
        TaskState::Queued,
        TaskState::Running,
    ] {
        setup
            .transition_task(&task.id, state, Actor::System, None, None, false, None)
            .unwrap();
    }
    setup
        .plan_action(&task.id, "git_commit", "recovery-race-key")
        .unwrap();
    setup
        .start_action(&task.id, "recovery-race-key", false)
        .unwrap();
    drop(setup);

    let barrier = Arc::new(Barrier::new(2));
    let recovery_path = db_path.clone();
    let recovery_barrier = Arc::clone(&barrier);
    let recovery_handle = thread::spawn(move || {
        let db = Database::open(&recovery_path).unwrap();
        recovery_barrier.wait();
        db.run_startup_recovery()
    });

    let read_path = db_path.clone();
    let read_barrier = Arc::clone(&barrier);
    let project_id = project.id.clone();
    let read_handle = thread::spawn(move || {
        let db = Database::open(&read_path).unwrap();
        read_barrier.wait();
        db.list_tasks_for_project(&project_id)
    });

    let recovery_result = recovery_handle.join().unwrap();
    let read_result = read_handle.join().unwrap();

    assert!(
        recovery_result.is_ok(),
        "recovery scan must not panic when racing a concurrent read: {recovery_result:?}"
    );
    assert!(
        read_result.is_ok(),
        "a plain read must not panic when racing a concurrent recovery scan: {read_result:?}"
    );

    let verify_conn = open_migrated(&db_path);
    let final_task = verify_conn.task(&task.id).unwrap();
    assert_eq!(final_task.state, TaskState::Interrupted);
    let action = verify_conn
        .find_action(&task.id, "recovery-race-key")
        .unwrap()
        .unwrap();
    assert_eq!(action.status, ActionStatus::Uncertain);

    let _ = fs::remove_dir_all(&harness_dir);
    let _ = fs::remove_dir_all(&project_dir);
}
