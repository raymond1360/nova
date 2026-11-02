//! NB1 tests use injected bytes/HTTP/events only: no socket or OS entropy calls.
use super::{
    opencode_broker::Broker,
    opencode_control::*,
    opencode_gate::{RunState, State},
    opencode_nonce::{Model, RunNonce, RunSecrets},
    opencode_paths::*,
    opencode_probe::*,
    opencode_protocol::{Control, ControlBudget, EventStream},
    Result,
};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
fn rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}
fn limit() -> Instant {
    Instant::now() + Duration::from_secs(2)
}
fn secrets() -> RunSecrets {
    RunSecrets {
        model: RunNonce::injected(|v| {
            v.fill(0x11);
            Ok(())
        })
        .unwrap(),
        control: RunNonce::injected(|v| {
            v.fill(0x22);
            Ok(())
        })
        .unwrap(),
    }
}
fn id(s: &str) -> Id {
    Id::parse(s).unwrap()
}
fn env(t: &str, mut p: Value) -> Vec<u8> {
    // Every PartUpdated fixture uses the real outer sessionID/part/time envelope.
    if t == "message.part.updated" && p.get("part").is_some() {
        let session = p["part"]["sessionID"].clone();
        p.as_object_mut()
            .unwrap()
            .entry("sessionID")
            .or_insert(session);
        p.as_object_mut().unwrap().entry("time").or_insert(json!(0));
    }
    format!("data: {}\n\n", json!({"type":t,"properties":p})).into_bytes()
}
fn permission(edit: bool) -> Value {
    json!({"id":if edit{"per_edit"}else{"per_read"},"sessionID":"ses_fixed","permission":if edit{"edit"}else{"read"},"patterns":["Logo.tsx"],"metadata":if edit{json!({"filepath":TARGET})}else{json!({})},"always":[],"tool":{"messageID":if edit{"msg_edit"}else{"msg_read"},"callID":if edit{"call_edit"}else{"call_read"}}})
}
fn part(edit: bool, status: &str) -> Value {
    json!({"type":"tool","id":"prt_fixed","sessionID":"ses_fixed","messageID":if edit{"msg_edit"}else{"msg_read"},"callID":if edit{"call_edit"}else{"call_read"},"tool":if edit{"edit"}else{"read"},"state":{"status":status,"input":if edit{json!({"filePath":TARGET,"oldString":"alt=\"Payload Logo\"","newString":"alt=\"Northstar Studio logo\"","replaceAll":false})}else{json!({"filePath":TARGET})},"error":"The user rejected permission to use this specific tool call."}})
}
struct Scripted {
    tx: mpsc::Sender<Result<Vec<u8>>>,
    rx: Mutex<Option<mpsc::Receiver<Result<Vec<u8>>>>>,
    calls: Mutex<Vec<(String, Option<Value>)>>,
    broker: Arc<Broker>,
    model: RunNonce<Model>,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
    model_results: Arc<Mutex<Vec<bool>>>,
    fail: Mutex<Option<(String, u16, bool)>>,
    event_status: u16,
    event_challenge: bool,
    event_sse: bool,
    bad_root: bool,
    agent_rules: Value,
    early_edit: bool,
    delay: Duration,
    pending: AtomicUsize,
    emitted: Arc<AtomicUsize>,
    omit_error: bool,
    omit_read_ack: bool,
    raw_error: bool,
    message_event: Option<Vec<u8>>,
    message_delay: Duration,
    upstream_sequence: bool,
    pause_http_until_stopped: bool,
    injection: Option<Value>,
    reader_released: Arc<AtomicUsize>,
    sent: Arc<Mutex<Vec<String>>>,
}
impl Scripted {
    fn new(s: Arc<RunState>) -> Self {
        let (tx, rx) = mpsc::channel();
        let secrets = secrets();
        let b = Broker::from_run(s, &secrets.model).unwrap();
        Self {
            tx,
            rx: Mutex::new(Some(rx)),
            calls: Mutex::new(vec![]),
            broker: Arc::new(b),
            model: secrets.model,
            workers: Mutex::new(vec![]),
            model_results: Arc::new(Mutex::new(vec![])),
            fail: Mutex::new(None),
            event_status: 200,
            event_challenge: false,
            event_sse: true,
            bad_root: false,
            agent_rules: frozen_permissions(),
            early_edit: false,
            delay: Duration::from_millis(3),
            pending: AtomicUsize::new(0),
            emitted: Arc::new(AtomicUsize::new(0)),
            omit_error: false,
            omit_read_ack: false,
            raw_error: false,
            message_event: None,
            message_delay: Duration::ZERO,
            upstream_sequence: false,
            pause_http_until_stopped: false,
            injection: None,
            reader_released: Arc::new(AtomicUsize::new(0)),
            sent: Arc::new(Mutex::new(vec![])),
        }
    }
    fn send(&self, t: &str, p: Value) {
        self.sent.lock().unwrap().push(event_label(t, &p));
        let _ = self.tx.send(Ok(env(t, p)));
    }
    fn model_request(&self, edit: bool) -> Vec<u8> {
        self.model
            .with_bytes(|n| super::opencode_broker::fixed_request(edit, n).unwrap())
    }
    fn join(&self) {
        for h in self.workers.lock().unwrap().drain(..) {
            h.join().unwrap();
        }
    }
}
impl ControlTransport for Scripted {
    fn request(&self, r: Request, d: Instant) -> Async<'_, Response> {
        Box::pin(async move {
            // Authentication is asserted on EVERY request, including health; never printed.
            assert!(r.auth.is_sensitive());
            assert_eq!(
                r.auth.as_bytes(),
                super::opencode_nonce::basic(&secrets().control)
                    .unwrap()
                    .as_bytes()
            );
            assert!(!r.url.contains("auth_token"));
            assert!(r.url.contains("directory=C%3A%5CNovaM0B"));
            if self.raw_error {
                return Err(secrets()
                    .control
                    .with_bytes(|n| String::from_utf8(n.to_vec()).unwrap()));
            }
            let path = r
                .url
                .split('?')
                .next()
                .unwrap()
                .strip_prefix("http://127.0.0.1:7001")
                .unwrap()
                .to_string();
            self.calls
                .lock()
                .unwrap()
                .push((path.clone(), r.body.clone()));
            if let Some((p, status, auth)) = self.fail.lock().unwrap().clone() {
                if path == p {
                    return Ok(Response {
                        status,
                        authenticate: auth,
                        body: b"{}".to_vec(),
                    });
                }
            }
            let value = match path.as_str() {
                "/global/health" => json!({"healthy":true,"version":"1.19.4"}),
                "/path" => {
                    json!({"directory":if self.bad_root{"C:\\other"}else{ROOT},"worktree":ROOT})
                }
                "/project/current" => json!({"worktree":ROOT}),
                "/agent" => json!([{"name":"nova_probe","permission":self.agent_rules.clone()}]),
                "/permission" => json!([]),
                "/session/status" => json!({}),
                "/session" => json!({"id":"ses_fixed","directory":ROOT}),
                "/session/ses_fixed/prompt_async" => {
                    assert!(self
                        .broker
                        .request(&self.model_request(false), d)?
                        .starts_with(b"HTTP/1.1 200"));
                    if self.upstream_sequence {
                        for (t, p) in upstream_start(false) {
                            self.send(&t, p);
                        }
                    }
                    if let Some(v) = &self.injection {
                        self.sent
                            .lock()
                            .unwrap()
                            .push(event_label(v["type"].as_str().unwrap(), &v["properties"]));
                        let _ = self.tx.send(Ok(format!("data: {v}\n\n").into_bytes()));
                    }
                    self.send("permission.asked", permission(false));
                    Value::Null
                }
                "/session/ses_fixed/message/msg_read" => {
                    let pending = self
                        .pending
                        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |x| x.checked_sub(1))
                        .is_ok();
                    // HTTP sleeps while the independent event pump still consumes heartbeat.
                    let before = self.emitted.load(Ordering::SeqCst);
                    if let Some(bytes) = &self.message_event {
                        self.tx.send(Ok(bytes.clone())).unwrap();
                    } else {
                        self.send("server.heartbeat", json!({}));
                    }
                    if self.pause_http_until_stopped {
                        while self.broker.state.state() != State::Stopped {
                            if Instant::now() >= d {
                                return Err("injected synchronization deadline".into());
                            }
                            tokio::task::yield_now().await;
                        }
                    } else {
                        tokio::time::sleep(Duration::from_millis(2) + self.message_delay).await;
                    }
                    // The reader, not this awaiting HTTP dispatcher, consumed the frame.
                    assert!(self.emitted.load(Ordering::SeqCst) > before);
                    json!({"parts":[part(false,if pending{"pending"}else{"running"})]})
                }
                "/session/ses_fixed/message/msg_edit" => json!({"parts":[part(true,"running")]}),
                "/permission/per_read/reply" => {
                    assert_eq!(r.body, Some(json!({"reply":"once"})));
                    if !self.omit_read_ack {
                        self.send(
                            "permission.replied",
                            json!({"sessionID":"ses_fixed","requestID":"per_read","reply":"once"}),
                        );
                    }
                    let b = self.broker.clone();
                    let raw = self.model_request(true);
                    let tx = self.tx.clone();
                    let results = self.model_results.clone();
                    let early = self.early_edit;
                    let upstream = self.upstream_sequence;
                    let sent = self.sent.clone();
                    // Independent broker thread starts BEFORE completed-read is emitted.
                    let (ready_tx, ready_rx) = mpsc::channel();
                    let h = std::thread::spawn(move || {
                        ready_tx.send(()).unwrap();
                        let result = b.request(&raw, d);
                        let ok = result.is_ok_and(|v| v.starts_with(b"HTTP/1.1 200"));
                        results.lock().unwrap().push(ok);
                        if ok && !early {
                            if upstream {
                                for (t, p) in upstream_finish(false)
                                    .into_iter()
                                    .chain(upstream_start(true))
                                {
                                    sent.lock().unwrap().push(event_label(&t, &p));
                                    let _ = tx.send(Ok(env(&t, p)));
                                }
                            }
                            sent.lock()
                                .unwrap()
                                .push(event_label("permission.asked", &permission(true)));
                            let _ = tx.send(Ok(env("permission.asked", permission(true))));
                        }
                    });
                    self.workers.lock().unwrap().push(h);
                    ready_rx.recv_timeout(Duration::from_secs(1)).unwrap();
                    let began = Instant::now();
                    while !self.broker.state.waiting_for_read() {
                        assert!(began.elapsed() < Duration::from_secs(1));
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    assert_eq!(self.broker.state.ledger.lock().unwrap().budget.admitted, 1);
                    if self.early_edit {
                        self.send("permission.asked", permission(true));
                    }
                    let tx = self.tx.clone();
                    let delay = self.delay;
                    let sent = self.sent.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        sent.lock()
                            .unwrap()
                            .push("message.part.updated/tool/read/completed".into());
                        let _ = tx.send(Ok(env(
                            "message.part.updated",
                            json!({"part":part(false,"completed")}),
                        )));
                    });
                    json!(true)
                }
                "/permission/per_edit/reply" => {
                    assert_eq!(r.body, Some(json!({"reply":"reject"})));
                    assert_eq!(self.broker.state.state(), State::Stopped);
                    self.send(
                        "permission.replied",
                        json!({"sessionID":"ses_fixed","requestID":"per_edit","reply":"reject"}),
                    );
                    if !self.omit_error {
                        self.send("message.part.updated", json!({"part":part(true,"error")}));
                    }
                    if self.upstream_sequence {
                        for (t, p) in upstream_finish(true) {
                            self.send(&t, p);
                        }
                    }
                    self.send(
                        "session.status",
                        json!({"sessionID":"ses_fixed","status":{"type":"idle"}}),
                    );
                    self.send("session.idle", json!({"sessionID":"ses_fixed"}));
                    json!(true)
                }
                "/session/ses_fixed/abort" => json!(true),
                _ => return Err("unexpected fixture route".into()),
            };
            Ok(Response {
                status: if path.ends_with("prompt_async") {
                    204
                } else {
                    200
                },
                authenticate: false,
                body: if value.is_null() {
                    vec![]
                } else {
                    serde_json::to_vec(&value).unwrap()
                },
            })
        })
    }
    fn event(&self, r: Request, _: Instant) -> Async<'_, EventResponse> {
        Box::pin(async move {
            assert!(r.auth.is_sensitive());
            assert!(r.url.contains("/event?"));
            let rx = self.rx.lock().unwrap().take().ok_or("event reopened")?;
            self.send("server.connected", json!({}));
            let count = self.emitted.clone();
            let lease = ReaderLease(self.reader_released.clone());
            Ok(EventResponse {
                status: self.event_status,
                authenticate: self.event_challenge,
                sse_content_type: self.event_sse,
                stream: Box::pin(futures_util::stream::unfold(
                    (rx, count, lease),
                    |(rx, c, lease)| async move {
                        loop {
                            match rx.try_recv() {
                                Ok(v) => {
                                    c.fetch_add(1, Ordering::SeqCst);
                                    return Some((v, (rx, c, lease)));
                                }
                                Err(mpsc::TryRecvError::Empty) => {
                                    tokio::time::sleep(Duration::from_millis(1)).await
                                }
                                Err(_) => return None,
                            }
                        }
                    },
                )) as EventBytes,
            })
        })
    }
}
fn driver(t: Arc<Scripted>, s: Arc<RunState>, d: Instant) -> ControlDriver<Scripted> {
    ControlDriver::new(
        t,
        Endpoint::registered("http://127.0.0.1:7001", 7002).unwrap(),
        secrets().control,
        s,
        d,
    )
}
struct Harness {
    runtime: tokio::runtime::Runtime,
    driver: ControlDriver<Scripted>,
    transport: Arc<Scripted>,
    state: Arc<RunState>,
    before: TreeSnapshot,
    starts: usize,
    cas: bool,
    start: Instant,
}
impl Harness {
    fn new() -> Self {
        Self::configured(|_| {})
    }
    fn configured(configure: impl FnOnce(&mut Scripted)) -> Self {
        let s = RunState::new().unwrap();
        let mut scripted = Scripted::new(s.clone());
        configure(&mut scripted);
        let t = Arc::new(scripted);
        Self {
            runtime: rt(),
            driver: driver(t.clone(), s.clone(), limit()),
            transport: t,
            state: s,
            before: injected_tree(),
            starts: 0,
            cas: false,
            start: Instant::now(),
        }
    }
    fn run(&mut self) -> Outcome {
        run(self, self.state.clone(), "INJECTED_NB1")
    }
}
impl Ops for Harness {
    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis() as u64
    }
    fn evidence(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn preflight(&mut self) -> Result<Preflight> {
        Ok(injected_preflight())
    }
    fn capture_start(&mut self) -> Result<()> {
        Ok(())
    }
    fn git_preflight(&mut self) -> Result<GitPreflight> {
        Ok(injected_git())
    }
    fn before_tree(&mut self) -> Result<TreeSnapshot> {
        Ok(self.before.clone())
    }
    fn start_and_verify(&mut self) -> Result<ProcessIdentity> {
        self.starts += 1;
        assert_eq!(self.state.state(), State::Preflight);
        self.runtime.block_on(self.driver.prepare())?;
        Ok(injected_target())
    }
    fn task(&mut self, s: &Arc<RunState>) -> Result<super::contract::Patch> {
        assert!(Arc::ptr_eq(s, &self.state));
        self.runtime.block_on(self.driver.task())
    }
    fn reject(&mut self, b: &[u8]) -> Result<()> {
        assert_eq!(b, br#"{"reply":"reject"}"#);
        self.runtime.block_on(self.driver.reject_and_idle())?;
        Ok(())
    }
    fn idle(&mut self) -> Result<bool> {
        Ok(self.driver.confirm_idle()?.event_confirmed)
    }
    fn abort(&mut self) -> Result<()> {
        self.runtime.block_on(self.driver.abort())?;
        Ok(())
    }
    fn stop_job(&mut self) -> Result<JobStopReceipt> {
        Ok(injected_job())
    }
    fn capture_stop(&mut self) -> Result<()> {
        Ok(())
    }
    fn export_and_review(&mut self) -> Result<ObservationReview> {
        Ok(injected_observation())
    }
    fn cas_and_verify(&mut self, p: &super::contract::Patch) -> Result<CasReview> {
        p.validate("M0B", "per_edit", super::PRE)?;
        self.cas = true;
        Ok(CasReview {
            state: JournalState::CommittedHistorical,
            receipt: true,
            independently_verified: true,
        })
    }
    fn cleanup(&mut self) -> Result<()> {
        Ok(())
    }
}
#[test]
fn driver_permission_flow_run_complete_injected_chain() {
    let mut h = Harness::new();
    let o = h.run();
    h.transport.join();
    assert!(o.success, "{:?}", o.error);
    assert!(h.cas);
    assert!(h.driver.facts.ready.event_connected);
    assert!(h.driver.facts.read_reply.event_confirmed);
    assert!(h.driver.facts.edit_reply.tool_error_confirmed);
    assert!(h.driver.facts.idle.event_confirmed);
    assert_eq!(h.starts, 1);
    assert_eq!(*h.transport.model_results.lock().unwrap(), [true]);
    let l = h.state.ledger.lock().unwrap();
    assert_eq!(
        (l.budget.received, l.budget.admitted, l.budget.denied),
        (2, 2, 0)
    );
}
#[test]
fn r1_run_marker_missing_and_invalid_before_start_and_cas() {
    for mode in 0..8 {
        let mut h = Harness::new();
        let i = h
            .before
            .files
            .iter()
            .position(|f| f.path == ".git/opencode")
            .unwrap();
        match mode {
            0 => {
                h.before.files.remove(i);
            }
            1 => h.before.files[i].bytes[0] = b'b',
            2 => h.before.files[i].bytes.push(b'\n'),
            3 => h.before.files[i].attributes = 0,
            4 => h.before.files[i].ads.push(("x".into(), vec![1])),
            5 => h.before.files[i].owner = "other".into(),
            6 => h.before.files[i].identity = "replacement".into(),
            _ => h.before.files[i].identity.clear(),
        };
        let o = h.run();
        assert!(!o.success);
        assert_eq!(h.starts, 0);
        assert!(!h.cas);
        assert_eq!(h.state.state(), State::Stopped);
    }
}
#[test]
fn basic_auth_all_requests_and_no_query_fallback() {
    let mut h = Harness::new();
    assert!(h.run().success);
    h.transport.join();
    let c = h.transport.calls.lock().unwrap();
    assert!(c.iter().any(|(p, _)| p == "/global/health"));
    assert_eq!(
        c.iter()
            .filter(|(p, _)| p.ends_with("prompt_async"))
            .count(),
        1
    );
    assert_eq!(
        c.iter()
            .filter(|(p, b)| p.ends_with("reply") && *b == Some(json!({"reply":"reject"})))
            .count(),
        1
    );
}
#[test]
fn authentication_401_or_challenge_stops_without_prompt() {
    for (status, auth) in [(401, false), (200, true)] {
        let s = RunState::new().unwrap();
        let t = Arc::new(Scripted::new(s.clone()));
        *t.fail.lock().unwrap() = Some(("/global/health".into(), status, auth));
        let mut d = driver(t.clone(), s.clone(), limit());
        assert!(rt()
            .block_on(d.prepare())
            .unwrap_err()
            .contains("authentication"));
        assert_eq!(s.state(), State::Stopped);
        assert!(!d.facts.prompt_decision);
        assert_eq!(t.calls.lock().unwrap().len(), 1);
    }
}
#[test]
fn no_redirect_retry_or_bad_root_fallback() {
    for code in [301, 302, 307, 308, 429, 500] {
        let s = RunState::new().unwrap();
        let t = Arc::new(Scripted::new(s.clone()));
        *t.fail.lock().unwrap() = Some(("/global/health".into(), code, false));
        let mut d = driver(t.clone(), s.clone(), limit());
        assert!(rt().block_on(d.prepare()).is_err());
        assert_eq!(t.calls.lock().unwrap().len(), 1);
        assert_eq!(s.state(), State::Stopped);
    }
    let s = RunState::new().unwrap();
    let mut t = Scripted::new(s.clone());
    t.bad_root = true;
    let mut d = driver(Arc::new(t), s.clone(), limit());
    assert!(rt().block_on(d.prepare()).is_err());
    assert_eq!(s.state(), State::Stopped);
}
#[test]
fn endpoint_ids_and_port_accounts_are_closed() {
    for p in [
        "https://127.0.0.1:7001",
        "http://localhost:7001",
        "http://127.0.0.1:0",
        "http://127.0.0.1:07001",
        "http://127.0.0.1:7001/",
        "http://127.0.0.1:7001?auth_token=x",
        "http://127.0.0.1:7002",
    ] {
        assert!(Endpoint::registered(p, 7002).is_err());
    }
    for bad in ["", "\n", "a\rX"] {
        assert!(Id::parse(bad).is_err());
    }
    assert!(Id::parse(&"x".repeat(129)).is_err());
    let secret = secrets().control;
    let request = Request::new_for_test(&secret, &id("x/y?z#%"));
    assert!(request.contains("x%2Fy%3Fz%23%25"));
}
#[test]
fn double_nonce_purposes_independent_and_entropy_failure() {
    let s = secrets();
    s.distinct().unwrap();
    assert!(RunNonce::<Model>::injected(|_| Err("injected entropy failure".into())).is_err());
    for reserved in [
        super::opencode_broker::INJECTED_NONCE,
        b"UNPREPARED_RUN_NONCE",
        b"wrong",
    ] {
        assert!(RunNonce::<Model>::validate_external(reserved).is_err());
    }
    let equal = RunSecrets {
        model: RunNonce::injected(|v| {
            v.fill(1);
            Ok(())
        })
        .unwrap(),
        control: RunNonce::injected(|v| {
            v.fill(1);
            Ok(())
        })
        .unwrap(),
    };
    assert!(equal.distinct().is_err());
}
#[test]
fn basic_encoding_fixed_user_and_sensitive_header() {
    let n = secrets().control;
    let h = super::opencode_nonce::basic(&n).unwrap();
    assert!(h.is_sensitive());
    assert!(h.as_bytes().starts_with(b"Basic b3BlbmNvZGU6"));
    assert!(!format!("{h:?}").contains("b3BlbmNvZGU6"));
}
#[test]
fn credentials_never_appear_in_facts_or_errors() {
    let mut h = Harness::new();
    assert!(h.run().success);
    h.transport.join();
    let text = serde_json::to_string(&h.driver.facts).unwrap();
    for b in [b"11".repeat(32), b"22".repeat(32)] {
        assert!(!text.as_bytes().windows(b.len()).any(|x| x == b));
    }
    assert!(!text.contains("Authorization"));
}
#[test]
fn event_taxonomy_framing_and_bound_are_distinct() {
    let sid = id("ses_fixed");
    for kind in ["server.connected", "server.heartbeat"] {
        assert!(event(json!({"type":kind,"properties":{}}), Some(&sid)).is_ok());
    }
    assert!(matches!(
        event(
            json!({"type":"server.instance.disposed","properties":{}}),
            Some(&sid)
        )
        .unwrap(),
        Event::Disposed
    ));
    for kind in ["unknown", "external_directory", "global.event"] {
        assert!(event(json!({"type":kind,"properties":{}}), Some(&sid)).is_err());
    }
    let mut f = EventStream::new();
    assert!(f.push(b"event: message\ndata: {}\n\n").is_err());
    let mut f = EventStream::new();
    assert!(f.push(&vec![b'x'; 256 * 1024 + 1]).is_err());
}
#[test]
fn foreign_and_missing_event_fields_refused() {
    let sid = id("ses_fixed");
    for v in [
        json!({"type":"permission.asked","properties":{}}),
        json!({"type":"session.idle","properties":{"sessionID":"other"}}),
        json!({"type":"message.part.updated","properties":{"part":{}}}),
        json!({"type":"permission.replied","properties":{"sessionID":"ses_fixed"}}),
    ] {
        assert!(event(v, Some(&sid)).is_err());
    }
}
#[test]
fn controls_bound_health_and_single_subscription() {
    let mut b = ControlBudget::new();
    for _ in 0..5 {
        b.reserve(Control::Health).unwrap();
    }
    assert!(b.reserve(Control::Health).is_err());
    let mut b = ControlBudget::new();
    for _ in 0..40 {
        b.reserve(Control::Path).unwrap();
    }
    assert!(b.reserve(Control::Path).is_err());
    b.reserve(Control::Event).unwrap();
    assert!(b.reserve(Control::Event).is_err());
}
#[test]
fn run_consumes_prompt_decision_once_and_stop_wins() {
    let s = RunState::new().unwrap();
    assert!(s.consume_prompt(limit()).is_err());
    let s = RunState::new().unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    s.consume_prompt(limit()).unwrap();
    assert!(s.consume_prompt(limit()).is_err());
    let s = RunState::new().unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    s.stop();
    assert!(s.consume_prompt(limit()).is_err());
    assert!(s.open_once(true).is_err());
}
#[test]
fn pending_reread_bounded_and_event_pump_keeps_advancing() {
    let s = RunState::new().unwrap();
    let t = Arc::new(Scripted::new(s.clone()));
    t.pending.store(1, Ordering::SeqCst);
    let mut d = driver(t.clone(), s.clone(), limit());
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    runtime.block_on(d.task()).unwrap();
    t.join();
    assert!(t.emitted.load(Ordering::SeqCst) >= 5);
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("msg_read"))
            .count(),
        2
    );
}
#[test]
fn exhausted_reread_stops_and_aborts_once() {
    let s = RunState::new().unwrap();
    let t = Arc::new(Scripted::new(s.clone()));
    t.pending.store(4, Ordering::SeqCst);
    let mut d = driver(t.clone(), s.clone(), limit());
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    assert!(runtime.block_on(d.task()).is_err());
    assert_eq!(s.state(), State::Stopped);
    let calls = t.calls.lock().unwrap();
    assert_eq!(
        calls
            .iter()
            .filter(|(p, _)| p.ends_with("msg_read"))
            .count(),
        3
    );
    assert_eq!(
        calls.iter().filter(|(p, _)| p.ends_with("abort")).count(),
        1
    );
}
#[test]
fn late_read_completion_refuses_and_accounts_no_deadlock() {
    let s = RunState::new().unwrap();
    let mut t = Scripted::new(s.clone());
    t.delay = Duration::from_millis(150);
    let t = Arc::new(t);
    let mut d = driver(
        t.clone(),
        s.clone(),
        Instant::now() + Duration::from_millis(70),
    );
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    assert!(runtime.block_on(d.task()).is_err());
    t.join();
    assert_eq!(s.state(), State::Stopped);
    let l = s.ledger.lock().unwrap();
    assert_eq!(l.budget.admitted, 1);
    assert_eq!(l.budget.received, 2);
    assert_eq!(l.budget.denied, 1);
}
#[test]
fn third_model_request_is_denied_after_closed_proposal() {
    let mut h = Harness::new();
    assert!(h.run().success);
    h.transport.join();
    let r = h
        .transport
        .broker
        .request(&h.transport.model_request(false), limit())
        .unwrap();
    assert!(r.starts_with(b"HTTP/1.1 400"));
    let l = h.state.ledger.lock().unwrap();
    assert_eq!(
        (l.budget.received, l.budget.admitted, l.budget.denied),
        (3, 2, 1)
    );
}
#[test]
fn expired_prepare_does_not_send_or_open() {
    let s = RunState::new().unwrap();
    let t = Arc::new(Scripted::new(s.clone()));
    let mut d = driver(t.clone(), s.clone(), Instant::now());
    assert!(rt().block_on(d.prepare()).is_err());
    assert!(t.calls.lock().unwrap().is_empty());
    assert_eq!(s.state(), State::Stopped);
}
#[test]
fn stopped_after_prepare_never_dispatches_prompt() {
    let s = RunState::new().unwrap();
    let t = Arc::new(Scripted::new(s.clone()));
    let mut d = driver(t.clone(), s.clone(), limit());
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    s.stop();
    assert!(runtime.block_on(d.task()).is_err());
    assert!(!d.facts.prompt_decision);
    assert!(!t
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(p, _)| p.ends_with("prompt_async")));
}
#[test]
fn untrusted_transport_error_cannot_leak_secret() {
    let s = RunState::new().unwrap();
    let mut t = Scripted::new(s.clone());
    t.raw_error = true;
    let mut d = driver(Arc::new(t), s.clone(), limit());
    let e = rt().block_on(d.prepare()).unwrap_err();
    assert_eq!(e, "control transport/deadline failure");
    assert!(!serde_json::to_string(&d.facts)
        .unwrap()
        .contains(&"22".repeat(32)));
    assert_eq!(s.state(), State::Stopped);
}
#[test]
fn unknown_and_disposed_events_stop_while_http_is_pending() {
    for kind in ["unknown.type", "server.instance.disposed"] {
        let s = RunState::new().unwrap();
        let mut t = Scripted::new(s.clone());
        t.message_event = Some(env(kind, json!({})));
        t.message_delay = Duration::from_millis(10);
        let t = Arc::new(t);
        let mut d = driver(t.clone(), s.clone(), limit());
        let runtime = rt();
        runtime.block_on(d.prepare()).unwrap();
        s.open_once(true).unwrap();
        s.prompt_once(limit()).unwrap();
        assert!(runtime.block_on(d.task()).is_err());
        t.join();
        assert_eq!(s.state(), State::Stopped);
        assert!(d.confirm_idle().is_err());
        assert!(!t
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|(p, _)| p == "/permission/per_read/reply"));
        assert_eq!(
            d.pump_failure(),
            Some(if kind == "unknown.type" {
                PumpFault::UnknownEvent
            } else {
                PumpFault::InstanceDisposed
            })
        );
    }
}
#[test]
fn strict_partial_frame_and_event_four_mib_bound() {
    let mut p = EventStream::new();
    p.push(b"data: {\"type\":").unwrap();
    assert!(p.end().is_err());
    let mut p = EventStream::new();
    let bytes = format!("data: {}\n\n", json!({"padding":"a".repeat(32768)})).into_bytes();
    let mut failed = false;
    for _ in 0..129 {
        if p.push(&bytes).is_err() {
            failed = true;
            break;
        }
    }
    assert!(failed);
}
#[test]
fn unified_deadline_cancels_pending_http_and_no_retry() {
    let s = RunState::new().unwrap();
    let mut t = Scripted::new(s.clone());
    t.message_delay = Duration::from_millis(200);
    let t = Arc::new(t);
    let mut d = driver(
        t.clone(),
        s.clone(),
        Instant::now() + Duration::from_millis(60),
    );
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    let began = Instant::now();
    assert!(runtime.block_on(d.task()).is_err());
    assert!(began.elapsed() < Duration::from_secs(1));
    assert_eq!(s.state(), State::Stopped);
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("msg_read"))
            .count(),
        1
    );
}
#[test]
fn http_completion_without_read_event_ack_is_not_success() {
    let s = RunState::new().unwrap();
    let mut t = Scripted::new(s.clone());
    t.omit_read_ack = true;
    let t = Arc::new(t);
    let mut d = driver(t.clone(), s.clone(), limit());
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    runtime.block_on(d.task()).unwrap();
    t.join();
    s.proposal_ready(limit()).unwrap();
    s.stop();
    assert!(runtime.block_on(d.reject_and_idle()).is_err());
    assert!(!d.facts.read_reply.event_confirmed);
    assert!(d.facts.abort.http_complete);
    assert!(d.confirm_idle().is_err());
}
#[test]
fn http_reject_and_idle_without_tool_error_remains_unconfirmed() {
    let s = RunState::new().unwrap();
    let mut t = Scripted::new(s.clone());
    t.omit_error = true;
    let t = Arc::new(t);
    let mut d = driver(t.clone(), s.clone(), limit());
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    runtime.block_on(d.task()).unwrap();
    t.join();
    s.proposal_ready(limit()).unwrap();
    s.stop();
    d.shorten_deadline(Duration::from_millis(100));
    assert!(runtime.block_on(d.reject_and_idle()).is_err());
    assert!(d.facts.edit_reply.http_complete);
    assert!(d.facts.idle.event_confirmed);
    assert!(!d.facts.edit_reply.tool_error_confirmed);
    assert!(d.confirm_idle().is_err());
}
#[test]
fn repeated_task_does_not_dispatch_second_prompt() {
    let s = RunState::new().unwrap();
    let t = Arc::new(Scripted::new(s.clone()));
    let mut d = driver(t.clone(), s.clone(), limit());
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    runtime.block_on(d.task()).unwrap();
    t.join();
    assert!(runtime.block_on(d.task()).is_err());
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("prompt_async"))
            .count(),
        1
    );
    assert_eq!(s.state(), State::Stopped);
}
#[test]
fn driver_drop_stops_state_and_cancels_single_reader() {
    let s = RunState::new().unwrap();
    let t = Arc::new(Scripted::new(s.clone()));
    let released = t.reader_released.clone();
    let mut d = driver(t, s.clone(), Instant::now() + Duration::from_secs(3600));
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    assert_eq!(released.load(Ordering::SeqCst), 0);
    drop(d);
    runtime.block_on(async {
        let end = limit();
        while released.load(Ordering::SeqCst) == 0 {
            assert!(Instant::now() < end);
            tokio::task::yield_now().await;
        }
    });
    assert_eq!(released.load(Ordering::SeqCst), 1);
    assert_eq!(s.state(), State::Stopped);
    assert!(s.open_once(true).is_err());
}
#[test]
fn prompt_dispatch_stop_barrier_linearizations() {
    for mode in 0..3 {
        let s = RunState::new().unwrap();
        s.open_once(true).unwrap();
        s.prompt_once(limit()).unwrap();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let b = barrier.clone();
        let c = s.clone();
        let h = std::thread::spawn(move || {
            if mode == 0 {
                let ok = c.consume_prompt(limit()).is_ok();
                b.wait();
                ok
            } else {
                b.wait();
                c.consume_prompt(limit()).is_ok()
            }
        });
        if mode == 0 {
            barrier.wait();
            s.stop();
        } else if mode == 1 {
            s.stop();
            barrier.wait();
        } else {
            barrier.wait();
            s.stop();
        }
        let sent = h.join().unwrap();
        if mode == 0 {
            assert!(sent);
        }
        if mode == 1 {
            assert!(!sent);
        }
        assert_eq!(s.state(), State::Stopped);
        assert!(s.consume_prompt(limit()).is_err());
        assert!(s.open_once(true).is_err());
    }
}
#[test]
fn edit_permission_arrives_before_completed_read_is_queued_not_replied() {
    let s = RunState::new().unwrap();
    let mut t = Scripted::new(s.clone());
    t.early_edit = true;
    let t = Arc::new(t);
    let mut d = driver(t.clone(), s.clone(), limit());
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    s.open_once(true).unwrap();
    s.prompt_once(limit()).unwrap();
    let p = runtime.block_on(d.task()).unwrap();
    t.join();
    assert_eq!(p.attempt_id, "per_edit");
    assert!(!t
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(p, _)| p == "/permission/per_edit/reply"));
    assert_eq!(*t.model_results.lock().unwrap(), [true]);
    assert!(d.facts.read_reply.event_confirmed);
}

