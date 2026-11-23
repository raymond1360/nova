//! Shared bounded contracts and file identity/write mechanics extracted from the audited API path.
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
pub type Result<T> = std::result::Result<T, &'static str>;
fn io<T>(r: std::io::Result<T>) -> Result<T> {
    r.map_err(|_| "TASK_FILE_IO")
}
const RELAY_SIGNATURE: &str = "pub fn normalize_label(s: &str) -> String {";
pub fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn relative(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 512
        && !s.contains(['\\', ':', '*', '?', '[', ']', '\0'])
        && s.split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && !p.ends_with(['.', ' ']))
}
pub fn relay_json<T: serde::de::DeserializeOwned>(b: &[u8], limit: usize) -> Result<T> {
    if b.is_empty() || b.len() > limit {
        return Err("BOUND");
    }
    let u: Unique = serde_json::from_slice(b).map_err(|_| "SCHEMA_REJECTED")?;
    fn valid(v: &Value, d: usize) -> bool {
        if d > 32 {
            return false;
        }
        match v {
            Value::String(s) => s.len() <= 65536,
            Value::Array(a) => a.len() <= 256 && a.iter().all(|v| valid(v, d + 1)),
            Value::Object(m) => {
                m.len() <= 128 && m.iter().all(|(k, v)| k.len() <= 256 && valid(v, d + 1))
            }
            _ => true,
        }
    }
    if !valid(&u.0, 0) {
        return Err("BOUND");
    }
    serde_json::from_value(u.0).map_err(|_| "SCHEMA_REJECTED")
}
pub fn api_json<T: serde::de::DeserializeOwned>(b: &[u8], cap: usize) -> Result<T> {
    let v: serde_json::Value = relay_json(b, cap)?;
    fn walk(v: &serde_json::Value, d: usize, n: &mut usize) -> Result<()> {
        if d > 16 {
            return Err("BOUND");
        };
        *n = n.checked_add(1).ok_or("BOUND")?;
        if *n > 2048 {
            return Err("BOUND");
        }
        match v {
            serde_json::Value::String(s) if s.len() > 65536 => Err("BOUND"),
            serde_json::Value::Array(a) => {
                for x in a {
                    walk(x, d + 1, n)?
                }
                Ok(())
            }
            serde_json::Value::Object(m) => {
                for (k, x) in m {
                    if k.len() > 4096 {
                        return Err("BOUND");
                    };
                    walk(x, d + 1, n)?
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    walk(&v, 0, &mut 0)?;
    serde_json::from_value(v).map_err(|_| "SCHEMA_REJECTED")
}
pub(crate) struct Unique(pub(crate) Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bounded JSON")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut m: M,
            ) -> std::result::Result<Unique, M::Error> {
                let mut out = serde_json::Map::new();
                while let Some(k) = m.next_key::<String>()? {
                    if out.contains_key(&k) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    out.insert(k, m.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(out)))
            }
            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut s: S,
            ) -> std::result::Result<Unique, S::Error> {
                let mut v = vec![];
                while let Some(x) = s.next_element::<Unique>()? {
                    v.push(x.0)
                }
                Ok(Unique(Value::Array(v)))
            }
            fn visit_str<E: de::Error>(self, s: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::String(s.into())))
            }
            fn visit_string<E: de::Error>(self, s: String) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::String(s)))
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Bool(b)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(n.into()))
            }
            fn visit_u64<E: de::Error>(self, n: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(n.into()))
            }
            fn visit_f64<E: de::Error>(self, n: f64) -> std::result::Result<Unique, E> {
                serde_json::Number::from_f64(n)
                    .map(|v| Unique(Value::Number(v)))
                    .ok_or_else(|| de::Error::custom("number"))
            }
        }
        d.deserialize_any(V)
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Severity {
    Blocker,
    High,
    Medium,
    Low,
    Info,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub schema: String,
    pub target_path: String,
    pub old_text: String,
    pub new_text: String,
    pub rationale: String,
}
impl Proposal {
    pub fn validate_target(&self, target: &str) -> Result<()> {
        if self.schema != "nova.relay.proposal/1"
            || self.target_path != target
            || self.old_text.is_empty()
            || self.old_text.len() > 8192
            || self.new_text.len() > 16384
            || self.rationale.len() > 2048
            || self.old_text.contains(['\0', '\r'])
            || self.new_text.contains(['\0', '\r'])
        {
            return Err("SCHEMA_REJECTED");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewIssue {
    pub id: String,
    pub severity: Severity,
    pub blocking: bool,
    pub location: String,
    pub summary: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewClaim {
    pub schema: String,
    pub base_sha256: String,
    pub candidate_sha256: String,
    pub verdict: String,
    pub issues: Vec<ReviewIssue>,
    pub unknowns: Vec<String>,
}
impl ReviewClaim {
    pub fn validate(&self, b: &str, c: &str) -> Result<()> {
        if self.schema != "nova.relay.review/1"
            || self.base_sha256 != b
            || self.candidate_sha256 != c
        {
            return Err("BINDING_MISMATCH");
        }
        if !["PASS", "CHANGES_REQUIRED", "BLOCKED"].contains(&self.verdict.as_str())
            || self.issues.len() > 32
            || self.unknowns.len() > 16
            || self.unknowns.iter().any(|s| s.len() > 512)
        {
            return Err("SCHEMA_REJECTED");
        }
        let mut seen = BTreeSet::new();
        for i in &self.issues {
            let n = i.id.strip_prefix("R-").ok_or("SCHEMA_REJECTED")?;
            if n.is_empty()
                || n.len() > 3
                || !n.bytes().all(|b| b.is_ascii_digit())
                || !seen.insert(&i.id)
                || i.location.len() > 256
                || i.summary.len() > 1024
            {
                return Err("SCHEMA_REJECTED");
            }
        }
        Ok(())
    }
}

pub const TOOL_HASH_CAP: u64 = 512 * 1024 * 1024;
/// Approval paths are not normalized on behalf of their author.
pub fn approved_final_path(approved: &Path, actual: &Path) -> Result<()> {
    if approved.as_os_str() != actual.as_os_str() {
        return Err("APPROVAL_MISMATCH");
    }
    Ok(())
}
pub fn approved_root(path: &Path, exists: bool) -> Result<()> {
    absolute(path)?;
    safe_location(path)?;
    let parent = Guard::open(path.parent().ok_or("PATH_REJECTED")?, true)?;
    let expected = parent.path.join(path.file_name().ok_or("PATH_REJECTED")?);
    approved_final_path(path, &expected)?;
    if exists {
        approved_final_path(path, &Guard::open(path, true)?.path)?;
    } else if path.try_exists().map_err(|_| "PATH_REJECTED")? {
        return Err("CREATE_NEW_REJECTED");
    }
    Ok(())
}
pub struct Guard {
    file: File,
    pub path: PathBuf,
    pub identity: String,
    tool_links: bool,
}
fn normal(s: &str) -> String {
    s.trim_start_matches("\\\\?\\")
        .replace('/', "\\")
        .to_lowercase()
}
pub fn absolute(path: &Path) -> Result<()> {
    let s = path.to_str().ok_or("PATH_REJECTED")?;
    if !path.is_absolute()
        || s.starts_with("\\\\")
        || s.contains('\0')
        || s.chars().filter(|c| *c == ':').count() != 1
        || s.split(['\\', '/'])
            .skip(1)
            .any(|p| p == ".." || p == "." || p.ends_with(['.', ' ']))
    {
        return Err("PATH_REJECTED");
    }
    Ok(())
}
fn components(path: &Path) -> Result<()> {
    absolute(path)?;
    let mut part = PathBuf::new();
    for c in path.components() {
        part.push(c);
        let m = fs::symlink_metadata(&part).map_err(|_| "PATH_MISSING")?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if m.file_attributes() & 0x400 != 0 {
                return Err("LINK_REJECTED");
            }
        }
        if m.file_type().is_symlink() {
            return Err("LINK_REJECTED");
        }
    }
    Ok(())
}
#[cfg(windows)]
fn facts(file: &File) -> Result<(String, PathBuf, u32)> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION,
    };
    let mut i: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    let h = file.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE;
    if unsafe { GetFileInformationByHandle(h, &mut i) } == 0 || i.dwFileAttributes & 0x400 != 0 {
        return Err("FILE_IDENTITY_UNKNOWN");
    }
    let mut buffer = vec![0u16; 32768];
    let n = unsafe { GetFinalPathNameByHandleW(h, buffer.as_mut_ptr(), buffer.len() as u32, 0) }
        as usize;
    if n == 0 || n >= buffer.len() {
        return Err("FILE_IDENTITY_UNKNOWN");
    }
    let p = String::from_utf16(&buffer[..n]).map_err(|_| "PATH_REJECTED")?;
    if p.starts_with("\\\\?\\UNC\\") {
        return Err("PATH_REJECTED");
    }
    Ok((
        format!(
            "{:08x}:{:08x}{:08x}",
            i.dwVolumeSerialNumber, i.nFileIndexHigh, i.nFileIndexLow
        ),
        PathBuf::from(p.trim_start_matches("\\\\?\\")),
        i.nNumberOfLinks,
    ))
}
#[cfg(not(windows))]
fn facts(_: &File) -> Result<(String, PathBuf, u32)> {
    Err("WINDOWS_REQUIRED")
}
// Probe helper validates the already-open exclusive write handle before any bytes change.
#[cfg(any(test, all(feature = "scheduler-validation", debug_assertions)))]
pub(crate) fn single_link_identity(file: &File) -> Result<String> {
    let (id, _, links) = facts(file)?;
    if links != 1 {
        return Err("ALIAS_REJECTED");
    }
    Ok(id)
}
impl Guard {
    pub fn open(path: &Path, db: bool) -> Result<Self> {
        Self::open_policy(path, db, false)
    }
    /// Installed tools may legitimately be hardlinked (Git for Windows). This
    /// exception never applies to repositories, reports or owned databases.
    pub fn tool(path: &Path) -> Result<Self> {
        Self::open_policy(path, false, true)
    }
    fn open_policy(path: &Path, db: bool, tool_links: bool) -> Result<Self> {
        components(path)?;
        let mut opts = OpenOptions::new();
        opts.read(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(if db { 3 } else { 1 })
                .custom_flags(0x00200000 | 0x02000000);
        }
        let file = opts.open(path).map_err(|_| "FILE_OPEN_REJECTED")?;
        let (identity, final_path, links) = facts(&file)?;
        if normal(path.to_str().ok_or("PATH_REJECTED")?)
            != normal(final_path.to_str().ok_or("PATH_REJECTED")?)
            || (!tool_links && !file.metadata().map_err(|_| "FILE_STAT")?.is_dir() && links != 1)
        {
            return Err("ALIAS_REJECTED");
        }
        Ok(Self {
            file,
            path: final_path,
            identity,
            tool_links,
        })
    }
    pub fn verify(&self) -> Result<()> {
        let (id, path, _) = facts(&self.file)?;
        let other = Guard::open_policy(&self.path, true, self.tool_links)?;
        if id != self.identity || path != self.path || other.identity != id {
            return Err("FILE_CHANGED");
        }
        Ok(())
    }
    pub fn verify_sha256_stream(
        &mut self,
        bytes: u64,
        expected: &str,
        end: std::time::Instant,
    ) -> Result<()> {
        if !hex(expected, 64) || self.sha256_stream(bytes, end)? != expected {
            return Err("TOOL_HASH_MISMATCH");
        }
        Ok(())
    }
    pub fn sha256_stream(
        &mut self,
        expected_bytes: u64,
        end: std::time::Instant,
    ) -> Result<String> {
        use std::io::{Seek, SeekFrom};
        if expected_bytes > TOOL_HASH_CAP {
            return Err("FILE_BOUND");
        }
        let before = self.file.metadata().map_err(|_| "FILE_STAT")?;
        if !before.is_file() || before.len() != expected_bytes {
            return Err("TOOL_SIZE_MISMATCH");
        }
        self.verify()?;
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|_| "FILE_READ")?;
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
        let mut buf = vec![0u8; 1048576];
        let mut count = 0u64;
        loop {
            if std::time::Instant::now() >= end {
                return Err("TOOL_HASH_DEADLINE");
            }
            let n = self.file.read(&mut buf).map_err(|_| "FILE_READ")?;
            if n == 0 {
                break;
            }
            count = count.checked_add(n as u64).ok_or("FILE_BOUND")?;
            if count > expected_bytes {
                return Err("FILE_CHANGED");
            }
            ctx.update(&buf[..n]);
        }
        let after = self.file.metadata().map_err(|_| "FILE_STAT")?;
        self.verify()?;
        if count != expected_bytes
            || after.len() != before.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err("FILE_CHANGED");
        }
        Ok(ctx
            .finish()
            .as_ref()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
    pub fn bytes(&mut self, limit: usize) -> Result<Vec<u8>> {
        let before = self.file.metadata().map_err(|_| "FILE_STAT")?;
        if !before.is_file() || before.len() > limit as u64 {
            return Err("FILE_BOUND");
        }
        let mut b = vec![];
        (&mut self.file)
            .take(limit as u64 + 1)
            .read_to_end(&mut b)
            .map_err(|_| "FILE_READ")?;
        let after = self.file.metadata().map_err(|_| "FILE_STAT")?;
        self.verify()?;
        if b.len() as u64 != before.len()
            || after.len() != before.len()
            || before.modified().ok() != after.modified().ok()
        {
            return Err("FILE_CHANGED");
        }
        Ok(b)
    }
}
pub fn read_file(path: &Path, limit: usize) -> Result<(Vec<u8>, String)> {
    let mut g = Guard::open(path, false)?;
    let b = g.bytes(limit)?;
    Ok((b, g.identity))
}
pub fn inside(path: &Path, root: &Path) -> bool {
    let p = normal(&path.to_string_lossy());
    let r = normal(&root.to_string_lossy());
    p == r || p.starts_with(&(r + "\\"))
}
pub fn safe_location(path: &Path) -> Result<()> {
    absolute(path)?;
    let s = normal(&path.to_string_lossy());
    if s.contains("\\appdata\\")
        || s.split('\\')
            .any(|p| matches!(p, ".ssh" | ".aws" | "credentials"))
        || matches!(
            path.file_name().and_then(|s| s.to_str()),
            Some("auth.json" | ".npmrc" | ".env")
        )
    {
        return Err("SENSITIVE_PATH_REJECTED");
    }
    Ok(())
}

pub(crate) fn parents(path: &Path) -> Result<Vec<Guard>> {
    let mut out = Vec::new();
    for p in path.ancestors().skip(1) {
        out.push(Guard::open(p, true)?);
    }
    Ok(out)
}
#[cfg(windows)]
fn handle_identity(f: &File, expected: &Path) -> Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION,
    };
    let h = f.as_raw_handle() as _;
    let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(h, &mut info) } == 0
        || info.dwFileAttributes & (0x400 | 0x10) != 0
        || info.nNumberOfLinks != 1
    {
        return Err("TASK_IDENTITY_REJECTED");
    }
    let mut buffer = vec![0u16; 32768];
    let n = unsafe { GetFinalPathNameByHandleW(h, buffer.as_mut_ptr(), 32768, 0) } as usize;
    if n == 0 || n >= buffer.len() {
        return Err("TASK_IDENTITY_REJECTED");
    }
    let final_path = String::from_utf16(&buffer[..n]).map_err(|_| "TASK_IDENTITY_REJECTED")?;
    if Path::new(
        final_path
            .strip_prefix(r"\\?\")
            .ok_or("TASK_IDENTITY_REJECTED")?,
    ) != expected
    {
        return Err("TASK_PATH_CHANGED");
    }
    Ok(format!(
        "{:08x}:{:08x}{:08x}",
        info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
    ))
}
#[cfg(not(windows))]
fn handle_identity(_: &File, _: &Path) -> Result<String> {
    Err("WINDOWS_REQUIRED")
}

