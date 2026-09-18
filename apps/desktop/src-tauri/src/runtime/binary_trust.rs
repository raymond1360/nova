//! Second re-review remediation (Blocker 2 / "executable trust boundary"):
//! a configured `binary_path` — whether it is a default, PATH-discovered
//! name or a user-typed custom path — must never be launched purely
//! because a database row *says* it is trusted. This module computes a
//! concrete [`BinaryFingerprint`] (canonical path, size, mtime, SHA-256)
//! for a candidate executable, which `commands::confirm_cli_binary_path`
//! persists only after the user has explicitly seen and confirmed it
//! (`storage::binary_trust`), and which readiness/execution re-derive and
//! compare against that stored record every time before ever spawning the
//! binary — so a setting changed directly in the database, or a binary
//! swapped out on disk after being trusted, can never silently inherit an
//! old confirmation.
//!
//! Deliberately conservative: relative paths, directories, and anything
//! that is (or resolves through) a symlink/reparse point are refused
//! outright rather than "trusted anyway" — a batch script or a redirect to
//! an attacker-controlled binary must never pass as a confirmed CLI.

use std::io;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[cfg(windows)]
use std::os::windows::fs::MetadataExt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryFingerprint {
    pub canonical_path: String,
    pub file_size: u64,
    pub modified_unix_ms: i64,
    pub sha256_hex: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FingerprintError {
    NotAbsolute,
    NotFound,
    NotAFile,
    ReparsePoint,
    Io(String),
}

impl std::fmt::Display for FingerprintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAbsolute => write!(f, "可执行文件路径必须是绝对路径"),
            Self::NotFound => write!(f, "找不到指定的可执行文件"),
            Self::NotAFile => write!(f, "指定路径不是一个普通文件"),
            Self::ReparsePoint => write!(f, "拒绝：指定路径是符号链接或联接点（reparse point）"),
            Self::Io(detail) => write!(f, "读取可执行文件信息失败：{detail}"),
        }
    }
}

impl From<io::Error> for FingerprintError {
    fn from(value: io::Error) -> Self {
        if value.kind() == io::ErrorKind::NotFound {
            Self::NotFound
        } else {
            Self::Io(value.to_string())
        }
    }
}

/// Windows `FILE_ATTRIBUTE_REPARSE_POINT`.
#[cfg(windows)]
const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

/// Computes a concrete fingerprint for `path`, refusing anything that is
/// not a real, directly-readable regular file at an absolute path:
/// relative paths (a relative path's meaning depends on an unpredictable
/// process cwd — never suitable as a trust anchor), symlinks/reparse
/// points (checked via `symlink_metadata`, i.e. *without* following the
/// link first — the whole point is to refuse before ever touching
/// whatever it points at), and directories/non-files.
pub fn compute_fingerprint(path: &Path) -> Result<BinaryFingerprint, FingerprintError> {
    if !path.is_absolute() {
        return Err(FingerprintError::NotAbsolute);
    }
    let link_metadata = std::fs::symlink_metadata(path)?;
    if link_metadata.file_type().is_symlink() {
        return Err(FingerprintError::ReparsePoint);
    }
    #[cfg(windows)]
    if link_metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(FingerprintError::ReparsePoint);
    }
    if !link_metadata.is_file() {
        return Err(FingerprintError::NotAFile);
    }

    let canonical = std::fs::canonicalize(path)?;
    let canonical_metadata = std::fs::symlink_metadata(&canonical)?;
    if canonical_metadata.file_type().is_symlink() || !canonical_metadata.is_file() {
        return Err(FingerprintError::ReparsePoint);
    }

    let modified = canonical_metadata.modified()?;
    let modified_unix_ms: i64 = modified
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    let bytes = std::fs::read(&canonical)?;
    let digest = ring::digest::digest(&ring::digest::SHA256, &bytes);
    let sha256_hex = digest
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();

    #[cfg(windows)]
    let canonical_path = super::win_spawn::win32_cwd_form(&canonical);
    #[cfg(not(windows))]
    let canonical_path = canonical.to_string_lossy().into_owned();

    Ok(BinaryFingerprint {
        canonical_path,
        file_size: canonical_metadata.len(),
        modified_unix_ms,
        sha256_hex,
    })
}

/// True only when every field matches exactly — any drift (the file was
/// replaced, resized, touched, or the confirmed path itself changed) means
/// "not the same binary the user confirmed", full stop; there is no
/// partial-trust concept.
pub fn fingerprints_match(a: &BinaryFingerprint, b: &BinaryFingerprint) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-binary-trust-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn computes_a_stable_fingerprint_for_a_real_file() {
        let dir = temp_dir("stable");
        let file = dir.join("tool.exe");
        fs::write(&file, b"pretend binary content").unwrap();
        let a = compute_fingerprint(&file).unwrap();
        let b = compute_fingerprint(&file).unwrap();
        assert!(fingerprints_match(&a, &b));
        assert_eq!(a.file_size, "pretend binary content".len() as u64);
        assert_eq!(a.sha256_hex.len(), 64);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_content_change_produces_a_different_fingerprint() {
        let dir = temp_dir("content-change");
        let file = dir.join("tool.exe");
        fs::write(&file, b"version 1").unwrap();
        let before = compute_fingerprint(&file).unwrap();
        fs::write(&file, b"version 2, quite different").unwrap();
        let after = compute_fingerprint(&file).unwrap();
        assert!(!fingerprints_match(&before, &after));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_relative_path_is_refused() {
        let result = compute_fingerprint(Path::new("relative/tool.exe"));
        assert_eq!(result, Err(FingerprintError::NotAbsolute));
    }

    #[test]
    fn a_directory_is_refused_not_treated_as_a_binary() {
        let dir = temp_dir("dir-refused");
        let result = compute_fingerprint(&dir);
        assert_eq!(result, Err(FingerprintError::NotAFile));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_file_is_refused() {
        let dir = temp_dir("missing");
        let result = compute_fingerprint(&dir.join("does-not-exist.exe"));
        assert_eq!(result, Err(FingerprintError::NotFound));
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_symlink_to_a_real_binary_is_refused_not_followed() {
        let dir = temp_dir("symlink-refused");
        let real = dir.join("real.exe");
        fs::write(&real, b"real content").unwrap();
        let link = dir.join("link.exe");
        if std::os::windows::fs::symlink_file(&real, &link).is_ok() {
            let result = compute_fingerprint(&link);
            assert_eq!(result, Err(FingerprintError::ReparsePoint));
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
