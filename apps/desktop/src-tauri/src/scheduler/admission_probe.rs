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
pub const ROOT: &str = r"D:\nova-self2-permission-probe-1119d";
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
    environment: BTreeMap<String, String>,
    helper_environment: BTreeMap<String, String>,
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
    validate_environment(root, None)
}
fn validate_environment(root: &Path, helper: Option<&BTreeMap<String, String>>) -> Result<Plan> {
    let p: Plan = load(&root.join("plan.json"))?;
    if root.join("home/config.toml").exists()
        || root.join("home/AGENTS.md").exists()
        || p.helper_environment != helper_environment(root, &p.environment)?
        || match helper {
            None => p.environment != env(root)?,
            Some(actual) => canonical_environment(actual)? != canonical_environment(&p.helper_environment)?,
        }
    {
        return Err("PROBE_ENVIRONMENT_DRIFT");
    }
    if p.helper != std::env::current_exe().map_err(|_| "SELF_PATH")?
        || uuid::Uuid::parse_str(&p.nonce).is_err()
    {
        return Err("PROBE_BINDING_REJECTED");
    }
    if p.schema != "nova.self2.permission-probe/2"
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
        "--permission-profile".into(),
        if step == "W" {
            ":workspace"
        } else {
            ":read-only"
        }
        .into(),
        "--include-managed-config".into(),
        "-C".into(),
        root.join("workspace").to_string_lossy().into_owned(),
        "-c".into(),
        "windows.sandbox=\"unelevated\"".into(),
        "-c".into(),
        "sandbox_workspace_write.network_access=false".into(),
        "-c".into(),
        "shell_environment_policy.inherit=\"none\"".into(),
        "-c".into(),
        format!("shell_environment_policy.set={{{}}}", p.environment.iter()
            .map(|(k, v)| format!("{}={}", json!(k), json!(v)))
            .collect::<Vec<_>>().join(",")),
        "-c".into(),
        format!("shell_environment_policy.include_only={}", json!(p.environment.keys().collect::<Vec<_>>())),
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
// rust-v0.162.1 create_env: inherit=none, explicit safe set, exact include_only,
// no thread ID. Legacy then normalizes null values, adds pager/offline entries,
// prepends the known-folder denybin and reorders PATHEXT. No tools run here.
fn canonical_environment(input: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for (k, v) in input {
        if !k.is_ascii() || k.contains('=') || k.contains('\0') || v.contains('\0')
            || out.insert(k.to_ascii_uppercase(), v.clone()).is_some() {
            return Err("PROBE_ENVIRONMENT_DRIFT");
        }
    }
    Ok(out)
}
#[cfg(windows)]
fn legacy_profile_dir() -> Result<PathBuf> {
    use std::ffi::c_void;
    use windows_sys::core::GUID;
    // dirs-next 2.0.0 / dirs-sys-next 0.1.2 use this exact read-only API,
    // not USERPROFILE or CODEX_HOME. Avoid enabling any new dependency feature.
    #[link(name = "shell32")]
    extern "system" { fn SHGetKnownFolderPath(id: *const GUID, flags: u32, token: *mut c_void, path: *mut *mut u16) -> i32; }
    #[link(name = "ole32")]
    extern "system" { fn CoTaskMemFree(memory: *mut c_void); }
    let id = GUID::from_u128(0x5e6c858f_0e22_4760_9afe_ea3317b67173);
    let mut ptr = std::ptr::null_mut();
    let status = unsafe { SHGetKnownFolderPath(&id, 0, std::ptr::null_mut(), &mut ptr) };
    if status < 0 || ptr.is_null() { return Err("PROBE_HELPER_HOME_UNKNOWN"); }
    let value = (|| {
        let mut len = 0;
        while len < 32768 && unsafe { *ptr.add(len) } != 0 { len += 1; }
        if len == 32768 { return Err("PROBE_HELPER_HOME_UNKNOWN"); }
        let s = String::from_utf16(unsafe { std::slice::from_raw_parts(ptr, len) })
            .map_err(|_| "PROBE_HELPER_HOME_UNKNOWN")?;
        let p = PathBuf::from(s);
        if !p.is_absolute() { return Err("PROBE_HELPER_HOME_UNKNOWN"); }
        Ok(p)
    })();
    unsafe { CoTaskMemFree(ptr.cast()); }
    value
}
#[cfg(not(windows))]
fn legacy_profile_dir() -> Result<PathBuf> { Err("WINDOWS_REQUIRED") }
fn helper_environment(root: &Path, host: &BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    helper_environment_for_profile(root, host, &legacy_profile_dir()?)
}
fn helper_environment_for_profile(root: &Path, host: &BTreeMap<String, String>, profile: &Path) -> Result<BTreeMap<String, String>> {
    let mut safe = cli_adapter::environment(host, root)?;
    safe.insert("CODEX_HOME".into(), root.join("home").to_string_lossy().into_owned());
    if safe != *host || !profile.is_absolute() { return Err("PROBE_ENVIRONMENT_DRIFT"); }
    let mut child = host.clone();
    child.entry("PATHEXT".into()).or_insert_with(|| ".COM;.EXE;.BAT;.CMD".into());
    for value in child.values_mut() {
        let normalized = value.trim().to_ascii_lowercase();
        if normalized == "/dev/null" || normalized == "\\\\\\\\dev\\\\\\\\null" { *value = "NUL".into(); }
    }
    for (k, v) in [
        ("GIT_PAGER", "more.com"), ("PAGER", "more.com"), ("LESS", ""),
        ("SBX_NONET_ACTIVE", "1"), ("HTTP_PROXY", "http://127.0.0.1:9"),
        ("HTTPS_PROXY", "http://127.0.0.1:9"), ("ALL_PROXY", "http://127.0.0.1:9"),
        ("NO_PROXY", "localhost,127.0.0.1,::1"), ("PIP_NO_INDEX", "1"),
        ("PIP_DISABLE_PIP_VERSION_CHECK", "1"), ("NPM_CONFIG_OFFLINE", "true"),
        ("CARGO_NET_OFFLINE", "true"), ("GIT_HTTP_PROXY", "http://127.0.0.1:9"),
        ("GIT_HTTPS_PROXY", "http://127.0.0.1:9"), ("GIT_SSH_COMMAND", "cmd /c exit 1"),
        ("GIT_ALLOW_PROTOCOLS", ""),
    ] { child.insert(k.into(), v.into()); }
    let base = profile.join(".sbx-denybin").to_string_lossy().into_owned();
    let path = child.get("PATH").cloned().unwrap_or_default();
    if !path.split(';').next().unwrap_or_default().eq_ignore_ascii_case(&base) {
        child.insert("PATH".into(), if path.is_empty() { base } else { format!("{base};{path}") });
    }
    let exts: Vec<_> = child["PATHEXT"].split(';').filter(|v| !v.is_empty()).collect();
    let mut ordered = Vec::new();
    for want in [".BAT", ".CMD"] {
        if let Some(value) = exts.iter().find(|v| v.eq_ignore_ascii_case(want)) { ordered.push(*value); }
    }
    ordered.extend(exts.iter().copied().filter(|v| !v.eq_ignore_ascii_case(".BAT") && !v.eq_ignore_ascii_case(".CMD")));
    child.insert("PATHEXT".into(), ordered.join(";"));
    Ok(child)
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
fn expected_terminal(step: &str) -> &'static str {
    if step == "W" {
        "SUCCESS"
    } else {
        "RESULT_ERROR"
    }
}
fn check_process(process: &Value, e: &Execution, step: &str) -> Result<()> {
    if e.pid.is_none()
        || e.process_creation_utc.is_none()
        || process["pid"].as_u64() != e.pid.map(|v| v as u64)
        || process["creation"].as_str() != e.process_creation_utc.as_deref()
    {
        return Err("PROBE_PROCESS_RECEIPT_REJECTED");
    }
    if e.natural
        && (e.exit_code != Some(if step == "W" { 0 } else { 23 })
            || e.terminal != expected_terminal(step))
    {
        return Err("PROBE_CLI_EXIT_REJECTED");
    }
    Ok(())
}
// Only this exact fixed parser message can be shown; arbitrary CLI stderr stays in the raw receipt.
const MISSING_PROFILE_STDERR: &str = "error: the following required arguments were not provided:\n  --permission-profile <NAME>\n\nUsage: codex sandbox --permission-profile <NAME> --cd <DIR> --config <key=value> <COMMAND>...\n\nFor more information, try '--help'.\n";
fn safe_stderr(e: &Execution) -> Option<&'static str> {
    std::str::from_utf8(&e.stderr)
        .ok()
        .filter(|v| v.replace("\r\n", "\n") == MISSING_PROFILE_STDERR)
        .map(|_| MISSING_PROFILE_STDERR)
}
/// Observations of a persisted attempt, not a claim that this error-reporting invocation launched it.
fn failure_at(root: &Path, operation: &str, step: &str, code: &str) -> Value {
    let mut result = json!({"error_code":code,"admission":"NOT_VERIFIED",
        "model_started":false,"network_executed":false,"network_scope":"MODEL_REQUESTS",
        "sandbox_helper_execution":"UNKNOWN","attempt_observation":null});
    if operation == "permission-helper" {
        result["sandbox_helper_execution"] = json!("HELPER_HANDLER_ENTERED_NOT_VERIFIED");
        return result;
    }
    if !["permission-run", "permission-evaluate"].contains(&operation) || phase(step).is_err() {
        return result;
    }
    let x = load::<Exit>(&root.join("evidence").join(format!("{step}.exit.json"))).ok();
    let process = load::<Value>(&root.join("evidence").join(format!("{step}.process.json"))).ok();
    let started = root
        .join("evidence")
        .join(format!("{step}.started.json"))
        .exists();
    let e = x.as_ref().map(|v| &v.execution);
    let process_has_identity = process.as_ref().is_some_and(|p| {
        p["pid"].as_u64().is_some_and(|v| v > 0)
            && p["creation"].as_str().is_some_and(|v| !v.is_empty())
    });
    let observed_start = match e {
        Some(v) if !v.created && process_has_identity => None,
        Some(v) => Some(v.created),
        None if process_has_identity => Some(true),
        None => None,
    };
    let identity = e.zip(process.as_ref()).map(|(e, p)| {
        e.pid.is_some()
            && e.process_creation_utc.is_some()
            && p["pid"].as_u64() == e.pid.map(|v| v as u64)
            && p["creation"].as_str() == e.process_creation_utc.as_deref()
    });
    let safe = e.and_then(safe_stderr);
    result["attempt_observation"] = json!({"scope":"PERSISTED_PHASE_ATTEMPT_NOT_THIS_INVOCATION",
        "phase":step,"started_record_present":started,"cli_process_started":observed_start,
        "process_receipt_present":process.is_some(),"exit_receipt_present":x.is_some(),
        "process_identity_matches":identity,"pid":e.and_then(|v|v.pid),
        "creation":e.and_then(|v|v.process_creation_utc.as_deref()),
        "exit_code":e.and_then(|v|v.exit_code),"terminal":e.map(|v|v.terminal.as_str()),
        "natural_exit":e.map(|v|v.natural),"reclaim":e.map(|v|v.reclaim.as_str()),
        "cleanup":x.as_ref().and_then(|v|v.cleanup.as_ref()),
        "safe_stderr":safe,"stderr_policy":"EXACT_FIXED_ALLOWLIST_OTHERWISE_WITHHELD",
        "stderr_bytes":e.map(|v|v.stderr.len()),
        "raw_stderr_evidence":format!("evidence/{step}.exit.json#execution.stderr")});
    if e.is_some_and(|v| v.natural && v.exit_code == Some(2) && v.stdout.is_empty())
        && safe.is_some()
    {
        result["sandbox_helper_execution"] =
            json!("ARGUMENT_REJECTION_BEFORE_DISPATCH_INFERRED_FROM_CLI");
    }
    if let Some(e) = e {
        let tool = api_json(&e.stdout, 16384)
            .ok()
            .and_then(|v| serde_json::from_value::<ToolReceipt>(v).ok());
        let p = load::<Plan>(&root.join("plan.json")).ok();
        if tool.as_ref().zip(p.as_ref()).is_some_and(|(t, p)| {
            t.schema == "nova.self2.permission-tool/1"
                && t.nonce == p.nonce
                && t.phase == step
                && t.target == target(step)
                && ["WROTE", "DENIED", "UNEXPECTED_WRITE_OPEN"].contains(&t.outcome.as_str())
        }) {
            result["sandbox_helper_execution"] = json!("FIXED_TOOL_RECEIPT_OBSERVED_NOT_ADMISSION");
        }
    }
    result
}
pub fn failure(operation: &str, step: &str, code: &str) -> Value {
    failure_at(Path::new(ROOT), operation, step, code)
}
fn check(root: &Path, step: &str) -> Result<Value> {
    phase(step)?;
    let _guards = pinned(root)?;
    let p = validate(root)?;
    let s: Start = load(&root.join("evidence").join(format!("{step}.started.json")))?;
    let x: Exit = load(&root.join("evidence").join(format!("{step}.exit.json")))?;
    let e = &x.execution;
    let process: Value = load(&root.join("evidence").join(format!("{step}.process.json")))?;
    check_process(&process, e, step)?;
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
        || e.terminal != expected_terminal(step)
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
        json!({"schema":"nova.self2.permission-verdict/1","phase":step,"tool_verified":true,"cli_process_started":true,"sandbox_helper_execution":"VERIFIED_FIXED_TOOL_RECEIPT","model_started":false,"model_admission":false,"plan_digest":s.digest,"command_exit":e.exit_code,"pid":e.pid,"creation":e.process_creation_utc,"reclaim":e.reclaim,"tool_receipt":r,"current_files":current(root)?,"stdout_hash":hash(&e.stdout),"stderr_hash":hash(&e.stderr)}),
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
        schema: "nova.self2.permission-probe/2".into(),
        root_identity: Guard::open(root, true)?.identity,
        nonce: uuid::Uuid::new_v4().to_string(),
        cli_hash: self_codex::CLI_SHA.into(),
        environment: env(root)?,
        helper_environment: helper_environment(root, &env(root)?)?,
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
        env: p.environment.clone(),
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
    helper_at(Path::new(ROOT), step, nonce, &std::env::vars().collect())
}
fn helper_at(root: &Path, step: &str, nonce: &str, environment: &BTreeMap<String, String>) -> Result<(Value, i32)> {
    phase(step)?;
    let _guards = pinned(root)?;
    let p = validate_environment(root, Some(environment))?;
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
