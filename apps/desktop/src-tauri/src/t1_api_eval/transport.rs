//! Closed mock transport. Numeric IPv4 loopback only; no provider/credential configuration.
use super::{
    budget::{AdmissionFault, Budget, FaultKind},
    Result,
};
use std::{
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
pub const PROVIDER_ID: &str = "nova_t1_mock";
pub const DENIAL_BODY:&[u8]=br#"{"error":{"message":"NOVA_ADMISSION_DENIED","type":"invalid_request_error","code":"nova_admission_denied"}}"#;
pub const DENIAL_HEADER: &[u8] =
    b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\n";
pub const MOCK_BODY: &[u8] = br#"{"source":"SYNTHETIC","usage":{"total_tokens":1},"choices":[]}"#;

#[derive(Debug, Clone)]
pub struct MockEndpoint(SocketAddrV4);
impl MockEndpoint {
    pub fn parse(value: &str) -> Result<Self> {
        // Deliberately no URL normalization, DNS, userinfo, IPv6 aliases or redirect surface.
        let authority = value
            .strip_prefix("http://127.0.0.1:")
            .ok_or("nonfixed loopback URL")?;
        let port = authority.strip_suffix("/model").ok_or("nonfixed path")?;
        if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) || port.starts_with('0') {
            return Err("invalid port".into());
        }
        let port: u16 = port.parse().map_err(|_| "invalid port")?;
        if port == 0 {
            return Err("zero port".into());
        }
        Ok(Self(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)))
    }
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/model", self.0.port())
    }
    fn client() -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(2))
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|e| e.to_string())
    }
    pub async fn post(&self, id: &str, worst: u32) -> Result<reqwest::Response> {
        Self::client()?.post(self.url()).json(&serde_json::json!({"source":"SYNTHETIC","provider":PROVIDER_ID,"id":id,"worst":worst}))
            .send().await.map_err(|e|e.to_string())
    }
}
pub struct MockBroker {
    listener: TcpListener,
    pub endpoint: MockEndpoint,
    pub ledger: Arc<Mutex<Budget>>,
    fault: Arc<AdmissionFault>,
}
impl MockBroker {
    pub fn bind(ledger: Arc<Mutex<Budget>>) -> Result<Self> {
        let fault = ledger
            .lock()
            .map_err(|_| "poisoned ledger at bind")?
            .fault_handle();
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|e| e.to_string())?;
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        Ok(Self {
            listener,
            endpoint: MockEndpoint::parse(&format!("http://127.0.0.1:{port}/model"))?,
            ledger,
            fault,
        })
    }
    /// One bounded connection, in-process mock response. No upstream networking exists here.
    pub fn serve_one(&self) -> Result<()> {
        self.serve_with_window(Duration::from_secs(5))
    }
    pub(super) fn serve_with_window(&self, window: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let mut s = loop {
            match self.listener.accept() {
                Ok((s, addr)) => {
                    if !addr.ip().is_loopback() {
                        return Err("nonloopback peer".into());
                    }
                    break s;
                }
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock
                        && std::time::Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => return Err(e.to_string()),
            }
        };
        // Explicitly reset the accepted socket: listener nonblocking inheritance is platform-specific.
        s.set_nonblocking(false).map_err(|e| e.to_string())?;
        let connection_deadline = Instant::now() + window;
        let input = read_request(&mut s, connection_deadline);
        let mut ledger = loop {
            match self.ledger.try_lock() {
                Ok(lock) => break lock,
                Err(std::sync::TryLockError::Poisoned(_)) => {
                    self.fault.record_unledgered(FaultKind::Poisoned);
                    return Err("poisoned ledger; failed closed; HTTP 400 unconfirmed".into());
                }
                Err(std::sync::TryLockError::WouldBlock) => {
                    if self.fault.is_closed() {
                        self.fault
                            .record_unledgered(FaultKind::ClosedWhileContended);
                        return Err(
                            "admission fault; ledger unavailable; HTTP 400 unconfirmed".into()
                        );
                    }
                    let left = match remaining(connection_deadline) {
                        Ok(left) => left,
                        Err(_) => {
                            self.fault.record_unledgered(FaultKind::LockDeadline);
                            return Err(
                                "ledger lock deadline; failed closed; HTTP 400 unconfirmed".into(),
                            );
                        }
                    };
                    std::thread::sleep(left.min(Duration::from_millis(2)));
                }
            }
        };
        // The captured shared fault remains authoritative after the lock becomes available.
        if self.fault.is_closed() {
            ledger.close(true);
        }
        // Invalid payloads also consume receive/denial accounting and close admission.
        let admitted = match input {
            Ok((id, worst)) if Instant::now() < connection_deadline => {
                ledger.reserve(&id, worst).map(|_| id)
            }
            _ => ledger.reserve("", 0).map(|_| String::new()),
        };
        let (header, body) = match admitted {
            Ok(id) => {
                ledger.settle(&id, Some(1))?;
                (
                    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n"
                        .as_slice(),
                    MOCK_BODY,
                )
            }
            Err(_) => (DENIAL_HEADER, DENIAL_BODY),
        };
        drop(ledger);
        let response = [
            header,
            format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes(),
            body,
        ]
        .concat();
        let mut at = 0;
        while at < response.len() {
            s.set_write_timeout(Some(remaining(connection_deadline)?))
                .map_err(|e| e.to_string())?;
            let n = s.write(&response[at..]).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("zero response write".into());
            }
            at += n;
        }
        Ok(())
    }
}
fn remaining(deadline: Instant) -> Result<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|d| !d.is_zero())
        .map(|d| d.min(Duration::from_secs(2)))
        .ok_or_else(|| "connection total deadline exceeded".into())
}
fn read_exact_deadline(s: &mut TcpStream, buf: &mut [u8], deadline: Instant) -> Result<()> {
    let mut at = 0;
    while at < buf.len() {
        s.set_read_timeout(Some(remaining(deadline)?))
            .map_err(|e| e.to_string())?;
        let n = s.read(&mut buf[at..]).map_err(|e| e.to_string())?;
        if n == 0 {
            return Err("incomplete request".into());
        }
        at += n;
    }
    Ok(())
}
fn read_request(s: &mut TcpStream, deadline: Instant) -> Result<(String, u32)> {
    let mut bytes = Vec::new();
    let mut b = [0; 1];
    while !bytes.ends_with(b"\r\n\r\n") {
        if bytes.len() >= 4096 {
            return Err("header bound".into());
        }
        read_exact_deadline(s, &mut b, deadline)?;
        bytes.push(b[0]);
    }
    let h = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let mut lines = h.split("\r\n");
    if lines.next() != Some("POST /model HTTP/1.1") {
        return Err("fixed request only".into());
    }
    let mut lengths = Vec::new();
    for l in lines.filter(|l| !l.is_empty()) {
        let (k, v) = l.split_once(':').ok_or("malformed header")?;
        if k.eq_ignore_ascii_case("transfer-encoding") {
            return Err("transfer encoding rejected".into());
        }
        if k.eq_ignore_ascii_case("content-length") {
            lengths.push(v.trim().parse::<usize>().map_err(|_| "bad length")?);
        }
    }
    if lengths.len() != 1 || lengths[0] > 4096 {
        return Err("body bound/ambiguous length".into());
    }
    let mut body = vec![0; lengths[0]];
    read_exact_deadline(s, &mut body, deadline)?;
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Request {
        source: String,
        provider: String,
        id: String,
        worst: u32,
    }
    let r: Request = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
    if r.source != "SYNTHETIC" || r.provider != PROVIDER_ID {
        return Err("not a synthetic mock provider".into());
    }
    Ok((r.id, r.worst))
}
