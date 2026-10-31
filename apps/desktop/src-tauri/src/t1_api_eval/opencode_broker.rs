use super::{opencode_gate::RunState, Result};
use std::{sync::Arc, time::Instant};
pub struct Broker {
    pub state: Arc<RunState>,
    nonce_len: usize,
    auth_key: ring::hmac::Key,
    nonce_tag: ring::hmac::Tag,
}
// Public test stimulus only, never a production default or prepared run credential.
pub(super) const INJECTED_NONCE: &[u8] = b"injected-not-a-real-run-credential-0001";
fn valid_nonce(n: &[u8]) -> bool {
    (32..=128).contains(&n.len())
        && n.iter()
            .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
}
impl Broker {
    pub fn new(state: Arc<RunState>, nonce: &[u8]) -> Result<Self> {
        if !valid_nonce(nonce) {
            return Err("run authentication not prepared".into());
        }
        // ring::hmac::verify compares fixed-size tags in constant time. Raw nonce is not retained.
        let auth_key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"Nova-M0B-auth-compare-v1");
        let nonce_tag = ring::hmac::sign(&auth_key, nonce);
        Ok(Self {
            state,
            nonce_len: nonce.len(),
            auth_key,
            nonce_tag,
        })
    }
    pub fn request(&self, raw: &[u8], deadline: Instant) -> Result<Vec<u8>> {
        let parsed = (|| -> Result<serde_json::Value> {
            let split = raw
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .ok_or("incomplete headers")?;
            if split > 16384 || raw.len() - split - 4 > 1048576 || Instant::now() >= deadline {
                self.state.stop();
                return Err("request total bound/deadline".into());
            }
            let header = std::str::from_utf8(&raw[..split]).map_err(|_| "header encoding")?;
            let mut lines = header.split("\r\n");
            if lines.next() != Some("POST /v1/chat/completions HTTP/1.1") {
                self.state.stop();
                return Err("fixed route".into());
            }
            let mut lengths = Vec::new();
            let mut authorization = Vec::new();
            for line in lines {
                let (k, v) = line.split_once(':').ok_or("header syntax")?;
                if k.eq_ignore_ascii_case("transfer-encoding") {
                    self.state.stop();
                    return Err("transfer encoding".into());
                }
                if k.eq_ignore_ascii_case("authorization") {
                    authorization.push(v.trim());
                }
                if k.eq_ignore_ascii_case("content-length") {
                    lengths.push(v.trim().parse::<usize>().map_err(|_| "content length")?)
                }
            }
            if authorization.len() != 1 {
                return Err("authentication rejected".into());
            }
            let supplied = authorization[0]
                .strip_prefix("Bearer ")
                .ok_or("authentication rejected")?
                .as_bytes();
            if supplied.len() != self.nonce_len
                || !valid_nonce(supplied)
                || ring::hmac::verify(&self.auth_key, supplied, self.nonce_tag.as_ref()).is_err()
            {
                return Err("authentication rejected".into());
            }
            if lengths != [raw.len() - split - 4] {
                self.state.stop();
                return Err("ambiguous/short body".into());
            }
            let body: serde_json::Value =
                serde_json::from_slice(&raw[split + 4..]).map_err(|_| "body JSON")?;
            if body["model"] != "nova_t1_fixture"
                || body["stream"] != true
                || !body["messages"].is_array()
                || !body["tools"].is_array()
            {
                self.state.stop();
                return Err("fixed model/SSE schema".into());
            }
            let names: Vec<_> = body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| {
                    if v["type"] != "function" || !v["function"]["parameters"].is_object() {
                        return None;
                    }
                    v["function"]["name"].as_str()
                })
                .collect();
            if names != [Some("read"), Some("edit")]
                || body["messages"].as_array().unwrap().is_empty()
            {
                return Err("fixed tool vocabulary/messages".into());
            }
            Ok(body)
        })();
        let body = match parsed {
            Ok(b) => b,
            Err(e) => {
                let _ = self.state.reject_received(deadline);
                return Err(e);
            }
        };
        let edit = body["messages"]
            .as_array()
            .is_some_and(|a| a.iter().any(|m| m["role"] == "tool"));
        let id = if edit { "edit" } else { "read" };
        if self.state.admit(id, edit, deadline).is_err() {
            return Ok([
                super::transport::DENIAL_HEADER,
                format!(
                    "Content-Length: {}\r\n\r\n",
                    super::transport::DENIAL_BODY.len()
                )
                .as_bytes(),
                super::transport::DENIAL_BODY,
            ]
            .concat());
        }
        let input = if edit {
            serde_json::json!({"filePath":super::opencode_paths::TARGET,"oldString":"alt=\"Payload Logo\"","newString":"alt=\"Northstar Studio logo\"","replaceAll":false})
        } else {
            serde_json::json!({"filePath":super::opencode_paths::TARGET})
        };
        let data = super::opencode_protocol::sse(id, id, &input)?;
        self.state.settle_injected(id, Some(1), deadline)?;
        Ok([
            b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n"
                .as_slice(),
            &data,
        ]
        .concat())
    }
}

