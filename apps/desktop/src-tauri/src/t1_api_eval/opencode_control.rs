//! Closed control plane. HTTP completion and upstream acknowledgements are separate facts.
use super::{
    contract::Patch,
    opencode_gate::{RunState, State},
    opencode_nonce::{Control as ControlSecret, RunNonce},
    opencode_protocol::{
        self as wire, Control, ControlBudget, EventStream, Permission, PermissionFlow, ToolPart,
    },
    Result,
};
use futures_util::{Stream, StreamExt};
use serde_json::{json, Value};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        mpsc::{self, Receiver, TryRecvError},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
pub type Async<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;
pub type EventBytes = Pin<Box<dyn Stream<Item = Result<Vec<u8>>> + Send>>;
#[derive(Clone)]
pub struct Endpoint {
    port: u16,
}
impl Endpoint {
    pub fn registered(value: &str, model_port: u16) -> Result<Self> {
        let p = value
            .strip_prefix("http://127.0.0.1:")
            .ok_or("fixed control authority")?;
        if p.is_empty() || p.starts_with('0') || !p.bytes().all(|b| b.is_ascii_digit()) {
            return Err("fixed control port".into());
        }
        let port = p.parse::<u16>().map_err(|_| "control port")?;
        if port == 0 || model_port == 0 || port == model_port {
            return Err("control/model port separation".into());
        }
        Ok(Self { port })
    }
    fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct Id(String);
impl Id {
    pub fn parse(s: &str) -> Result<Self> {
        if s.is_empty() || s.len() > 128 || s.chars().any(|c| c.is_control()) {
            return Err("bounded identifier".into());
        }
        Ok(Self(s.into()))
    }
    fn segment(&self) -> String {
        self.0
            .as_bytes()
            .iter()
            .map(|b| {
                if b.is_ascii_alphanumeric() || b"-._~".contains(b) {
                    (*b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect()
    }
}
// No Debug or serialization: this object contains a sensitive header.
pub struct Request {
    pub(super) method: reqwest::Method,
    pub(super) url: String,
    pub(super) auth: reqwest::header::HeaderValue,
    pub(super) body: Option<Value>,
}
impl Request {
    #[cfg(test)]
    pub(super) fn new_for_test(secret: &RunNonce<ControlSecret>, item: &Id) -> String {
        Self::new(
            &Endpoint { port: 7001 },
            secret,
            Control::Message,
            Some(&Id("ses_fixed".into())),
            Some(item),
            None,
        )
        .unwrap()
        .url
    }
    fn new(
        endpoint: &Endpoint,
        secret: &RunNonce<ControlSecret>,
        route: Control,
        session: Option<&Id>,
        item: Option<&Id>,
        body: Option<Value>,
    ) -> Result<Self> {
        let sid = || session.map(Id::segment).ok_or("session id");
        let iid = || item.map(Id::segment).ok_or("item id");
        let (method, path) = match route {
            Control::Health => ("GET", "/global/health".into()),
            Control::Path => ("GET", "/path".into()),
            Control::Project => ("GET", "/project/current".into()),
            Control::Agent => ("GET", "/agent".into()),
            Control::Permissions => ("GET", "/permission".into()),
            Control::Status => ("GET", "/session/status".into()),
            Control::Event => ("GET", "/event".into()),
            Control::Session => ("POST", "/session".into()),
            Control::Message => ("GET", format!("/session/{}/message/{}", sid()?, iid()?)),
            Control::Prompt => ("POST", format!("/session/{}/prompt_async", sid()?)),
            Control::Reply => ("POST", format!("/permission/{}/reply", iid()?)),
            Control::Abort => ("POST", format!("/session/{}/abort", sid()?)),
        };
        let root: String = super::opencode_paths::ROOT
            .bytes()
            .map(|b| {
                if b.is_ascii_alphanumeric() {
                    (b as char).to_string()
                } else {
                    format!("%{b:02X}")
                }
            })
            .collect();
        Ok(Self {
            method: reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| "method")?,
            url: format!("{}?directory={root}", endpoint.url(&path)),
            auth: super::opencode_nonce::basic(secret)?,
            body,
        })
    }
}
pub struct Response {
    pub status: u16,
    pub authenticate: bool,
    pub body: Vec<u8>,
}
pub struct EventResponse {
    pub status: u16,
    pub authenticate: bool,
    pub sse_content_type: bool,
    pub stream: EventBytes,
}
pub trait ControlTransport: Send + Sync + 'static {
    fn request(&self, request: Request, deadline: Instant) -> Async<'_, Response>;
    fn event(&self, request: Request, deadline: Instant) -> Async<'_, EventResponse>;
}
pub struct HttpControlTransport {
    client: reqwest::Client,
}
impl HttpControlTransport {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(2))
                .build()
                .map_err(|_| "control client setup")?,
        })
    }
    async fn send(&self, r: Request, d: Instant) -> Result<reqwest::Response> {
        let mut b = self
            .client
            .request(r.method, r.url)
            .header(reqwest::header::AUTHORIZATION, r.auth)
            .header("x-opencode-directory", super::opencode_paths::ROOT)
            .timeout(remaining(d)?);
        if let Some(body) = r.body {
            b = b.json(&body);
        }
        let response = until(d, async {
            b.send()
                .await
                .map_err(|_| "control transport failure".into())
        })
        .await?;
        if response
            .headers()
            .iter()
            .map(|(k, v)| k.as_str().len() + v.as_bytes().len())
            .sum::<usize>()
            > 16384
        {
            return Err("control header bound".into());
        }
        Ok(response)
    }
}
fn remaining(d: Instant) -> Result<Duration> {
    let r = d.saturating_duration_since(Instant::now());
    if r.is_zero() {
        Err("control deadline".into())
    } else {
        Ok(r)
    }
}
async fn until<T>(d: Instant, f: impl Future<Output = Result<T>>) -> Result<T> {
    remaining(d)?;
    tokio::time::timeout_at(tokio::time::Instant::from_std(d), f)
        .await
        .map_err(|_| "control deadline")?
}
impl ControlTransport for HttpControlTransport {
    fn request(&self, r: Request, d: Instant) -> Async<'_, Response> {
        Box::pin(async move {
            let response = self.send(r, d).await?;
            let status = response.status().as_u16();
            let authenticate = response
                .headers()
                .contains_key(reqwest::header::WWW_AUTHENTICATE);
            let mut stream = response.bytes_stream();
            let mut body = vec![];
            while let Some(chunk) = until(d, async { Ok(stream.next().await) }).await? {
                let chunk = chunk.map_err(|_| "control body failure")?;
                if body
                    .len()
                    .checked_add(chunk.len())
                    .filter(|n| *n <= 1048576)
                    .is_none()
                {
                    return Err("control body bound".into());
                }
                body.extend_from_slice(&chunk);
            }
            Ok(Response {
                status,
                authenticate,
                body,
            })
        })
    }
    fn event(&self, r: Request, d: Instant) -> Async<'_, EventResponse> {
        Box::pin(async move {
            let response = self.send(r, d).await?;
            let status = response.status().as_u16();
            let authenticate = response
                .headers()
                .contains_key(reqwest::header::WWW_AUTHENTICATE);
            let sse_content_type = response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(';').next() == Some("text/event-stream"));
            Ok(EventResponse {
                status,
                authenticate,
                sse_content_type,
                stream: Box::pin(response.bytes_stream().map(|c| {
                    c.map(|b| b.to_vec())
                        .map_err(|_| "event transport failure".into())
                })) as EventBytes,
            })
        })
    }
}
#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct ReadyReceipt {
    pub health_http: bool,
    pub paths_checked: bool,
    pub permission_checked: bool,
    pub session_created: bool,
    pub event_headers_received: bool,
    pub event_status: Option<u16>,
    pub event_connected: bool,
}
#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct ReplyReceipt {
    pub decision: bool,
    pub http_complete: bool,
    pub event_confirmed: bool,
    pub tool_error_confirmed: bool,
}
#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct IdleReceipt {
    pub event_confirmed: bool,
}
#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct AbortReceipt {
    pub decision: bool,
    pub http_complete: bool,
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct HttpFact {
    pub route: &'static str,
    pub decision: bool,
    pub status: Option<u16>,
    pub completed: bool,
    pub authentication_failed: bool,
}
#[derive(Default, Clone, Debug, serde::Serialize)]
pub struct Facts {
    pub http: Vec<HttpFact>,
    pub ready: ReadyReceipt,
    pub read_reply: ReplyReceipt,
    pub edit_reply: ReplyReceipt,
    pub idle: IdleReceipt,
    pub abort: AbortReceipt,
    pub prompt_decision: bool,
    pub prompt_http: bool,
    pub consumed: u32,
    pub ignored: u32,
    pub terminal: u32,
    pub error: Option<String>,
}
// Fixed ignore vocabulary, not an extension point. Sources: locked event handler
// (connected/heartbeat/disposed), SessionStatus and MessageV2 bus schema.
pub(super) enum Event {
    Connected,
    Ignored,
    Permission(Permission),
    Replied {
        session: String,
        id: String,
        reply: String,
    },
    Part(ToolPart),
    Idle(String),
    Disposed,
    Error,
}
pub(super) fn event(v: Value, session: Option<&Id>) -> Result<Event> {
    let p = v
        .get("properties")
        .filter(|p| p.is_object())
        .ok_or("event properties")?;
    if v.get("id")
        .is_some_and(|v| v.as_str().is_none_or(|s| s.len() > 256))
    {
        return Err("event id".into());
    }
    let s = || -> Result<String> {
        let s = p["sessionID"].as_str().ok_or("event session")?;
        Id::parse(s)?;
        if session.is_none_or(|id| id.0 != s) {
            return Err("foreign event session".into());
        }
        Ok(s.into())
    };
    Ok(match v["type"].as_str().ok_or("event type")? {
        "server.connected" => {
            if !p.as_object().unwrap().is_empty() {
                return Err("connected schema".into());
            }
            Event::Connected
        }
        "server.heartbeat" => {
            if !p.as_object().unwrap().is_empty() {
                return Err("heartbeat schema".into());
            }
            Event::Ignored
        }
        "server.instance.disposed" => Event::Disposed,
        "permission.asked" => {
            let x = wire::permission_wire(p)?;
            if session.is_none_or(|s| s.0 != x.session_id) {
                return Err("foreign permission".into());
            }
            Event::Permission(x)
        }
        "permission.replied" => Event::Replied {
            session: s()?,
            id: Id::parse(p["requestID"].as_str().ok_or("reply id")?)?.0,
            reply: p["reply"].as_str().ok_or("reply kind")?.into(),
        },
        // Locked 148842ef, schema/src/v1/session.ts blob
        // 75e9282f117cb25078b087661434fcf5e6660620:81-85,233-256,613-649.
        // processor.ts 9f8530929c154a3718b135bd481d7e2e492cb7ff:424-470
        // publishes step parts; 498-521 publishes text then delta. Outer PartUpdated
        // has sessionID/part/time, NOT an outer messageID (schema:613-623).
        "message.part.updated" => {
            s()?;
            let part = &p["part"];
            if part["sessionID"].as_str() != session.map(|s| s.0.as_str())
                || !wire_id(&part["messageID"], "msg")
                || !wire_id(&part["id"], "prt")
                || !finite(&p["time"])
            {
                return Err("part envelope schema".into());
            }
            match part["type"].as_str() {
                Some("tool") => Event::Part(wire::tool_wire(part)?),
                Some("text" | "reasoning") if part["text"].is_string() => Event::Ignored,
                Some("step-start") if optional_string(part, "snapshot") => Event::Ignored,
                Some("step-finish")
                    if part["reason"].is_string()
                        && optional_string(part, "snapshot")
                        && finite(&part["cost"])
                        && valid_step_tokens(&part["tokens"]) =>
                {
                    Event::Ignored
                }
                // patch/snapshot/file/agent/subtask/compaction/retry are NOT approved.
                _ => return Err("unapproved or invalid part schema".into()),
            }
        }
        // session.ts a2a91cd47b5e854606444d1fc09fb18515fbe3b7:877-884;
        // processor.ts:287-305,498-521; schema/v1/session.ts:632-641.
        // Delta is display-only: do not require a previously observed PartID.
        // The locked publisher orders start before delta, but subscription begins
        // at an arbitrary point; prior-part inventory is not an upstream guarantee.
        "message.part.delta" => {
            s()?;
            if !wire_id(&p["messageID"], "msg")
                || !wire_id(&p["partID"], "prt")
                || p["field"] != "text"
                || !p["delta"].is_string()
            {
                return Err("part delta schema".into());
            }
            Event::Ignored
        }
        // summary.ts 6484730d0bd526289febe26014229c83f20f93b9:114:
        // diff=[] is published even before snapshot:false early return.
        "session.diff" => {
            s()?;
            if p["diff"].as_array().is_none_or(|diff| !diff.is_empty()) {
                // A refusal signal, NOT a final proof of any on-disk mutation.
                return Err("nonempty or invalid session diff".into());
            }
            Event::Ignored
        }
        "session.idle" => Event::Idle(s()?),
        "session.status" => {
            let x = s()?;
            match p["status"]["type"].as_str() {
                Some("idle") => {
                    let _ = x;
                    Event::Ignored
                }
                Some("busy") => Event::Ignored,
                _ => return Err("session status schema".into()),
            }
        }
        "session.error" => {
            s()?;
            Event::Error
        }
        // These carry information, never permission or completion authority.
        "message.updated" => {
            let info = &p["info"];
            if info["sessionID"].as_str() != session.map(|s| s.0.as_str())
                || info["id"].as_str().is_none()
            {
                return Err("message update schema".into());
            }
            Event::Ignored
        }
        "session.updated" => {
            let info = &p["info"];
            if info["id"].as_str() != session.map(|s| s.0.as_str()) {
                return Err("session update schema".into());
            }
            Event::Ignored
        }
        _ => return Err("unknown event type".into()),
    })
}
fn finite(value: &Value) -> bool {
    value.as_f64().is_some_and(f64::is_finite)
}
fn wire_id(value: &Value, prefix: &str) -> bool {
    value
        .as_str()
        .is_some_and(|id| id.starts_with(prefix) && Id::parse(id).is_ok())
}
fn optional_string(object: &Value, field: &str) -> bool {
    object.get(field).is_none_or(Value::is_string)
}
fn valid_step_tokens(t: &Value) -> bool {
    t.is_object()
        && ["input", "output", "reasoning"]
            .iter()
            .all(|key| finite(&t[key]))
        && t.get("total").is_none_or(finite)
        && t["cache"].is_object()
        && finite(&t["cache"]["read"])
        && finite(&t["cache"]["write"])
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub enum PumpFault {
    UnknownEvent,
    Schema,
    InstanceDisposed,
    QueueFull,
    ReceiverClosed,
    Framing,
    Transport,
    Disconnected,
    Timeout,
    ReceiptPoisoned,
}
impl PumpFault {
    fn label(self) -> &'static str {
        match self {
            Self::UnknownEvent => "event unknown type",
            Self::Schema => "event schema rejected",
            Self::InstanceDisposed => "instance disposed",
            Self::QueueFull => "event queue bound",
            Self::ReceiverClosed => "event receiver closed",
            Self::Framing => "event framing/bound rejected",
            Self::Transport => "event transport failure",
            Self::Disconnected => "event disconnected",
            Self::Timeout => "event deadline",
            Self::ReceiptPoisoned => "event fault receipt poisoned",
        }
    }
}
pub struct ControlDriver<T: ControlTransport> {
    transport: Arc<T>,
    endpoint: Endpoint,
    secret: RunNonce<ControlSecret>,
    state: Arc<RunState>,
    budget: ControlBudget,
    pub facts: Facts,
    session: Option<Id>,
    flow: Option<PermissionFlow>,
    events: Option<Receiver<Result<Value>>>,
    pump: Option<tokio::task::JoinHandle<()>>,
    read: Option<Permission>,
    edit: Option<Permission>,
    prompt_sent: bool,
    read_completed: bool,
    previews: std::collections::BTreeSet<(String, String, String)>,
    deadline: Instant,
    phase_deadline: Instant,
    pump_fault: Arc<Mutex<Option<PumpFault>>>,
}
impl<T: ControlTransport> Drop for ControlDriver<T> {
    fn drop(&mut self) {
        self.state.stop();
        if let Some(h) = self.pump.take() {
            h.abort();
        }
    }
}
impl<T: ControlTransport> ControlDriver<T> {
    pub fn new(
        transport: Arc<T>,
        endpoint: Endpoint,
        secret: RunNonce<ControlSecret>,
        state: Arc<RunState>,
        deadline: Instant,
    ) -> Self {
        Self {
            transport,
            endpoint,
            secret,
            state,
            budget: ControlBudget::new(),
            facts: Facts::default(),
            session: None,
            flow: None,
            events: None,
            pump: None,
            read: None,
            edit: None,
            prompt_sent: false,
            read_completed: false,
            previews: Default::default(),
            deadline,
            phase_deadline: deadline,
            pump_fault: Arc::new(Mutex::new(None)),
        }
    }
    #[cfg(test)]
    pub(super) fn shorten_deadline(&mut self, duration: Duration) {
        self.deadline = self.deadline.min(Instant::now() + duration);
        self.phase_deadline = self.phase_deadline.min(self.deadline);
    }
    /// Fixed, sanitized terminal reason; does not expose raw event/transport text.
    pub fn pump_failure(&self) -> Option<PumpFault> {
        match self.pump_fault.lock() {
            Ok(f) => *f,
            Err(_) => Some(PumpFault::ReceiptPoisoned),
        }
    }
    pub fn control_count(&self) -> u32 {
        self.budget.received
    }
    fn fail(&mut self, e: String) -> String {
        self.state.stop();
        self.facts.error = Some(e.clone());
        e
    }
    async fn request(
        &mut self,
        route: Control,
        item: Option<Id>,
        body: Option<Value>,
        d: Instant,
    ) -> Result<Value> {
        self.budget.reserve(route)?;
        let name = match route {
            Control::Health => "health",
            Control::Path => "path",
            Control::Project => "project",
            Control::Agent => "agent",
            Control::Permissions => "permissions",
            Control::Status => "status",
            Control::Message => "message",
            Control::Event => "event",
            Control::Session => "session",
            Control::Prompt => "prompt",
            Control::Reply => "reply",
            Control::Abort => "abort",
        };
        self.facts.http.push(HttpFact {
            route: name,
            decision: true,
            status: None,
            completed: false,
            authentication_failed: false,
        });
        let index = self.facts.http.len() - 1;
        let r = Request::new(
            &self.endpoint,
            &self.secret,
            route,
            self.session.as_ref(),
            item.as_ref(),
            body,
        )?;
        let response = until(
            d.min(self.phase_deadline).min(self.deadline),
            self.transport
                .request(r, d.min(self.phase_deadline).min(self.deadline)),
        )
        .await
        .map_err(|_| "control transport/deadline failure")?;
        self.facts.http[index].status = Some(response.status);
        self.facts.http[index].completed = true;
        if response.status == 401 || response.authenticate {
            self.facts.http[index].authentication_failed = true;
            return Err("control authentication failed".into());
        }
        if !(200..300).contains(&response.status) {
            return Err("control HTTP rejected".into());
        }
        if response.body.len() > 1048576 {
            return Err("control body bound".into());
        }
        if response.body.is_empty() {
            Ok(Value::Null)
        } else {
            serde_json::from_slice(&response.body).map_err(|_| "control JSON".into())
        }
    }
    async fn next(&mut self, d: Instant) -> Result<Event> {
        loop {
            remaining(d.min(self.phase_deadline).min(self.deadline))?;
            match self.events.as_ref().ok_or("event not open")?.try_recv() {
                Ok(Ok(v)) => {
                    let e = event(v, self.session.as_ref())?;
                    match e {
                        Event::Ignored => {
                            self.facts.ignored += 1;
                            continue;
                        }
                        Event::Disposed => {
                            self.facts.terminal += 1;
                            return Err("instance disposed".into());
                        }
                        Event::Error => return Err("upstream session error".into()),
                        _ => {
                            self.facts.consumed += 1;
                            return Ok(e);
                        }
                    }
                }
                Ok(Err(e)) => return Err(e),
                Err(TryRecvError::Disconnected) => return Err("event disconnected".into()),
                Err(TryRecvError::Empty) => tokio::time::sleep(Duration::from_millis(1)).await,
            }
        }
    }
    pub async fn prepare(&mut self) -> Result<ReadyReceipt> {
        self.phase_deadline = self.deadline.min(Instant::now() + Duration::from_secs(20));
        let result = self.prepare_inner().await;
        match result {
            Ok(r) => Ok(r),
            Err(e) => {
                self.fail(e.clone());
                if self.session.is_some() {
                    let _ = self.abort().await;
                }
                Err(e)
            }
        }
    }
    async fn prepare_inner(&mut self) -> Result<ReadyReceipt> {
        if self.state.state() != State::Preflight || self.session.is_some() {
            return Err("prepare phase/replay".into());
        }
        let h = self
            .request(Control::Health, None, None, self.deadline)
            .await?;
        if h["healthy"] != true || h["version"] != "1.19.4" {
            return Err("health identity".into());
        }
        self.facts.ready.health_http = true;
        let p = self
            .request(Control::Path, None, None, self.deadline)
            .await?;
        if p["directory"] != super::opencode_paths::ROOT
            || p["worktree"] != super::opencode_paths::ROOT
        {
            return Err("control root mismatch".into());
        }
        let p = self
            .request(Control::Project, None, None, self.deadline)
            .await?;
        if p["worktree"] != super::opencode_paths::ROOT {
            return Err("control project mismatch".into());
        }
        self.facts.ready.paths_checked = true;
        let agents = self
            .request(Control::Agent, None, None, self.deadline)
            .await?;
        let agent = agents
            .as_array()
            .ok_or("agents array")?
            .iter()
            .filter(|a| a["name"] == "nova_probe")
            .collect::<Vec<_>>();
        if agent.len() != 1 || !closed_permissions(&agent[0]["permission"]) {
            return Err("agent permissions mismatch".into());
        }
        if self
            .request(Control::Permissions, None, None, self.deadline)
            .await?
            != json!([])
        {
            return Err("preexisting permission".into());
        }
        self.facts.ready.permission_checked = true;
        let s = self
            .request(
                Control::Session,
                None,
                Some(json!({"title":"Nova fixed offline probe"})),
                self.deadline,
            )
            .await?;
        if s["directory"] != super::opencode_paths::ROOT {
            return Err("session root".into());
        }
        let sid = Id::parse(s["id"].as_str().ok_or("session id")?)?;
        self.flow = Some(PermissionFlow::new(&sid.0));
        self.session = Some(sid);
        self.facts.ready.session_created = true;
        let statuses = self
            .request(Control::Status, None, None, self.deadline)
            .await?;
        if !statuses.is_object()
            || statuses.as_object().unwrap().iter().any(|(k, v)| {
                Some(k.as_str()) != self.session.as_ref().map(|s| s.0.as_str())
                    || v["type"] != "idle"
            })
        {
            return Err("session status preflight".into());
        }
        self.budget.reserve(Control::Event)?;
        let req = Request::new(
            &self.endpoint,
            &self.secret,
            Control::Event,
            self.session.as_ref(),
            None,
            None,
        )?;
        let response = until(
            self.phase_deadline,
            self.transport.event(req, self.deadline),
        )
        .await
        .map_err(|_| "control subscription failure")?;
        self.facts.ready.event_headers_received = true;
        self.facts.ready.event_status = Some(response.status);
        if response.status == 401 || response.authenticate {
            return Err("control authentication failed".into());
        }
        if response.status != 200 || !response.sse_content_type {
            return Err("control event response".into());
        }
        let mut stream = response.stream;
        let (tx, rx) = mpsc::sync_channel(64);
        let state = self.state.clone();
        let d = self.deadline;
        let sid = self.session.clone().unwrap();
        let fault = self.pump_fault.clone();
        self.events = Some(rx);
        self.pump = Some(tokio::spawn(async move {
            let mut framing = EventStream::new();
            let failure = 'pump: loop {
                match until(d, async { Ok(stream.next().await) }).await {
                    Ok(Some(Ok(chunk))) => match framing.push(&chunk) {
                        Ok(values) => {
                            for v in values {
                                match event(v.clone(), Some(&sid)) {
                                    Err(e) => {
                                        break 'pump if e == "unknown event type" {
                                            PumpFault::UnknownEvent
                                        } else {
                                            PumpFault::Schema
                                        }
                                    }
                                    Ok(Event::Disposed) => break 'pump PumpFault::InstanceDisposed,
                                    _ => {}
                                }
                                match tx.try_send(Ok(v)) {
                                    Ok(()) => {}
                                    Err(mpsc::TrySendError::Full(_)) => {
                                        break 'pump PumpFault::QueueFull
                                    }
                                    Err(mpsc::TrySendError::Disconnected(_)) => {
                                        break 'pump PumpFault::ReceiverClosed
                                    }
                                }
                            }
                        }
                        Err(_) => break PumpFault::Framing,
                    },
                    Ok(Some(Err(_))) => break PumpFault::Transport,
                    Ok(None) => {
                        break if framing.end().is_err() {
                            PumpFault::Framing
                        } else {
                            PumpFault::Disconnected
                        }
                    }
                    Err(_) => break PumpFault::Timeout,
                }
            };
            state.stop();
            if let Ok(mut f) = fault.lock() {
                *f = Some(failure);
            }
            let _ = tx.try_send(Err(failure.label().into()));
        }));
        match self.next(self.deadline).await? {
            Event::Connected => {}
            _ => return Err("connected event required".into()),
        }
        self.facts.ready.event_connected = true;
        if self.state.state() != State::Preflight {
            return Err("stopped during prepare".into());
        }
        Ok(self.facts.ready.clone())
    }
    async fn input(&mut self, p: &Permission) -> Result<Vec<ToolPart>> {
        let d = self
            .deadline
            .min(self.phase_deadline)
            .min(Instant::now() + Duration::from_millis(1500));
        for _ in 0..3 {
            remaining(d)?;
            let v = self
                .request(Control::Message, Some(Id::parse(&p.message_id)?), None, d)
                .await?;
            let parts = v["parts"].as_array().ok_or("message parts")?;
            let parts = parts
                .iter()
                .filter(|p| p["type"] == "tool")
                .map(wire::tool_wire)
                .collect::<Result<Vec<_>>>()?;
            let matches = parts
                .iter()
                .filter(|x| x.call_id == p.call_id)
                .collect::<Vec<_>>();
            if matches.len() != 1 {
                return Err("missing/ambiguous message call".into());
            }
            let t = matches[0];
            if t.session_id != p.session_id
                || t.message_id != p.message_id
                || t.tool != p.permission
            {
                return Err("message association".into());
            }
            if t.status == "pending" || t.input.as_object().is_some_and(|o| o.is_empty()) {
                continue;
            }
            remaining(d)?;
            return Ok(parts);
        }
        Err("bounded reread exhausted".into())
    }
    pub async fn task(&mut self) -> Result<Patch> {
        self.phase_deadline = self.deadline.min(Instant::now() + Duration::from_secs(60));
        let result = self.task_inner().await;
        match result {
            Ok(p) => Ok(p),
            Err(e) => {
                self.fail(e.clone());
                let _ = self.abort().await;
                Err(e)
            }
        }
    }
    async fn task_inner(&mut self) -> Result<Patch> {
        if !self.facts.ready.event_connected || self.prompt_sent {
            return Err("task readiness/replay".into());
        }
        self.state.consume_prompt(self.deadline)?;
        self.prompt_sent = true;
        self.facts.prompt_decision = true;
        self.request(Control::Prompt,None,Some(json!({"agent":"nova_probe","model":{"providerID":"nova","modelID":"nova_t1_fixture"},"parts":[{"type":"text","text":include_str!("../../../../../tools/t1-api-eval/m0b/task.txt") }]})),self.deadline).await?;
        self.facts.prompt_http = true;
        let mut pending_edit = None;
        loop {
            if self.state.state() != State::Open {
                return Err("stopped during task".into());
            }
            let e = if self.read_completed && pending_edit.is_some() {
                Event::Permission(pending_edit.take().unwrap())
            } else {
                self.next(self.deadline).await?
            };
            match e {
                Event::Permission(p) => {
                    if p.permission == "edit" && !self.read_completed {
                        if self.read.is_none() || pending_edit.replace(p).is_some() {
                            return Err("edit before read/duplicate".into());
                        }
                        continue;
                    }
                    self.previews.remove(&(
                        p.message_id.clone(),
                        p.call_id.clone(),
                        p.permission.clone(),
                    ));
                    let parts = self.input(&p).await?;
                    if self.state.state() != State::Open {
                        return Err("stopped during input lookup".into());
                    }
                    let patch =
                        self.flow
                            .as_mut()
                            .ok_or("flow")?
                            .permission(&self.state, &p, || Ok(parts.clone()))?;
                    if p.permission == "read" {
                        if self.read.is_some() {
                            return Err("duplicate read".into());
                        }
                        self.read = Some(p.clone());
                        self.facts.read_reply.decision = true;
                        if self
                            .request(
                                Control::Reply,
                                Some(Id::parse(&p.id)?),
                                Some(json!({"reply":"once"})),
                                self.deadline,
                            )
                            .await?
                            != true
                        {
                            return Err("read reply HTTP".into());
                        }
                        self.facts.read_reply.http_complete = true;
                    } else {
                        if !self.previews.is_empty() {
                            return Err("unassociated tool previews".into());
                        }
                        self.edit = Some(p);
                        return patch.ok_or("edit patch missing".into());
                    }
                }
                Event::Part(t) => {
                    if ["pending", "running"].contains(&t.status.as_str())
                        && ["read", "edit"].contains(&t.tool.as_str())
                    {
                        let known = self.read.as_ref().filter(|p| {
                            p.message_id == t.message_id
                                && p.call_id == t.call_id
                                && p.permission == t.tool
                        });
                        if known.is_none() {
                            self.previews.insert((t.message_id, t.call_id, t.tool));
                            if self.previews.len() > 2 {
                                return Err("tool preview bound".into());
                            }
                        }
                        continue;
                    }
                    if t.tool != "read" || t.status != "completed" {
                        return Err("unexpected task tool part".into());
                    }
                    self.flow
                        .as_mut()
                        .ok_or("flow")?
                        .completed_read(&self.state, &t)?;
                    self.read_completed = true;
                }
                Event::Replied { session, id, reply } => {
                    let p = self.read.as_ref().ok_or("unexpected reply")?;
                    if session != p.session_id
                        || id != p.id
                        || reply != "once"
                        || self.facts.read_reply.event_confirmed
                    {
                        return Err("read reply association".into());
                    }
                    self.facts.read_reply.event_confirmed = true;
                }
                _ => return Err("unexpected task event".into()),
            }
        }
    }
    pub async fn reject_and_idle(&mut self) -> Result<IdleReceipt> {
        let result = self.reject_inner().await;
        match result {
            Ok(v) => Ok(v),
            Err(e) => {
                self.fail(e.clone());
                let _ = self.abort().await;
                Err(e)
            }
        }
    }
    async fn reject_inner(&mut self) -> Result<IdleReceipt> {
        if self.state.state() != State::Stopped
            || !self.facts.read_reply.event_confirmed
            || self.facts.edit_reply.decision
        {
            return Err("reject phase/read receipt/replay".into());
        }
        let p = self.edit.clone().ok_or("no edit")?;
        self.facts.edit_reply.decision = true;
        if self
            .request(
                Control::Reply,
                Some(Id::parse(&p.id)?),
                Some(json!({"reply":"reject"})),
                self.deadline.min(Instant::now() + Duration::from_secs(2)),
            )
            .await?
            != true
        {
            return Err("edit reply HTTP".into());
        }
        self.facts.edit_reply.http_complete = true;
        let d = self.deadline.min(Instant::now() + Duration::from_secs(5));
        while !(self.facts.edit_reply.event_confirmed
            && self.facts.edit_reply.tool_error_confirmed
            && self.facts.idle.event_confirmed)
        {
            match self.next(d).await? {
                Event::Replied { session, id, reply }
                    if session == p.session_id
                        && id == p.id
                        && reply == "reject"
                        && !self.facts.edit_reply.event_confirmed =>
                {
                    self.facts.edit_reply.event_confirmed = true
                }
                Event::Part(t)
                    if t.session_id == p.session_id
                        && t.message_id == p.message_id
                        && t.call_id == p.call_id
                        && t.tool == "edit"
                        && t.status == "error"
                        && !self.facts.edit_reply.tool_error_confirmed =>
                {
                    self.facts.edit_reply.tool_error_confirmed = true
                }
                Event::Idle(s) if s == p.session_id && !self.facts.idle.event_confirmed => {
                    self.facts.idle.event_confirmed = true
                }
                _ => return Err("reject/idle association".into()),
            }
        }
        self.confirm_idle()
    }
    pub fn confirm_idle(&self) -> Result<IdleReceipt> {
        if self
            .pump_fault
            .lock()
            .map_err(|_| "event fault receipt poisoned")?
            .is_some()
            || !self.facts.idle.event_confirmed
            || !self.facts.edit_reply.event_confirmed
            || !self.facts.edit_reply.tool_error_confirmed
        {
            return Err("idle or event integrity unconfirmed".into());
        }
        Ok(self.facts.idle.clone())
    }
    pub async fn abort(&mut self) -> Result<AbortReceipt> {
        self.state.stop();
        if self.facts.abort.decision {
            return Err("abort already decided".into());
        }
        self.facts.abort.decision = true;
        if self.session.is_none() {
            return Err("abort session unavailable".into());
        }
        let v = self
            .request(
                Control::Abort,
                None,
                None,
                self.deadline.min(Instant::now() + Duration::from_secs(2)),
            )
            .await?;
        if v != true {
            return Err("abort HTTP".into());
        }
        self.facts.abort.http_complete = true;
        Ok(self.facts.abort.clone())
    }
}
pub(super) fn frozen_permissions() -> Value {
    json!([
        {"permission":"*","pattern":"*","action":"deny"},
        {"permission":"read","pattern":"*","action":"deny"},
        {"permission":"read","pattern":"Logo.tsx","action":"ask"},
        {"permission":"edit","pattern":"*","action":"deny"},
        {"permission":"edit","pattern":"Logo.tsx","action":"ask"}
    ])
}

