//! NB3 tests use only in-memory facts, injected ports and clocks. No native construction or IO.
use super::{
    contract::Patch, hash, opencode_cas_bridge::*, opencode_native_files::FactSource,
    opencode_observation::*, opencode_paths::*, opencode_probe::*, oracle, PRE,
};
use std::{cell::Cell, rc::Rc, sync::Arc};
fn target() -> ProcessIdentity {
    injected_target()
}
fn fixture() -> (FixtureReservation, TreeSnapshot) {
    (injected_preflight().fixture, injected_tree())
}
fn events() -> Vec<Event> {
    vec![
        Event::Process(Process {
            at: 1,
            process: target(),
            parent: None,
        }),
        Event::Write(Write {
            at: 10,
            process: target(),
            path: format!("{}\\.git/opencode", ROOT),
            kind: WriteKind::Overwrite,
            open_access: None,
        }),
        Event::Network(Network {
            at: 20,
            process: target(),
            protocol: "TCP".into(),
            address: "127.0.0.1".into(),
            port: 1234,
        }),
    ]
}
fn finish(mut es: Vec<Event>) -> Vec<Event> {
    let mut c = [0; 3];
    for e in &es {
        match e {
            Event::Process(_) => c[0] += 1,
            Event::Write(_) => c[1] += 1,
            Event::Network(_) => c[2] += 1,
            _ => (),
        }
    }
    for (i, class) in [
        ObservationClass::ProcessTree,
        ObservationClass::FileWriteAds,
        ObservationClass::NetworkAttempts,
    ]
    .into_iter()
    .enumerate()
    {
        es.push(Event::End(End {
            at: 100,
            class,
            records: c[i],
            loss: 0,
        }))
    }
    es
}
fn encode(es: Vec<Event>) -> Vec<u8> {
    let mut body = Vec::new();
    for e in &es {
        body.extend(serde_json::to_vec(e).unwrap());
        body.push(b'\n')
    }
    let digest = hash(&body);
    let h = Header {
        schema: "NB3_SYNTHETIC_V1".into(),
        source: FactSource::Injected,
        run: "M0B".into(),
        attempt: "p1".into(),
        generation: "INJECTED".into(),
        capture: "lease".into(),
        count: es.len() as u64,
        body_sha256: digest.clone(),
        started: 1,
        stopped: 100,
        closed: 101,
        root: target(),
    };
    let tail = Tail {
        schema: "NB3_END_V1".into(),
        count: es.len() as u64,
        body_sha256: digest,
    };
    let mut b = serde_json::to_vec(&h).unwrap();
    b.push(b'\n');
    b.extend(body);
    b.extend(serde_json::to_vec(&tail).unwrap());
    b.push(b'\n');
    b
}
fn parsed(b: &[u8]) -> super::Result<ParsedObservation> {
    let (r, t) = fixture();
    let target = target();
    let ownership = ownership();
    let bind = Binding {
        run: "M0B",
        attempt: "p1",
        capture: "lease",
        generation: "INJECTED",
        ownership: &ownership,
        model_port: 1234,
        target: &target,
    };
    parse(b, &bind, &r, t)
}
fn agent() -> ReviewedAgentTree {
    let (r, t) = fixture();
    ReviewedAgentTree::review(
        &t,
        &parsed(&encode(finish(events()))).unwrap().review.tree,
        &r,
        &target(),
        FactSource::Injected,
    )
    .unwrap()
}
fn patch() -> Patch {
    Patch {
        run_id: "M0B".into(),
        role: "developer".into(),
        attempt_id: "p1".into(),
        target_id: "logo".into(),
        expected_sha256: hash(PRE),
        old_literal: "alt=\"Payload Logo\"".into(),
        new_literal: "alt=\"Northstar Studio logo\"".into(),
        reason: "INJECTED".into(),
        replace_all: false,
    }
}
fn receipt() -> super::journal::Receipt {
    let a = agent();
    let p = oracle(PRE).unwrap();
    super::journal::Receipt {
        identity: a.reservation().logo_identity.clone(),
        pre_sha256: hash(PRE),
        post_sha256: hash(&p),
        bytes: p.len(),
        state: "COMMITTED".into(),
        durability_limit: "INJECTED".into(),
    }
}
fn nova() -> TreeSnapshot {
    let mut t = injected_tree();
    let f = t.files.iter_mut().find(|f| f.path == "Logo.tsx").unwrap();
    f.bytes = oracle(PRE).unwrap();
    f.write = 1_050_000;
    f.change = 1_050_000;
    t.enumeration.as_mut().unwrap().default_bytes =
        t.files.iter().map(|f| f.bytes.len() as u64).sum();
    t
}
fn assets() -> JsAssets {
    let res = |p: &str| Resource {
        path: p.into(),
        bytes: 1,
        sha256: hash(b"INJECTED"),
    };
    let mut node = res(r"E:\node\node.exe");
    node.sha256 = NODE_HASH.into();
    JsAssets {
        node,
        script: res(r"E:\tools\check-logo.cjs"),
        contract: res(r"E:\tools\logo-contract.json"),
        policy: res(r"E:\tools\opencode-policy.json"),
        lock: res(r"E:\deps\package-lock.json"),
        typescript: res(r"E:\deps\node_modules\typescript"),
        package: r"E:\deps\package.json".into(),
        isolated_home: r"E:\owned\home".into(),
    }
}
fn output() -> JsResult {
    JsResult{exit:Some(0),stdout:serde_json::to_vec(&serde_json::json!({"scope":"deterministic contract and TSX parse only; not typecheck, lint, DOM or production build","bytes":795,"sha256":hash(&oracle(PRE).unwrap())})).unwrap(),stderr:vec![],exited:true,readers_done:true,timed_out:false}
}
struct MemoryCas {
    now: Rc<Cell<u64>>,
    point: &'static str,
    state: &'static str,
    prepared: bool,
    apply_calls: usize,
    trees: usize,
    clock_calls: Cell<usize>,
    status_calls: usize,
}
impl MemoryCas {
    fn new() -> Self {
        Self {
            now: Rc::new(Cell::new(0)),
            point: "",
            state: "PREPARED",
            prepared: false,
            apply_calls: 0,
            trees: 0,
            clock_calls: Cell::new(0),
            status_calls: 0,
        }
    }
}
impl CasPort for MemoryCas {
    fn now_ms(&self) -> u64 {
        self.now.get()
    }
    fn filetime(&self) -> super::Result<u64> {
        let n = self.clock_calls.get();
        self.clock_calls.set(n + 1);
        Ok(if n == 0 { 1_000_000 } else { 1_100_000 })
    }
    fn verify_root(&mut self, _: &RootReservation) -> super::Result<()> {
        if self.point == "root" {
            Err("INJECTED".into())
        } else {
            Ok(())
        }
    }
    fn prepare(&mut self, _: &str, _: &[u8], _: &[u8]) -> super::Result<()> {
        if self.point == "partial-prepare" {
            return Err("partial directory preserved".into());
        }
        self.prepared = true;
        Ok(())
    }
    fn reserve(&mut self, _: &FixtureReservation) -> super::Result<()> {
        if self.point == "reserve" {
            Err("INJECTED".into())
        } else {
            Ok(())
        }
    }
    fn reservation(&mut self) -> super::Result<Reserved> {
        Ok(Reserved {
            identity: if self.point == "identity" {
                "replacement".into()
            } else {
                agent().reservation().logo_identity.clone()
            },
            root: ROOT.into(),
            final_path: TARGET.into(),
        })
    }
    fn apply(&mut self, _: &FixtureReservation, _: &[u8], _: &[u8]) -> super::Result<()> {
        self.apply_calls += 1;
        self.state = match self.point {
            "intent" | "short-write" | "flush" | "receipt" => "UNCERTAIN",
            "before-intent" => "PREPARED",
            "recovered" => "RECOVERED_HISTORICAL",
            "recovering" => "UNCERTAIN_RECOVERY",
            _ => "COMMITTED_HISTORICAL",
        };
        if self.point == "late" {
            self.now.set(40000)
        }
        if ["intent", "short-write", "flush", "receipt", "before-intent"].contains(&self.point) {
            return Err("INJECTED not used for mapping".into());
        }
        Ok(())
    }
    fn status(&mut self) -> super::Result<String> {
        self.status_calls += 1;
        if self.status_calls == 2 {
            match self.point {
                "final-status-error" => return Err("INJECTED final read".into()),
                "final-status-prepared" => return Ok("PREPARED".into()),
                "final-status-recovered" => return Ok("RECOVERED_HISTORICAL".into()),
                "final-status-unknown" => return Ok("UNKNOWN".into()),
                _ => (),
            }
        }
        if self.point == "status" {
            return Err("INJECTED".into());
        }
        Ok(self.state.into())
    }
    fn receipt(&mut self) -> super::Result<super::journal::Receipt> {
        let mut receipt = receipt();
        if self.point == "receipt-mismatch" {
            receipt.post_sha256 = hash(b"INJECTED wrong receipt");
        }
        Ok(receipt)
    }
    fn after_tree(&mut self) -> super::Result<TreeSnapshot> {
        self.trees += 1;
        let mut n = nova();
        if self.point == "tree" {
            n.files[0].creation += 1
        }
        Ok(n)
    }
}
struct MemoryJs {
    calls: usize,
    result: JsResult,
    now: Rc<Cell<u64>>,
    late: bool,
}
impl JsVerifier for MemoryJs {
    fn verify(&mut self, r: &JsRequest) -> super::Result<JsResult> {
        self.calls += 1;
        assert_eq!(r.argv.len(), 4);
        assert!(r.environment.iter().all(|(k, _)| !k.starts_with("NODE_")));
        if self.late {
            self.now.set(r.deadline_ms + 1)
        }
        Ok(self.result.clone())
    }
}
fn invoke(p: &mut MemoryCas) -> (super::Result<BridgeResult>, usize) {
    let mut j = MemoryJs {
        calls: 0,
        result: output(),
        now: p.now.clone(),
        late: false,
    };
    let out = bridge(
        p,
        &mut j,
        &agent(),
        &patch(),
        "p1",
        r"E:\tx\one",
        &assets(),
        30000,
    );
    (out, j.calls)
}
#[test]
fn nb3_observation_valid_source_and_native_blocked() {
    let p = parsed(&encode(finish(events()))).unwrap();
    assert_eq!(p.retained_events.len(), 6);
    p.review.validate_source(FactSource::Injected).unwrap();
    assert!(p
        .review
        .validate_source(FactSource::NativePrepared)
        .is_err());
    assert!(native_parser(b"anything")
        .err()
        .unwrap()
        .contains("BLOCKED"));
}
#[test]
fn nb3_observation_header_bindings_and_truncation() {
    let bytes = encode(finish(events()));
    for (a, b) in [
        ("INJECTED", "different"),
        ("M0B", "wrong"),
        ("p1", "wrong"),
        ("lease", "wrong"),
        ("NB3_SYNTHETIC_V1", "UNKNOWN"),
        ("Injected", "NativePrepared"),
    ] {
        let s = String::from_utf8(bytes.clone()).unwrap().replacen(a, b, 1);
        assert!(parsed(s.as_bytes()).is_err(), "{a}")
    }
    for n in [0, 1, bytes.len() - 1] {
        assert!(parsed(&bytes[..n]).is_err())
    }
}
#[test]
fn nb3_observation_hash_and_tail_count() {
    let bytes = encode(finish(events()));
    let s = String::from_utf8(bytes).unwrap();
    assert!(parsed(
        s.replacen("body_sha256\":\"", "body_sha256\":\"f", 1)
            .as_bytes()
    )
    .is_err());
    assert!(parsed(
        s.rsplit_once("\"count\":6")
            .map(|(a, b)| format!("{a}\"count\":7{b}"))
            .unwrap()
            .as_bytes()
    )
    .is_err());
}
#[test]
fn nb3_observation_unknown_duplicate_fields_and_types() {
    let b = String::from_utf8(encode(finish(events()))).unwrap();
    for s in [
        b.replacen("{", "{\"extra\":1,", 1),
        b.replacen("{", "{\"count\":6,", 1),
        b.replacen("\"count\":6", "\"count\":6.0", 1),
        b.replacen("\"count\":6", "\"count\":\"6\"", 1),
    ] {
        assert!(parsed(s.as_bytes()).is_err())
    }
}
#[test]
fn nb3_observation_input_line_depth_string_bounds() {
    assert!(parsed(&vec![b'x'; MAX_INPUT + 1]).is_err());
    let b = String::from_utf8(encode(finish(events()))).unwrap();
    assert!(parsed(b.replacen("lease", &"x".repeat(4097), 1).as_bytes()).is_err());
    assert!(parsed(b.replacen("lease", &"x".repeat(MAX_LINE), 1).as_bytes()).is_err());
    let deep = b.replacen(
        "\"capture\":\"lease\"",
        &format!("\"capture\":{}0{}", "[".repeat(17), "]".repeat(17)),
        1,
    );
    assert_eq!(parsed(deep.as_bytes()).err().unwrap(), "OBS_DEPTH");
}
#[test]
fn nb3_observation_missing_coverage_loss_and_count() {
    for mode in 0..4 {
        let mut es = finish(events());
        match mode {
            0 => {
                es.pop();
            }
            1 => {
                if let Event::End(e) = &mut es[4] {
                    e.loss = 1
                }
            }
            2 => {
                if let Event::End(e) = &mut es[4] {
                    e.records += 1
                }
            }
            _ => es.push(es[4].clone()),
        }
        assert!(parsed(&encode(es)).is_err(), "mode {mode}")
    }
}
#[test]
fn nb3_observation_time_root_ancestry() {
    for mode in 0..5 {
        let mut es = events();
        match mode {
            0 => {
                if let Event::Write(x) = &mut es[1] {
                    x.at = 101
                }
            }
            1 => {
                if let Event::Process(x) = &mut es[0] {
                    x.process.created = 0
                }
            }
            2 => {
                if let Event::Process(x) = &mut es[0] {
                    x.process.pid = 18
                }
            }
            3 => {
                es.push(Event::Process(Process {
                    at: 30,
                    process: ProcessIdentity {
                        pid: 20,
                        created: 2,
                    },
                    parent: Some(ProcessIdentity {
                        pid: 21,
                        created: 2,
                    }),
                }));
            }
            _ => es.push(es[0].clone()),
        }
        assert!(parsed(&encode(finish(es))).is_err(), "mode {mode}")
    }
}
#[test]
fn nb3_observation_pid_reuse_is_distinct() {
    let mut es = events();
    es.push(Event::Process(Process {
        at: 30,
        process: ProcessIdentity {
            pid: 17,
            created: 2,
        },
        parent: Some(target()),
    }));
    assert!(parsed(&encode(finish(es))).is_ok())
}
#[test]
fn nb3_observation_all_writes_preserved_logo_restore_rejected() {
    let mut es = events();
    es.push(Event::Write(Write {
        at: 30,
        process: target(),
        path: TARGET.into(),
        kind: WriteKind::Overwrite,
        open_access: None,
    }));
    let p = parsed(&encode(finish(es))).unwrap();
    assert_eq!(p.review.tree.writes.len(), 2);
    let (r, t) = fixture();
    assert!(
        ReviewedAgentTree::review(&t, &p.review.tree, &r, &target(), FactSource::Injected).is_err()
    );
}
#[test]
fn nb3_observation_foreign_fixture_and_outside_writes() {
    for path in [
        TARGET.to_string(),
        TARGET.to_lowercase(),
        format!(r"\\?\{}", TARGET),
    ] {
        let mut es = events();
        es.push(Event::Write(Write {
            at: 30,
            process: ProcessIdentity {
                pid: 99,
                created: 3,
            },
            path,
            kind: WriteKind::Overwrite,
            open_access: None,
        }));
        assert!(parsed(&encode(finish(es))).is_err())
    }
    let mut es = events();
    es.push(Event::Write(Write {
        at: 30,
        process: ProcessIdentity {
            pid: 99,
            created: 3,
        },
        path: r"E:\other\file".into(),
        kind: WriteKind::Overwrite,
        open_access: None,
    }));
    let p = parsed(&encode(finish(es))).unwrap();
    assert_eq!(p.retained_events.len(), 7);
    assert_eq!(p.review.tree.writes.len(), 1);
}
#[test]
fn nb3_observation_marker_alias_ads_and_metadata_rejected() {
    for path in [
        format!(r"\\?\{}\.git\opencode", ROOT),
        format!(r"{}\.git/opencode:ads", ROOT),
        format!(r"{}\.GIT/opencode", ROOT),
        format!(r"{}\.git/OPENCO~1", ROOT),
    ] {
        let mut es = events();
        if let Event::Write(x) = &mut es[1] {
            x.path = path
        }
        let p = parsed(&encode(finish(es))).unwrap();
        let (r, t) = fixture();
        assert!(
            ReviewedAgentTree::review(&t, &p.review.tree, &r, &target(), FactSource::Injected)
                .is_err()
        )
    }
}
#[test]
fn nb3_observation_write_access_open_is_unknown() {
    let mut es = events();
    if let Event::Write(x) = &mut es[1] {
        x.path = TARGET.into();
        x.kind = WriteKind::Overwrite;
        x.open_access = Some(0x40000)
    }
    let p = parsed(&encode(finish(es))).unwrap();
    let (r, t) = fixture();
    assert!(
        ReviewedAgentTree::review(&t, &p.review.tree, &r, &target(), FactSource::Injected).is_err()
    )
}
#[test]
fn nb3_observation_network_denies_non_model_attempts() {
    for (protocol, address, port) in [
        ("UDP", "127.0.0.1", 1234),
        ("DNS", "host", 53),
        ("TCP", "8.8.8.8", 443),
        ("TCP", "127.0.0.1", 1235),
    ] {
        let mut es = events();
        if let Event::Network(n) = &mut es[2] {
            n.protocol = protocol.into();
            n.address = address.into();
            n.port = port
        }
        assert!(parsed(&encode(finish(es))).is_err())
    }
}
#[test]
fn nb3_observation_projection_network_edge_event_limits() {
    let w = events()[1].clone();
    let mut es = events();
    es.extend(vec![w; 64]);
    assert!(parsed(&encode(finish(es))).is_err());
    let mut es = events();
    es.extend(vec![events()[2].clone(); 4096]);
    assert!(parsed(&encode(finish(es))).is_err());
    let mut es = events();
    for i in 0..4097 {
        es.push(Event::Process(Process {
            at: 30,
            process: ProcessIdentity {
                pid: 100 + i,
                created: 2,
            },
            parent: Some(target()),
        }))
    }
    assert!(parsed(&encode(finish(es))).is_err());
    assert!(parsed(&vec![b'\n'; MAX_EVENTS + 4]).is_err())
}
#[test]
fn nb3_agent_capability_rejects_wrong_source() {
    let (r, t) = fixture();
    let p = parsed(&encode(finish(events()))).unwrap();
    assert!(ReviewedAgentTree::review(
        &t,
        &p.review.tree,
        &r,
        &target(),
        FactSource::NativePrepared
    )
    .is_err())
}
#[test]
fn nb3_nova_success_and_access_only() {
    let mut n = nova();
    for f in &mut n.files {
        f.access += 100
    }
    review_nova_tree(
        &agent(),
        &n,
        &receipt(),
        &oracle(PRE).unwrap(),
        1_000_000,
        1_100_000,
    )
    .unwrap()
}
#[test]
fn nb3_nova_marker_never_reexcused() {
    let mut n = nova();
    n.files
        .iter_mut()
        .find(|f| f.path == ".git/opencode")
        .unwrap()
        .write += 1;
    assert!(review_nova_tree(
        &agent(),
        &n,
        &receipt(),
        &oracle(PRE).unwrap(),
        1_000_000,
        1_100_000
    )
    .is_err())
}
#[test]
fn nb3_nova_logo_security_identity_and_attributes() {
    for mode in 0..8 {
        let mut n = nova();
        let f = n.files.iter_mut().find(|f| f.path == "Logo.tsx").unwrap();
        match mode {
            0 => f.owner = "other".into(),
            1 => f.dacl = "other".into(),
            2 => f.ads.push(("ads".into(), vec![])),
            3 => f.creation += 1,
            4 => f.attributes = 0,
            5 => f.identity = "other".into(),
            6 => f.links = 2,
            _ => f.bytes[0] ^= 1,
        }
        assert!(
            review_nova_tree(
                &agent(),
                &n,
                &receipt(),
                &oracle(PRE).unwrap(),
                1_000_000,
                1_100_000
            )
            .is_err(),
            "mode {mode}"
        )
    }
}
#[test]
fn nb3_nova_time_boundaries_unknowns() {
    for (t0, t1) in [
        (0, 1),
        (1_000_000, 999999),
        (200001, 300000),
        (1_000_000, 302_000_000),
        (2_000_000, 2_100_000),
    ] {
        assert!(
            review_nova_tree(&agent(), &nova(), &receipt(), &oracle(PRE).unwrap(), t0, t1).is_err(),
            "{t0}/{t1}"
        )
    }
    let mut n = nova();
    n.files
        .iter_mut()
        .find(|f| f.path == "Logo.tsx")
        .unwrap()
        .write = 800000;
    review_nova_tree(
        &agent(),
        &n,
        &receipt(),
        &oracle(PRE).unwrap(),
        1_000_000,
        1_100_000,
    )
    .unwrap()
}
#[test]
fn nb3_nova_set_and_receipt_mismatch() {
    let mut n = nova();
    n.files.pop();
    assert!(review_nova_tree(
        &agent(),
        &n,
        &receipt(),
        &oracle(PRE).unwrap(),
        1_000_000,
        1_100_000
    )
    .is_err());
    let mut r = receipt();
    r.post_sha256 = hash(b"bad");
    assert!(review_nova_tree(
        &agent(),
        &nova(),
        &r,
        &oracle(PRE).unwrap(),
        1_000_000,
        1_100_000
    )
    .is_err())
}
#[test]
fn nb3_bridge_success() {
    let mut p = MemoryCas::new();
    let (o, c) = invoke(&mut p);
    let o = o.unwrap();
    assert_eq!(o.code, "NONE");
    assert_eq!(o.review.state, JournalState::CommittedHistorical);
    assert!(o.review.receipt && o.review.independently_verified);
    assert_eq!(c, 1)
}
#[test]
fn nb3_bridge_root_and_prepare_fail_no_transaction() {
    for point in ["root", "partial-prepare"] {
        let mut p = MemoryCas::new();
        p.point = point;
        let (o, c) = invoke(&mut p);
        assert!(o.is_err());
        assert_eq!(c, 0);
        assert_eq!(p.apply_calls, 0)
    }
}
#[test]
fn nb3_bridge_pre_intent_failures_keep_prepared() {
    for point in ["reserve", "identity", "before-intent"] {
        let mut p = MemoryCas::new();
        p.point = point;
        let (o, c) = invoke(&mut p);
        assert_eq!(o.unwrap().review.state, JournalState::Prepared);
        assert_eq!(c, 0)
    }
}
#[test]
fn nb3_bridge_intent_short_flush_receipt_uncertain() {
    for point in ["intent", "short-write", "flush", "receipt", "status"] {
        let mut p = MemoryCas::new();
        p.point = point;
        let (o, c) = invoke(&mut p);
        let o = o.unwrap();
        assert_eq!(o.review.state, JournalState::Uncertain, "{point}");
        assert!(!o.review.independently_verified);
        assert_eq!(c, 0)
    }
}
#[test]
fn nb3_bridge_recovered_and_recovery_never_committed() {
    for (point, state) in [
        ("recovered", JournalState::RecoveredHistorical),
        ("recovering", JournalState::UncertainRecovery),
    ] {
        let mut p = MemoryCas::new();
        p.point = point;
        let (o, c) = invoke(&mut p);
        assert_eq!(o.unwrap().review.state, state);
        assert_eq!(c, 0)
    }
}
#[test]
fn nb3_bridge_late_apply_preserves_history_without_js() {
    let mut p = MemoryCas::new();
    p.point = "late";
    let (o, c) = invoke(&mut p);
    let o = o.unwrap();
    assert!(o.partial);
    assert_eq!(o.review.state, JournalState::CommittedHistorical);
    assert!(!o.review.independently_verified);
    assert_eq!(c, 0)
}
#[test]
fn nb3_bridge_tree_failure_preserves_committed() {
    let mut p = MemoryCas::new();
    p.point = "tree";
    let (o, c) = invoke(&mut p);
    let o = o.unwrap();
    assert_eq!(o.review.state, JournalState::CommittedHistorical);
    assert!(!o.review.independently_verified);
    assert_eq!(c, 1)
}
#[test]
fn nb3_bridge_js_failure_and_late_keep_history() {
    for mode in 0..3 {
        let mut p = MemoryCas::new();
        let mut j = MemoryJs {
            calls: 0,
            result: output(),
            now: p.now.clone(),
            late: mode == 2,
        };
        if mode == 0 {
            j.result.exit = Some(1)
        }
        if mode == 1 {
            j.result.timed_out = true
        }
        let o = bridge(
            &mut p,
            &mut j,
            &agent(),
            &patch(),
            "p1",
            r"E:\tx\one",
            &assets(),
            30000,
        )
        .unwrap();
        assert_eq!(o.review.state, JournalState::CommittedHistorical);
        assert!(!o.review.independently_verified);
        assert_eq!(p.trees, 0)
    }
}
#[test]
fn nb3_transaction_inside_root_and_budget_do_not_prepare() {
    let mut p = MemoryCas::new();
    let mut j = MemoryJs {
        calls: 0,
        result: output(),
        now: p.now.clone(),
        late: false,
    };
    assert!(bridge(
        &mut p,
        &mut j,
        &agent(),
        &patch(),
        "p1",
        &format!(r"{}\tx", ROOT),
        &assets(),
        30000
    )
    .is_err());
    assert!(!p.prepared);
    assert!(bridge(
        &mut p,
        &mut j,
        &agent(),
        &patch(),
        "p1",
        r"E:\tx\one",
        &assets(),
        1000
    )
    .is_err());
    assert!(!p.prepared)
}
#[test]
fn nb3_journal_exact_mapping_no_error_text_inference() {
    for (s, state) in [
        ("PREPARED", JournalState::Prepared),
        ("UNCERTAIN", JournalState::Uncertain),
        ("COMMITTED_HISTORICAL", JournalState::CommittedHistorical),
        ("UNCERTAIN_RECOVERY", JournalState::UncertainRecovery),
        ("RECOVERED_HISTORICAL", JournalState::RecoveredHistorical),
        ("new", JournalState::Uncertain),
    ] {
        assert_eq!(journal_state(Ok(s.into())), state)
    }
    assert_eq!(
        journal_state(Err("COMMITTED_HISTORICAL".into())),
        JournalState::Uncertain
    )
}
#[test]
fn nb3_js_request_fixed_resources_and_no_environment_inheritance() {
    let a = assets();
    let r = js_request(&a, r"E:\tx\one", 10000).unwrap();
    assert_eq!(
        r.argv,
        [
            a.script.path,
            a.package,
            TARGET.into(),
            r"E:\tx\one\preimage.bin".into()
        ]
    );
    assert_eq!(r.resources.len(), 5);
    assert_eq!(r.environment.len(), 6);
    for k in ["NODE_OPTIONS", "NODE_PATH", "API_KEY", "HTTP_PROXY"] {
        assert!(!r.environment.iter().any(|(n, _)| n == k))
    }
}
#[test]
fn nb3_js_bad_resources_rejected() {
    for i in 0..5 {
        let mut a = assets();
        match i {
            0 => a.node.sha256 = hash(b"bad"),
            1 => a.script.path = "--self-test".into(),
            2 => a.lock.path = r"E:\foreign\package-lock.json".into(),
            3 => a.typescript.bytes = 0,
            _ => a.policy.sha256 = "unknown".into(),
        }
        assert!(js_request(&a, r"E:\tx\one", 10000).is_err())
    }
}
#[test]
fn nb3_js_output_exact_contract() {
    check_js(&output(), &oracle(PRE).unwrap()).unwrap();
    for mode in 0..9 {
        let mut r = output();
        match mode {
            0 => r.exit = None,
            1 => r.exit = Some(1),
            2 => r.stderr = b"error".to_vec(),
            3 => r.exited = false,
            4 => r.readers_done = false,
            5 => r.timed_out = true,
            6 => r.stdout.extend(b"\n{}"),
            7 => r.stdout = b"{\"bytes\":795,\"bytes\":795}".to_vec(),
            _ => r.stdout = b"{}".to_vec(),
        }
        assert!(check_js(&r, &oracle(PRE).unwrap()).is_err(), "{mode}")
    }
}
// Full production run with the actual observation adapter, Agent capability and bridge ports.
struct Full {
    tick: u64,
    export_bytes: Option<Vec<u8>>,
    observed: ObservationReview,
    port: MemoryCas,
    cas_calls: usize,
}
impl Ops for Full {
    fn now_ms(&self) -> u64 {
        self.tick
    }
    fn evidence(&mut self, _: &str) -> super::Result<()> {
        Ok(())
    }
    fn preflight(&mut self) -> super::Result<Preflight> {
        Ok(injected_preflight())
    }
    fn capture_start(&mut self) -> super::Result<()> {
        Ok(())
    }
    fn git_preflight(&mut self) -> super::Result<GitPreflight> {
        Ok(injected_git())
    }
    fn before_tree(&mut self) -> super::Result<TreeSnapshot> {
        Ok(injected_tree())
    }
    fn start_and_verify(&mut self) -> super::Result<ProcessIdentity> {
        Ok(target())
    }
    fn task(&mut self, s: &Arc<super::opencode_gate::RunState>) -> super::Result<Patch> {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(1);
        s.consume_prompt(end)?;
        s.admit("read", false, end)?;
        s.settle_injected("read", Some(1), end)?;
        s.read_completed(end)?;
        s.admit("edit", true, end)?;
        s.settle_injected("edit", Some(1), end)?;
        Ok(patch())
    }
    fn reject(&mut self, _: &[u8]) -> super::Result<()> {
        Ok(())
    }
    fn idle(&mut self) -> super::Result<bool> {
        Ok(true)
    }
    fn abort(&mut self) -> super::Result<()> {
        Ok(())
    }
    fn stop_job(&mut self) -> super::Result<JobStopReceipt> {
        Ok(injected_job())
    }
    fn capture_stop(&mut self) -> super::Result<()> {
        Ok(())
    }
    fn export_and_review(&mut self) -> super::Result<ObservationReview> {
        if let Some(bytes) = &self.export_bytes {
            self.observed = parsed(bytes)?.review;
        }
        Ok(self.observed.clone())
    }
    fn cas_and_verify(&mut self, p: &Patch) -> super::Result<CasReview> {
        self.cas_calls += 1;
        let a = ReviewedAgentTree::review(
            &injected_tree(),
            &self.observed.tree,
            &injected_preflight().fixture,
            &target(),
            FactSource::Injected,
        )?;
        let mut j = MemoryJs {
            calls: 0,
            result: output(),
            now: self.port.now.clone(),
            late: false,
        };
        bridge(
            &mut self.port,
            &mut j,
            &a,
            p,
            "p1",
            r"E:\tx\one",
            &assets(),
            30000,
        )
        .map(|r| r.review)
    }
    fn cleanup(&mut self) -> super::Result<()> {
        Ok(())
    }
}
#[test]
fn nb3_complete_synthetic_adapter_bridge_run_success() {
    let observed = parsed(&encode(finish(events()))).unwrap().review;
    let mut f = Full {
        tick: 0,
        export_bytes: None,
        observed,
        port: MemoryCas::new(),
        cas_calls: 0,
    };
    let o = run(
        &mut f,
        super::opencode_gate::RunState::new().unwrap(),
        "INJECTED_NB3",
    );
    assert!(o.success, "{:?}", o);
    assert_eq!(f.cas_calls, 1);
    assert!(!o.execution_authorized)
}
#[test]
fn nb3_run_observation_source_mismatch_stops_before_cas() {
    let mut observed = parsed(&encode(finish(events()))).unwrap().review;
    observed.source = None;
    let mut f = Full {
        tick: 0,
        export_bytes: None,
        observed,
        port: MemoryCas::new(),
        cas_calls: 0,
    };
    let state = super::opencode_gate::RunState::new().unwrap();
    let o = run(&mut f, state.clone(), "INJECTED_NB3");
    assert!(!o.success);
    assert_eq!(state.state(), super::opencode_gate::State::Stopped);
    assert_eq!(f.cas_calls, 0)
}
#[test]
fn nb3_observation_dropped_body_event_truncation_detected() {
    let b = encode(finish(events()));
    let mut ls: Vec<_> = b.split_inclusive(|x| *x == b'\n').collect();
    ls.remove(2);
    assert!(parsed(&ls.concat()).is_err())
}
#[test]
fn nb3_observation_windows_marker_and_process_descendant() {
    let mut es = events();
    if let Event::Write(x) = &mut es[1] {
        x.path = format!(r"{}\.git\opencode", ROOT)
    }
    let p = parsed(&encode(finish(es))).unwrap();
    let (r, t) = fixture();
    ReviewedAgentTree::review(&t, &p.review.tree, &r, &target(), FactSource::Injected).unwrap();
}
#[test]
fn nb3_observation_path_bound_and_backwards_reference() {
    let mut es = events();
    if let Event::Write(x) = &mut es[1] {
        x.path = "x".repeat(2049)
    }
    assert!(parsed(&encode(finish(es))).is_err());
    let mut es = events();
    if let Event::Network(x) = &mut es[2] {
        x.process.created = 99
    }
    assert!(parsed(&encode(finish(es))).is_err());
}
#[test]
fn nb3_observation_capture_clock_and_terminal_binding() {
    let b = String::from_utf8(encode(finish(events()))).unwrap();
    for (from, to) in [
        ("\"closed\":101", "\"closed\":99"),
        ("\"started\":1", "\"started\":100"),
        ("\"stopped\":100", "\"stopped\":1800000002"),
    ] {
        assert!(parsed(b.replace(from, to).as_bytes()).is_err())
    }
}
#[test]
fn nb3_nova_time_upper_and_lower_rejected() {
    for t in [799999, 1100001] {
        let mut n = nova();
        let f = n.files.iter_mut().find(|f| f.path == "Logo.tsx").unwrap();
        f.change = t;
        assert!(review_nova_tree(
            &agent(),
            &n,
            &receipt(),
            &oracle(PRE).unwrap(),
            1_000_000,
            1_100_000
        )
        .is_err())
    }
}
#[test]
fn nb3_run_logo_restored_stops_with_zero_cas() {
    let mut es = events();
    es.push(Event::Write(Write {
        at: 30,
        process: target(),
        path: TARGET.into(),
        kind: WriteKind::Overwrite,
        open_access: None,
    }));
    let mut f = Full {
        tick: 0,
        export_bytes: None,
        observed: parsed(&encode(finish(es))).unwrap().review,
        port: MemoryCas::new(),
        cas_calls: 0,
    };
    let s = super::opencode_gate::RunState::new().unwrap();
    let o = run(&mut f, s.clone(), "INJECTED_NB3");
    assert!(!o.success);
    assert_eq!(s.state(), super::opencode_gate::State::Stopped);
    assert_eq!(f.cas_calls, 0)
}
#[test]
fn nb3_run_recovered_historical_not_success() {
    let mut f = Full {
        tick: 0,
        export_bytes: None,
        observed: parsed(&encode(finish(events()))).unwrap().review,
        port: MemoryCas::new(),
        cas_calls: 0,
    };
    f.port.point = "recovered";
    let o = run(
        &mut f,
        super::opencode_gate::RunState::new().unwrap(),
        "INJECTED_NB3",
    );
    assert!(!o.success);
    assert_eq!(o.journal.unwrap().state, JournalState::RecoveredHistorical)
}
#[test]
fn nb3_js_wrong_bytes_hash_and_unknown_fields() {
    for s in [
        "{\"scope\":\"wrong\",\"bytes\":795,\"sha256\":\"bad\"}".to_string(),
        String::from_utf8(output().stdout)
            .unwrap()
            .replace("795", "794"),
        String::from_utf8(output().stdout)
            .unwrap()
            .replacen('{', "{\"unknown\":true,", 1),
    ] {
        let mut o = output();
        o.stdout = s.into_bytes();
        assert!(check_js(&o, &oracle(PRE).unwrap()).is_err())
    }
}
#[test]
fn nb3_no_recovery_surface_or_calls() {
    let code = include_str!("opencode_cas_bridge.rs");
    assert!(!code.contains("::recover("));
    assert!(!code.contains("::recover_with_fault("));
    assert!(!code.contains("fn recover("));
}

