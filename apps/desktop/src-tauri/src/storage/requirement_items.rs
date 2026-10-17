//! WP2A-0: splits a task's `goal`/`in_scope`/`not_in_scope`/`done_criteria`
//! free-text fields into stable, individually-addressable requirement
//! items (see `models::ConfirmedRequirementItem`) at the moment the task's
//! requirement is confirmed. These four fields have no edit path anywhere
//! in this codebase (only `Database::create_task` ever writes them), so
//! the split, once taken, never needs to be versioned or re-taken for the
//! same task — it is written once and is permanently accurate.

use rusqlite::params;

use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::models::{ConfirmedRequirementItem, Task, TaskState};
use crate::security::redact_sensitive_text;

use super::{new_id, now};

/// The four fields, in the fixed order their items are generated — purely
/// for readability of callers; the split itself is independent per field.
/// Splits one field's text into line-items: normalize `\r\n`/`\r` to `\n`,
/// split on `\n`, trim each line, drop lines that are empty after
/// trimming, keep the rest in their original relative order (including
/// exact duplicates — two identical lines in the source text become two
/// distinct items). Returns `item_index`-ordered text, not yet persisted.
pub fn split_requirement_field(text: &str) -> Vec<String> {
    text.replace("\r\n", "\n")
        .replace('\r', "\n")
        .split('\n')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Splits all four of a task's requirement fields into the full ordered
/// `(field, item_index, text)` list a fresh snapshot would insert — pure
/// computation, no I/O. A field that is `None` or splits to zero non-empty
/// lines simply contributes no items.
pub fn split_requirement_fields(task: &Task) -> Vec<(&'static str, i64, String)> {
    let mut items = Vec::new();
    for (field_name, field_value) in [
        ("goal", &task.goal),
        ("in_scope", &task.in_scope),
        ("not_in_scope", &task.not_in_scope),
        ("done_criteria", &task.done_criteria),
    ] {
        if let Some(text) = field_value {
            for (index, line) in split_requirement_field(text).into_iter().enumerate() {
                items.push((field_name, index as i64, line));
            }
        }
    }
    items
}

impl Database {
    /// Explicit user-requested backfill; never changes requirements or legacy freeze.
    pub fn generate_requirement_snapshot(&self, project_id: &str, task_id: &str) -> NovaResult<Vec<ConfirmedRequirementItem>> {
        self.conn().execute_batch("BEGIN IMMEDIATE;")?;
        let result = (|| {
            let task = self.task(task_id)?;
            if task.project_id != project_id {
                return Err(NovaError::Config("task_id 与 project_id 不匹配，已拒绝".into()));
            }
            // Check before even reading an existing snapshot (idempotence is not permission).
            if !matches!(task.state, TaskState::AwaitingConfirmation | TaskState::Confirmed
                | TaskState::Queued | TaskState::Running | TaskState::Verifying | TaskState::SafeFixing
                | TaskState::PreviewReady | TaskState::AwaitingUserAcceptance | TaskState::NeedsUserAction
                | TaskState::Interrupted) {
                return Err(NovaError::Config("Draft 尚未确认需求，终态任务不允许补建需求快照".into()));
            }
            self.insert_confirmed_requirement_items_if_absent(project_id, &task)
        })();
        match result {
            Ok(items) => { self.conn().execute_batch("COMMIT;")?; Ok(items) }
            Err(error) => { let _ = self.conn().execute_batch("ROLLBACK;"); Err(error) }
        }
    }

    /// Inserts the full requirement-item snapshot for a task, computed by
    /// [`split_requirement_fields`]. Idempotent in effect: if the task
    /// already has any rows, this is a silent no-op (never deletes/
    /// overwrites an existing snapshot — see the WP2A design docs' "must
    /// never silently regenerate a confirmed snapshot" requirement). Must
    /// be called from inside the same transaction as whatever state write
    /// is triggering it (`Database::transition_task`'s `AwaitingConfirmation`
    /// branch, or the standalone `generate_requirement_snapshot` command),
    /// not as an independent, separately-committed step.
    pub(crate) fn insert_confirmed_requirement_items_if_absent(
        &self,
        project_id: &str,
        task: &Task,
    ) -> NovaResult<Vec<ConfirmedRequirementItem>> {
        if !self.list_confirmed_requirement_items(&task.id)?.is_empty() {
            return self.list_confirmed_requirement_items(&task.id);
        }
        let now = now();
        for (field, item_index, text) in split_requirement_fields(task) {
            let redacted = redact_sensitive_text(&text);
            self.conn().execute(
                "INSERT INTO confirmed_requirement_items (id, project_id, task_id, field, item_index, item_text_redacted, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![new_id(), project_id, task.id, field, item_index, redacted, now],
            )?;
        }
        self.list_confirmed_requirement_items(&task.id)
    }

    /// Every confirmed requirement item for a task, in stable
    /// `(field, item_index)` order — the authoritative set a self-check's
    /// `requirement_coverage` must match exactly.
    pub fn list_confirmed_requirement_items(
        &self,
        task_id: &str,
    ) -> NovaResult<Vec<ConfirmedRequirementItem>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, task_id, field, item_index, item_text_redacted, created_at
             FROM confirmed_requirement_items WHERE task_id = ?1 ORDER BY field ASC, item_index ASC",
        )?;
        let rows = stmt
            .query_map(params![task_id], read_item)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn read_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<ConfirmedRequirementItem> {
    Ok(ConfirmedRequirementItem {
        id: row.get(0)?,
        project_id: row.get(1)?,
        task_id: row.get(2)?,
        field: row.get(3)?,
        item_index: row.get(4)?,
        item_text_redacted: row.get(5)?,
        created_at: row.get(6)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_with(goal: Option<&str>, in_scope: Option<&str>) -> Task {
        Task {
            id: "t1".to_string(),
            project_id: "p1".to_string(),
            title: "T".to_string(),
            goal: goal.map(str::to_string),
            in_scope: in_scope.map(str::to_string),
            not_in_scope: None,
            done_criteria: None,
            ai_kind: None,
            state: crate::models::TaskState::Draft,
            result_generation: 0,
            test_command_redacted: None,
            test_command_source: None,
            document_sync_pending: false,
            legacy_classification: None,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
        }
    }

    #[test]
    fn splits_on_newlines_drops_empty_lines_keeps_order_and_duplicates() {
        let items = split_requirement_field("支持导出Excel\n\n支持导出PDF  \n支持导出Excel");
        assert_eq!(items, vec!["支持导出Excel", "支持导出PDF", "支持导出Excel"]);
    }

    #[test]
    fn a_none_or_blank_field_contributes_zero_items() {
        let task = task_with(None, Some("   \n\n  "));
        let items = split_requirement_fields(&task);
        assert!(items.is_empty());
    }

    #[test]
    fn items_are_keyed_by_field_and_index_in_source_order() {
        let task = task_with(Some("line a\nline b"), Some("scope 1"));
        let items = split_requirement_fields(&task);
        assert_eq!(
            items,
            vec![
                ("goal", 0, "line a".to_string()),
                ("goal", 1, "line b".to_string()),
                ("in_scope", 0, "scope 1".to_string()),
            ]
        );
    }
}
