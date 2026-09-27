//! Phase 4 (evidence and document gate): a safe, read-only listing and
//! reading path for a task's seven result documents (see
//! `core_loop::documents`, which generates them). Deliberately narrow:
//!
//! - The only "path" a caller may ever supply is one of seven fixed
//!   `document_type` tags (below) — never a file name, never a relative or
//!   absolute path, never anything that reaches the filesystem unvalidated.
//! - The on-disk directory is (re)computed via
//!   [`crate::app_paths::safe_app_owned_dir`] — the exact same helper
//!   [`super::documents::generate_result_documents`] itself uses — so this
//!   inherits the same component sanitization, symlink-ancestor rejection,
//!   and canonical-containment check, rather than re-implementing (and
//!   risking diverging from) that logic.
//! - Project/task ownership is checked first, before any filesystem access,
//!   exactly like every other function in this codebase that reads
//!   task-scoped data.
//! - Document content is re-redacted and length-capped before being handed
//!   to the frontend — defense in depth on top of the redaction/capping
//!   `core_loop`'s evidence-recording functions already apply when the
//!   underlying facts were first written.

use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::app_paths;
use crate::db::Database;
use crate::error::{NovaError, NovaResult};
use crate::security::redact_sensitive_text;

/// Hard cap on document content returned to the frontend — these are
/// Nova-generated summaries, not raw logs, so real content is always far
/// smaller than this; the cap exists purely as a defensive backstop.
const MAX_DOCUMENT_LEN: usize = 200_000;

