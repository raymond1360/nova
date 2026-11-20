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
pub const ROOT: &str = r"D:\nova-self2-permission-probe-1119e";
pub const PROJECT_ROOT: &str = r"D:\nova-self2-permission-project-1120a";
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    project: Option<ProjectLayout>,
}
/// V3: ordinary standalone Git workspace plus ignored, immutable permission sentinels.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProjectLayout {
    workspace_identity: String,
    git_identity: String,
    targets_identity: String,
    git_config: Fact,
    git_exclude: Fact,
    git_ignore: Fact,
}
const PROJECT_TARGETS: &str = ".nova-permission";
fn project_layout(root: &Path) -> Result<ProjectLayout> {
    let workspace=root.join("workspace");let git=workspace.join(".git");
    // No ancestor discovery, gitdir pointers, worktrees or common config indirection.
    for name in ["commondir","config.worktree","worktrees","info/attributes"] {
        match fs::symlink_metadata(git.join(name)) {
            Err(e) if e.kind()==std::io::ErrorKind::NotFound => {},
            _ => return Err("PROBE_PROJECT_LAYOUT_REJECTED"),
        }
    }
    let ignore=fact(&workspace.join(".gitignore"))?;
    if fs::read(workspace.join(".gitignore")).map_err(|_|"PROBE_READ")? != b"/.nova-permission/\n" {
        return Err("PROBE_PROJECT_LAYOUT_REJECTED");
    }
    Ok(ProjectLayout { workspace_identity: Guard::open(&workspace,true)?.identity,
        git_identity: Guard::open(&git,true)?.identity,
        targets_identity: Guard::open(&workspace.join(PROJECT_TARGETS),true)?.identity,
        git_config: fact(&git.join("config"))?,git_exclude:fact(&git.join("info/exclude"))?,git_ignore:ignore })
}
fn target_directory(root: &Path, p: &Plan) -> PathBuf {
    let workspace=root.join("workspace");
    if p.project.is_some() {workspace.join(PROJECT_TARGETS)} else {workspace}
}
fn current_for(root: &Path, p: &Plan) -> Result<BTreeMap<String,Fact>> {
    let mut out=BTreeMap::new();
    for entry in fs::read_dir(target_directory(root,p)).map_err(|_|"PROBE_READ")? {
        let entry=entry.map_err(|_|"PROBE_READ")?;
        let name=entry.file_name().into_string().map_err(|_|"PROBE_PATH")?;
        out.insert(name,fact(&entry.path())?);
    }
    Ok(out)
}
/// Exact scope of the accepted non-model fixture, not a transferable sandbox certificate.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PermissionScope {
    pub project_root: PathBuf,
    pub cli: PathBuf,
    pub cli_sha256: String,
    pub controller: PathBuf,
    pub controller_sha256: String,
    pub permission_plan_sha256: String,
}
pub(super) struct PermissionBinding {
    pub(super) scope: PermissionScope,
    fingerprint: String,
}
impl PermissionBinding {
    pub(super) fn fingerprint(&self) -> &str { &self.fingerprint }
}
#[derive(Clone)]
struct AcceptedPermission {
    scope: PermissionScope,
    receipts: BTreeMap<String, String>,
}
pub fn permission_scope_1119e() -> PermissionScope {
    PermissionScope {
        project_root: PathBuf::from(r"D:\nova-self2-permission-probe-1119e\workspace"),
        cli: CLI.into(), cli_sha256: self_codex::CLI_SHA.into(),
        controller: PathBuf::from(r"D:\nova\audit\self2-permission-ready-1119e\target\debug\nova_scheduler_self_codex.exe"),
        controller_sha256: "e999703a84c517e46981e4d30da777b2675729627ba8f95d237eaf223402f7f8".into(),
        permission_plan_sha256: "5c1239902fe69b5c4198f0d5bda65e18ac7570d57e43ae2b70645a2aae05f6cc".into(),
    }
}
fn accepted_1119e() -> AcceptedPermission {
    // Raw tool receipts from the accepted object, not a report PASS or caller-supplied digest.
    AcceptedPermission { scope: permission_scope_1119e(), receipts: [
        ("W.started.json", "a7cbbb3f6325f950cd36c3f9e8f22b0cc1210c257e90be577f50055c6afc58bf"),
        ("W.process.json", "ba71cbbddb475f4a8ab72cdfe0259c54c10e2476a4cfe65f6ed5a50fa90df15b"),
        ("W.exit.json", "9a6e6022927fb12cbcd145c511c67f0f44bf0d9708604a0b6167c76c22e4e5e3"),
        ("R.started.json", "7ccf2e1880787bedc9465caaf2369d48f162b2af134b9aac105cc08491083017"),
        ("R.process.json", "1c88c0916ff256fd092a8f2bf1e55f1d7de3ddda3a4ad3f523428067929aebc8"),
        ("R.exit.json", "0fe64f27cee49154edd56a24a95e685697c77fc65b3979ad034da0b82d18217f"),
    ].into_iter().map(|(n,h)|(n.into(),h.into())).collect() }
}
fn receipt_hashes(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for entry in fs::read_dir(root.join("evidence")).map_err(|_| "SELF2_PERMISSION_EVIDENCE_REQUIRED")?.take(7) {
        let entry=entry.map_err(|_| "SELF2_PERMISSION_EVIDENCE_REQUIRED")?;
        let n=entry.file_name().into_string().map_err(|_| "SELF2_PERMISSION_EVIDENCE_REJECTED")?;
        if !["W.started.json","W.process.json","W.exit.json","R.started.json","R.process.json","R.exit.json"].contains(&n.as_str()) {
            return Err("SELF2_PERMISSION_EVIDENCE_REJECTED");
        }
        out.insert(n, fact(&entry.path())?.sha256);
    }
    if out.len()!=6 {return Err("SELF2_PERMISSION_EVIDENCE_REQUIRED")}
    Ok(out)
}
fn permission_at(root: &Path, scope: &PermissionScope, accepted: &AcceptedPermission, actual_cli_hash: &str) -> Result<PermissionBinding> {
    if scope!=&accepted.scope || scope.project_root!=root.join("workspace") || actual_cli_hash!=scope.cli_sha256 {
        return Err("SELF2_PERMISSION_SCOPE_MISMATCH");
    }
    let _pins=pinned(root)?;
    if fact(&root.join("plan.json"))?.sha256!=scope.permission_plan_sha256 || receipt_hashes(root)?!=accepted.receipts {
        return Err("SELF2_PERMISSION_EVIDENCE_REJECTED");
    }
    // Reuse the original environment/config/control, program, process, tool, cleanup and NOW-file guards.
    let p=validate_bound(root, None, &scope.controller,&scope.cli_sha256)?;
    if p.helper_hash!=scope.controller_sha256 || p.cli_hash!=scope.cli_sha256 {
        return Err("SELF2_PERMISSION_SCOPE_MISMATCH");
    }
    check_bound(root,"R",&scope.controller,&scope.cli_sha256)?; // recursively rechecks W; no run/evaluate executable invoked.
    if receipt_hashes(root)?!=accepted.receipts {return Err("SELF2_PERMISSION_EVIDENCE_REJECTED")}
    Ok(PermissionBinding { scope: scope.clone(), fingerprint: hash(&bytes(&json!({"scope":scope,"receipts":accepted.receipts}))?) })
}
// Consumer-only registration after real W/R and independent acceptance. Never overwrite the helper.
fn accepted_project() -> Option<AcceptedPermission> { None }
pub(super) fn bind_permission(scope: &PermissionScope) -> Result<PermissionBinding> {
    #[cfg(test)]
    if SYNTHETIC_PENDING.with(|v|v.borrow().as_ref()==Some(scope))
        && !SYNTHETIC_ACCEPTED.with(|v|v.borrow().as_ref().is_some_and(|a|a.scope==*scope)) {
        return Err("SELF2_PERMISSION_OBJECT_NOT_ACCEPTED");
    }

    #[cfg(test)]
    if let Some(accepted)=SYNTHETIC_ACCEPTED.with(|v|v.borrow().clone()).filter(|v|v.scope==*scope) {
        return permission_at(scope.project_root.parent().ok_or("PROBE_PATH")?,scope,&accepted,&tool_hash(&scope.cli)?);
    }

    let root=if scope.project_root==Path::new(PROJECT_ROOT).join("workspace") {Path::new(PROJECT_ROOT)}else{Path::new(ROOT)};
    let accepted=if root==Path::new(PROJECT_ROOT) {accepted_project().ok_or("SELF2_PERMISSION_OBJECT_NOT_ACCEPTED")?}else{accepted_1119e()};
    if scope!=&accepted.scope {return Err("SELF2_PERMISSION_SCOPE_MISMATCH")}
    permission_at(root,scope,&accepted,&tool_hash(&scope.cli)?)
}
#[cfg(test)]
thread_local! {
    static SYNTHETIC_ACCEPTED: std::cell::RefCell<Option<AcceptedPermission>> = const { std::cell::RefCell::new(None) };
    static SYNTHETIC_PENDING: std::cell::RefCell<Option<PermissionScope>> = const { std::cell::RefCell::new(None) };
}
#[cfg(test)]
pub(super) fn synthetic_cli_binding(root: &Path, cli: &Path, sha: &str) -> bool {
    SYNTHETIC_ACCEPTED.with(|v|v.borrow().as_ref().is_some_and(|a|a.scope.project_root==root && a.scope.cli==cli && a.scope.cli_sha256==sha))
        || SYNTHETIC_PENDING.with(|v|v.borrow().as_ref().is_some_and(|s|s.project_root==root && s.cli==cli && s.cli_sha256==sha))
}
/// Read-only preparation interface. A binding is rechecked on each call and is never a launch permit.
pub fn permission_preflight(scope: &PermissionScope) -> Result<Value> {
    let binding=bind_permission(scope)?;
    Ok(json!({"schema":"nova.self2.permission-binding/1","scope":binding.scope,"permission_fingerprint":binding.fingerprint(),"permission_verified":true,"model_authorization":"REQUIRED_SEPARATELY","model_admission":false,"real_codex_started":false}))
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
    validate_environment_for(root, helper, &std::env::current_exe().map_err(|_| "SELF_PATH")?)
}
fn validate_environment_for(root: &Path, helper: Option<&BTreeMap<String, String>>, expected_helper: &Path) -> Result<Plan> {
    validate_bound(root,helper,expected_helper,self_codex::CLI_SHA)
}
fn validate_bound(root: &Path, helper: Option<&BTreeMap<String,String>>, expected_helper: &Path, expected_cli: &str) -> Result<Plan> {
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
    if p.helper != expected_helper
        || uuid::Uuid::parse_str(&p.nonce).is_err()
    {
        return Err("PROBE_BINDING_REJECTED");
    }
    if !matches!((&*p.schema,&p.project),("nova.self2.permission-probe/2",None)|("nova.self2.permission-probe/3",Some(_)))
        || p.cli_hash != expected_cli
        || p.root_identity != Guard::open(root, true)?.identity
        || tool_hash(&p.helper)? != p.helper_hash
    {
        return Err("PROBE_BINDING_REJECTED");
    }
    if let Some(expected)=&p.project {
        if &project_layout(root)?!=expected {return Err("PROBE_PROJECT_DRIFT")}
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
        if p.project.is_some() {"permission-project-helper"}else{"permission-helper"}.into(),
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
    if ["permission-helper","permission-project-helper"].contains(&operation) {
        result["sandbox_helper_execution"] = json!("HELPER_HANDLER_ENTERED_NOT_VERIFIED");
        return result;
    }
    if !["permission-run","permission-evaluate","permission-project-run","permission-project-evaluate"].contains(&operation) || phase(step).is_err() {
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
    failure_at(if operation.starts_with("permission-project-"){Path::new(PROJECT_ROOT)}else{Path::new(ROOT)},operation,step,code)
}
fn check(root: &Path, step: &str) -> Result<Value> {
    check_for(root, step, &std::env::current_exe().map_err(|_| "SELF_PATH")?)
}
fn check_for(root: &Path, step: &str, expected_helper: &Path) -> Result<Value> {
    check_bound(root,step,expected_helper,self_codex::CLI_SHA)
}
fn check_bound(root: &Path, step: &str, expected_helper: &Path, expected_cli: &str) -> Result<Value> {
    phase(step)?;
    let _guards = pinned(root)?;
    let p = validate_bound(root, None, expected_helper, expected_cli)?;
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
    if current_for(root,&p)? != expected(&p, step, true) {
        return Err("PROBE_CURRENT_FILE_DRIFT");
    }
    if step == "R" {
        check_bound(root, "W", expected_helper, expected_cli)?;
    }
    Ok(
        json!({"schema":"nova.self2.permission-verdict/1","phase":step,"tool_verified":true,"cli_process_started":true,"sandbox_helper_execution":"VERIFIED_FIXED_TOOL_RECEIPT","model_started":false,"model_admission":false,"plan_digest":s.digest,"command_exit":e.exit_code,"pid":e.pid,"creation":e.process_creation_utc,"reclaim":e.reclaim,"tool_receipt":r,"current_files":current_for(root,&p)?,"stdout_hash":hash(&e.stdout),"stderr_hash":hash(&e.stderr)}),
    )
}

// Only fixed preparation uses this command list; never accepts user/model commands.
fn project_git(workspace: &Path, args: &[&str]) -> Result<String> {
    use std::process::{Command, Stdio};
    use std::os::windows::process::CommandExt;
    let mut c=Command::new(r"C:\Program Files\Git\cmd\git.exe");
    c.env_clear().env("GIT_CONFIG_NOSYSTEM","1").env("GIT_CONFIG_GLOBAL","NUL")
        .env("GIT_TERMINAL_PROMPT","0").env("GIT_NO_REPLACE_OBJECTS","1");
    if let Some(v)=std::env::var_os("SystemRoot") {c.env("SystemRoot",v);}
    c.args(["-c","init.templateDir=","-c","core.hooksPath=NUL","-c","commit.gpgsign=false",
        "-c","core.autocrlf=false","-c","user.name=Nova SELF2 fixture","-c","user.email=fixture@invalid"])
        .arg("-C").arg(workspace).args(args).creation_flags(0x08000000)
        .stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped());
    let mut child=c.spawn().map_err(|_|"PROBE_PROJECT_GIT_START")?;
    let deadline=Instant::now()+Duration::from_secs(10);
    let status=loop {
        match child.try_wait() {
            Ok(Some(s))=>break s,
            Ok(None) if Instant::now()<deadline=>std::thread::sleep(Duration::from_millis(10)),
            _=>{let _=child.kill();let _=child.wait();return Err("PROBE_PROJECT_GIT_WAIT");}
        }
    };
    if !status.success(){return Err("PROBE_PROJECT_GIT_EXIT")}
    let mut out=Vec::new();child.stdout.take().ok_or("PROBE_PROJECT_GIT_OUTPUT")?.take(4097)
        .read_to_end(&mut out).map_err(|_|"PROBE_PROJECT_GIT_OUTPUT")?;
    if out.len()>4096{return Err("PROBE_PROJECT_GIT_OUTPUT")}
    Ok(String::from_utf8(out).map_err(|_|"PROBE_PROJECT_GIT_OUTPUT")?.trim().into())
}
fn project_scope(root:&Path,p:&Plan,cli:&Path)->Result<PermissionScope>{
    Ok(PermissionScope {project_root:root.join("workspace"),cli:cli.into(),cli_sha256:p.cli_hash.clone(),
        controller:p.helper.clone(),controller_sha256:p.helper_hash.clone(),
        permission_plan_sha256:hash(&super::store::read_file(&root.join("plan.json"),1048576)?.0)})
}
// Shared production constructor. Only the fixed wrapper is public; tests substitute identities.
fn prepare_project_at(root:&Path,helper:&Path,cli:&Path,cli_hash:&str)->Result<Value>{
    if root.exists(){return Err("PROBE_ROOT_ALREADY_EXISTS")}
    super::store::approved_root(root,false)?;
    let _parent=Guard::open(root.parent().ok_or("PROBE_ROOT_REJECTED")?,true)?;
    if tool_hash(cli)?!=cli_hash{return Err("PROBE_CLI_REJECTED")}
    let helper_hash=tool_hash(helper)?;
    fs::create_dir(root).map_err(|_|"PROBE_CREATE")?;
    for n in ["workspace","control","evidence","home","tmp","reports"] {
        fs::create_dir(root.join(n)).map_err(|_|"PROBE_CREATE")?;
    }
    let _guards=pinned(root)?;
    let workspace=root.join("workspace");
    fs::create_dir(workspace.join(PROJECT_TARGETS)).map_err(|_|"PROBE_CREATE")?;
    for n in ["write.txt","readonly.txt"] {fs::write(workspace.join(PROJECT_TARGETS).join(n),BEFORE).map_err(|_|"PROBE_CREATE")?;}
    fs::write(workspace.join(".gitignore"),b"/.nova-permission/\n").map_err(|_|"PROBE_CREATE")?;
    fs::write(workspace.join("a.txt"),b"SELF2_CANARY_BASE\n").map_err(|_|"PROBE_CREATE")?;
    project_git(&workspace,&["init","-q","--initial-branch=self2-canary"])?;
    fs::create_dir_all(workspace.join(".git/info")).map_err(|_|"PROBE_CREATE")?;
    fs::write(workspace.join(".git/info/exclude"),b"").map_err(|_|"PROBE_CREATE")?;
    project_git(&workspace,&["add","--","a.txt",".gitignore"])?;
    project_git(&workspace,&["commit","--no-gpg-sign","--no-verify","-qm","canary base"])?;
    let base=project_git(&workspace,&["rev-parse","HEAD"])?;
    fs::write(workspace.join("a.txt"),b"SELF2_CANARY_PENDING\n").map_err(|_|"PROBE_CREATE")?;
    project_git(&workspace,&["add","--","a.txt"])?;
    project_git(&workspace,&["commit","--no-gpg-sign","--no-verify","-qm","canary pending"])?;
    let head=project_git(&workspace,&["rev-parse","HEAD"])?;
    let source=json!({"path":"a.txt","allowed_paths":["a.txt"],"base_sha":base,"head_sha":head,
        "initial_text":"SELF2_CANARY_PENDING\n","candidate_text":"SELF2_CANARY_READY\n","model_authorized":false});
    fresh(&root.join("control/instructions.txt"),&json!({"purpose":"NON-MODEL permission only; no model authorization. ACL/capability/NUL and denybin initialization require separate approval.","source_contract":source}))?;
    let environment=env(root)?;
    let p=Plan {schema:"nova.self2.permission-probe/3".into(),root_identity:Guard::open(root,true)?.identity,
        nonce:uuid::Uuid::new_v4().to_string(),cli_hash:cli_hash.into(),helper:helper.into(),helper_hash,
        helper_environment:helper_environment(root,&environment)?,environment,
        controls:BTreeMap::from([("instructions.txt".into(),fact(&root.join("control/instructions.txt"))?)]),
        targets:BTreeMap::from([("write.txt".into(),fact(&workspace.join(PROJECT_TARGETS).join("write.txt"))?),("readonly.txt".into(),fact(&workspace.join(PROJECT_TARGETS).join("readonly.txt"))?)]),project:Some(project_layout(root)?)};
    fresh(&root.join("plan.json"),&p)?;
    validate_bound(root,None,helper,cli_hash)?;
    Ok(json!({"prepared_only":true,"accepted":false,"model_admission":false,"root":root,"plan":p,
        "scope":project_scope(root,&p,cli)?,"digest":hash(&bytes(&p)?),"source_contract":source,
        "write_argv":argv(root,&p,"W"),"readonly_argv":argv(root,&p,"R"),"sandbox_started":false}))
}
pub fn prepare_project()->Result<Value>{
    let helper=std::env::current_exe().map_err(|_|"SELF_PATH")?;
    if helper.file_name().and_then(|n|n.to_str())!=Some("nova_scheduler_self_codex.exe"){return Err("PROBE_CLI_REJECTED")}
    prepare_project_at(Path::new(PROJECT_ROOT),&helper,Path::new(CLI),self_codex::CLI_SHA)
}
pub fn project_status()->Result<Value>{
    let root=Path::new(PROJECT_ROOT);let p:Plan=load(&root.join("plan.json"))?;
    let _guards=pinned(root)?;
    if p.project.is_none(){return Err("PROBE_PLAN_REJECTED")}
    validate_bound(root,None,&p.helper,self_codex::CLI_SHA)?;
    let scope=project_scope(root,&p,Path::new(CLI))?;
    let accepted=match bind_permission(&scope){Ok(_)=>true,Err("SELF2_PERMISSION_OBJECT_NOT_ACCEPTED")=>false,Err(e)=>return Err(e)};
    Ok(json!({"scope":scope,"accepted":accepted,"model_admission":false}))
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
        project: None,
    };
    fresh(&root.join("plan.json"), &p)?;
    Ok(
        json!({"prepared_only":true,"root":ROOT,"plan":p,"digest":hash(&bytes(&p)?),"write_argv":argv(root,&p,"W"),"readonly_argv":argv(root,&p,"R"),"sandbox_started":false,"approval_required":"AUTHORIZE NON-MODEL ACL NUL <phase> <digest>"}),
    )
}
/// Permission-only native path: fixed CLI, fixed root, generated command, interactive approval.
pub fn run(step: &str, digest: &str, confirmation: &str) -> Result<Value> {
    run_at(Path::new(ROOT),step,digest,confirmation)
}
pub fn run_project(step: &str, digest: &str, confirmation: &str) -> Result<Value> {
    run_at(Path::new(PROJECT_ROOT),step,digest,confirmation)
}
fn run_at(root: &Path, step: &str, digest: &str, confirmation: &str) -> Result<Value> {
    phase(step)?;
    if !super::contract::hex(digest, 64)
        || confirmation != format!("AUTHORIZE NON-MODEL ACL NUL {step} {digest}")
    {
        return Err("PROBE_EXPLICIT_APPROVAL_REQUIRED");
    }
    let _guards = pinned(root)?;
    let p = validate(root)?;
    let actual = hash(&bytes(&p)?);
    if actual != digest || confirmation != format!("AUTHORIZE NON-MODEL ACL NUL {step} {actual}") {
        return Err("PROBE_EXPLICIT_APPROVAL_REQUIRED");
    }
    if step == "R" {
        check(root, "W")?;
    }
    if current_for(root,&p)? != expected(&p, step, false) {
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
        .open(target_directory(root,&p).join(target(step)))
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
        before: current_for(root,&p)?,
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
pub fn evaluate_project(step: &str) -> Result<Value> {check(Path::new(PROJECT_ROOT),step)}
pub fn helper_project(step: &str, nonce: &str) -> Result<(Value,i32)> {helper_at(Path::new(PROJECT_ROOT),step,nonce,&std::env::vars().collect())}
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
    if current_for(root,&p)? != expected(&p, step, false) {
        return Err("PROBE_CURRENT_FILE_DRIFT");
    }
    let path = target_directory(root,&p).join(target(step));
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
