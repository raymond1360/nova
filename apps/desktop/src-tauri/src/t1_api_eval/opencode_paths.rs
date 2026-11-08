use super::opencode_native_files::FactSource;
use super::Result;
use serde::{Deserialize, Serialize};
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
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
            if f.identity.is_empty()
                || f.dacl.is_empty()
                || f.owner != sid
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
            declared_git_write(
                &old,
                new,
                commit,
                sid,
                declared_write_in_target_tree
                    || super::opencode_native_files::same_except_access(&old, new),
            )?;
        } else if !super::opencode_native_files::same_except_access(&old, new) {
            return Err(format!("undeclared Agent change: {path}"));
        }
    }
    Ok(())
}

/// Frozen fixture configuration, not an environment override or user config.
pub const FROZEN_GIT_CONFIG: &[u8] = b"[core]\n\trepositoryformatversion = 0\n\tfilemode = false\n\tbare = false\n\tlogallrefupdates = true\n[protocol]\n\tallow = never\n";
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum StopError {
    QueryFailed,
    TerminationFailed,
    Deadline,
    Malformed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobStopReceipt {
    pub flags: Option<u32>,
    pub active: Option<u32>,
    pub direct_exited: Option<bool>,
    pub error: Option<StopError>,
}
impl JobStopReceipt {
    pub fn validate(&self) -> Result<()> {
        if self.flags != Some(0x2000)
            || self.active != Some(0)
            || self.direct_exited != Some(true)
            || self.error.is_some()
        {
            return Err("Job flags/active/direct exit UNKNOWN or mismatch".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootReservation {
    pub path: String,
    pub identity: String,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureReservation {
    pub root: RootReservation,
    pub logo_identity: String,
    pub git_marker_identity: String,
    pub commit: String,
    pub sid: String,
    pub git_images: Vec<(String, String)>,
    pub git_config_sha256: String,
    pub policy: Option<super::opencode_native_files::FrozenFixturePolicy>,
    pub prepared: Option<super::opencode_native_files::PreparedFixtureReceipt>,
}
impl FixtureReservation {
    pub fn validate(&self, expected: FactSource) -> Result<()> {
        if self.root.path != ROOT
            || self.root.identity.is_empty()
            || self.logo_identity.is_empty()
            || self.git_marker_identity.is_empty()
            || self.sid.is_empty()
            || self.commit.len() != 40
            || !self
                .commit
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.git_images.is_empty()
            || self.git_config_sha256 != super::hash(FROZEN_GIT_CONFIG)
        {
            return Err("fixture reservation/manifest not frozen".into());
        }
        let policy = self
            .policy
            .as_ref()
            .ok_or("fixture expected policy missing")?;
        if policy.source != expected
            || self
                .prepared
                .as_ref()
                .ok_or("fixture prepared receipt missing")?
                .source
                != expected
        {
            return Err("fixture source differs from expected run mode".into());
        }
        self.prepared
            .as_ref()
            .ok_or("fixture prepared receipt missing")?
            .validate(policy)?;
        if policy.sid != self.sid
            || policy.root != self.root.path
            || self.prepared.as_ref().unwrap().identities.get(".") != Some(&self.root.identity)
        {
            return Err("fixture policy/root binding".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TreeSnapshot {
    pub complete: Option<bool>,
    pub files: Vec<FileFact>,
    pub enumeration: Option<super::opencode_native_files::EnumerationReceipt>,
}
pub fn validate_snapshot(
    s: &TreeSnapshot,
    r: &FixtureReservation,
    expected: FactSource,
) -> Result<()> {
    r.validate(expected)?;
    super::opencode_native_files::validate_collected_snapshot(s, r, true, expected)?;
    if s.complete != Some(true) {
        return Err("tree enumeration UNKNOWN/incomplete".into());
    }
    // No write is being excused here: this compares the baseline with itself.
    review_tree(&s.files, &s.files, &r.commit, &r.sid, true)?;
    let get = |path: &str| {
        s.files
            .iter()
            .find(|f| f.path == path)
            .ok_or("required fixture entry missing")
    };
    let marker = get(".git/opencode")?;
    declared_git_write(marker, marker, &r.commit, &r.sid, true)?;
    if marker.identity != r.git_marker_identity {
        return Err("Git marker identity changed".into());
    }
    if get(".")?.identity != r.root.identity
        || get(".")?.attributes & 0x10 == 0
        || get(".git")?.attributes & 0x10 == 0
        || get("Logo.tsx")?.bytes != super::PRE
        || get("Logo.tsx")?.identity != r.logo_identity
        || get(".git/config")?.bytes != FROZEN_GIT_CONFIG
        || super::hash(&get(".git/config")?.bytes) != r.git_config_sha256
    {
        return Err("root/logo/config reservation mismatch".into());
    }
    Ok(())
}
/// One fixed Git call, with raw exit, image, root identity and Job-stop facts.
#[derive(Clone, Debug)]
pub struct GitPreflight {
    pub attempts: u32,
    pub exit: Option<u32>,
    pub timed_out: bool,
    pub job: JobStopReceipt,
    pub stdout: Vec<u8>,
    pub owner_sid: String,
    pub manifest_complete: bool,
    pub images: Vec<(String, String)>,
    pub root_identity: Option<String>,
}
pub fn git_preflight(
    r: &GitPreflight,
    sid: &str,
    manifest: &[(String, String)],
    root: &RootReservation,
) -> Result<()> {
    if root.path != ROOT
        || root.identity.is_empty()
        || r.root_identity.as_deref() != Some(root.identity.as_str())
        || r.attempts != 1
        || r.exit != Some(0)
        || r.timed_out
        || r.owner_sid != sid
        || !r.manifest_complete
        || r.stdout.len() > 1024
    {
        return Err("fixed Git/owner/root identity preflight incomplete".into());
    }
    r.job.validate()?;
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
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub created: u64,
}
impl ProcessIdentity {
    pub fn validate(&self) -> Result<()> {
        if self.pid == 0 || self.created == 0 {
            Err("process identity UNKNOWN".into())
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessEdge {
    pub process: ProcessIdentity,
    pub parent: ProcessIdentity,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteKind {
    Overwrite,
    Rename,
    Delete,
    Replace,
    Metadata,
    Unknown,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileWriteFact {
    pub path: String,
    pub process: ProcessIdentity,
    pub kind: WriteKind,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentTreeReceipt {
    pub after: TreeSnapshot,
    pub descendants: Vec<ProcessEdge>,
    pub writes: Vec<FileWriteFact>,
}
pub fn review_agent_tree(
    before: &TreeSnapshot,
    receipt: &AgentTreeReceipt,
    r: &FixtureReservation,
    target: &ProcessIdentity,
    expected: FactSource,
) -> Result<()> {
    use std::collections::BTreeSet;
    validate_snapshot(before, r, expected)?;
    target.validate()?;
    super::opencode_native_files::validate_collected_snapshot(&receipt.after, r, false, expected)?;
    if receipt.after.complete != Some(true) {
        return Err("after tree incomplete/UNKNOWN".into());
    }
    let mut members = BTreeSet::from([target.clone()]);
    let mut pending = receipt.descendants.clone();
    let mut unique = BTreeSet::new();
    for edge in &pending {
        edge.process.validate()?;
        edge.parent.validate()?;
        if edge.process.pid == edge.parent.pid
            || edge.process.created < edge.parent.created
            || edge.process == *target
            || !unique.insert(edge.process.clone())
        {
            return Err("ambiguous process ancestry".into());
        }
    }
    while !pending.is_empty() {
        let n = pending.len();
        pending.retain(|edge| {
            if members.contains(&edge.parent) {
                members.insert(edge.process.clone());
                false
            } else {
                true
            }
        });
        if pending.len() == n {
            return Err("unknown/cyclic process ancestry".into());
        }
    }
    for write in &receipt.writes {
        if write.path != ".git/opencode"
            || write.kind != WriteKind::Overwrite
            || !members.contains(&write.process)
        {
            return Err("undeclared write/path/kind/PID-tree attribution".into());
        }
    }
    let marker = |files: &[FileFact]| files.iter().find(|f| f.path == ".git/opencode").cloned();
    let marker_changed = match (marker(&before.files), marker(&receipt.after.files)) {
        (Some(a), Some(b)) => !super::opencode_native_files::same_except_access(&a, &b),
        _ => true,
    };
    if marker_changed && receipt.writes.is_empty() {
        return Err("changed Git marker lacks attributed write evidence".into());
    }
    // Attribution is computed above from facts, not supplied as a backend verdict.
    let attributed = receipt.writes.iter().all(|w| members.contains(&w.process));
    review_tree(
        &before.files,
        &receipt.after.files,
        &r.commit,
        &r.sid,
        attributed,
    )
}

/// Capability constructed only after the unchanged Agent checker has accepted all projected writes.
#[derive(Clone, Debug)]
pub struct ReviewedAgentTree {
    snapshot: TreeSnapshot,
    reservation: FixtureReservation,
    source: FactSource,
}
impl ReviewedAgentTree {
    pub fn review(
        before: &TreeSnapshot,
        receipt: &AgentTreeReceipt,
        r: &FixtureReservation,
        target: &ProcessIdentity,
        expected: FactSource,
    ) -> Result<Self> {
        review_agent_tree(before, receipt, r, target, expected)?;
        let logo = receipt
            .after
            .files
            .iter()
            .find(|f| f.path == "Logo.tsx")
            .ok_or("NOVA_LOGO_MISSING")?;
        if logo.attributes != 0x20 {
            return Err("NOVA_LOGO_FROZEN_ATTRIBUTES".into());
        }
        Ok(Self {
            snapshot: receipt.after.clone(),
            reservation: r.clone(),
            source: expected,
        })
    }
    pub fn snapshot(&self) -> &TreeSnapshot {
        &self.snapshot
    }
    pub fn reservation(&self) -> &FixtureReservation {
        &self.reservation
    }
    pub fn source(&self) -> FactSource {
        self.source
    }
}
/// FILETIME consistency only; real NTFS timing/granularity remains unverified.
pub fn review_nova_tree(
    agent: &ReviewedAgentTree,
    nova: &TreeSnapshot,
    receipt: &super::journal::Receipt,
    post: &[u8],
    t0: u64,
    t1: u64,
) -> Result<()> {
    use super::opencode_native_files::{same_except_access, validate_collected_snapshot};
    let r = agent.reservation();
    validate_collected_snapshot(nova, r, false, agent.source)?;
    if nova.complete != Some(true) || t1.checked_sub(t0).is_none_or(|d| d > 30 * 10_000_000) {
        return Err("NOVA_TIME_UNKNOWN".into());
    }
    let low = t0.checked_sub(200000).ok_or("NOVA_TIME_UNDERFLOW")?;
    let map = |s: &TreeSnapshot| -> Result<std::collections::BTreeMap<String, FileFact>> {
        let mut m = std::collections::BTreeMap::new();
        for f in &s.files {
            if m.insert(f.path.to_ascii_uppercase(), f.clone()).is_some() {
                return Err("NOVA_PATH_AMBIGUOUS".into());
            }
        }
        Ok(m)
    };
    let a = map(&agent.snapshot)?;
    let b = map(nova)?;
    if a.keys().ne(b.keys()) {
        return Err("NOVA_TREE_SET".into());
    }
    for (path, old) in a {
        let new = &b[&path];
        if old.path != "Logo.tsx" {
            if !same_except_access(&old, new) {
                return Err("NOVA_UNDECLARED_CHANGE".into());
            }
            continue;
        }
        if new.bytes != post
            || receipt.post_sha256 != super::hash(post)
            || receipt.pre_sha256 != super::hash(&old.bytes)
            || receipt.bytes != post.len()
            || receipt.state != "COMMITTED"
            || receipt.identity != r.logo_identity
            || old.identity != r.logo_identity
            || new.identity != r.logo_identity
            || old.attributes != 0x20
            || new.attributes != old.attributes
            || new.owner != old.owner
            || new.dacl != old.dacl
            || new.links != old.links
            || new.ads != old.ads
            || !new.ads.is_empty()
            || new.creation != old.creation
            || new.path != old.path
        {
            return Err("NOVA_LOGO_FACT_MISMATCH".into());
        }
        if old.write >= low
            || old.change >= low
            || new.write < low
            || new.write > t1
            || new.change < low
            || new.change > t1
        {
            return Err("NOVA_TIME_INDISCERNIBLE".into());
        }
    }
    Ok(())
}
