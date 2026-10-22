use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{FixedOffset, Utc};
use rusqlite::{Connection, DatabaseName};
use uuid::Uuid;

/// Current schema version this build of Nova understands. Bump this and
/// append a new [`Migration`] to [`MIGRATIONS`] whenever the schema changes
/// — never edit an already-shipped migration's SQL in place.
pub const CURRENT_SCHEMA_VERSION: i64 = 12;

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

/// Phase 2 remediation (Codex `CODEX_PHASE2_TASK_STATE_RECOVERY_REVIEW`):
///
/// 1. **Blocker 1 fix** — `tasks.result_generation` is a monotonically
///    incrementing counter bumped every time a task produces a fresh
///    result (enters `preview_ready`). `user_acceptance` is redesigned
///    from "one row per task" to "one row per (task, result_generation)":
///    old rows are preserved for audit but migrated in as
///    `result_generation = 0`, a value no real task ever reaches via
///    [`crate::storage::tasks::bump_result_generation`] (which starts
///    counting at 1) — so pre-migration acceptance rows can never satisfy
///    the `completed` gate, which requires an `accepted = 1` row whose
///    `result_generation` matches the task's *current* one.
/// 2. **Suggestion / Minor 3 fix** — every Phase 2 table that carries a
///    `project_id` but lacked a foreign key back to `nova_projects` is
///    recreated with that FK added (`task_state_events`,
///    `recovery_checkpoints`, `timeline_events`, `command_evidence`,
///    `git_evidence`, `file_change_evidence`, `test_evidence`,
///    `generated_documents`, and the redesigned `user_acceptance`).
///    SQLite cannot `ALTER TABLE ... ADD FOREIGN KEY`, so each is rebuilt
///    via `CREATE TABLE ... AS copy -> INSERT SELECT -> DROP -> RENAME`,
///    preserving any existing rows exactly.
const V3_SQL: &str = "
    ALTER TABLE tasks ADD COLUMN result_generation INTEGER NOT NULL DEFAULT 0;

    CREATE TABLE task_state_events_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
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
    INSERT INTO task_state_events_v3 (id, project_id, task_id, old_state, new_state, reason, actor, correlation_id, is_recovery, safety_summary, created_at)
      SELECT id, project_id, task_id, old_state, new_state, reason, actor, correlation_id, is_recovery, safety_summary, created_at FROM task_state_events;
    DROP TABLE task_state_events;
    ALTER TABLE task_state_events_v3 RENAME TO task_state_events;
    CREATE INDEX idx_task_state_events_task ON task_state_events(task_id);
    CREATE INDEX idx_task_state_events_created_at ON task_state_events(created_at);

    CREATE TABLE recovery_checkpoints_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
        detected_state TEXT NOT NULL,
        summary TEXT NOT NULL,
        risk_flag INTEGER NOT NULL DEFAULT 0,
        resolved_at TEXT,
        resolution TEXT,
        created_at TEXT NOT NULL
    );
    INSERT INTO recovery_checkpoints_v3 (id, project_id, task_id, detected_state, summary, risk_flag, resolved_at, resolution, created_at)
      SELECT id, project_id, task_id, detected_state, summary, risk_flag, resolved_at, resolution, created_at FROM recovery_checkpoints;
    DROP TABLE recovery_checkpoints;
    ALTER TABLE recovery_checkpoints_v3 RENAME TO recovery_checkpoints;
    CREATE INDEX idx_recovery_checkpoints_project ON recovery_checkpoints(project_id);
    CREATE INDEX idx_recovery_checkpoints_task ON recovery_checkpoints(task_id);

    CREATE TABLE timeline_events_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
        phase TEXT,
        event_type TEXT NOT NULL,
        summary TEXT NOT NULL,
        detail TEXT,
        source TEXT NOT NULL CHECK(source IN ('cli','nova_collected','ai_summarized')),
        correlation_id TEXT,
        created_at TEXT NOT NULL
    );
    INSERT INTO timeline_events_v3 (id, project_id, task_id, phase, event_type, summary, detail, source, correlation_id, created_at)
      SELECT id, project_id, task_id, phase, event_type, summary, detail, source, correlation_id, created_at FROM timeline_events;
    DROP TABLE timeline_events;
    ALTER TABLE timeline_events_v3 RENAME TO timeline_events;
    CREATE INDEX idx_timeline_events_project ON timeline_events(project_id);
    CREATE INDEX idx_timeline_events_task ON timeline_events(task_id);
    CREATE INDEX idx_timeline_events_created_at ON timeline_events(created_at);

    CREATE TABLE command_evidence_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        command_redacted TEXT NOT NULL,
        exit_code INTEGER,
        started_at TEXT,
        finished_at TEXT,
        stdout_ref TEXT,
        stderr_ref TEXT,
        created_at TEXT NOT NULL
    );
    INSERT INTO command_evidence_v3 (id, project_id, task_id, command_redacted, exit_code, started_at, finished_at, stdout_ref, stderr_ref, created_at)
      SELECT id, project_id, task_id, command_redacted, exit_code, started_at, finished_at, stdout_ref, stderr_ref, created_at FROM command_evidence;
    DROP TABLE command_evidence;
    ALTER TABLE command_evidence_v3 RENAME TO command_evidence;
    CREATE INDEX idx_command_evidence_task ON command_evidence(task_id);

    CREATE TABLE git_evidence_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        event_kind TEXT NOT NULL,
        commit_sha TEXT,
        branch TEXT,
        diff_stat TEXT,
        created_at TEXT NOT NULL
    );
    INSERT INTO git_evidence_v3 (id, project_id, task_id, event_kind, commit_sha, branch, diff_stat, created_at)
      SELECT id, project_id, task_id, event_kind, commit_sha, branch, diff_stat, created_at FROM git_evidence;
    DROP TABLE git_evidence;
    ALTER TABLE git_evidence_v3 RENAME TO git_evidence;
    CREATE INDEX idx_git_evidence_task ON git_evidence(task_id);

    CREATE TABLE file_change_evidence_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        file_path TEXT NOT NULL,
        change_kind TEXT NOT NULL CHECK(change_kind IN ('created','modified','deleted')),
        summary TEXT,
        created_at TEXT NOT NULL
    );
    INSERT INTO file_change_evidence_v3 (id, project_id, task_id, file_path, change_kind, summary, created_at)
      SELECT id, project_id, task_id, file_path, change_kind, summary, created_at FROM file_change_evidence;
    DROP TABLE file_change_evidence;
    ALTER TABLE file_change_evidence_v3 RENAME TO file_change_evidence;
    CREATE INDEX idx_file_change_evidence_task ON file_change_evidence(task_id);

    CREATE TABLE test_evidence_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        test_command_redacted TEXT NOT NULL,
        passed_count INTEGER NOT NULL DEFAULT 0,
        failed_count INTEGER NOT NULL DEFAULT 0,
        skipped_count INTEGER NOT NULL DEFAULT 0,
        exit_code INTEGER,
        created_at TEXT NOT NULL
    );
    INSERT INTO test_evidence_v3 (id, project_id, task_id, test_command_redacted, passed_count, failed_count, skipped_count, exit_code, created_at)
      SELECT id, project_id, task_id, test_command_redacted, passed_count, failed_count, skipped_count, exit_code, created_at FROM test_evidence;
    DROP TABLE test_evidence;
    ALTER TABLE test_evidence_v3 RENAME TO test_evidence;
    CREATE INDEX idx_test_evidence_task ON test_evidence(task_id);

    CREATE TABLE generated_documents_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT REFERENCES tasks(id) ON DELETE CASCADE,
        document_type TEXT NOT NULL,
        file_path TEXT,
        generated_by TEXT,
        generated_at TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    INSERT INTO generated_documents_v3 (id, project_id, task_id, document_type, file_path, generated_by, generated_at, created_at)
      SELECT id, project_id, task_id, document_type, file_path, generated_by, generated_at, created_at FROM generated_documents;
    DROP TABLE generated_documents;
    ALTER TABLE generated_documents_v3 RENAME TO generated_documents;
    CREATE INDEX idx_generated_documents_task ON generated_documents(task_id);

    CREATE TABLE user_acceptance_v3 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        result_generation INTEGER NOT NULL,
        accepted INTEGER NOT NULL,
        user_note TEXT,
        accepted_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE(task_id, result_generation)
    );
    INSERT INTO user_acceptance_v3 (id, project_id, task_id, result_generation, accepted, user_note, accepted_at, created_at)
      SELECT id, project_id, task_id, 0, accepted, user_note, accepted_at, created_at FROM user_acceptance;
    DROP TABLE user_acceptance;
    ALTER TABLE user_acceptance_v3 RENAME TO user_acceptance;
    CREATE INDEX idx_user_acceptance_task ON user_acceptance(task_id);
