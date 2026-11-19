//! SELF-2: Codex protocol and durable launch adapter over SELF-1, not a second scheduler.
use super::{
    cli_adapter::{self, Spec},
    contract::*,
    relay::{Driver as Supervised, Execution},
    self_handoff::{self as h, DeliveryInput, ReviewInput, SessionBinding},
    store::{Guard, LaunchPermit, Store},
    Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
pub const INPUT_CAP: usize = 2 * 1024 * 1024;
pub const LOG_CAP: usize = 256 * 1024;
pub const CLI_VERSION: &str = "codex-cli 0.162.1";
pub const CLI_SHA: &str = "dd13bdb162a184fa04039488b234a8258ef9b76f84e926c505d5742b19249a6a";
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Role {
    Develop,
    Verify,
    Review,
}
impl Role {
    fn relay(self) -> RelayRole {
        if self == Self::Review {
            RelayRole::Review
        } else {
            RelayRole::Develop
        }
    }
    fn profile(self) -> String {
        format!("SELF2_{self:?}").to_uppercase()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixedCommand {
    pub exe: PathBuf,
    pub sha256: String,
    pub args: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Mode {
    Synthetic,
    RealPendingAdmission,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plan {
    pub mode: Mode,
    pub schema: String,
    pub input: DeliveryInput,
    pub project_root: PathBuf,
    pub cli: PathBuf,
    pub cli_sha256: String,
    pub cli_version: String,
    pub model: String,
    pub commands: Vec<FixedCommand>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub key: String,
    pub attempt: Attempt,
    pub spec_sha256: String,
    pub expected_uuid: Option<String>,
    pub outcome: String,
    pub execution: Option<Execution>,
    pub stdout_path: Option<PathBuf>,
    pub stderr_path: Option<PathBuf>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Journal {
    pub schema: String,
    pub plan: Plan,
    pub plan_sha256: String,
    pub records: Vec<Record>,
}
fn bytes<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|_| "ENCODING")
}
fn uuid_ok(s: &str) -> bool {
    uuid::Uuid::parse_str(s).is_ok_and(|u| u.to_string() == s)
}
fn plan_cli_pin(p: &Plan) -> bool {
    if p.cli_sha256==CLI_SHA {return true}
    #[cfg(test)]
    { return super::admission_probe::synthetic_cli_binding(&p.project_root,&p.cli,&p.cli_sha256); }
    #[cfg(not(test))]
    { false }
}
fn check(s: &Store, p: &Plan) -> Result<()> {
    let st = h::load(s)?.1;
    if p.schema != "nova.self2.plan/1"
        || p.input.stage != st.stage.stage
        || !plan_cli_pin(p)
        || p.cli_version != CLI_VERSION
        || !p.cli.is_absolute()
        || p.model.is_empty()
        || p.model.len() > 128
        || p.model.chars().any(char::is_control)
        || p.commands.len() > 16
        || p.input.required_commands.len() != p.commands.len()
    {
        return Err("SELF2_PLAN_REJECTED");
    }
    let snap = s.snapshot()?;
    if st.closed.is_empty()
        && (snap.manifest.package_id != p.input.package
            || snap.manifest.goal != p.input.goal
            || snap.manifest.allowed_paths != p.input.allowed_paths
            || snap.manifest.acceptance_items != p.input.acceptance
            || snap.manifest.baseline.base_sha.as_deref() != Some(&p.input.base_sha))
    {
        return Err("SELF2_APPROVED_INPUT_MISMATCH");
    }
    let g = Guard::open(&p.project_root, true)?;
    if g.identity != s.audit_envelope()?.root_identity
        || g.path != p.project_root
        || s.snapshot()?.manifest.repo_path != p.project_root.to_string_lossy()
    {
        return Err("SELF2_ROOT_MISMATCH");
    }
    for (c, r) in p.commands.iter().zip(&p.input.required_commands) {
        let mut a = vec![c.exe.to_string_lossy().into_owned()];
        a.extend(c.args.clone());
        if !c.exe.is_absolute()
            || !hex(&c.sha256, 64)
            || a != *r
            || c.args.iter().any(|a| a.len() > 4096 || a.contains('\0'))
        {
            return Err("SELF2_COMMAND_REJECTED");
        }
    }
    if bytes(p)?.len() > INPUT_CAP {
        return Err("SELF2_MATERIAL_BOUND");
    }
    Ok(())
}
/// Help output cannot authorize model usage or establish Windows sandbox enforcement.
/// This offline package deliberately exposes no live enable flag or permissive fallback.
pub fn live_admission() -> Result<()> {
    #[cfg(any(test, all(feature = "scheduler-validation", debug_assertions)))]
    { finish_admission(None, None, "") }
    #[cfg(not(any(test, all(feature = "scheduler-validation", debug_assertions))))]
    { Err("SELF2_NATIVE_PERMISSION_AND_UUID_ADMISSION_REQUIRED") }
}
/// Opaque future model authorization: no public constructor, JSON import, PASS field or env switch.
/// This package issues none. Permission success alone cannot construct this independent authority.
#[cfg(any(test, all(feature = "scheduler-validation", debug_assertions)))]
pub struct ModelAuthorization {
    plan_sha256: String,
    permission_fingerprint: String,
    expires_utc: i64,
}
#[cfg(any(test, all(feature = "scheduler-validation", debug_assertions)))]
pub(super) fn finish_admission(
    permission: Option<&super::admission_probe::PermissionBinding>,
    model: Option<&ModelAuthorization>,
    plan_sha256: &str,
) -> Result<()> {
    let permission=permission.ok_or("SELF2_NATIVE_PERMISSION_AND_UUID_ADMISSION_REQUIRED")?;
    let model=model.ok_or("SELF2_MODEL_AUTHORIZATION_REQUIRED")?;
    if model.plan_sha256!=plan_sha256 || model.permission_fingerprint!=permission.fingerprint() {
        return Err("SELF2_MODEL_AUTHORIZATION_BINDING_MISMATCH");
    }
    if model.expires_utc<=chrono::Utc::now().timestamp() {return Err("SELF2_MODEL_AUTHORIZATION_EXPIRED")}
    // Non-model W/R does not establish exec/resume/model-session or UUID enforcement.
    // A later specifically authorized canary must supply that admission; never launch here.
    Err("SELF2_MODEL_SESSION_AND_UUID_ADMISSION_REQUIRED")
}
#[cfg(test)]
pub(super) fn test_model_authorization_binding(permission: &super::admission_probe::PermissionBinding) {
    let mut model=ModelAuthorization { plan_sha256: "synthetic-plan".into(),
        permission_fingerprint: permission.fingerprint().into(), expires_utc: chrono::Utc::now().timestamp()+60 };
    assert_eq!(finish_admission(Some(permission),Some(&model),"wrong-plan"),Err("SELF2_MODEL_AUTHORIZATION_BINDING_MISMATCH"));
    model.permission_fingerprint="0".repeat(64);
    assert_eq!(finish_admission(Some(permission),Some(&model),"synthetic-plan"),Err("SELF2_MODEL_AUTHORIZATION_BINDING_MISMATCH"));
    model.permission_fingerprint=permission.fingerprint().into();model.expires_utc=0;
    assert_eq!(finish_admission(Some(permission),Some(&model),"synthetic-plan"),Err("SELF2_MODEL_AUTHORIZATION_EXPIRED"));
    model.expires_utc=chrono::Utc::now().timestamp()+60;
    assert_eq!(finish_admission(Some(permission),Some(&model),"synthetic-plan"),Err("SELF2_MODEL_SESSION_AND_UUID_ADMISSION_REQUIRED"));
}
/// Read-only Store-scoped admission check. No CAS, permit acquisition or transport construction.
#[cfg(any(test, all(feature = "scheduler-validation", debug_assertions)))]
pub fn admission_for(
    s: &Store,
    scope: &super::admission_probe::PermissionScope,
    model: Option<&ModelAuthorization>,
) -> Result<()> {
    let j=journal(s)?;
    if j.records.iter().any(|r| r.outcome!="COMPLETE") {return Err("SELF2_RECONCILE_REQUIRED")}
    let stage=h::load(s)?.1.stage;
    if [&stage.developer,&stage.reviewer].iter().any(|v|v.as_ref().is_some_and(|v|v.source=="SYNTHETIC_CLI_RECEIPT")) {
        return Err("SELF2_SYNTHETIC_IDENTITY_NOT_ADMISSION");
    }
    if !matches!(j.plan.mode,Mode::RealPendingAdmission) || j.plan.project_root!=scope.project_root
        || j.plan.cli!=scope.cli || j.plan.cli_sha256!=scope.cli_sha256 {
        return Err("SELF2_PERMISSION_SCOPE_MISMATCH");
    }
    if j.plan.input.allowed_paths.iter().any(|p|p==".gitignore" || p==".git" || p.starts_with(".git/") || p==".nova-permission" || p.starts_with(".nova-permission/")) {
        return Err("SELF2_PERMISSION_RESERVED_SCOPE");
    }
    // Existing material contract allows declared source edits but rejects outside-scope drift.
    let binding=super::admission_probe::bind_permission(scope)?;
    h::material_for(&j.plan.project_root,&j.plan.input,false)?;
    finish_admission(Some(&binding),model,&j.plan_sha256)
}
pub fn attach(s: &Store, p: Plan) -> Result<()> {
    check(s, &p)?;
    let (rev, mut st) = h::load(s)?;
    if st.self2.is_some()
        || st.active.is_some()
        || st.closed.iter().any(|v| v["package"] == p.input.package)
    {
        return Err("SELF2_ALREADY_RESERVED");
    }
    if p.input.developer_report.claimed_conclusion != Conclusion::NotRun || !p.input.logs.is_empty()
    {
        return Err("SELF2_PREDEVELOPMENT_REQUIRED");
    }
    h::material_for(&p.project_root, &p.input, false)?;
    st.schema = "nova.self-state/2".into();
    st.self2 = Some(Journal {
        schema: "nova.self2.journal/1".into(),
        plan_sha256: hash(&bytes(&p)?),
        plan: p,
        records: vec![],
    });
    h::save(s, rev, &st)
}
fn journal(s: &Store) -> Result<Journal> {
    let j = h::load(s)?.1.self2.ok_or("SELF2_UNINITIALIZED")?;
    if j.schema != "nova.self2.journal/1" || hash(&bytes(&j.plan)?) != j.plan_sha256 {
        return Err("SELF2_JOURNAL_REJECTED");
    }
    check(s, &j.plan)?;
    Ok(j)
}
/// Use the enclosing exec permission options even for resume; never --last/--all.
pub fn codex_args(
    p: &Plan,
    role: Role,
    session: Option<&str>,
    schema: &Path,
) -> Result<Vec<String>> {
    if role == Role::Verify || !schema.is_absolute() || session.is_some_and(|u| !uuid_ok(u)) {
        return Err("SELF2_ARGUMENT_REJECTED");
    }
    let policy = if role == Role::Review {
        "read-only"
    } else {
        "workspace-write"
    };
    let mut a = vec![
        "--no-daemon".into(),
        "--ask-for-approval".into(),
        "never".into(),
        "exec".into(),
        "-C".into(),
        p.project_root.to_string_lossy().into_owned(),
        "--sandbox".into(),
        policy.into(),
    ];
    if let Some(u) = session {
        a.extend(["resume".into(), u.into()]);
    }
    a.extend([
        "--ignore-user-config".into(),
        "--json".into(),
        "--output-schema".into(),
        schema.to_string_lossy().into_owned(),
        "-m".into(),
        p.model.clone(),
        "-c".into(),
        "approval_policy=\"never\"".into(),
        "-c".into(),
        format!("sandbox_mode=\"{policy}\""),
        "-".into(),
    ]);
    Ok(a)
}
fn output_schema() -> Value {
    json!({"type":"object","additionalProperties":false,"required":["schema","stage","package","head_sha","packet_sha256","report"],"properties":{"schema":{"type":"string","const":"nova.self2.result/1"},"stage":{"type":"string"},"package":{"type":"string"},"head_sha":{"type":"string"},"packet_sha256":{"type":"string"},"report":{"type":"string","description":"Complete nova.scheduler.report/1 JSON, strictly decoded locally; not prose"}}})
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Reply {
    schema: String,
    stage: String,
    package: String,
    head_sha: String,
    packet_sha256: String,
    #[serde(deserialize_with = "decode_report")]
    report: Report,
}
fn decode_report<'de, D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Report, D::Error> {
    let text = String::deserialize(d)?;
    Report::parse(text.as_bytes()).map_err(serde::de::Error::custom)
}
fn parse_wire(b: &[u8], role: Role, expected: Option<&str>) -> Result<(String, Reply, Value)> {
    if b.len() > 8 * 1024 * 1024 || !b.ends_with(b"\n") {
        return Err("SELF2_PROTOCOL_BOUND");
    }
    let (mut id, mut started, mut done, mut reply) = (None, false, false, None);
    let mut commands = vec![];
    let mut usage = Value::Null;
    for line in b.split(|b| *b == b'\n').filter(|l| !l.is_empty()) {
        let v: Value = relay_json(line, 1024 * 1024)?;
        if done {
            return Err("SELF2_TRAILING_EVENT");
        }
        match v["type"].as_str() {
            Some("thread.started") if id.is_none() && !started => {
                let u = v["thread_id"].as_str().ok_or("SELF2_UUID_MISSING")?;
                if !uuid_ok(u) || expected.is_some_and(|e| e != u) {
                    return Err("SELF2_UUID_MISMATCH");
                }
                id = Some(u.to_owned());
            }
            Some("turn.started") if id.is_some() && !started => started = true,
            Some("item.started" | "item.updated" | "item.completed") if started => {
                let i = &v["item"];
                match i["type"].as_str() {
                    Some("reasoning" | "todo_list") => {}
                    Some("agent_message") if v["type"] == "item.completed" => {
                        if reply.is_some() {
                            return Err("SELF2_DUPLICATE_RESULT");
                        }
                        reply = Some(relay_json::<Reply>(
                            i["text"].as_str().ok_or("SELF2_RESULT_MISSING")?.as_bytes(),
                            65536,
                        )?);
                    }
                    Some("agent_message") => {}
                    Some("command_execution") => {
                        if v["type"] == "item.completed" {
                            commands.push(json!({"exit_code":i["exit_code"],"status":i["status"],"source":"CLI_EVENT_NOT_INDEPENDENT_TEST_VERIFICATION"}));
                        }
                    }
                    Some("file_change") if role == Role::Develop => {}
                    _ => return Err("SELF2_TOOL_OR_EVENT_REJECTED"),
                }
            }
            Some("turn.completed") if started => {
                done = true;
                usage = json!({"upstream_usage":v["usage"],"billing":"NOT_OBSERVED","commands":commands});
            }
            Some("turn.failed" | "error") => return Err("SELF2_TURN_FAILED"),
            _ => return Err("SELF2_EVENT_REJECTED"),
        }
    }
    if !done {
        return Err("SELF2_TERMINAL_MISSING");
    }
    Ok((
        id.ok_or("SELF2_UUID_MISSING")?,
        reply.ok_or("SELF2_RESULT_MISSING")?,
        usage,
    ))
}
fn save_new(p: &Path, b: &[u8]) -> Result<()> {
    let g = Guard::open(p.parent().ok_or("PATH_REJECTED")?, true)?;
    let mut f = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(p)
        .map_err(|_| "EVIDENCE_WRITE")?;
    f.write_all(b)
        .and_then(|_| f.sync_all())
        .map_err(|_| "EVIDENCE_FLUSH")?;
    g.verify()
}
mod sealed {
    pub trait Sealed {}
}
pub trait Driver: sealed::Sealed {
    fn invoke(
        &mut self,
        s: &Spec,
        b: &[u8],
        p: &LaunchPermit,
        end: Instant,
        r: &mut dyn FnMut(u32, String) -> Result<()>,
    ) -> Execution;
}
/// Synthetic native transport only. It cannot point at the installed real Codex executable.
pub struct FakeCli {
    inner: cli_adapter::NativeDriver,
    exe: PathBuf,
    sha: String,
}
impl sealed::Sealed for FakeCli {}
impl FakeCli {
    pub fn new(exe: &Path, sha: &str) -> Result<Self> {
        if exe.file_name().and_then(|s| s.to_str()) != Some("nova_scheduler_self2_fake_cli.exe") {
            return Err("SELF2_FAKE_NAME_REJECTED");
        }
        Ok(Self {
            inner: cli_adapter::NativeDriver::self2_transport(exe, sha)?,
            exe: exe.into(),
            sha: sha.into(),
        })
    }
    pub fn cancel_handle(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.inner.cancel.clone()
    }
    pub fn cancel(&self) {
        self.inner
            .cancel
            .store(true, std::sync::atomic::Ordering::Release);
    }
}
impl Driver for FakeCli {
    fn invoke(
        &mut self,
        s: &Spec,
        b: &[u8],
        p: &LaunchPermit,
        end: Instant,
        r: &mut dyn FnMut(u32, String) -> Result<()>,
    ) -> Execution {
        if s.exe != self.exe || s.exe_sha256 != self.sha {
            return Execution::failed("BINDING_MISMATCH");
        }
        let spec = s.clone();
        if spec.argv.first().map(String::as_str) != Some("--nova-self2-synthetic-only") {
            return Execution::failed("BINDING_MISMATCH");
        }
        // The permit binds the actual prefixed vector; real Codex rejects this fixture-only switch.
        if spec.argv_sha256() != p.attempt().argv_sha256 {
            return Execution::failed("BINDING_MISMATCH");
        }
        self.inner.execute(&spec, b, p, end, r)
    }
}
fn invoke(
    s: &Store,
    d: &mut impl Driver,
    role: Role,
    index: usize,
    spec: Spec,
    b: Vec<u8>,
    expected: Option<String>,
    dir: &Path,
) -> Result<Execution> {
    if b.len() > INPUT_CAP {
        return Err("SELF2_MATERIAL_BOUND");
    }
    let (rev, mut st) = h::load(s)?;
    let repairs = st.active.as_ref().map_or(0, |p| p.repairs);
    let j = st.self2.as_mut().ok_or("SELF2_UNINITIALIZED")?;
    let key = format!("{}:{repairs}:{role:?}:{index}", j.plan.input.package);
    let sh = hash(&bytes(
        &json!({"exe":spec.exe,"sha":spec.exe_sha256,"argv":spec.argv,"cwd":spec.cwd,"env":spec.env}),
    )?);
    if let Some(old) = j.records.iter().find(|r| r.key == key) {
        if old.attempt.input_identity != hash(&b)
            || old.spec_sha256 != sh
            || old.expected_uuid != expected
        {
            return Err("SELF2_REPLAY_BINDING_MISMATCH");
        }
        return if old.outcome == "COMPLETE" {
            {
                let mut e = old.execution.clone().ok_or("SELF2_RECEIPT_MISSING")?;
                let out = super::store::read_file(
                    old.stdout_path.as_ref().ok_or("SELF2_RECEIPT_MISSING")?,
                    LOG_CAP,
                )?
                .0;
                let err = super::store::read_file(
                    old.stderr_path.as_ref().ok_or("SELF2_RECEIPT_MISSING")?,
                    LOG_CAP,
                )?
                .0;
                if old.attempt.stdout_sha256.as_deref() != Some(hash(&out).as_str())
                    || old.attempt.stderr_sha256.as_deref() != Some(hash(&err).as_str())
                {
                    return Err("SELF2_LOG_TAMPERED");
                }
                e.stdout = out;
                e.stderr = err;
                Ok(e)
            }
        } else {
            Err("SELF2_RECONCILE_REQUIRED")
        };
    }
    if j.records.iter().any(|r| r.outcome != "COMPLETE") {
        return Err("SELF2_RECONCILE_REQUIRED");
    }
    let a = Attempt {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        role: role.relay(),
        state: "LAUNCHING".into(),
        state_version: 0,
        input_identity: hash(&b),
        parent_attempt_id: if role == Role::Develop {
            None
        } else {
            Some(
                j.records
                    .iter()
                    .rev()
                    .find(|r| {
                        r.attempt.profile_id == Role::Develop.profile() && r.outcome == "COMPLETE"
                    })
                    .ok_or("SELF2_DEVELOPER_RECEIPT_REQUIRED")?
                    .attempt
                    .attempt_id
                    .clone(),
            )
        },
        profile_id: role.profile(),
        exe_sha256: spec.exe_sha256.clone(),
        argv_sha256: spec.argv_sha256(),
        env_names_sha256: spec.env_names_sha256(),
        started_utc: Some(chrono::Utc::now().to_rfc3339()),
        ended_utc: None,
        pid: None,
        process_creation_utc: None,
        exit_code: None,
        terminal: None,
        reclaim: None,
        session_id: None,
        stdout_bytes: None,
        stdout_sha256: None,
        stderr_bytes: None,
        stderr_sha256: None,
        result_sha256: None,
        model_claim_json: None,
        usage_json: None,
    };
    j.records.push(Record {
        key: key.clone(),
        attempt: a.clone(),
        spec_sha256: sh,
        expected_uuid: expected.clone(),
        outcome: "STARTED".into(),
        execution: None,
        stdout_path: None,
        stderr_path: None,
    });
    let permit = s.self2_launch(
        rev,
        &serde_json::to_value(&st).map_err(|_| "ENCODING")?,
        a.clone(),
    )?;
    let mut e = d.invoke(
        &spec,
        &b,
        &permit,
        Instant::now() + Duration::from_secs(600),
        &mut |pid, created| {
            let (rev, mut st) = h::load(s)?;
            let r = st
                .self2
                .as_mut()
                .unwrap()
                .records
                .last_mut()
                .ok_or("SELF2_RECEIPT_MISSING")?;
            if r.key != key || r.outcome != "STARTED" {
                return Err("SELF2_RECEIPT_CONFLICT");
            }
            r.attempt.pid = Some(pid);
            r.attempt.process_creation_utc = Some(created);
            r.attempt.state = "RUNNING".into();
            h::save(s, rev, &st)
        },
    );
    if role != Role::Verify && e.natural && e.exit_code == Some(0) && e.terminal == "SUCCESS" {
        match parse_wire(&e.stdout, role, expected.as_deref()) {
            Ok((id, _, usage)) => {
                e.session_id = Some(id);
                e.usage = Some(usage);
            }
            Err(code) => {
                e.terminal = code.into();
                e.unknowns.push("SELF2_RECEIPT_UNTRUSTED".into());
            }
        }
    }
    let out = dir.join(format!("{}.stdout", a.attempt_id));
    let err = dir.join(format!("{}.stderr", a.attempt_id));
    let saved = e.stdout.len() <= LOG_CAP
        && e.stderr.len() <= LOG_CAP
        && save_new(&out, &e.stdout).is_ok()
        && save_new(&err, &e.stderr).is_ok();
    let ok = e.natural
        && e.exit_code == Some(0)
        && e.terminal == "SUCCESS"
        && e.reclaim == "CONFIRMED"
        && e.unknowns.is_empty()
        && saved;
    if !saved {
        e.unknowns.push("SELF2_EVIDENCE_UNAVAILABLE".into());
    }
    let (rev, mut st) = h::load(s)?;
    let r = st
        .self2
        .as_mut()
        .unwrap()
        .records
        .last_mut()
        .ok_or("SELF2_RECEIPT_MISSING")?;
    if r.key != key || r.outcome != "STARTED" {
        return Err("SELF2_RECEIPT_CONFLICT");
    }
    r.outcome = if ok {
        "COMPLETE"
    } else if e.natural
        && e.exit_code.is_some()
        && saved
        && e.reclaim == "CONFIRMED"
        && e.unknowns.is_empty()
    {
        "STOPPED"
    } else {
        "UNKNOWN"
    }
    .into();
    r.attempt.state = if r.outcome == "COMPLETE" {
        "EXITED"
    } else {
        &r.outcome
    }
    .into();
    r.attempt.session_id = e.session_id.clone();
    r.attempt.usage_json = e.usage.as_ref().map(|v| v.to_string());
    r.attempt.exit_code = e.exit_code;
    r.attempt.ended_utc = Some(chrono::Utc::now().to_rfc3339());
    r.attempt.terminal = Some(e.terminal.clone());
    r.attempt.reclaim = Some(e.reclaim.clone());
    r.attempt.stdout_sha256 = Some(hash(&e.stdout));
    r.attempt.stderr_sha256 = Some(hash(&e.stderr));
    r.attempt.stdout_bytes = Some(e.stdout.len() as u64);
    r.attempt.stderr_bytes = Some(e.stderr.len() as u64);
    r.stdout_path = saved.then_some(out);
    r.stderr_path = saved.then_some(err);
    let mut projected = e.clone();
    projected.stdout.clear();
    projected.stderr.clear();
    r.execution = Some(projected);
    h::save(s, rev, &st)?;
    if !ok {
        return Err("SELF2_EXECUTION_STOPPED_OR_UNKNOWN");
    }
    Ok(e)
}
fn accept(
    e: &Execution,
    role: Role,
    expected: Option<&str>,
    p: &Plan,
    packet: &str,
) -> Result<(String, Report)> {
    let (id, r, _) = parse_wire(&e.stdout, role, expected)?;
    if r.schema != "nova.self2.result/1"
        || r.stage != p.input.stage
        || r.package != p.input.package
        || r.head_sha != p.input.head_sha
        || r.packet_sha256 != packet
        || r.report.claimed_delivery_sha != r.head_sha
        || r.report.context.package_id != r.package
        || r.report.report_type
            != if role == Role::Develop {
                ReportType::Development
            } else {
                ReportType::Review
            }
    {
        return Err("SELF2_RESULT_BINDING_REJECTED");
    }
    r.report.validate()?;
    Ok((id, r.report))
}
fn accept_saved(
    s: &Store,
    e: &Execution,
    role: Role,
    expected: Option<&str>,
    p: &Plan,
    packet: &str,
) -> Result<(String, Report)> {
    let result = accept(e, role, expected, p, packet);
    if result.is_err() {
        let (rev, mut st) = h::load(s)?;
        let r = st
            .self2
            .as_mut()
            .ok_or("SELF2_UNINITIALIZED")?
            .records
            .last_mut()
            .ok_or("SELF2_RECEIPT_MISSING")?;
        r.outcome = "STOPPED".into();
        r.attempt.state = "STOPPED".into();
        r.attempt.terminal = Some("SELF2_RESULT_BINDING_REJECTED".into());
        h::save(s, rev, &st)?;
    }
    result
}
fn bind_receipt(s: &Store, role: Role, id: &str) -> Result<()> {
    let (rev, mut st) = h::load(s)?;
    let other = if role == Role::Develop {
        &st.stage.reviewer
    } else {
        &st.stage.developer
    };
    if other.as_ref().is_some_and(|v| v.uuid == id) {
        return Err("SELF_ROLE_COLLISION");
    }
    let slot = if role == Role::Develop {
        &mut st.stage.developer
    } else {
        &mut st.stage.reviewer
    };
    if slot.as_ref().is_some_and(|v| v.uuid != id) {
        return Err("SELF2_UUID_MISMATCH");
    }
    if slot.is_none() {
        *slot = Some(SessionBinding {
            uuid: id.into(),
            source: if st
                .self2
                .as_ref()
                .is_some_and(|j| matches!(j.plan.mode, Mode::Synthetic))
            {
                "SYNTHETIC_CLI_RECEIPT"
            } else {
                "TOOL_CLI_RECEIPT"
            }
            .into(),
            reference: "SELF2 owned process receipt, not desktop identity".into(),
        });
    }
    h::save(s, rev, &st)
}
// Live adapter wiring is compiled/offline tested but not admitted by this package.
struct NativeCodex {
    root: PathBuf,
    tools: BTreeMap<PathBuf, String>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl sealed::Sealed for NativeCodex {}
impl Driver for NativeCodex {
    fn invoke(
        &mut self,
        s: &Spec,
        b: &[u8],
        p: &LaunchPermit,
        end: Instant,
        r: &mut dyn FnMut(u32, String) -> Result<()>,
    ) -> Execution {
        if s.cwd != self.root
            || self.tools.get(&s.exe) != Some(&s.exe_sha256)
            || p.attempt().argv_sha256 != s.argv_sha256()
            || p.attempt().env_names_sha256 != s.env_names_sha256()
        {
            return Execution::failed("BINDING_MISMATCH");
        }
        match cli_adapter::NativeDriver::self2_transport(&s.exe, &s.exe_sha256) {
            Ok(mut driver) => {
                driver.cancel = self.cancel.clone();
                driver.execute(s, b, p, end, r)
            }
            Err(_) => Execution::failed("BINDING_MISMATCH"),
        }
    }
}
/// Real admission is intentionally fail-closed before creating a process or reading authentication.
/// Future authorized native sandbox/UUID probes must replace the single admission gate, not weaken arguments.
pub fn run_native(s: &Store) -> Result<Value> {
    run_native_with_cancel(
        s,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    )
}
pub fn run_native_with_cancel(
    s: &Store,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> Result<Value> {
    live_admission()?;
    let p = journal(s)?.plan;
    let stage = h::load(s)?.1.stage;
    if [&stage.developer, &stage.reviewer].iter().any(|b| {
        b.as_ref()
            .is_some_and(|v| v.source == "SYNTHETIC_CLI_RECEIPT")
    }) {
        return Err("SELF2_SYNTHETIC_IDENTITY_NOT_ADMISSION");
    }
    if !matches!(p.mode, Mode::RealPendingAdmission) {
        return Err("SELF2_MODE_REJECTED");
    }
    let mut tools = BTreeMap::from([(p.cli.clone(), p.cli_sha256.clone())]);
    for c in &p.commands {
        if let Some(old) = tools.insert(c.exe.clone(), c.sha256.clone()) {
            if old != c.sha256 {
                return Err("SELF2_TOOL_IDENTITY_CONFLICT");
            }
        }
    }
    let mut d = NativeCodex {
        root: p.project_root.clone(),
        tools,
        cancel,
    };
    run(s, &mut d, &p.cli, &p.cli_sha256, false)
}
pub fn run_fake(s: &Store, d: &mut FakeCli) -> Result<Value> {
    if !matches!(journal(s)?.plan.mode, Mode::Synthetic) {
        return Err("SELF2_MODE_REJECTED");
    }
    let exe = d.exe.clone();
    let sha = d.sha.clone();
    run(s, d, &exe, &sha, true)
}
fn run(s: &Store, d: &mut impl Driver, exe: &Path, sha: &str, synthetic: bool) -> Result<Value> {
    let j = journal(s)?;
    if j.records.iter().any(|r| r.outcome != "COMPLETE") {
        return Err("SELF2_RECONCILE_REQUIRED");
    }
    let p = j.plan;
    let root = h::store_path(&p.project_root)?;
    let dir = root.join("backups/self2");
    // Hold each existing/created parent against rename; do not follow a junction while creating children.
    let mut pins = vec![Guard::open(&root, true)?];
    for child in [root.join("backups"), dir.clone(), dir.join("tmp")] {
        match fs::symlink_metadata(&child) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&child).map_err(|_| "EVIDENCE_WRITE")?
            }
            Err(_) => return Err("PATH_REJECTED"),
        };
        pins.push(Guard::open(&child, true)?);
    }
    let guard = Guard::open(&dir, true)?;
    let schema_path = dir.join("output.schema.json");
    let schema = bytes(&output_schema())?;
    if schema_path.exists() {
        if super::store::read_file(&schema_path, 65536)?.0 != schema {
            return Err("SELF2_SCHEMA_DRIFT");
        }
    } else {
        save_new(&schema_path, &schema)?;
    }
    let parent: BTreeMap<String, String> = cli_adapter::ENV_ALLOW
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.to_string(), v)))
        .collect();
    let make = |role: Role, session: Option<&str>| -> Result<Spec> {
        Ok(Spec {
            role: role.relay(),
            profile: role.profile(),
            exe: exe.into(),
            exe_sha256: sha.into(),
            argv: {
                let mut a = codex_args(&p, role, session, &schema_path)?;
                if synthetic {
                    a.insert(0, "--nova-self2-synthetic-only".into());
                }
                a
            },
            env: cli_adapter::environment(&parent, &dir)?,
            cwd: p.project_root.clone(),
            session: session.unwrap_or("").into(),
        })
    };
    let st = h::load(s)?.1;
    let dev = st.stage.developer.as_ref().map(|b| b.uuid.clone());
    if st.active.is_none() {
        let (material, _) = h::material_for(&p.project_root, &p.input, false)?;
        let input = bytes(
            &json!({"schema":"nova.self2.input/1","role":"DEVELOP","stage":p.input.stage,"package":p.input.package,"head_sha":p.input.head_sha,"packet_sha256":"","material":material,"instruction":"Only implement the approved package. No commit/push, outside-scope edits, delegated model/API or workflow actions. Fixed verification is independently supervised.","result_contract":output_schema()}),
        )?;
        let e = invoke(
            s,
            d,
            Role::Develop,
            0,
            make(Role::Develop, dev.as_deref())?,
            input,
            dev.clone(),
            &dir,
        )?;
        let (id, report) = accept_saved(s, &e, Role::Develop, dev.as_deref(), &p, "")?;
        if report.claimed_conclusion != Conclusion::Pass {
            return Err("SELF2_DEVELOPMENT_NOT_SUCCESSFUL");
        }
        bind_receipt(s, Role::Develop, &id)?;
        let mut delivery = p.input.clone();
        delivery.developer_report = report;
        for (i, c) in p.commands.iter().enumerate() {
            let spec = Spec {
                role: RelayRole::Develop,
                profile: Role::Verify.profile(),
                exe: c.exe.clone(),
                exe_sha256: c.sha256.clone(),
                argv: c.args.clone(),
                env: cli_adapter::environment(&parent, &dir)?,
                cwd: p.project_root.clone(),
                session: String::new(),
            };
            invoke(s, d, Role::Verify, i, spec, vec![], None, &dir)?;
            let r = journal(s)?
                .records
                .last()
                .cloned()
                .ok_or("SELF2_RECEIPT_MISSING")?;
            let mut a = vec![c.exe.to_string_lossy().into_owned()];
            a.extend(c.args.clone());
            delivery.logs.push(h::LogInput {
                command: a,
                delivery_sha: p.input.head_sha.clone(),
                stdout: r.stdout_path.ok_or("SELF2_RECEIPT_MISSING")?,
                stdout_sha256: r.attempt.stdout_sha256.ok_or("SELF2_RECEIPT_MISSING")?,
                stderr: r.stderr_path.ok_or("SELF2_RECEIPT_MISSING")?,
                stderr_sha256: r.attempt.stderr_sha256.ok_or("SELF2_RECEIPT_MISSING")?,
                declared_exit: r.attempt.exit_code.map(|e| e as i32),
            });
        }
        h::collect(s, delivery)?;
    }
    let st = h::load(s)?.1;
    let active = st.active.as_ref().ok_or("SELF_NO_PACKAGE")?;
    if active.review.is_some() {
        return status(s);
    }
    h::verify_packet(s, active)?;
    if !active.packet.missing.is_empty() {
        return Err("SELF_REQUIRED_FACTS_MISSING");
    }
    let reviewer = st.stage.reviewer.as_ref().map(|b| b.uuid.clone());
    let input = bytes(
        &json!({"schema":"nova.self2.input/1","role":"REVIEW","stage":p.input.stage,"package":p.input.package,"head_sha":p.input.head_sha,"packet_sha256":active.packet_sha256,"material":active.packet,"instruction":"Independent read-only review of complete packet/source/diff/logs; no writes, tests, external models or remediation. Process exits are tool facts; your conclusion is a model claim.","result_contract":output_schema()}),
    )?;
    let e = invoke(
        s,
        d,
        Role::Review,
        0,
        make(Role::Review, reviewer.as_deref())?,
        input,
        reviewer.clone(),
        &dir,
    )?;
    let (id, report) = accept_saved(
        s,
        &e,
        Role::Review,
        reviewer.as_deref(),
        &p,
        &active.packet_sha256,
    )?;
    bind_receipt(s, Role::Review, &id)?;
    let route = if let Some(id) = &active.route_id {
        id.clone()
    } else {
        h::route(s, &p.input.stage, &p.input.package)?["route_id"]
            .as_str()
            .ok_or("SELF2_ROUTE_MISSING")?
            .into()
    };
    h::review(
        s,
        ReviewInput {
            stage: p.input.stage.clone(),
            package: p.input.package.clone(),
            head_sha: p.input.head_sha.clone(),
            packet_sha256: active.packet_sha256.clone(),
            route_id: route,
            reviewer_uuid: id,
            report,
        },
    )?;
    guard.verify()?;
    for pin in &pins {
        pin.verify()?;
    }
    status(s)
}
pub fn status(s: &Store) -> Result<Value> {
    let j = journal(s)?;
    Ok(
        json!({"schema":"nova.self2.summary/1","self1":h::summary(s)?,"journal":j,"mode":j.plan.mode,"real_codex_started":matches!(j.plan.mode,Mode::RealPendingAdmission)&&!j.records.is_empty(),"native_permission_admission":"NOT RUN","subscription_authorization":"NOT GRANTED","process_liveness":"UNKNOWN_AFTER_RETURN","command_exit_source":"SUPERVISED_PROCESS_RECEIPT; SYNTHETIC DRIVER IF TEST","model_claim_source":"SEPARATE_MODEL_DECLARATION","user_acceptance":"PENDING"}),
    )
}
/// Build the synthetic fixture's JSONL without reading any real account/session content.
/// It returns a labelled synthetic claim and never edits project files.
pub fn fixture_wire(input: &[u8], id: &str) -> Result<Vec<u8>> {
    let v: Value = relay_json(input, INPUT_CAP)?;
    if !uuid_ok(id) {
        return Err("SELF2_UUID_MISMATCH");
    }
    let review = v["role"] == "REVIEW";
    let mut r: Report = serde_json::from_value(v["material"]["input"]["developer_report"].clone())
        .map_err(|_| "SELF2_FIXTURE_INPUT")?;
    r.report_type = if review {
        ReportType::Review
    } else {
        ReportType::Development
    };
    r.claims.role = r.report_type.role().into();
    r.claims.author = Some("SYNTHETIC SELF2 FAKE CLI; NOT A MODEL RESULT".into());
    r.claimed_conclusion = Conclusion::Pass;
    let reply = json!({"schema":"nova.self2.result/1","stage":v["stage"],"package":v["package"],"head_sha":v["head_sha"],"packet_sha256":v["packet_sha256"],"report":serde_json::to_string(&r).map_err(|_|"ENCODING")?});
    let mut b = vec![];
    for e in [
        json!({"type":"thread.started","thread_id":id}),
        json!({"type":"turn.started"}),
        json!({"type":"item.completed","item":{"type":"agent_message","text":serde_json::to_string(&reply).map_err(|_|"ENCODING")?}}),
        json!({"type":"turn.completed","usage":{"synthetic":true}}),
    ] {
        b.extend(bytes(&e)?);
        b.push(b'\n');
    }
    Ok(b)
}
#[cfg(test)]
mod tests {
    use super::*;
    use h::tests::{report, F};
    struct Fake {
        calls: Vec<Spec>,
        case: &'static str,
        uuid: String,
    }
    impl sealed::Sealed for Fake {}
    impl Driver for Fake {
        fn invoke(
            &mut self,
            s: &Spec,
            b: &[u8],
            p: &LaunchPermit,
            _: Instant,
            r: &mut dyn FnMut(u32, String) -> Result<()>,
        ) -> Execution {
            assert!(p.claim_once());
            assert!(!p.claim_once());
            self.calls.push(s.clone());
            if self.case == "crash" {
                panic!("SYNTHETIC host crash after durable reservation");
            }
            r(123, "SYNTHETIC_PROCESS_TIME".into()).unwrap();
            let mut e = Execution::failed("SUCCESS");
            e.natural = true;
            e.created = true;
            e.exit_code = Some(0);
            e.reclaim = "CONFIRMED".into();
            if self.case == "cancel" {
                e.natural = false;
                e.exit_code = None;
                e.terminal = "CANCELLED".into();
                return e;
            }
            if self.case == "fail"
                || (self.case == "verify-fail" && s.profile == Role::Verify.profile())
            {
                e.exit_code = Some(1);
                e.terminal = "RESULT_ERROR".into();
                return e;
            }
            if s.profile == Role::Verify.profile() {
                e.stdout = b"SYNTHETIC fixed command exit 0\n".to_vec();
                return e;
            }
            let id = if self.case == "wrong-uuid"
                || (self.case == "wrong-review" && s.role == RelayRole::Review)
            {
                self.uuid.clone()
            } else if s.session.is_empty() {
                uuid::Uuid::new_v4().to_string()
            } else {
                s.session.clone()
            };
            e.stdout = fixture_wire(b, &id).unwrap();
            if self.case == "wrong-sha" {
                let text = String::from_utf8(e.stdout)
                    .unwrap()
                    .replace(&"a".repeat(40), &"b".repeat(40));
                e.stdout = text.into_bytes();
            }
            e
        }
    }
    fn fixture() -> (F, Store, Plan) {
        let f = F::new();
        let mut input = f.input.clone();
        input.logs.clear();
        input.developer_report.claimed_conclusion = Conclusion::NotRun;
        input.required_commands = vec![vec!["C:/synthetic/fixed.exe".into(), "--fixture".into()]];
        let s = h::init_work(
            &h::store_path(&f.repo).unwrap(),
            &f.repo,
            &f.logs,
            f.stage.clone(),
            input.clone(),
        )
        .unwrap();
        let p = Plan {
            mode: Mode::Synthetic,
            schema: "nova.self2.plan/1".into(),
            input,
            project_root: f.repo.clone(),
            cli: PathBuf::from("C:/synthetic/codex.exe"),
            cli_sha256: CLI_SHA.into(),
            cli_version: CLI_VERSION.into(),
            model: "EXPLICIT_SYNTHETIC_MODEL".into(),
            commands: vec![FixedCommand {
                exe: PathBuf::from("C:/synthetic/fixed.exe"),
                sha256: "0".repeat(64),
                args: vec!["--fixture".into()],
            }],
        };
        attach(&s, p.clone()).unwrap();
        (f, s, p)
    }
    fn fake(case: &'static str) -> Fake {
        Fake {
            calls: vec![],
            case,
            uuid: uuid::Uuid::new_v4().to_string(),
        }
    }
    fn run_memory(s: &Store, d: &mut Fake) -> Result<Value> {
        run(s, d, Path::new("C:/synthetic/codex.exe"), CLI_SHA, true)
    }
    #[test]
    fn self2_normal_relay_shared_packet_and_summary() {
        let (f, s, _) = fixture();
        let mut d = fake("ok");
        let v = run_memory(&s, &mut d).unwrap();
        assert_eq!(d.calls.len(), 3);
        let receipts = journal(&s).unwrap().records;
        assert!(receipts.iter().all(|r| r.attempt.state == "EXITED"));
        assert_eq!(
            receipts[2].attempt.parent_attempt_id.as_deref(),
            Some(receipts[0].attempt.attempt_id.as_str())
        );
        assert_eq!(v["self1"]["active"]["status"], "AWAITING_PM_AND_USER");
        assert_eq!(v["real_codex_started"], false);
        h::export(&s, &f.dir.join("self2-export")).unwrap();
        assert!(f.dir.join("self2-export/REVIEW.md").exists());
    }
    #[test]
    fn self2_failure_does_not_route_review_or_replay() {
        let (_, s, _) = fixture();
        let mut d = fake("fail");
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(d.calls.len(), 1);
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(d.calls.len(), 1);
        assert!(h::load(&s).unwrap().1.active.is_none());
    }
    #[test]
    fn self2_cancel_unknown_reopen_never_resends() {
        let (f, s, _) = fixture();
        let mut d = fake("cancel");
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(journal(&s).unwrap().records[0].outcome, "UNKNOWN");
        drop(s);
        let s = Store::self_open(&h::store_path(&f.repo).unwrap()).unwrap();
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(d.calls.len(), 1);
    }
    #[test]
    fn self2_exact_review_uuid_and_readonly_parameters() {
        let (f, s, p) = fixture();
        let mut d = fake("ok");
        run_memory(&s, &mut d).unwrap();
        let a = &d.calls[2].argv;
        let expected = &f.stage.reviewer.unwrap().uuid;
        assert!(a.windows(2).any(|w| w == ["resume", expected]));
        assert!(a.windows(2).any(|w| w == ["--sandbox", "read-only"]));
        assert!(!a.iter().any(|a| a == "--last"
            || a == "--all"
            || a == "--dangerously-bypass-approvals-and-sandbox"));
        assert!(codex_args(
            &p,
            Role::Review,
            Some("desktop-window-1"),
            Path::new("C:/fixed/schema.json")
        )
        .is_err());
    }
    #[test]
    fn self2_wrong_uuid_stops_and_reopen_no_replay() {
        let (_, s, _) = fixture();
        let mut d = fake("wrong-uuid");
        assert_eq!(
            run_memory(&s, &mut d).unwrap_err(),
            "SELF2_EXECUTION_STOPPED_OR_UNKNOWN"
        );
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(d.calls.len(), 1);
    }
    #[test]
    fn self2_duplicate_completed_run_does_not_launch() {
        let (_, s, _) = fixture();
        let mut d = fake("ok");
        run_memory(&s, &mut d).unwrap();
        run_memory(&s, &mut d).unwrap();
        assert_eq!(d.calls.len(), 3);
    }
    #[test]
    fn self2_wrong_stage_sha_and_packet_rejected() {
        let (_, s, p) = fixture();
        let r = report(
            &p.input.package,
            &p.input.head_sha,
            ReportType::Development,
            Conclusion::Pass,
        );
        let mut e = Execution::failed("SUCCESS");
        let mut payload = json!({"role":"DEVELOP","stage":"other","package":p.input.package,"head_sha":p.input.head_sha,"packet_sha256":"","material":{"input":{"developer_report":r}}});
        e.stdout =
            fixture_wire(&bytes(&payload).unwrap(), &uuid::Uuid::new_v4().to_string()).unwrap();
        assert_eq!(
            accept(&e, Role::Develop, None, &p, "").unwrap_err(),
            "SELF2_RESULT_BINDING_REJECTED"
        );
        payload["stage"] = json!(p.input.stage);
        payload["head_sha"] = json!("a".repeat(40));
        e.stdout =
            fixture_wire(&bytes(&payload).unwrap(), &uuid::Uuid::new_v4().to_string()).unwrap();
        assert!(accept(&e, Role::Develop, None, &p, "").is_err());
        assert!(attach(&s, p).is_err());
    }
    #[test]
    fn self2_protocol_failures_and_forbidden_tools() {
        let (_, _, p) = fixture();
        let payload = json!({"role":"DEVELOP","stage":p.input.stage,"package":p.input.package,"head_sha":p.input.head_sha,"packet_sha256":"","material":{"input":p.input}});
        let wire =
            fixture_wire(&bytes(&payload).unwrap(), &uuid::Uuid::new_v4().to_string()).unwrap();
        assert!(parse_wire(&wire[..wire.len() - 1], Role::Develop, None).is_err());
        for event in [
            "file_change",
            "mcp_tool_call",
            "web_search",
            "collab_tool_call",
        ] {
            let mut b = wire.clone();
            let marker = b"{\"type\":\"turn.completed\"";
            let text = String::from_utf8(b.clone()).unwrap();
            let lines: Vec<_> = text.lines().collect();
            b.clear();
            for line in &lines[..3] {
                b.extend(line.as_bytes());
                b.push(b'\n');
            }
            b.extend(bytes(&json!({"type":"item.completed","item":{"type":event}})).unwrap());
            b.push(b'\n');
            b.extend(lines[3].as_bytes());
            b.push(b'\n');
            assert!(parse_wire(&b, Role::Review, None).is_err());
            let _ = marker;
        }
    }
    #[test]
    fn self2_scope_drift_before_review_is_rejected() {
        let (f, s, _) = fixture();
        fs::write(f.repo.join("outside.txt"), b"unapproved").unwrap();
        let mut d = fake("ok");
        assert!(run_memory(&s, &mut d).is_err());
        assert!(d.calls.is_empty());
    }
    #[test]
    fn self2_live_admission_closed_and_fake_path_rejects_real_codex() {
        assert_eq!(
            live_admission().unwrap_err(),
            "SELF2_NATIVE_PERMISSION_AND_UUID_ADMISSION_REQUIRED"
        );
        assert!(FakeCli::new(Path::new("C:/real/codex.exe"), CLI_SHA).is_err());
        let (_,s,_)=fixture();let before=h::load(&s).unwrap().0;
        let scope=super::super::admission_probe::permission_scope_1119e();
        assert_eq!(admission_for(&s,&scope,None),Err("SELF2_PERMISSION_SCOPE_MISMATCH"));
        assert!(run_native(&s).is_err());
        assert_eq!(h::load(&s).unwrap().0,before);assert!(journal(&s).unwrap().records.is_empty());

    }
    #[test]
    fn self2_unbound_uuid_captured_only_from_receipt() {
        let (_, s, _) = fixture();
        let (rev, mut st) = h::load(&s).unwrap();
        st.stage.developer = None;
        st.stage.reviewer = None;
        h::save(&s, rev, &st).unwrap();
        let mut d = fake("ok");
        run_memory(&s, &mut d).unwrap();
        let stage = h::load(&s).unwrap().1.stage;
        assert_ne!(
            stage.developer.as_ref().unwrap().uuid,
            stage.reviewer.as_ref().unwrap().uuid
        );
        assert_eq!(stage.reviewer.unwrap().source, "SYNTHETIC_CLI_RECEIPT");
    }
    #[test]
    fn self2_capacity_and_plan_tamper_fail_closed() {
        let (_, s, _) = fixture();
        let (rev, mut st) = h::load(&s).unwrap();
        st.self2.as_mut().unwrap().plan.model = "changed".into();
        h::save(&s, rev, &st).unwrap();
        assert_eq!(journal(&s).unwrap_err(), "SELF2_JOURNAL_REJECTED");
        assert!(parse_wire(&vec![b'x'; 8 * 1024 * 1024 + 1], Role::Review, None).is_err());
    }
    #[test]
    fn self2_fixed_command_failure_overrides_model_pass() {
        let (_, s, _) = fixture();
        let mut d = fake("verify-fail");
        assert_eq!(
            run_memory(&s, &mut d).unwrap_err(),
            "SELF2_EXECUTION_STOPPED_OR_UNKNOWN"
        );
        assert_eq!(d.calls.len(), 2);
        assert!(d.calls.iter().all(|c| c.role != RelayRole::Review));
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(d.calls.len(), 2);
    }
    #[test]
    fn self2_started_crash_and_stale_cas_never_replay() {
        let (f, s, _) = fixture();
        let (rev, st) = h::load(&s).unwrap();
        let mut d = fake("crash");
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_memory(&s, &mut d)))
                .is_err()
        );
        assert_eq!(journal(&s).unwrap().records[0].outcome, "STARTED");
        assert_eq!(h::save(&s, rev, &st).unwrap_err(), "CAS_CONFLICT");
        drop(s);
        let s = Store::self_open(&h::store_path(&f.repo).unwrap()).unwrap();
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(d.calls.len(), 1);
    }
    #[test]
    fn self2_review_uuid_conflict_stops_without_new_session() {
        let (_, s, _) = fixture();
        let mut d = fake("wrong-review");
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(d.calls.len(), 3);
        assert_eq!(journal(&s).unwrap().records[2].outcome, "UNKNOWN");
        assert!(run_memory(&s, &mut d).is_err());
        assert_eq!(d.calls.len(), 3);
        assert!(h::load(&s).unwrap().1.active.unwrap().review.is_none());
    }
}
