//! Fixed zero-turn P probe. No model authorization and no generic program/path entry.
use super::*;
use std::process::{Command, Stdio};
pub const ROOT: &str = r"D:\nova-self2-app-server-validation-1122a";
pub const CONTROLLER: &str =
    r"D:\nova\audit\app-server-native-ready-1122a\controller\nova_scheduler_self_codex.exe";
const REMOTE: &str = "CODEX_INTERNAL_APP_SERVER_REMOTE_CONTROL_DISABLED";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conditions {
    pub model: String,
    pub expires_utc: i64,
    pub initialization_terms: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Request {
    schema: String,
    mode: String,
    root: PathBuf,
    root_identity: String,
    material: String,
    consumer: PathBuf,
    consumer_hash: String,
    cli: PathBuf,
    cli_hash: String,
    argv: Vec<String>,
    env: BTreeMap<String, String>,
    model: String,
    terms: String,
    expires: i64,
    plan_hash: String,
}
#[derive(Clone)]
struct Context {
    root: PathBuf,
    cli: PathBuf,
    consumer: PathBuf,
    #[cfg(test)]
    case: String,
    #[cfg(test)]
    starts: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}
#[cfg(test)]
thread_local! {static SOURCE:std::cell::RefCell<Option<Context>>=const {std::cell::RefCell::new(None)};}
fn context() -> Context {
    #[cfg(test)]
    if let Some(v) = SOURCE.with(|s| s.borrow().clone()) {
        return v;
    }
    Context {
        root: ROOT.into(),
        cli: super::super::super::admission_probe::CLI.into(),
        consumer: CONTROLLER.into(),
        #[cfg(test)]
        case: "ok".into(),
        #[cfg(test)]
        starts: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }
}
// Verified frozen lib.rs defaults PluginStartupTasks::Start; MessageProcessor::new calls
// maybe_start_plugin_startup_tasks_for_config before the first client RPC. We do not
// manufacture a disabled capability from config/read or the removed remote_control feature.
struct StartupFacts {
    remote_marker: &'static str,
    host_initialization_closed: bool,
}
fn startup_facts() -> StartupFacts {
    #[cfg(test)]
    if SOURCE.with(|s| s.borrow().is_some()) {
        // Only the pinned current test executable, never a production CLI identity.
        return StartupFacts {
            remote_marker: REMOTE,
            host_initialization_closed: true,
        };
    }
    StartupFacts {
        remote_marker: REMOTE,
        host_initialization_closed: false,
    }
}
pub(crate) fn startup_admission() -> Result<()> {
    let facts = startup_facts();
    if facts.remote_marker != REMOTE || !facts.host_initialization_closed {
        return Err("P_HOST_TOOL_INITIALIZATION_UNVERIFIED");
    }
    Ok(())
}
#[cfg(test)]
pub(crate) fn test_cli_binding(p: &Plan) -> bool {
    SOURCE.with(|s| {
        s.borrow()
            .as_ref()
            .is_some_and(|c| p.project_root == c.root.join("project") && p.cli == c.cli)
    }) && matches!(p.mode, Mode::Synthetic)
        && std::env::current_exe().is_ok_and(|exe| {
            exe == p.cli && tool_digest(&exe).ok().as_deref() == Some(&p.cli_sha256)
        })
}
fn valid_time(expires: i64) -> Result<()> {
    let now = chrono::Utc::now().timestamp();
    if expires <= now || expires > now + 86400 {
        Err("P_EXPIRED")
    } else {
        Ok(())
    }
}
fn valid_conditions(c: &Conditions) -> Result<()> {
    if c.model.is_empty()
        || c.model.len() > 128
        || c.model.chars().any(char::is_control)
        || c.initialization_terms.trim().is_empty()
        || c.initialization_terms.len() > 1024
        || c.expires_utc <= 0
    {
        return Err("P_CONDITIONS_REJECTED");
    }
    Ok(())
}
fn material(root: &Path) -> Result<String> {
    let mut rows = BTreeMap::new();
    for name in ["project", "control", "protected"] {
        let dir = root.join(name);
        let g = Guard::open(&dir, true)?;
        rows.insert(
            name,
            json!({"identity":g.identity,"tree":canary_tree(&dir)?}),
        );
    }
    let (a, id) = super::super::super::store::read_file(&root.join("project/a.txt"), 131072)?;
    rows.insert("a", json!({"id":id,"sha256":hash(&a)}));
    let runtime = Guard::open(&root.join("runtime"), true)?;
    let tmp = Guard::open(&root.join("runtime/tmp"), true)?;
    rows.insert("runtime", json!({"id":runtime.identity,"tmp":tmp.identity}));
    Ok(hash(&bytes(&rows)?))
}
fn environment(root: &Path) -> Result<BTreeMap<String, String>> {
    let parent = cli_adapter::ENV_ALLOW
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    let mut env = cli_adapter::environment(&parent, &root.join("runtime"))?;
    env.insert(REMOTE.into(), "1".into());
    #[cfg(test)]
    if SOURCE.with(|s| s.borrow().is_some()) {
        let c = SOURCE.with(|s| s.borrow().as_ref().unwrap().case.clone());
        env.insert(
            "NOVA_PERMISSION_FAKE_CASE".into(),
            format!("app-server:p-{c}"),
        );
    }
    Ok(env)
}
fn argv() -> Vec<String> {
    #[cfg(test)]
    if SOURCE.with(|s| s.borrow().is_some()) {
        return vec![
            "--exact".into(),
            "scheduler::admission_probe::tests::probe_fake_child".into(),
            "--quiet".into(),
            "--nocapture".into(),
        ];
    }
    ["app-server", "--stdio", "--strict-config"]
        .map(str::to_string)
        .to_vec()
}
fn request(s: &Store, c: &Conditions) -> Result<Request> {
    valid_conditions(c)?;
    let ctx = context();
    let p = journal(s)?;
    if p.plan.project_root != ctx.root.join("project") || !p.plan.commands.is_empty() {
        return Err("P_FIXED_SCOPE_REQUIRED");
    }
    let me = std::env::current_exe().map_err(|_| "P_CONSUMER_REJECTED")?;
    if me != ctx.consumer || p.plan.cli != ctx.cli || p.plan.model != c.model {
        return Err("P_PROGRAM_REJECTED");
    }
    let cli_hash = tool_digest(&ctx.cli)?;
    #[cfg(not(test))]
    if cli_hash != CLI_SHA {
        return Err("P_PROGRAM_REJECTED");
    }
    #[cfg(test)]
    if !test_cli_binding(&p.plan) {
        return Err("P_PROGRAM_REJECTED");
    }
    Ok(Request {
        schema: "nova.app-server.p/1".into(),
        mode: "P_ZERO_TURN".into(),
        root: ctx.root.clone(),
        root_identity: Guard::open(&ctx.root, true)?.identity,
        material: material(&ctx.root)?,
        consumer: me.clone(),
        consumer_hash: tool_digest(&me)?,
        cli: ctx.cli,
        cli_hash,
        argv: argv(),
        env: environment(&ctx.root)?,
        model: c.model.clone(),
        terms: c.initialization_terms.clone(),
        expires: c.expires_utc,
        plan_hash: p.plan_sha256,
    })
}
fn load_request(s: &Store) -> Result<Request> {
    let a = app_state(s)?;
    let r: Request =
        serde_json::from_value(a["probe_request"].clone()).map_err(|_| "P_REQUEST_REJECTED")?;
    let c = Conditions {
        model: r.model.clone(),
        expires_utc: r.expires,
        initialization_terms: r.terms.clone(),
    };
    if request(s, &c)? != r || a["digest"] != hash(&bytes(&r)?) {
        return Err("P_BINDING_REJECTED");
    }
    Ok(r)
}
fn open() -> Result<Store> {
    Store::self_open(&h::store_path(&context().root.join("project"))?)
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    let mut c = Command::new("C:/Program Files/Git/cmd/git.exe");
    c.env_clear()
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "NUL")
        .env("GIT_TERMINAL_PROMPT", "0");
    if let Some(v) = std::env::var_os("SystemRoot") {
        c.env("SystemRoot", v);
    }
    c.args([
        "-c",
        "core.hooksPath=NUL",
        "-c",
        "core.autocrlf=false",
        "-C",
    ])
    .arg(root)
    .args(args)
    .stdin(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x08000000);
    }
    let o = c.output().map_err(|_| "P_PREPARE_GIT")?;
    if !o.status.success() || o.stdout.len() > 16384 || o.stderr.len() > 16384 {
        return Err("P_PREPARE_GIT");
    }
    String::from_utf8(o.stdout)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "P_PREPARE_GIT")
}
fn create_file(path: &Path, raw: &[u8]) -> Result<fs::File> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|_| "P_PREPARE_FAILED")?;
    f.write_all(raw)
        .and_then(|_| f.sync_all())
        .map_err(|_| "P_PREPARE_FAILED")?;
    Ok(f)
}
pub fn prepare(c: Conditions) -> Result<Value> {
    valid_conditions(&c)?;
    valid_time(c.expires_utc)?;
    let ctx = context();
    if ctx.root.exists() {
        return Err("P_ROOT_EXISTS");
    }
    if std::env::current_exe().map_err(|_| "P_CONSUMER_REJECTED")? != ctx.consumer {
        return Err("P_CONSUMER_REJECTED");
    }
    let cli_hash = tool_digest(&ctx.cli)?;
    #[cfg(not(test))]
    if cli_hash != CLI_SHA {
        return Err("P_PROGRAM_REJECTED");
    }
    // Create-new only. Never called by this offline implementation against the real fixed root.
    let _parent = Guard::open(ctx.root.parent().ok_or("PATH_REJECTED")?, true)?;
    fs::create_dir(&ctx.root).map_err(|_| "P_ROOT_EXISTS")?;
    for name in [
        "project",
        "control",
        "protected",
        "evidence",
        "runtime",
        "runtime/tmp",
    ] {
        fs::create_dir(ctx.root.join(name)).map_err(|_| "P_PREPARE_FAILED")?;
    }
    let repo = ctx.root.join("project");
    git(&repo, &["init", "-q"])?;
    let mut owned_a = create_file(&repo.join("a.txt"), b"BASE\r\n")?;
    create_file(&repo.join("readonly.txt"), b"READONLY_INITIAL\r\n")?;
    create_file(
        &ctx.root.join("protected/outside.txt"),
        b"OUTSIDE_INITIAL\r\n",
    )?;
    create_file(&ctx.root.join("control/mode.txt"), b"P_ZERO_TURN\n")?;
    git(&repo, &["add", "a.txt", "readonly.txt"])?;
    let commit = [
        "-c",
        "user.name=Nova P fixture",
        "-c",
        "user.email=p@invalid",
        "commit",
        "-qm",
        "P fixture",
    ];
    git(&repo, &commit)?;
    let base = git(&repo, &["rev-parse", "HEAD"])?;
    {
        use std::io::{Seek, SeekFrom};
        let raw = b"CANARY_INITIAL\r\n";
        owned_a
            .seek(SeekFrom::Start(0))
            .and_then(|_| owned_a.write_all(raw))
            .and_then(|_| owned_a.set_len(raw.len() as u64))
            .and_then(|_| owned_a.sync_all())
            .map_err(|_| "P_PREPARE_FAILED")?;
    }
    drop(owned_a);
    git(&repo, &["add", "a.txt"])?;
    git(&repo, &commit)?;
    let head = git(&repo, &["rev-parse", "HEAD"])?;
    let input:DeliveryInput=serde_json::from_value(json!({"stage":"p-zero-turn","package":"p1122a","base_sha":base,"head_sha":head,"goal":"Fixed P protocol observation; no turn",
        "acceptance":["eight fixed frames; no model turn"],"allowed_paths":["a.txt"],"required_commands":[],"logs":[],"developer_report":{
        "schema":"nova.scheduler.report/1","report_type":"DEVELOPMENT","claimed_delivery_sha":head,"claimed_conclusion":"NOT_RUN","issues":[],
        "context":{"package_id":"p1122a","stage":"INITIAL","supersedes_sha":null,"pm_decision_ref":null,"decision_kind":null,"slot_resolution":null},
        "claims":{"author":"TOOL PREPARED; MODEL NOT RUN","role":"development","fresh_session":null,"model":null,"ci_runs":[]},"source_report":null,"transcribed_by":null}})).map_err(|_|"ENCODING")?;
    let s = h::init_work(
        &h::store_path(&repo)?,
        &repo,
        &ctx.root.join("evidence"),
        h::Stage {
            stage: input.stage.clone(),
            developer: None,
            reviewer: None,
        },
        input.clone(),
    )?;
    let mode = {
        #[cfg(test)]
        {
            Mode::Synthetic
        }
        #[cfg(not(test))]
        {
            Mode::RealPendingAdmission
        }
    };
    attach(
        &s,
        Plan {
            schema: "nova.self2.plan/1".into(),
            mode,
            input,
            project_root: repo,
            cli: ctx.cli,
            cli_sha256: cli_hash,
            cli_version: CLI_VERSION.into(),
            model: c.model.clone(),
            commands: vec![],
        },
    )?;
    let r = request(&s, &c)?;
    let digest = hash(&bytes(&r)?);
    let (rev, mut st) = h::load(&s)?;
    st.self2.as_mut().unwrap().app_server = Some(
        json!({"mode":"P_ZERO_TURN","status":"P_DRAFT","plan_sha256":r.plan_hash,"probe_request":r,"digest":digest,"sends":[]}),
    );
    h::save(&s, rev, &st)?;
    evaluate_store(&s)
}
pub fn approve(digest: &str, line: &str) -> Result<Value> {
    startup_admission()?;
    let s = open()?;
    approve_store(&s, digest, line)
}
fn approve_store(s: &Store, digest: &str, line: &str) -> Result<Value> {
    startup_admission()?;
    let r = load_request(s)?;
    valid_time(r.expires)?;
    if digest != hash(&bytes(&r)?) || line != format!("AUTHORIZE P ZERO TURN 1 SERVER {digest}") {
        return Err("P_APPROVAL_REJECTED");
    }
    modify(s, |a| {
        if a["status"] != "P_DRAFT" || a["digest"] != digest {
            return Err("P_NO_REPLAY");
        };
        a["status"] = json!("P_APPROVED");
        Ok(())
    })?;
    evaluate_store(s)
}
pub fn evaluate() -> Result<Value> {
    evaluate_store(&open()?)
}
fn evaluate_store(s: &Store) -> Result<Value> {
    let r = load_request(s)?;
    let a = app_state(s)?;
    Ok(
        json!({"schema":"nova.app-server.p-summary/1","request":r,"digest":hash(&bytes(&r)?),"status":a["status"],"zero_turn":true,"approval_current":valid_time(r.expires).is_ok(),
        "startup_admission":startup_admission().err(),"pid":a["pid"],"creation":a["creation"],"sends":a["sends"],"notifications":a["notifications"],"process":a["p_process"],
        "error":a["error"],"model_authorized":false,"network":"CLI_INTERNAL_EFFECTS_NOT_PROVEN_ABSENT","retry":"FORBIDDEN"}),
    )
}
fn spec(r: &Request) -> Spec {
    Spec {
        role: RelayRole::Develop,
        profile: "SELF2_APP_SERVER_P".into(),
        exe: r.cli.clone(),
        exe_sha256: r.cli_hash.clone(),
        argv: r.argv.clone(),
        env: r.env.clone(),
        cwd: r.root.join("project"),
        session: String::new(),
    }
}
pub fn run() -> Result<Value> {
    startup_admission()?;
    let s = open()?;
    run_store(&s, None)
}
fn run_store(
    s: &Store,
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<Value> {
    startup_admission()?;
    let r = load_request(s)?;
    valid_time(r.expires)?;
    let spec = spec(&r);
    let (rev, mut st) = h::load(s)?;
    let a = st.self2.as_mut().unwrap().app_server.as_mut().unwrap();
    if a["status"] != "P_APPROVED" || a["digest"] != hash(&bytes(&r)?) {
        return Err("P_NO_APPROVAL_OR_REPLAY");
    }
    a["status"] = json!("STARTED");
    let mut attempt = launch_attempt(&spec);
    attempt.profile_id = spec.profile.clone();
    let permit = s.self2_launch(
        rev,
        &serde_json::to_value(st).map_err(|_| "ENCODING")?,
        attempt,
    )?;
    let binding = TransportBinding {
        probe_only: true,
        probe_model: Some(r.model.clone()),
        spec_sha256: spec_hash(&spec)?,
        plan_sha256: r.plan_hash,
        root: spec.cwd.clone(),
        immutable: canary_tree(&spec.cwd)?,
        before: vec![],
        after: vec![],
        #[cfg(test)]
        case: SOURCE.with(|s| s.borrow().as_ref().unwrap().case.clone()),
    };
    let end = Instant::now() + Duration::from_secs(90);
    let mut pid = 0;
    let mut created = String::new();
    #[cfg(test)]
    SOURCE.with(|s| {
        s.borrow()
            .as_ref()
            .unwrap()
            .starts
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    });
    let io = AppServerChannel::open(&binding, &spec, &permit, end, &mut |p, c| {
        pid = p;
        created = c.clone();
        modify(s, |a| {
            a["pid"] = json!(p);
            a["creation"] = json!(c);
            a["status"] = json!("ACTIVE");
            Ok(())
        })
    });
    let io = match io {
        Ok(io) => io,
        Err(e) => {
            modify(s, |a| {
                a["status"] = json!("UNKNOWN");
                a["error"] = json!(e);
                Ok(())
            })?;
            return Err(e);
        }
    };
    let mut d = Adapter {
        store: s,
        binding: &binding,
        io,
        next_id: 1,
        received_frames: 0,
        pending_thread_started: None,
        started_threads: BTreeSet::new(),
        turn_ids: BTreeSet::new(),
        round: 0,
        developer: None,
        reviewer: None,
        review_session: None,
        review_packet: None,
        pid,
        created,
        end,
    };
    #[cfg(test)]
    if let Some(c) = cancel {
        let token = d.io.cancel_handle();
        std::thread::spawn(move || {
            while !c.load(std::sync::atomic::Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(5));
            }
            token.store(true, std::sync::atomic::Ordering::Release);
        });
    }
    #[cfg(not(test))]
    let _ = cancel;
    let result = (|| {
        #[cfg(test)]
        {
            let before = bytes(&h::load(s)?)?;
            assert_eq!(
                d.io.send(b"{\"id\":1,\"method\":\"turn/start\"}\n", end),
                Err("P_FRAME_REJECTED")
            );
            assert_eq!(before, bytes(&h::load(s)?)?);
        }
        d.ready()?;
        d.thread(false, false)?;
        d.thread(true, false)?;
        d.thread(true, true)?;
        d.check_config()?;
        if material(&r.root)? != r.material {
            return Err("P_MATERIAL_DRIFT");
        }
        Ok(())
    })();
    let cleanup = d.io.finish();
    let close = cleanup.natural && cleanup.exit_code == Some(0) && cleanup.reclaim == "CONFIRMED";
    modify(s, |a| {
        a["status"] = json!(if result.is_ok() && close {
            "P_COMPLETE"
        } else {
            "UNKNOWN"
        });
        a["error"] = json!(result.err().or(if close {
            None
        } else {
            Some("P_CLOSE_UNVERIFIED")
        }));
        a["p_process"] = json!({"pid":cleanup.pid,"creation":cleanup.process_creation_utc,"created":cleanup.created,"natural":cleanup.natural,"exit_code":cleanup.exit_code,"reclaim":cleanup.reclaim,"stdout_bytes":cleanup.stdout.len(),"stderr_bytes":cleanup.stderr.len(),"cleanup":cleanup.cleanup});
        if let Some(row) = a["sends"].as_array_mut().and_then(|v| v.last_mut()) {
            if row["state"] == "STARTED" {
                row["state"] = json!("UNKNOWN")
            }
        }
        Ok(())
    })?;
    result?;
    if !close {
        return Err("P_CLOSE_UNVERIFIED");
    };
    evaluate_store(s)
}
// Independent lower transport guard: even direct send cannot emit a turn or ninth frame.
pub(crate) fn wire_gate(index: usize, wire: &[u8]) -> Result<()> {
    const SEQ: [(&str, Option<u64>); 8] = [
        ("initialize", Some(1)),
        ("initialized", None),
        ("config/read", Some(2)),
        ("thread/start", Some(3)),
        ("thread/start", Some(4)),
        ("thread/loaded/list", Some(5)),
        ("thread/resume", Some(6)),
        ("config/read", Some(7)),
    ];
    if index >= SEQ.len() || wire.len() > INPUT_CAP || !wire.ends_with(b"\n") {
        return Err("P_FRAME_REJECTED");
    }
    let v: Value = relay_json(wire, INPUT_CAP)?;
    let (method, id) = SEQ[index];
    if v["method"] != method
        || v.get("id") != id.map(|n| json!(n)).as_ref()
        || v.get("result").is_some()
        || v.get("error").is_some()
    {
        return Err("P_FRAME_REJECTED");
    }
    Ok(())
}
pub(super) fn check_frame(
    d: &Adapter<'_>,
    method: &str,
    params: &Value,
    id: Option<u64>,
) -> Result<()> {
    let i = app_state(d.store)?["sends"]
        .as_array()
        .ok_or("P_STATE")?
        .len();
    let raw = if let Some(id) = id {
        bytes(&json!({"id":id,"method":method,"params":params}))?
    } else {
        bytes(&json!({"method":method}))?
    };
    let mut raw = raw;
    raw.push(b'\n');
    wire_gate(i, &raw)?;
    // All params are constructed internally. In particular there is no free input text/tool/RPC argument.
    Ok(())
}
// Only fixed codes/counts/canonical UUIDs enter durable P evidence. No arbitrary message/config/paths.
pub(super) fn safe_reply(v: &Value) -> Value {
    let r = &v["result"];
    if let Some(c) = r.get("config") {
        return json!({"kind":"CONFIG","openai":c["model_provider"]=="openai","model_present":c["model"].is_string(),"project_docs_disabled":c["project_doc_max_bytes"]==0,"mcp_empty":c["mcp_servers"].as_object().is_some_and(|m|m.is_empty()),"features_disabled":(["plugins","hooks","plugin_hooks","multi_agent","multi_agent_v2"].iter().all(|k|c["features"][k]==false))});
    }
    if let Some(t) = r.get("thread") {
        return json!({"kind":"THREAD","id":t["id"].as_str().filter(|v|uuid_ok(v)),"session":t["sessionId"].as_str().filter(|v|uuid_ok(v)),"idle":t["status"]["type"]=="idle","read_only":r["sandbox"]["type"]=="readOnly"});
    }
    if let Some(ids) = r["data"].as_array() {
        return json!({"kind":"LOADED","count":ids.len(),"no_more":r["nextCursor"].is_null()});
    }
    json!({"kind":"ACK"})
}
#[cfg(test)]
pub(crate) mod tests;

