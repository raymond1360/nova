//! Phase 3 remediation (Blocker 1): technical, filesystem-level read-only
//! enforcement for the reviewer AI.
//!
//! The original Phase 3 implementation ran the reviewer directly inside the
//! user's real project directory and detected a violation only *after the
//! fact*, by diffing `git status`/`HEAD` before and after the reviewer ran
//! (still done today — see `core_loop::run_one_round` — but now only as a
//! supplementary audit signal, never the primary safety boundary). Codex's
//! review correctly flagged that as "not technical enforcement": a reviewer
//! could still have modified, exfiltrated, or deleted files *outside* the
//! git-tracked set (ignored files, `.env`, anything above/beside the
//! project root) and the git-diff check would never see it.
//!
//! The fix here is physical, not behavioral: before the reviewer AI is ever
//! invoked, Nova copies the project's working tree (excluding `.git`, never
//! following a symlink/junction encountered inside it) into a disposable,
//! app-owned directory, and the reviewer's `cwd`/argv are pointed at *that
//! copy* — never at the real project path, which is never passed to the
//! adapter in any form (not cwd, not an argument, not file content). Even a
//! fully compromised or malicious reviewer process, with unrestricted write
//! access to its own `cwd`, is physically incapable of touching the original
//! project: it does not have the path.
//!
//! This lives in its own app-owned root (`review-workspaces/`), a sibling
//! of — and always distinct from — `task-packages/` (`core_loop.rs`'s
//! `task_docs_dir`), per the remediation requirement that the two never
//! share a directory.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::app_paths;

/// A prepared, disposable reviewer workspace. `root` is the whole per-round
/// directory (safe to hand to [`cleanup`] afterward); `source_dir` is the
/// project tree copy the reviewer should actually be pointed at as its
/// `cwd`.
pub struct ReviewWorkspace {
    pub root: PathBuf,
    pub source_dir: PathBuf,
}

/// Directory/file names never copied into a review workspace, regardless of
/// depth. `.git` specifically: the reviewer has no legitimate need to write
/// git history, and copying it would be needless size/time for a read-only
/// code review.
const EXCLUDED_NAMES: &[&str] = &[".git"];

/// Builds `<app_data_dir>/review-workspaces/<project_id>/<task_id>/round-<n>/<uuid>/`
/// and copies `project_dir`'s tree into `<...>/source/`.
///
/// `project_id`/`task_id` are validated single path components exactly like
/// `core_loop::task_docs_dir` (via `app_paths::safe_app_owned_dir` — rejects
/// `..`, separators, drive/UNC prefixes, reserved device names, trailing
/// dot/space, and any symlink/junction ancestor). The trailing UUID
/// component means a crash-recovery re-run of the same round never reuses
/// (and never needs to worry about partially-populated) an earlier attempt's
/// workspace.
pub fn prepare(
    app_data_dir: &Path,
    project_id: &str,
    task_id: &str,
    round: i64,
    project_dir: &Path,
) -> io::Result<ReviewWorkspace> {
    let round_component = format!("round-{round}");
    let run_component = uuid::Uuid::new_v4().to_string();
    let root = app_paths::safe_app_owned_dir(
        &app_data_dir.join("review-workspaces"),
        &[project_id, task_id, &round_component, &run_component],
    )?;
    let source_dir = root.join("source");
    fs::create_dir_all(&source_dir)?;
    copy_tree_no_symlinks(project_dir, &source_dir)?;
    Ok(ReviewWorkspace { root, source_dir })
}

/// Best-effort cleanup, called once the round using this workspace has
/// finished (success, needs-fix, or integrity violation). Never panics and
/// never touches anything outside `<app_data_dir>/review-workspaces/` — a
/// caller that (by a bug) passed the wrong `root` gets a hard `Err` from
/// `safe_remove_dir_all`'s containment check rather than a deletion outside
/// the expected tree. If Nova's process exits before this runs (crash, kill,
/// power loss), the workspace is simply left on disk for later manual/
/// startup cleanup — it is disposable, app-owned content, never anything
/// that could affect the user's project.
pub fn cleanup(app_data_dir: &Path, workspace_root: &Path) -> io::Result<()> {
    app_paths::safe_remove_dir_all(&app_data_dir.join("review-workspaces"), workspace_root)
}

