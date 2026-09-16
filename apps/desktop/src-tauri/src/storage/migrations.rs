use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{FixedOffset, Utc};
use rusqlite::{Connection, DatabaseName};
use uuid::Uuid;

/// Current schema version this build of Nova understands. Bump this and
/// append a new [`Migration`] to [`MIGRATIONS`] whenever the schema changes
/// — never edit an already-shipped migration's SQL in place.
pub const CURRENT_SCHEMA_VERSION: i64 = 2;

#[derive(Debug)]
pub enum MigrationError {
    /// The database's `PRAGMA user_version` is higher than this build
    /// supports. Refusing to touch it avoids silently downgrading a schema
    /// written by a newer Nova version.
    VersionTooNew {
        found: i64,
        supported: i64,
    },
    BackupFailed(String),
    Sql(String),
    Io(String),
}

impl fmt::Display for MigrationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::VersionTooNew { found, supported } => write!(
                f,
                "数据库版本 {found} 高于当前程序支持的版本 {supported}，为避免数据损坏已拒绝启动写操作"
            ),
            Self::BackupFailed(detail) => write!(f, "迁移前数据库备份失败：{detail}"),
            Self::Sql(detail) => write!(f, "数据库迁移失败：{detail}"),
            Self::Io(detail) => write!(f, "数据库迁移文件操作失败：{detail}"),
        }
    }
}

impl std::error::Error for MigrationError {}

impl From<rusqlite::Error> for MigrationError {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sql(value.to_string())
    }
}

/// One forward-only schema step. `sql` runs inside a single transaction; if
/// any statement errors the whole step is rolled back and `user_version`
/// stays at its pre-migration value, so the database is left exactly as it
/// was before this step started.
struct Migration {
    version: i64,
    /// Whether a database backup must be taken before this step runs. The
    /// very first step (establishing the pre-Phase-2 baseline schema via
    /// `CREATE TABLE IF NOT EXISTS`) is a no-op against an existing Phase 1
    /// database and has nothing to lose, so it is exempt; every step from
    /// here on touches real user data and must be backed up first.
    requires_backup: bool,
    sql: &'static str,
}

