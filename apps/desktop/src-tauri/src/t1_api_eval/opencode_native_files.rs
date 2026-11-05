//! Frozen expectation -> preparation receipt -> independent collection -> mandatory checker.
use super::{
    hash,
    opencode_paths::{FileFact, FixtureReservation, RootReservation, TreeSnapshot},
    Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FactSource {
    Injected,
    NativePrepared,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedEntry {
    pub path: String,
    pub owner: String,
    pub dacl: String,
    pub attributes: u32,
    pub default_sha256: String,
    pub ads: Vec<(String, u64, String)>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenFixturePolicy {
    pub schema: String,
    pub source: FactSource,
    pub root: String,
    pub sid: String,
    pub entries: Vec<ExpectedEntry>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedFixtureReceipt {
    pub source: FactSource,
    pub policy_sha256: String,
    pub generation: String,
    pub filesystem: String,
    pub identities: BTreeMap<String, String>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnumerationReceipt {
    pub source: FactSource,
    pub root_identity: String,
    pub started: u64,
    pub finished: u64,
    pub entries: u32,
    pub default_bytes: u64,
    pub ads_bytes: u64,
    pub directories_ended: u32,
    pub streams_ended: u32,
    pub error: Option<String>,
}
pub const MAX_ENTRIES: usize = 4096;
pub const MAX_DEPTH: usize = 32;
pub const MAX_FILE: usize = 8 * 1024 * 1024;
pub const MAX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_ADS: usize = 16;
pub const MAX_STREAM: usize = 1024 * 1024;
pub const MAX_ADS_BYTES: usize = 8 * 1024 * 1024;
pub const DACL_MASK: u16 = 0x150c;
pub fn relative(p: &str) -> bool {
    p == "."
        || (p.is_ascii()
            && !p.is_empty()
            && p.encode_utf16().count() <= 2048
            && p.split('/').count() <= MAX_DEPTH
            && p.split('/').all(|s| {
                !s.is_empty()
                    && s != "."
                    && s != ".."
                    && !s.ends_with([' ', '.'])
                    && !s.contains([':', '\\', '\0'])
                    && s.chars().all(|c| !c.is_control())
            }))
}
pub fn canonical_acl(bytes: &[u8], used: usize, mask: u16) -> Result<String> {
    if used < 8
        || used > 65536
        || used > bytes.len()
        || u16::from_le_bytes([bytes[2], bytes[3]]) as usize > bytes.len()
        || used > u16::from_le_bytes([bytes[2], bytes[3]]) as usize
    {
        return Err("ACL length UNKNOWN".into());
    }
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    let mut at = 8;
    for _ in 0..count {
        if at + 4 > used {
            return Err("ACL ACE header".into());
        }
        let n = u16::from_le_bytes([bytes[at + 2], bytes[at + 3]]) as usize;
        if n < 4 || n % 4 != 0 {
            return Err("ACL ACE length".into());
        }
        at = at.checked_add(n).ok_or("ACL overflow")?;
        if at > used {
            return Err("ACL ACE truncated".into());
        }
    }
    if at != used {
        return Err("ACL used boundary".into());
    }
    let mut v = bytes[..used].to_vec();
    v[2..4].copy_from_slice(&(used as u16).to_le_bytes());
    Ok(format!(
        "{:04x}:{}",
        mask & DACL_MASK,
        v.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
impl FrozenFixturePolicy {
    pub fn digest(&self) -> Result<String> {
        Ok(hash(
            &serde_json::to_vec(self).map_err(|_| "policy encoding")?,
        ))
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != "NB2_FIXTURE_V1"
            || self.root != super::opencode_paths::ROOT
            || self.sid.is_empty()
            || self.entries.is_empty()
            || self.entries.len() > MAX_ENTRIES
        {
            return Err("frozen policy incomplete".into());
        }
        if self.source == FactSource::NativePrepared && !self.sid.starts_with("S-1-") {
            return Err("native SID required".into());
        }
        let mut seen = BTreeSet::new();
        for e in &self.entries {
            if !relative(&e.path)
                || !seen.insert(e.path.to_ascii_uppercase())
                || e.owner != self.sid
                || e.dacl.is_empty()
                || e.attributes & 0x400 != 0
                || (e.default_sha256.len() != 64
                    || !e
                        .default_sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
                || e.ads.len() > MAX_ADS
            {
                return Err("entry policy rejected".into());
            }
            if self.source == FactSource::NativePrepared {
                let (flags, acl) = e.dacl.split_once(':').ok_or("DACL format")?;
                if flags.len() != 4
                    || acl.len() < 16
                    || acl.len() % 2 != 0
                    || !flags
                        .bytes()
                        .chain(acl.bytes())
                        .all(|b| b.is_ascii_hexdigit())
                {
                    return Err("native ACL required".into());
                }
            }
            let mut streams = BTreeSet::new();
            for (n, size, digest) in &e.ads {
                if n.is_empty()
                    || n.contains([':', '/', '\\', '\0'])
                    || !streams.insert(n.to_uppercase())
                    || *size > MAX_STREAM as u64
                    || digest.len() != 64
                {
                    return Err("ADS policy rejected".into());
                }
            }
            if ["Logo.tsx", ".git/opencode"].contains(&e.path.as_str()) && !e.ads.is_empty() {
                return Err("target ADS must be empty".into());
            }
        }
        for p in [".", ".git", ".git/config", ".git/opencode", "Logo.tsx"] {
            if !self.entries.iter().any(|e| e.path == p) {
                return Err("required policy entry".into());
            }
        }
        Ok(())
    }
}
impl PreparedFixtureReceipt {
    pub fn validate(&self, p: &FrozenFixturePolicy) -> Result<()> {
        p.validate()?;
        if self.source != p.source
            || self.policy_sha256 != p.digest()?
            || self.filesystem != "NTFS"
            || self.generation.is_empty()
            || self.identities.len() != p.entries.len()
        {
            return Err("preparation policy binding".into());
        }
        for e in &p.entries {
            let id = self
                .identities
                .get(&e.path)
                .ok_or("preparation identity missing")?;
            if id.is_empty()
                || (self.source == FactSource::NativePrepared
                    && (id.starts_with("INJECTED") || !id.contains(':')))
            {
                return Err("identity source".into());
            }
        }
        Ok(())
    }
}
/// This is called by run's baseline and post-agent checkers, not left to a backend verdict.
pub fn validate_collected_snapshot(
    s: &TreeSnapshot,
    r: &FixtureReservation,
    content_baseline: bool,
    expected: FactSource,
) -> Result<()> {
    let p = r.policy.as_ref().ok_or("frozen policy missing")?;
    let prepared = r.prepared.as_ref().ok_or("prepared receipt missing")?;
    prepared.validate(p)?;
    let proof = s
        .enumeration
        .as_ref()
        .ok_or("enumeration receipt missing")?;
    if p.source != expected || prepared.source != expected || proof.source != expected {
        return Err("snapshot source differs from expected run mode".into());
    }
    let dir_count = s.files.iter().filter(|f| f.attributes & 0x10 != 0).count();
    if s.complete != Some(true)
        || proof.error.is_some()
        || proof.source != p.source
        || proof.entries as usize != s.files.len()
        || s.files.len() != p.entries.len()
        || proof.finished < proof.started
        || proof.finished - proof.started > 10000
        || proof.streams_ended as usize != s.files.len()
        || proof.directories_ended as usize != dir_count
        || proof.root_identity != r.root.identity
        || prepared.identities.get(".") != Some(&r.root.identity)
    {
        return Err("enumeration UNKNOWN/incomplete/source".into());
    }
    let mut paths = BTreeSet::new();
    let (mut bytes, mut ads_bytes) = (0usize, 0usize);
    for f in &s.files {
        if !relative(&f.path)
            || !paths.insert(f.path.to_ascii_uppercase())
            || f.links != 1
            || f.attributes & 0x400 != 0
            || f.bytes.len() > MAX_FILE
            || f.ads.len() > MAX_ADS
        {
            return Err("file fact bound/identity".into());
        }
        let e = p
            .entries
            .iter()
            .find(|e| e.path == f.path)
            .ok_or("unapproved entry")?;
        if f.owner != e.owner
            || f.dacl != e.dacl
            || f.attributes != e.attributes
            || prepared.identities.get(&f.path) != Some(&f.identity)
        {
            return Err("owner/DACL/attributes/identity differs from frozen expectation".into());
        }
        if content_baseline && hash(&f.bytes) != e.default_sha256 {
            return Err("baseline content differs".into());
        }
        let mut actual = Vec::new();
        for (n, b) in &f.ads {
            if b.len() > MAX_STREAM {
                return Err("ADS size".into());
            }
            ads_bytes = ads_bytes.checked_add(b.len()).ok_or("ADS overflow")?;
            actual.push((n.clone(), b.len() as u64, hash(b)));
        }
        actual.sort();
        let mut expected = e.ads.clone();
        expected.sort();
        if actual != expected {
            return Err("ADS expectation differs".into());
        }
        bytes = bytes
            .checked_add(f.bytes.len())
            .ok_or("snapshot overflow")?;
    }
    if bytes > MAX_BYTES
        || ads_bytes > MAX_ADS_BYTES
        || proof.default_bytes != bytes as u64
        || proof.ads_bytes != ads_bytes as u64
    {
        return Err("snapshot counters UNKNOWN".into());
    }
    Ok(())
}
/// Reading a file may change access time. Nothing else is implicitly forgiven.
pub fn same_except_access(a: &FileFact, b: &FileFact) -> bool {
    let mut b = b.clone();
    b.access = a.access;
    *a == b
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReadStamp {
    pub identity: String,
    pub size: u64,
    pub write: u64,
    pub change: u64,
}
pub fn unchanged_read(before: &ReadStamp, after: &ReadStamp) -> Result<()> {
    if before == after {
        Ok(())
    } else {
        Err("content read changed identity/size/write/change UNKNOWN".into())
    }
}
#[cfg(windows)]
pub mod native {
    use super::*;
    use std::{
        fs::File,
        io::Read,
        os::windows::io::{AsRawHandle, FromRawHandle},
        path::{Path, PathBuf},
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::{Authorization::*, *},
        Storage::FileSystem::*,
    };
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }
    fn api(b: BOOL) -> Result<()> {
        if b == 0 {
            Err("file API UNKNOWN".into())
        } else {
            Ok(())
        }
    }
    pub fn sid_string(sid: PSID) -> Result<String> {
        unsafe {
            if sid.is_null() || IsValidSid(sid) == 0 {
                return Err("invalid SID".into());
            }
            let n = *GetSidSubAuthorityCount(sid) as usize;
            if n > 15 {
                return Err("SID bound".into());
            }
            let authority = &*GetSidIdentifierAuthority(sid);
            let mut a = 0u64;
            for b in authority.Value {
                a = (a << 8) | b as u64
            }
            let mut s = format!("S-1-{a}");
            for i in 0..n {
                s.push_str(&format!("-{}", *GetSidSubAuthority(sid, i as u32)))
            }
            Ok(s)
        }
    }
    struct Freed(HLOCAL);
    impl Drop for Freed {
        fn drop(&mut self) {
            unsafe {
                LocalFree(self.0);
            }
        }
    }
    fn open(path: &Path, dir: bool) -> Result<File> {
        open_impl(path, Some(dir))
    }
    fn open_impl(path: &Path, dir: Option<bool>) -> Result<File> {
        unsafe {
            let text = path.to_str().ok_or("path encoding")?;
            if !path.is_absolute()
                || text.contains('\0')
                || text.contains("..")
                || text.starts_with(r"\\")
                || text.ends_with([' ', '.'])
            {
                return Err("noncanonical local path".into());
            }
            let h = CreateFileW(
                wide(text).as_ptr(),
                GENERIC_READ | READ_CONTROL,
                FILE_SHARE_READ,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT
                    | if dir != Some(false) {
                        FILE_FLAG_BACKUP_SEMANTICS
                    } else {
                        FILE_ATTRIBUTE_NORMAL
                    },
                std::ptr::null_mut(),
            );
            if h == INVALID_HANDLE_VALUE {
                return Err("open/ownership UNKNOWN".into());
            }
            let f = File::from_raw_handle(h as _);
            let m = meta(&f)?;
            if m.attrs & FILE_ATTRIBUTE_REPARSE_POINT != 0
                || dir.is_some_and(|dir| (m.attrs & FILE_ATTRIBUTE_DIRECTORY != 0) != dir)
            {
                return Err("reparse/type rejected".into());
            }
            let mut b = vec![0u16; 32768];
            let n = GetFinalPathNameByHandleW(h, b.as_mut_ptr(), b.len() as u32, 0);
            if n == 0 || n as usize >= b.len() {
                return Err("final path UNKNOWN".into());
            }
            let got = String::from_utf16(&b[..n as usize]).map_err(|_| "path UTF16")?;
            let got = got.strip_prefix(r"\\?\").ok_or("final local path")?;
            if !got.eq_ignore_ascii_case(text) {
                return Err("path alias rejected".into());
            }
            Ok(f)
        }
    }
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Meta {
        id: String,
        size: u64,
        attrs: u32,
        links: u32,
        creation: u64,
        access: u64,
        write: u64,
        change: u64,
    }
    fn meta(f: &File) -> Result<Meta> {
        unsafe {
            let h = f.as_raw_handle() as HANDLE;
            let mut id: BY_HANDLE_FILE_INFORMATION = std::mem::zeroed();
            api(GetFileInformationByHandle(h, &mut id))?;
            let mut basic: FILE_BASIC_INFO = std::mem::zeroed();
            api(GetFileInformationByHandleEx(
                h,
                FileBasicInfo,
                &mut basic as *mut _ as _,
                std::mem::size_of_val(&basic) as u32,
            ))?;
            let mut standard: FILE_STANDARD_INFO = std::mem::zeroed();
            api(GetFileInformationByHandleEx(
                h,
                FileStandardInfo,
                &mut standard as *mut _ as _,
                std::mem::size_of_val(&standard) as u32,
            ))?;
            if standard.EndOfFile < 0 {
                return Err("negative stream length".into());
            }
            Ok(Meta {
                id: format!(
                    "{:08x}:{:08x}{:08x}",
                    id.dwVolumeSerialNumber, id.nFileIndexHigh, id.nFileIndexLow
                ),
                size: standard.EndOfFile as u64,
                attrs: id.dwFileAttributes,
                links: id.nNumberOfLinks,
                creation: basic.CreationTime as u64,
                access: basic.LastAccessTime as u64,
                write: basic.LastWriteTime as u64,
                change: basic.ChangeTime as u64,
            })
        }
    }
    fn stable(a: &Meta, b: &Meta) -> bool {
        unchanged_read(
            &ReadStamp {
                identity: a.id.clone(),
                size: a.size,
                write: a.write,
                change: a.change,
            },
            &ReadStamp {
                identity: b.id.clone(),
                size: b.size,
                write: b.write,
                change: b.change,
            },
        )
        .is_ok()
            && a.creation == b.creation
            && a.attrs == b.attrs
            && a.links == b.links
    }
    fn security(f: &File) -> Result<(String, String)> {
        unsafe {
            let (mut owner, mut acl, mut sd) = (
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            let rc = GetSecurityInfo(
                f.as_raw_handle() as HANDLE,
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                &mut owner,
                std::ptr::null_mut(),
                &mut acl,
                std::ptr::null_mut(),
                &mut sd,
            );
            if rc != 0 || sd.is_null() {
                return Err("security query UNKNOWN".into());
            }
            let _sd = Freed(sd as _);
            let sd_bytes = GetSecurityDescriptorLength(sd) as usize;
            let base = sd as usize;
            let ptr = acl as usize;
            if sd_bytes < 20
                || sd_bytes > 65536
                || ptr < base
                || ptr.checked_add(8).is_none_or(|end| end > base + sd_bytes)
            {
                return Err("security descriptor range".into());
            }
            if acl.is_null() {
                return Err("null DACL rejected".into());
            }
            let mut ctrl = 0;
            let mut revision = 0;
            api(GetSecurityDescriptorControl(sd, &mut ctrl, &mut revision))?;
            if ctrl & 4 == 0 {
                return Err("absent DACL".into());
            }
            let mut size: ACL_SIZE_INFORMATION = std::mem::zeroed();
            api(GetAclInformation(
                acl,
                &mut size as *mut _ as _,
                std::mem::size_of_val(&size) as u32,
                AclSizeInformation,
            ))?;
            let capacity = (*acl).AclSize as usize;
            if capacity < 8
                || capacity > 65535
                || ptr
                    .checked_add(capacity)
                    .is_none_or(|end| end > base + sd_bytes)
            {
                return Err("ACL capacity".into());
            }
            let bytes = std::slice::from_raw_parts(acl as *const u8, capacity);
            Ok((
                sid_string(owner)?,
                canonical_acl(bytes, size.AclBytesInUse as usize, ctrl)?,
            ))
        }
    }
    pub fn verify_root(r: &RootReservation) -> Result<()> {
        let f = open(Path::new(&r.path), true)?;
        if meta(&f)?.id != r.identity {
            return Err("root identity changed".into());
        }
        Ok(())
    }
    pub fn guard_image(path: &Path, digest: &str) -> Result<File> {
        let mut f = open(path, false)?;
        let a = meta(&f)?;
        if a.links != 1 || a.size > 128 * 1024 * 1024 {
            return Err("image bound/link".into());
        }
        let mut b = Vec::new();
        (&mut f)
            .take(128 * 1024 * 1024 + 1)
            .read_to_end(&mut b)
            .map_err(|_| "image read")?;
        if hash(&b) != digest || !stable(&a, &meta(&f)?) {
            return Err("image hash/identity UNKNOWN".into());
        }
        Ok(f)
    }
    pub fn verify_image_guard(guard: &File, path: &Path) -> Result<()> {
        let by_name = open(path, false)?;
        if !stable(&meta(guard)?, &meta(&by_name)?) {
            return Err("guarded image identity changed".into());
        }
        Ok(())
    }
    struct Find(HANDLE);
    impl Drop for Find {
        fn drop(&mut self) {
            unsafe {
                FindClose(self.0);
            }
        }
    }
    fn stream_facts(path: &Path, deadline: Instant) -> Result<Vec<(String, Vec<u8>)>> {
        unsafe {
            let mut data: WIN32_FIND_STREAM_DATA = std::mem::zeroed();
            let h = FindFirstStreamW(
                wide(path.to_str().ok_or("path encoding")?).as_ptr(),
                FindStreamInfoStandard,
                &mut data as *mut _ as _,
                0,
            );
            if h == INVALID_HANDLE_VALUE {
                if GetLastError() == ERROR_HANDLE_EOF {
                    return Ok(vec![]);
                }
                return Err("stream enumeration UNKNOWN".into());
            }
            let _find = Find(h);
            let mut out: Vec<(String, Vec<u8>)> = Vec::new();
            let mut seen = BTreeSet::new();
            loop {
                if Instant::now() >= deadline {
                    return Err("stream deadline".into());
                }
                let end = data
                    .cStreamName
                    .iter()
                    .position(|v| *v == 0)
                    .ok_or("stream name bound")?;
                let n =
                    String::from_utf16(&data.cStreamName[..end]).map_err(|_| "stream encoding")?;
                if n != "::$DATA" {
                    let n = n
                        .strip_prefix(':')
                        .and_then(|n| n.strip_suffix(":$DATA"))
                        .ok_or("stream format")?;
                    if n.is_empty()
                        || n.contains([':', '/', '\\'])
                        || !seen.insert(n.to_uppercase())
                        || out.len() >= MAX_ADS
                        || data.StreamSize < 0
                        || data.StreamSize as usize > MAX_STREAM
                    {
                        return Err("stream bound/alias".into());
                    }
                    let stream_path = PathBuf::from(format!("{}:{n}", path.display()));
                    let mut f = open_impl(&stream_path, None)?;
                    let before = meta(&f)?;
                    let mut b = Vec::new();
                    (&mut f)
                        .take((MAX_STREAM + 1) as u64)
                        .read_to_end(&mut b)
                        .map_err(|_| "stream read")?;
                    if b.len() > MAX_STREAM
                        || b.len() as u64 != before.size
                        || !stable(&before, &meta(&f)?)
                    {
                        return Err("stream changed during read".into());
                    }
                    out.push((n.into(), b));
                }
                if FindNextStreamW(h, &mut data as *mut _ as _) == 0 {
                    if GetLastError() != ERROR_HANDLE_EOF {
                        return Err("stream end UNKNOWN".into());
                    }
                    break;
                }
            }
            out.sort_by(|a, b| a.0.cmp(&b.0));
            Ok(out)
        }
    }
    pub struct FixtureFactCollector;
    impl FixtureFactCollector {
        pub fn collect(
            policy: &FrozenFixturePolicy,
            prepared: &PreparedFixtureReceipt,
            origin: Instant,
            outer: Instant,
        ) -> Result<TreeSnapshot> {
            policy.validate()?;
            prepared.validate(policy)?;
            if policy.source != FactSource::NativePrepared {
                return Err("native collector rejects injected policy".into());
            }
            let start = Instant::now();
            let deadline = outer.min(start + Duration::from_secs(10));
            if start >= deadline {
                return Err("snapshot budget exhausted".into());
            }
            let root = Path::new(&policy.root);
            let guard = open(root, true)?;
            let root_id = meta(&guard)?.id;
            unsafe {
                let mut fs = [0u16; 32];
                let mut serial = 0;
                let mut max = 0;
                let mut flags = 0;
                api(GetVolumeInformationByHandleW(
                    guard.as_raw_handle() as HANDLE,
                    std::ptr::null_mut(),
                    0,
                    &mut serial,
                    &mut max,
                    &mut flags,
                    fs.as_mut_ptr(),
                    fs.len() as u32,
                ))?;
                let n = fs
                    .iter()
                    .position(|x| *x == 0)
                    .ok_or("filesystem name bound")?;
                if String::from_utf16(&fs[..n]).map_err(|_| "filesystem encoding")? != "NTFS" {
                    return Err("NTFS required".into());
                }
            }
            if prepared.identities.get(".") != Some(&root_id) {
                return Err("prepared root mismatch".into());
            }
            let mut pending = vec![(".".to_string(), true)];
            let mut files = Vec::new();
            let (mut total, mut ads_total, mut directories) = (0usize, 0usize, 0u32);
            while let Some((relative_path, dir)) = pending.pop() {
                if !relative(&relative_path)
                    || files.len() >= MAX_ENTRIES
                    || Instant::now() >= deadline
                {
                    return Err("enumeration bound/deadline".into());
                }
                let path = if relative_path == "." {
                    root.to_path_buf()
                } else {
                    root.join(&relative_path)
                };
                let mut f = if relative_path == "." {
                    guard.try_clone().map_err(|_| "root duplicate")?
                } else {
                    open(&path, dir)?
                };
                let before = meta(&f)?;
                let (owner, dacl) = security(&f)?;
                let mut bytes = Vec::new();
                if !dir {
                    if before.size > MAX_FILE as u64 {
                        return Err("default stream bound".into());
                    }
                    (&mut f)
                        .take((MAX_FILE + 1) as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|_| "default stream read")?;
                    if bytes.len() > MAX_FILE || bytes.len() as u64 != before.size {
                        return Err("short/oversize default stream".into());
                    }
                }
                let ads = stream_facts(&path, deadline)?;
                if dir {
                    for e in std::fs::read_dir(&path).map_err(|_| "directory start UNKNOWN")? {
                        let e = e.map_err(|_| "directory end UNKNOWN")?;
                        let name = e.file_name().into_string().map_err(|_| "directory UTF16")?;
                        let rel = if relative_path == "." {
                            name
                        } else {
                            format!("{relative_path}/{name}")
                        };
                        if !relative(&rel) || pending.len() + files.len() >= MAX_ENTRIES {
                            return Err("directory bound".into());
                        }
                        let kind = e.file_type().map_err(|_| "entry type")?;
                        if kind.is_symlink() || (!kind.is_file() && !kind.is_dir()) {
                            return Err("entry reparse/type".into());
                        }
                        pending.push((rel, kind.is_dir()));
                    }
                    directories = directories.checked_add(1).ok_or("directory overflow")?;
                }
                let after = meta(&f)?;
                if !stable(&before, &after)
                    || security(&f)? != (owner.clone(), dacl.clone())
                    || Instant::now() >= deadline
                {
                    return Err("file changed while collecting UNKNOWN".into());
                }
                total = total.checked_add(bytes.len()).ok_or("byte overflow")?;
                for (_, b) in &ads {
                    ads_total = ads_total.checked_add(b.len()).ok_or("ADS overflow")?
                }
                if total > MAX_BYTES || ads_total > MAX_ADS_BYTES {
                    return Err("snapshot byte bound".into());
                }
                files.push(FileFact {
                    path: relative_path,
                    bytes,
                    identity: before.id,
                    owner,
                    dacl,
                    attributes: before.attrs,
                    ads,
                    links: before.links,
                    creation: before.creation,
                    access: before.access,
                    write: before.write,
                    change: before.change,
                });
            }
            if meta(&guard)?.id != root_id || Instant::now() >= deadline {
                return Err("root/deadline changed".into());
            }
            // Reopen by name and cross-check every previously sampled identity/metadata.
            // This detects observed replacement/change; it is not a namespace-wide atomic snapshot.
            for fact in &files {
                if Instant::now() >= deadline {
                    return Err("final cross-check deadline".into());
                }
                let path = if fact.path == "." {
                    root.to_path_buf()
                } else {
                    root.join(&fact.path)
                };
                let f = open(&path, fact.attributes & FILE_ATTRIBUTE_DIRECTORY != 0)?;
                let m = meta(&f)?;
                if m.id != fact.identity
                    || m.size != fact.bytes.len() as u64
                        && fact.attributes & FILE_ATTRIBUTE_DIRECTORY == 0
                    || m.creation != fact.creation
                    || m.write != fact.write
                    || m.change != fact.change
                    || m.attrs != fact.attributes
                    || m.links != fact.links
                    || security(&f)? != (fact.owner.clone(), fact.dacl.clone())
                {
                    return Err("final identity/metadata changed UNKNOWN".into());
                }
            }
            files.sort_by(|a, b| a.path.cmp(&b.path));
            let elapsed = start.elapsed().as_millis() as u64;
            let started = start
                .checked_duration_since(origin)
                .ok_or("monotonic origin")?
                .as_millis() as u64;
            let receipt = EnumerationReceipt {
                source: FactSource::NativePrepared,
                root_identity: root_id,
                started,
                finished: started.checked_add(elapsed).ok_or("clock overflow")?,
                entries: files.len() as u32,
                default_bytes: total as u64,
                ads_bytes: ads_total as u64,
                directories_ended: directories,
                streams_ended: files.len() as u32,
                error: None,
            };
            Ok(TreeSnapshot {
                complete: Some(true),
                files,
                enumeration: Some(receipt),
            })
        }
    }
    #[cfg(test)]
    pub(crate) fn read_fixed_native_fixture() -> Result<()> {
        // Ignored separately-authorized test: creates only a new child under explicit own test root.
        let parent = std::env::var_os("NOVA_NB2_TEST_ROOT").ok_or("own test root required")?;
        let root = PathBuf::from(parent).join("native-file-fixture");
        std::fs::create_dir(&root).map_err(|_| "native fixture already exists/creation failed")?;
        let path = root.join("fixed.txt");
        std::fs::write(&path, b"fixture").map_err(|_| "fixture write")?;
        std::fs::write(root.join("fixed.txt:probe"), b"stream").map_err(|_| "fixture ADS write")?;
        let mut f = open(&path, false)?;
        let before = meta(&f)?;
        let (owner, dacl) = security(&f)?;
        let mut b = Vec::new();
        f.read_to_end(&mut b).map_err(|_| "fixture read")?;
        let ads = stream_facts(&path, Instant::now() + Duration::from_secs(1))?;
        let after = meta(&f)?;
        assert!(stable(&before, &after));
        assert_eq!(b, b"fixture");
        assert_eq!(ads, vec![("probe".into(), b"stream".to_vec())]);
        assert!(owner.starts_with("S-1-"));
        assert!(!dacl.is_empty());
        // This is raw IO evidence only; never produces an approved expected policy from these facts.
        Ok(())
    }
}
