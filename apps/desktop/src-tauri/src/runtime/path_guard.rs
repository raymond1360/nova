use std::path::{Path, PathBuf};

/// Reason a candidate path was rejected by the directory boundary check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRejection {
    Empty,
    NotFound,
    NotADirectory,
    PermissionDenied,
    OutsideAllowedRoot,
}

impl PathRejection {
    pub fn detail(self) -> &'static str {
        match self {
            Self::Empty => "路径为空",
            Self::NotFound => "路径不存在",
            Self::NotADirectory => "路径不是目录",
            Self::PermissionDenied => "没有访问该目录的权限",
            Self::OutsideAllowedRoot => "路径超出允许的目录边界",
        }
    }
}

/// Resolve `candidate` to a real, canonical directory and confirm it is
/// `allowed_root` itself or a descendant of it.
///
/// This is the single boundary check every part of Nova must use before
/// treating a user-supplied or stored path as a safe process working
/// directory. It defeats `..` traversal and symlink/junction escapes by
/// resolving both paths to their real filesystem target with
/// [`std::fs::canonicalize`] *before* comparing — a candidate that looks
/// like it is inside the root as a string, but actually resolves (via a
/// symlink/junction) outside it, is rejected because the *canonical*
/// target fails the prefix check, not the pre-resolution string.
pub fn validate_directory_boundary(
    candidate: &Path,
    allowed_root: &Path,
) -> Result<PathBuf, PathRejection> {
    if candidate.as_os_str().is_empty() {
        return Err(PathRejection::Empty);
    }

    let canonical_root = canonicalize(allowed_root)?;
    if !canonical_root.is_dir() {
        return Err(PathRejection::NotADirectory);
    }

    let canonical_candidate = canonicalize(candidate)?;
    if !canonical_candidate.is_dir() {
        return Err(PathRejection::NotADirectory);
    }

    if canonical_candidate == canonical_root || canonical_candidate.starts_with(&canonical_root) {
        Ok(canonical_candidate)
    } else {
        Err(PathRejection::OutsideAllowedRoot)
    }
}

fn canonicalize(path: &Path) -> Result<PathBuf, PathRejection> {
    std::fs::canonicalize(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::PermissionDenied => PathRejection::PermissionDenied,
        _ => PathRejection::NotFound,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-path-guard-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn accepts_the_root_itself() {
        let root = temp_dir("root-itself");
        let resolved = validate_directory_boundary(&root, &root).expect("root should validate");
        assert!(resolved.is_dir());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn accepts_a_plain_subdirectory() {
        let root = temp_dir("subdir-root");
        let child = root.join("child");
        fs::create_dir_all(&child).unwrap();
        let resolved = validate_directory_boundary(&child, &root).expect("child should validate");
        assert!(resolved.starts_with(canonicalize(&root).unwrap()));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn accepts_chinese_and_space_containing_paths() {
        let root = temp_dir("unicode-root");
        let child = root.join("项目 目录 with spaces 中文");
        fs::create_dir_all(&child).unwrap();
        assert!(validate_directory_boundary(&child, &root).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_dot_dot_traversal_outside_root() {
        let root = temp_dir("traversal-root");
        let sibling = temp_dir("traversal-sibling");
        let escape = root.join("..").join(sibling.file_name().unwrap());
        let result = validate_directory_boundary(&escape, &root);
        assert_eq!(result, Err(PathRejection::OutsideAllowedRoot));
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&sibling);
    }

    #[test]
    fn dot_dot_that_resolves_back_inside_root_is_allowed() {
        let root = temp_dir("traversal-inside-root");
        let child = root.join("a").join("b");
        fs::create_dir_all(&child).unwrap();
        let back_to_a = child.join("..");
        assert!(validate_directory_boundary(&back_to_a, &root).is_ok());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_nonexistent_path() {
        let root = temp_dir("missing-root");
        let missing = root.join("does-not-exist");
        assert_eq!(
            validate_directory_boundary(&missing, &root),
            Err(PathRejection::NotFound)
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_file_used_as_directory() {
        let root = temp_dir("file-not-dir-root");
        let file_path = root.join("file.txt");
        fs::write(&file_path, b"not a directory").unwrap();
        assert_eq!(
            validate_directory_boundary(&file_path, &root),
            Err(PathRejection::NotADirectory)
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_unrelated_sibling_with_overlapping_name_prefix() {
        // "root-evil" must not be treated as inside "root" just because the
        // string "root" is a text prefix of "root-evil".
        let base = temp_dir("prefix-base");
        let root = base.join("root");
        let evil = base.join("root-evil");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&evil).unwrap();
        assert_eq!(
            validate_directory_boundary(&evil, &root),
            Err(PathRejection::OutsideAllowedRoot)
        );
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn rejects_unreachable_unc_style_path_without_panicking() {
        let root = temp_dir("unc-root");
        // Not expected to exist in the test environment; must fail safely
        // as NotFound rather than panicking or being silently accepted.
        let unc = PathBuf::from(r"\\nova-test-nonexistent-host\share\project");
        assert_eq!(
            validate_directory_boundary(&unc, &root),
            Err(PathRejection::NotFound)
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rejects_relative_path_that_escapes_root() {
        let root = temp_dir("relative-root");
        let previous = std::env::current_dir().unwrap();
        std::env::set_current_dir(&root).unwrap();
        let result = validate_directory_boundary(Path::new(".."), &root);
        std::env::set_current_dir(previous).unwrap();
        assert_eq!(result, Err(PathRejection::OutsideAllowedRoot));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    #[cfg(windows)]
    fn rejects_directory_junction_that_escapes_root() {
        use std::process::Command;

        let root = temp_dir("junction-root");
        let outside = temp_dir("junction-outside");
        let link = root.join("escape-link");

        let status = Command::new("cmd")
            .args([
                "/C",
                "mklink",
                "/J",
                link.to_str().unwrap(),
                outside.to_str().unwrap(),
            ])
            .status();

        match status {
            Ok(status) if status.success() => {
                let result = validate_directory_boundary(&link, &root);
                assert_eq!(result, Err(PathRejection::OutsideAllowedRoot));
            }
            _ => {
                eprintln!(
                    "skipping junction-escape test: `mklink /J` unavailable in this environment"
                );
            }
        }

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
    }
}
