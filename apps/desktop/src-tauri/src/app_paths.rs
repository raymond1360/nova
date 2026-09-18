//! Phase 3 remediation (Blocker 4): safe construction of app-owned
//! directories under Nova's own app-data tree — used for both task
//! packages (`storage::core_loop`) and isolated reviewer workspaces
//! (`core_loop::review_isolation`). Never used for paths inside a
//! user-selected project directory; that boundary is `runtime::path_guard`.
//!
//! Every path segment supplied by request-derived data (project id, task
//! id, a generation/round label) goes through [`sanitize_id_component`]
//! before ever touching the filesystem, and the final constructed
//! directory is canonicalized and verified to still be inside the
//! intended app-owned root — closing the "what if an id contains `..`, a
//! drive letter, or a reserved device name" and "what if an ancestor is a
//! symlink/junction that redirects us outside the root" classes of attack
//! in one place, rather than trusting every call site to get it right.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Windows reserved device names — illegal as a file/directory name
/// regardless of extension (`NUL`, `nul.txt`, `Nul.tar.gz` are all
/// reserved).
const RESERVED_WINDOWS_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Validates that `value` is safe to use as exactly one path *component*
/// (never a multi-segment path) when building an app-owned directory.
/// Rejects: empty, `.`/`..`, any path separator (`/` or `\`), a drive
/// letter or UNC marker (`:`), Windows reserved device names, control
/// characters, and a trailing dot or space (both silently stripped by
/// Windows APIs, which has historically been an escape vector for
/// same-looking-different-meaning paths).
pub fn sanitize_id_component(value: &str) -> Result<&str, String> {
    if value.is_empty() {
        return Err("路径片段不能为空".to_string());
    }
    if value == "." || value == ".." {
        return Err("路径片段不能是 . 或 ..".to_string());
    }
    if value.contains('/') || value.contains('\\') {
        return Err("路径片段不能包含路径分隔符".to_string());
    }
    if value.contains(':') {
        return Err("路径片段不能包含冒号（盘符/UNC 标记）".to_string());
    }
    if value.chars().any(|c| c.is_control()) {
        return Err("路径片段不能包含控制字符".to_string());
    }
    if value.ends_with('.') || value.ends_with(' ') {
        return Err("路径片段不能以点或空格结尾".to_string());
    }
    if value.starts_with(' ') {
        return Err("路径片段不能以空格开头".to_string());
    }
    let base = value.split('.').next().unwrap_or(value);
    if RESERVED_WINDOWS_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(base))
    {
        return Err(format!("路径片段是 Windows 保留设备名：{value}"));
    }
    Ok(value)
}

/// Builds (creating as needed) `root/components[0]/components[1]/...`,
/// entirely under an app-owned `root`, with every intermediate level
/// checked for a symlink/junction/reparse-point *before* descending into
/// it — so a pre-existing malicious reparse point anywhere in the chain is
/// rejected instead of silently followed. Returns the canonicalized final
/// path, which is verified to still be a descendant of the canonicalized
/// `root` (the actual containment guarantee; component sanitization alone
/// only rules out the obvious cases).
pub fn safe_app_owned_dir(root: &Path, components: &[&str]) -> io::Result<PathBuf> {
    for component in components {
        sanitize_id_component(component)
            .map_err(|message| io::Error::new(io::ErrorKind::InvalidInput, message))?;
    }

    fs::create_dir_all(root)?;
    let canonical_root = fs::canonicalize(root)?;
    let mut current = canonical_root.clone();

    for component in components {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    return Err(io::Error::other(format!(
                        "拒绝：路径祖先是符号链接或联接点：{}",
                        current.display()
                    )));
                }
                if !metadata.is_dir() {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("路径祖先已存在但不是目录：{}", current.display()),
                    ));
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Another thread/process can win a race and create this
                // exact directory between the `symlink_metadata` check above
                // and this `create_dir` call — expected and safe under
                // concurrent task-package creation (Blocker 4 requirement),
                // so `AlreadyExists` here is not itself an error: fall
                // through to re-checking what now exists, exactly as the
                // `Ok(metadata)` branch above already does.
                match fs::create_dir(&current) {
                    Ok(()) => {}
                    Err(create_error) if create_error.kind() == io::ErrorKind::AlreadyExists => {
                        let metadata = fs::symlink_metadata(&current)?;
                        if metadata.file_type().is_symlink() {
                            return Err(io::Error::other(format!(
                                "拒绝：路径祖先是符号链接或联接点：{}",
                                current.display()
                            )));
                        }
                        if !metadata.is_dir() {
                            return Err(io::Error::new(
                                io::ErrorKind::AlreadyExists,
                                format!("路径祖先已存在但不是目录：{}", current.display()),
                            ));
                        }
                    }
                    Err(create_error) => return Err(create_error),
                }
            }
            Err(error) => return Err(error),
        }
    }

    let canonical_final = fs::canonicalize(&current)?;
    if !canonical_final.starts_with(&canonical_root) {
        return Err(io::Error::other("拒绝：最终路径逃逸出 app-owned 根目录"));
    }
    Ok(canonical_final)
}