#[test]
fn inherited_defaults_are_sealed_but_implicit_truncate_allow_is_rejected() {
    let mut inherited = vec![
        json!({"permission":"*","pattern":"*","action":"allow"}),
        json!({"permission":"external_directory","pattern":"C:/outside/*","action":"ask"}),
    ];
    inherited.extend(frozen_permissions().as_array().unwrap().clone());
    assert!(closed_permissions(&Value::Array(inherited.clone())));
    inherited.push(
        json!({"permission":"external_directory","pattern":"C:/agent/truncate/*","action":"allow"}),
    );
    assert!(!closed_permissions(&Value::Array(inherited)));
    let mut v = frozen_permissions().as_array().unwrap().clone();
    v.push(json!({"permission":"read","pattern":"Logo.tsx","action":"deny"}));
    assert!(!closed_permissions(&Value::Array(v)));
    assert!(!closed_permissions(
        &json!([{"permission":"*","pattern":"*","action":"allow"}])
    ));
}

#[test]
fn implicit_external_allow_blocks_driver_before_session_and_prompt() {
    let state = RunState::new().unwrap();
    let mut transport = Scripted::new(state.clone());
    transport.agent_rules.as_array_mut().unwrap().push(
        json!({"permission":"external_directory","pattern":"C:/agent/truncate/*","action":"allow"}),
    );
    let t = Arc::new(transport);
    let mut d = driver(t.clone(), state.clone(), limit());
    assert!(rt().block_on(d.prepare()).is_err());
    assert_eq!(state.state(), State::Stopped);
    assert!(!d.facts.ready.session_created);
    assert!(!d.facts.prompt_decision);
    assert!(!t.calls.lock().unwrap().iter().any(|(p, _)| p == "/session"));
}