/// Recursively copies `src` into `dst` (which must already exist), skipping
/// [`EXCLUDED_NAMES`] and — critically — skipping (never following) any
/// symlink or junction/reparse-point entry encountered anywhere in the
/// source tree. A malicious or merely careless prior commit could otherwise
/// have planted a symlink pointing outside the project; naively following it
/// during the copy would leak content the reviewer was never meant to see
/// (or, worse, hand the reviewer a real path back out of the isolated copy).
fn copy_tree_no_symlinks(src: &Path, dst: &Path) -> io::Result<()> {
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if EXCLUDED_NAMES
            .iter()
            .any(|excluded| name.to_string_lossy() == *excluded)
        {
            continue;
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            // Deliberately skipped, not followed and not copied as a link.
            continue;
        }
        let src_path = entry.path();
        let dst_path = dst.join(&name);
        if file_type.is_dir() {
            fs::create_dir_all(&dst_path)?;
            copy_tree_no_symlinks(&src_path, &dst_path)?;
        } else if file_type.is_file() {
            fs::copy(&src_path, &dst_path)?;
        }
        // Anything else (device files, etc.) is not a realistic case on a
        // user's project directory and is silently skipped rather than
        // failing the whole copy.
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as StdCommand;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-review-isolation-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn copies_ordinary_files_and_nested_directories() {
        let project = temp_dir("copy-basic-project");
        fs::write(project.join("a.txt"), b"hello").unwrap();
        fs::create_dir_all(project.join("sub")).unwrap();
        fs::write(project.join("sub").join("b.txt"), b"world").unwrap();
        let app_data_dir = temp_dir("copy-basic-appdata");

        let workspace = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();

        assert_eq!(
            fs::read_to_string(workspace.source_dir.join("a.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(workspace.source_dir.join("sub").join("b.txt")).unwrap(),
            "world"
        );
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn excludes_the_dot_git_directory() {
        let project = temp_dir("copy-excludes-git-project");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::write(project.join(".git").join("config"), b"secret git internals").unwrap();
        fs::write(project.join("README.md"), b"hi").unwrap();
        let app_data_dir = temp_dir("copy-excludes-git-appdata");

        let workspace = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();

        assert!(!workspace.source_dir.join(".git").exists());
        assert!(workspace.source_dir.join("README.md").exists());
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn the_isolated_copy_is_outside_the_original_project_directory() {
        let project = temp_dir("copy-outside-project");
        fs::write(project.join("a.txt"), b"hi").unwrap();
        let app_data_dir = temp_dir("copy-outside-appdata");

        let workspace = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();

        assert!(!workspace.source_dir.starts_with(&project));
        assert!(workspace
            .source_dir
            .starts_with(fs::canonicalize(&app_data_dir).unwrap()));
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn writing_into_the_isolated_copy_never_touches_the_original_project() {
        // The core "technical read-only" proof: even a write that *does*
        // happen (simulating a hostile/compromised reviewer with full write
        // access to its own cwd) lands only in the disposable copy.
        let project = temp_dir("copy-write-safety-project");
        fs::write(project.join("original.txt"), b"untouched").unwrap();
        let app_data_dir = temp_dir("copy-write-safety-appdata");

        let workspace = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();
        fs::write(
            workspace.source_dir.join("reviewer_planted_this.txt"),
            b"malicious",
        )
        .unwrap();
        fs::write(workspace.source_dir.join("original.txt"), b"tampered").unwrap();

        assert!(!project.join("reviewer_planted_this.txt").exists());
        assert_eq!(
            fs::read_to_string(project.join("original.txt")).unwrap(),
            "untouched",
            "a write inside the isolated copy must never reach the real project file"
        );
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn deleting_files_in_the_isolated_copy_never_touches_the_original_project() {
        let project = temp_dir("copy-delete-safety-project");
        fs::write(project.join("keep.txt"), b"still here").unwrap();
        let app_data_dir = temp_dir("copy-delete-safety-appdata");

        let workspace = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();
        fs::remove_file(workspace.source_dir.join("keep.txt")).unwrap();

        assert!(
            project.join("keep.txt").exists(),
            "deleting the copy's file must never delete the original"
        );
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn running_git_commit_inside_the_isolated_copy_never_touches_the_original_repository() {
        let project = temp_dir("copy-git-safety-project");
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "test@example.com"],
            vec!["config", "user.name", "Nova Test"],
        ] {
            StdCommand::new("git")
                .args(&args)
                .current_dir(&project)
                .status()
                .unwrap();
        }
        fs::write(project.join("README.md"), "hello").unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&project)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(&project)
            .status()
            .unwrap();
        let original_head = StdCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&project)
            .output()
            .unwrap()
            .stdout;

        let app_data_dir = temp_dir("copy-git-safety-appdata");
        let workspace = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();
        // The isolated copy excludes `.git`, so re-initializing a throwaway
        // repo inside it and committing there is itself proof the operation
        // cannot reach the real repository at all.
        StdCommand::new("git")
            .args(["init", "-q"])
            .current_dir(&workspace.source_dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.email", "test@example.com"])
            .current_dir(&workspace.source_dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["config", "user.name", "Nova Test"])
            .current_dir(&workspace.source_dir)
            .status()
            .unwrap();
        fs::write(workspace.source_dir.join("README.md"), "tampered").unwrap();
        StdCommand::new("git")
            .args(["add", "."])
            .current_dir(&workspace.source_dir)
            .status()
            .unwrap();
        StdCommand::new("git")
            .args(["commit", "-q", "-m", "reviewer commit"])
            .current_dir(&workspace.source_dir)
            .status()
            .unwrap();

        let head_after = StdCommand::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&project)
            .output()
            .unwrap()
            .stdout;
        assert_eq!(
            original_head, head_after,
            "a git commit inside the isolated copy must never change the real project's HEAD"
        );
        assert_eq!(
            fs::read_to_string(project.join("README.md")).unwrap(),
            "hello",
            "the real project's tracked file content must be untouched"
        );
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    #[cfg(windows)]
    fn a_symlink_inside_the_project_is_skipped_not_followed() {
        let project = temp_dir("copy-symlink-skip-project");
        let outside = temp_dir("copy-symlink-skip-outside");
        fs::write(outside.join("secret.txt"), b"outside content").unwrap();
        let link_path = project.join("planted-link");
        let status = StdCommand::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                link_path.to_str().unwrap(),
                outside.to_str().unwrap(),
            ])
            .status()
            .expect("run mklink");
        assert!(status.success(), "test setup: mklink /J must succeed");
        fs::write(project.join("real.txt"), b"real content").unwrap();

        let app_data_dir = temp_dir("copy-symlink-skip-appdata");
        let workspace = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();

        assert!(
            !workspace.source_dir.join("planted-link").exists(),
            "a symlink/junction inside the project must be skipped, never followed into the copy"
        );
        assert!(workspace.source_dir.join("real.txt").exists());
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&outside);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn each_round_gets_its_own_disposable_workspace() {
        let project = temp_dir("copy-per-round-project");
        fs::write(project.join("a.txt"), b"hi").unwrap();
        let app_data_dir = temp_dir("copy-per-round-appdata");

        let workspace1 = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();
        let workspace2 = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();
        assert_ne!(
            workspace1.root, workspace2.root,
            "two prepare() calls for the same round must never collide (crash-recovery re-run safety)"
        );
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn cleanup_removes_the_workspace_and_leaves_the_project_untouched() {
        let project = temp_dir("cleanup-project");
        fs::write(project.join("a.txt"), b"hi").unwrap();
        let app_data_dir = temp_dir("cleanup-appdata");
        let workspace = prepare(&app_data_dir, "proj-1", "task-1", 1, &project).unwrap();

        cleanup(&app_data_dir, &workspace.root).unwrap();

        assert!(!workspace.root.exists());
        assert!(project.join("a.txt").exists());
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }

    #[test]
    fn cleanup_refuses_a_path_outside_the_review_workspaces_root() {
        let app_data_dir = temp_dir("cleanup-refuse-appdata");
        let unrelated = temp_dir("cleanup-refuse-unrelated");
        fs::write(unrelated.join("important.txt"), b"do not delete").unwrap();

        let result = cleanup(&app_data_dir, &unrelated);
        assert!(result.is_err());
        assert!(unrelated.join("important.txt").exists());
        let _ = fs::remove_dir_all(&app_data_dir);
        let _ = fs::remove_dir_all(&unrelated);
    }

    #[test]
    fn task_id_path_traversal_is_rejected_before_any_copy_happens() {
        let project = temp_dir("traversal-project");
        fs::write(project.join("a.txt"), b"hi").unwrap();
        let app_data_dir = temp_dir("traversal-appdata");

        let result = prepare(&app_data_dir, "proj-1", "../escape", 1, &project);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&project);
        let _ = fs::remove_dir_all(&app_data_dir);
    }
}
