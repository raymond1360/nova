use super::*;
use std::sync::{Arc, Barrier};
fn v11(conn: &Connection) {
    for m in MIGRATIONS.iter().filter(|m| m.version <= 11) {
        if m.version == 5 {
            quarantine_rows_v5_will_drop(conn).unwrap();
        }
        conn.execute_batch(m.sql).unwrap();
        conn.pragma_update(None, "user_version", m.version).unwrap();
    }
    conn.execute(
        "INSERT INTO providers VALUES ('fixture','name','https://example.invalid',NULL,0,'old')",
        [],
    )
    .unwrap();
    conn.execute("INSERT INTO nova_projects (id,name,local_path,normalized_path,source_type,last_activity_at,created_at,updated_at,last_opened_at,data_format_version) VALUES ('p','P','fixture','fixture','local','t','t','t','t',1)",[]).unwrap();
    conn.execute("INSERT INTO tasks (id,project_id,title,goal,state,created_at,updated_at) VALUES ('t','p','Title','raw','draft','old','old')",[]).unwrap();
}
fn dump(conn: &Connection) -> Vec<(String, Vec<String>)> {
    let names:Vec<String>=conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<Result<_,_>>().unwrap();
    names
        .into_iter()
        .map(|name| {
            let mut stmt = conn
                .prepare(&format!("SELECT * FROM \"{name}\" ORDER BY rowid"))
                .unwrap();
            let n = stmt.column_count();
            let values = stmt
                .query_map([], |r| {
                    Ok((0..n)
                        .map(|i| format!("{:?}", r.get_ref(i).unwrap()))
                        .collect::<Vec<_>>()
                        .join("|"))
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            (name, values)
        })
        .collect()
}
fn path() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("nova-v12-{}", Uuid::new_v4()));
    fs::create_dir_all(&dir).unwrap();
    dir.join("fixture.sqlite3")
}
#[test]
fn v12_only_adds_tables_without_modifying_any_old_business_row() {
    let conn = Connection::open_in_memory().unwrap();
    v11(&conn);
    conn.execute_batch("INSERT INTO confirmed_requirement_items VALUES ('req','p','t','goal',0,'raw','old');
        INSERT INTO task_actions (id,task_id,action_type,idempotency_key,status,created_at,updated_at) VALUES ('a','t','ai_development','old-action','succeeded','old','old');
        INSERT INTO test_evidence (id,project_id,task_id,test_command_redacted,passed_count,failed_count,skipped_count,exit_code,created_at,result_generation,round,result_attempt,status) VALUES ('test','p','t','npm test',2,0,0,0,'old',0,1,1,'passed');
        INSERT INTO user_acceptance VALUES ('accepted','p','t',0,1,'fixture acceptance','old','old');
        INSERT INTO generated_documents (id,project_id,task_id,document_type,file_path,generated_by,generated_at,created_at) VALUES ('doc','p','t','TEST_REPORT','fixture-only','nova','old','old');").unwrap();
    let before = dump(&conn);
    run(&conn, None).unwrap();
    let after = dump(&conn);
    for old in &before {
        assert_eq!(Some(old), after.iter().find(|v| v.0 == old.0), "{}", old.0);
    }
    assert_eq!(after.len(), before.len() + 8);
    assert_eq!(
        conn.query_row("PRAGMA foreign_key_check", [], |_| Ok(()))
            .optional()
            .unwrap(),
        None
    );
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        12
    );
}
#[test]
fn v12_backup_failure_stops_before_ddl() {
    let p = path();
    let conn = Connection::open(&p).unwrap();
    v11(&conn);
    let before = dump(&conn);
    fs::write(p.parent().unwrap().join("backups"), b"not a directory").unwrap();
    assert!(run(&conn, Some(&p)).is_err());
    assert_eq!(dump(&conn), before);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        11
    );
}
#[test]
fn v12_ddl_failure_rolls_back_created_index_and_all_tables() {
    let conn = Connection::open_in_memory().unwrap();
    v11(&conn);
    conn.execute_batch("CREATE TABLE requirement_revisions (collision TEXT);")
        .unwrap();
    let before = dump(&conn);
    assert!(run(&conn, None).is_err());
    assert_eq!(dump(&conn), before);
    assert_eq!(
        conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        11
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='requirement_initial'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
}
#[test]
fn concurrent_migrations_both_read_v11_then_second_skips_ddl_under_lock() {
    let p = path();
    let conn = Connection::open(&p).unwrap();
    v11(&conn);
    drop(conn);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let p = p.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                let conn = Connection::open(&p).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
                BEFORE_MIGRATION_LOCK.with(|h| {
                    *h.borrow_mut() = Some(Box::new(move || {
                        barrier.wait();
                    }))
                });
                run(&conn, None).unwrap(); // duplicate CREATE TABLE would fail without lock-time version reread
                let locked = LOCKED_MIGRATION_VERSIONS.with(|v| v.borrow().clone());
                assert_eq!(
                    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                        .unwrap(),
                    12
                );
                locked
            })
        })
        .collect();
    let mut witnessed = Vec::new();
    for t in threads {
        witnessed.extend(t.join().unwrap());
    }
    witnessed.sort_unstable();
    assert_eq!(witnessed, vec![(12, 11), (12, 12)]);
    let conn = Connection::open(p).unwrap();
    assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='requirement_conversations'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
}
use rusqlite::OptionalExtension;
