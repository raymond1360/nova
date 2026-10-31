//! Injectable orchestration. Native Job methods are compiled, never used by mock tests.
use super::opencode_paths::{
    AgentTreeReceipt, FixtureReservation, GitPreflight, ProcessIdentity, TreeSnapshot,
};
pub use super::opencode_paths::{JobStopReceipt, StopError};
use super::{
    opencode_gate::{RunState, State},
    Result,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
/// Fixed policy gate only. Missing assets are not manufactured from expected hashes.
pub fn resource_admission(text: &str) -> Result<()> {
    let p: serde_json::Value = serde_json::from_str(text).map_err(|_| "resource policy JSON")?;
    if p["schema"] != "nova-m0b-resource-policy-v1"
        || p["prepared"] != true
        || p["execution_authorized"] != true
        || p["root"] != super::opencode_paths::ROOT
        || p["probe_sid"].as_str().is_none()
        || p["facility_receipt"].is_null()
    {
        return Err("unprepared/unapproved resource or facility receipt".into());
    }
    for name in [
        "opencode",
        "rg",
        "git",
        "git_children",
        "plugin_closure",
        "fixture",
        "node",
    ] {
        if p["resources"][name].is_null() {
            return Err(format!("resource not prepared: {name}"));
        }
    }
    Ok(())
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preflight {
    pub resources_prepared: bool,
    pub parent_policy_flags: Option<u32>,
    pub standard_probe_account: bool,
    pub capture_exclusive: bool,
    pub observation_complete: bool,
    pub loss: Option<u64>,
    pub fixture: FixtureReservation,
}
impl Preflight {
    pub fn validate(&self) -> Result<()> {
        if self.parent_policy_flags != Some(0)
            || !self.standard_probe_account
            || !self.resources_prepared
            || !self.capture_exclusive
            || !self.observation_complete
            || self.loss != Some(0)
        {
            return Err("resource/facility/ownership incomplete; execution BLOCKED".into());
        }
        self.fixture.validate()
    }
}
/// Injected observation receipt contract; neither Procmon nor WFP supplies a generic owned ID.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationClass {
    ProcessTree,
    FileWriteAds,
    NetworkAttempts,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationReview {
    pub exclusive_vm: bool,
    pub same_generation: bool,
    pub no_prior_capture: bool,
    pub start_receipt: bool,
    pub stop_receipt: bool,
    pub outputs_closed: bool,
    pub classes: Vec<ObservationClass>,
    pub tree: AgentTreeReceipt,
    pub pml_loss: Option<u64>,
    pub etl_loss: Option<u64>,
    pub pml_bytes: u64,
    pub etl_bytes: u64,
}
impl ObservationReview {
    pub fn validate(&self) -> Result<()> {
        if self.classes.len() != 3
            || ![
                ObservationClass::ProcessTree,
                ObservationClass::FileWriteAds,
                ObservationClass::NetworkAttempts,
            ]
            .iter()
            .all(|c| self.classes.contains(c))
            || !self.exclusive_vm
            || !self.same_generation
            || !self.no_prior_capture
            || !self.start_receipt
            || !self.stop_receipt
            || !self.outputs_closed
            || self.pml_loss != Some(0)
            || self.etl_loss != Some(0)
            || self.pml_bytes == 0
            || self.etl_bytes == 0
            || self.pml_bytes > 512 * 1024 * 1024
            || self.etl_bytes > 64 * 1024 * 1024
        {
            return Err("observation ownership/coverage/loss incomplete".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Attempt,
    Preflight,
    Capture,
    Target,
    CaptureStop,
    Export,
    Cas,
    Cleanup,
}
pub struct Timeline {
    pub phase: Phase,
    pub start: u64,
    pub cap: Option<u64>,
    pub target: Option<u64>,
    pub entered: u64,
}
impl Timeline {
    pub fn new(now: u64) -> Self {
        Self {
            phase: Phase::Attempt,
            start: now,
            cap: None,
            target: None,
            entered: now,
        }
    }
    pub fn enter(&mut self, next: Phase, now: u64) -> Result<()> {
        let elapsed = now.checked_sub(self.start).ok_or("clock reversal")?;
        if elapsed > 240000 {
            return Err("global deadline".into());
        }
        let previous_limit = match self.phase {
            Phase::Attempt | Phase::Preflight => 15000,
            Phase::Capture => 15000,
            Phase::Target => 120000,
            Phase::CaptureStop => 15000,
            Phase::Export => 15000,
            Phase::Cas => 30000,
            Phase::Cleanup => 10000,
        };
        if now.checked_sub(self.entered).ok_or("clock reversal")? > previous_limit {
            return Err("phase deadline including late API return".into());
        }
        let expected = matches!(
            (self.phase, next),
            (Phase::Attempt, Phase::Preflight)
                | (Phase::Preflight, Phase::Capture)
                | (Phase::Capture, Phase::Target)
                | (Phase::Target, Phase::CaptureStop)
                | (Phase::CaptureStop, Phase::Export)
                | (Phase::Export, Phase::Cas)
                | (Phase::Cas, Phase::Cleanup)
        );
        if !expected {
            return Err("phase sequence".into());
        }
        if self.cap.is_some_and(|c| {
            matches!(self.phase, Phase::Target | Phase::CaptureStop) && now - c > 180000
        }) {
            return Err("capture deadline".into());
        }
        let needed = match next {
            Phase::Preflight => 220000,
            Phase::Capture => 205000,
            Phase::Target => 190000,
            Phase::Cas => 40000,
            Phase::Export => 55000,
            Phase::CaptureStop => 70000,
            Phase::Cleanup => 10000,
            Phase::Attempt => return Err("attempt cannot reenter".into()),
        };
        if 240000 - elapsed < needed {
            return Err("insufficient remaining budget".into());
        }
        if next == Phase::Capture {
            self.cap = Some(now)
        }
        if next == Phase::Target {
            self.target = Some(now)
        }
        self.phase = next;
        self.entered = now;
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum JournalState {
    Prepared,
    Uncertain,
    CommittedHistorical,
    UncertainRecovery,
}
#[derive(Clone, Debug, Serialize)]
pub struct CasReview {
    pub state: JournalState,
    pub receipt: bool,
    pub independently_verified: bool,
}
pub trait Ops {
    fn now_ms(&self) -> u64;
    fn evidence(&mut self, label: &str) -> Result<()>;
    fn preflight(&mut self) -> Result<Preflight>;
    fn capture_start(&mut self) -> Result<()>;
    fn git_preflight(&mut self) -> Result<GitPreflight>;
    fn before_tree(&mut self) -> Result<TreeSnapshot>;
    fn start_and_verify(&mut self) -> Result<ProcessIdentity>;
    fn task(&mut self, state: &Arc<RunState>) -> Result<super::contract::Patch>;
    fn reject(&mut self, body: &[u8]) -> Result<()>;
    fn idle(&mut self) -> Result<bool>;
    fn abort(&mut self) -> Result<()>;
    fn stop_job(&mut self) -> Result<JobStopReceipt>;
    fn capture_stop(&mut self) -> Result<()>;
    fn export_and_review(&mut self) -> Result<ObservationReview>;
    fn cas_and_verify(&mut self, patch: &super::contract::Patch) -> Result<CasReview>;
    fn cleanup(&mut self) -> Result<()>;
}
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub source: String,
    pub success: bool,
    pub cas_started: bool,
    pub journal: Option<CasReview>,
    pub job_active: Option<u32>,
    pub job_stop: Option<JobStopReceipt>,
    pub target: Option<ProcessIdentity>,
    pub error: Option<String>,
    pub cleanup_errors: Vec<String>,
    pub phases: Vec<String>,
    pub execution_authorized: bool,
}
fn within(start: u64, end: u64, limit: u64) -> Result<()> {
    if end.checked_sub(start).is_some_and(|n| n <= limit) {
        Ok(())
    } else {
        Err("substage deadline/late API/clock reversal".into())
    }
}
pub fn run<O: Ops>(ops: &mut O, state: Arc<RunState>, source: &str) -> Outcome {
    let mut out = Outcome {
        source: source.into(),
        success: false,
        cas_started: false,
        journal: None,
        job_active: None,
        job_stop: None,
        target: None,
        error: None,
        cleanup_errors: vec![],
        phases: vec![],
        execution_authorized: false,
    };
    let mut clock = Timeline::new(ops.now_ms());
    let mut capture = false;
    let mut job_touched = false;
    let mut task_touched = false;
    if let Err(e) = ops.evidence("attempt-create-new") {
        state.stop();
        out.error = Some(e);
        return out;
    }
    let result = (|| -> Result<()> {
        clock.enter(Phase::Preflight, ops.now_ms())?;
        let preflight = ops.preflight()?;
        preflight.validate()?;
        clock.enter(Phase::Capture, ops.now_ms())?;
        ops.capture_start()?;
        capture = true;
        ops.evidence("capture-started")?;
        clock.enter(Phase::Target, ops.now_ms())?;
        job_touched = true;
        let git = ops.git_preflight()?;
        super::opencode_paths::git_preflight(
            &git,
            &preflight.fixture.sid,
            &preflight.fixture.git_images,
            &preflight.fixture.root,
        )?;
        let before = ops.before_tree()?;
        super::opencode_paths::validate_snapshot(&before, &preflight.fixture)?;
        // Target reserves 90s for OpenCode/start/task/idle/Job; Git+snapshot may use at most 30s.
        within(clock.entered, ops.now_ms(), 30000)?;
        let startup_start = ops.now_ms();
        let target = ops.start_and_verify()?;
        target.validate()?;
        out.target = Some(target.clone());
        within(startup_start, ops.now_ms(), 20000)?;
        state.open_once(true)?;
        state.prompt_once(Instant::now() + Duration::from_secs(1))?;
        task_touched = true;
        let task_start = ops.now_ms();
        let patch = ops.task(&state)?;
        within(task_start, ops.now_ms(), 60000)?;
        patch.validate("M0B", &patch.attempt_id, super::PRE)?;
        state.proposal_ready(Instant::now() + Duration::from_secs(1))?;
        state.stop();
        ops.evidence("proposal-stopped")?;
        let idle_start = ops.now_ms();
        ops.reject(br#"{"reply":"reject"}"#)?;
        within(idle_start, ops.now_ms(), 2000)?;
        if !ops.idle()? {
            return Err("idle unconfirmed".into());
        }
        within(idle_start, ops.now_ms(), 5000)?;
        let job_start = ops.now_ms();
        let receipt = ops.stop_job()?;
        out.job_stop = Some(receipt.clone());
        within(job_start, ops.now_ms(), 5000)?;
        receipt.validate()?;
        out.job_active = receipt.active;
        clock.enter(Phase::CaptureStop, ops.now_ms())?;
        capture = false;
        ops.capture_stop()?;
        clock.enter(Phase::Export, ops.now_ms())?;
        let observation = ops.export_and_review()?;
        observation.validate()?;
        super::opencode_paths::review_agent_tree(
            &before,
            &observation.tree,
            &preflight.fixture,
            &target,
        )?;
        ops.evidence("observation-reviewed")?;
        clock.enter(Phase::Cas, ops.now_ms())?;
        if state.state() != State::Stopped {
            return Err("CAS while admission open".into());
        }
        out.cas_started = true;
        out.journal = Some(ops.cas_and_verify(&patch)?);
        let review = out.journal.as_ref().unwrap();
        if review.state != JournalState::CommittedHistorical
            || !review.receipt
            || !review.independently_verified
        {
            return Err("CAS/journal/independent verification incomplete; no recovery".into());
        }
        clock.enter(Phase::Cleanup, ops.now_ms())?;
        ops.cleanup()?;
        ops.evidence("complete")?;
        if ops
            .now_ms()
            .checked_sub(clock.entered)
            .ok_or("clock reversal")?
            > 10000
            || ops
                .now_ms()
                .checked_sub(clock.start)
                .ok_or("clock reversal")?
                > 240000
        {
            return Err("cleanup late return; PARTIAL".into());
        }
        Ok(())
    })();
    state.stop();
    if let Err(e) = result {
        out.error = Some(e);
        let _ = ops.evidence(if out.cas_started {
            "CAS-failure-preserve-journal-no-restore"
        } else {
            "stopped-before-CAS"
        });
        if task_touched {
            let t = ops.now_ms();
            if let Err(e) = ops.abort().and_then(|_| within(t, ops.now_ms(), 2000)) {
                out.cleanup_errors.push(e);
            }
        }
        if job_touched {
            let t = ops.now_ms();
            match ops.stop_job().and_then(|n| {
                within(t, ops.now_ms(), 5000)?;
                Ok(n)
            }) {
                Ok(receipt) => {
                    out.job_stop = Some(receipt.clone());
                    match receipt.validate() {
                        Ok(()) => out.job_active = receipt.active,
                        Err(e) => {
                            out.job_active = None;
                            out.cleanup_errors.push(e);
                        }
                    }
                }
                Err(e) => {
                    out.job_active = None;
                    out.cleanup_errors.push(e);
                }
            }
        }
        if capture {
            if let Err(e) = ops.capture_stop() {
                out.cleanup_errors.push(e);
            }
        }
        let t = ops.now_ms();
        if let Err(e) = ops.cleanup().and_then(|_| within(t, ops.now_ms(), 10000)) {
            out.cleanup_errors.push(e);
        }
    } else {
        out.success = true
    }
    out.phases.push(format!("{:?}", clock.phase));
    out
}

// These fixtures are explicitly INJECTED, never prepared resource identities.
pub(super) fn injected_target() -> ProcessIdentity {
    ProcessIdentity {
        pid: 17,
        created: 1,
    }
}
pub(super) fn injected_job() -> JobStopReceipt {
    JobStopReceipt {
        flags: Some(0x2000),
        active: Some(0),
        direct_exited: Some(true),
        error: None,
    }
}
pub(super) fn injected_preflight() -> Preflight {
    Preflight {
        resources_prepared: true,
        parent_policy_flags: Some(0),
        standard_probe_account: true,
        capture_exclusive: true,
        observation_complete: true,
        loss: Some(0),
        fixture: FixtureReservation {
            root: super::opencode_paths::RootReservation {
                path: super::opencode_paths::ROOT.into(),
                identity: "INJECTED:root".into(),
            },
            logo_identity: "INJECTED:Logo.tsx".into(),
            commit: "a".repeat(40),
            sid: "probe-sid".into(),
            git_images: vec![("git.exe".into(), super::hash(b"INJECTED Git image"))],
            git_config_sha256: super::hash(super::opencode_paths::FROZEN_GIT_CONFIG),
        },
    }
}
pub(super) fn injected_tree() -> TreeSnapshot {
    use super::opencode_paths::{FileFact, FROZEN_GIT_CONFIG};
    let files = [
        (".", vec![], 0x10),
        (".git", vec![], 0x10),
        ("Logo.tsx", super::PRE.to_vec(), 0x20),
        (".git/config", FROZEN_GIT_CONFIG.to_vec(), 0x20),
        (".git/opencode", vec![b'a'; 40], 0x20),
    ]
    .into_iter()
    .map(|(path, bytes, attributes)| FileFact {
        path: path.into(),
        bytes,
        identity: if path == "." {
            "INJECTED:root".into()
        } else {
            format!("INJECTED:{path}")
        },
        owner: "probe-sid".into(),
        dacl: "INJECTED_DACL".into(),
        attributes,
        ads: vec![],
        links: 1,
        creation: 1,
        write: 1,
        access: 1,
        change: 1,
    })
    .collect();
    TreeSnapshot {
        complete: Some(true),
        files,
    }
}
pub(super) fn injected_git() -> GitPreflight {
    let p = injected_preflight();
    GitPreflight {
        attempts: 1,
        exit: Some(0),
        timed_out: false,
        job: injected_job(),
        stdout: format!("{}\r\n", p.fixture.root.path).into_bytes(),
        owner_sid: p.fixture.sid,
        manifest_complete: true,
        images: p.fixture.git_images,
        root_identity: Some(p.fixture.root.identity),
    }
}
pub(super) fn injected_observation() -> ObservationReview {
    let mut after = injected_tree();
    after
        .files
        .iter_mut()
        .find(|f| f.path == ".git/opencode")
        .unwrap()
        .write = 2;
    ObservationReview {
        exclusive_vm: true,
        same_generation: true,
        no_prior_capture: true,
        start_receipt: true,
        stop_receipt: true,
        outputs_closed: true,
        classes: vec![
            ObservationClass::ProcessTree,
            ObservationClass::FileWriteAds,
            ObservationClass::NetworkAttempts,
        ],
        tree: AgentTreeReceipt {
            after,
            descendants: vec![],
            writes: vec![super::opencode_paths::FileWriteFact {
                path: ".git/opencode".into(),
                process: injected_target(),
                kind: super::opencode_paths::WriteKind::Overwrite,
            }],
        },
        pml_loss: Some(0),
        etl_loss: Some(0),
        pml_bytes: 1,
        etl_bytes: 1,
    }
}
/// A complete in-memory run through the production orchestration, explicitly INJECTED.
/// It neither imports SYNTHETIC upstream facts nor touches processes/files/network.
pub fn mock_self_test() -> Result<serde_json::Value> {
    struct Memory {
        tick: std::cell::Cell<u64>,
        records: Vec<String>,
        attempt: bool,
        post: Option<Vec<u8>>,
    }
    impl Ops for Memory {
        fn now_ms(&self) -> u64 {
            let t = self.tick.get();
            self.tick.set(t + 1);
            t
        }
        fn evidence(&mut self, label: &str) -> Result<()> {
            if label == "attempt-create-new" {
                if self.attempt {
                    return Err("attempt exists".into());
                }
                self.attempt = true;
            }
            self.records.push(label.into());
            Ok(())
        }
        fn preflight(&mut self) -> Result<Preflight> {
            Ok(injected_preflight())
        }
        fn git_preflight(&mut self) -> Result<GitPreflight> {
            self.records.push("INJECTED Git receipt".into());
            Ok(injected_git())
        }
        fn before_tree(&mut self) -> Result<TreeSnapshot> {
            self.records.push("INJECTED tree before".into());
            Ok(injected_tree())
        }
        fn capture_start(&mut self) -> Result<()> {
            self.records.push("INJECTED capture start".into());
            Ok(())
        }
        fn start_and_verify(&mut self) -> Result<ProcessIdentity> {
            self.records
                .push("INJECTED assign-query-resume-health-path-permission".into());
            Ok(injected_target())
        }
        fn task(&mut self, state: &Arc<RunState>) -> Result<super::contract::Patch> {
            use super::{
                opencode_broker::{fixed_request, Broker},
                opencode_paths::TARGET,
                opencode_protocol::*,
            };
            let deadline = || Instant::now() + Duration::from_secs(1);
            let b = Broker::new(state.clone(), super::opencode_broker::INJECTED_NONCE).unwrap();
            b.request(
                &fixed_request(false, super::opencode_broker::INJECTED_NONCE)?,
                deadline(),
            )?;
            let read = Permission {
                id: "per_read".into(),
                session_id: "s".into(),
                message_id: "m_read".into(),
                call_id: "read".into(),
                permission: "read".into(),
                patterns: vec!["Logo.tsx".into()],
                metadata: serde_json::json!({}),
            };
            let mut part = ToolPart {
                session_id: "s".into(),
                message_id: "m_read".into(),
                call_id: "read".into(),
                tool: "read".into(),
                status: "running".into(),
                input: serde_json::json!({"filePath":TARGET}),
            };
            let mut flow = PermissionFlow::new("s");
            flow.permission(state, &read, || Ok(vec![part.clone()]))?;
            self.records
                .push("INJECTED permission read reply once".into());
            part.status = "completed".into();
            flow.completed_read(state, &part)?;
            b.request(
                &fixed_request(true, super::opencode_broker::INJECTED_NONCE)?,
                deadline(),
            )?;
            let p = Permission {
                id: "per_edit".into(),
                session_id: "s".into(),
                message_id: "m_edit".into(),
                call_id: "edit".into(),
                permission: "edit".into(),
                patterns: vec!["Logo.tsx".into()],
                metadata: serde_json::json!({"filepath":TARGET}),
            };
            let edit = ToolPart {
                session_id: "s".into(),
                message_id: "m_edit".into(),
                call_id: "edit".into(),
                tool: "edit".into(),
                status: "running".into(),
                input: serde_json::json!({"filePath":TARGET,"oldString":"alt=\"Payload Logo\"","newString":"alt=\"Northstar Studio logo\"","replaceAll":false}),
            };
            flow.permission(state, &p, || Ok(vec![edit.clone()]))?
                .ok_or("no proposal".into())
        }
        fn reject(&mut self, b: &[u8]) -> Result<()> {
            if b != br#"{"reply":"reject"}"# {
                return Err("reject body".into());
            }
            self.records.push("INJECTED reject".into());
            Ok(())
        }
        fn idle(&mut self) -> Result<bool> {
            Ok(true)
        }
        fn abort(&mut self) -> Result<()> {
            self.records.push("INJECTED abort".into());
            Ok(())
        }
        fn stop_job(&mut self) -> Result<JobStopReceipt> {
            self.records.push("INJECTED Job stop receipt".into());
            Ok(injected_job())
        }
        fn capture_stop(&mut self) -> Result<()> {
            self.records.push("INJECTED capture stopped".into());
            Ok(())
        }
        fn export_and_review(&mut self) -> Result<ObservationReview> {
            self.records.push("INJECTED observation facts".into());
            Ok(injected_observation())
        }
        fn cas_and_verify(&mut self, p: &super::contract::Patch) -> Result<CasReview> {
            self.post = Some(p.validate("M0B", &p.attempt_id, super::PRE)?);
            Ok(CasReview {
                state: JournalState::CommittedHistorical,
                receipt: true,
                independently_verified: true,
            })
        }
        fn cleanup(&mut self) -> Result<()> {
            self.records.push("INJECTED cleanup".into());
            Ok(())
        }
    }
    let mut m = Memory {
        tick: std::cell::Cell::new(0),
        records: vec![],
        attempt: false,
        post: None,
    };
    let state = RunState::new()?;
    let out = run(&mut m, state.clone(), "INJECTED");
    if !out.success || m.post.as_deref() != Some(super::oracle(super::PRE)?.as_slice()) {
        return Err(format!("mock pipeline: {:?}", out.error));
    }
    Ok(
        serde_json::json!({"outcome":out,"records":m.records,"ledger":state.snapshot(Instant::now()+Duration::from_secs(1))?,"postimage_sha256":super::hash(m.post.as_ref().unwrap()),"native_executed":false,"upstream_executed":false}),
    )
}

/// Empty-map environment, for a new isolated controller only; never set parent env.
pub fn minimal_environment() -> std::collections::BTreeMap<String, String> {
    let mut env = std::collections::BTreeMap::new();
    for (k, v) in [
        ("SystemRoot", r"C:\Windows"),
        ("WINDIR", r"C:\Windows"),
        ("PATH", r"C:\NovaM0B\run\agent\bin;C:\Windows\System32"),
        ("HOME", r"C:\NovaM0B\run\agent\home"),
        ("USERPROFILE", r"C:\NovaM0B\run\agent\home"),
        ("APPDATA", r"C:\NovaM0B\run\agent\config"),
        ("LOCALAPPDATA", r"C:\NovaM0B\run\agent\local"),
        ("ProgramData", r"C:\NovaM0B\run\agent\managed"),
        ("XDG_CONFIG_HOME", r"C:\NovaM0B\run\agent\config"),
        ("XDG_DATA_HOME", r"C:\NovaM0B\run\agent\data"),
        ("XDG_CACHE_HOME", r"C:\NovaM0B\run\agent\cache"),
        ("XDG_STATE_HOME", r"C:\NovaM0B\run\agent\state"),
        ("TEMP", r"C:\NovaM0B\run\agent\tmp"),
        ("TMP", r"C:\NovaM0B\run\agent\tmp"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", r"C:\NovaM0B\run\agent\empty.gitconfig"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("OPENCODE_DB", r"C:\NovaM0B\run\agent\state\opencode.db"),
        ("OPENCODE_MODELS_PATH", r"C:\NovaM0B\run\agent\models.json"),
        ("OPENCODE_CONFIG", r"C:\NovaM0B\run\agent\config.json"),
    ] {
        env.insert(k.into(), v.into());
    }
    for k in [
        "PURE",
        "DISABLE_DEFAULT_PLUGINS",
        "DISABLE_PROJECT_CONFIG",
        "DISABLE_CLAUDE_CODE",
        "DISABLE_CLAUDE_CODE_PROMPT",
        "DISABLE_CLAUDE_CODE_SKILLS",
        "DISABLE_EXTERNAL_SKILLS",
        "DISABLE_MODELS_FETCH",
        "DISABLE_AUTOUPDATE",
        "DISABLE_LSP_DOWNLOAD",
        "DISABLE_AUTOCOMPACT",
        "DISABLE_FFF",
    ] {
        env.insert(format!("OPENCODE_{k}"), "true".into());
    }
    env
}
/// All native calls sit behind this boundary. Tests supply no OS implementation.
pub trait ProcessOps {
    fn create_suspended(&mut self) -> Result<()>;
    fn assign(&mut self) -> Result<()>;
    fn flags_and_active(&mut self) -> Result<(u32, u32)>;
    fn resume(&mut self) -> Result<()>;
    fn terminate_suspended(&mut self) -> Result<()>;
    fn terminate_job(&mut self) -> Result<()>;
    fn exited(&mut self) -> Result<bool>;
}

#[derive(Clone, Debug, Serialize)]
pub struct SuspendedCleanupReceipt {
    pub requested: bool,
    pub direct_exited: Option<bool>,
    pub error: Option<StopError>,
}
impl SuspendedCleanupReceipt {
    pub fn confirmed(&self) -> bool {
        self.requested && self.direct_exited == Some(true) && self.error.is_none()
    }
}
#[derive(Debug)]
pub struct LaunchFailure {
    pub reason: String,
    pub resume_attempted: bool,
    pub cleanup: Option<SuspendedCleanupReceipt>,
    pub job_termination_error: Option<String>,
}
impl LaunchFailure {
    /// Direct-child cleanup only; never evidence that the Job has reached zero.
    pub fn direct_cleanup_confirmed(&self) -> bool {
        self.job_termination_error.is_none() && self.cleanup.as_ref().is_some_and(|r| r.confirmed())
    }
}
fn exit_after_request<P: ProcessOps>(
    p: &mut P,
    requested: bool,
    deadline: Instant,
) -> SuspendedCleanupReceipt {
    let mut receipt = SuspendedCleanupReceipt {
        requested,
        direct_exited: None,
        error: None,
    };
    if !requested {
        receipt.error = Some(StopError::TerminationFailed);
        return receipt;
    }
    loop {
        if Instant::now() >= deadline {
            receipt.error = Some(StopError::Deadline);
            return receipt;
        }
        match p.exited() {
            Err(_) => {
                receipt.error = Some(StopError::QueryFailed);
                return receipt;
            }
            Ok(value) => {
                receipt.direct_exited = Some(value);
                if Instant::now() >= deadline {
                    receipt.error = Some(StopError::Deadline);
                    return receipt;
                }
                if value {
                    return receipt;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
/// Shared launch skeleton: no resume after failed assignment/query, with bounded direct-exit evidence.
pub fn launch_verified<P: ProcessOps>(p: &mut P) -> std::result::Result<(), LaunchFailure> {
    launch_verified_until(p, Instant::now() + Duration::from_secs(5))
}
pub(super) fn launch_verified_until<P: ProcessOps>(
    p: &mut P,
    deadline: Instant,
) -> std::result::Result<(), LaunchFailure> {
    if let Err(reason) = p.create_suspended() {
        return Err(LaunchFailure {
            reason,
            resume_attempted: false,
            cleanup: None,
            job_termination_error: None,
        });
    }
    let verified = p.assign().and_then(|_| p.flags_and_active()).and_then(|v| {
        if v == (0x2000, 1) {
            Ok(())
        } else {
            Err("Job flags/member UNKNOWN".into())
        }
    });
    if let Err(reason) = verified {
        let requested = p.terminate_suspended().is_ok();
        let job_termination_error = p.terminate_job().err();
        let cleanup = Some(exit_after_request(p, requested, deadline));
        return Err(LaunchFailure {
            reason,
            resume_attempted: false,
            cleanup,
            job_termination_error,
        });
    }
    if let Err(reason) = p.resume() {
        let error = p.terminate_job().err();
        let cleanup = Some(exit_after_request(p, error.is_none(), deadline));
        return Err(LaunchFailure {
            reason,
            resume_attempted: true,
            cleanup,
            job_termination_error: error,
        });
    }
    Ok(())
}
pub fn confirm_stopped<P: ProcessOps>(p: &mut P, deadline: Instant) -> Result<JobStopReceipt> {
    p.terminate_job()?;
    loop {
        if Instant::now() >= deadline {
            return Err("cleanup deadline UNKNOWN".into());
        }
        let (flags, active) = p.flags_and_active()?;
        if flags != 0x2000 {
            return Err("cleanup flags UNKNOWN".into());
        }
        if active == 0 {
            let direct = p.exited()?;
            if Instant::now() >= deadline {
                return Err("cleanup query late/UNKNOWN".into());
            }
            let receipt = JobStopReceipt {
                flags: Some(flags),
                active: Some(active),
                direct_exited: Some(direct),
                error: None,
            };
            if receipt.validate().is_ok() {
                return Ok(receipt);
            }
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}
/// Evidence writes must check short writes and durable flush; no success repair.
pub trait EvidenceSink {
    fn create_new(&mut self, label: &str) -> Result<()>;
    fn write(&mut self, bytes: &[u8]) -> Result<usize>;
    fn flush(&mut self) -> Result<()>;
}
pub fn record<S: EvidenceSink>(s: &mut S, label: &str, bytes: &[u8], nonce: &[u8]) -> Result<()> {
    if label.is_empty()
        || !label
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
    {
        return Err("evidence label".into());
    }
    if !nonce.is_empty() && bytes.windows(nonce.len()).any(|w| w == nonce) {
        return Err("nonce must not enter evidence".into());
    }
    s.create_new(label)?;
    if s.write(bytes)? != bytes.len() {
        return Err("evidence short write".into());
    }
    s.flush()
}
/// Single ownership mechanism shared by the compiled native Job and pure close tests.
pub(super) struct JobHandle<H, C: FnMut(H)> {
    value: Option<H>,
    close: C,
}
impl<H, C: FnMut(H)> JobHandle<H, C> {
    pub(super) fn new(value: H, close: C) -> Self {
        Self {
            value: Some(value),
            close,
        }
    }
    fn get(&self) -> &H {
        self.value.as_ref().expect("owned Job handle")
    }
}
impl<H, C: FnMut(H)> Drop for JobHandle<H, C> {
    fn drop(&mut self) {
        if let Some(value) = self.value.take() {
            (self.close)(value);
        }
    }
}
#[cfg(windows)]
pub mod native {
    use super::Result;
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HANDLE},
        System::JobObjects::*,
    };
    pub struct ProbeJob(super::JobHandle<HANDLE, fn(HANDLE)>);
    fn close_native(h: HANDLE) {
        unsafe {
            CloseHandle(h);
        }
    }
    impl ProbeJob {
        pub fn new() -> Result<Self> {
            unsafe {
                let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if h.is_null() {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let own = Self(super::JobHandle::new(h, close_native as fn(HANDLE)));
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                if SetInformationJobObject(
                    h,
                    JobObjectExtendedLimitInformation,
                    &info as *const _ as _,
                    std::mem::size_of_val(&info) as u32,
                ) == 0
                {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                if own.query()? != 0 {
                    return Err("new Job nonempty".into());
                }
                Ok(own)
            }
        }
        pub fn query(&self) -> Result<u32> {
            unsafe {
                let mut e: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                let mut n = 0;
                if QueryInformationJobObject(
                    *self.0.get(),
                    JobObjectExtendedLimitInformation,
                    &mut e as *mut _ as _,
                    std::mem::size_of_val(&e) as u32,
                    &mut n,
                ) == 0
                {
                    return Err(format!(
                        "Job flags query UNKNOWN: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                if n as usize != std::mem::size_of_val(&e) {
                    return Err("Job flags query length UNKNOWN".into());
                }
                if e.BasicLimitInformation.LimitFlags != JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE {
                    return Err("Job flags mismatch".into());
                }
                let mut a: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
                if QueryInformationJobObject(
                    *self.0.get(),
                    JobObjectBasicAccountingInformation,
                    &mut a as *mut _ as _,
                    std::mem::size_of_val(&a) as u32,
                    &mut n,
                ) == 0
                {
                    return Err(format!(
                        "Job accounting UNKNOWN: {}",
                        std::io::Error::last_os_error()
                    ));
                }
                if n as usize != std::mem::size_of_val(&a) {
                    return Err("Job accounting length UNKNOWN".into());
                }
                Ok(a.ActiveProcesses)
            }
        }
        /// Borrowed process handle: caller retains ownership; assignment never resumes it.
        pub fn assign(&self, process: HANDLE) -> Result<()> {
            unsafe {
                if AssignProcessToJobObject(*self.0.get(), process) == 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                let mut member = 0;
                if IsProcessInJob(process, *self.0.get(), &mut member) == 0
                    || member == 0
                    || self.query()? != 1
                {
                    return Err("Job membership UNKNOWN".into());
                }
                Ok(())
            }
        }
        pub fn terminate(&self) -> Result<()> {
            unsafe {
                if TerminateJobObject(*self.0.get(), 1) == 0 {
                    Err(std::io::Error::last_os_error().to_string())
                } else {
                    Ok(())
                }
            }
        }
    }
}