/// Writes `contents` to `path` via a temp-file-then-rename sequence, so a
/// concurrent reader (or a crash mid-write) never observes a partially
/// written file. `path` must already be inside an app-owned directory —
/// this function does not itself re-verify containment (callers use
/// [`safe_app_owned_dir`] to obtain the directory first).
///
/// Phase 3 re-review hardening (Blocker 4 / Minor "atomic writes weaker
/// than requested"): the temp file is opened with `create_new` (never
/// silently reuses/truncates an existing file — a name collision is a hard
/// error, not a silent overwrite), its bytes are `flush`ed and `sync_all`ed
/// before the rename so the *content* is durable on disk before the rename
/// that makes it visible, the parent directory is best-effort synced after
/// the rename, and a symlink/junction sitting at the destination path is
/// refused rather than followed. On any failure, only the temp file this
/// call itself created is removed — an existing `path` is never touched
/// unless the rename actually succeeds.
pub fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    use std::io::Write as _;

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "路径没有父目录"))?;

    if let Ok(metadata) = fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(io::Error::other(format!(
                "拒绝：目标路径是符号链接或联接点，拒绝写入：{}",
                path.display()
            )));
        }
    }

    let tmp_path = parent.join(format!(".tmp-{}", uuid::Uuid::new_v4()));
    let write_result = (|| -> io::Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&tmp_path)?;
        file.write_all(contents)?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = write_result {
        let _ = fs::remove_file(&tmp_path);
        return Err(error);
    }

    if let Err(error) = fs::rename(&tmp_path, path) {
        let _ = fs::remove_file(&tmp_path);
        return Err(error);
    }

    // Best-effort durability for the rename itself. Not all platforms
    // support opening a plain directory handle for this (notably Windows,
    // where this reliably fails) — that failure is intentionally
    // non-fatal: the file's own content is already fsynced above, and the
    // rename has already completed; this step only strengthens "did the
    // rename survive a crash" where the OS actually supports it.
    if let Ok(dir) = fs::File::open(parent) {
        let _ = dir.sync_all();
    }

    Ok(())
}