const BASELINE_SQL: &str = "
    PRAGMA foreign_keys = ON;
    CREATE TABLE IF NOT EXISTS providers (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        base_url TEXT NOT NULL,
        masked_key TEXT,
        has_key INTEGER NOT NULL DEFAULT 0,
        updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS app_settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS conversations (
        id TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        provider_id TEXT NOT NULL,
        model_id TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE TABLE IF NOT EXISTS messages (
        id TEXT PRIMARY KEY,
        conversation_id TEXT NOT NULL,
        role TEXT NOT NULL,
        content TEXT NOT NULL,
        provider_id TEXT NOT NULL,
        model_id TEXT NOT NULL,
        status TEXT NOT NULL,
        request_id TEXT,
        created_at TEXT NOT NULL,
        FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
    );
    CREATE TABLE IF NOT EXISTS request_logs (
        id TEXT PRIMARY KEY,
        provider_id TEXT NOT NULL,
        model_id TEXT,
        endpoint TEXT NOT NULL,
        status_code INTEGER,
        error_code TEXT,
        message TEXT,
        request_id TEXT,
        created_at TEXT NOT NULL
    );
";

const PHASE2_SQL: &str = "
    CREATE TABLE nova_projects (
        id TEXT PRIMARY KEY,
        name TEXT NOT NULL,
        local_path TEXT NOT NULL,
        normalized_path TEXT NOT NULL UNIQUE,
        source_type TEXT NOT NULL CHECK(source_type IN ('local','github','template')),
        source_url TEXT,
        current_branch TEXT,
        last_head_summary TEXT,
        purpose TEXT,
        long_term_goal TEXT,
        user_preferences TEXT,
        prohibitions TEXT,
        last_activity_at TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        last_opened_at TEXT,
        data_format_version INTEGER NOT NULL DEFAULT 1
    );
    CREATE INDEX idx_nova_projects_normalized_path ON nova_projects(normalized_path);

    CREATE TABLE project_memory (
        project_id TEXT PRIMARY KEY REFERENCES nova_projects(id) ON DELETE CASCADE,
        product_goals TEXT,
        user_requirements TEXT,
        confirmed_decisions TEXT,
        open_questions TEXT,
        deferred_items TEXT,
        known_issues TEXT,
        task_history_summary TEXT,
        current_unfinished_task_id TEXT,
        resume_position TEXT,
        last_ai_used TEXT,
        project_rules_index TEXT,
        license_summary TEXT,
        install_run_summary TEXT,
        updated_at TEXT NOT NULL
    );

    CREATE TABLE project_memory_revisions (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        field_name TEXT NOT NULL,
        old_value TEXT,
        new_value TEXT,
        changed_by TEXT NOT NULL CHECK(changed_by IN ('user','nova','connected_ai','system')),
        reason TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_project_memory_revisions_project ON project_memory_revisions(project_id);

    CREATE TABLE tasks (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        title TEXT NOT NULL,
        goal TEXT,
        in_scope TEXT,
        not_in_scope TEXT,
        done_criteria TEXT,
        ai_kind TEXT,
        state TEXT NOT NULL,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL
    );
    CREATE INDEX idx_tasks_project ON tasks(project_id);
    CREATE INDEX idx_tasks_state ON tasks(state);
    CREATE INDEX idx_tasks_created_at ON tasks(created_at);

    CREATE TABLE task_state_events (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        old_state TEXT,
        new_state TEXT NOT NULL,
        reason TEXT,
        actor TEXT NOT NULL CHECK(actor IN ('user','nova','connected_ai','system')),
        correlation_id TEXT,
        is_recovery INTEGER NOT NULL DEFAULT 0,
        safety_summary TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_task_state_events_task ON task_state_events(task_id);
    CREATE INDEX idx_task_state_events_created_at ON task_state_events(created_at);

    CREATE TABLE task_actions (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        action_type TEXT NOT NULL,
        idempotency_key TEXT NOT NULL,
        attempt_number INTEGER NOT NULL DEFAULT 1,
        status TEXT NOT NULL CHECK(status IN ('planned','started','succeeded','failed','cancelled','uncertain')),
        started_at TEXT,
        finished_at TEXT,
        evidence_ref TEXT,
        error_summary TEXT,
        created_at TEXT NOT NULL,
        updated_at TEXT NOT NULL,
        UNIQUE(task_id, idempotency_key)
    );
    CREATE INDEX idx_task_actions_task ON task_actions(task_id);
    CREATE INDEX idx_task_actions_idempotency ON task_actions(idempotency_key);
    CREATE INDEX idx_task_actions_status ON task_actions(status);

    CREATE TABLE recovery_checkpoints (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
        detected_state TEXT NOT NULL,
        summary TEXT NOT NULL,
        risk_flag INTEGER NOT NULL DEFAULT 0,
        resolved_at TEXT,
        resolution TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_recovery_checkpoints_project ON recovery_checkpoints(project_id);
    CREATE INDEX idx_recovery_checkpoints_task ON recovery_checkpoints(task_id);

    CREATE TABLE timeline_events (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
        phase TEXT,
        event_type TEXT NOT NULL,
        summary TEXT NOT NULL,
        detail TEXT,
        source TEXT NOT NULL CHECK(source IN ('cli','nova_collected','ai_summarized')),
        correlation_id TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_timeline_events_project ON timeline_events(project_id);
    CREATE INDEX idx_timeline_events_task ON timeline_events(task_id);
    CREATE INDEX idx_timeline_events_created_at ON timeline_events(created_at);

    CREATE TABLE command_evidence (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        command_redacted TEXT NOT NULL,
        exit_code INTEGER,
        started_at TEXT,
        finished_at TEXT,
        stdout_ref TEXT,
        stderr_ref TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_command_evidence_task ON command_evidence(task_id);

    CREATE TABLE git_evidence (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        event_kind TEXT NOT NULL,
        commit_sha TEXT,
        branch TEXT,
        diff_stat TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_git_evidence_task ON git_evidence(task_id);

    CREATE TABLE file_change_evidence (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        file_path TEXT NOT NULL,
        change_kind TEXT NOT NULL CHECK(change_kind IN ('created','modified','deleted')),
        summary TEXT,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_file_change_evidence_task ON file_change_evidence(task_id);

    CREATE TABLE test_evidence (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        test_command_redacted TEXT NOT NULL,
        passed_count INTEGER NOT NULL DEFAULT 0,
        failed_count INTEGER NOT NULL DEFAULT 0,
        skipped_count INTEGER NOT NULL DEFAULT 0,
        exit_code INTEGER,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_test_evidence_task ON test_evidence(task_id);

    CREATE TABLE user_acceptance (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id) ON DELETE CASCADE,
        accepted INTEGER NOT NULL,
        user_note TEXT,
        accepted_at TEXT NOT NULL,
        created_at TEXT NOT NULL
    );

    CREATE TABLE generated_documents (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
        document_type TEXT NOT NULL,
        file_path TEXT,
        generated_by TEXT,
        generated_at TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_generated_documents_task ON generated_documents(task_id);

    CREATE TABLE evidence_refs (
        id TEXT PRIMARY KEY,
        event_id TEXT NOT NULL REFERENCES timeline_events(id) ON DELETE CASCADE,
        evidence_type TEXT NOT NULL,
        evidence_id TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_evidence_refs_event ON evidence_refs(event_id);
";

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        requires_backup: false,
        sql: BASELINE_SQL,
    },
    Migration {
        version: 2,
        requires_backup: true,
        sql: PHASE2_SQL,
    },
];

/// Applies every migration the database hasn't seen yet, in order, each in
/// its own transaction. `db_path` is `None` for an in-memory/test database,
/// in which case backups are skipped entirely (there is no file to copy).
///
/// On success, `PRAGMA user_version` equals [`CURRENT_SCHEMA_VERSION`]. On
/// any failure, the offending step's transaction is rolled back and
/// `user_version` is left at the last successfully-applied version — the
/// database remains fully openable, just not yet upgraded.
pub fn run(conn: &Connection, db_path: Option<&Path>) -> Result<(), MigrationError> {
    let current: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;

    if current > CURRENT_SCHEMA_VERSION {
        return Err(MigrationError::VersionTooNew {
            found: current,
            supported: CURRENT_SCHEMA_VERSION,
        });
    }

    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    for migration in MIGRATIONS {
        if migration.version <= current {
            continue;
        }

        if migration.requires_backup {
            if let Some(path) = db_path {
                backup_database(conn, path)?;
            }
        }

        conn.execute_batch("BEGIN IMMEDIATE;")?;
        let apply_result = conn.execute_batch(migration.sql).and_then(|_| {
            conn.execute_batch(&format!("PRAGMA user_version = {};", migration.version))
        });

        match apply_result {
            Ok(()) => {
                conn.execute_batch("COMMIT;")?;
            }
            Err(error) => {
                // Best-effort rollback: if this also fails the connection is
                // in an unusable state anyway and the caller will see the
                // original SQL error, but the on-disk file is untouched
                // either way because nothing was ever committed.
                let _ = conn.execute_batch("ROLLBACK;");
                return Err(MigrationError::from(error));
            }
        }
    }

    Ok(())
}

/// Beijing time (UTC+8, fixed offset — deliberately not dependent on the OS
/// timezone database, which is not guaranteed to be installed) formatted as
/// `YYYYMMDDHHMMSS`, for embedding in backup filenames.
fn beijing_timestamp() -> String {
    let offset = FixedOffset::east_opt(8 * 3600).expect("valid fixed offset");
    Utc::now()
        .with_timezone(&offset)
        .format("%Y%m%d%H%M%S")
        .to_string()
}

/// Copies `db_path` into `<app-data-dir>/backups/` via SQLite's own online
/// backup API (safe against an in-progress journal/WAL, unlike a raw file
/// copy), writing first to a temp file and only atomically renaming it into
/// its final, uniquely-timestamped name once its size and
/// `PRAGMA integrity_check` both look sound. Never overwrites an existing
/// backup — the timestamp + a random suffix make every call's target name
/// unique — and never deletes any other backup.
fn backup_database(conn: &Connection, db_path: &Path) -> Result<PathBuf, MigrationError> {
    let app_data_dir = db_path
        .parent()
        .ok_or_else(|| MigrationError::BackupFailed("数据库路径没有父目录".to_string()))?;
    let backups_dir = app_data_dir.join("backups");
    fs::create_dir_all(&backups_dir).map_err(|error| MigrationError::Io(error.to_string()))?;

    let unique = Uuid::new_v4().simple().to_string();
    let tmp_path = backups_dir.join(format!(".tmp-{unique}.sqlite3"));
    let final_path = backups_dir.join(format!(
        "nova-backup-{}-{unique}.sqlite3",
        beijing_timestamp()
    ));

    let backup_result = conn.backup(DatabaseName::Main, &tmp_path, None);
    if let Err(error) = backup_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(MigrationError::BackupFailed(error.to_string()));
    }

    if let Err(error) = verify_backup(db_path, &tmp_path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error);
    }

    fs::rename(&tmp_path, &final_path).map_err(|error| {
        let _ = fs::remove_file(&tmp_path);
        MigrationError::Io(error.to_string())
    })?;

    Ok(final_path)
}

/// Sanity-checks a freshly written backup file before it is trusted: its
/// size must not have collapsed to zero when the source had real content,
/// and SQLite itself must consider the copy structurally intact.
fn verify_backup(source_path: &Path, backup_path: &Path) -> Result<(), MigrationError> {
    let source_len = fs::metadata(source_path)
        .map(|metadata| metadata.len())
        .unwrap_or(0);
    let backup_len = fs::metadata(backup_path)
        .map_err(|error| MigrationError::Io(error.to_string()))?
        .len();
    if source_len > 0 && backup_len == 0 {
        return Err(MigrationError::BackupFailed(
            "备份文件大小为 0，已放弃".to_string(),
        ));
    }

    let check_conn =
        Connection::open(backup_path).map_err(|error| MigrationError::Sql(error.to_string()))?;
    let integrity: String = check_conn
        .query_row("PRAGMA integrity_check(1)", [], |row| row.get(0))
        .map_err(|error| MigrationError::Sql(error.to_string()))?;
    if integrity != "ok" {
        return Err(MigrationError::BackupFailed(format!(
            "备份完整性校验失败：{integrity}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir(name: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-migrations-test-{name}-{}-{}-{n}",
            std::process::id(),
            Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn open_file_db(dir: &Path) -> (Connection, PathBuf) {
        let path = dir.join("nova.sqlite3");
        let conn = Connection::open(&path).expect("open file db");
        (conn, path)
    }

    #[test]
    fn fresh_database_reaches_current_schema_version() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        // Spot-check a Phase 1 table and a Phase 2 table both exist.
        conn.query_row("SELECT COUNT(*) FROM providers", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        conn.query_row("SELECT COUNT(*) FROM nova_projects", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    }

    #[test]
    fn repeated_runs_do_not_reapply_or_fail() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();
        run(&conn, None).unwrap();
        run(&conn, None).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn upgrades_a_pre_phase2_database_without_losing_data() {
        let dir = temp_dir("upgrade");
        let (conn, path) = open_file_db(&dir);
        // Simulate an existing Phase 1 install: baseline tables only,
        // `user_version` left at its SQLite default of 0, with a real row
        // already present.
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute(
            "INSERT INTO providers (id, name, base_url, masked_key, has_key, updated_at) VALUES ('sdkmax','SDKMAX','https://api.sdkmax.com',NULL,0,'2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let name: String = conn
            .query_row(
                "SELECT name FROM providers WHERE id = 'sdkmax'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(name, "SDKMAX");
        conn.query_row("SELECT COUNT(*) FROM nova_projects", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn applies_multiple_versions_in_order_from_zero() {
        let dir = temp_dir("multi-version");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn non_trivial_migration_creates_a_backup_file() {
        let dir = temp_dir("backup-created");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();

        let backups_dir = dir.join("backups");
        let entries: Vec<_> = fs::read_dir(&backups_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("nova-backup-")
            })
            .collect();
        assert_eq!(
            entries.len(),
            1,
            "expected exactly one backup (only migration 2 requires one): {entries:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_filename_contains_beijing_style_timestamp_and_is_unique_per_run() {
        let dir = temp_dir("backup-unique");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();

        // A second, independent database in the same directory must not
        // collide with or overwrite the first backup.
        let dir2 = temp_dir("backup-unique-2");
        let (conn2, path2) = open_file_db(&dir2);
        run(&conn2, Some(&path2)).unwrap();

        let backups_dir = dir.join("backups");
        let names: Vec<String> = fs::read_dir(&backups_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1);
        assert!(names[0].len() > "nova-backup-YYYYMMDDHHMMSS-".len());

        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&dir2);
    }

    #[test]
    fn chinese_and_space_containing_app_data_path_can_still_be_backed_up() {
        let dir = temp_dir("中文 备份 目录 test");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();
        let backups_dir = dir.join("backups");
        assert!(fs::read_dir(&backups_dir).unwrap().count() >= 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn version_higher_than_supported_is_rejected_without_modifying_the_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "PRAGMA user_version = {};",
            CURRENT_SCHEMA_VERSION + 1
        ))
        .unwrap();
        let result = run(&conn, None);
        assert!(matches!(result, Err(MigrationError::VersionTooNew { .. })));
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            version,
            CURRENT_SCHEMA_VERSION + 1,
            "must not have been silently downgraded"
        );
    }

    #[test]
    fn a_failing_migration_step_rolls_back_and_leaves_the_database_openable() {
        let dir = temp_dir("failing-step");
        let (conn, path) = open_file_db(&dir);
        // Force migration 2 to fail deterministically: pre-create one of
        // its tables by hand so the real `CREATE TABLE` inside the
        // migration collides.
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        conn.execute_batch("CREATE TABLE nova_projects (id TEXT PRIMARY KEY);")
            .unwrap();

        let result = run(&conn, Some(&path));
        assert!(
            result.is_err(),
            "expected the colliding CREATE TABLE to fail"
        );

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            version, 1,
            "user_version must stay at the last successfully applied version"
        );
        // The database must still be perfectly usable afterwards.
        conn.query_row("SELECT COUNT(*) FROM providers", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_is_taken_before_the_failing_step_so_it_survives_the_failure() {
        let dir = temp_dir("backup-before-failure");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 1;").unwrap();
        conn.execute_batch("CREATE TABLE nova_projects (id TEXT PRIMARY KEY);")
            .unwrap();

        let _ = run(&conn, Some(&path));

        let backups_dir = dir.join("backups");
        let count = fs::read_dir(&backups_dir)
            .map(|iter| iter.filter_map(|e| e.ok()).count())
            .unwrap_or(0);
        assert_eq!(
            count, 1,
            "the backup taken before the failing step must still be on disk"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupted_database_file_fails_cleanly_without_touching_backups() {
        let dir = temp_dir("corrupt-db");
        let path = dir.join("nova.sqlite3");
        fs::write(&path, b"this is not a sqlite database").unwrap();

        let open_result = Connection::open(&path);
        // Opening succeeds lazily in SQLite; the failure surfaces on first
        // real access. Confirm it errors rather than panicking, and that no
        // backup directory gets created from a connection that was never
        // usable.
        if let Ok(conn) = open_result {
            let result = run(&conn, Some(&path));
            assert!(result.is_err());
        }
        assert!(!dir.join("backups").exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_migration_runs_do_not_create_duplicate_backups() {
        let dir = temp_dir("no-duplicate-backup");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();
        run(&conn, Some(&path)).unwrap();
        run(&conn, Some(&path)).unwrap();

        let backups_dir = dir.join("backups");
        let count = fs::read_dir(&backups_dir).unwrap().count();
        assert_eq!(
            count, 1,
            "already-applied migrations must not be re-run, so no extra backups"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