pub(crate) struct ExclusiveTarget {
    file: File,
    path: PathBuf,
    identity: String,
    _parents: Vec<Guard>,
}
impl ExclusiveTarget {
    pub(crate) fn open(path: &Path, identity: &str, before: &[u8]) -> Result<Self> {
        let pg = parents(path)?;
        // Resolve/reject aliases before opening; inspect again through the exclusive handle.
        {
            let g = Guard::open(path, false)?;
            if g.identity != identity || g.path != path {
                return Err("TASK_PATH_CHANGED");
            }
        }
        let mut opts = OpenOptions::new();
        opts.read(true).write(true);
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            opts.share_mode(0).custom_flags(0x00200000);
        }
        #[cfg(not(windows))]
        return Err("WINDOWS_REQUIRED");
        let file = opts.open(path).map_err(|_| "TASK_EXCLUSIVE_REJECTED")?;
        if handle_identity(&file, path)? != identity {
            return Err("TASK_IDENTITY_REJECTED");
        }
        let mut s = Self {
            file,
            path: path.into(),
            identity: identity.into(),
            _parents: pg,
        };
        if s.read()? != before {
            return Err("TASK_PREIMAGE_CHANGED");
        }
        Ok(s)
    }
    fn verify(&self) -> Result<()> {
        for p in &self._parents {
            p.verify()?;
        }
        if handle_identity(&self.file, &self.path)? != self.identity {
            return Err("TASK_IDENTITY_REJECTED");
        }
        Ok(())
    }
    pub(crate) fn read(&mut self) -> Result<Vec<u8>> {
        self.verify()?;
        io(self.file.seek(SeekFrom::Start(0)))?;
        let mut b = Vec::new();
        io((&mut self.file).take(65537).read_to_end(&mut b))?;
        if b.len() > 65536 {
            return Err("BOUND");
        }
        self.verify()?;
        Ok(b)
    }
    // Faults are crate-test-only values, not CLI flags or a runtime recovery API.
    pub(crate) fn write(&mut self, candidate: &[u8], fault: u8) -> Result<()> {
        self.verify()?;
        io(self.file.seek(SeekFrom::Start(0)))?;
        if fault == 3 {
            io(self.file.write_all(&candidate[..candidate.len() / 2]))?;
            return Err("TASK_WRITE_UNKNOWN");
        }
        io(self.file.write_all(candidate))?;
        if fault == 4 {
            return Err("TASK_TRUNCATE_UNKNOWN");
        }
        io(self.file.set_len(candidate.len() as u64))?;
        if fault == 5 {
            return Err("TASK_FLUSH_UNKNOWN");
        }
        io(self.file.sync_all())?;
        if self.read()? != candidate {
            return Err("TASK_WRITE_UNKNOWN");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub text: String,
    pub candidate_sha256: String,
    pub diff: String,
    pub diff_sha256: String,
    pub proposal_sha256: String,
    pub signature_preserved: bool,
}
fn unified(a: &str, b: &str, path: &str) -> String {
    let a: Vec<_> = a.split_inclusive('\n').collect();
    let b: Vec<_> = b.split_inclusive('\n').collect();
    let mut prefix = 0;
    while prefix < a.len().min(b.len()) && a[prefix] == b[prefix] {
        prefix += 1
    }
    let mut suffix = 0;
    while suffix
        < a.len()
            .saturating_sub(prefix)
            .min(b.len().saturating_sub(prefix))
        && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix]
    {
        suffix += 1
    }
    let start = prefix.saturating_sub(3);
    let ae = (a.len() - suffix + 3).min(a.len());
    let be = (b.len() - suffix + 3).min(b.len());
    let mut d = format!(
        "--- a/{path}\n+++ b/{path}\n@@ -{},{} +{},{} @@\n",
        start + 1,
        ae - start,
        start + 1,
        be - start
    );
    for line in &a[start..prefix] {
        d.push(' ');
        d.push_str(line);
        if !line.ends_with('\n') {
            d.push_str("\n\\ No newline at end of file\n")
        }
    }
    for line in &a[prefix..a.len() - suffix] {
        d.push('-');
        d.push_str(line);
        if !line.ends_with('\n') {
            d.push_str("\n\\ No newline at end of file\n")
        }
    }
    for line in &b[prefix..b.len() - suffix] {
        d.push('+');
        d.push_str(line);
        if !line.ends_with('\n') {
            d.push_str("\n\\ No newline at end of file\n")
        }
    }
    for line in &a[a.len() - suffix..ae] {
        d.push(' ');
        d.push_str(line);
        if !line.ends_with('\n') {
            d.push_str("\n\\ No newline at end of file\n")
        }
    }
    d
}
pub(crate) fn candidate_text(
    target: &str,
    base_sha256: &str,
    base: &[u8],
    raw: &[u8],
    sample_signature: bool,
) -> Result<Candidate> {
    if base.len() > 65536 || hash(base) != base_sha256 {
        return Err("BASE_CHANGED");
    }
    let b = std::str::from_utf8(base).map_err(|_| "BASE_ENCODING")?;
    if (b.contains('\0') || (sample_signature && b.contains('\r')))
        || (sample_signature && !b.lines().any(|l| l == RELAY_SIGNATURE))
    {
        return Err("BASE_SIGNATURE");
    }
    let proposal: Proposal = relay_json(raw, 32768)?;
    proposal.validate_target(target)?;
    let matches: Vec<_> = base
        .windows(proposal.old_text.len())
        .enumerate()
        .filter(|(_, v)| *v == proposal.old_text.as_bytes())
        .map(|(i, _)| i)
        .collect();
    if matches.is_empty() {
        return Err("PROPOSAL_NO_MATCH");
    }
    if matches.len() != 1 {
        return Err("PROPOSAL_AMBIGUOUS");
    }
    let at = matches[0];
    let text = format!(
        "{}{}{}",
        &b[..at],
        proposal.new_text,
        &b[at + proposal.old_text.len()..]
    );
    if text == b {
        return Err("PROPOSAL_NOOP");
    }
    if text.len() > 65536 {
        return Err("BOUND");
    }
    let signature_preserved = text.lines().filter(|l| *l == RELAY_SIGNATURE).count() == 1
        && b.lines().filter(|l| *l == RELAY_SIGNATURE).count() == 1;
    if sample_signature && !signature_preserved {
        return Err("SIGNATURE_CHANGED");
    }
    let diff = unified(b, &text, target);
    Ok(Candidate {
        candidate_sha256: hash(text.as_bytes()),
        text,
        diff_sha256: hash(diff.as_bytes()),
        diff,
        proposal_sha256: hash(raw),
        signature_preserved,
    })
}

