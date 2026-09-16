use rusqlite::params;

use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::models::{Actor, ProjectMemory, ProjectMemoryRevision};

use super::{new_id, now};

/// The only fields `update_project_memory_field` will write. Deliberately a
/// closed set matched against a literal string, both so a typo fails loudly
/// instead of silently no-oping, and so nothing outside this list — in
/// particular no "private chain of thought" field, which this schema does
/// not and must not have — can ever be persisted through this path.
const MEMORY_FIELDS: &[&str] = &[
    "product_goals",
    "user_requirements",
    "confirmed_decisions",
    "open_questions",
    "deferred_items",
    "known_issues",
    "task_history_summary",
    "current_unfinished_task_id",
    "resume_position",
    "last_ai_used",
    "project_rules_index",
    "license_summary",
    "install_run_summary",
];

impl Database {
    pub fn project_memory(&self, project_id: &str) -> NovaResult<ProjectMemory> {
        self.conn()
            .query_row(
                "SELECT project_id, product_goals, user_requirements, confirmed_decisions,
                        open_questions, deferred_items, known_issues, task_history_summary,
                        current_unfinished_task_id, resume_position, last_ai_used,
                        project_rules_index, license_summary, install_run_summary, updated_at
                 FROM project_memory WHERE project_id = ?1",
                params![project_id],
                read_memory,
            )
            .map_err(Into::into)
    }

    /// Updates a single project-memory field, transactionally recording a
    /// revision row whenever the value actually changes (byte-for-byte
    /// equal writes are a no-op, so re-saving the same decision does not
    /// spam the revision log). Both the `project_memory` update and the
    /// `project_memory_revisions` insert happen in one transaction.
    pub fn update_project_memory_field(
        &self,
        project_id: &str,
        field_name: &str,
        new_value: Option<&str>,
        changed_by: Actor,
        reason: Option<&str>,
    ) -> NovaResult<ProjectMemory> {
        if !MEMORY_FIELDS.contains(&field_name) {
            return Err(NovaError::Config(format!("未知的记忆字段：{field_name}")));
        }

        let old_value: Option<String> = self.conn().query_row(
            &format!("SELECT {field_name} FROM project_memory WHERE project_id = ?1"),
            params![project_id],
            |row| row.get(0),
        )?;

        if old_value.as_deref() == new_value {
            return self.project_memory(project_id);
        }

        let now = now();
        self.conn().execute("BEGIN IMMEDIATE;", [])?;
        let result = (|| -> NovaResult<()> {
            self.conn().execute(
                &format!("UPDATE project_memory SET {field_name} = ?1, updated_at = ?2 WHERE project_id = ?3"),
                params![new_value, now, project_id],
            )?;
            self.conn().execute(
                "INSERT INTO project_memory_revisions (id, project_id, field_name, old_value, new_value, changed_by, reason, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![new_id(), project_id, field_name, old_value, new_value, changed_by.as_str(), reason, now],
            )?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn().execute("COMMIT;", [])?;
                self.project_memory(project_id)
            }
            Err(error) => {
                let _ = self.conn().execute("ROLLBACK;", []);
                Err(error)
            }
        }
    }