fn ownership() -> Vec<OwnershipReceipt> {
    [
        OwnershipKind::NoPriorCapture,
        OwnershipKind::Started,
        OwnershipKind::Stopped,
        OwnershipKind::OutputsClosed,
    ]
    .into_iter()
    .zip([0, 1, 100, 101])
    .map(|(kind, at)| OwnershipReceipt {
        source: FactSource::Injected,
        run: "M0B".into(),
        attempt: "p1".into(),
        generation: "INJECTED".into(),
        capture: "lease".into(),
        kind,
        at,
    })
    .collect()
}
#[test]
fn nb3_observation_typed_ownership_missing_duplicate_wrong_source_time() {
    let (r, t) = fixture();
    let target = target();
    for mode in 0..5 {
        let mut o = ownership();
        match mode {
            0 => {
                o.pop();
            }
            1 => o[0].kind = OwnershipKind::Started,
            2 => o[0].source = FactSource::NativePrepared,
            3 => o[0].generation = "other".into(),
            _ => o[0].at = 2,
        }
        let b = Binding {
            run: "M0B",
            attempt: "p1",
            capture: "lease",
            generation: "INJECTED",
            ownership: &o,
            model_port: 1234,
            target: &target,
        };
        assert!(
            parse(&encode(finish(events())), &b, &r, t.clone()).is_err(),
            "{mode}"
        )
    }
}
// B-1: exports are parsed inside the real run; rejection must never reach the CAS port.
fn assert_export_rejected_before_cas(es: Vec<Event>, code: &str) {
    let bytes = encode(finish(es));
    assert_eq!(parsed(&bytes).err().unwrap(), code);
    let mut full = Full {
        tick: 0,
        export_bytes: Some(bytes),
        observed: parsed(&encode(finish(events()))).unwrap().review,
        port: MemoryCas::new(),
        cas_calls: 0,
    };
    let state = super::opencode_gate::RunState::new().unwrap();
    let out = run(&mut full, state.clone(), "INJECTED_NB3_REMEDIATION");
    assert!(!out.success);
    assert!(!out.cas_started);
    assert_eq!(out.error.as_deref(), Some(code));
    assert_eq!(state.state(), super::opencode_gate::State::Stopped);
    assert_eq!(full.cas_calls, 0);
    assert!(!full.port.prepared);
}
fn foreign_write(path: &str) -> Vec<Event> {
    let mut es = events();
    es.push(Event::Write(Write {
        at: 30,
        process: ProcessIdentity {
            pid: 99,
            created: 3,
        },
        path: path.into(),
        kind: WriteKind::Overwrite,
        open_access: None,
    }));
    es
}
#[test]
fn nb3_b1_foreign_aliases_and_fixture_paths_run_reject_zero_cas() {
    let paths = [
        r"C:/NovaM0B/run/project/Logo.tsx".to_string(),
        r"\??\C:\NovaM0B\run\project\Logo.tsx".to_string(),
        r"C:\NOVAM0~1\run\project\Logo.tsx".to_string(),
        r"\\.\C:\NovaM0B\run\project\Logo.tsx".to_string(),
        r"\Device\HarddiskVolume3\NovaM0B\run\project\Logo.tsx".to_string(),
        r"\\localhost\C$\NovaM0B\run\project\Logo.tsx".to_string(),
        TARGET.to_string(),
        TARGET.to_lowercase(),
        format!(r"\\?\{}", TARGET),
    ];
    for path in paths {
        assert_export_rejected_before_cas(foreign_write(&path), "OBS_FOREIGN_FIXTURE_WRITE");
    }
}
#[test]
fn nb3_b1_foreign_abnormal_components_and_ads_reject() {
    for path in [
        r"E:\..\Logo.tsx",
        r"E:\foo\.\Logo.tsx",
        r"E:\foo.\Logo.tsx",
        r"E:\foo \Logo.tsx",
        r"E:\foo\\Logo.tsx",
        r"E:\foo\Logo.tsx:ads",
        r"E:Logo.tsx",
        r"E:\NUL\Logo.tsx",
        r"E:\foo\a?b",
        r"E:\foo\CON.txt",
        r"E:\foo\a~1",
    ] {
        assert_export_rejected_before_cas(foreign_write(path), "OBS_FOREIGN_FIXTURE_WRITE");
    }
}
#[test]
fn nb3_b1_confirmed_foreign_write_kept_and_success_chain_retained() {
    for path in [
        r"E:\other\file",
        r"e:\Other\File",
        r"C:\NovaM0B\run\project-other\Logo.tsx",
    ] {
        let bytes = encode(finish(foreign_write(path)));
        let p = parsed(&bytes).unwrap();
        assert_eq!(p.retained_events.len(), 7);
        assert_eq!(p.review.tree.writes.len(), 1);
        let mut f = Full {
            tick: 0,
            export_bytes: Some(bytes),
            observed: p.review,
            port: MemoryCas::new(),
            cas_calls: 0,
        };
        let out = run(
            &mut f,
            super::opencode_gate::RunState::new().unwrap(),
            "INJECTED_NB3_REMEDIATION",
        );
        assert!(out.success, "{out:?}");
        assert_eq!(f.cas_calls, 1);
    }
}
#[test]
fn nb3_b1_undeclared_network_and_pid_creation_mismatch_run_reject() {
    for process in [
        ProcessIdentity {
            pid: 99,
            created: 3,
        },
        ProcessIdentity {
            pid: 17,
            created: 2,
        },
    ] {
        let mut es = events();
        es.push(Event::Network(Network {
            at: 30,
            process,
            protocol: "TCP".into(),
            address: "8.8.8.8".into(),
            port: 443,
        }));
        assert_export_rejected_before_cas(es, "OBS_NETWORK_PROCESS_UNDECLARED");
    }
    let mut es = events();
    es.push(Event::Process(Process {
        at: 5,
        process: ProcessIdentity {
            pid: 99,
            created: 3,
        },
        parent: None,
    }));
    es.push(Event::Network(Network {
        at: 30,
        process: ProcessIdentity {
            pid: 99,
            created: 4,
        },
        protocol: "TCP".into(),
        address: "127.0.0.1".into(),
        port: 1234,
    }));
    assert_export_rejected_before_cas(es, "OBS_NETWORK_PROCESS_UNDECLARED");
}
#[test]
fn nb3_b1_declared_outside_network_retained() {
    let mut es = events();
    es.push(Event::Process(Process {
        at: 5,
        process: ProcessIdentity {
            pid: 99,
            created: 3,
        },
        parent: None,
    }));
    es.push(Event::Network(Network {
        at: 30,
        process: ProcessIdentity {
            pid: 99,
            created: 3,
        },
        protocol: "TCP".into(),
        address: "8.8.8.8".into(),
        port: 443,
    }));
    let p = parsed(&encode(finish(es))).unwrap();
    assert_eq!(p.retained_events.len(), 8);
    assert_eq!(p.review.tree.descendants.len(), 0);
}
#[test]
fn nb3_n1_final_status_error_and_drift_revoke_verified_preserve_facts() {
    for (point, expected) in [
        ("final-status-error", JournalState::Uncertain),
        ("final-status-prepared", JournalState::Prepared),
        ("final-status-recovered", JournalState::RecoveredHistorical),
        ("final-status-unknown", JournalState::Uncertain),
    ] {
        let mut p = MemoryCas::new();
        p.point = point;
        let (o, c) = invoke(&mut p);
        let o = o.unwrap();
        assert_eq!(p.status_calls, 2, "{point}");
        assert_eq!(p.apply_calls, 1);
        assert_eq!(p.trees, 1);
        assert_eq!(c, 1);
        assert_eq!(o.review.state, expected);
        assert!(o.review.receipt);
        assert!(!o.review.independently_verified);
        assert_eq!(o.code, "CAS_FINAL_STATUS_NOT_COMMITTED");
        assert!(!o.partial);
    }
}
#[test]
fn nb3_n6_committed_receipt_mismatch_hits_receipt_check() {
    let mut p = MemoryCas::new();
    p.point = "receipt-mismatch";
    let (o, c) = invoke(&mut p);
    let o = o.unwrap();
    assert_eq!(p.apply_calls, 1);
    assert_eq!(p.state, "COMMITTED_HISTORICAL");
    assert_eq!(p.status_calls, 2);
    assert_eq!(o.review.state, JournalState::CommittedHistorical);
    assert_eq!(o.code, "CAS_RECEIPT_MISMATCH");
    assert!(!o.review.receipt);
    assert!(!o.review.independently_verified);
    assert_eq!(c, 0);
    assert_eq!(p.trees, 0);
}
#[test]
fn nb3_n6_only_old_logo_indistinguishable_time_is_rejected() {
    let low = 800000;
    for field in ["write", "change"] {
        for old_time in [low - 1, low] {
            let (r, mut before) = fixture();
            let logo = before
                .files
                .iter_mut()
                .find(|f| f.path == "Logo.tsx")
                .unwrap();
            if field == "write" {
                logo.write = old_time;
            } else {
                logo.change = old_time;
            }
            let fact = AgentTreeReceipt {
                after: before.clone(),
                descendants: vec![],
                writes: vec![],
            };
            let reviewed =
                ReviewedAgentTree::review(&before, &fact, &r, &target(), FactSource::Injected)
                    .unwrap();
            // Nova timestamps remain valid in the same [800000,1100000] window. All other facts unchanged.
            let result = review_nova_tree(
                &reviewed,
                &nova(),
                &receipt(),
                &oracle(PRE).unwrap(),
                1_000_000,
                1_100_000,
            );
            if old_time < low {
                result.unwrap();
            } else {
                assert_eq!(result.unwrap_err(), "NOVA_TIME_INDISCERNIBLE", "{field}");
            }
        }
    }
}