/// Recursively deletes `target`, but only after verifying it is a
/// descendant of `expected_root` (both canonicalized first) — the guard
/// against ever recursively deleting an uncertain/attacker-influenced
/// path. Symlinks/junctions encountered *within* `target` are removed as
/// entries themselves (never followed), so a symlink planted inside a
/// cleanup target cannot cause deletion to escape into whatever it points
/// at.
pub fn safe_remove_dir_all(expected_root: &Path, target: &Path) -> io::Result<()> {
    let canonical_root = fs::canonicalize(expected_root)?;
    let canonical_target = match fs::canonicalize(target) {
        Ok(path) => path,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    if !canonical_target.starts_with(&canonical_root) {
        return Err(io::Error::other(format!(
            "拒绝：清理目标 {} 不在预期根目录 {} 之内",
            canonical_target.display(),
            canonical_root.display()
        )));
    }
    remove_dir_all_no_follow(&canonical_target)?;
    fs::remove_dir(&canonical_target)
}

fn remove_dir_all_no_follow(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        if file_type.is_symlink() {
            if file_type.is_dir() {
                fs::remove_dir(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
        } else if file_type.is_dir() {
            remove_dir_all_no_follow(&path)?;
            fs::remove_dir(&path)?;
        } else {
            fs::remove_file(&path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Arc;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = env::temp_dir();
        dir.push(format!(
            "nova-app-paths-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn accepts_ordinary_uuid_like_components() {
        assert!(sanitize_id_component("b7e2f6c0-1234-4a5b-9c1d-abcdef123456").is_ok());
        assert!(sanitize_id_component("gen-3").is_ok());
    }

    #[test]
    fn rejects_dot_dot_and_separators() {
        for value in ["..", ".", "a/b", "a\\b", "../x", "..\\x"] {
            assert!(
                sanitize_id_component(value).is_err(),
                "expected rejection for {value:?}"
            );
        }
    }

    #[test]
    fn rejects_absolute_drive_and_unc_forms() {
        for value in ["C:", "C:\\x", "\\\\server\\share"] {
            assert!(
                sanitize_id_component(value).is_err(),
                "expected rejection for {value:?}"
            );
        }
    }

    #[test]
    fn rejects_reserved_windows_device_names_case_insensitively() {
        for value in ["CON", "con", "NUL", "nul.txt", "COM1", "lpt9"] {
            assert!(
                sanitize_id_component(value).is_err(),
                "expected rejection for {value:?}"
            );
        }
    }

    #[test]
    fn rejects_trailing_dot_or_space_and_leading_space() {
        for value in ["name.", "name ", " name"] {
            assert!(
                sanitize_id_component(value).is_err(),
                "expected rejection for {value:?}"
            );
        }
    }

    #[test]
    fn rejects_control_characters() {
        assert!(sanitize_id_component("a\0b").is_err());
        assert!(sanitize_id_component("a\nb").is_err());
    }

    #[test]
    fn builds_and_contains_a_nested_app_owned_directory() {
        let root = temp_dir("build-nested");
        let result = safe_app_owned_dir(&root, &["proj-1", "task-1", "gen-1"]).unwrap();
        assert!(result.starts_with(fs::canonicalize(&root).unwrap()));
        assert!(result.is_dir());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_a_task_id_that_tries_to_escape_with_dot_dot() {
        let root = temp_dir("escape-dotdot");
        let result = safe_app_owned_dir(&root, &["proj-1", "..", "gen-1"]);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_a_drive_prefixed_component() {
        let root = temp_dir("escape-drive");
        let result = safe_app_owned_dir(&root, &["proj-1", "C:", "gen-1"]);
        assert!(result.is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn rejects_a_junction_ancestor() {
        let root = temp_dir("junction-ancestor");
        let outside = temp_dir("junction-target-outside");
        let proj_dir = root.join("proj-1");
        // Create the junction using the real `mklink /J` tool (matches the
        // pattern already used by Phase 1/2 junction-escape tests).
        let status = std::process::Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                proj_dir.to_str().unwrap(),
                outside.to_str().unwrap(),
            ])
            .status();
        if let Ok(status) = status {
            if status.success() {
                let result = safe_app_owned_dir(&root, &["proj-1", "task-1"]);
                assert!(result.is_err(), "a junction ancestor must be rejected");
            }
        }
        let _ = fs::remove_dir_all(&outside);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn atomic_write_produces_full_content_and_can_be_overwritten() {
        let root = temp_dir("atomic-write");
        let dir = safe_app_owned_dir(&root, &["proj-1"]).unwrap();
        let path = dir.join("REQUIREMENT.md");
        atomic_write(&path, b"first version").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "first version");
        atomic_write(&path, b"second version, longer than the first").unwrap();
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "second version, longer than the first"
        );
        // No stray temp files left behind.
        let stray: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(stray.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn atomic_write_refuses_to_overwrite_a_symlink_destination() {
        let root = temp_dir("atomic-write-symlink-dest");
        let dir = safe_app_owned_dir(&root, &["proj-1"]).unwrap();
        let real_target = temp_dir("atomic-write-symlink-target");
        let outside_file = real_target.join("secret.txt");
        fs::write(&outside_file, b"outside content").unwrap();
        let link_path = dir.join("REQUIREMENT.md");

        #[cfg(windows)]
        let link_created = std::os::windows::fs::symlink_file(&outside_file, &link_path).is_ok();
        #[cfg(unix)]
        let link_created = std::os::unix::fs::symlink(&outside_file, &link_path).is_ok();

        if link_created {
            let result = atomic_write(&link_path, b"HACKED");
            assert!(
                result.is_err(),
                "writing through a symlink destination must be refused"
            );
            assert_eq!(
                fs::read_to_string(&outside_file).unwrap(),
                "outside content",
                "the real file the symlink points at must be untouched"
            );
        }
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&real_target);
    }

    #[test]
    fn atomic_write_fails_cleanly_when_destination_is_a_directory() {
        let root = temp_dir("atomic-write-dest-is-dir");
        let dir = safe_app_owned_dir(&root, &["proj-1"]).unwrap();
        let path = dir.join("REQUIREMENT.md");
        fs::create_dir(&path).unwrap();

        let result = atomic_write(&path, b"content");
        assert!(
            result.is_err(),
            "rename onto an existing directory must fail, not silently succeed"
        );
        assert!(path.is_dir(), "the directory must survive the failed write");

        let stray: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(
            stray.is_empty(),
            "the temp file created for the failed attempt must be cleaned up"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn atomic_write_never_touches_the_old_file_when_the_write_step_fails() {
        // Simulate a mid-write failure by pointing `path` at a location
        // whose parent directory does not exist — `atomic_write` must
        // return a clean error without panicking, and must not have
        // fabricated any partial state at the (nonexistent) destination.
        let root = temp_dir("atomic-write-bad-parent");
        let missing_parent = root.join("does-not-exist");
        let path = missing_parent.join("REQUIREMENT.md");
        let result = atomic_write(&path, b"content");
        assert!(result.is_err());
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn concurrent_atomic_writes_to_the_same_path_never_produce_torn_content() {
        let root = temp_dir("atomic-write-concurrent");
        let dir = safe_app_owned_dir(&root, &["proj-1"]).unwrap();
        let path = Arc::new(dir.join("REQUIREMENT.md"));

        // Each writer's payload is internally consistent and distinguishable
        // from every other writer's payload, so if the final file ever
        // contained bytes spliced from two different writes, it would fail
        // to match *any* single expected payload below.
        let payloads: Vec<Vec<u8>> = (0..8)
            .map(|i| format!("writer-{i}-{}", "x".repeat(500 + i * 37)).into_bytes())
            .collect();

        let handles: Vec<_> = payloads
            .iter()
            .cloned()
            .map(|payload| {
                let path = Arc::clone(&path);
                std::thread::spawn(move || atomic_write(&path, &payload))
            })
            .collect();
        for handle in handles {
            handle.join().unwrap().unwrap();
        }

        let final_content = fs::read(path.as_path()).unwrap();
        assert!(
            payloads.iter().any(|payload| payload == &final_content),
            "final content must be exactly one writer's full payload, never a mix"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn safe_remove_dir_all_refuses_a_target_outside_the_expected_root() {
        let root = temp_dir("cleanup-root");
        let outside = temp_dir("cleanup-outside");
        let result = safe_remove_dir_all(&root, &outside);
        assert!(
            result.is_err(),
            "must refuse to delete a directory outside the expected root"
        );
        assert!(
            outside.exists(),
            "the outside directory must survive the refused deletion"
        );
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }

    #[test]
    fn safe_remove_dir_all_deletes_a_real_descendant() {
        let root = temp_dir("cleanup-descendant-root");
        let target = safe_app_owned_dir(&root, &["proj-1", "task-1"]).unwrap();
        fs::write(target.join("file.txt"), b"data").unwrap();
        safe_remove_dir_all(&root, &target).unwrap();
        assert!(!target.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn safe_remove_dir_all_on_an_already_missing_target_is_a_harmless_no_op() {
        let root = temp_dir("cleanup-missing-root");
        let missing = root.join("does-not-exist");
        assert!(safe_remove_dir_all(&root, &missing).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn concurrent_calls_building_the_same_nested_directory_never_spuriously_fail() {
        // Blocker 4 requires concurrent writes to the same task package to
        // be safe. `safe_app_owned_dir` builds each ancestor with a
        // check-then-create sequence; without race handling, two threads
        // racing to create the *same* not-yet-existing ancestor directory
        // would have one of them observe `ERROR_ALREADY_EXISTS` from
        // `create_dir` and fail outright instead of recognizing "it already
        // exists as a real directory, that's fine."
        let root = Arc::new(temp_dir("concurrent-same-dir"));
        let handles: Vec<_> = (0..16)
            .map(|_| {
                let root = Arc::clone(&root);
                std::thread::spawn(move || {
                    safe_app_owned_dir(&root, &["proj-1", "task-1", "round-1"])
                })
            })
            .collect();
        let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(
            results.iter().all(|r| r.is_ok()),
            "every racing caller building the same directory must succeed: {results:?}"
        );
        let first = results[0].as_ref().unwrap();
        for result in &results {
            assert_eq!(
                result.as_ref().unwrap(),
                first,
                "all callers must agree on the final path"
            );
        }
        let _ = fs::remove_dir_all(root.as_ref());
    }
}