    pub fn list_memory_revisions(
        &self,
        project_id: &str,
    ) -> NovaResult<Vec<ProjectMemoryRevision>> {
        let mut stmt = self.conn().prepare(
            "SELECT id, project_id, field_name, old_value, new_value, changed_by, reason, created_at
             FROM project_memory_revisions WHERE project_id = ?1 ORDER BY created_at ASC",
        )?;
        let rows = stmt
            .query_map(params![project_id], read_revision)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Builds a plain-text structured context summary for handing to a
    /// development AI, from this project's memory alone — the query is
    /// scoped strictly by `project_id`, so it structurally cannot pull in
    /// another project's fields. This is a summary of durable memory
    /// fields, not a chat transcript: Phase 2 does not call a real AI, so
    /// this is exercised directly by tests for now.
    /// Not yet called from any Tauri command — Phase 2 does not invoke a
    /// real AI, so nothing consumes this summary outside its own tests
    /// until Phase 3 exists. Kept here now specifically so Phase 3 does not
    /// need another migration/schema round just to add it.
    #[allow(dead_code)]
    pub fn project_context_summary(&self, project_id: &str) -> NovaResult<String> {
        let memory = self.project_memory(project_id)?;
        let project = self.project(project_id)?;
        let mut sections = Vec::new();
        sections.push(format!("项目：{}", project.name));
        if let Some(goal) = project.long_term_goal.as_deref().filter(|s| !s.is_empty()) {
            sections.push(format!("长期产品目标：{goal}"));
        }
        if let Some(v) = memory.product_goals.as_deref().filter(|s| !s.is_empty()) {
            sections.push(format!("产品目标：{v}"));
        }
        if let Some(v) = memory
            .user_requirements
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            sections.push(format!("用户要求：{v}"));
        }
        if let Some(v) = memory
            .confirmed_decisions
            .as_deref()
            .filter(|s| !s.is_empty())
        {
            sections.push(format!("已确认决定：{v}"));
        }
        if let Some(v) = memory.open_questions.as_deref().filter(|s| !s.is_empty()) {
            sections.push(format!("待确认问题：{v}"));
        }
        if let Some(v) = memory.deferred_items.as_deref().filter(|s| !s.is_empty()) {
            sections.push(format!("暂不处理：{v}"));
        }
        if let Some(v) = memory.known_issues.as_deref().filter(|s| !s.is_empty()) {
            sections.push(format!("已知问题：{v}"));
        }
        if let Some(v) = memory.resume_position.as_deref().filter(|s| !s.is_empty()) {
            sections.push(format!("恢复位置：{v}"));
        }
        Ok(sections.join("\n"))
    }
}

