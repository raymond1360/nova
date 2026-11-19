//! Fixed non-model permission probe. No model/session admission and no caller-selected command.
use super::{
    cli_adapter::{self, NativeDriver, Spec},
    contract::{api_json, hash, RelayRole},
    relay::Execution,
    self_codex,
    store::Guard,
    Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
pub const ROOT: &str = r"D:\nova-self2-permission-probe-1119b";
pub const CLI: &str = r"C:\Users\Lenovo\AppData\Roaming\npm\node_modules\@openai\codex\node_modules\@openai\codex-win32-x64\vendor\x86_64-pc-windows-msvc\bin\codex.exe";
const BEFORE: &[u8] = b"CANARY_INITIAL\r\n";
const AFTER: &[u8] = b"CANARY_DEVELOPED\r\n";
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Fact {
    identity: String,
    sha256: String,
    bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    schema: String,
    root_identity: String,
    nonce: String,
    cli_hash: String,
    helper: PathBuf,
    helper_hash: String,
    controls: BTreeMap<String, Fact>,
    targets: BTreeMap<String, Fact>,
}
fn bytes<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|_| "ENCODING")
}
fn fresh<T: Serialize>(p: &Path, v: &T) -> Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(p)
        .map_err(|_| "PROBE_ALREADY_RESERVED")?;
    f.write_all(&bytes(v)?)
        .and_then(|_| f.sync_all())
        .map_err(|_| "PROBE_EVIDENCE_UNKNOWN")
}
fn load<T: serde::de::DeserializeOwned>(p: &Path) -> Result<T> {
    let mut g = Guard::open(p, false)?;
    let b = g.bytes(1024 * 1024)?;
    serde_json::from_value(api_json(&b, 1024 * 1024)?).map_err(|_| "PROBE_EVIDENCE_REJECTED")
}
fn fact(p: &Path) -> Result<Fact> {
    let mut g = Guard::open(p, false)?;
    let b = g.bytes(1024 * 1024)?;
    Ok(Fact {
        identity: g.identity.clone(),
        sha256: hash(&b),
        bytes: b.len() as u64,
    })
}
fn tool_hash(p: &Path) -> Result<String> {
    let mut g = Guard::tool(p)?;
    let n = fs::metadata(p).map_err(|_| "FILE_STAT")?.len();
    g.sha256_stream(n, Instant::now() + Duration::from_secs(60))
}
fn phase(p: &str) -> Result<()> {
    if ["W", "R"].contains(&p) {
        Ok(())
    } else {
        Err("PROBE_PHASE_REJECTED")
    }
}
fn target(p: &str) -> &str {
    if p == "W" {
        "write.txt"
    } else {
        "readonly.txt"
    }
}
fn pinned(root: &Path) -> Result<Vec<Guard>> {
    [
        root.to_path_buf(),
        root.join("workspace"),
        root.join("control"),
        root.join("evidence"),
        root.join("home"),
        root.join("tmp"),
    ]
    .iter()
    .map(|p| Guard::open(p, true))
    .collect()
}
fn validate(root: &Path) -> Result<Plan> {
    let p: Plan = load(&root.join("plan.json"))?;
    if p.helper != std::env::current_exe().map_err(|_| "SELF_PATH")?
        || uuid::Uuid::parse_str(&p.nonce).is_err()
    {
        return Err("PROBE_BINDING_REJECTED");
    }
    if p.schema != "nova.self2.permission-probe/1"
        || p.cli_hash != self_codex::CLI_SHA
        || p.root_identity != Guard::open(root, true)?.identity
        || tool_hash(&p.helper)? != p.helper_hash
    {
        return Err("PROBE_BINDING_REJECTED");
    }
    for (n, f) in &p.controls {
        if !["instructions.txt"].contains(&n.as_str()) || fact(&root.join("control").join(n))? != *f
        {
            return Err("PROBE_CONTROL_DRIFT");
        }
    }
    if p.controls.len() != 1
        || p.targets.len() != 2
        || !p.targets.contains_key("write.txt")
        || !p.targets.contains_key("readonly.txt")
    {
        return Err("PROBE_BINDING_REJECTED");
    }
    Ok(p)
}
fn current(root: &Path) -> Result<BTreeMap<String, Fact>> {
    let mut out = BTreeMap::new();
    for e in fs::read_dir(root.join("workspace")).map_err(|_| "PROBE_READ")? {
        let e = e.map_err(|_| "PROBE_READ")?;
        let n = e.file_name().into_string().map_err(|_| "PROBE_PATH")?;
        out.insert(n, fact(&e.path())?);
    }
    Ok(out)
}
fn expected(p: &Plan, step: &str, after: bool) -> BTreeMap<String, Fact> {
    let mut x = p.targets.clone();
    if step == "R" || after {
        let f = x.get_mut("write.txt").unwrap();
        f.sha256 = hash(AFTER);
        f.bytes = AFTER.len() as u64;
    }
    x
}
fn argv(root: &Path, p: &Plan, step: &str) -> Vec<String> {
    vec![
        "sandbox".into(),
        "-C".into(),
        root.join("workspace").to_string_lossy().into_owned(),
        "-c".into(),
        "windows.sandbox=\"unelevated\"".into(),
        "-c".into(),
        format!(
            "sandbox_mode=\"{}\"",
            if step == "W" {
                "workspace-write"
            } else {
                "read-only"
            }
        ),
        "-c".into(),
        "sandbox_workspace_write.network_access=false".into(),
        "--".into(),
        p.helper.to_string_lossy().into_owned(),
        "permission-helper".into(),
        step.into(),
        p.nonce.clone(),
    ]
}
fn env(root: &Path) -> Result<BTreeMap<String, String>> {
    let parent = cli_adapter::ENV_ALLOW
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).into(), v)))
        .collect();
    let mut e = cli_adapter::environment(&parent, root)?;
    e.insert(
        "CODEX_HOME".into(),
        root.join("home").to_string_lossy().into_owned(),
    );
    Ok(e)
}
/// Only this module constructs the capability; no arbitrary Spec crosses the public boundary.
pub(super) struct Command {
    spec: Spec,
}
impl Command {
    pub(super) fn spec(&self) -> &Spec {
        &self.spec
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Start {
    digest: String,
    phase: String,
    command_hash: String,
    before: BTreeMap<String, Fact>,
    host_write_allowed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolReceipt {
    schema: String,
    nonce: String,
    phase: String,
    target: String,
    outcome: String,
    win32_error: u32,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Exit {
    start_hash: String,
    execution: Execution,
    cleanup: Option<super::contract::CleanupReceipt>,
}
fn reserve(root: &Path, step: &str, s: &Start) -> Result<()> {
    fresh(
        &root.join("evidence").join(format!("{step}.started.json")),
        s,
    )
}
fn capture(
    root: &Path,
    step: &str,
    start: &Start,
    c: &Command,
    d: &mut NativeDriver,
    end: Instant,
) -> Result<()> {
    reserve(root, step, start)?;
    let e = d.permission_probe(c, end, &mut |pid, created| {
        fresh(
            &root.join("evidence").join(format!("{step}.process.json")),
            &json!({"pid":pid,"creation":created}),
        )
    });
    fresh(
        &root.join("evidence").join(format!("{step}.exit.json")),
        &Exit {
            start_hash: hash(&bytes(start)?),
            cleanup: e.cleanup.clone(),
            execution: e,
        },
    )
}
fn check(root: &Path, step: &str) -> Result<Value> {
    phase(step)?;
    let _guards = pinned(root)?;
    let p = validate(root)?;
    let s: Start = load(&root.join("evidence").join(format!("{step}.started.json")))?;
    let x: Exit = load(&root.join("evidence").join(format!("{step}.exit.json")))?;
    let e = &x.execution;
    let process: Value = load(&root.join("evidence").join(format!("{step}.process.json")))?;
    if process["pid"].as_u64() != e.pid.map(|v| v as u64)
        || process["creation"].as_str() != e.process_creation_utc.as_deref()
        || e.terminal
            != if step == "W" {
                "SUCCESS"
            } else {
                "RESULT_ERROR"
            }
    {
        return Err("PROBE_PROCESS_RECEIPT_REJECTED");
    }
    if s.phase != step
        || s.digest != hash(&bytes(&p)?)
        || s.command_hash != hash(&bytes(&argv(root, &p, step))?)
        || s.before != expected(&p, step, false)
        || !s.host_write_allowed
        || x.start_hash != hash(&bytes(&s)?)
        || !e.created
        || e.pid.is_none()
        || e.process_creation_utc.is_none()
        || !e.natural
        || e.reclaim != "CONFIRMED"
        || !e.unknowns.is_empty()
        || !x
            .cleanup
            .as_ref()
            .is_some_and(|v| v.job_confirmed && v.readers_finished)
        || e.exit_code != Some(if step == "W" { 0 } else { 23 })
    {
        return Err("PROBE_EXECUTION_NOT_VERIFIED");
    }
    let r: ToolReceipt = serde_json::from_value(api_json(&e.stdout, 16384)?)
        .map_err(|_| "PROBE_TOOL_RECEIPT_REJECTED")?;
    if r.schema != "nova.self2.permission-tool/1"
        || r.nonce != p.nonce
        || r.phase != step
        || r.target != target(step)
        || r.outcome != if step == "W" { "WROTE" } else { "DENIED" }
        || r.win32_error != if step == "W" { 0 } else { 5 }
    {
        return Err("PROBE_TOOL_RECEIPT_REJECTED");
    }
    // Re-read NOW: neither the exited process nor a stored snapshot can attest current bytes/identity.
    if current(root)? != expected(&p, step, true) {
        return Err("PROBE_CURRENT_FILE_DRIFT");
    }
    if step == "R" {
        check(root, "W")?;
    }
    Ok(
        json!({"schema":"nova.self2.permission-verdict/1","phase":step,"tool_verified":true,"model_started":false,"model_admission":false,"plan_digest":s.digest,"command_exit":e.exit_code,"pid":e.pid,"creation":e.process_creation_utc,"reclaim":e.reclaim,"tool_receipt":r,"current_files":current(root)?,"stdout_hash":hash(&e.stdout),"stderr_hash":hash(&e.stderr)}),
    )
}
pub fn prepare() -> Result<Value> {
    let root = Path::new(ROOT);
    if root.exists() {
        return Err("PROBE_ROOT_ALREADY_EXISTS");
    }
    let helper = std::env::current_exe().map_err(|_| "SELF_PATH")?;
    if helper.file_name().and_then(|n| n.to_str()) != Some("nova_scheduler_self_codex.exe")
        || tool_hash(Path::new(CLI))? != self_codex::CLI_SHA
    {
        return Err("PROBE_CLI_REJECTED");
    }
    fs::create_dir(root).map_err(|_| "PROBE_CREATE")?;
    for n in ["workspace", "control", "evidence", "home", "tmp"] {
        fs::create_dir(root.join(n)).map_err(|_| "PROBE_CREATE")?;
    }
    let note=b"NON-MODEL synthetic permission probe only. No Nova source, login, session, provider or network commands. Legacy backend capability/ACL and NUL initialization may occur. This is NOT model admission.\n";
    fs::write(root.join("control/instructions.txt"), note).map_err(|_| "PROBE_CREATE")?;
    for n in ["write.txt", "readonly.txt"] {
        fs::write(root.join("workspace").join(n), BEFORE).map_err(|_| "PROBE_CREATE")?;
    }
    let p = Plan {
        schema: "nova.self2.permission-probe/1".into(),
        root_identity: Guard::open(root, true)?.identity,
        nonce: uuid::Uuid::new_v4().to_string(),
        cli_hash: self_codex::CLI_SHA.into(),
        helper: helper.clone(),
        helper_hash: tool_hash(&helper)?,
        controls: BTreeMap::from([(
            "instructions.txt".into(),
            fact(&root.join("control/instructions.txt"))?,
        )]),
        targets: current(root)?,
    };
    fresh(&root.join("plan.json"), &p)?;
    Ok(
        json!({"prepared_only":true,"root":ROOT,"plan":p,"digest":hash(&bytes(&p)?),"write_argv":argv(root,&p,"W"),"readonly_argv":argv(root,&p,"R"),"sandbox_started":false,"approval_required":"AUTHORIZE NON-MODEL ACL NUL <phase> <digest>"}),
    )
}
/// Permission-only native path: fixed CLI, fixed root, generated command, interactive approval.
pub fn run(step: &str, digest: &str, confirmation: &str) -> Result<Value> {
    phase(step)?;
    if !super::contract::hex(digest, 64)
        || confirmation != format!("AUTHORIZE NON-MODEL ACL NUL {step} {digest}")
    {
        return Err("PROBE_EXPLICIT_APPROVAL_REQUIRED");
    }
    let root = Path::new(ROOT);
    let _guards = pinned(root)?;
    let p = validate(root)?;
    let actual = hash(&bytes(&p)?);
    if actual != digest || confirmation != format!("AUTHORIZE NON-MODEL ACL NUL {step} {actual}") {
        return Err("PROBE_EXPLICIT_APPROVAL_REQUIRED");
    }
    if step == "R" {
        check(root, "W")?;
    }
    if current(root)? != expected(&p, step, false) {
        return Err("PROBE_CURRENT_FILE_DRIFT");
    }
    if tool_hash(Path::new(CLI))? != self_codex::CLI_SHA
        || std::env::current_exe().map_err(|_| "SELF_PATH")? != p.helper
    {
        return Err("PROBE_CLI_REJECTED");
    }
    // Host open-only check distinguishes native sandbox denial from an unwritable host fixture.
    let host = OpenOptions::new()
        .write(true)
        .open(root.join("workspace").join(target(step)))
        .map_err(|_| "PROBE_HOST_NOT_WRITABLE")?;
    drop(host);
    let spec = Spec {
        role: RelayRole::Develop,
        profile: "SELF2_PERMISSION_ONLY".into(),
        exe: PathBuf::from(CLI),
        exe_sha256: self_codex::CLI_SHA.into(),
        argv: argv(root, &p, step),
        env: env(root)?,
        cwd: root.join("workspace"),
        session: String::new(),
    };
    let start = Start {
        digest: actual,
        phase: step.into(),
        command_hash: hash(&bytes(&spec.argv)?),
        before: current(root)?,
        host_write_allowed: true,
    };
    let mut d = NativeDriver::self2_transport(Path::new(CLI), self_codex::CLI_SHA)?;
    let mut console = cli_adapter::ConsoleHandler::install()?;
    let result = capture(
        root,
        step,
        &start,
        &Command { spec },
        &mut d,
        Instant::now() + Duration::from_secs(30),
    );
    console.close()?;
    result?;
    check(root, step)
}
pub fn evaluate(step: &str) -> Result<Value> {
    check(Path::new(ROOT), step)
}
/// Fixed helper, no shell/path/program argument. Only two existing synthetic target files.
pub fn helper(step: &str, nonce: &str) -> Result<(Value, i32)> {
    helper_at(Path::new(ROOT), step, nonce)
}
fn helper_at(root: &Path, step: &str, nonce: &str) -> Result<(Value, i32)> {
    phase(step)?;
    let _guards = pinned(root)?;
    let p = validate(root)?;
    if nonce != p.nonce {
        return Err("PROBE_NONCE_REJECTED");
    }
    let s: Start = load(&root.join("evidence").join(format!("{step}.started.json")))?;
    if s.digest != hash(&bytes(&p)?) || s.phase != step {
        return Err("PROBE_HELPER_PERMIT_REJECTED");
    }
    if current(root)? != expected(&p, step, false) {
        return Err("PROBE_CURRENT_FILE_DRIFT");
    }
    let path = root.join("workspace").join(target(step));
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0).custom_flags(0x00200000);
    }
    let receipt = |outcome: &str, err: u32| json!({"schema":"nova.self2.permission-tool/1","nonce":nonce,"phase":step,"target":target(step),"outcome":outcome,"win32_error":err});
    let mut f = match options.open(&path) {
        Ok(f) => f,
        Err(e) if step == "R" && e.raw_os_error() == Some(5) => {
            return Ok((receipt("DENIED", 5), 23))
        }
        Err(_) => return Err("PROBE_HELPER_OPEN_UNKNOWN"),
    };
    if step == "R" {
        return Ok((receipt("UNEXPECTED_WRITE_OPEN", 0), 24));
    }
    if super::store::single_link_identity(&f)? != p.targets[target(step)].identity {
        return Err("PROBE_HELPER_IDENTITY");
    }
    let mut old = vec![];
    f.read_to_end(&mut old).map_err(|_| "PROBE_HELPER_READ")?;
    if old != BEFORE {
        return Err("PROBE_HELPER_PREIMAGE");
    }
    use std::io::{Seek, SeekFrom};
    f.seek(SeekFrom::Start(0))
        .and_then(|_| f.write_all(AFTER))
        .and_then(|_| f.set_len(AFTER.len() as u64))
        .and_then(|_| f.sync_all())
        .map_err(|_| "PROBE_HELPER_WRITE_UNKNOWN")?;
    drop(f);
    Ok((receipt("WROTE", 0), 0))
}
#[cfg(test)]
mod tests;
