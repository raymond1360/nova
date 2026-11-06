//! Fixed, hash-bound profiles. Real execution remains disabled before any reservation.
use super::{
    contract::*,
    relay::{Driver, Execution},
    store::{Guard, LaunchPermit},
    Result,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
pub const DEVELOP_PROFILE: &str = "codex-exec-0.162.1";
pub const REVIEW_PROFILE: &str = "claude-print-2.1.197";
pub const DEVELOP_HASH: &str = "dd13bdb162a184fa04039488b234a8258ef9b76f84e926c505d5742b19249a6a";
pub const REVIEW_HASH: &str = "038bd9fe90c60304601e19751269a50d62925c541dd6a2b3da5274549e9416ee";
pub const ENV_ALLOW: &[&str] = &[
    "SystemRoot",
    "windir",
    "SystemDrive",
    "ProgramData",
    "ProgramFiles",
    "ProgramFiles(x86)",
    "CommonProgramFiles",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "USERNAME",
    "COMPUTERNAME",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "OS",
    "PATHEXT",
    "COMSPEC",
];
pub fn forbidden_env(k: &str) -> bool {
    let k = k.to_ascii_uppercase();
    [
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "CREDENTIAL",
        "AUTH",
        "COOKIE",
    ]
    .iter()
    .any(|x| k.contains(x))
        || [
            "ANTHROPIC_",
            "CLAUDE_",
            "CODEX_",
            "OPENAI_",
            "AWS_",
            "GOOGLE_",
            "AZURE_",
            "VERTEX",
            "BEDROCK",
            "HTTP_PROXY",
            "HTTPS_PROXY",
            "ALL_PROXY",
        ]
        .iter()
        .any(|x| k.starts_with(x))
        || k == "NODE_OPTIONS"
}
pub fn environment(
    parent: &BTreeMap<String, String>,
    root: &Path,
) -> Result<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    for k in ENV_ALLOW {
        let hits: Vec<_> = parent
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(k))
            .collect();
        if hits.len() > 1 {
            return Err("ENV_DUPLICATE");
        }
        if let Some((_, v)) = hits.first() {
            if !forbidden_env(k) {
                if v.contains('\0') {
                    return Err("ENV_NUL");
                }
                out.insert((*k).into(), (*v).clone());
            }
        }
    }
    let sr = out.get("SystemRoot").ok_or("SYSTEMROOT_MISSING")?.clone();
    out.insert("PATH".into(), format!("{sr}\\System32;{sr}"));
    for k in ["TEMP", "TMP"] {
        out.insert(
            k.into(),
            root.join("tmp").to_str().ok_or("PATH_REJECTED")?.into(),
        );
    }
    Ok(out)
}
pub fn proposal_schema() -> String {
    r#"{"type":"object","additionalProperties":false,"required":["schema","target_path","old_text","new_text","rationale"],"properties":{"schema":{"const":"nova.relay.proposal/1"},"target_path":{"const":"pilot/src/normalize_label.rs"},"old_text":{"type":"string","minLength":1,"maxLength":8192},"new_text":{"type":"string","maxLength":16384},"rationale":{"type":"string","maxLength":2048}}}"#.into()
}
pub fn review_schema() -> String {
    r#"{"type":"object","additionalProperties":false,"required":["schema","base_sha256","candidate_sha256","verdict","issues","unknowns"],"properties":{"schema":{"const":"nova.relay.review/1"},"base_sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"candidate_sha256":{"type":"string","pattern":"^[0-9a-f]{64}$"},"verdict":{"enum":["PASS","CHANGES_REQUIRED","BLOCKED"]},"issues":{"type":"array","maxItems":32,"items":{"type":"object","additionalProperties":false,"required":["id","severity","blocking","location","summary"],"properties":{"id":{"type":"string","pattern":"^R-[0-9]{1,3}$"},"severity":{"enum":["BLOCKER","HIGH","MEDIUM","LOW","INFO"]},"blocking":{"type":"boolean"},"location":{"type":"string","maxLength":256},"summary":{"type":"string","maxLength":1024}}}},"unknowns":{"type":"array","maxItems":16,"items":{"type":"string","maxLength":512}}}}"#.into()
}
pub fn argv(role: RelayRole, root: &Path, model: &str, session: &str) -> Vec<String> {
    let r = root.to_string_lossy();
    if role == RelayRole::Develop {
        vec![
            "--no-daemon".into(),
            "-a".into(),
            "never".into(),
            "exec".into(),
            "-C".into(),
            format!("{r}\\develop-cwd"),
            "--skip-git-repo-check".into(),
            "--ignore-user-config".into(),
            "--sandbox".into(),
            "read-only".into(),
            "--ephemeral".into(),
            "--json".into(),
            "--color".into(),
            "never".into(),
            "-m".into(),
            model.into(),
            "--output-schema".into(),
            format!("{r}\\proposal.schema.json"),
            "-".into(),
        ]
    } else {
        vec![
            "--safe-mode".into(),
            "--print".into(),
            "--input-format".into(),
            "text".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--verbose".into(),
            "--json-schema".into(),
            review_schema(),
            "--tools".into(),
            "".into(),
            "--permission-mode".into(),
            "dontAsk".into(),
            "--strict-mcp-config".into(),
            "--mcp-config".into(),
            "{\"mcpServers\":{}}".into(),
            "--disable-slash-commands".into(),
            "--no-chrome".into(),
            "--no-session-persistence".into(),
            "--session-id".into(),
            session.into(),
            "--model".into(),
            model.into(),
        ]
    }
}
pub struct Spec {
    pub role: RelayRole,
    pub profile: String,
    pub exe: PathBuf,
    pub exe_sha256: String,
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub session: String,
}
impl Spec {
    pub fn argv_sha256(&self) -> String {
        hash(&serde_json::to_vec(&self.argv).unwrap())
    }
    pub fn env_names_sha256(&self) -> String {
        let mut names: Vec<_> = self.env.keys().cloned().collect();
        names.sort_by_key(|s| s.to_ascii_uppercase());
        hash(&serde_json::to_vec(&names).unwrap())
    }
}
pub struct Parser {
    role: RelayRole,
    session: String,
    pending: Vec<u8>,
    total: usize,
    first: bool,
    started: bool,
    init: bool,
    pub terminal: Option<String>,
    pub claim: Option<Vec<u8>>,
    pub usage: Option<Value>,
    pub unknowns: Vec<String>,
    auth_failed: bool,
    retry_count: u32,
    retry_classes: BTreeMap<String, u32>,
}
impl Parser {
    pub fn new(role: RelayRole, session: &str) -> Self {
        Self {
            role,
            session: session.into(),
            pending: vec![],
            total: 0,
            first: false,
            started: false,
            init: false,
            terminal: None,
            claim: None,
            usage: None,
            unknowns: vec![],
            auth_failed: false,
            retry_count: 0,
            retry_classes: BTreeMap::new(),
        }
    }
    pub fn feed(&mut self, b: &[u8]) -> Result<()> {
        self.total = self.total.checked_add(b.len()).ok_or("BOUND")?;
        if self.total > 8388608 {
            return Err("BOUND");
        }
        for c in b {
            if *c == b'\n' {
                let line = std::mem::take(&mut self.pending);
                if line.is_empty() {
                    continue;
                }
                self.event(&line)?;
            } else {
                self.pending.push(*c);
                if self.pending.len() > 1048576 {
                    return Err("BOUND");
                }
            }
        }
        Ok(())
    }
    fn event(&mut self, line: &[u8]) -> Result<()> {
        if self.terminal.is_some() {
            return Err("PROTOCOL_ERROR");
        }
        let v: Value = relay_json(line, 1048576).map_err(|_| "PROTOCOL_ERROR")?;
        if !v.is_object() {
            return Err("PROTOCOL_ERROR");
        }
        let typ = v["type"].as_str().ok_or("PROTOCOL_ERROR")?;
        if self.role == RelayRole::Develop {
            if !self.first && typ != "thread.started" {
                return Err("PROTOCOL_ERROR");
            }
            match typ {
                "thread.started" => {
                    if self.first {
                        return Err("PROTOCOL_ERROR");
                    }
                    let id = v["thread_id"].as_str().ok_or("PROTOCOL_ERROR")?;
                    if !id_check(id) {
                        return Err("PROTOCOL_ERROR");
                    }
                    self.session = id.into();
                    self.first = true;
                }
                "turn.started" => {
                    if self.started {
                        return Err("PROTOCOL_ERROR");
                    }
                    self.started = true;
                }
                "turn.completed" => {
                    if !self.started {
                        return Err("PROTOCOL_ERROR");
                    }
                    self.terminal = Some("SUCCESS".into());
                    self.usage = safe_usage(v.get("usage"), None)?;
                }
                "turn.failed" => self.terminal = Some("TURN_FAILED".into()),
                "error" => self.terminal = Some("RESULT_ERROR".into()),
                "item.started" | "item.updated" | "item.completed" => {
                    if !self.started {
                        return Err("PROTOCOL_ERROR");
                    }
                    match v["item"]["type"].as_str().ok_or("PROTOCOL_ERROR")? {
                        "command_execution" | "file_change" | "mcp_tool_call" | "web_search"
                        | "collab_tool_call" => return Err("TOOL_USE_OBSERVED"),
                        "reasoning" | "todo_list" => (),
                        "error" => self.unknowns.push("UPSTREAM_DIAGNOSTIC".into()),
                        "agent_message" => {
                            if typ == "item.completed" {
                                self.claim = Some(
                                    v["item"]["text"]
                                        .as_str()
                                        .ok_or("PROTOCOL_ERROR")?
                                        .as_bytes()
                                        .to_vec(),
                                );
                            }
                        }
                        _ => return Err("PROTOCOL_ERROR"),
                    }
                }
                _ => return Err("PROTOCOL_ERROR"),
            }
        } else {
            match typ {
                "system" => match v["subtype"].as_str().ok_or("PROTOCOL_ERROR")? {
                    "init" => {
                        if self.init {
                            return Err("PROTOCOL_ERROR");
                        }
                        if !empty_array(&v, "tools")
                            || !empty_array(&v, "mcp_servers")
                            || !empty_array(&v, "plugins")
                            || v["session_id"].as_str() != Some(self.session.as_str())
                        {
                            return Err("TOOL_USE_OBSERVED");
                        }
                        self.init = true;
                    }
                    "api_retry" => {
                        self.unknowns.push("UPSTREAM_RETRY".into());
                        self.retry_count = self.retry_count.checked_add(1).ok_or("BOUND")?;
                        let raw = v["error"]["type"].as_str().unwrap_or("");
                        let class = if [
                            "authentication_failed",
                            "oauth_org_not_allowed",
                            "account_on_hold",
                            "billing_error",
                        ]
                        .contains(&raw)
                        {
                            raw
                        } else {
                            "OTHER"
                        };
                        let n = self.retry_classes.entry(class.into()).or_default();
                        *n = n.checked_add(1).ok_or("BOUND")?;
                        if [
                            "authentication_failed",
                            "oauth_org_not_allowed",
                            "account_on_hold",
                            "billing_error",
                        ]
                        .contains(&v["error"]["type"].as_str().unwrap_or(""))
                        {
                            self.auth_failed = true;
                        }
                    }
                    _ => (),
                },
                "assistant" | "user" => {
                    if !self.init {
                        return Err("PROTOCOL_ERROR");
                    }
                    if !v["parent_tool_use_id"].is_null() {
                        return Err("TOOL_USE_OBSERVED");
                    }
                    if let Some(a) = v["message"]["content"].as_array() {
                        for item in a {
                            if item["type"] == "tool_use" {
                                return Err("TOOL_USE_OBSERVED");
                            }
                        }
                    } else {
                        return Err("PROTOCOL_ERROR");
                    }
                }
                "stream_event" | "prompt_suggestion" => (),
                "result" => {
                    if !self.init || v["session_id"].as_str() != Some(self.session.as_str()) {
                        return Err("BINDING_MISMATCH");
                    }
                    if v.get("permission_denials")
                        .is_some_and(|x| !x.as_array().is_some_and(|a| a.is_empty()))
                    {
                        return Err("TOOL_USE_OBSERVED");
                    }
                    let ok =
                        v["subtype"] == "success" && v["is_error"] == false && !self.auth_failed;
                    self.terminal = Some(if ok { "SUCCESS" } else { "RESULT_ERROR" }.into());
                    if ok {
                        let raw = raw_field(line, "structured_output")?.ok_or("SCHEMA_REJECTED")?;
                        self.claim = Some(raw);
                    }
                    self.usage = safe_usage(v.get("usage"), v.get("total_cost_usd"))?;
                }
                x if x.starts_with("hook_") => return Err("TOOL_USE_OBSERVED"),
                _ => return Err("PROTOCOL_ERROR"),
            }
        }
        if self.unknowns.len() > 16 {
            return Err("BOUND");
        }
        Ok(())
    }
    pub fn finish(mut self, exit: Option<i64>) -> Execution {
        let mut e = Execution::failed("TERMINAL_MISSING");
        e.created = true;
        e.natural = true;
        e.exit_code = exit;
        e.session_id = Some(self.session);
        e.unknowns = self.unknowns;
        e.usage = self.usage;
        e.retry_count = Some(self.retry_count);
        e.retry_classes = self.retry_classes;
        e.terminal = if !self.pending.is_empty() {
            "PROTOCOL_ERROR".into()
        } else if exit != Some(0) {
            "RESULT_ERROR".into()
        } else {
            self.terminal.take().unwrap_or("TERMINAL_MISSING".into())
        };
        if e.terminal == "SUCCESS" {
            e.claim = self.claim;
            if e.claim.is_none() {
                e.terminal = "SCHEMA_REJECTED".into()
            }
        }
        e
    }
}
// Preserve the untrusted reported usage/cost bytes semantically, never infer actual billing.
fn safe_usage(usage: Option<&Value>, cost: Option<&Value>) -> Result<Option<Value>> {
    if usage.is_none() && cost.is_none() {
        return Ok(None);
    }
    let v = json!({"usage":usage,"total_cost_usd":cost});
    if serde_json::to_vec(&v).map_err(|_| "PROTOCOL_ERROR")?.len() > 32768 {
        return Err("BOUND");
    }
    Ok(Some(v))
}
fn id_check(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
}
fn empty_array(v: &Value, key: &str) -> bool {
    v.get(key)
        .and_then(Value::as_array)
        .is_some_and(|a| a.is_empty())
}
fn raw_field(line: &[u8], key: &str) -> Result<Option<Vec<u8>>> {
    let mut at = 1;
    while at < line.len() {
        while at < line.len() && (line[at].is_ascii_whitespace() || line[at] == b',') {
            at += 1
        }
        if at == line.len() || line[at] == b'}' {
            break;
        }
        let mut keys = serde_json::Deserializer::from_slice(&line[at..]).into_iter::<String>();
        let k = keys
            .next()
            .ok_or("PROTOCOL_ERROR")?
            .map_err(|_| "PROTOCOL_ERROR")?;
        at += keys.byte_offset();
        while at < line.len() && line[at].is_ascii_whitespace() {
            at += 1
        }
        if line.get(at) != Some(&b':') {
            return Err("PROTOCOL_ERROR");
        }
        at += 1;
        while at < line.len() && line[at].is_ascii_whitespace() {
            at += 1
        }
        let mut val = serde_json::Deserializer::from_slice(&line[at..]).into_iter::<Value>();
        val.next()
            .ok_or("PROTOCOL_ERROR")?
            .map_err(|_| "PROTOCOL_ERROR")?;
        let len = val.byte_offset();
        if k == key {
            return Ok(Some(line[at..at + len].to_vec()));
        }
        at += len;
    }
    Ok(None)
}
pub struct NativeDriver {
    fake: PathBuf,
    fake_hash: String,
    _guard: Guard,
    pub cancel: Arc<AtomicBool>,
    #[cfg(test)]
    pub fault: Option<&'static str>,
    #[cfg(windows)]
    residuals: Vec<native::Session>,
}
impl NativeDriver {
    pub fn new() -> Result<Self> {
        let exe = std::env::current_exe().map_err(|_| "SELF_PATH")?;
        Self::from_directory(exe.parent().ok_or("SELF_PATH")?)
    }
    fn from_directory(dir: &Path) -> Result<Self> {
        let fake = dir.join("nova_scheduler_relay_fake_cli.exe");
        let mut g = Guard::tool(&fake)?;
        let h = hash(&g.bytes(16777216)?);
        Ok(Self {
            fake: g.path.clone(),
            fake_hash: h,
            _guard: g,
            cancel: Arc::new(AtomicBool::new(false)),
            #[cfg(test)]
            fault: None,
            #[cfg(windows)]
            residuals: vec![],
        })
    }
    #[cfg(all(test, windows))]
    pub fn test_spawn_contention(&mut self, spec: &Spec, input: &[u8], end: Instant) -> Execution {
        native::test_spawn_contention(self, spec, input, end)
    }
    #[cfg(all(test, windows))]
    pub fn test_execute_io(&mut self, spec: &Spec, input: &[u8], end: Instant) -> Execution {
        native::execute(self, spec, input, end, &mut |_, _| Ok(()))
    }
    #[cfg(test)]
    pub fn for_test() -> Result<Self> {
        let exe = std::env::current_exe().map_err(|_| "SELF_PATH")?;
        let dir = exe.parent().ok_or("SELF_PATH")?;
        Self::from_directory(if dir.file_name().is_some_and(|n| n == "deps") {
            dir.parent().ok_or("SELF_PATH")?
        } else {
            dir
        })
    }
}
impl Driver for NativeDriver {
    fn prepare(
        &mut self,
        p: &RelayPlan,
        role: RelayRole,
        root: &Path,
        _input: &[u8],
    ) -> Result<Spec> {
        if !p.mock {
            return Err("REAL_PILOT_NOT_AUTHORIZED");
        }
        p.validate()?;
        self._guard.verify()?;
        let cwd = root.join(if role == RelayRole::Develop {
            "develop-cwd"
        } else {
            "review-cwd"
        });
        fs::create_dir(&cwd).map_err(|_| "CREATE_NEW_REJECTED")?;
        if role == RelayRole::Develop {
            fs::create_dir(root.join("tmp")).map_err(|_| "CREATE_NEW_REJECTED")?;
            let mut f = fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(root.join("proposal.schema.json"))
                .map_err(|_| "CREATE_NEW_REJECTED")?;
            f.write_all(proposal_schema().as_bytes())
                .map_err(|_| "EVIDENCE_WRITE")?;
            f.sync_all().map_err(|_| "EVIDENCE_FLUSH")?;
        }
        let session = uuid::Uuid::new_v4().to_string();
        let model = if role == RelayRole::Develop {
            &p.model_develop
        } else {
            &p.model_review
        };
        if !mock_scenario(model) {
            return Err("MOCK_SCENARIO_REJECTED");
        }
        let parent: BTreeMap<_, _> = ENV_ALLOW
            .iter()
            .filter_map(|k| {
                std::env::var_os(k).map(|v| ((*k).to_string(), v.to_string_lossy().into_owned()))
            })
            .collect();
        Ok(Spec {
            role,
            profile: if role == RelayRole::Develop {
                p.develop_profile.clone()
            } else {
                p.review_profile.clone()
            },
            exe: self.fake.clone(),
            exe_sha256: self.fake_hash.clone(),
            argv: argv(role, root, model, &session),
            env: environment(&parent, root)?,
            cwd,
            session,
        })
    }
    fn execute(
        &mut self,
        spec: &Spec,
        input: &[u8],
        permit: &LaunchPermit,
        end: Instant,
        running: &mut dyn FnMut(u32, String) -> Result<()>,
    ) -> Execution {
        if !permit.claim_once() {
            let mut e = Execution::failed("BINDING_MISMATCH");
            e.unknowns.push("LAUNCH_PERMIT_USED".into());
            return e;
        }
        if self._guard.verify().is_err()
            || spec.exe != self.fake
            || spec.exe_sha256 != self.fake_hash
            || permit.attempt().role != spec.role
            || permit.attempt().state != "LAUNCHING"
            || permit.attempt().profile_id != spec.profile
            || permit.attempt().input_identity != hash(input)
        {
            return Execution::failed("BINDING_MISMATCH");
        }
        #[cfg(windows)]
        {
            native::execute(self, spec, input, end, running)
        }
        #[cfg(not(windows))]
        {
            let _ = (spec, input, end, running);
            Execution::failed("LAUNCH_FAILED")
        }
    }
}
pub fn mock_scenario(s: &str) -> bool {
    [
        "ok",
        "blocked",
        "changes",
        "conflict",
        "unknowns",
        "turn-failed",
        "error",
        "exit",
        "missing",
        "duplicate",
        "after",
        "half",
        "non-json",
        "unknown",
        "tool",
        "tools",
        "mcp",
        "hook",
        "tool-use",
        "denial",
        "result-error",
        "no-structured",
        "session",
        "hash",
        "auth-retry",
        "stdout-bound",
        "stderr-bound",
        "hang",
        "descendant",
        "duplex",
        "no-read",
        "io",
        "no-stdin-exit",
    ]
    .iter()
    .any(|x| s == format!("mock:{x}"))
}

