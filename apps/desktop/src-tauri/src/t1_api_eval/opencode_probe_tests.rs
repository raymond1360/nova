use super::{
    opencode_broker::Broker, opencode_gate::*, opencode_paths::*, opencode_probe::*,
    opencode_protocol::*,
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
fn deadline() -> Instant {
    Instant::now() + Duration::from_millis(100)
}
fn open() -> Arc<RunState> {
    let s = RunState::new().unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(deadline()).unwrap();
    s
}
#[test]
fn preflight_is_closed_and_stopped_terminal() {
    let s = RunState::new().unwrap();
    assert_eq!(s.state(), State::Preflight);
    assert!(s.admit("x", false, deadline()).is_err());
    assert_eq!(s.state(), State::Stopped);
    assert!(s.open_once(true).is_err());
}
#[test]
fn duplicate_open_stops() {
    let s = open();
    assert!(s.open_once(true).is_err());
    assert_eq!(s.state(), State::Stopped)
}
#[test]
fn prompt_is_once() {
    let s = open();
    assert!(s.prompt_once(deadline()).is_err());
    assert_eq!(s.state(), State::Stopped)
}
#[test]
fn stop_open_race_never_reopens() {
    for _ in 0..16 {
        let s = RunState::new().unwrap();
        let a = s.clone();
        let h = std::thread::spawn(move || {
            let _ = a.open_once(true);
        });
        s.stop();
        h.join().unwrap();
        assert_eq!(s.state(), State::Stopped);
        assert!(s.open_once(true).is_err());
    }
}
#[test]
fn hard_tickets_third_denied_counted() {
    let s = open();
    s.admit("r", false, deadline()).unwrap();
    s.settle_injected("r", Some(1), deadline()).unwrap();
    s.read_completed(deadline()).unwrap();
    s.admit("e", true, deadline()).unwrap();
    s.settle_injected("e", Some(1), deadline()).unwrap();
    assert!(s.admit("third", true, deadline()).is_err());
    let g = s.ledger.lock().unwrap();
    assert_eq!(
        (g.budget.received, g.budget.admitted, g.budget.denied),
        (3, 2, 1)
    );
}
#[test]
fn failed_ticket_not_refunded() {
    let s = open();
    s.admit("r", false, deadline()).unwrap();
    assert!(s.settle_injected("r", None, deadline()).is_err());
    assert!(s.admit("retry", false, deadline()).is_err());
    assert_eq!(s.ledger.lock().unwrap().budget.admitted, 1)
}
#[test]
fn read_wait_does_not_hold_lock() {
    let s = open();
    s.admit("r", false, deadline()).unwrap();
    s.settle_injected("r", Some(1), deadline()).unwrap();
    let a = s.clone();
    let h = std::thread::spawn(move || a.admit("e", true, Instant::now() + Duration::from_secs(1)));
    s.read_completed(Instant::now() + Duration::from_secs(1))
        .unwrap();
    assert!(h.join().unwrap().is_ok());
}
#[test]
fn contended_lock_closes_permanently() {
    let s = open();
    let g = s.ledger.lock().unwrap();
    let a = s.clone();
    let h =
        std::thread::spawn(move || a.admit("r", false, Instant::now() + Duration::from_millis(15)));
    assert!(h.join().unwrap().is_err());
    drop(g);
    assert_eq!(s.state(), State::Stopped);
    assert!(s.admit("next", false, deadline()).is_err());
    let x = s.snapshot(deadline()).unwrap();
    assert_eq!(x["ledger"]["broker_fault"]["unledgered_received"], 1);
}
#[test]
fn stopped_does_not_erase_prior_admission() {
    let s = open();
    s.admit("r", false, deadline()).unwrap();
    s.stop();
    s.settle_injected("r", Some(1), deadline()).unwrap();
    assert_eq!(s.ledger.lock().unwrap().budget.admitted, 1);
    assert!(s.admit("e", true, deadline()).is_err());
}
#[test]
fn paths_fail_closed_for_patterns_and_external() {
    for p in [
        TARGET.to_owned(),
        TARGET.replace('\\', "/"),
        format!(r"\\?\{}", TARGET),
    ] {
        assert!(target(&p, &["Logo.tsx".into()], false).is_ok());
        assert!(target(&p, &[p.clone()], false).is_err());
        assert!(target(&p, &["Logo.tsx".into()], true).is_err());
    }
    for p in [
        r"C:\other\Logo.tsx",
        r"C:\NovaM0B\run\project\Logo.tsx:ads",
        r"C:\NovaM0B\run\project\..\Logo.tsx",
    ] {
        assert!(target(p, &["Logo.tsx".into()], false).is_err())
    }
}
fn fact() -> FileFact {
    FileFact {
        path: ".git/opencode".into(),
        bytes: vec![b'a'; 40],
        identity: "volume:file".into(),
        owner: "probe-sid".into(),
        dacl: "fixed".into(),
        attributes: 32,
        ads: vec![],
        links: 1,
        creation: 1,
        write: 1,
        access: 1,
        change: 1,
    }
}
#[test]
fn declared_git_write_only_time() {
    let a = fact();
    let mut b = a.clone();
    b.write = 2;
    assert!(declared_git_write(&a, &b, &"a".repeat(40), "probe-sid", true).is_ok());
    for change in 0..8 {
        let mut b = a.clone();
        match change {
            0 => b.bytes[0] = b'b',
            1 => b.identity = "new".into(),
            2 => b.owner = "admin".into(),
            3 => b.attributes = 34,
            4 => b.ads.push(("ads".into(), vec![1])),
            5 => b.links = 2,
            6 => b.creation = 2,
            _ => b.dacl = "changed".into(),
        };
        assert!(declared_git_write(&a, &b, &"a".repeat(40), "probe-sid", true).is_err())
    }
    assert!(declared_git_write(&a, &a, &"a".repeat(40), "probe-sid", false).is_err())
}
#[test]
fn git_images_require_frozen_manifest() {
    assert!(git_images(&[], &[]).is_err());
    let a = vec![("git.exe".into(), "hash".into())];
    assert!(git_images(&a, &a).is_ok());
    assert!(git_images(&a, &[]).is_err())
}
fn permission() -> Permission {
    Permission {
        id: "p".into(),
        session_id: "s".into(),
        message_id: "m".into(),
        call_id: "c".into(),
        permission: "edit".into(),
        patterns: vec!["Logo.tsx".into()],
        metadata: serde_json::json!({"filepath":TARGET,"diff":"display only"}),
    }
}
fn part() -> ToolPart {
    ToolPart {
        session_id: "s".into(),
        message_id: "m".into(),
        call_id: "c".into(),
        tool: "edit".into(),
        status: "running".into(),
        input: serde_json::json!({"filePath":TARGET,"oldString":"alt=\"Payload Logo\"","newString":"alt=\"Northstar Studio logo\"","replaceAll":false}),
    }
}
#[test]
fn bounded_reread_and_association() {
    let p = permission();
    let mut gate = Association::new("s");
    let mut reads = 0;
    assert!(gate
        .input(&p, || {
            reads += 1;
            let mut t = part();
            if reads < 3 {
                t.status = "pending".into();
                t.input = serde_json::json!({});
            }
            Ok(vec![t])
        })
        .is_ok());
    assert_eq!(reads, 3);
    assert!(gate.input(&p, || Ok(vec![part()])).is_err());
}
#[test]
fn missing_late_duplicate_cross_run_rejected() {
    for n in 0..5 {
        let mut a = Association::new("s");
        let p = permission();
        assert!(a
            .input(&p, || {
                let mut t = part();
                match n {
                    0 => return Ok(vec![]),
                    1 => return Ok(vec![t.clone(), t]),
                    2 => t.session_id = "other".into(),
                    3 => t.status = "completed".into(),
                    _ => {
                        t.status = "pending".into();
                        t.input = serde_json::json!({})
                    }
                }
                Ok(vec![t])
            })
            .is_err())
    }
}
#[test]
fn edit_input_oracle_not_display_diff() {
    let mut t = part();
    t.input["newString"] = serde_json::json!("wrong");
    assert!(Association::new("s")
        .input(&permission(), || Ok(vec![t.clone()]))
        .is_err());
}
#[test]
fn sse_fixed_tool_and_framing() {
    let s = sse("read", "r", &serde_json::json!({"filePath":TARGET})).unwrap();
    let s = String::from_utf8(s).unwrap();
    assert!(s.ends_with("data: [DONE]\n\n"));
    assert!(s.contains("tool_calls"));
    assert!(sse("bash", "r", &serde_json::json!({})).is_err())
}
fn request(edit: bool) -> Vec<u8> {
    super::opencode_broker::fixed_request(edit)
}

#[test]
fn broker_read_edit_and_fixed_denial() {
    let s = open();
    let b = Broker { state: s.clone() };
    assert!(
        String::from_utf8(b.request(&request(false), deadline()).unwrap())
            .unwrap()
            .contains("200 OK")
    );
    s.read_completed(deadline()).unwrap();
    assert!(b.request(&request(true), deadline()).is_ok());
    let x = b.request(&request(true), deadline()).unwrap();
    assert!(x.starts_with(super::transport::DENIAL_HEADER));
    assert!(x.ends_with(super::transport::DENIAL_BODY));
}
#[test]
fn broker_route_size_deadline() {
    for raw in [
        b"GET / HTTP/1.1\r\n\r\n".to_vec(),
        vec![b'x'; 1048600],
        request(false),
    ] {
        let b = Broker { state: open() };
        assert!(b.request(&raw, Instant::now()).is_err());
    }
}
#[test]
fn timeline_order_and_late_api() {
    let mut t = Timeline::new(0);
    assert!(t.enter(Phase::Cas, 0).is_err());
    t.enter(Phase::Preflight, 1).unwrap();
    t.enter(Phase::Capture, 2).unwrap();
    t.enter(Phase::Target, 3).unwrap();
    assert!(t.enter(Phase::Export, 120004).is_err());
}
#[test]
fn resource_unknown_loss_blocks() {
    let p = Preflight {
        resources_prepared: true,
        owner_verified: true,
        git_images_verified: true,
        git_root_verified: true,
        capture_exclusive: true,
        observation_complete: true,
        loss: Some(0),
    };
    assert!(p.validate().is_ok());
    let mut q = p;
    q.loss = None;
    assert!(q.validate().is_err())
}

struct Injected {
    now: u64,
    fail: &'static str,
    calls: Vec<String>,
    active: Option<u32>,
    idle: bool,
    review: bool,
    cas: bool,
    journal: JournalState,
    receipt: bool,
    late: &'static str,
}
impl Injected {
    fn new() -> Self {
        Self {
            now: 0,
            fail: "",
            calls: vec![],
            active: Some(0),
            idle: true,
            review: true,
            cas: false,
            journal: JournalState::CommittedHistorical,
            receipt: true,
            late: "",
        }
    }
    fn call(&mut self, s: &str) -> super::Result<()> {
        self.calls.push(s.into());
        self.now += if self.late == s { 100001 } else { 1 };
        if self.fail == s {
            Err(format!("INJECTED {s}"))
        } else {
            Ok(())
        }
    }
}
impl Ops for Injected {
    fn now_ms(&self) -> u64 {
        self.now
    }
    fn evidence(&mut self, s: &str) -> super::Result<()> {
        self.call(s)
    }
    fn preflight(&mut self) -> super::Result<Preflight> {
        self.call("preflight")?;
        Ok(Preflight {
            resources_prepared: true,
            owner_verified: true,
            git_images_verified: true,
            git_root_verified: true,
            capture_exclusive: true,
            observation_complete: true,
            loss: Some(0),
        })
    }
    fn capture_start(&mut self) -> super::Result<()> {
        self.call("capture")
    }
    fn start_and_verify(&mut self) -> super::Result<()> {
        self.call("assign-query-resume")
    }
    fn task(&mut self, s: &Arc<RunState>) -> super::Result<super::contract::Patch> {
        self.call("task")?;
        let b = Broker { state: s.clone() };
        b.request(&request(false), deadline())?;
        let mut flow = PermissionFlow::new("s");
        let mut read_permission = permission();
        read_permission.id = "read_permission".into();
        read_permission.permission = "read".into();
        let mut read_part = part();
        read_part.tool = "read".into();
        read_part.input = serde_json::json!({"filePath":TARGET});
        flow.permission(s, &read_permission, || Ok(vec![read_part.clone()]))?;
        read_part.status = "completed".into();
        flow.completed_read(s, &read_part)?;
        b.request(&request(true), deadline())?;
        flow.permission(s, &permission(), || Ok(vec![part()]))?
            .ok_or("missing proposal".into())
    }
    fn reject(&mut self, b: &[u8]) -> super::Result<()> {
        assert_eq!(b, br#"{"reply":"reject"}"#);
        self.call("reject")
    }
    fn idle(&mut self) -> super::Result<bool> {
        self.call("idle")?;
        Ok(self.idle)
    }
    fn abort(&mut self) -> super::Result<()> {
        self.call("abort")
    }
    fn stop_job(&mut self) -> super::Result<Option<u32>> {
        self.call("job-zero")?;
        Ok(self.active)
    }
    fn capture_stop(&mut self) -> super::Result<()> {
        self.call("capture-stop")
    }
    fn export_and_review(&mut self) -> super::Result<ObservationReview> {
        self.call("capture-review")?;
        Ok(ObservationReview {
            exclusive_vm: self.review,
            same_generation: true,
            no_prior_capture: true,
            start_receipt: true,
            stop_receipt: true,
            outputs_closed: true,
            process_tree: true,
            file_writes_ads: true,
            network_attempts: true,
            pml_loss: Some(0),
            etl_loss: Some(0),
            pml_bytes: 1,
            etl_bytes: 1,
        })
    }
    fn cas_and_verify(&mut self, patch: &super::contract::Patch) -> super::Result<CasReview> {
        self.cas = true;
        self.call("CAS")?;
        let candidate = patch.validate("M0B", "p", super::PRE)?;
        assert_eq!(candidate, super::oracle(super::PRE)?);
        Ok(CasReview {
            state: self.journal,
            receipt: self.receipt,
            independently_verified: true,
        })
    }
    fn cleanup(&mut self) -> super::Result<()> {
        self.call("cleanup")
    }
}
#[test]
fn injected_full_pipeline_orders_reject_stop_review_cas() {
    let mut o = Injected::new();
    let r = run(&mut o, RunState::new().unwrap(), "INJECTED");
    assert!(r.success, "{r:?}");
    let index = |s: &str| o.calls.iter().position(|x| x == s).unwrap();
    assert!(index("reject") < index("job-zero"));
    assert!(index("job-zero") < index("capture-review"));
    assert!(index("capture-review") < index("CAS"));
    assert!(!r.execution_authorized);
}
#[test]
fn every_injected_failure_before_cas_blocks_write() {
    for f in [
        "attempt-create-new",
        "preflight",
        "capture",
        "capture-started",
        "assign-query-resume",
        "task",
        "proposal-stopped",
        "reject",
        "idle",
        "job-zero",
        "capture-stop",
        "capture-review",
        "observation-reviewed",
    ] {
        let mut o = Injected::new();
        o.fail = f;
        let state = RunState::new().unwrap();
        let r = run(&mut o, state.clone(), "INJECTED");
        assert!(!r.success, "{f}");
        assert!(!o.cas, "{f}");
        assert_eq!(state.state(), State::Stopped);
    }
}
#[test]
fn unknown_job_idle_and_loss_do_not_cas() {
    for i in 0..4 {
        let mut o = Injected::new();
        match i {
            0 => o.active = None,
            1 => o.active = Some(1),
            2 => o.idle = false,
            _ => o.review = false,
        }
        let r = run(&mut o, RunState::new().unwrap(), "INJECTED");
        assert!(!r.success);
        assert!(!o.cas);
    }
}
#[test]
fn failed_cas_never_auto_recovers() {
    let mut o = Injected::new();
    o.fail = "CAS";
    let r = run(&mut o, RunState::new().unwrap(), "INJECTED");
    assert!(r.cas_started);
    assert!(!r.success);
    assert!(o
        .calls
        .contains(&"CAS-failure-preserve-journal-no-restore".into()));
    assert!(!o.calls.iter().any(|x| x == "recover"));
}
#[test]
fn malformed_request_is_retained_and_stops() {
    let state = open();
    let b = Broker {
        state: state.clone(),
    };
    assert!(b.request(b"invalid", deadline()).is_err());
    let g = state.ledger.lock().unwrap();
    assert_eq!((g.budget.received, g.budget.denied), (1, 1));
    assert_eq!(state.state(), State::Stopped);
}
#[test]
fn stop_before_prompt_and_open_first_have_distinct_decisions() {
    let s = RunState::new().unwrap();
    s.stop();
    assert!(s.open_once(true).is_err());
    assert!(s.prompt_once(deadline()).is_err());
    let s = RunState::new().unwrap();
    s.open_once(true).unwrap();
    assert_eq!(s.state(), State::Open);
    s.prompt_once(deadline()).unwrap();
    s.stop();
    assert_eq!(s.state(), State::Stopped);
    assert!(s.prompt_once(deadline()).is_err());
}

#[derive(Default)]
struct Child {
    calls: Vec<&'static str>,
    fail: &'static str,
    active: u32,
    flags: u32,
    exit: bool,
}
impl Child {
    fn hit(&mut self, s: &'static str) -> super::Result<()> {
        self.calls.push(s);
        if self.fail == s {
            Err(format!("INJECTED {s}"))
        } else {
            Ok(())
        }
    }
}
impl ProcessOps for Child {
    fn create_suspended(&mut self) -> super::Result<()> {
        self.hit("create")
    }
    fn assign(&mut self) -> super::Result<()> {
        self.hit("assign")
    }
    fn flags_and_active(&mut self) -> super::Result<(u32, u32)> {
        self.hit("query")?;
        Ok((self.flags, self.active))
    }
    fn resume(&mut self) -> super::Result<()> {
        self.hit("resume")
    }
    fn terminate_suspended(&mut self) -> super::Result<()> {
        self.hit("terminate-direct")
    }
    fn terminate_job(&mut self) -> super::Result<()> {
        self.hit("terminate-job")
    }
    fn exited(&mut self) -> super::Result<bool> {
        self.hit("exit")?;
        Ok(self.exit)
    }
}
#[test]
fn assigned_query_before_resume_and_cleanup_is_observed() {
    let mut c = Child {
        flags: 0x2000,
        active: 1,
        ..Default::default()
    };
    launch_verified(&mut c).unwrap();
    assert_eq!(c.calls, ["create", "assign", "query", "resume"]);
    c.active = 0;
    c.exit = true;
    confirm_stopped(&mut c, deadline()).unwrap();
    assert_eq!(&c.calls[4..], ["terminate-job", "query", "exit"]);
}
#[test]
fn assign_query_flags_failures_never_resume() {
    for f in ["create", "assign", "query", ""] {
        let mut c = Child {
            fail: f,
            flags: 0,
            active: 1,
            ..Default::default()
        };
        assert!(launch_verified(&mut c).is_err());
        assert!(!c.calls.contains(&"resume"));
        if f != "create" {
            assert!(c.calls.contains(&"terminate-direct"));
        }
    }
}
#[test]
fn cleanup_unknown_exit_and_deadline_are_not_zero() {
    let mut c = Child {
        fail: "query",
        ..Default::default()
    };
    assert!(confirm_stopped(&mut c, deadline()).is_err());
    c.fail = "";
    c.flags = 0x2000;
    c.active = 0;
    c.exit = false;
    assert!(confirm_stopped(&mut c, Instant::now() + Duration::from_millis(2)).is_err());
    assert!(confirm_stopped(&mut c, Instant::now()).is_err());
}
#[derive(Default)]
struct Sink {
    exists: bool,
    short: bool,
    flush_bad: bool,
    data: Vec<u8>,
}
impl EvidenceSink for Sink {
    fn create_new(&mut self, _: &str) -> super::Result<()> {
        if self.exists {
            Err("exists".into())
        } else {
            self.exists = true;
            Ok(())
        }
    }
    fn write(&mut self, b: &[u8]) -> super::Result<usize> {
        let n = if self.short { b.len() - 1 } else { b.len() };
        self.data.extend_from_slice(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> super::Result<()> {
        if self.flush_bad {
            Err("Flush".into())
        } else {
            Ok(())
        }
    }
}
#[test]
fn evidence_no_overwrite_short_write_flush_and_nonce() {
    let mut s = Sink::default();
    record(&mut s, "attempt", b"data", b"secret").unwrap();
    assert!(record(&mut s, "attempt", b"data", b"").is_err());
    for (short, flush_bad) in [(true, false), (false, true)] {
        let mut s = Sink {
            short,
            flush_bad,
            ..Default::default()
        };
        assert!(record(&mut s, "result", b"data", b"").is_err());
    }
    let mut s = Sink::default();
    assert!(record(&mut s, "result", b"a secret here", b"secret").is_err());
    assert!(!s.exists);
}
#[test]
fn receiptless_resources_block_without_native_calls() {
    let s = include_str!("../../../../../tools/t1-api-eval/m0b/resource-policy.json");
    assert!(resource_admission(s).is_err());
    assert!(resource_admission("{}").is_err());
    assert!(resource_admission("null").is_err());
}
#[test]
fn proposal_requires_both_completed_tickets() {
    let s = open();
    assert!(s.proposal_ready(deadline()).is_err());
    let s = open();
    s.admit("r", false, deadline()).unwrap();
    s.settle_injected("r", Some(1), deadline()).unwrap();
    s.read_completed(deadline()).unwrap();
    assert!(s.proposal_ready(deadline()).is_err());
}
#[test]
fn count_all_denied_received_including_after_eight() {
    let s = open();
    for _ in 0..9 {
        assert!(s.admit("x", true, deadline()).is_err());
    }
    let l = s.ledger.lock().unwrap();
    assert_eq!(
        (l.budget.received, l.budget.denied, l.budget.admitted),
        (9, 9, 0)
    );
}
#[test]
fn unknown_usage_blocks_proposal() {
    let s = open();
    s.admit("r", false, deadline()).unwrap();
    assert!(s.settle_injected("r", None, deadline()).is_err());
    assert_eq!(s.state(), State::Stopped);
    assert!(s.proposal_ready(deadline()).is_err());
}
#[test]
fn capture_export_cas_clocks_are_distinct() {
    let mut t = Timeline::new(0);
    t.enter(Phase::Preflight, 1).unwrap();
    t.enter(Phase::Capture, 2).unwrap();
    t.enter(Phase::Target, 3).unwrap();
    t.enter(Phase::CaptureStop, 120003).unwrap();
    t.enter(Phase::Export, 120004).unwrap();
    assert!(t.enter(Phase::Cas, 135005).is_err());
}
#[test]
fn backwards_time_and_remaining_budget_refuse() {
    let mut t = Timeline::new(100);
    assert!(t.enter(Phase::Preflight, 99).is_err());
    let mut t = Timeline::new(0);
    t.phase = Phase::Export;
    t.entered = 225000;
    assert!(t.enter(Phase::Cas, 225001).is_err());
}
#[test]
fn failed_attempt_performs_no_cleanup_or_other_actions() {
    let mut o = Injected::new();
    o.fail = "attempt-create-new";
    assert!(!run(&mut o, RunState::new().unwrap(), "INJECTED").success);
    assert_eq!(o.calls, ["attempt-create-new"]);
}
#[test]
fn every_path_representation_still_needs_exact_pattern() {
    for path in [
        TARGET.to_string(),
        TARGET.replace('\\', "/"),
        format!(r"\\?\{}", TARGET),
    ] {
        assert!(target(&path, &["Logo.tsx".into()], false).is_ok());
        assert!(target(&path, &[path.clone()], false).is_err());
        assert!(target(&path, &["Logo.tsx".into()], true).is_err());
    }
}
#[test]
fn upstream_dto_rejects_unknown_fields() {
    assert!(
        serde_json::from_value::<Permission>(serde_json::json!({"id":"x","unrecognized":"x"}))
            .is_err()
    );
    let mut v = serde_json::to_value(part().input).unwrap();
    v["replaceAll"] = serde_json::Value::Bool(true);
    assert!(Association::patch("p", &v).is_err());
}

#[test]
fn slow_trickle_has_one_total_deadline() {
    let mut clock = 0u64;
    let result = super::opencode_broker::read_bounded(
        |b, _| {
            b[0] = b'x';
            Ok(1)
        },
        || {
            clock += 500;
            clock
        },
    );
    assert!(result.is_err());
    assert!(clock <= 5500);
}
#[test]
fn connection_bytes_match_fixed_request() {
    let bytes = request(false);
    let mut at = 0;
    let out = super::opencode_broker::read_bounded(
        |b, _| {
            let n = (bytes.len() - at).min(b.len());
            b[..n].copy_from_slice(&bytes[at..at + n]);
            at += n;
            Ok(n)
        },
        || 0,
    )
    .unwrap();
    assert_eq!(out, bytes);
}
#[test]
fn actual_permission_wire_is_normalized_explicitly() {
    let v = serde_json::json!({"id":"per_x","sessionID":"s","permission":"edit","patterns":["Logo.tsx"],"metadata":{"filepath":TARGET},"always":["*"],"tool":{"messageID":"m","callID":"c"}});
    let p = permission_wire(&v).unwrap();
    assert_eq!(p.message_id, "m");
    assert_eq!(p.call_id, "c");
    let mut bad = v.clone();
    bad["tool"] = serde_json::Value::Null;
    assert!(permission_wire(&bad).is_err());
    let mut bad = v;
    bad["unrecognized"] = serde_json::Value::Bool(true);
    assert!(permission_wire(&bad).is_err());
}
#[test]
fn actual_tool_wire_reads_nested_state() {
    let v = serde_json::json!({"id":"prt_x","sessionID":"s","messageID":"m","type":"tool","callID":"c","tool":"read","state":{"status":"running","input":{"filePath":TARGET},"time":{"start":0}}});
    let t = tool_wire(&v).unwrap();
    assert_eq!(t.status, "running");
    assert_eq!(t.input["filePath"], TARGET);
    let mut bad = v;
    bad["state"]["input"] = serde_json::Value::Null;
    assert!(tool_wire(&bad).is_err());
}
#[test]
fn sse_stream_fragment_bounds_and_truncation() {
    let mut s = EventStream::new();
    assert!(s.push(b"data: {\"type\":").unwrap().is_empty());
    assert_eq!(s.push(b"\"permission.asked\"}\n\n").unwrap().len(), 1);
    s.end().unwrap();
    let mut s = EventStream::new();
    s.push(b"data: {}").unwrap();
    assert!(s.end().is_err());
    let mut s = EventStream::new();
    assert!(s.push(&vec![b'x'; 256 * 1024 + 1]).is_err());
}
#[test]
fn control_health_event_and_request_limits() {
    let mut b = ControlBudget::new();
    for _ in 0..5 {
        b.reserve(Control::Health).unwrap();
    }
    assert!(b.reserve(Control::Health).is_err());
    let mut b = ControlBudget::new();
    b.reserve(Control::Event).unwrap();
    assert!(b.reserve(Control::Event).is_err());
    for _ in 0..40 {
        b.reserve(Control::Status).unwrap();
    }
    assert!(b.reserve(Control::Abort).is_err());
}

#[test]
fn mandatory_observer_fields_and_unknown_loss_block() {
    let good = ObservationReview {
        exclusive_vm: true,
        same_generation: true,
        no_prior_capture: true,
        start_receipt: true,
        stop_receipt: true,
        outputs_closed: true,
        process_tree: true,
        file_writes_ads: true,
        network_attempts: true,
        pml_loss: Some(0),
        etl_loss: Some(0),
        pml_bytes: 1,
        etl_bytes: 1,
    };
    good.validate().unwrap();
    for i in 0..9 {
        let mut v = good.clone();
        match i {
            0 => v.pml_loss = None,
            1 => v.etl_loss = None,
            2 => v.pml_loss = Some(1),
            3 => v.outputs_closed = false,
            4 => v.network_attempts = false,
            5 => v.process_tree = false,
            6 => v.file_writes_ads = false,
            7 => v.same_generation = false,
            _ => v.no_prior_capture = false,
        };
        assert!(v.validate().is_err());
    }
}
#[test]
fn tree_rejects_new_hidden_other_git_and_ads() {
    let a = fact();
    let mut b = a.clone();
    b.access = 5;
    b.change = 9;
    b.write = 7;
    review_tree(
        &[a.clone()],
        &[b.clone()],
        &"a".repeat(40),
        "probe-sid",
        true,
    )
    .unwrap();
    let mut extra = a.clone();
    extra.path = ".git/config".into();
    assert!(review_tree(
        &[a.clone()],
        &[b, extra],
        &"a".repeat(40),
        "probe-sid",
        true
    )
    .is_err());
    let mut b = a.clone();
    b.ads.push(("stream".into(), vec![]));
    assert!(review_tree(&[a.clone()], &[b], &"a".repeat(40), "probe-sid", true).is_err());
    let mut a = fact();
    a.path = ".git/config".into();
    let mut b = a.clone();
    b.write = 8;
    assert!(review_tree(&[a], &[b], &"a".repeat(40), "probe-sid", true).is_err());
}
#[test]
fn fixed_public_mock_runs_complete_orchestration() {
    let v = mock_self_test().unwrap();
    assert_eq!(v["outcome"]["success"], true);
    assert_eq!(v["native_executed"], false);
    assert_eq!(v["ledger"]["ledger"]["admitted"], 2);
}

#[test]
fn uncertain_and_uncertain_recovery_are_never_success() {
    for state in [
        JournalState::Prepared,
        JournalState::Uncertain,
        JournalState::UncertainRecovery,
    ] {
        let mut o = Injected::new();
        o.journal = state;
        let result = run(&mut o, RunState::new().unwrap(), "INJECTED");
        assert!(result.cas_started);
        assert!(!result.success);
        assert_eq!(result.journal.unwrap().state, state);
        assert!(!o.calls.iter().any(|s| s == "recover"));
    }
    let mut o = Injected::new();
    o.receipt = false;
    assert!(!run(&mut o, RunState::new().unwrap(), "INJECTED").success);
}
#[test]
fn minimal_environment_has_no_inherited_injections() {
    let env = minimal_environment();
    for k in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NODE_OPTIONS",
        "NODE_EXTRA_CA_CERTS",
        "NPM_CONFIG_USERCONFIG",
        "GIT_ASKPASS",
        "OPENCODE_AUTH_JSON",
    ] {
        assert!(!env.contains_key(k));
    }
    assert_eq!(env["GIT_CONFIG_NOSYSTEM"], "1");
    assert_eq!(env["GIT_CONFIG_VALUE_0"], "never");
    assert!(env["HOME"].starts_with(r"C:\NovaM0B\run\agent"));
}

#[test]
fn git_owner_root_image_preflight_is_fixed_and_fail_closed() {
    let manifest = vec![("git.exe".into(), "frozen-test-hash".into())];
    let mut r = GitPreflight {
        attempts: 1,
        exit: Some(0),
        timed_out: false,
        job_active: Some(0),
        stdout: format!("{ROOT}\r\n").into_bytes(),
        owner_sid: "probe-sid".into(),
        manifest_complete: true,
        images: manifest.clone(),
    };
    git_preflight(&r, "probe-sid", &manifest).unwrap();
    r.stdout = b"C:\\other\r\n".to_vec();
    assert!(git_preflight(&r, "probe-sid", &manifest).is_err());
    r.stdout = format!("{ROOT}\r\n").into_bytes();
    r.owner_sid = "admin".into();
    assert!(git_preflight(&r, "probe-sid", &manifest).is_err());
    r.owner_sid = "probe-sid".into();
    r.job_active = None;
    assert!(git_preflight(&r, "probe-sid", &manifest).is_err());
    r.job_active = Some(0);
    r.manifest_complete = false;
    assert!(git_preflight(&r, "probe-sid", &manifest).is_err());
}
#[test]
fn normal_stop_before_or_after_prompt_decision_cannot_reopen() {
    let s = RunState::new().unwrap();
    s.open_once(true).unwrap();
    s.stop();
    assert!(s.prompt_once(deadline()).is_err());
    let s = RunState::new().unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(deadline()).unwrap();
    s.stop();
    assert!(s.open_once(true).is_err());
    assert!(s.admit("late", false, deadline()).is_err());
}

#[test]
fn late_start_task_or_job_blocks_cas_and_keeps_unknown() {
    for stage in ["assign-query-resume", "task", "job-zero"] {
        let mut o = Injected::new();
        o.late = stage;
        let r = run(&mut o, RunState::new().unwrap(), "INJECTED");
        assert!(!r.success);
        assert!(!o.cas);
        if stage == "job-zero" {
            assert_eq!(r.job_active, None);
            assert!(!r.cleanup_errors.is_empty());
        }
    }
}
#[test]
fn late_cas_keeps_historical_receipt_without_automatic_recovery() {
    let mut o = Injected::new();
    o.late = "CAS";
    let r = run(&mut o, RunState::new().unwrap(), "INJECTED");
    assert!(!r.success);
    assert!(r.cas_started);
    assert_eq!(r.journal.unwrap().state, JournalState::CommittedHistorical);
    assert!(!o.calls.iter().any(|s| s == "recover"));
}
#[test]
fn failure_in_one_run_does_not_pollute_fresh_scope() {
    let old = RunState::new().unwrap();
    old.stop();
    let fresh = open();
    assert!(fresh.admit("r", false, deadline()).is_ok());
    assert_eq!(old.state(), State::Stopped);
}