// A closed eight-frame capability at the byte transport, not just a method filter.
pub(crate) struct WireGate {
    next: usize,
    model: String,
    root: PathBuf,
    review: Option<String>,
}
impl WireGate {
    pub(crate) fn new(model: &str, root: &Path) -> Self {
        Self {
            next: 0,
            model: model.into(),
            root: root.into(),
            review: None,
        }
    }
    pub(crate) fn bind(&mut self, dev: &str, review: &str) -> Result<()> {
        if self.next != 5
            || self.review.is_some()
            || !uuid_ok(dev)
            || !uuid_ok(review)
            || dev == review
        {
            return Err("P_THREAD_BINDING");
        }
        self.review = Some(review.into());
        Ok(())
    }
    pub(crate) fn advance(&mut self) {
        self.next += 1;
    }
    pub(crate) fn check(&self, wire: &[u8]) -> Result<()> {
        wire_gate(self.next, wire)?;
        let v: Value = relay_json(wire, INPUT_CAP)?;
        let params = match self.next {
            0 => {
                json!({"clientInfo":{"name":"nova_fixed_canary","version":"1"},"capabilities":{"experimentalApi":true}})
            }
            1 => Value::Null,
            2 | 7 => json!({"includeLayers":false,"cwd":self.root}),
            5 => json!({"limit":3}),
            3 | 4 | 6 => {
                let mut p = json!({"model":self.model,"modelProvider":"openai","cwd":self.root,"runtimeWorkspaceRoots":[],"approvalPolicy":"never","approvalsReviewer":"user","sandbox":if self.next==3{"workspace-write"}else{"read-only"},
                    "config":{"windows.sandbox":"unelevated","sandbox_workspace_write.network_access":false,"features.plugins":false,"features.hooks":false,"features.multi_agent":false,"project_doc_max_bytes":0}});
                if self.next == 6 {
                    p["threadId"] = json!(self.review.as_ref().ok_or("P_THREAD_BINDING")?);
                    p["excludeTurns"] = json!(true);
                } else {
                    p["dynamicTools"] = json!([]);
                    p["ephemeral"] = json!(false);
                }
                p
            }
            _ => return Err("P_FRAME_REJECTED"),
        };
        let mut expected = json!({"method":v["method"]});
        if self.next != 1 {
            expected["id"] = v["id"].clone();
            expected["params"] = params;
        }
        if v != expected {
            return Err("P_FRAME_REJECTED");
        };
        Ok(())
    }
}