const SENTINEL: &[u8] = b"NOVA_INJECTED_ONLY_SECRET_SENTINEL";
pub struct SecretLease(Vec<u8>);
impl SecretLease {
    pub(crate) fn real(bytes: Vec<u8>) -> Result<Self> {
        if bytes.len() < 8 || bytes.len() > 4096 || bytes.iter().any(|b| b.is_ascii_control()) {
            return Err("CREDENTIAL_REJECTED");
        }
        Ok(Self(bytes))
    }
    pub(crate) fn bearer(&self) -> Result<&str> {
        std::str::from_utf8(&self.0).map_err(|_| "CREDENTIAL_REJECTED")
    }
    pub(crate) fn injected() -> Self {
        Self(SENTINEL.to_vec())
    }
    pub fn scan_text(&self, b: &[u8]) -> Result<()> {
        if b.windows(self.0.len()).any(|x| x == self.0.as_slice()) {
            Err("SECRET_ECHO")
        } else {
            Ok(())
        }
    }
    pub fn scan_json(&self, b: &[u8], cap: usize) -> Result<Value> {
        self.scan_text(b)?;
        let v: Value = api_json(b, cap)?;
        self.walk(&v, 0)?;
        Ok(v)
    }
    // Only the response envelope defines JSON-encoded assistant content.
    // A field named content inside a proposal remains ordinary code/text.
    pub(crate) fn scan_response(&self, b: &[u8]) -> Result<Value> {
        let value = self.scan_json(b, 131072)?;
        if let Some(choices) = value["choices"].as_array() {
            for choice in choices {
                if let Some(content) = choice["message"]["content"].as_str() {
                    self.scan_json(content.as_bytes(), 65536)?;
                }
            }
        }
        Ok(value)
    }
    fn walk(&self, v: &Value, depth: usize) -> Result<()> {
        if depth > 16 {
            return Err("BOUND");
        }
        match v {
            Value::String(s) => {
                self.scan_text(s.as_bytes())?;
            }
            Value::Array(a) => {
                for x in a {
                    self.walk(x, depth + 1)?
                }
            }
            Value::Object(m) => {
                for (k, x) in m {
                    self.scan_text(k.as_bytes())?;
                    self.walk(x, depth + 1)?
                }
            }
            _ => (),
        }
        Ok(())
    }
}
impl Drop for SecretLease {
    fn drop(&mut self) {
        self.0.fill(0)
    }
}
