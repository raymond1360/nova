//! CAS/journal straight-through seam and injected-only JS contract. No Node process launcher.
use super::{
    contract::Patch,
    hash,
    journal::{Receipt, Transaction},
    opencode_paths::*,
    opencode_probe::{CasReview, JournalState},
    Result,
};
use serde::{Deserialize, Serialize};
use std::{
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
pub const NODE_HASH: &str = "63c259c81e5d472b5f11c8d506070130cb04a1ecf84b80377a34ed6ec9048088";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reserved {
    pub identity: String,
    pub root: String,
    pub final_path: String,
}
pub trait CasPort {
    fn now_ms(&self) -> u64;
    fn filetime(&self) -> Result<u64>;
    fn verify_root(&mut self, r: &RootReservation) -> Result<()>;
    fn prepare(&mut self, dir: &str, pre: &[u8], post: &[u8]) -> Result<()>;
    fn reserve(&mut self, r: &FixtureReservation) -> Result<()>;
    fn reservation(&mut self) -> Result<Reserved>;
    fn apply(&mut self, r: &FixtureReservation, pre: &[u8], post: &[u8]) -> Result<()>;
    fn set_deadline(&mut self, _end_ms: u64) {}
    fn status(&mut self) -> Result<String>;
    fn receipt(&mut self) -> Result<Receipt>;
    fn after_tree(&mut self) -> Result<TreeSnapshot>;
}
/// Receipt data is frozen by the caller, never inferred from a live executable in the pure core.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resource {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
}
#[derive(Clone, Debug)]
pub struct JsAssets {
    pub node: Resource,
    pub script: Resource,
    pub contract: Resource,
    pub policy: Resource,
    pub lock: Resource,
    pub typescript: Resource,
    pub package: String,
    pub isolated_home: String,
}
#[derive(Clone, Debug)]
pub struct JsRequest {
    pub node: Resource,
    pub node_version: &'static str,
    pub node_arch: &'static str,
    pub typescript_version: &'static str,
    pub argv: [String; 4],
    pub environment: Vec<(String, String)>,
    pub resources: Vec<Resource>,
    pub deadline_ms: u64,
}
#[derive(Clone, Debug)]
pub struct JsResult {
    pub exit: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub exited: bool,
    pub readers_done: bool,
    pub timed_out: bool,
}
pub trait JsVerifier {
    fn verify(&mut self, request: &JsRequest) -> Result<JsResult>;
}
fn digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn absolute(s: &str) -> bool {
    s.len() > 3
        && s.as_bytes()[0].is_ascii_alphabetic()
        && s.as_bytes()[1..3] == *b":\\"
        && !s.contains(['\0', '/'])
        && !s.chars().any(|c| c.is_control())
        && s.split('\\').skip(1).all(|x| {
            !x.is_empty() && x != "." && x != ".." && !x.ends_with([' ', '.']) && !x.contains(':')
        })
}
fn child_of(p: &str, root: &str) -> bool {
    p.eq_ignore_ascii_case(root)
        || p.to_ascii_lowercase()
            .starts_with(&(root.to_ascii_lowercase() + "\\"))
}
pub fn transaction_location(dir: &str, root: &str) -> Result<()> {
    if !absolute(dir) || !absolute(root) || child_of(dir, root) {
        return Err("CAS_TRANSACTION_LOCATION".into());
    }
    Ok(())
}
pub fn js_request(a: &JsAssets, tx: &str, deadline_ms: u64) -> Result<JsRequest> {
    for x in [
        &a.node,
        &a.script,
        &a.contract,
        &a.policy,
        &a.lock,
        &a.typescript,
    ] {
        if !absolute(&x.path) || x.bytes == 0 || !digest(&x.sha256) {
            return Err("JS_RESOURCE_IDENTITY".into());
        }
    }
    if a.node.sha256 != NODE_HASH
        || !a.script.path.ends_with("\\check-logo.cjs")
        || !a.contract.path.ends_with("\\logo-contract.json")
        || !a.policy.path.ends_with("\\opencode-policy.json")
        || !a.lock.path.ends_with("\\package-lock.json")
        || !absolute(&a.package)
        || !a.package.ends_with("\\package.json")
        || !absolute(&a.isolated_home)
        || !absolute(tx)
    {
        return Err("JS_FROZEN_INPUT".into());
    }
    let tools = a.script.path.rsplit_once('\\').unwrap().0;
    let deps = a.package.rsplit_once('\\').unwrap().0;
    if a.contract.path != format!("{tools}\\logo-contract.json")
        || a.policy.path != format!("{tools}\\opencode-policy.json")
        || a.lock.path != format!("{deps}\\package-lock.json")
        || a.typescript.path != format!("{deps}\\node_modules\\typescript")
    {
        return Err("JS_RESOURCE_ROOT".into());
    }
    // Construct from empty, not ambient environment. No NODE_OPTIONS, NODE_PATH, credentials or proxy.
    let environment = [
        "TEMP",
        "TMP",
        "HOME",
        "USERPROFILE",
        "APPDATA",
        "LOCALAPPDATA",
    ]
    .into_iter()
    .map(|k| (k.into(), a.isolated_home.clone()))
    .collect();
    Ok(JsRequest {
        node: a.node.clone(),
        node_version: "v24.14.0",
        node_arch: "x64",
        typescript_version: "5.9.3",
        argv: [
            a.script.path.clone(),
            a.package.clone(),
            TARGET.into(),
            format!("{tx}\\preimage.bin"),
        ],
        environment,
        resources: vec![
            a.script.clone(),
            a.contract.clone(),
            a.policy.clone(),
            a.lock.clone(),
            a.typescript.clone(),
        ],
        deadline_ms,
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JsOutput {
    scope: String,
    bytes: u64,
    sha256: String,
}
pub fn check_js(result: &JsResult, post: &[u8]) -> Result<()> {
    if result.exit != Some(0)
        || !result.exited
        || !result.readers_done
        || result.timed_out
        || !result.stderr.is_empty()
        || result.stdout.len() > 4096
    {
        return Err("JS_EXECUTION".into());
    }
    let text = std::str::from_utf8(&result.stdout).map_err(|_| "JS_ENCODING")?;
    let line = text
        .strip_suffix('\n')
        .unwrap_or(text)
        .strip_suffix('\r')
        .unwrap_or(text.strip_suffix('\n').unwrap_or(text));
    if line.contains(['\n', '\r']) || line.is_empty() {
        return Err("JS_SINGLE_LINE".into());
    }
    let x: JsOutput = serde_json::from_str(line).map_err(|_| "JS_SCHEMA")?;
    if x.bytes!=795||x.bytes!=post.len() as u64||x.sha256!=hash(post)||x.scope!="deterministic contract and TSX parse only; not typecheck, lint, DOM or production build"{return Err("JS_RESULT".into())}
    Ok(())
}
pub fn journal_state(s: Result<String>) -> JournalState {
    match s.as_deref() {
        Ok("PREPARED") => JournalState::Prepared,
        Ok("UNCERTAIN") => JournalState::Uncertain,
        Ok("UNCERTAIN_RECOVERY") => JournalState::UncertainRecovery,
        Ok("COMMITTED_HISTORICAL") => JournalState::CommittedHistorical,
        Ok("RECOVERED_HISTORICAL") => JournalState::RecoveredHistorical,
        _ => JournalState::Uncertain,
    }
}
pub struct BridgeResult {
    pub review: CasReview,
    pub code: &'static str,
    pub partial: bool,
}
/// end_ms is the earliest outer/phase deadline supplied by the caller. One CAS30s budget includes JS10s.
pub fn bridge<P: CasPort, J: JsVerifier>(
    port: &mut P,
    js: &mut J,
    agent: &ReviewedAgentTree,
    patch: &Patch,
    permission: &str,
    tx: &str,
    assets: &JsAssets,
    end_ms: u64,
) -> Result<BridgeResult> {
    let start = port.now_ms();
    let end = end_ms.min(start.checked_add(30000).ok_or("CAS_CLOCK")?);
    let enough = |now: u64| now >= start && now.checked_add(1000).is_some_and(|t| t < end);
    if !enough(start) {
        return Err("CAS_BUDGET".into());
    }
    port.set_deadline(end);
    transaction_location(tx, &agent.reservation().root.path)?;
    let post = patch.validate("M0B", permission, super::PRE)?;
    port.verify_root(&agent.reservation().root)
        .map_err(|_| "CAS_ROOT_IDENTITY")?;
    if !enough(port.now_ms()) {
        return Err("CAS_BUDGET".into());
    }
    port.prepare(tx, super::PRE, &post)
        .map_err(|_| "CAS_PREPARE_PARTIAL_PRESERVE_DIRECTORY")?;
    // From here on all exits return an actual status-derived CasReview, never ? away history.
    let mut receipt_ok = false;
    let mut verified = false;
    let mut partial = false;
    let operation = (|| -> Result<()> {
        if !enough(port.now_ms()) {
            return Err("CAS_LATE".into());
        }
        port.reserve(agent.reservation())
            .map_err(|_| "CAS_RESERVE")?;
        if !enough(port.now_ms()) {
            return Err("CAS_LATE".into());
        }
        let read = port.reservation().map_err(|_| "CAS_RESERVATION_READ")?;
        if read.identity != agent.reservation().logo_identity
            || read.root != agent.reservation().root.path
            || read.final_path != TARGET
        {
            return Err("CAS_RESERVATION_MISMATCH".into());
        }
        if !enough(port.now_ms()) {
            return Err("CAS_LATE".into());
        }
        let t0 = port.filetime().map_err(|_| "CAS_TIME")?;
        if !enough(port.now_ms()) {
            return Err("CAS_LATE".into());
        }
        let applied = port.apply(agent.reservation(), super::PRE, &post);
        let t1 = port.filetime().map_err(|_| "CAS_TIME")?;
        if !enough(port.now_ms()) {
            partial = true;
            return Err("CAS_LATE_PARTIAL_NO_JS".into());
        }
        // Always read status, including when apply returns Err. Error text never determines journal state.
        let current = journal_state(port.status());
        if applied.is_err() {
            return Err("CAS_APPLY".into());
        }
        if current != JournalState::CommittedHistorical {
            return Err("CAS_NOT_COMMITTED".into());
        }
        if !enough(port.now_ms()) {
            return Err("CAS_LATE".into());
        }
        let receipt = port.receipt().map_err(|_| "CAS_RECEIPT")?;
        receipt_ok = receipt.state == "COMMITTED"
            && receipt.identity == agent.reservation().logo_identity
            && receipt.pre_sha256 == hash(super::PRE)
            && receipt.post_sha256 == hash(&post)
            && receipt.bytes == post.len();
        if !receipt_ok {
            return Err("CAS_RECEIPT_MISMATCH".into());
        }
        if !enough(port.now_ms()) {
            return Err("CAS_LATE".into());
        }
        let js_end = end
            .saturating_sub(1000)
            .min(port.now_ms().checked_add(10000).ok_or("CAS_CLOCK")?);
        let req = js_request(assets, tx, js_end)?;
        if !enough(port.now_ms()) {
            return Err("CAS_LATE".into());
        }
        let result = js.verify(&req).map_err(|_| "JS_PORT")?;
        if port.now_ms() > js_end || port.now_ms() < start {
            partial = true;
            return Err("JS_LATE".into());
        }
        check_js(&result, &post)?;
        if !enough(port.now_ms()) {
            return Err("CAS_LATE".into());
        }
        let after = port.after_tree().map_err(|_| "CAS_AFTER_TREE")?;
        review_nova_tree(agent, &after, &receipt, &post, t0, t1)?;
        if !enough(port.now_ms()) {
            partial = true;
            return Err("CAS_LATE".into());
        }
        verified = true;
        Ok(())
    })();
    let code = match operation {
        Ok(()) => "NONE",
        Err(e) => match e.as_str() {
            "CAS_LATE_PARTIAL_NO_JS" => "CAS_LATE_PARTIAL_NO_JS",
            "CAS_RESERVATION_MISMATCH" => "CAS_RESERVATION_MISMATCH",
            "CAS_RESERVE" => "CAS_RESERVE",
            "CAS_APPLY" => "CAS_APPLY",
            "CAS_NOT_COMMITTED" => "CAS_NOT_COMMITTED",
            "JS_LATE" => "JS_LATE",
            "CAS_AFTER_TREE" => "CAS_AFTER_TREE",
            _ => "CAS_VERIFICATION_FAILED",
        },
    };
    let state = journal_state(port.status());
    if port.now_ms() > end || port.now_ms() < start {
        partial = true;
        verified = false;
    }
    Ok(BridgeResult {
        review: CasReview {
            state,
            receipt: receipt_ok,
            independently_verified: verified,
        },
        code,
        partial,
    })
}
/// Straight-through original IO only. Not connected to an executable entry. Never instantiated by new tests.
#[cfg(windows)]
pub struct NativeCasPort {
    pub reservation: FixtureReservation,
    pub directory: PathBuf,
    pub origin: Instant,
    pub outer: Instant,
    transaction: Option<Transaction>,
}
#[cfg(windows)]
impl NativeCasPort {
    pub fn new(
        reservation: FixtureReservation,
        directory: PathBuf,
        origin: Instant,
        outer: Instant,
    ) -> Self {
        Self {
            reservation,
            directory,
            origin,
            outer,
            transaction: None,
        }
    }
    fn tx(&self) -> Result<&Transaction> {
        self.transaction.as_ref().ok_or("CAS_NO_TRANSACTION".into())
    }
}
#[cfg(windows)]
impl CasPort for NativeCasPort {
    fn set_deadline(&mut self, end_ms: u64) {
        self.outer = self
            .outer
            .min(self.origin + std::time::Duration::from_millis(end_ms));
    }
    fn now_ms(&self) -> u64 {
        self.origin
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    fn filetime(&self) -> Result<u64> {
        let d = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| "FILETIME")?;
        d.as_secs()
            .checked_add(11644473600)
            .and_then(|s| s.checked_mul(10_000_000))
            .and_then(|s| s.checked_add(d.subsec_nanos() as u64 / 100))
            .ok_or("FILETIME_OVERFLOW".into())
    }
    fn verify_root(&mut self, r: &RootReservation) -> Result<()> {
        super::opencode_native_files::native::verify_root(r)
    }
    fn prepare(&mut self, dir: &str, pre: &[u8], post: &[u8]) -> Result<()> {
        if self.transaction.is_some() || Path::new(dir) != self.directory {
            return Err("CAS_TRANSACTION_REUSE".into());
        }
        let parent = self.directory.parent().ok_or("CAS_TRANSACTION_PARENT")?;
        let final_parent = std::fs::canonicalize(parent).map_err(|_| "CAS_TRANSACTION_PARENT")?;
        let root = std::fs::canonicalize(&self.reservation.root.path).map_err(|_| "CAS_ROOT")?;
        if final_parent.starts_with(&root) {
            return Err("CAS_TRANSACTION_LOCATION".into());
        }
        for p in parent.ancestors() {
            use std::os::windows::fs::MetadataExt;
            if std::fs::symlink_metadata(p)
                .map_err(|_| "CAS_PARENT")?
                .file_attributes()
                & 0x400
                != 0
            {
                return Err("CAS_PARENT_LINK".into());
            }
        }
        self.transaction = Some(Transaction::prepare(&self.directory, pre, post)?);
        Ok(())
    }
    fn reserve(&mut self, r: &FixtureReservation) -> Result<()> {
        super::opencode_native_files::native::verify_root(&r.root)?;
        super::cas::reserve_identity(Path::new(TARGET), Path::new(&r.root.path), self.tx()?)
    }
    fn reservation(&mut self) -> Result<Reserved> {
        let b = bounded_read(&self.directory.join("target-identity.json"))?;
        let read: Reserved = serde_json::from_slice(&b).map_err(|_| "CAS_RESERVATION_JSON")?;
        // Original reserve_identity persists canonical Windows paths (usually extended prefix).
        // Check the raw persisted paths against canonical ROOT before projecting the fixed logical paths.
        let root =
            std::fs::canonicalize(&self.reservation.root.path).map_err(|_| "CAS_ROOT_PATH")?;
        if read.root != root.to_string_lossy()
            || read.final_path != root.join("Logo.tsx").to_string_lossy()
        {
            return Err("CAS_CANONICAL_RESERVATION_MISMATCH".into());
        }
        Ok(Reserved {
            identity: read.identity,
            root: self.reservation.root.path.clone(),
            final_path: TARGET.into(),
        })
    }
    fn apply(&mut self, r: &FixtureReservation, pre: &[u8], post: &[u8]) -> Result<()> {
        super::cas::apply(
            Path::new(TARGET),
            Path::new(&r.root.path),
            pre,
            post,
            self.tx()?,
            super::cas::Fault::None,
        )
        .map(|_| ())
    }
    fn status(&mut self) -> Result<String> {
        self.tx()?.status().map(str::to_owned)
    }
    fn receipt(&mut self) -> Result<Receipt> {
        serde_json::from_slice(&bounded_read(&self.directory.join("receipt.json"))?)
            .map_err(|_| "CAS_RECEIPT_JSON".into())
    }
    fn after_tree(&mut self) -> Result<TreeSnapshot> {
        super::opencode_native_files::native::FixtureFactCollector::collect(
            self.reservation.policy.as_ref().ok_or("CAS_POLICY")?,
            self.reservation.prepared.as_ref().ok_or("CAS_PREPARED")?,
            self.origin,
            self.outer,
        )
    }
}
#[cfg(windows)]
fn bounded_read(p: &Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let f = std::fs::File::open(p).map_err(|_| "CAS_EVIDENCE_READ")?;
    let mut b = Vec::new();
    f.take(4097)
        .read_to_end(&mut b)
        .map_err(|_| "CAS_EVIDENCE_READ")?;
    if b.len() > 4096 {
        return Err("CAS_EVIDENCE_BOUND".into());
    }
    Ok(b)
}