/// Injectable connection boundary: implementation must bound each OS read by remaining time.
/// A late synchronous return is detected, not retroactively called a hard interruption.
pub fn read_bounded(
    mut read: impl FnMut(&mut [u8], std::time::Duration) -> Result<usize>,
    mut now: impl FnMut() -> u64,
) -> Result<Vec<u8>> {
    let start = now();
    let deadline = start.checked_add(5000).ok_or("connection clock")?;
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let before = now();
        if before < start || before >= deadline {
            return Err("connection total deadline".into());
        }
        let n = read(
            &mut buf,
            std::time::Duration::from_millis(deadline - before),
        )?;
        let after = now();
        if after < before || after >= deadline {
            return Err("late connection read".into());
        }
        if n > buf.len() {
            return Err("reader returned invalid count".into());
        }
        if n == 0 {
            return Err("connection truncated".into());
        }
        out.extend_from_slice(&buf[..n]);
        if let Some(split) = out.windows(4).position(|w| w == b"\r\n\r\n") {
            if split > 16384 {
                return Err("header limit".into());
            }
            let header = std::str::from_utf8(&out[..split]).map_err(|_| "header encoding")?;
            let lengths: Vec<_> = header
                .split("\r\n")
                .skip(1)
                .filter_map(|l| l.split_once(':'))
                .filter(|(k, _)| k.eq_ignore_ascii_case("Content-Length"))
                .collect();
            if lengths.len() != 1 {
                return Err("length count".into());
            }
            let size = lengths[0]
                .1
                .trim()
                .parse::<usize>()
                .map_err(|_| "length syntax")?;
            if size > 1048576 {
                return Err("body limit".into());
            }
            if out.len() >= split + 4 + size {
                if out.len() != split + 4 + size {
                    return Err("trailing request".into());
                }
                return Ok(out);
            }
        } else if out.len() > 16384 {
            return Err("header limit".into());
        }
    }
}

/// Fixed pure mock stimulus, not a configurable target or provider interface.
pub fn fixed_request(edit: bool, nonce: &[u8]) -> Result<Vec<u8>> {
    if !valid_nonce(nonce) {
        return Err("mock authentication input invalid".into());
    }
    let tools = serde_json::json!([
        {"type":"function","function":{"name":"read","parameters":{"type":"object","properties":{"filePath":{"type":"string"}}}}},
        {"type":"function","function":{"name":"edit","parameters":{"type":"object","properties":{"filePath":{"type":"string"},"oldString":{"type":"string"},"newString":{"type":"string"},"replaceAll":{"type":"boolean"}}}}}
    ]);
    let messages = if edit {
        serde_json::json!([{"role":"user","content":"fixed task"},{"role":"tool","tool_call_id":"read","content":"INJECTED read completion"}])
    } else {
        serde_json::json!([{"role":"user","content":"fixed task"}])
    };
    let b=serde_json::json!({"model":"nova_t1_fixture","stream":true,"tools":tools,"messages":messages}).to_string();
    Ok(format!(
        "POST /v1/chat/completions HTTP/1.1\r\nAuthorization: Bearer {}\r\nContent-Length: {}\r\n\r\n{}",
        std::str::from_utf8(nonce).map_err(|_|"mock nonce encoding")?, b.len(), b
    ).into_bytes())
}