/// A last global deny seals inherited defaults. Only two exact asks may follow;
/// later implicit external-directory allows (including upstream Truncate) reject.
pub(super) fn closed_permissions(value: &Value) -> bool {
    let Some(rules) = value.as_array().filter(|r| r.len() <= 128) else {
        return false;
    };
    if rules.iter().any(|r| {
        r.as_object().is_none_or(|m| m.len() != 3)
            || ["permission", "pattern", "action"]
                .iter()
                .any(|k| r[k].as_str().is_none())
    }) {
        return false;
    }
    let Some(start) = rules
        .iter()
        .rposition(|r| r["permission"] == "*" && r["pattern"] == "*" && r["action"] == "deny")
    else {
        return false;
    };
    let mut read = false;
    let mut edit = false;
    for r in &rules[start + 1..] {
        let name = r["permission"].as_str().unwrap();
        let pattern = r["pattern"].as_str().unwrap();
        let action = r["action"].as_str().unwrap();
        if action == "ask" && pattern == "Logo.tsx" && ["read", "edit"].contains(&name) {
            if name == "read" {
                read = true
            } else {
                edit = true
            }
        } else if action == "deny" {
            if ["read", "edit", "*"].contains(&name) && !["*", "Logo.tsx"].contains(&pattern) {
                return false;
            }
            if ["*", "Logo.tsx"].contains(&pattern) {
                if name == "read" || name == "*" {
                    read = false
                }
                if name == "edit" || name == "*" {
                    edit = false
                }
            }
        } else {
            return false;
        }
    }
    read && edit
}