#[test]
fn event_http_status_and_challenge_are_not_connected_acknowledgement() {
    for (status, challenge, sse) in [
        (401, false, true),
        (200, true, true),
        (302, false, true),
        (200, false, false),
    ] {
        let state = RunState::new().unwrap();
        let mut t = Scripted::new(state.clone());
        t.event_status = status;
        t.event_challenge = challenge;
        t.event_sse = sse;
        let t = Arc::new(t);
        let mut d = driver(t.clone(), state.clone(), limit());
        let error = rt().block_on(d.prepare()).unwrap_err();
        if status == 401 || challenge {
            assert_eq!(error, "control authentication failed");
        }
        assert_eq!(d.facts.ready.event_status, Some(status));
        assert!(d.facts.ready.event_headers_received);
        assert!(!d.facts.ready.event_connected);
        assert_eq!(state.state(), State::Stopped);
        assert!(!d.facts.prompt_decision);
        assert!(d.facts.abort.http_complete);
    }
}
#[test]
fn abort_is_once_and_not_an_idle_or_job_receipt() {
    let state = RunState::new().unwrap();
    let t = Arc::new(Scripted::new(state.clone()));
    let mut d = driver(t.clone(), state.clone(), limit());
    let runtime = rt();
    runtime.block_on(d.prepare()).unwrap();
    let receipt = runtime.block_on(d.abort()).unwrap();
    assert!(receipt.decision && receipt.http_complete);
    assert!(runtime.block_on(d.abort()).is_err());
    assert!(d.confirm_idle().is_err());
    assert_eq!(state.state(), State::Stopped);
    assert_eq!(
        t.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| p.ends_with("abort"))
            .count(),
        1
    );
    assert_eq!(state.ledger.lock().unwrap().budget.admitted, 0);
}
#[test]
fn consumed_tool_lifecycle_does_not_grant_extra_permission() {
    let sid = id("ses_fixed");
    assert!(matches!(
        event(
            json!({"type":"message.part.updated","properties":{"sessionID":"ses_fixed","time":0,"part":part(false,"pending")}}),
            Some(&sid)
        )
        .unwrap(),
        Event::Part(_)
    ));
    assert!(matches!(event(json!({"type":"message.part.updated","properties":{"sessionID":"ses_fixed","time":0,"part":{"id":"prt_text","type":"text","sessionID":"ses_fixed","messageID":"msg_fixed","text":"bounded display only"}}}),Some(&sid)).unwrap(),Event::Ignored));
    assert!(event(json!({"type":"message.part.updated","properties":{"part":{"type":"text","sessionID":"other","messageID":"msg_fixed","text":"x"}}}),Some(&sid)).is_err());
    assert!(event(json!({"type":"session.status","properties":{"sessionID":"ses_fixed","status":{"type":"retry"}}}),Some(&sid)).is_err());
}