";

/// Phase 3 (Open Local Project Dual-AI Core Loop): one row per review round
/// (first/second) for a task, recording the independent reviewer's
/// structured verdict. `integrity_violation` is a fourth verdict beyond the
/// product spec's three (`approve`/`changes_required`/
/// `insufficient_evidence`) — it is what `storage::core_loop` records when
/// its technical read-only enforcement detects the reviewer actually
/// changed the working tree, which must never be silently treated as a
/// normal review outcome.
const V4_SQL: &str = "
    CREATE TABLE task_reviews (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        reviewer_ai_kind TEXT NOT NULL,
        verdict TEXT NOT NULL CHECK(verdict IN ('approve', 'changes_required', 'insufficient_evidence', 'integrity_violation')),
        findings_summary TEXT,
        created_at TEXT NOT NULL,
        UNIQUE(task_id, result_generation, round)
    );
    CREATE INDEX idx_task_reviews_task ON task_reviews(task_id);
    CREATE INDEX idx_task_reviews_project ON task_reviews(project_id);
";

/// Phase 3 remediation (Major 3): `task_reviews` project/task ownership
/// integrity. V4 left `project_id`/`task_id` as two independent, only
/// loosely-related columns — a row could claim any `project_id` regardless
/// of which project the referenced task actually belongs to, letting the
/// frontend read another project's review findings just by guessing/reusing
/// a `task_id`. This step adds a composite foreign key
/// `(task_id, project_id) REFERENCES tasks(id, project_id)`, which SQLite
/// can only enforce against a UNIQUE index on that exact column pair — so
/// `tasks(id, project_id)` gets one first (harmless: `id` is already the
/// primary key, so `(id, project_id)` is trivially unique too). `round` also
/// gains `CHECK(round IN (1, 2))`, matching Phase 3's fixed "at most one
/// automatic fix pass" product rule (never a third round). The migration
/// copies forward only rows that are actually internally consistent
/// (`INNER JOIN tasks ... AND t.project_id = tr.project_id`, `round IN
/// (1,2)`); any row that fails either check is a `V4_SQL` data-integrity bug
/// that must not have been possible during normal Phase 3 operation, so
/// silently dropping it during the upgrade (rather than aborting the whole
/// migration) is the safe choice — never a legal Phase 3-produced row.
const V5_SQL: &str = "
    CREATE UNIQUE INDEX idx_tasks_id_project_id ON tasks(id, project_id);
    CREATE TABLE task_reviews_v5 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL CHECK(round IN (1, 2)),
        reviewer_ai_kind TEXT NOT NULL,
        verdict TEXT NOT NULL CHECK(verdict IN ('approve', 'changes_required', 'insufficient_evidence', 'integrity_violation')),
        findings_summary TEXT,
        created_at TEXT NOT NULL,
        UNIQUE(task_id, result_generation, round),
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    INSERT INTO task_reviews_v5 (id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at)
      SELECT tr.id, tr.project_id, tr.task_id, tr.result_generation, tr.round, tr.reviewer_ai_kind, tr.verdict, tr.findings_summary, tr.created_at
      FROM task_reviews tr
      INNER JOIN tasks t ON t.id = tr.task_id AND t.project_id = tr.project_id
      WHERE tr.round IN (1, 2);
    DROP TABLE task_reviews;
    ALTER TABLE task_reviews_v5 RENAME TO task_reviews;
    CREATE INDEX idx_task_reviews_task ON task_reviews(task_id);
    CREATE INDEX idx_task_reviews_project ON task_reviews(project_id);
";

/// Phase 3 remediation (Blocker 3): independent target-project test
/// execution. `tasks` gains the user-confirmed-or-Nova-auto-detected test
/// command (`test_command_redacted`/`test_command_source`); `test_evidence`
/// is rebuilt with a `(result_generation, round)`-scoped composite FK to
/// `tasks` (same pattern as v5's `task_reviews` fix) plus the richer status
/// vocabulary Blocker 3 requires — `no_tests_applicable`/`timed_out`/
/// `cancelled`/`insufficient_evidence` alongside the original `passed`/
/// `failed` — so \"no test command\" and \"tests genuinely passed\" can
/// never be confused. Pre-existing v5 `test_evidence` rows (which have no
/// generation/round concept at all) migrate forward as `(generation 0,
/// round 1)` with a `status` inferred from their `failed_count`, exactly
/// mirroring how v3 relabeled pre-Phase-3 acceptance rows.
const V6_SQL: &str = "
    ALTER TABLE tasks ADD COLUMN test_command_redacted TEXT;
    ALTER TABLE tasks ADD COLUMN test_command_source TEXT CHECK(test_command_source IS NULL OR test_command_source IN ('auto', 'user'));

    CREATE TABLE test_evidence_v6 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        test_command_redacted TEXT,
        status TEXT NOT NULL CHECK(status IN ('passed', 'failed', 'no_tests_applicable', 'timed_out', 'cancelled', 'insufficient_evidence')),
        passed_count INTEGER NOT NULL DEFAULT 0,
        failed_count INTEGER NOT NULL DEFAULT 0,
        skipped_count INTEGER NOT NULL DEFAULT 0,
        exit_code INTEGER,
        reason TEXT,
        stdout_ref TEXT,
        stderr_ref TEXT,
        created_at TEXT NOT NULL,
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    INSERT INTO test_evidence_v6 (id, project_id, task_id, result_generation, round, test_command_redacted, status, passed_count, failed_count, skipped_count, exit_code, created_at)
      SELECT te.id, te.project_id, te.task_id, 0, 1, te.test_command_redacted,
             CASE WHEN te.failed_count > 0 THEN 'failed' ELSE 'passed' END,
             te.passed_count, te.failed_count, te.skipped_count, te.exit_code, te.created_at
      FROM test_evidence te
      INNER JOIN tasks t ON t.id = te.task_id AND t.project_id = te.project_id;
    DROP TABLE test_evidence;
    ALTER TABLE test_evidence_v6 RENAME TO test_evidence;
    CREATE INDEX idx_test_evidence_task ON test_evidence(task_id);
    CREATE INDEX idx_test_evidence_task_gen_round ON test_evidence(task_id, result_generation, round);
";

/// Second re-review (Major 3, continued): copies every `task_reviews` row
/// v5's own `INNER JOIN` is about to exclude (and therefore lose forever
/// once v5's `DROP TABLE` runs) into a permanent quarantine table first,
/// with the specific reason it failed v5's consistency check. Must run
/// inside the *same transaction* as v5's migration (see `run()`) so a
/// failure anywhere in v5 rolls this back too — never a half-quarantined
/// state. Idempotent (`CREATE TABLE IF NOT EXISTS`) so a retried migration
/// attempt after an unrelated earlier failure does not error on the second
/// try.
fn quarantine_rows_v5_will_drop(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS task_reviews_migration_quarantine_v5 (
            id TEXT PRIMARY KEY,
            original_project_id TEXT,
            original_task_id TEXT,
            result_generation INTEGER,
            round INTEGER,
            reviewer_ai_kind TEXT,
            verdict TEXT,
            findings_summary TEXT,
            original_created_at TEXT,
            quarantine_reason TEXT NOT NULL,
            quarantined_at TEXT NOT NULL
        );",
    )?;
    let now = Utc::now().to_rfc3339();
    conn.execute(
        "INSERT INTO task_reviews_migration_quarantine_v5
            (id, original_project_id, original_task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, original_created_at, quarantine_reason, quarantined_at)
         SELECT tr.id, tr.project_id, tr.task_id, tr.result_generation, tr.round, tr.reviewer_ai_kind, tr.verdict, tr.findings_summary, tr.created_at,
                CASE
                    WHEN t.id IS NULL THEN 'no task found owning (task_id, project_id)'
                    WHEN tr.round NOT IN (1, 2) THEN 'round not in (1, 2)'
                    ELSE 'failed v5 consistency check for an unrecorded reason'
                END,
                ?1
         FROM task_reviews tr
         LEFT JOIN tasks t ON t.id = tr.task_id AND t.project_id = tr.project_id
         WHERE t.id IS NULL OR tr.round NOT IN (1, 2)",
        rusqlite::params![now],
    )?;
    Ok(())
}