/// Native socket boundary, compiled only in this pack; tests never call bind/serve.
/// It does not claim the host/VM has no other egress routes.
pub fn bind_loopback() -> Result<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .map_err(|_| "loopback bind")?;
    listener
        .set_nonblocking(true)
        .map_err(|_| "listener nonblocking")?;
    Ok(listener)
}
#[derive(Debug, serde::Serialize)]
pub struct ConnectionEvidence {
    pub source: &'static str,
    pub request_bytes: usize,
    pub request_sha256: Option<String>,
    pub response_complete: bool,
    pub error: Option<String>,
}
/// One bounded connection; caller controls the finite target window and no reconnect.
pub fn serve_one(
    listener: &std::net::TcpListener,
    broker: &Broker,
    window: Instant,
) -> Result<ConnectionEvidence> {
    use std::io::{Read, Write};
    let (mut stream, peer) = loop {
        if Instant::now() >= window {
            broker.state.stop();
            return Err("accept window".into());
        }
        match listener.accept() {
            Ok(v) => break v,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(1))
            }
            Err(_) => {
                broker.state.stop();
                return Err("accept failure".into());
            }
        }
    };
    if peer.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
        broker.state.stop();
        return Err("non-fixed loopback peer".into());
    }
    let start = Instant::now();
    let deadline = window.min(start + std::time::Duration::from_secs(5));
    let raw = read_bounded(
        |buf, remaining| {
            let remaining = remaining.min(deadline.saturating_duration_since(Instant::now()));
            if remaining.is_zero() {
                return Err("connection deadline".into());
            }
            stream
                .set_read_timeout(Some(remaining))
                .map_err(|_| "read timeout setup")?;
            stream.read(buf).map_err(|_| "bounded read failure".into())
        },
        || start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64,
    );
    let mut evidence = ConnectionEvidence {
        source: "NATIVE_UNTESTED",
        request_bytes: 0,
        request_sha256: None,
        response_complete: false,
        error: None,
    };
    let response = match raw {
        Ok(raw) => {
            evidence.request_bytes = raw.len();
            // Full request hash only; no raw Authorization/header/body is archived here.
            evidence.request_sha256 = Some(super::hash(&raw));
            match broker.request(&raw, deadline) {
                Ok(v) => v,
                Err(e) => {
                    evidence.error = Some(e);
                    denial_response()
                }
            }
        }
        Err(e) => {
            let _ = broker.state.reject_received(deadline);
            evidence.error = Some(e);
            denial_response()
        }
    };
    let mut sent = 0;
    while sent < response.len() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            broker.state.stop();
            evidence.error = Some("response deadline; delivery UNKNOWN".into());
            return Ok(evidence);
        }
        stream
            .set_write_timeout(Some(remaining))
            .map_err(|_| "write timeout setup")?;
        match stream.write(&response[sent..]) {
            Ok(0) | Err(_) => {
                broker.state.stop();
                evidence.error = Some("response delivery UNKNOWN".into());
                return Ok(evidence);
            }
            Ok(n) => sent += n,
        }
    }
    evidence.response_complete = true;
    Ok(evidence)
}
pub fn denial_response() -> Vec<u8> {
    [
        super::transport::DENIAL_HEADER,
        format!(
            "Content-Length: {}\r\n\r\n",
            super::transport::DENIAL_BODY.len()
        )
        .as_bytes(),
        super::transport::DENIAL_BODY,
    ]
    .concat()
}