// Callback performs no allocation, I/O, waiting or database work.
#[cfg(windows)]
static CONSOLE_CANCEL: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
static HANDLER_OWNED: AtomicBool = AtomicBool::new(false);
#[cfg(windows)]
unsafe extern "system" fn console_handler(code: u32) -> i32 {
    if code == 0 || code == 1 {
        CONSOLE_CANCEL.store(true, Ordering::Release);
        1
    } else {
        0
    }
}
#[cfg(windows)]
pub struct ConsoleHandler {
    owned: bool,
}
#[cfg(windows)]
impl ConsoleHandler {
    pub fn install() -> Result<Self> {
        use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
        if HANDLER_OWNED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("HANDLER_OWNED");
        }
        CONSOLE_CANCEL.store(false, Ordering::Release);
        if unsafe { SetConsoleCtrlHandler(Some(console_handler), 1) } == 0 {
            HANDLER_OWNED.store(false, Ordering::Release);
            return Err("HANDLER_INSTALL");
        }
        Ok(Self { owned: true })
    }
    pub fn close(&mut self) -> Result<()> {
        if self.owned {
            if unsafe {
                windows_sys::Win32::System::Console::SetConsoleCtrlHandler(Some(console_handler), 0)
            } == 0
            {
                return Err("HANDLER_UNREGISTER");
            }
            self.owned = false;
            HANDLER_OWNED.store(false, Ordering::Release);
        }
        Ok(())
    }
    pub fn cancelled(&self) -> bool {
        CONSOLE_CANCEL.load(Ordering::Acquire)
    }
}
#[cfg(windows)]
impl Drop for ConsoleHandler {
    fn drop(&mut self) {
        let _ = self.close();
    }
}
pub fn recover_identity(pid: Option<u32>, created: Option<&str>) -> String {
    #[cfg(windows)]
    {
        native::recover(pid, created)
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, created);
        "UNKNOWN".into()
    }
}
#[cfg(windows)]
mod native {
    use super::*;
    use std::{
        ffi::OsStr,
        fs::File,
        os::windows::{ffi::OsStrExt, io::FromRawHandle},
        ptr::{null, null_mut},
        sync::{mpsc, Mutex},
        thread::{self, JoinHandle},
    };
    use windows_sys::Win32::{
        Foundation::*,
        Security::SECURITY_ATTRIBUTES,
        System::{JobObjects::*, Pipes::CreatePipe, Threading::*},
    };
    static SPAWN: Mutex<()> = Mutex::new(());
    struct H(HANDLE);
    impl Drop for H {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
    fn wide(s: &OsStr) -> Vec<u16> {
        s.encode_wide().chain(Some(0)).collect()
    }
    pub(super) fn quote(s: &str) -> Vec<u16> {
        let mut v = vec![34];
        let mut n = 0;
        for c in s.encode_utf16() {
            if c == 92 {
                n += 1;
                continue;
            }
            if c == 34 {
                v.extend(std::iter::repeat_n(92, n * 2 + 1));
            } else {
                v.extend(std::iter::repeat_n(92, n));
            }
            v.push(c);
            n = 0;
        }
        v.extend(std::iter::repeat_n(92, n * 2));
        v.push(34);
        v
    }
    fn pipe() -> Result<(H, H)> {
        unsafe {
            let (mut a, mut b) = (null_mut(), null_mut());
            let sa = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: null_mut(),
                bInheritHandle: 1,
            };
            if CreatePipe(&mut a, &mut b, &sa, 0) == 0 {
                return Err("LAUNCH_FAILED");
            }
            Ok((H(a), H(b)))
        }
    }
    fn file(h: H) -> File {
        let f = unsafe { File::from_raw_handle(h.0 as _) };
        std::mem::forget(h);
        f
    }
    struct Tree {
        job: H,
        process: H,
        thread: H,
        pid: u32,
        created: String,
        assigned: bool,
        stdin: Option<File>,
        out: Option<File>,
        err: Option<File>,
    }
    impl Tree {
        fn create(s: &Spec, end: Instant) -> Result<Self> {
            let _serial = loop {
                match SPAWN.try_lock() {
                    Ok(g) => break g,
                    Err(std::sync::TryLockError::Poisoned(_)) => return Err("LAUNCH_FAILED"),
                    Err(std::sync::TryLockError::WouldBlock) => {
                        if Instant::now() >= end {
                            return Err("DEADLINE");
                        }
                        thread::sleep(Duration::from_millis(2));
                    }
                }
            };
            unsafe {
                let mut g = Guard::tool(&s.exe)?;
                if hash(&g.bytes(16777216)?) != s.exe_sha256 {
                    return Err("BINDING_MISMATCH");
                }
                let job = H(CreateJobObjectW(null(), null()));
                if job.0.is_null() {
                    return Err("LAUNCH_FAILED");
                }
                let mut l: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                l.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    job.0,
                    JobObjectExtendedLimitInformation,
                    &l as *const _ as _,
                    std::mem::size_of_val(&l) as u32,
                ) == 0
                {
                    return Err("LAUNCH_FAILED");
                }
                let (out, childout) = pipe()?;
                let (err, childerr) = pipe()?;
                let (childin, input) = pipe()?;
                for h in [&out, &err, &input] {
                    if SetHandleInformation(h.0, HANDLE_FLAG_INHERIT, 0) == 0 {
                        return Err("LAUNCH_FAILED");
                    }
                }
                let mut size = 0;
                InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
                let mut attr = vec![0usize; size.div_ceil(std::mem::size_of::<usize>())];
                let list = attr.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
                if InitializeProcThreadAttributeList(list, 1, 0, &mut size) == 0 {
                    return Err("LAUNCH_FAILED");
                }
                struct A(LPPROC_THREAD_ATTRIBUTE_LIST);
                impl Drop for A {
                    fn drop(&mut self) {
                        unsafe { DeleteProcThreadAttributeList(self.0) }
                    }
                }
                let _a = A(list);
                let handles = [childin.0, childout.0, childerr.0];
                if UpdateProcThreadAttribute(
                    list,
                    0,
                    PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                    handles.as_ptr() as _,
                    std::mem::size_of_val(&handles),
                    null_mut(),
                    null(),
                ) == 0
                {
                    return Err("LAUNCH_FAILED");
                }
                let mut si: STARTUPINFOEXW = std::mem::zeroed();
                si.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
                si.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
                si.StartupInfo.hStdInput = childin.0;
                si.StartupInfo.hStdOutput = childout.0;
                si.StartupInfo.hStdError = childerr.0;
                si.lpAttributeList = list;
                let app = wide(s.exe.as_os_str());
                let mut cmd = quote(s.exe.to_str().ok_or("PATH_REJECTED")?);
                for a in &s.argv {
                    if a.contains('\0') {
                        return Err("BINDING_MISMATCH");
                    }
                    cmd.push(32);
                    cmd.extend(quote(a))
                }
                cmd.push(0);
                let mut vars: Vec<_> = s.env.iter().collect();
                vars.sort_by_key(|(k, _)| k.to_ascii_uppercase());
                let mut env = vec![];
                let mut prior = String::new();
                for (k, v) in vars {
                    let upper = k.to_ascii_uppercase();
                    if !k.is_ascii()
                        || k.is_empty()
                        || k.contains(['=', '\0'])
                        || v.contains('\0')
                        || upper == prior
                        || forbidden_env(k)
                        || !(ENV_ALLOW.iter().any(|a| a.eq_ignore_ascii_case(k))
                            || ["PATH", "TEMP", "TMP"].contains(&upper.as_str()))
                    {
                        return Err("BINDING_MISMATCH");
                    }
                    prior = upper;
                    env.extend(k.encode_utf16());
                    env.push(61);
                    env.extend(v.encode_utf16());
                    env.push(0)
                }
                env.push(0);
                if env.len() > 32767 {
                    return Err("BOUND");
                }
                let cwd = wide(s.cwd.as_os_str());
                let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
                if CreateProcessW(
                    app.as_ptr(),
                    cmd.as_mut_ptr(),
                    null(),
                    null(),
                    1,
                    CREATE_SUSPENDED
                        | CREATE_NO_WINDOW
                        | EXTENDED_STARTUPINFO_PRESENT
                        | CREATE_UNICODE_ENVIRONMENT,
                    env.as_ptr() as _,
                    cwd.as_ptr(),
                    &si.StartupInfo,
                    &mut pi,
                ) == 0
                {
                    return Err("CREATE_PROCESS_FAILED");
                }
                let process = H(pi.hProcess);
                let created = creation(process.0).unwrap_or_default();
                Ok(Self {
                    job,
                    process,
                    thread: H(pi.hThread),
                    pid: pi.dwProcessId,
                    created,
                    assigned: false,
                    stdin: Some(file(input)),
                    out: Some(file(out)),
                    err: Some(file(err)),
                })
            }
        }
        fn query(&self) -> (Option<u32>, Option<u32>, bool) {
            unsafe {
                let mut l: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                let mut a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                let flags = (QueryInformationJobObject(
                    self.job.0,
                    JobObjectExtendedLimitInformation,
                    &mut l as *mut _ as _,
                    std::mem::size_of_val(&l) as u32,
                    null_mut(),
                ) != 0)
                    .then_some(l.BasicLimitInformation.LimitFlags);
                let active = (QueryInformationJobObject(
                    self.job.0,
                    JobObjectBasicAccountingInformation,
                    &mut a as *mut _ as _,
                    std::mem::size_of_val(&a) as u32,
                    null_mut(),
                ) != 0)
                    .then_some(a.ActiveProcesses);
                (
                    flags,
                    active,
                    WaitForSingleObject(self.process.0, 0) == WAIT_OBJECT_0,
                )
            }
        }
        fn confirmed(&self) -> bool {
            self.query() == (Some(0x2000), Some(0), true)
        }
        fn kill(&self) {
            unsafe {
                if self.assigned {
                    TerminateJobObject(self.job.0, 1);
                } else {
                    TerminateProcess(self.process.0, 1);
                }
            }
        }
        fn code(&self) -> Option<i64> {
            if !self.query().2 {
                return None;
            }
            let mut c = 0;
            (unsafe { GetExitCodeProcess(self.process.0, &mut c) } != 0).then_some(c as i64)
        }
        fn start(&mut self, fault: Option<&str>) -> Result<()> {
            unsafe {
                if AssignProcessToJobObject(self.job.0, self.process.0) == 0 {
                    return Err("LIFECYCLE_UNKNOWN");
                }
                self.assigned = true;
                if fault == Some("query") || fault == Some("resume") {
                    return Err("LIFECYCLE_UNKNOWN");
                }
                if self.created.is_empty()
                    || self.query().0 != Some(0x2000)
                    || self.query().1 != Some(1)
                {
                    return Err("LIFECYCLE_UNKNOWN");
                }
                if ResumeThread(self.thread.0) != 1 {
                    return Err("LIFECYCLE_UNKNOWN");
                }
                Ok(())
            }
        }
    }
    fn creation(h: HANDLE) -> Option<String> {
        unsafe {
            let (mut a, mut b, mut c, mut d) = (
                std::mem::zeroed(),
                std::mem::zeroed(),
                std::mem::zeroed(),
                std::mem::zeroed(),
            );
            (GetProcessTimes(h, &mut a, &mut b, &mut c, &mut d) != 0)
                .then(|| format!("{:08x}{:08x}", a.dwHighDateTime, a.dwLowDateTime))
        }
    }
    pub(super) fn recover(pid: Option<u32>, created: Option<&str>) -> String {
        let (Some(pid), Some(created)) = (pid, created) else {
            return "UNKNOWN".into();
        };
        unsafe {
            let p = H(OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            ));
            if p.0.is_null() {
                return if GetLastError() == 87 {
                    "CONFIRMED"
                } else {
                    "UNKNOWN"
                }
                .into();
            }
            if creation(p.0).as_deref() != Some(created) {
                return "UNKNOWN".into();
            }
            if WaitForSingleObject(p.0, 0) == WAIT_OBJECT_0 {
                "CONFIRMED"
            } else {
                "UNKNOWN"
            }
            .into()
        }
    }
    pub(super) struct Session {
        tree: Tree,
        out: Option<JoinHandle<(Vec<u8>, Result<Parser>)>>,
        err: Option<JoinHandle<(Vec<u8>, bool)>>,
        writer: Option<JoinHandle<bool>>,
    }
    impl Session {
        fn cleanup(&mut self, hard: Instant) -> bool {
            if !self.tree.confirmed() {
                self.tree.kill()
            }
            let job_end = (Instant::now() + Duration::from_secs(5)).min(hard);
            while !self.tree.confirmed() && Instant::now() < job_end {
                thread::sleep(Duration::from_millis(2))
            }
            let job = self.tree.confirmed();
            let mut readers = true;
            for which in 0..3 {
                let end = (Instant::now() + Duration::from_secs(2)).min(hard);
                while match which {
                    0 => self.out.as_ref().is_some_and(|h| !h.is_finished()),
                    1 => self.err.as_ref().is_some_and(|h| !h.is_finished()),
                    _ => self.writer.as_ref().is_some_and(|h| !h.is_finished()),
                } && Instant::now() < end
                {
                    thread::sleep(Duration::from_millis(2))
                }
                readers &= match which {
                    0 => self.out.as_ref().is_none_or(|h| h.is_finished()),
                    1 => self.err.as_ref().is_none_or(|h| h.is_finished()),
                    _ => self.writer.as_ref().is_none_or(|h| h.is_finished()),
                };
            }
            job && readers
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            if !self.tree.confirmed() {
                self.tree.kill()
            }
        }
    }
    #[cfg(test)]
    pub(super) fn test_spawn_contention(
        d: &mut NativeDriver,
        s: &Spec,
        input: &[u8],
        end: Instant,
    ) -> Execution {
        let _held = SPAWN.lock().unwrap();
        execute(d, s, input, end, &mut |_, _| {
            panic!("lock deadline must not create")
        })
    }
    pub(super) fn execute(
        driver: &mut NativeDriver,
        s: &Spec,
        input: &[u8],
        end: Instant,
        running: &mut dyn FnMut(u32, String) -> Result<()>,
    ) -> Execution {
        let hard = end + Duration::from_secs(15);
        if driver.cancel.load(Ordering::Acquire) || CONSOLE_CANCEL.load(Ordering::Acquire) {
            return Execution::failed("CANCELLED");
        }
        if Instant::now() >= end {
            return Execution::failed("DEADLINE");
        }
        let tree = match Tree::create(s, end) {
            Ok(t) => t,
            Err(c) => {
                let mut result = Execution::failed(match c {
                    "BINDING_MISMATCH" => "BINDING_MISMATCH",
                    "BOUND" => "BOUND",
                    "DEADLINE" => "DEADLINE",
                    _ => "LAUNCH_FAILED",
                });
                result.create_process_failed = c == "CREATE_PROCESS_FAILED";
                return result;
            }
        };
        let mut session = Session {
            tree,
            out: None,
            err: None,
            writer: None,
        };
        let mut error = None;
        #[cfg(not(test))]
        let fault = None::<&str>;
        #[cfg(test)]
        let fault = driver.fault;
        if driver.cancel.load(Ordering::Acquire) || CONSOLE_CANCEL.load(Ordering::Acquire) {
            error = Some("CANCELLED")
        } else if Instant::now() >= end {
            error = Some("DEADLINE")
        } else if fault == Some("created") || fault == Some("assign") {
            error = Some("LIFECYCLE_UNKNOWN")
        } else if let Err(e) = session.tree.start(fault) {
            error = Some(e)
        } else if fault == Some("persist") {
            error = Some("LIFECYCLE_UNKNOWN")
        } else if let Err(_) = running(session.tree.pid, session.tree.created.clone()) {
            error = Some("LIFECYCLE_UNKNOWN")
        }
        let mut result = Execution::failed(error.unwrap_or("TERMINAL_MISSING"));
        result.created = true;
        result.pid = Some(session.tree.pid);
        result.process_creation_utc = Some(session.tree.created.clone());
        let (tx, rx) = mpsc::sync_channel::<&'static str>(4);
        if error.is_none() {
            let mut out = session.tree.out.take().unwrap();
            let role = s.role;
            let id = s.session.clone();
            let t = tx.clone();
            session.out = Some(thread::spawn(move || {
                let mut p = Parser::new(role, &id);
                let mut data = vec![];
                let mut buf = [0u8; 8192];
                let mut fail = None;
                loop {
                    match out.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let keep = n.min(8388608usize.saturating_sub(data.len()));
                            data.extend_from_slice(&buf[..keep]);
                            if let Err(c) = p.feed(&buf[..n]) {
                                let _ = t.try_send(c);
                                fail = Some(c);
                                break;
                            }
                        }
                        Err(_) => {
                            fail = Some("PROTOCOL_ERROR");
                            let _ = t.try_send("PROTOCOL_ERROR");
                            break;
                        }
                    }
                }
                (data, fail.map_or(Ok(p), Err))
            }));
            let mut err = session.tree.err.take().unwrap();
            let t = tx.clone();
            session.err = Some(thread::spawn(move || {
                let mut data = vec![];
                let mut buf = [0u8; 8192];
                let mut ok = true;
                loop {
                    match err.read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            let keep = n.min(1048576usize.saturating_sub(data.len()));
                            data.extend_from_slice(&buf[..keep]);
                            if keep < n {
                                ok = false;
                                let _ = t.try_send("BOUND");
                                break;
                            }
                        }
                        Err(_) => {
                            ok = false;
                            let _ = t.try_send("LIFECYCLE_UNKNOWN");
                            break;
                        }
                    }
                }
                (data, ok)
            }));
            let mut stdin = session.tree.stdin.take().unwrap();
            let bytes = input.to_vec();
            let t = tx.clone();
            session.writer = Some(thread::spawn(move || {
                let ok = stdin.write_all(&bytes).and_then(|_| stdin.flush()).is_ok();
                if !ok {
                    let _ = t.try_send("BOUND");
                }
                ok
            }));
            loop {
                if driver.cancel.load(Ordering::Acquire) || CONSOLE_CANCEL.load(Ordering::Acquire) {
                    error = Some("CANCELLED");
                    break;
                }
                if let Ok(c) = rx.try_recv() {
                    error = Some(c);
                    break;
                }
                let (flags, active, exited) = session.tree.query();
                if flags != Some(0x2000) || active.is_none() {
                    error = Some("LIFECYCLE_UNKNOWN");
                    break;
                }
                if exited {
                    result.natural = true;
                    break;
                }
                if Instant::now() >= end {
                    error = Some("DEADLINE");
                    break;
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
        // No worker starts on launch-chain failure; close own endpoints before cleanup.
        if error.is_some() && session.out.is_none() {
            session.tree.stdin.take();
            session.tree.out.take();
            session.tree.err.take();
        }
        let observed = session.cleanup(hard);
        let reclaimed = observed && fault != Some("reclaim");
        result.exit_code = session.tree.code();
        result.reclaim = if reclaimed { "CONFIRMED" } else { "UNKNOWN" }.into();
        if reclaimed {
            if let Some(h) = session.out.take() {
                match h.join() {
                    Ok((bytes, p)) => {
                        result.stdout = bytes;
                        match p {
                            Ok(p) => {
                                let mut parsed = p.finish(result.exit_code);
                                parsed.pid = result.pid;
                                parsed.process_creation_utc = result.process_creation_utc;
                                parsed.stdout = result.stdout;
                                parsed.natural = result.natural;
                                result = parsed
                            }
                            Err(c) => error = Some(c),
                        }
                    }
                    Err(_) => error = Some("LIFECYCLE_UNKNOWN"),
                }
            }
            if let Some(h) = session.err.take() {
                match h.join() {
                    Ok((bytes, ok)) => {
                        result.stderr = bytes;
                        if !ok && error.is_none() {
                            error = Some("BOUND")
                        }
                    }
                    Err(_) => error = Some("LIFECYCLE_UNKNOWN"),
                }
            }
            if let Some(h) = session.writer.take() {
                if !h.join().unwrap_or(false) && error.is_none() {
                    error = Some("BOUND")
                }
            }
        } else {
            result.unknowns.push("RESOURCES_NOT_RECLAIMED".into());
            driver.residuals.push(session);
        }
        if let Some(c) = error {
            result.terminal = c.into();
            result.claim = None;
        }
        result
    }
}