fn read_memory(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectMemory> {
    Ok(ProjectMemory {
        project_id: row.get(0)?,
        product_goals: row.get(1)?,
        user_requirements: row.get(2)?,
        confirmed_decisions: row.get(3)?,
        open_questions: row.get(4)?,
        deferred_items: row.get(5)?,
        known_issues: row.get(6)?,
        task_history_summary: row.get(7)?,
        current_unfinished_task_id: row.get(8)?,
        resume_position: row.get(9)?,
        last_ai_used: row.get(10)?,
        project_rules_index: row.get(11)?,
        license_summary: row.get(12)?,
        install_run_summary: row.get(13)?,
        updated_at: row.get(14)?,
    })
}

fn read_revision(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectMemoryRevision> {
    Ok(ProjectMemoryRevision {
        id: row.get(0)?,
        project_id: row.get(1)?,
        field_name: row.get(2)?,
        old_value: row.get(3)?,
        new_value: row.get(4)?,
        changed_by: row.get(5)?,
        reason: row.get(6)?,
        created_at: row.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::CreateProjectRequest;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-memory-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_project(dir: &std::path::Path, name: &str) -> (Database, String) {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let project = db
            .create_project(&CreateProjectRequest {
                name: name.to_string(),
                local_path: dir.to_string_lossy().into_owned(),
                source_type: "local".to_string(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap();
        (db, project.id)
    }

    #[test]
    fn a_new_project_starts_with_an_empty_memory_row() {
        let dir = temp_dir("empty-memory");
        let (db, project_id) = db_with_project(&dir, "P");
        let memory = db.project_memory(&project_id).unwrap();
        assert!(memory.product_goals.is_none());
        assert!(memory.confirmed_decisions.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn setting_product_goals_persists_and_is_readable() {
        let dir = temp_dir("goals");
        let (db, project_id) = db_with_project(&dir, "P");
        db.update_project_memory_field(
            &project_id,
            "product_goals",
            Some("做一个任务恢复系统"),
            Actor::User,
            Some("初始设定"),
        )
        .unwrap();
        let memory = db.project_memory(&project_id).unwrap();
        assert_eq!(memory.product_goals.as_deref(), Some("做一个任务恢复系统"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn changing_a_confirmed_decision_creates_a_revision_with_old_and_new_value() {
        let dir = temp_dir("decision-revision");
        let (db, project_id) = db_with_project(&dir, "P");
        db.update_project_memory_field(
            &project_id,
            "confirmed_decisions",
            Some("使用 SQLite"),
            Actor::User,
            None,
        )
        .unwrap();
        db.update_project_memory_field(
            &project_id,
            "confirmed_decisions",
            Some("使用 SQLite + 迁移备份"),
            Actor::User,
            Some("补充了备份要求"),
        )
        .unwrap();

        let revisions = db.list_memory_revisions(&project_id).unwrap();
        assert_eq!(revisions.len(), 2);
        assert_eq!(revisions[1].old_value.as_deref(), Some("使用 SQLite"));
        assert_eq!(
            revisions[1].new_value.as_deref(),
            Some("使用 SQLite + 迁移备份")
        );
        assert_eq!(revisions[1].reason.as_deref(), Some("补充了备份要求"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn writing_the_same_value_again_does_not_create_a_redundant_revision() {
        let dir = temp_dir("no-op-write");
        let (db, project_id) = db_with_project(&dir, "P");
        db.update_project_memory_field(
            &project_id,
            "known_issues",
            Some("issue A"),
            Actor::User,
            None,
        )
        .unwrap();
        db.update_project_memory_field(
            &project_id,
            "known_issues",
            Some("issue A"),
            Actor::User,
            None,
        )
        .unwrap();
        let revisions = db.list_memory_revisions(&project_id).unwrap();
        assert_eq!(revisions.len(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_field_name_is_rejected_not_silently_ignored() {
        let dir = temp_dir("unknown-field");
        let (db, project_id) = db_with_project(&dir, "P");
        let result = db.update_project_memory_field(
            &project_id,
            "chain_of_thought",
            Some("x"),
            Actor::System,
            None,
        );
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn two_projects_memory_never_cross_contaminates() {
        let dir_a = temp_dir("isolation-a");
        let dir_b = temp_dir("isolation-b");
        let (db, project_a) = db_with_project(&dir_a, "A");
        let project_b = db
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
            .unwrap()
            .id;

        db.update_project_memory_field(
            &project_a,
            "product_goals",
            Some("A 的目标"),
            Actor::User,
            None,
        )
        .unwrap();
        db.update_project_memory_field(
            &project_b,
            "product_goals",
            Some("B 的目标"),
            Actor::User,
            None,
        )
        .unwrap();

        let memory_a = db.project_memory(&project_a).unwrap();
        let memory_b = db.project_memory(&project_b).unwrap();
        assert_eq!(memory_a.product_goals.as_deref(), Some("A 的目标"));
        assert_eq!(memory_b.product_goals.as_deref(), Some("B 的目标"));

        let revisions_a = db.list_memory_revisions(&project_a).unwrap();
        assert!(revisions_a.iter().all(|r| r.project_id == project_a));

        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn context_summary_only_contains_this_projects_content() {
        let dir_a = temp_dir("summary-a");
        let dir_b = temp_dir("summary-b");
        let (db, project_a) = db_with_project(&dir_a, "PlanA");
        let project_b = db
            .create_project(&CreateProjectRequest {
                name: "PlanB".to_string(),
                local_path: dir_b.to_string_lossy().into_owned(),
                source_type: "local".to_string(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap()
            .id;
        db.update_project_memory_field(
            &project_a,
            "product_goals",
            Some("秘密目标A-只属于A"),
            Actor::User,
            None,
        )
        .unwrap();
        db.update_project_memory_field(
            &project_b,
            "product_goals",
            Some("秘密目标B-只属于B"),
            Actor::User,
            None,
        )
        .unwrap();

        let summary_a = db.project_context_summary(&project_a).unwrap();
        assert!(summary_a.contains("秘密目标A-只属于A"));
        assert!(!summary_a.contains("秘密目标B-只属于B"));
        assert!(summary_a.contains("PlanA"));
        assert!(!summary_a.contains("PlanB"));

        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }
}