struct ReaderLease(Arc<AtomicUsize>);
impl Drop for ReaderLease {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}
fn tokens() -> Value {
    json!({"input":1,"output":1,"reasoning":0,"cache":{"read":0,"write":0}})
}
fn non_tool(kind: &str, edit: bool) -> Value {
    let mut p = json!({"id":format!("prt_{kind}"),"sessionID":"ses_fixed","messageID":if edit{"msg_edit"}else{"msg_read"},"type":kind});
    match kind {
        "step-finish" => {
            p["reason"] = json!("tool-calls");
            p["cost"] = json!(0);
            p["tokens"] = tokens();
        }
        "text" | "reasoning" => p["text"] = json!(""),
        _ => {}
    }
    json!({"sessionID":"ses_fixed","part":p,"time":0})
}
fn message(edit: bool, user: bool) -> Value {
    let info = if user {
        json!({"id":"msg_user","sessionID":"ses_fixed","role":"user","time":{"created":0},"agent":"nova_probe","model":{"providerID":"nova","modelID":"nova_t1_fixture"}})
    } else {
        json!({"id":if edit{"msg_edit"}else{"msg_read"},"sessionID":"ses_fixed","role":"assistant","parentID":"msg_user","time":{"created":0},"agent":"nova_probe","mode":"nova_probe","modelID":"nova_t1_fixture","providerID":"nova","path":{"cwd":ROOT,"root":ROOT},"cost":0,"tokens":tokens()})
    };
    json!({"sessionID":"ses_fixed","info":info})
}
fn upstream_start(edit: bool) -> Vec<(String, Value)> {
    let mut events = vec![(
        "session.status".into(),
        json!({"sessionID":"ses_fixed","status":{"type":"busy"}}),
    )];
    if !edit {
        events.push(("message.updated".into(), message(false, true)));
        let mut text = non_tool("text", false);
        text["part"]["messageID"] = json!("msg_user");
        text["part"]["text"] = json!("fixed task");
        events.push(("message.part.updated".into(), text));
    }
    events.push(("message.updated".into(), message(edit, false)));
    events.push(("message.part.updated".into(), non_tool("step-start", edit)));
    events.push(("message.part.updated".into(), non_tool("text", edit)));
    events.push(("message.part.delta".into(),json!({"sessionID":"ses_fixed","messageID":if edit{"msg_edit"}else{"msg_read"},"partID":"prt_text","field":"text","delta":"bounded injected text"})));
    for status in ["pending", "running"] {
        events.push((
            "message.part.updated".into(),
            json!({"sessionID":"ses_fixed","part":part(edit,status),"time":0}),
        ));
    }
    events
}
fn upstream_finish(edit: bool) -> Vec<(String, Value)> {
    vec![
        ("message.part.updated".into(), non_tool("step-finish", edit)),
        ("message.updated".into(), message(edit, false)),
        (
            "session.diff".into(),
            json!({"sessionID":"ses_fixed","diff":[]}),
        ),
    ]
}
fn event_label(t: &str, p: &Value) -> String {
    match t {
        "message.part.updated" => {
            let part = &p["part"];
            if part["type"] == "tool" {
                format!(
                    "{t}/tool/{}/{}",
                    part["tool"].as_str().unwrap(),
                    part["state"]["status"].as_str().unwrap()
                )
            } else {
                format!("{t}/{}", part["type"].as_str().unwrap())
            }
        }
        "message.updated" => format!("{t}/{}", p["info"]["role"].as_str().unwrap()),
        "permission.asked" => format!("{t}/{}", p["permission"].as_str().unwrap()),
        _ => t.into(),
    }
}
#[test]
fn upstream_ordered_full_run_steps_delta_diff_and_reject_idle() {
    let mut h = Harness::configured(|t| t.upstream_sequence = true);
    let outcome = h.run();
    h.transport.join();
    assert!(outcome.success, "{:?}", outcome.error);
    assert!(h.cas);
    let sent = h.transport.sent.lock().unwrap().clone();
    let expected = [
        "session.status",
        "message.updated/user",
        "message.part.updated/text",
        "message.updated/assistant",
        "message.part.updated/step-start",
        "message.part.updated/text",
        "message.part.delta",
        "message.part.updated/tool/read/pending",
        "message.part.updated/tool/read/running",
        "permission.asked/read",
        "permission.replied",
        "message.part.updated/tool/read/completed",
        "message.part.updated/step-finish",
        "message.updated/assistant",
        "session.diff",
        "session.status",
        "message.updated/assistant",
        "message.part.updated/step-start",
        "message.part.updated/text",
        "message.part.delta",
        "message.part.updated/tool/edit/pending",
        "message.part.updated/tool/edit/running",
        "permission.asked/edit",
        "permission.replied",
        "message.part.updated/tool/edit/error",
        "message.part.updated/step-finish",
        "message.updated/assistant",
        "session.diff",
        "session.status",
        "session.idle",
    ];
    // The connected/heartbeat infrastructure events do not assert task ordering.
    let task = sent
        .iter()
        .filter(|s| !s.starts_with("server."))
        .map(String::as_str)
        .collect::<Vec<_>>();
    assert_eq!(task, expected);
    assert_eq!(h.driver.pump_failure(), None);
    assert!(h.driver.facts.idle.event_confirmed);
    assert_eq!(*h.transport.model_results.lock().unwrap(), [true]);
    let l = h.state.ledger.lock().unwrap();
    assert_eq!(
        (l.budget.received, l.budget.admitted, l.budget.denied),
        (2, 2, 0)
    );
    println!(
        "INJECTED_UPSTREAM_ORDER={}",
        serde_json::to_string(&task).unwrap()
    );
}
fn negative_event(v: Value) {
    let mut h = Harness::configured(|t| t.injection = Some(v));
    let outcome = h.run();
    h.transport.join();
    assert!(!outcome.success);
    assert!(!h.cas);
    assert!(!outcome.cas_started);
    assert_eq!(h.state.state(), State::Stopped);
    assert!(!h
        .transport
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(p, _)| p == "/permission/per_read/reply"));
}
fn updated(p: Value) -> Value {
    serde_json::json!({"type":"message.part.updated","properties":p})
}
#[test]
fn step_start_run_rejects_foreign_ids_snapshot_type_and_missing_time() {
    for mode in 0..7 {
        let mut p = non_tool("step-start", false);
        match mode {
            0 => p["sessionID"] = json!("ses_other"),
            1 => p["part"]["sessionID"] = json!("ses_other"),
            2 => p["part"]["messageID"] = json!("wrong_prefix"),
            3 => p["part"]["id"] = json!(""),
            4 => p["part"]["snapshot"] = json!(false),
            5 => {
                p.as_object_mut().unwrap().remove("time");
            }
            _ => p["part"]["messageID"] = json!(3),
        };
        negative_event(updated(p));
    }
}
#[test]
fn step_finish_run_rejects_reason_cost_tokens_and_foreign_identity() {
    for mode in 0..10 {
        let mut p = non_tool("step-finish", false);
        match mode {
            0 => p["sessionID"] = json!("ses_other"),
            1 => p["part"]["messageID"] = json!(""),
            2 => p["part"]["id"] = json!("bad"),
            3 => p["part"]["reason"] = json!(12),
            4 => p["part"]["cost"] = json!("0"),
            5 => p["part"]["tokens"]["input"] = Value::Null,
            6 => p["part"]["tokens"]["cache"]["read"] = json!(true),
            7 => p["part"]["tokens"]["total"] = json!("1"),
            8 => p["part"]["tokens"]["reasoning"] = json!({}),
            _ => p["part"]["snapshot"] = json!(0),
        };
        negative_event(updated(p));
    }
}
#[test]
fn delta_run_rejects_session_ids_field_and_type() {
    for mode in 0..6 {
        let mut p = json!({"sessionID":"ses_fixed","messageID":"msg_read","partID":"prt_text","field":"text","delta":"fragment"});
        match mode {
            0 => p["sessionID"] = json!("ses_other"),
            1 => p["messageID"] = json!("wrong"),
            2 => p["partID"] = json!("wrong"),
            3 => p["messageID"] = json!("msg_\n"),
            4 => p["field"] = json!("input"),
            _ => p["delta"] = json!(1),
        };
        negative_event(json!({"type":"message.part.delta","properties":p}));
    }
}
#[test]
fn empty_diff_only_and_nonempty_is_refusal_not_disk_proof() {
    for p in [
        json!({"sessionID":"ses_other","diff":[]}),
        json!({"sessionID":"ses_fixed","diff":[{"file":"Logo.tsx"}]}),
        json!({"sessionID":"ses_fixed","diff":"[]"}),
        json!({"sessionID":"ses_fixed","diff":null}),
    ] {
        negative_event(json!({"type":"session.diff","properties":p}));
    }
}
#[test]
fn nonapproved_parts_removals_and_future_inventories_still_stop_run() {
    for kind in [
        "patch",
        "snapshot",
        "file",
        "agent",
        "subtask",
        "compaction",
        "retry",
    ] {
        negative_event(updated(non_tool(kind, false)));
    }
    for kind in [
        "message.part.removed",
        "message.removed",
        "file.watcher.updated",
        "vcs.branch.updated",
        "lsp.updated",
        "project.updated",
        "todo.updated",
        "session.next.started",
    ] {
        negative_event(
            json!({"type":kind,"properties":{"sessionID":"ses_fixed","messageID":"msg_read","partID":"prt_text"}}),
        );
    }
}
#[test]
fn delta_is_display_only_and_does_not_require_prior_part_inventory() {
    let sid = id("ses_fixed");
    assert!(matches!(event(json!({"type":"message.part.delta","properties":{"sessionID":"ses_fixed","messageID":"msg_read","partID":"prt_unseen","field":"text","delta":"fragment"}}),Some(&sid)).unwrap(),Event::Ignored));
    let p = non_tool("step-start", false);
    assert!(p.get("messageID").is_none());
    assert!(matches!(
        event(updated(p), Some(&sid)).unwrap(),
        Event::Ignored
    ));
    let mut p = non_tool("step-finish", false);
    p["part"]["tokens"]["total"] = json!(2.5);
    p["part"]["cost"] = json!(-0.1);
    // Locked Schema.Finite, not an invented integer/nonnegative constraint.
    assert!(matches!(
        event(updated(p), Some(&sid)).unwrap(),
        Event::Ignored
    ));
}
#[test]
fn queue_capacity_64_overflow_is_deterministic_while_dispatcher_is_blocked() {
    let mut h = Harness::configured(|t| {
        t.message_event = Some(
            (0..65)
                .flat_map(|_| env("server.heartbeat", json!({})))
                .collect(),
        );
        t.pause_http_until_stopped = true;
    });
    let outcome = h.run();
    h.transport.join();
    assert!(!outcome.success);
    assert!(!h.cas);
    assert_eq!(h.driver.pump_failure(), Some(PumpFault::QueueFull));
    assert_eq!(h.state.state(), State::Stopped);
    assert!(!h
        .transport
        .calls
        .lock()
        .unwrap()
        .iter()
        .any(|(p, _)| p == "/permission/per_read/reply"));
}
#[test]
fn schema_fault_preserved_instead_of_misreported_queue_overflow() {
    let mut h = Harness::configured(|t| {
        t.message_event = Some(env(
            "message.part.delta",
            json!({"sessionID":"ses_fixed","messageID":"msg_read","partID":"prt_text","field":"text","delta":42}),
        ));
        t.pause_http_until_stopped = true;
    });
    assert!(!h.run().success);
    h.transport.join();
    assert_eq!(h.driver.pump_failure(), Some(PumpFault::Schema));
    assert_eq!(h.state.state(), State::Stopped);
    assert!(!h.cas);
}
