use std::path::Path;

use rusqlite::{params, OptionalExtension};

use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::models::{CreateProjectRequest, Project, ProjectDirectoryState, ProjectSourceType};

use super::{new_id, now};

const LAST_ACTIVE_PROJECT_SETTING_KEY: &str = "last_active_project_id";

/// Canonicalizes `path` and lowercases the result, so two different-case
/// spellings of the same Windows directory (`C:\Work\Nova` vs
/// `c:\work\nova`) always normalize to the same string. Returns an error if
/// the directory does not exist — a project can only be registered against
/// a real, currently-accessible directory.
pub fn normalize_project_path(path: &Path) -> NovaResult<String> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| NovaError::Config(format!("项目目录不可用：{error}")))?;
    if !canonical.is_dir() {
        return Err(NovaError::Config("项目路径不是目录".to_string()));
    }
    Ok(canonical.to_string_lossy().to_lowercase())
}

impl Database {
    /// Registers a new project. Fails with a clear error (rather than
    /// silently creating a duplicate) if `local_path` normalizes to a
    /// directory that is already registered — see `normalize_project_path`.
    pub fn create_project(&self, request: &CreateProjectRequest) -> NovaResult<Project> {
        let source_type: ProjectSourceType =
            request.source_type.parse().map_err(NovaError::Config)?;
        let normalized_path = normalize_project_path(Path::new(&request.local_path))?;
        if self
            .find_project_by_normalized_path(&normalized_path)?
            .is_some()
        {
            return Err(NovaError::Config(
                "该目录已注册为一个 Nova 项目，不能重复注册".to_string(),
            ));
        }

        let id = new_id();
        let now = now();

        // The project row and its one-per-project memory row must exist
        // together or not at all: if the memory insert failed after the
        // project insert already committed, the database would carry a
        // half-created project that every other repository function
        // assumes can never happen (see the comment below). Both writes
        // therefore happen inside one transaction, rolled back in full on
        // either failure.
        self.conn().execute("BEGIN IMMEDIATE;", [])?;
        let result = (|| -> NovaResult<()> {
            self.conn().execute(
                "INSERT INTO nova_projects (
                    id, name, local_path, normalized_path, source_type, source_url,
                    current_branch, last_head_summary, purpose, long_term_goal,
                    user_preferences, prohibitions, last_activity_at, created_at,
                    updated_at, last_opened_at, data_format_version
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 1)",
                params![
                    id,
                    request.name,
                    request.local_path,
                    normalized_path,
                    source_type.as_str(),
                    request.source_url,
                    request.purpose,
                    request.long_term_goal,
                    request.user_preferences,
                    request.prohibitions,
                    now,
                    now,
                    now,
                    now,
                ],
            )?;
            // A newly-created project's memory row is established up front
            // so every other repository can assume `project_memory` always
            // has exactly one row per project.
            self.conn().execute(
                "INSERT INTO project_memory (project_id, updated_at) VALUES (?1, ?2)",
                params![id, now],
            )?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn().execute("COMMIT;", [])?;
                self.project(&id)
            }
            Err(error) => {
                let _ = self.conn().execute("ROLLBACK;", []);
                Err(error)
            }
        }
    }

    pub fn project(&self, id: &str) -> NovaResult<Project> {
        let last_active = self.last_active_project_id()?;
        self.conn()
            .query_row(
                "SELECT id, name, local_path, normalized_path, source_type, source_url,
                        current_branch, last_head_summary, purpose, long_term_goal,
                        user_preferences, prohibitions, last_activity_at, created_at,
                        updated_at, last_opened_at, data_format_version
                 FROM nova_projects WHERE id = ?1",
                params![id],
                |row| read_project(row, last_active.as_deref()),
            )
            .map_err(Into::into)
    }

    pub fn find_project_by_normalized_path(
        &self,
        normalized_path: &str,
    ) -> NovaResult<Option<Project>> {
        let last_active = self.last_active_project_id()?;
        self.conn()
            .query_row(
                "SELECT id, name, local_path, normalized_path, source_type, source_url,
                        current_branch, last_head_summary, purpose, long_term_goal,
                        user_preferences, prohibitions, last_activity_at, created_at,
                        updated_at, last_opened_at, data_format_version
                 FROM nova_projects WHERE normalized_path = ?1",
                params![normalized_path],
                |row| read_project(row, last_active.as_deref()),
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn list_projects(&self) -> NovaResult<Vec<Project>> {
        let last_active = self.last_active_project_id()?;
        let mut stmt = self.conn().prepare(
            "SELECT id, name, local_path, normalized_path, source_type, source_url,
                    current_branch, last_head_summary, purpose, long_term_goal,
                    user_preferences, prohibitions, last_activity_at, created_at,
                    updated_at, last_opened_at, data_format_version
             FROM nova_projects ORDER BY last_activity_at DESC",
        )?;
        let rows = stmt
            .query_map([], |row| read_project(row, last_active.as_deref()))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Marks `project_id` as the last-active project (single source of
    /// truth in `app_settings`, see [`Project::is_last_active`]) and
    /// refreshes its `last_opened_at`/`last_activity_at`/`updated_at`.
    /// Fails if the project does not exist, so a stale/deleted ID can never
    /// silently become "last active".
    /// Refreshes the project's activity timestamps and moves the single
    /// last-active pointer, atomically: if the `app_settings` write were to
    /// fail after the timestamp `UPDATE` had already committed, the project
    /// would look freshly touched without actually being last-active,
    /// which is exactly the drift this transaction rules out.
    pub fn set_last_active_project(&self, project_id: &str) -> NovaResult<Project> {
        let now = now();
        self.conn().execute("BEGIN IMMEDIATE;", [])?;
        let result = (|| -> NovaResult<()> {
            let updated = self.conn().execute(
                "UPDATE nova_projects SET last_activity_at = ?1, updated_at = ?1, last_opened_at = ?1 WHERE id = ?2",
                params![now, project_id],
            )?;
            if updated == 0 {
                return Err(NovaError::Config("项目不存在".to_string()));
            }
            self.conn().execute(
                "INSERT INTO app_settings (key, value, updated_at) VALUES (?1, ?2, ?3)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                params![LAST_ACTIVE_PROJECT_SETTING_KEY, project_id, now],
            )?;
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.conn().execute("COMMIT;", [])?;
                self.project(project_id)
            }
            Err(error) => {
                let _ = self.conn().execute("ROLLBACK;", []);
                Err(error)
            }
        }
    }

    pub fn last_active_project_id(&self) -> NovaResult<Option<String>> {
        self.get_setting(LAST_ACTIVE_PROJECT_SETTING_KEY)
            .map_err(Into::into)
    }

    /// The last-active project, if one is set and its row still exists.
    /// Returns `Ok(None)` rather than an error when the pointer is stale
    /// (project deleted) — callers should treat that the same as "no
    /// project has ever been opened", never crash.
    pub fn last_active_project(&self) -> NovaResult<Option<Project>> {
        match self.last_active_project_id()? {
            Some(id) => {
                let last_active = Some(id.clone());
                self.conn()
                    .query_row(
                        "SELECT id, name, local_path, normalized_path, source_type, source_url,
                                current_branch, last_head_summary, purpose, long_term_goal,
                                user_preferences, prohibitions, last_activity_at, created_at,
                                updated_at, last_opened_at, data_format_version
                         FROM nova_projects WHERE id = ?1",
                        params![id],
                        |row| read_project(row, last_active.as_deref()),
                    )
                    .optional()
                    .map_err(Into::into)
            }
            None => Ok(None),
        }
    }
}

/// Checks whether a project's registered directory still exists, without
/// creating, writing, or deleting anything. Used both right after startup
/// recovery and whenever the UI opens a project, to give a safe status
/// instead of assuming the directory is still there.
pub fn check_project_directory_state(project: &Project) -> ProjectDirectoryState {
    let path = Path::new(&project.local_path);
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => ProjectDirectoryState::Ok,
        Ok(_) => ProjectDirectoryState::NotADirectory,
        Err(_) => ProjectDirectoryState::Missing,
    }
}

fn read_project(
    row: &rusqlite::Row<'_>,
    last_active_id: Option<&str>,
) -> rusqlite::Result<Project> {
    let id: String = row.get(0)?;
    let is_last_active = last_active_id.is_some_and(|active| active == id);
    Ok(Project {
        id,
        name: row.get(1)?,
        local_path: row.get(2)?,
        normalized_path: row.get(3)?,
        source_type: row.get(4)?,
        source_url: row.get(5)?,
        current_branch: row.get(6)?,
        last_head_summary: row.get(7)?,
        purpose: row.get(8)?,
        long_term_goal: row.get(9)?,
        user_preferences: row.get(10)?,
        prohibitions: row.get(11)?,
        last_activity_at: row.get(12)?,
        created_at: row.get(13)?,
        updated_at: row.get(14)?,
        last_opened_at: row.get(15)?,
        is_last_active,
        data_format_version: row.get(16)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Database;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-projects-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db() -> Database {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        db
    }

    fn create_req(local_path: &Path, name: &str) -> CreateProjectRequest {
        CreateProjectRequest {
            name: name.to_string(),
            local_path: local_path.to_string_lossy().into_owned(),
            source_type: "local".to_string(),
            source_url: None,
            purpose: Some("测试用途".to_string()),
            long_term_goal: Some("长期目标".to_string()),
            user_preferences: None,
            prohibitions: None,
        }
    }

    #[test]
    fn creates_and_reads_back_a_project() {
        let db = db();
        let dir = temp_dir("create");
        let project = db.create_project(&create_req(&dir, "Demo")).unwrap();
        assert_eq!(project.name, "Demo");
        assert_eq!(project.data_format_version, 1);
        assert!(!project.is_last_active);
        let fetched = db.project(&project.id).unwrap();
        assert_eq!(fetched, project);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn creating_a_project_also_creates_exactly_one_memory_row_in_the_same_transaction() {
        let db = db();
        let dir = temp_dir("create-with-memory");
        let project = db.create_project(&create_req(&dir, "Demo")).unwrap();

        let memory_rows: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM project_memory WHERE project_id = ?1",
                params![project.id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(memory_rows, 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_memory_insert_leaves_no_half_created_project_behind() {
        let db = db();
        let dir = temp_dir("atomic-failure");
        // Force the second insert (`project_memory`) to fail deterministically
        // by dropping its table first, while `nova_projects` remains intact
        // — this proves the whole `create_project` call rolls back as one
        // unit rather than leaving a project row with no memory row.
        db.conn()
            .execute_batch("DROP TABLE project_memory;")
            .unwrap();

        let result = db.create_project(&create_req(&dir, "WillFail"));
        assert!(
            result.is_err(),
            "create_project must fail when the memory insert fails"
        );

        let project_rows: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM nova_projects", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            project_rows, 0,
            "a failed create_project must not leave a half-created nova_projects row behind"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_duplicate_or_constraint_conflicting_project_leaves_no_half_created_row() {
        let db = db();
        let dir = temp_dir("duplicate-atomic");
        db.create_project(&create_req(&dir, "First")).unwrap();

        let project_count_before: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM nova_projects", [], |row| row.get(0))
            .unwrap();
        let memory_count_before: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM project_memory", [], |row| row.get(0))
            .unwrap();

        let result = db.create_project(&create_req(&dir, "Second-different-name"));
        assert!(result.is_err());

        let project_count_after: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM nova_projects", [], |row| row.get(0))
            .unwrap();
        let memory_count_after: i64 = db
            .conn()
            .query_row("SELECT COUNT(*) FROM project_memory", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            project_count_before, project_count_after,
            "a rejected duplicate must not add a project row"
        );
        assert_eq!(
            memory_count_before, memory_count_after,
            "a rejected duplicate must not add a memory row"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_same_normalized_directory_cannot_be_registered_twice() {
        let db = db();
        let dir = temp_dir("dup");
        db.create_project(&create_req(&dir, "First")).unwrap();
        let second = db.create_project(&create_req(&dir, "Second-different-name"));
        assert!(
            second.is_err(),
            "duplicate directory registration must be rejected"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn registering_the_same_directory_with_different_case_is_still_a_duplicate() {
        let db = db();
        let dir = temp_dir("Case-Test-DIR");
        db.create_project(&create_req(&dir, "First")).unwrap();

        let mut upper = dir.clone();
        let file_name = upper.file_name().unwrap().to_string_lossy().to_uppercase();
        upper.pop();
        upper.push(file_name);

        let result = db.create_project(&create_req(&upper, "Second"));
        assert!(
            result.is_err(),
            "Windows path comparison must be case-insensitive: {dir:?} vs {upper:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn chinese_and_space_containing_paths_can_be_registered() {
        let db = db();
        let dir = temp_dir("中文 项目 空格 test");
        let project = db.create_project(&create_req(&dir, "中文项目")).unwrap();
        assert_eq!(project.name, "中文项目");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn set_last_active_project_is_atomic_with_the_app_settings_write() {
        let db = db();
        let dir = temp_dir("last-active-atomic");
        let project = db.create_project(&create_req(&dir, "P")).unwrap();
        let before = db.project(&project.id).unwrap();

        // Force the `app_settings` half of the write to fail
        // deterministically, while the `nova_projects` timestamp update
        // would otherwise have succeeded on its own.
        db.conn().execute_batch("DROP TABLE app_settings;").unwrap();

        let result = db.set_last_active_project(&project.id);
        assert!(result.is_err());

        // The failure must not have left the timestamps updated without
        // the pointer actually moving.
        db.conn()
            .execute_batch(
                "CREATE TABLE app_settings (key TEXT PRIMARY KEY, value TEXT NOT NULL, updated_at TEXT NOT NULL);",
            )
            .unwrap();
        let after = db.project(&project.id).unwrap();
        assert_eq!(
            before.last_activity_at, after.last_activity_at,
            "a failed app_settings write must roll back the timestamp update too"
        );
        assert_eq!(before.last_opened_at, after.last_opened_at);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn last_active_project_can_be_set_and_read_back() {
        let db = db();
        let dir_a = temp_dir("last-active-a");
        let dir_b = temp_dir("last-active-b");
        let a = db.create_project(&create_req(&dir_a, "A")).unwrap();
        let b = db.create_project(&create_req(&dir_b, "B")).unwrap();

        assert!(db.last_active_project().unwrap().is_none());

        db.set_last_active_project(&a.id).unwrap();
        let active = db.last_active_project().unwrap().unwrap();
        assert_eq!(active.id, a.id);
        assert!(active.is_last_active);

        let listed = db.list_projects().unwrap();
        let a_listed = listed.iter().find(|p| p.id == a.id).unwrap();
        let b_listed = listed.iter().find(|p| p.id == b.id).unwrap();
        assert!(a_listed.is_last_active);
        assert!(!b_listed.is_last_active);

        db.set_last_active_project(&b.id).unwrap();
        let active = db.last_active_project().unwrap().unwrap();
        assert_eq!(
            active.id, b.id,
            "switching projects must move the single pointer, not add a second one"
        );

        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
    }

    #[test]
    fn missing_project_directory_is_reported_safely() {
        let db = db();
        let dir = temp_dir("will-vanish");
        let project = db.create_project(&create_req(&dir, "Vanishing")).unwrap();
        fs::remove_dir_all(&dir).unwrap();

        let state = check_project_directory_state(&project);
        assert_eq!(state, ProjectDirectoryState::Missing);
    }

    #[test]
    fn moved_project_directory_is_also_reported_as_missing_not_a_crash() {
        let db = db();
        let dir = temp_dir("will-move-src");
        let moved_to = temp_dir("will-move-dst");
        fs::remove_dir_all(&moved_to).unwrap();
        let project = db.create_project(&create_req(&dir, "Movable")).unwrap();

        fs::rename(&dir, &moved_to).unwrap();

        let state = check_project_directory_state(&project);
        assert_eq!(
            state,
            ProjectDirectoryState::Missing,
            "the registered path no longer resolves; Nova must not guess where it went"
        );
        let _ = fs::remove_dir_all(&moved_to);
    }

    #[test]
    fn unknown_source_type_is_rejected() {
        let db = db();
        let dir = temp_dir("bad-source-type");
        let mut req = create_req(&dir, "Bad");
        req.source_type = "ftp".to_string();
        let result = db.create_project(&req);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stale_last_active_pointer_to_a_deleted_project_returns_none_not_an_error() {
        let db = db();
        let dir = temp_dir("stale-pointer");
        let project = db.create_project(&create_req(&dir, "ToDelete")).unwrap();
        db.set_last_active_project(&project.id).unwrap();
        db.conn()
            .execute(
                "DELETE FROM nova_projects WHERE id = ?1",
                params![project.id],
            )
            .unwrap();

        let active = db.last_active_project().unwrap();
        assert!(active.is_none());
        let _ = fs::remove_dir_all(&dir);
    }
}
