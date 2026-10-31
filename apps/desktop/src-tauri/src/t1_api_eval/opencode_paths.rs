use super::Result;
pub const ROOT: &str = r"C:\NovaM0B\run\project";
pub const TARGET: &str = r"C:\NovaM0B\run\project\Logo.tsx";
pub fn target(path: &str, patterns: &[String], external_ask: bool) -> Result<()> {
    let allowed = [TARGET, TARGET.replace('\\', "/").as_str()]
        .iter()
        .any(|p| *p == path)
        || path == format!(r"\\?\{}", TARGET);
    if !allowed || patterns != ["Logo.tsx"] || external_ask {
        return Err("fixed target/pattern/external_directory contract".into());
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileFact {
    pub path: String,
    pub bytes: Vec<u8>,
    pub identity: String,
    pub owner: String,
    pub dacl: String,
    pub attributes: u32,
    pub ads: Vec<(String, Vec<u8>)>,
    pub links: u32,
    pub creation: u64,
    pub write: u64,
    pub access: u64,
    pub change: u64,
}
pub fn declared_git_write(
    before: &FileFact,
    after: &FileFact,
    root_commit: &str,
    probe_sid: &str,
    in_target_tree: bool,
) -> Result<()> {
    if before.path != r".git/opencode"
        || !in_target_tree
        || root_commit.len() != 40
        || !root_commit
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        || before.bytes != root_commit.as_bytes()
        || before.owner != probe_sid
        || before.attributes != 0x20
        || before.links != 1
        || !before.ads.is_empty()
    {
        return Err("Git declared write baseline rejected".into());
    }
    let mut a = after.clone();
    a.write = before.write;
    a.access = before.access;
    a.change = before.change;
    if &a != before {
        return Err("Git content/identity/owner/ACL/attributes/ADS changed".into());
    }
    Ok(())
}
pub fn git_images(actual: &[(String, String)], manifest: &[(String, String)]) -> Result<()> {
    if actual.is_empty() || actual.iter().any(|x| !manifest.contains(x)) {
        Err("Git image identity unprepared/mismatch".into())
    } else {
        Ok(())
    }
}
/// Compare complete injected manifests; real enumeration coverage remains V-3.
pub fn review_tree(
    before: &[FileFact],
    after: &[FileFact],
    commit: &str,
    sid: &str,
    declared_write_in_target_tree: bool,
) -> Result<()> {
    use std::collections::BTreeMap;
    let collect = |v: &[FileFact]| -> Result<BTreeMap<String, FileFact>> {
        let mut m = BTreeMap::new();
        for f in v {
            if f.owner != sid
                || f.path.contains(':')
                || f.path.contains("..")
                || f.attributes & 0x400 != 0
                || f.links != 1
                || m.insert(f.path.clone(), f.clone()).is_some()
            {
                return Err("manifest owner/path/reparse/identity ambiguity".into());
            }
        }
        Ok(m)
    };
    let a = collect(before)?;
    let b = collect(after)?;
    if a.keys().collect::<Vec<_>>() != b.keys().collect::<Vec<_>>() {
        return Err("new/missing file including hidden/ADS".into());
    }
    for (path, old) in a {
        let new = &b[&path];
        if path == ".git/opencode" {
            declared_git_write(&old, new, commit, sid, declared_write_in_target_tree)?;
        } else if &old != new {
            return Err(format!("undeclared Agent change: {path}"));
        }
    }
    Ok(())
}

/// One fixed Git preflight receipt supplied by the process boundary, never inferred from Drop.
pub struct GitPreflight {
    pub attempts: u32,
    pub exit: Option<u32>,
    pub timed_out: bool,
    pub job_active: Option<u32>,
    pub stdout: Vec<u8>,
    pub owner_sid: String,
    pub manifest_complete: bool,
    pub images: Vec<(String, String)>,
}
pub fn git_preflight(r: &GitPreflight, sid: &str, manifest: &[(String, String)]) -> Result<()> {
    if r.attempts != 1
        || r.exit != Some(0)
        || r.timed_out
        || r.job_active != Some(0)
        || r.owner_sid != sid
        || !r.manifest_complete
        || r.stdout.len() > 1024
    {
        return Err("fixed Git/owner preflight incomplete".into());
    }
    git_images(&r.images, manifest)?;
    let s = std::str::from_utf8(&r.stdout).map_err(|_| "Git output encoding")?;
    if ![
        format!("{ROOT}\n"),
        format!("{ROOT}\r\n"),
        format!("{}\n", ROOT.replace('\\', "/")),
        format!("{}\r\n", ROOT.replace('\\', "/")),
    ]
    .contains(&s.to_string())
    {
        return Err("Git root/multiple lines rejected; no safe.directory repair".into());
    }
    Ok(())
}