/// Only the fixed fake fixture invokes this check, in its own newly allocated console.
#[cfg(windows)]
pub fn isolated_console_check(is_break: bool) -> Result<()> {
    use windows_sys::Win32::System::Console::*;
    let saved_out = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let saved_err = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    unsafe {
        FreeConsole();
        if AllocConsole() == 0 {
            return Err("TEST_CONSOLE_CREATE");
        }
        SetStdHandle(STD_OUTPUT_HANDLE, saved_out);
        SetStdHandle(STD_ERROR_HANDLE, saved_err);
        if SetConsoleCtrlHandler(None, 0) == 0 {
            return Err("TEST_CONSOLE_ENABLE");
        }
    }
    let mut handler = ConsoleHandler::install()?;
    if ConsoleHandler::install().is_ok() {
        return Err("TEST_HANDLER_OWNERSHIP");
    }
    let root = std::env::temp_dir().join(format!("ar0-console-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).map_err(|_| "TEST_ROOT")?;
    for sub in ["develop-cwd", "tmp"] {
        fs::create_dir(root.join(sub)).map_err(|_| "TEST_ROOT")?;
    }
    fs::write(root.join("proposal.schema.json"), proposal_schema()).map_err(|_| "TEST_ROOT")?;
    let mut d = NativeDriver::new()?;
    let env = environment(
        &ENV_ALLOW
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).into(), v)))
            .collect(),
        &root,
    )?;
    let spec = Spec {
        role: RelayRole::Develop,
        profile: DEVELOP_PROFILE.into(),
        exe: d.fake.clone(),
        exe_sha256: d.fake_hash.clone(),
        argv: argv(RelayRole::Develop, &root, "mock:hang", "console"),
        env,
        cwd: root.join("develop-cwd"),
        session: "console".into(),
    };
    let result = native::execute(
        &mut d,
        &spec,
        b"fixed",
        Instant::now() + Duration::from_secs(3),
        &mut |_, _| {
            if unsafe {
                GenerateConsoleCtrlEvent(
                    if is_break {
                        CTRL_BREAK_EVENT
                    } else {
                        CTRL_C_EVENT
                    },
                    0,
                )
            } == 0
            {
                return Err("TEST_CONSOLE_SEND");
            }
            Ok(())
        },
    );
    if !handler.cancelled() || result.terminal != "CANCELLED" || result.reclaim != "CONFIRMED" {
        eprintln!(
            "{}",
            json!({"cancelled":handler.cancelled(),"terminal":result.terminal,"reclaim":result.reclaim,"created":result.created})
        );
        return Err("TEST_CONSOLE_RECLAIM");
    }
    handler.close()?;
    drop(handler);
    let mut second = ConsoleHandler::install()?;
    if second.cancelled() {
        return Err("TEST_HANDLER_RESET");
    }
    second.close()?;
    drop(second);
    if HANDLER_OWNED.load(Ordering::Acquire) {
        return Err("TEST_HANDLER_UNREGISTER");
    }
    println!(
        "{}",
        json!({"schema":"nova.relay.console-test/1","isolated_console":true,"event":if is_break{"CTRL_BREAK"}else{"CTRL_C"},"cancelled":true,"reclaim":"CONFIRMED","handler_unregistered":true})
    );
    Ok(())
}
#[cfg(not(windows))]
pub fn isolated_console_check(_: bool) -> Result<()> {
    Err("WINDOWS_REQUIRED")
}

