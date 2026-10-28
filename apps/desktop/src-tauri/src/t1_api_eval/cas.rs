//! Windows same-handle CAS + scoped, non-atomic persistent tree manifests.
use super::{
    hash,
    journal::{write_new, Receipt, Transaction},
    Result,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fault {
    None,
    ShortWrite,
    Flush,
    CrashAfterIntent,
    Receipt,
}

#[cfg(windows)]
mod native {
    use super::*;
    use std::os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows_sys::Win32::{Foundation::*, Storage::FileSystem::*};
    pub fn open(path: &Path, write: bool, dir: bool) -> Result<File> {
        let mut o = OpenOptions::new();
        o.read(true)
            .write(write)
            .share_mode(FILE_SHARE_READ)
            .custom_flags(
                FILE_FLAG_OPEN_REPARSE_POINT | if dir { FILE_FLAG_BACKUP_SEMANTICS } else { 0 },
            );
        o.open(path).map_err(|e| e.to_string())
    }
    pub fn identity(f: &File) -> Result<String> {
        let mut info: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
        if unsafe { GetFileInformationByHandle(f.as_raw_handle() as _, &mut info) } == 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 && info.nNumberOfLinks != 1)
        {
            return Err("reparse/hardlink rejected".into());
        }
        Ok(format!(
            "{:08x}:{:08x}{:08x}",
            info.dwVolumeSerialNumber, info.nFileIndexHigh, info.nFileIndexLow
        ))
    }
    pub fn attributes(path: &Path) -> Result<u32> {
        Ok(fs::symlink_metadata(path)
            .map_err(|e| e.to_string())?
            .file_attributes())
    }
    pub fn filesystem(f: &File) -> Result<String> {
        let mut name = [0u16; 64];
        if unsafe {
            GetVolumeInformationByHandleW(
                f.as_raw_handle() as _,
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                name.as_mut_ptr(),
                name.len() as u32,
            )
        } == 0
        {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let n = name.iter().position(|x| *x == 0).ok_or("invalid FS name")?;
        String::from_utf16(&name[..n]).map_err(|e| e.to_string())
    }
    pub fn streams(path: &Path) -> Result<Vec<Stream>> {
        let wide: Vec<_> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut data: WIN32_FIND_STREAM_DATA = unsafe { std::mem::zeroed() };
        let h = unsafe {
            FindFirstStreamW(
                wide.as_ptr(),
                FindStreamInfoStandard,
                (&mut data as *mut WIN32_FIND_STREAM_DATA).cast(),
                0,
            )
        };
        if h == INVALID_HANDLE_VALUE {
            let e = unsafe { GetLastError() };
            return if e == ERROR_HANDLE_EOF {
                Ok(Vec::new())
            } else {
                Err(format!("ADS NOT ENUMERATED win32={e}"))
            };
        }
        let result = (|| {
            let mut streams = Vec::new();
            loop {
                let end = data
                    .cStreamName
                    .iter()
                    .position(|c| *c == 0)
                    .ok_or("unterminated stream name")?;
                let name =
                    String::from_utf16(&data.cStreamName[..end]).map_err(|e| e.to_string())?;
                if name != "::$DATA" {
                    if !name.starts_with(':')
                        || !name.ends_with(":$DATA")
                        || data.StreamSize < 0
                        || data.StreamSize > 1048576
                        || streams.len() >= 64
                    {
                        return Err("invalid ADS structure".into());
                    }
                    let mut p = path.as_os_str().to_os_string();
                    p.push(&name);
                    let mut f = open(Path::new(&p), false, false)?;
                    let mut bytes = Vec::new();
                    f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                    if bytes.len() as i64 != data.StreamSize {
                        return Err("stream changed during enumeration".into());
                    }
                    streams.push(Stream {
                        name,
                        bytes: bytes.len() as u64,
                        sha256: hash(&bytes),
                    });
                }
                if unsafe { FindNextStreamW(h, (&mut data as *mut WIN32_FIND_STREAM_DATA).cast()) }
                    == 0
                {
                    let e = unsafe { GetLastError() };
                    if e != ERROR_HANDLE_EOF {
                        return Err(format!("ADS NOT ENUMERATED win32={e}"));
                    }
                    break;
                }
            }
            streams.sort_by(|a, b| a.name.cmp(&b.name));
            Ok(streams)
        })();
        unsafe { FindClose(h) };
        result
    }
}
#[cfg(not(windows))]
mod native {
    use super::*;
    pub fn open(_: &Path, _: bool, _: bool) -> Result<File> {
        Err("Windows required".into())
    }
    pub fn identity(_: &File) -> Result<String> {
        Err("Windows required".into())
    }
    pub fn attributes(_: &Path) -> Result<u32> {
        Err("NOT ENUMERATED".into())
    }
    pub fn filesystem(_: &File) -> Result<String> {
        Err("NOT ENUMERATED".into())
    }
    pub fn streams(_: &Path) -> Result<Vec<Stream>> {
        Err("NOT ENUMERATED".into())
    }
}
pub fn locked_identity(path: &Path) -> Result<String> {
    let f = native::open(path, false, false)?;
    native::identity(&f)
}
fn no_link_ancestors(path: &Path) -> Result<()> {
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err("parent path rejected".into());
    }
    for p in path.ancestors() {
        if native::attributes(p)? & 0x400 != 0 {
            return Err("reparse ancestor".into());
        }
    }
    Ok(())
}
fn read(f: &mut File) -> Result<Vec<u8>> {
    f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    if f.metadata().map_err(|e| e.to_string())?.len() > 4096 {
        return Err("CAS file too large".into());
    }
    let mut b = Vec::new();
    f.read_to_end(&mut b).map_err(|e| e.to_string())?;
    Ok(b)
}
pub fn apply(
    path: &Path,
    expected: &[u8],
    post: &[u8],
    tx: &Transaction,
    fault: Fault,
) -> Result<Receipt> {
    if expected != tx.pre || post != tx.post || post.len() > 4096 {
        return Err("transaction bytes mismatch".into());
    }
    no_link_ancestors(path)?;
    let mut f = native::open(path, true, false)?;
    let id = native::identity(&f)?;
    // Reject replacement since reservation, including same-byte path substitution.
    let reserved_path = tx.root.join("target-identity.json");
    if reserved_path.exists() {
        let expected_id: serde_json::Value =
            serde_json::from_slice(&fs::read(&reserved_path).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        if expected_id["identity"] != id {
            return Err("identity changed".into());
        }
    } else {
        return Err("target identity must be reserved before CAS".into());
    }
    if read(&mut f)? != expected {
        return Err("CAS preimage conflict".into());
    }
    tx.intent(&id)?;
    if fault == Fault::CrashAfterIntent {
        return Err("UNCERTAIN simulated crash after intent".into());
    }
    f.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    if fault == Fault::ShortWrite {
        f.write_all(&post[..post.len() / 2])
            .map_err(|e| e.to_string())?;
        return Err("UNCERTAIN short write".into());
    }
    f.write_all(post)
        .and_then(|_| f.set_len(post.len() as u64))
        .map_err(|e| format!("UNCERTAIN write: {e}"))?;
    if fault == Fault::Flush {
        return Err("UNCERTAIN injected Flush failure".into());
    }
    f.sync_all().map_err(|e| format!("UNCERTAIN Flush: {e}"))?;
    let actual = read(&mut f)?;
    if actual != post || native::identity(&f)? != id {
        return Err("UNCERTAIN postimage/identity".into());
    }
    let receipt=Receipt{identity:id,pre_sha256:hash(expected),post_sha256:hash(&actual),bytes:actual.len(),state:"COMMITTED".into(),
        durability_limit:"same handle readback + Flush; not a filesystem transaction or proof of media durability".into()};
    if fault == Fault::Receipt {
        return Err("UNCERTAIN receipt failure".into());
    }
    tx.receipt(&receipt)
        .map_err(|e| format!("UNCERTAIN receipt: {e}"))?;
    Ok(receipt)
}
pub fn reserve_identity(path: &Path, tx: &Transaction) -> Result<()> {
    no_link_ancestors(path)?;
    let id = locked_identity(path)?;
    write_new(
        &tx.root.join("target-identity.json"),
        &serde_json::to_vec(&serde_json::json!({"identity":id})).unwrap(),
    )
}
/// Explicit recovery, only known postimage with the original file identity. Unknown partials stay uncertain.
pub fn recover(path: &Path, tx: &Transaction) -> Result<()> {
    no_link_ancestors(path)?;
    let intent: serde_json::Value = serde_json::from_slice(
        &fs::read(tx.root.join("mutation-intent.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut f = native::open(path, true, false)?;
    if intent["identity"] != native::identity(&f)? || read(&mut f)? != tx.post {
        return Err("UNKNOWN/user edit; recovery refused".into());
    }
    write_new(
        &tx.root.join("recovery-intent.json"),
        b"restore known preimage only",
    )?;
    f.seek(SeekFrom::Start(0))
        .and_then(|_| f.write_all(&tx.pre))
        .and_then(|_| f.set_len(tx.pre.len() as u64))
        .and_then(|_| f.sync_all())
        .map_err(|e| format!("UNCERTAIN recovery {e}"))?;
    if read(&mut f)? != tx.pre {
        return Err("UNCERTAIN recovery readback".into());
    }
    write_new(&tx.root.join("recovered.json"), b"KNOWN_PREIMAGE_RESTORED")
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Stream {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub path: PathBuf,
    pub directory: bool,
    pub attributes: u32,
    pub identity: String,
    pub bytes: u64,
    pub sha256: Option<String>,
    pub streams: Vec<Stream>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Manifest {
    pub complete: bool,
    pub status: String,
    pub filesystem: String,
    pub started_utc: String,
    pub ended_utc: String,
    pub entries: Vec<Entry>,
    pub errors: Vec<String>,
    pub limitation: String,
}
pub fn snapshot(root: &Path, out: &Path) -> Result<Manifest> {
    let canonical = fs::canonicalize(root).map_err(|e| e.to_string())?;
    let parent =
        fs::canonicalize(out.parent().ok_or("no manifest parent")?).map_err(|e| e.to_string())?;
    if parent.starts_with(&canonical) {
        return Err("manifest must be outside enumerated tree".into());
    }
    let mut m=Manifest{complete:false,status:"NOT ENUMERATED".into(),filesystem:String::new(),started_utc:chrono::Utc::now().to_rfc3339(),ended_utc:String::new(),entries:vec![],errors:vec![],limitation:"Non-atomic persistent snapshot only. Does not observe transient/outside-tree/registry/IPC effects.".into()};
    let result = (|| {
        no_link_ancestors(root)?;
        let f = native::open(root, false, true)?;
        m.filesystem = native::filesystem(&f)?;
        if m.filesystem != "NTFS" {
            return Err("ADS NOT ENUMERATED: NTFS required".into());
        }
        visit(root, root, &mut m.entries)?;
        m.entries.sort_by(|a, b| a.path.cmp(&b.path));
        let mut names = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        for e in &m.entries {
            if !names.insert(e.path.to_string_lossy().to_lowercase())
                || !ids.insert(e.identity.clone())
            {
                return Err("path/identity alias".into());
            }
        }
        Ok(())
    })();
    match result {
        Ok(()) => {
            m.complete = true;
            m.status = "ENUMERATED_SCOPE_ONLY".into();
        }
        Err(e) => m.errors.push(e),
    }
    m.ended_utc = chrono::Utc::now().to_rfc3339();
    write_new(
        out,
        &serde_json::to_vec_pretty(&m).map_err(|e| e.to_string())?,
    )?;
    Ok(m)
}
fn visit(root: &Path, path: &Path, entries: &mut Vec<Entry>) -> Result<()> {
    if entries.len() >= 512 {
        return Err("tree bound exceeded".into());
    }
    let before = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    let attributes = native::attributes(path)?;
    if attributes & 0x400 != 0 {
        return Err("reparse rejected".into());
    }
    let directory = before.is_dir();
    let mut f = native::open(path, false, directory)?;
    let identity = native::identity(&f)?;
    let mut bytes = Vec::new();
    if !directory {
        if before.len() > 1048576 {
            return Err("file bound".into());
        }
        f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    }
    let streams = native::streams(path)?;
    if native::identity(&f)? != identity
        || f.metadata().map_err(|e| e.to_string())?.modified().ok() != before.modified().ok()
    {
        return Err("changed during enumeration".into());
    }
    entries.push(Entry {
        path: path.strip_prefix(root).unwrap().into(),
        directory,
        attributes,
        identity,
        bytes: bytes.len() as u64,
        sha256: if directory { None } else { Some(hash(&bytes)) },
        streams,
    });
    if directory {
        for child in fs::read_dir(path).map_err(|e| e.to_string())? {
            visit(root, &child.map_err(|e| e.to_string())?.path(), entries)?;
        }
        if f.metadata().map_err(|e| e.to_string())?.modified().ok() != before.modified().ok() {
            return Err("directory changed during enumeration".into());
        }
    }
    Ok(())
}
pub fn verify_tree_change(
    before: &Manifest,
    after: &Manifest,
    target: &Path,
    post: &[u8],
) -> Result<()> {
    if !before.complete || !after.complete || before.entries.len() != after.entries.len() {
        return Err("incomplete/tree shape change".into());
    }
    let mut changed = 0;
    for (a, b) in before.entries.iter().zip(&after.entries) {
        if a.path != b.path
            || a.directory != b.directory
            || a.identity != b.identity
            || a.attributes != b.attributes
            || a.streams != b.streams
        {
            return Err("tree/ADS/identity changed".into());
        }
        if a.path == target {
            if b.sha256 != Some(hash(post)) || b.bytes != post.len() as u64 {
                return Err("wrong target bytes".into());
            }
            changed += 1;
        } else if a.bytes != b.bytes || a.sha256 != b.sha256 {
            return Err("other file changed".into());
        }
    }
    if changed != 1 {
        return Err("fixed target absent".into());
    }
    Ok(())
}