/// `(document_type, display_name, file_name, is_optional_diagnostic)` — the
/// one and only allowlist. `file_name` is a fixed string literal everywhere
/// it is used below, never influenced by any caller input.
const DOCUMENT_ALLOWLIST: &[(&str, &str, &str, bool)] = &[
    ("REQUIREMENT", "需求确认单", "REQUIREMENT.md", false),
    (
        "DEVELOPMENT_REPORT",
        "开发报告",
        "DEVELOPMENT_REPORT.md",
        false,
    ),
    ("TEST_REPORT", "测试报告", "TEST_REPORT.md", false),
    (
        "FIRST_REVIEW",
        "第一次审核记录（可选诊断，v0.2.0 双 AI 审核能力，不影响本次结果）",
        "FIRST_REVIEW.md",
        true,
    ),
    ("FIX_REPORT", "修复报告", "FIX_REPORT.md", false),
    (
        "SECOND_REVIEW",
        "第二次审核记录（可选诊断，v0.2.0 双 AI 审核能力，不影响本次结果）",
        "SECOND_REVIEW.md",
        true,
    ),
    ("FINAL_RESULT", "最终结果", "FINAL_RESULT.md", false),
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDocumentInfo {
    pub document_type: String,
    pub display_name: String,
    pub exists: bool,
    pub is_optional_diagnostic: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskDocumentContent {
    pub document_type: String,
    pub display_name: String,
    pub exists: bool,
    pub content: Option<String>,
    /// Set only when the document is expected to exist (or its existence
    /// could not be determined) but reading it failed for a reason other
    /// than "not generated yet" — e.g. a permissions error. Never contains
    /// a raw OS error message (which could include a full filesystem
    /// path); always a fixed, user-safe phrase.
    pub error: Option<String>,
}

fn assert_ownership(db: &Database, project_id: &str, task_id: &str) -> NovaResult<()> {
    let task = db.task(task_id)?;
    if task.project_id != project_id {
        return Err(NovaError::Config(
            "project_id 与任务所属项目不一致，已拒绝读取".to_string(),
        ));
    }
    Ok(())
}

/// Lists all seven documents' display metadata for one task — never their
/// content (see [`read_task_document`] for that). Ownership is checked
/// before anything else; a wrong `project_id` gets a plain error, never a
/// hint about whether `task_id` exists at all.
pub fn list_task_documents(
    db: &Database,
    project_id: &str,
    task_id: &str,
    app_data_dir: &Path,
) -> NovaResult<Vec<TaskDocumentInfo>> {
    assert_ownership(db, project_id, task_id)?;
    let dir = app_paths::safe_app_owned_dir(
        &app_data_dir.join("result-documents"),
        &[project_id, task_id],
    )
    .map_err(NovaError::from)?;

    Ok(DOCUMENT_ALLOWLIST
        .iter()
        .map(
            |(document_type, display_name, file_name, is_optional_diagnostic)| TaskDocumentInfo {
                document_type: (*document_type).to_string(),
                display_name: (*display_name).to_string(),
                exists: dir.join(*file_name).is_file(),
                is_optional_diagnostic: *is_optional_diagnostic,
            },
        )
        .collect())
}

/// Reads exactly one document's redacted, length-capped content.
/// `document_type` must be one of [`DOCUMENT_ALLOWLIST`]'s exact tags —
/// anything else is rejected before any filesystem access, never treated
/// as a path. A document that has not been generated yet is not an error:
/// `exists: false`, `content: None`, `error: None`.
pub fn read_task_document(
    db: &Database,
    project_id: &str,
    task_id: &str,
    document_type: &str,
    app_data_dir: &Path,
) -> NovaResult<TaskDocumentContent> {
    assert_ownership(db, project_id, task_id)?;
    let (document_type, display_name, file_name, _) = DOCUMENT_ALLOWLIST
        .iter()
        .find(|(candidate, _, _, _)| *candidate == document_type)
        .copied()
        .ok_or_else(|| NovaError::Config(format!("未知的文档类型：{document_type}")))?;

    let dir = app_paths::safe_app_owned_dir(
        &app_data_dir.join("result-documents"),
        &[project_id, task_id],
    )
    .map_err(NovaError::from)?;
    let path = dir.join(file_name);

    match fs::read_to_string(&path) {
        Ok(content) => {
            let capped = cap_text(&content, MAX_DOCUMENT_LEN);
            Ok(TaskDocumentContent {
                document_type: document_type.to_string(),
                display_name: display_name.to_string(),
                exists: true,
                content: Some(redact_sensitive_text(&capped)),
                error: None,
            })
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(TaskDocumentContent {
            document_type: document_type.to_string(),
            display_name: display_name.to_string(),
            exists: false,
            content: None,
            error: None,
        }),
        Err(_) => Ok(TaskDocumentContent {
            document_type: document_type.to_string(),
            display_name: display_name.to_string(),
            exists: false,
            content: None,
            error: Some("文档读取失败".to_string()),
        }),
    }
}

fn cap_text(text: &str, max_len: usize) -> String {
    if text.len() <= max_len {
        return text.to_string();
    }
    let mut end = max_len;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... (截断)", &text[..end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core_loop::documents::generate_result_documents;
    use crate::models::{Actor, CreateProjectRequest, CreateTaskRequest};
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-task-documents-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn db_with_task(dir: &Path) -> (Database, String, String) {
        let db = Database::memory().unwrap();
        db.migrate().unwrap();
        let project = db
            .create_project(&CreateProjectRequest {
                name: "P".to_string(),
                local_path: dir.to_string_lossy().into_owned(),
                source_type: "local".to_string(),
                source_url: None,
                purpose: None,
                long_term_goal: None,
                user_preferences: None,
                prohibitions: None,
            })
            .unwrap();
        let task = db
            .create_task(
                &CreateTaskRequest {
                    project_id: project.id.clone(),
                    title: "T".to_string(),
                    goal: Some("goal text".to_string()),
                    in_scope: None,
                    not_in_scope: None,
                    done_criteria: None,
                    ai_kind: Some("codex_cli".to_string()),
                },
                Actor::User,
            )
            .unwrap();
        (db, project.id, task.id)
    }

    #[test]
    fn lists_all_seven_documents_with_real_existence_flags() {
        let dir = temp_dir("list-seven");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("list-seven-appdata");
        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let docs = list_task_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        assert_eq!(docs.len(), 7);
        assert!(docs.iter().all(|d| d.exists));
        assert!(docs
            .iter()
            .filter(|d| d.is_optional_diagnostic)
            .map(|d| d.document_type.as_str())
            .eq(["FIRST_REVIEW", "SECOND_REVIEW"]));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn lists_documents_as_not_existing_before_any_generation() {
        let dir = temp_dir("list-not-generated");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("list-not-generated-appdata");

        let docs = list_task_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();
        assert_eq!(docs.len(), 7);
        assert!(docs.iter().all(|d| !d.exists));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn reads_real_generated_content_for_a_known_document_type() {
        let dir = temp_dir("read-real");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("read-real-appdata");
        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let content =
            read_task_document(&db, &project_id, &task_id, "REQUIREMENT", &app_data_dir).unwrap();
        assert!(content.exists);
        assert!(content.error.is_none());
        assert!(content.content.unwrap().contains("goal text"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn reading_a_document_that_was_never_generated_is_not_an_error() {
        let dir = temp_dir("read-not-generated");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("read-not-generated-appdata");

        let content =
            read_task_document(&db, &project_id, &task_id, "FINAL_RESULT", &app_data_dir).unwrap();
        assert!(!content.exists);
        assert!(content.content.is_none());
        assert!(content.error.is_none());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn reading_an_unknown_document_type_is_rejected_before_any_filesystem_access() {
        let dir = temp_dir("read-unknown-type");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("read-unknown-type-appdata");

        let result = read_task_document(
            &db,
            &project_id,
            &task_id,
            "../../etc/passwd",
            &app_data_dir,
        );
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn a_traversal_style_document_type_never_escapes_the_app_owned_root() {
        let dir = temp_dir("traversal-attempt");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("traversal-attempt-appdata");
        // A secret file sitting one level above the app-owned root — must
        // never become readable through this command no matter what
        // `document_type` string is supplied.
        fs::write(app_data_dir.join("secret.txt"), "top secret").unwrap();

        for attempt in ["..", "../secret.txt", "..\\secret.txt", "FINAL_RESULT.md"] {
            let result =
                read_task_document(&db, &project_id, &task_id, attempt, &app_data_dir).unwrap_err();
            let _ = result; // every one of these must be rejected as "unknown document type"
        }
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn listing_documents_rejects_a_project_id_that_does_not_own_the_task() {
        let dir_a = temp_dir("list-wrong-project-a");
        let dir_b = temp_dir("list-wrong-project-b");
        let (db, _project_a, task_a) = db_with_task(&dir_a);
        let (_db_unused, project_b, _task_b) = db_with_task(&dir_b);
        let app_data_dir = temp_dir("list-wrong-project-appdata");

        let result = list_task_documents(&db, &project_b, &task_a, &app_data_dir);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn reading_a_document_rejects_a_project_id_that_does_not_own_the_task() {
        let dir_a = temp_dir("read-wrong-project-a");
        let dir_b = temp_dir("read-wrong-project-b");
        let (db, project_a, task_a) = db_with_task(&dir_a);
        let (_db_unused, project_b, _task_b) = db_with_task(&dir_b);
        let app_data_dir = temp_dir("read-wrong-project-appdata");
        generate_result_documents(&db, &project_a, &task_a, &app_data_dir).unwrap();

        let result = read_task_document(&db, &project_b, &task_a, "REQUIREMENT", &app_data_dir);
        assert!(
            result.is_err(),
            "a project_id that does not own task_a must never be able to read its documents"
        );
        let _ = fs::remove_dir_all(&dir_a);
        let _ = fs::remove_dir_all(&dir_b);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn document_content_is_redacted_even_though_the_underlying_evidence_was_already_redacted() {
        let dir = temp_dir("read-redacted");
        let (db, project_id, task_id) = db_with_task(&dir);
        let app_data_dir = temp_dir("read-redacted-appdata");
        db.record_test_evidence(&crate::storage::evidence::RecordTestEvidenceRequest {
            project_id: &project_id,
            task_id: &task_id,
            result_generation: 0,
            round: 1,
            test_command_redacted: Some("cargo test"),
            status: "failed",
            passed_count: 0,
            failed_count: 1,
            skipped_count: 0,
            exit_code: Some(1),
            reason: Some("token=abc123supersecretvalue"),
            stdout_ref: None,
            stderr_ref: None,
        })
        .unwrap();
        generate_result_documents(&db, &project_id, &task_id, &app_data_dir).unwrap();

        let content =
            read_task_document(&db, &project_id, &task_id, "TEST_REPORT", &app_data_dir).unwrap();
        assert!(!content.content.unwrap().contains("abc123supersecretvalue"));
        let _ = fs::remove_dir_all(&dir);
        let _ = fs::remove_dir_all(&app_data_dir);
    }
}