/// Fixed fake-controller fixture: credentials exist only in this test parent, never in its child.
#[cfg(windows)]
pub fn isolated_environment_check() -> Result<()> {
    let root = std::env::temp_dir().join(format!("ar0-env-{}", uuid::Uuid::new_v4()));
    fs::create_dir(&root).map_err(|_| "TEST_ROOT")?;
    for sub in ["develop-cwd", "tmp"] {
        fs::create_dir(root.join(sub)).map_err(|_| "TEST_ROOT")?;
    }
    fs::write(root.join("proposal.schema.json"), proposal_schema()).map_err(|_| "TEST_ROOT")?;
    let mut d = NativeDriver::new()?;
    let env = environment(
        &ENV_ALLOW
            .iter()
            .filter_map(|k| std::env::var(k).ok().map(|v| ((*k).into(), v)))
            .collect(),
        &root,
    )?;
    let spec = Spec {
        role: RelayRole::Develop,
        profile: DEVELOP_PROFILE.into(),
        exe: d.fake.clone(),
        exe_sha256: d.fake_hash.clone(),
        argv: argv(RelayRole::Develop, &root, "mock:ok", "fixed"),
        env,
        cwd: root.join("develop-cwd"),
        session: "fixed".into(),
    };
    let input =
        serde_json::to_vec(&json!({"base":format!("{}\n s.to_string()\n}}\n",RELAY_SIGNATURE)}))
            .map_err(|_| "ENCODING")?;
    let e = native::execute(
        &mut d,
        &spec,
        &input,
        Instant::now() + Duration::from_secs(3),
        &mut |_, _| Ok(()),
    );
    if e.terminal != "SUCCESS" || e.reclaim != "CONFIRMED" {
        return Err("TEST_ENVIRONMENT");
    }
    println!(
        "{}",
        json!({"schema":"nova.relay.environment-test/1","credential_names_absent":true,"reclaim":"CONFIRMED","source":"FIXED_FAKE_CHILD_VALIDATION"})
    );
    Ok(())
}
#[cfg(not(windows))]
pub fn isolated_environment_check() -> Result<()> {
    Err("WINDOWS_REQUIRED")
}