/// Second re-review remediation:
/// - Blocker 3: `no_tests_confirmations` — a task with no detectable test
///   command may no longer reach `no_tests_applicable` on Nova's own say-so;
///   a real, persisted, `Actor::User`-only confirmation row must exist for
///   the *exact* current `(task_id, result_generation, round)` first.
/// - Blocker 2 / executable trust: `trusted_cli_binaries` — a `binary_path`
///   (default-discovered or user-configured) is only ever actually launched
///   after its live file's fingerprint matches a row here that the user
///   explicitly confirmed; `UNIQUE(kind)` means re-confirming a path for a
///   kind replaces (via delete-then-insert in the storage layer, not SQL
///   here) whatever was trusted before, rather than accumulating history.
/// - Major 5: `tasks.document_sync_pending` — set when the seven documents
///   fail to regenerate after an acceptance/rejection/completion write that
///   itself already succeeded, so the *database* state is never rolled back
///   just because a docs-directory write failed, and a later retry knows
///   there is unfinished work.
const V7_SQL: &str = "
    CREATE TABLE no_tests_confirmations (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        confirmed_by TEXT NOT NULL CHECK(confirmed_by = 'user'),
        reason TEXT,
        confirmed_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE(task_id, result_generation, round),
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    CREATE INDEX idx_no_tests_confirmations_task ON no_tests_confirmations(task_id);

    CREATE TABLE trusted_cli_binaries (
        id TEXT PRIMARY KEY,
        kind TEXT NOT NULL CHECK(kind IN ('codex_cli', 'claude_cli')),
        canonical_path TEXT NOT NULL,
        file_size INTEGER NOT NULL,
        modified_unix_ms INTEGER NOT NULL,
        sha256_hex TEXT NOT NULL,
        confirmed_by TEXT NOT NULL CHECK(confirmed_by = 'user'),
        confirmed_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE(kind)
    );

    ALTER TABLE tasks ADD COLUMN document_sync_pending INTEGER NOT NULL DEFAULT 0;
";

/// Codex re-review remediation (Major 3): Nova's own fact-verification must
/// distinguish test evidence from build evidence — a project whose tests
/// pass but whose build fails must never be reported as fully verified.
/// `build_evidence` mirrors `test_evidence`'s exact shape (same
/// `(task_id, result_generation, round)` scoping, same composite FK to
/// `tasks`, same redaction discipline for text fields) so a round's test and
/// build results are always readable together against the same task/
/// generation/round/Git-version tuple. Purely additive — no existing table
/// is touched, so this is safe on top of every prior schema version and old
/// databases with no `build_evidence` rows at all remain fully readable
/// (an empty result set, never an error).
const V8_SQL: &str = "
    CREATE TABLE build_evidence (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        build_command_redacted TEXT,
        status TEXT NOT NULL CHECK(status IN ('passed', 'failed', 'no_build_applicable', 'timed_out', 'cancelled', 'insufficient_evidence')),
        exit_code INTEGER,
        reason TEXT,
        stdout_ref TEXT,
        stderr_ref TEXT,
        created_at TEXT NOT NULL,
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    CREATE INDEX idx_build_evidence_task ON build_evidence(task_id);
    CREATE INDEX idx_build_evidence_task_gen_round ON build_evidence(task_id, result_generation, round);
";

/// Codex re-review remediation (Major 2): the build-gate equivalent of
/// `no_tests_confirmations` (v7) — a persisted, explicit user confirmation
/// that build detection could not be reliably resolved (unreadable/
/// malformed `package.json`, or an unrecognized project type) and the user
/// has confirmed no build step actually applies. Same shape, same
/// `(task_id, result_generation, round)` uniqueness, same composite FK to
/// `tasks` as `no_tests_confirmations` — a deliberate structural mirror of
/// an already-accepted pattern, kept as its own table (not folded into
/// `no_tests_confirmations`) since it records a distinct fact. Purely
/// additive.
const V9_SQL: &str = "
    CREATE TABLE no_build_confirmations (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        confirmed_by TEXT NOT NULL CHECK(confirmed_by = 'user'),
        reason TEXT,
        confirmed_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE(task_id, result_generation, round),
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    CREATE INDEX idx_no_build_confirmations_task ON no_build_confirmations(task_id);
";

/// Phase 4 remediation (second re-review, remaining Major): persists the
/// authorization basis for a `no_build_applicable` row at the exact moment
/// it is written, instead of the prior design where the completion gate
/// re-ran `detect_build_command` against the project's *current* files at
/// read time — a mechanically-legitimate row could be invalidated by a
/// later build-script addition, and worse, an orphaned/unconfirmed row
/// could become retroactively "authorized" purely because the project's
/// files happened to look `NotApplicable` by the time someone read it.
///
/// Table-recreate pattern (mirrors every prior schema-evolution step in
/// this file, e.g. v6's `_v3`-suffix rename): purely additive in effect —
/// no column is removed, no existing table other than `build_evidence`
/// itself is touched. Every pre-existing `no_build_applicable` row is
/// migrated to `legacy_unknown` — deliberately never inferred as
/// `mechanical_not_applicable` from whatever the upgrading machine's
/// project directory happens to contain right now, and never silently left
/// `NULL` either (the `CHECK` below requires an explicit value whenever
/// `status = 'no_build_applicable'`). Every other status's row gets `NULL`,
/// enforced by the same `CHECK`. Old databases with zero `build_evidence`
/// rows (or none with this status) upgrade to an empty/unaffected table,
/// same as any other additive migration.
const V10_SQL: &str = "
    CREATE TABLE build_evidence_v10 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        build_command_redacted TEXT,
        status TEXT NOT NULL CHECK(status IN ('passed', 'failed', 'no_build_applicable', 'timed_out', 'cancelled', 'insufficient_evidence')),
        -- The `authorization_source IS NOT NULL AND` clause below is
        -- required, not redundant: SQLite's `IN` operator evaluates to
        -- NULL (neither true nor false) when its left side is NULL, and a
        -- CHECK constraint only rejects a row when its expression
        -- evaluates to *false* — a NULL result is treated as passing. Without
        -- the explicit NULL check, `no_build_applicable` rows with a
        -- missing `authorization_source` would silently be accepted.
        authorization_source TEXT CHECK(
            (status != 'no_build_applicable' AND authorization_source IS NULL)
            OR (status = 'no_build_applicable' AND authorization_source IS NOT NULL AND authorization_source IN ('user_confirmed_not_applicable', 'mechanical_not_applicable', 'legacy_unknown'))
        ),
        exit_code INTEGER,
        reason TEXT,
        stdout_ref TEXT,
        stderr_ref TEXT,
        created_at TEXT NOT NULL,
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    INSERT INTO build_evidence_v10 (id, project_id, task_id, result_generation, round, build_command_redacted, status, authorization_source, exit_code, reason, stdout_ref, stderr_ref, created_at)
      SELECT id, project_id, task_id, result_generation, round, build_command_redacted, status,
             CASE WHEN status = 'no_build_applicable' THEN 'legacy_unknown' ELSE NULL END,
             exit_code, reason, stdout_ref, stderr_ref, created_at
      FROM build_evidence;
    DROP TABLE build_evidence;
    ALTER TABLE build_evidence_v10 RENAME TO build_evidence;
    CREATE INDEX idx_build_evidence_task ON build_evidence(task_id);
    CREATE INDEX idx_build_evidence_task_gen_round ON build_evidence(task_id, result_generation, round);
";

/// WP2A: unified execution-attribution (`result_attempt`) across evidence
/// and confirmation tables, structured self-check contract, the needs-
/// confirmation snapshot of a task's requirement fields, and the one-time
/// legacy-task classification column that the migration itself populates
/// (see `classify_legacy_tasks_v11` below, run inside this same migration
/// step's transaction).
///
/// `result_attempt` is nullable and backfilled `NULL` on every pre-existing
/// row in `test_evidence`/`build_evidence`/`no_tests_confirmations`/
/// `no_build_confirmations` — deliberately never inferred from whatever
/// `task_actions.attempt_number` happens to be *now* (that would be
/// retroactively assigning an attribution the row was never actually
/// written with). `NULL` can never satisfy the completed gate's exact
/// `result_attempt = ?` match, so historical rows simply never again
/// participate in any future completion decision; they remain fully
/// readable for audit.
///
/// `no_tests_confirmations`/`no_build_confirmations`' unique key grows from
/// `(task_id, result_generation, round)` to additionally include
/// `result_attempt`. This is purely additive in effect: SQL's standard NULL
/// semantics (no two NULLs are ever considered equal for a UNIQUE
/// constraint) mean every pre-existing backfilled-NULL row coexists without
/// conflict with any number of distinct new per-attempt confirmations for
/// the same `(task_id, result_generation, round)` — old rows are never
/// deleted or renumbered, and a new attempt's confirmation is never blocked
/// by an older attempt's row occupying the same three-column prefix.
const V11_SQL: &str = "
    ALTER TABLE tasks ADD COLUMN legacy_classification TEXT
      CHECK(legacy_classification IS NULL
            OR legacy_classification IN ('dual_ai_evidenced', 'single_ai_evidenced', 'unknown_insufficient'));

    CREATE TABLE confirmed_requirement_items (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        field TEXT NOT NULL CHECK(field IN ('goal', 'in_scope', 'not_in_scope', 'done_criteria')),
        item_index INTEGER NOT NULL CHECK(item_index >= 0),
        item_text_redacted TEXT NOT NULL CHECK(length(item_text_redacted) > 0),
        created_at TEXT NOT NULL,
        UNIQUE(task_id, field, item_index)
    );
    CREATE INDEX idx_confirmed_requirement_items_task ON confirmed_requirement_items(task_id);

    CREATE TABLE self_checks (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL REFERENCES nova_projects(id) ON DELETE CASCADE,
        task_id TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
        round INTEGER NOT NULL,
        result_attempt INTEGER NOT NULL,
        parse_status TEXT NOT NULL CHECK(parse_status IN ('valid', 'invalid_format')),
        self_reported_status TEXT CHECK(
            self_reported_status IS NULL
            OR self_reported_status IN ('self_reported_complete', 'self_reported_incomplete', 'self_reported_blocked')
        ),
        requirement_coverage_json TEXT,
        check_results_json TEXT,
        unfinished_items_json TEXT,
        blocking_issues_json TEXT,
        raw_block_redacted TEXT NOT NULL,
        created_at TEXT NOT NULL
    );
    CREATE INDEX idx_self_checks_task_round_attempt ON self_checks(task_id, round, result_attempt);

    CREATE TABLE test_evidence_v11 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        result_attempt INTEGER,
        test_command_redacted TEXT,
        status TEXT NOT NULL CHECK(status IN ('passed', 'failed', 'no_tests_applicable', 'timed_out', 'cancelled', 'insufficient_evidence')),
        passed_count INTEGER NOT NULL DEFAULT 0,
        failed_count INTEGER NOT NULL DEFAULT 0,
        skipped_count INTEGER NOT NULL DEFAULT 0,
        exit_code INTEGER,
        reason TEXT,
        stdout_ref TEXT,
        stderr_ref TEXT,
        created_at TEXT NOT NULL,
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    INSERT INTO test_evidence_v11 (id, project_id, task_id, result_generation, round, result_attempt, test_command_redacted, status, passed_count, failed_count, skipped_count, exit_code, reason, stdout_ref, stderr_ref, created_at)
      SELECT id, project_id, task_id, result_generation, round, NULL, test_command_redacted, status, passed_count, failed_count, skipped_count, exit_code, reason, stdout_ref, stderr_ref, created_at
      FROM test_evidence;
    DROP TABLE test_evidence;
    ALTER TABLE test_evidence_v11 RENAME TO test_evidence;
    CREATE INDEX idx_test_evidence_task ON test_evidence(task_id);
    CREATE INDEX idx_test_evidence_task_gen_round ON test_evidence(task_id, result_generation, round);

    CREATE TABLE build_evidence_v11 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        result_attempt INTEGER,
        build_command_redacted TEXT,
        status TEXT NOT NULL CHECK(status IN ('passed', 'failed', 'no_build_applicable', 'timed_out', 'cancelled', 'insufficient_evidence')),
        authorization_source TEXT CHECK(
            (status != 'no_build_applicable' AND authorization_source IS NULL)
            OR (status = 'no_build_applicable' AND authorization_source IS NOT NULL AND authorization_source IN ('user_confirmed_not_applicable', 'mechanical_not_applicable', 'legacy_unknown'))
        ),
        exit_code INTEGER,
        reason TEXT,
        stdout_ref TEXT,
        stderr_ref TEXT,
        created_at TEXT NOT NULL,
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    INSERT INTO build_evidence_v11 (id, project_id, task_id, result_generation, round, result_attempt, build_command_redacted, status, authorization_source, exit_code, reason, stdout_ref, stderr_ref, created_at)
      SELECT id, project_id, task_id, result_generation, round, NULL, build_command_redacted, status, authorization_source, exit_code, reason, stdout_ref, stderr_ref, created_at
      FROM build_evidence;
    DROP TABLE build_evidence;
    ALTER TABLE build_evidence_v11 RENAME TO build_evidence;
    CREATE INDEX idx_build_evidence_task ON build_evidence(task_id);
    CREATE INDEX idx_build_evidence_task_gen_round ON build_evidence(task_id, result_generation, round);

    CREATE TABLE no_tests_confirmations_v11 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        result_attempt INTEGER,
        confirmed_by TEXT NOT NULL CHECK(confirmed_by = 'user'),
        reason TEXT,
        confirmed_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE(task_id, result_generation, round, result_attempt),
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    INSERT INTO no_tests_confirmations_v11 (id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at)
      SELECT id, project_id, task_id, result_generation, round, NULL, confirmed_by, reason, confirmed_at, created_at
      FROM no_tests_confirmations;
    DROP TABLE no_tests_confirmations;
    ALTER TABLE no_tests_confirmations_v11 RENAME TO no_tests_confirmations;
    CREATE INDEX idx_no_tests_confirmations_task ON no_tests_confirmations(task_id);

    CREATE TABLE no_build_confirmations_v11 (
        id TEXT PRIMARY KEY,
        project_id TEXT NOT NULL,
        task_id TEXT NOT NULL,
        result_generation INTEGER NOT NULL,
        round INTEGER NOT NULL,
        result_attempt INTEGER,
        confirmed_by TEXT NOT NULL CHECK(confirmed_by = 'user'),
        reason TEXT,
        confirmed_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        UNIQUE(task_id, result_generation, round, result_attempt),
        FOREIGN KEY (task_id, project_id) REFERENCES tasks(id, project_id) ON DELETE CASCADE
    );
    INSERT INTO no_build_confirmations_v11 (id, project_id, task_id, result_generation, round, result_attempt, confirmed_by, reason, confirmed_at, created_at)
      SELECT id, project_id, task_id, result_generation, round, NULL, confirmed_by, reason, confirmed_at, created_at
      FROM no_build_confirmations;
    DROP TABLE no_build_confirmations;
    ALTER TABLE no_build_confirmations_v11 RENAME TO no_build_confirmations;
    CREATE INDEX idx_no_build_confirmations_task ON no_build_confirmations(task_id);

    -- One-time legacy classification: only for tasks that are non-terminal
    -- *right now, at migration time*. Tasks created after this migration
    -- runs are never touched by these two statements (they start with
    -- legacy_classification already NULL and stay that way), so a brand
    -- new single-AI task is never mistakenly frozen just for lacking
    -- review signals it could never have had reason to produce.
    --
    -- dual_ai_evidenced: either strong, code-written (never directly user-
    -- editable) signal is present — an `ai_review`-typed action ever ran,
    -- or a `task_reviews` row exists. Either alone is sufficient (OR): both
    -- are independent witnesses to the same real event, not votes that need
    -- to agree.
    UPDATE tasks SET legacy_classification = 'dual_ai_evidenced'
      WHERE state NOT IN ('completed', 'failed', 'cancelled')
        AND (
          EXISTS (SELECT 1 FROM task_actions ta WHERE ta.task_id = tasks.id AND ta.action_type = 'ai_review')
          OR EXISTS (SELECT 1 FROM task_reviews tr WHERE tr.task_id = tasks.id)
        );

    -- Everything else still non-terminal (including a task whose
    -- development simply never got far enough to prove anything either
    -- way) falls back to unknown_insufficient — never silently treated as
    -- single-AI, because the shared dev-step code can't prove that on its
    -- own (see the design docs this migration implements).
    UPDATE tasks SET legacy_classification = 'unknown_insufficient'
      WHERE state NOT IN ('completed', 'failed', 'cancelled')
        AND legacy_classification IS NULL;
";

const V12_SQL: &str = include_str!("requirements_v12.sql");

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
    Migration {
        version: 3,
        requires_backup: true,
        sql: V3_SQL,
    },
    Migration {
        version: 4,
        requires_backup: true,
        sql: V4_SQL,
    },
    Migration {
        version: 5,
        requires_backup: true,
        sql: V5_SQL,
    },
    Migration {
        version: 6,
        requires_backup: true,
        sql: V6_SQL,
    },
    Migration {
        version: 7,
        requires_backup: true,
        sql: V7_SQL,
    },
    Migration {
        version: 8,
        requires_backup: true,
        sql: V8_SQL,
    },
    Migration {
        version: 9,
        requires_backup: true,
        sql: V9_SQL,
    },
    Migration {
        version: 10,
        requires_backup: true,
        sql: V10_SQL,
    },
    Migration {
        version: 11,
        requires_backup: true,
        sql: V11_SQL,
    },
    Migration { version: 12, requires_backup: true, sql: V12_SQL },
];

#[cfg(test)]
thread_local! { static LOCKED_MIGRATION_VERSIONS: std::cell::RefCell<Vec<(i64,i64)>> = const { std::cell::RefCell::new(Vec::new()) }; static BEFORE_MIGRATION_LOCK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) }; }


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
    #[cfg(test)]
    BEFORE_MIGRATION_LOCK.with(|hook| { if let Some(callback)=hook.borrow_mut().take(){callback();} });

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
        let locked_version: i64 = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
        #[cfg(test)]
        LOCKED_MIGRATION_VERSIONS.with(|v| v.borrow_mut().push((migration.version,locked_version)));
        if locked_version > CURRENT_SCHEMA_VERSION {
            conn.execute_batch("ROLLBACK;")?;
            return Err(MigrationError::VersionTooNew { found: locked_version, supported: CURRENT_SCHEMA_VERSION });
        }
        if migration.version <= locked_version {
            conn.execute_batch("COMMIT;")?;
            continue;
        }
        let apply_result = (|| -> Result<(), rusqlite::Error> {
            if migration.version == 5 {
                // Second re-review (Major 3, continued): v5's own SQL below
                // is already shipped and must never be edited in place — see
                // its doc comment. This runs *inside the same transaction*,
                // immediately before v5's `DROP TABLE`, so every row v5 is
                // about to discard is preserved first; if anything after
                // this fails, the whole transaction (including this
                // quarantine copy) rolls back together, exactly like any
                // other migration failure.
                quarantine_rows_v5_will_drop(conn)?;
            }
            conn.execute_batch(migration.sql)?;
            conn.execute_batch(&format!("PRAGMA user_version = {};", migration.version))
        })();

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
    use rusqlite::params;
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
    fn upgrades_a_v2_database_to_v3_relabeling_old_acceptance_as_generation_zero() {
        let dir = temp_dir("v2-to-v3");
        let (conn, path) = open_file_db(&dir);
        // Simulate an existing Phase 2 (v2) install: baseline + Phase 2
        // schema, `user_version = 2`, with one real project/task and one
        // pre-remediation acceptance row (the old `task_id`-only-unique
        // shape).
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, purpose, long_term_goal, user_preferences, prohibitions, last_activity_at, created_at, updated_at, last_opened_at, data_format_version) VALUES ('proj-1','P','C:\\p','c:\\p','local',NULL,NULL,NULL,NULL,'t','t','t','t',1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO project_memory (project_id, updated_at) VALUES ('proj-1','t')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, created_at, updated_at) VALUES ('task-1','proj-1','T','awaiting_user_acceptance','t','t')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO user_acceptance (id, project_id, task_id, accepted, user_note, accepted_at, created_at) VALUES ('acc-1','proj-1','task-1',1,'old note','t','t')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            version, CURRENT_SCHEMA_VERSION,
            "must continue past v3 all the way to the current version"
        );

        let result_generation: i64 = conn
            .query_row(
                "SELECT result_generation FROM tasks WHERE id = 'task-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            result_generation, 0,
            "existing tasks default to generation 0"
        );

        let (migrated_generation, migrated_accepted): (i64, i64) = conn
            .query_row(
                "SELECT result_generation, accepted FROM user_acceptance WHERE id = 'acc-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            migrated_generation, 0,
            "pre-remediation acceptance rows migrate in as generation 0"
        );
        assert_eq!(
            migrated_accepted, 1,
            "the historical accepted value itself is preserved"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn upgrades_a_v3_database_to_v4_adding_task_reviews() {
        let dir = temp_dir("v3-to-v4");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();

        run(&conn, Some(&path)).unwrap();

        // `run()` always migrates all the way to `CURRENT_SCHEMA_VERSION`,
        // not just to 4 — a v3 database run through today's `run()`
        // legitimately lands there, cascading through every later step too.
        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        // The new table must exist and enforce its CHECK/UNIQUE constraints.
        conn.query_row("SELECT COUNT(*) FROM task_reviews", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_v4_passes_integrity_and_foreign_key_pragma_checks() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");

        let fk_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            fk_violations, 0,
            "a fresh v4 database must have zero foreign key violations"
        );

        let tasks_columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(tasks)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(
            tasks_columns.contains(&"result_generation".to_string()),
            "tasks.result_generation must exist after v4: {tasks_columns:?}"
        );

        let acceptance_columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(user_acceptance)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(
            acceptance_columns.contains(&"result_generation".to_string()),
            "user_acceptance.result_generation must exist after v4: {acceptance_columns:?}"
        );

        let acceptance_fk_tables: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA foreign_key_list(user_acceptance)")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(2))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(
            acceptance_fk_tables
                .iter()
                .any(|table| table == "nova_projects"),
            "user_acceptance must have a foreign key to nova_projects: {acceptance_fk_tables:?}"
        );
        assert!(
            acceptance_fk_tables.iter().any(|table| table == "tasks"),
            "user_acceptance must have a foreign key to tasks: {acceptance_fk_tables:?}"
        );
    }

    #[test]
    fn upgrades_a_v4_database_to_v5_with_a_consistent_review_row_preserved() {
        let dir = temp_dir("v4-to-v5-preserve");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();

        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_reviews (id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at)
             VALUES ('rev-1', 'proj-1', 'task-1', 1, 1, 'claude_cli', 'approve', 'ok', '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_reviews", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 1,
            "a consistent v4 review row must survive the v5 upgrade"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn v4_to_v5_upgrade_drops_a_review_row_whose_project_id_does_not_match_its_task() {
        let dir = temp_dir("v4-to-v5-drop-inconsistent");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();

        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-real', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-other', 'Q', 'C:/q', 'c:/q', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-real', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        // A row whose project_id (proj-other) disagrees with what task-1
        // actually belongs to (proj-real) — exactly the cross-project
        // over-reach shape Major 3 flags. Foreign keys are off for this raw
        // insert so the pre-v5 schema (which has no composite FK at all)
        // accepts it, matching how a genuine V4-era inconsistency could
        // have been written.
        conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
        conn.execute(
            "INSERT INTO task_reviews (id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at)
             VALUES ('rev-bad', 'proj-other', 'task-1', 1, 1, 'claude_cli', 'approve', 'ok', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        run(&conn, Some(&path)).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_reviews", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            count, 0,
            "a review row with a project_id inconsistent with its task must not survive the v5 upgrade"
        );

        // Second re-review (Major 3, continued): v5 itself is unchanged
        // (still drops the row from the live table above), but the row must
        // not have vanished from the database entirely — it must be
        // preserved in the quarantine table with a clear reason.
        let (quarantined_id, reason): (String, String) = conn
            .query_row(
                "SELECT id, quarantine_reason FROM task_reviews_migration_quarantine_v5",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(quarantined_id, "rev-bad");
        assert!(
            reason.contains("no task found"),
            "reason should explain the ownership mismatch: {reason}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn v4_to_v5_upgrade_quarantines_a_review_row_with_an_out_of_range_round_instead_of_losing_it() {
        let dir = temp_dir("v4-to-v5-quarantine-round");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();

        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        // A genuinely consistent (task_id, project_id) pairing, but with a
        // round value v5's own CHECK(round IN (1,2)) cannot accept — the
        // *other* way a pre-v5 row could fail the upgrade's consistency
        // check, distinct from the cross-project case above.
        conn.execute(
            "INSERT INTO task_reviews (id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at)
             VALUES ('rev-round-3', 'proj-1', 'task-1', 1, 3, 'claude_cli', 'approve', 'ok', '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM task_reviews", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 0);

        let (quarantined_id, reason): (String, String) = conn
            .query_row(
                "SELECT id, quarantine_reason FROM task_reviews_migration_quarantine_v5",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(quarantined_id, "rev-round-3");
        assert!(
            reason.contains("round not in"),
            "reason should explain the round check failure: {reason}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_fresh_database_produces_an_empty_but_present_quarantine_table() {
        // The quarantine pre-step must be harmless (not merely "not
        // destructive") for the overwhelmingly common case: a brand-new
        // install with no pre-v5 data at all.
        let dir = temp_dir("fresh-db-empty-quarantine");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM task_reviews_migration_quarantine_v5",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_v5_rejects_a_round_outside_one_or_two() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO task_reviews (id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at)
             VALUES ('rev-1', 'proj-1', 'task-1', 1, 3, 'claude_cli', 'approve', NULL, '2026-01-01')",
            [],
        );
        assert!(
            result.is_err(),
            "round=3 must be rejected by CHECK(round IN (1,2))"
        );
    }

    #[test]
    fn schema_v5_rejects_a_review_whose_project_id_does_not_match_its_task_via_the_composite_fk() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-real', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-other', 'Q', 'C:/q', 'c:/q', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-real', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO task_reviews (id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, findings_summary, created_at)
             VALUES ('rev-bad', 'proj-other', 'task-1', 1, 1, 'claude_cli', 'approve', NULL, '2026-01-01')",
            [],
        );
        assert!(
            result.is_err(),
            "a project_id that disagrees with the task's real project must be rejected by the composite FK"
        );
    }

    #[test]
    fn schema_v5_passes_integrity_and_foreign_key_pragma_checks() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");

        let fk_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            fk_violations, 0,
            "a fresh v5 database must have zero foreign key violations"
        );
    }

    #[test]
    fn upgrades_a_v5_database_to_v6_migrating_a_pre_existing_test_evidence_row() {
        let dir = temp_dir("v5-to-v6-preserve");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();
        conn.execute_batch(V5_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 5;").unwrap();

        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO test_evidence (id, project_id, task_id, test_command_redacted, passed_count, failed_count, skipped_count, exit_code, created_at)
             VALUES ('te-1', 'proj-1', 'task-1', 'cargo test', 10, 0, 0, 0, '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        let (status, generation, round): (String, i64, i64) = conn
            .query_row(
                "SELECT status, result_generation, round FROM test_evidence WHERE id = 'te-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(status, "passed");
        assert_eq!(generation, 0);
        assert_eq!(round, 1);

        let tasks_columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(tasks)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(tasks_columns.contains(&"test_command_redacted".to_string()));
        assert!(tasks_columns.contains(&"test_command_source".to_string()));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_v6_rejects_an_invalid_test_evidence_status() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO test_evidence (id, project_id, task_id, result_generation, round, status, created_at)
             VALUES ('te-1', 'proj-1', 'task-1', 0, 1, 'maybe_passed', '2026-01-01')",
            [],
        );
        assert!(
            result.is_err(),
            "an unrecognized status must be rejected by the CHECK constraint"
        );
    }

    #[test]
    fn schema_v6_passes_integrity_and_foreign_key_pragma_checks() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");

        let fk_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            fk_violations, 0,
            "a fresh v6 database must have zero foreign key violations"
        );
    }

    #[test]
    fn schema_v8_passes_integrity_and_foreign_key_pragma_checks() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");

        let fk_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            fk_violations, 0,
            "a fresh v8 database must have zero foreign key violations"
        );

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(build_evidence)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for expected in [
            "id",
            "project_id",
            "task_id",
            "result_generation",
            "round",
            "build_command_redacted",
            "status",
            "exit_code",
            "reason",
            "stdout_ref",
            "stderr_ref",
            "created_at",
        ] {
            assert!(
                columns.contains(&expected.to_string()),
                "build_evidence missing column {expected}"
            );
        }
    }

    #[test]
    fn upgrades_a_v7_database_to_v8_adding_build_evidence() {
        // Codex re-review remediation (Major 3): a purely additive
        // migration — an old v7 database (with real project/task rows, and
        // no `build_evidence` table at all) must upgrade cleanly to v8, end
        // up with a real, empty, queryable `build_evidence` table, and must
        // never lose or alter any pre-existing row in any other table.
        let dir = temp_dir("v7-to-v8-additive");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();
        conn.execute_batch(V5_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 5;").unwrap();
        conn.execute_batch(V6_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 6;").unwrap();
        conn.execute_batch(V7_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 7;").unwrap();

        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        let build_evidence_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM build_evidence", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            build_evidence_rows, 0,
            "an upgraded v7 database must have a real, empty build_evidence table"
        );

        conn.execute(
            "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, status, created_at)
             VALUES ('be-1', 'proj-1', 'task-1', 0, 1, 'passed', '2026-01-01')",
            [],
        )
        .unwrap();
        let title: String = conn
            .query_row("SELECT title FROM tasks WHERE id = 'task-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            title, "T",
            "upgrading to v8 must never alter a pre-existing tasks row"
        );
    }

    #[test]
    fn build_evidence_rejects_an_unknown_status() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, status, created_at)
             VALUES ('be-1', 'proj-1', 'task-1', 0, 1, 'maybe_passed', '2026-01-01')",
            [],
        );
        assert!(
            result.is_err(),
            "an unrecognized build_evidence.status must be rejected by the CHECK constraint"
        );
    }

    #[test]
    fn schema_v9_passes_integrity_and_foreign_key_pragma_checks() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");

        let fk_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            fk_violations, 0,
            "a fresh v9 database must have zero foreign key violations"
        );

        let columns: Vec<String> = {
            let mut stmt = conn
                .prepare("PRAGMA table_info(no_build_confirmations)")
                .unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        for expected in [
            "id",
            "project_id",
            "task_id",
            "result_generation",
            "round",
            "confirmed_by",
            "reason",
            "confirmed_at",
            "created_at",
        ] {
            assert!(
                columns.contains(&expected.to_string()),
                "no_build_confirmations missing column {expected}"
            );
        }
    }

    #[test]
    fn upgrades_a_v8_database_to_v9_adding_no_build_confirmations() {
        let dir = temp_dir("v8-to-v9-additive");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();
        conn.execute_batch(V5_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 5;").unwrap();
        conn.execute_batch(V6_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 6;").unwrap();
        conn.execute_batch(V7_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 7;").unwrap();
        conn.execute_batch(V8_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 8;").unwrap();

        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        let confirmation_rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM no_build_confirmations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            confirmation_rows, 0,
            "an upgraded v8 database must have a real, empty no_build_confirmations table"
        );

        conn.execute(
            "INSERT INTO no_build_confirmations (id, project_id, task_id, result_generation, round, confirmed_by, confirmed_at, created_at)
             VALUES ('nbc-1', 'proj-1', 'task-1', 0, 1, 'user', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        let title: String = conn
            .query_row("SELECT title FROM tasks WHERE id = 'task-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            title, "T",
            "upgrading to v9 must never alter a pre-existing tasks row"
        );
    }

    #[test]
    fn no_build_confirmations_rejects_a_non_user_confirmed_by() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        let result = conn.execute(
            "INSERT INTO no_build_confirmations (id, project_id, task_id, result_generation, round, confirmed_by, confirmed_at, created_at)
             VALUES ('nbc-1', 'proj-1', 'task-1', 0, 1, 'system', '2026-01-01', '2026-01-01')",
            [],
        );
        assert!(
            result.is_err(),
            "confirmed_by must be rejected by the CHECK constraint unless it is exactly 'user'"
        );
    }

    // --- Phase 4 remediation (second re-review): schema v10, persisted
    // no-build authorization source ---------------------------------------

    #[test]
    fn schema_v10_passes_integrity_and_foreign_key_checks_and_has_authorization_source_column() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");

        let fk_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            fk_violations, 0,
            "a fresh v10 database must have zero foreign key violations"
        );

        let columns: Vec<String> = {
            let mut stmt = conn.prepare("PRAGMA table_info(build_evidence)").unwrap();
            stmt.query_map([], |row| row.get::<_, String>(1))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        assert!(
            columns.contains(&"authorization_source".to_string()),
            "build_evidence missing column authorization_source: {columns:?}"
        );
    }

    #[test]
    fn upgrades_a_v9_database_to_v10_marking_pre_existing_no_build_applicable_rows_as_legacy_unknown(
    ) {
        let dir = temp_dir("v9-to-v10-legacy");
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();
        conn.execute_batch(V5_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 5;").unwrap();
        conn.execute_batch(V6_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 6;").unwrap();
        conn.execute_batch(V7_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 7;").unwrap();
        conn.execute_batch(V8_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 8;").unwrap();
        conn.execute_batch(V9_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 9;").unwrap();

        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        // Two pre-v10 rows: one real "passed" build, and one
        // `no_build_applicable` row with no way to know — at migration
        // time — whether it was originally mechanical or user-confirmed.
        conn.execute(
            "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, status, created_at)
             VALUES ('be-passed', 'proj-1', 'task-1', 0, 1, 'passed', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, status, created_at)
             VALUES ('be-na', 'proj-1', 'task-1', 0, 2, 'no_build_applicable', '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let version: i64 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        let passed_source: Option<String> = conn
            .query_row(
                "SELECT authorization_source FROM build_evidence WHERE id = 'be-passed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            passed_source, None,
            "a real 'passed' row must never gain an authorization_source"
        );

        let na_source: Option<String> = conn
            .query_row(
                "SELECT authorization_source FROM build_evidence WHERE id = 'be-na'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            na_source,
            Some("legacy_unknown".to_string()),
            "a pre-existing no_build_applicable row must migrate to legacy_unknown — never \
             inferred as mechanical or user-confirmed from the upgrading machine's current state"
        );

        let title: String = conn
            .query_row("SELECT title FROM tasks WHERE id = 'task-1'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            title, "T",
            "upgrading to v10 must never alter a pre-existing tasks row"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn schema_v11_adds_wp2a_tables_and_result_attempt_columns() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();

        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap();
        assert_eq!(integrity, "ok");

        let fk_violations: i64 = conn
            .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(fk_violations, 0);

        for (table, column) in [
            ("tasks", "legacy_classification"),
            ("test_evidence", "result_attempt"),
            ("build_evidence", "result_attempt"),
            ("no_tests_confirmations", "result_attempt"),
            ("no_build_confirmations", "result_attempt"),
        ] {
            let columns: Vec<String> = {
                let mut stmt = conn
                    .prepare(&format!("PRAGMA table_info({table})"))
                    .unwrap();
                stmt.query_map([], |row| row.get::<_, String>(1))
                    .unwrap()
                    .collect::<Result<Vec<_>, _>>()
                    .unwrap()
            };
            assert!(
                columns.contains(&column.to_string()),
                "{table} missing column {column}: {columns:?}"
            );
        }

        conn.query_row("SELECT COUNT(*) FROM self_checks", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
        conn.query_row("SELECT COUNT(*) FROM confirmed_requirement_items", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    }

    /// Builds a file-backed database pinned at schema v10 (every migration
    /// through `V10_SQL` applied, `user_version` set to 10, nothing from
    /// `V11_SQL` yet) — the fixed starting point every v10-to-v11 migration
    /// test below upgrades from.
    fn open_v10_database(name: &str) -> (Connection, PathBuf, PathBuf) {
        let dir = temp_dir(name);
        let (conn, path) = open_file_db(&dir);
        conn.execute_batch(BASELINE_SQL).unwrap();
        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();
        conn.execute_batch(PHASE2_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 2;").unwrap();
        conn.execute_batch(V3_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 3;").unwrap();
        conn.execute_batch(V4_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 4;").unwrap();
        conn.execute_batch(V5_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 5;").unwrap();
        conn.execute_batch(V6_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 6;").unwrap();
        conn.execute_batch(V7_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 7;").unwrap();
        conn.execute_batch(V8_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 8;").unwrap();
        conn.execute_batch(V9_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 9;").unwrap();
        conn.execute_batch(V10_SQL).unwrap();
        conn.execute_batch("PRAGMA user_version = 10;").unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        (conn, path, dir)
    }


    fn seed_wp2a_v10_history(conn: &Connection) {
        conn.execute_batch("INSERT INTO tasks(id,project_id,title,state,created_at,updated_at) VALUES('history','proj-1','historical','awaiting_user_acceptance','time','time');
        INSERT INTO test_evidence(id,project_id,task_id,result_generation,round,status,passed_count,created_at) VALUES('old-test','proj-1','history',0,1,'passed',7,'time');
        INSERT INTO build_evidence(id,project_id,task_id,result_generation,round,status,authorization_source,created_at) VALUES('old-build','proj-1','history',0,1,'no_build_applicable','legacy_unknown','time');
        INSERT INTO no_tests_confirmations(id,project_id,task_id,result_generation,round,confirmed_by,reason,confirmed_at,created_at) VALUES('old-nt','proj-1','history',0,1,'user','preserve tests','time','time');
        INSERT INTO no_build_confirmations(id,project_id,task_id,result_generation,round,confirmed_by,reason,confirmed_at,created_at) VALUES('old-nb','proj-1','history',0,1,'user','preserve build','time','time');").unwrap();
    }

    #[test]
    fn wp2a_v11_preserves_four_tables_null_attribution_and_indexes() {
        let (conn,path,_dir)=open_v10_database("wp2a-preservation");seed_wp2a_v10_history(&conn);
        run(&conn,Some(&path)).unwrap();
        for table in ["test_evidence","build_evidence","no_tests_confirmations","no_build_confirmations"] {
            let values:(i64,i64)=conn.query_row(&format!("SELECT count(*),count(result_attempt) FROM {table} WHERE task_id='history'"),[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();assert_eq!(values,(1,0),"{table}");
            let indexes:i64=conn.query_row(&format!("SELECT count(*) FROM pragma_index_list('{table}')"),[],|r|r.get(0)).unwrap();assert!(indexes>=2,"{table}");
        }
        assert_eq!(conn.query_row("SELECT passed_count FROM test_evidence WHERE id='old-test'",[],|r|r.get::<_,i64>(0)).unwrap(),7);
        assert_eq!(conn.query_row("SELECT reason FROM no_build_confirmations WHERE id='old-nb'",[],|r|r.get::<_,String>(0)).unwrap(),"preserve build");
        for attempt in [1,2] {conn.execute("INSERT INTO no_build_confirmations(id,project_id,task_id,result_generation,round,result_attempt,confirmed_by,confirmed_at,created_at) VALUES(?1,'proj-1','history',0,1,?2,'user','time','time')",rusqlite::params![format!("new-{attempt}"),attempt]).unwrap();}
        assert!(conn.execute("INSERT INTO no_build_confirmations(id,project_id,task_id,result_generation,round,result_attempt,confirmed_by,confirmed_at,created_at) VALUES('duplicate','proj-1','history',0,1,2,'user','time','time')",[]).is_err());
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0)).unwrap(),0);
    }

    #[test]
    fn wp2a_v11_failure_after_test_table_rebuild_rolls_back_the_whole_step() {
        let (conn,path,dir)=open_v10_database("wp2a-atomic-v11");seed_wp2a_v10_history(&conn);
        // Collision occurs after test_evidence DROP/RENAME, not before any work.
        conn.execute_batch("CREATE TABLE build_evidence_v11(block INTEGER);").unwrap();
        assert!(run(&conn,Some(&path)).is_err());
        assert_eq!(conn.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),10);
        assert_eq!(conn.query_row("SELECT count(*) FROM pragma_table_info('tasks') WHERE name='legacy_classification'",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        for table in ["test_evidence","build_evidence","no_tests_confirmations","no_build_confirmations"] {
            assert_eq!(conn.query_row(&format!("SELECT count(*) FROM {table} WHERE task_id='history'"),[],|r|r.get::<_,i64>(0)).unwrap(),1);
            assert_eq!(conn.query_row(&format!("SELECT count(*) FROM pragma_table_info('{table}') WHERE name='result_attempt'"),[],|r|r.get::<_,i64>(0)).unwrap(),0);
        }
        assert_eq!(std::fs::read_dir(dir.join("backups")).unwrap().count(),1);
        assert_eq!(conn.query_row("PRAGMA integrity_check",[],|r|r.get::<_,String>(0)).unwrap(),"ok");
    }

    #[test]
    fn wp2a_online_backup_and_closed_connection_restore_include_wal_commits() {
        let (conn,path,dir)=open_v10_database("wp2a-wal-restore");
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;").unwrap();
        seed_wp2a_v10_history(&conn);
        assert!(path.with_extension("sqlite3-wal").exists());
        // Online backup reads committed WAL content while the source is open.
        let backup=backup_database(&conn,&path).unwrap();
        conn.execute("UPDATE tasks SET title='after backup' WHERE id='history'",[]).unwrap();
        drop(conn); // Restore only after every source connection is closed.
        let restored=dir.join("restored.sqlite3"); std::fs::copy(&backup,&restored).unwrap();
        // Restore to a fresh name: never pair a backup with stale WAL/SHM files.
        assert!(!restored.with_extension("sqlite3-wal").exists());
        let check=Connection::open_with_flags(&restored,rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        assert_eq!(check.query_row("SELECT title FROM tasks WHERE id='history'",[],|r|r.get::<_,String>(0)).unwrap(),"historical");
        assert_eq!(check.query_row("PRAGMA user_version",[],|r|r.get::<_,i64>(0)).unwrap(),10);
        assert_eq!(check.query_row("PRAGMA integrity_check",[],|r|r.get::<_,String>(0)).unwrap(),"ok");
        assert_eq!(check.query_row("SELECT count(*) FROM pragma_foreign_key_check",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(check.query_row("SELECT count(*) FROM test_evidence WHERE task_id='history'",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }

    #[test]
    fn migration_classifies_a_non_terminal_task_with_an_ai_review_action_as_dual_ai_evidenced() {
        let (conn, path, dir) = open_v10_database("v10-to-v11-dual-action");
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'verifying', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_actions (id, task_id, action_type, idempotency_key, status, created_at, updated_at)
             VALUES ('act-1', 'task-1', 'ai_review', 'review-task-1-round-1', 'succeeded', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let classification: Option<String> = conn
            .query_row(
                "SELECT legacy_classification FROM tasks WHERE id = 'task-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(classification, Some("dual_ai_evidenced".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_classifies_a_non_terminal_task_with_a_task_reviews_row_as_dual_ai_evidenced() {
        let (conn, path, dir) = open_v10_database("v10-to-v11-dual-review");
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'verifying', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO task_reviews (id, project_id, task_id, result_generation, round, reviewer_ai_kind, verdict, created_at)
             VALUES ('rev-1', 'proj-1', 'task-1', 0, 1, 'codex_cli', 'approve', '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let classification: Option<String> = conn
            .query_row(
                "SELECT legacy_classification FROM tasks WHERE id = 'task-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(classification, Some("dual_ai_evidenced".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_classifies_a_non_terminal_task_with_no_review_signal_as_unknown_insufficient() {
        let (conn, path, dir) = open_v10_database("v10-to-v11-unknown");
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'running', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();

        run(&conn, Some(&path)).unwrap();

        let classification: Option<String> = conn
            .query_row(
                "SELECT legacy_classification FROM tasks WHERE id = 'task-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(classification, Some("unknown_insufficient".to_string()));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migration_never_classifies_a_task_already_terminal_at_migration_time() {
        let (conn, path, dir) = open_v10_database("v10-to-v11-terminal");
        // Three terminal states, none with any review signal — proving the
        // freeze is about non-terminal-at-migration-time, not about
        // "lacks a review signal" alone (a terminal task lacking one is
        // simply done, not frozen).
        for (id, state) in [
            ("task-completed", "completed"),
            ("task-failed", "failed"),
            ("task-cancelled", "cancelled"),
        ] {
            conn.execute(
                "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
                 VALUES (?1, 'proj-1', 'T', ?2, 0, '2026-01-01', '2026-01-01')",
                params![id, state],
            )
            .unwrap();
        }

        run(&conn, Some(&path)).unwrap();

        for id in ["task-completed", "task-failed", "task-cancelled"] {
            let classification: Option<String> = conn
                .query_row(
                    "SELECT legacy_classification FROM tasks WHERE id = ?1",
                    params![id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(
                classification, None,
                "a task already terminal at migration time ({id}) must never be classified/frozen"
            );
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_task_created_after_the_v11_migration_runs_is_never_classified_or_frozen() {
        let dir = temp_dir("v11-fresh-task-not-frozen");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        // A brand-new task created on an already-v11 database (the real
        // `Database::create_task` never sets `legacy_classification`
        // itself — simulated here by the same bare INSERT, which leaves
        // the column at its default, implicit NULL).
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-new', 'proj-1', 'T', 'running', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();

        let classification: Option<String> = conn
            .query_row(
                "SELECT legacy_classification FROM tasks WHERE id = 'task-new'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            classification, None,
            "a task created after the v11 migration has already run must never be \
             retroactively classified/frozen — only the one-time migration UPDATE does that"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_evidence_authorization_source_check_rejects_invalid_combinations() {
        let conn = Connection::open_in_memory().unwrap();
        run(&conn, None).unwrap();
        conn.execute(
            "INSERT INTO nova_projects (id, name, local_path, normalized_path, source_type, created_at, updated_at)
             VALUES ('proj-1', 'P', 'C:/p', 'c:/p', 'local', '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks (id, project_id, title, state, result_generation, created_at, updated_at)
             VALUES ('task-1', 'proj-1', 'T', 'draft', 0, '2026-01-01', '2026-01-01')",
            [],
        )
        .unwrap();

        // A real "passed" row must never carry an authorization_source.
        let passed_with_source = conn.execute(
            "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, status, authorization_source, created_at)
             VALUES ('be-1', 'proj-1', 'task-1', 0, 1, 'passed', 'mechanical_not_applicable', '2026-01-01')",
            [],
        );
        assert!(
            passed_with_source.is_err(),
            "a non-no_build_applicable status must never carry an authorization_source"
        );

        // A `no_build_applicable` row must always carry one of the three
        // known values — never NULL, never an arbitrary string.
        let na_without_source = conn.execute(
            "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, status, created_at)
             VALUES ('be-2', 'proj-1', 'task-1', 0, 1, 'no_build_applicable', '2026-01-01')",
            [],
        );
        assert!(
            na_without_source.is_err(),
            "a no_build_applicable row without any authorization_source must be rejected"
        );

        let na_with_unknown_source = conn.execute(
            "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, status, authorization_source, created_at)
             VALUES ('be-3', 'proj-1', 'task-1', 0, 1, 'no_build_applicable', 'ai_claimed_not_applicable', '2026-01-01')",
            [],
        );
        assert!(
            na_with_unknown_source.is_err(),
            "an arbitrary authorization_source string must be rejected by the CHECK constraint"
        );

        // The three legitimate values must all be accepted.
        for (id, source) in [
            ("be-4", "user_confirmed_not_applicable"),
            ("be-5", "mechanical_not_applicable"),
            ("be-6", "legacy_unknown"),
        ] {
            let result = conn.execute(
                &format!(
                    "INSERT INTO build_evidence (id, project_id, task_id, result_generation, round, status, authorization_source, created_at)
                     VALUES ('{id}', 'proj-1', 'task-1', 0, 1, 'no_build_applicable', '{source}', '2026-01-01')"
                ),
                [],
            );
            assert!(result.is_ok(), "{source} must be accepted: {result:?}");
        }
    }

    #[test]
    fn reopening_a_v10_database_does_not_re_migrate_or_error() {
        let dir = temp_dir("v10-reopen-idempotent");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();
        drop(conn);

        // A second, independent connection to the same file must see the
        // already-migrated schema and do nothing further.
        let conn2 = Connection::open(&path).unwrap();
        run(&conn2, Some(&path)).unwrap();
        let version: i64 = conn2
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, CURRENT_SCHEMA_VERSION);

        let backups_dir = dir.join("backups");
        let count = fs::read_dir(&backups_dir)
            .map(|iter| iter.filter_map(|e| e.ok()).count())
            .unwrap_or(0);
        assert_eq!(
            count, 11,
            "reopening an already-fully-migrated database must not take any new backups"
        );
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
            11,
            "expected exactly eleven backups (migrations 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, and 12 each require one): {entries:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_filename_contains_beijing_style_timestamp_and_is_unique_per_run() {
        let dir = temp_dir("backup-unique");
        let (conn, path) = open_file_db(&dir);
        run(&conn, Some(&path)).unwrap();

        // A second, independent database in the same directory must not
        // collide with or overwrite the first database's backups.
        let dir2 = temp_dir("backup-unique-2");
        let (conn2, path2) = open_file_db(&dir2);
        run(&conn2, Some(&path2)).unwrap();

        let backups_dir = dir.join("backups");
        let names: Vec<String> = fs::read_dir(&backups_dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names.len(),
            11,
            "migrations 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, and 12 each produce their own backup"
        );
        let unique: std::collections::HashSet<&String> = names.iter().collect();
        assert_eq!(
            unique.len(),
            11,
            "all backups from the same run must have distinct names, never collide"
        );
        for name in &names {
            assert!(name.len() > "nova-backup-YYYYMMDDHHMMSS-".len());
        }

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
            count, 11,
            "already-applied migrations must not be re-run, so no extra backups beyond the eleven produced by the first run"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
#[path = "workspace_migration_tests.rs"]
mod workspace_migration_tests;
