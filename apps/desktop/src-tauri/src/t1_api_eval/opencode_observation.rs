//! SYNTHETIC closed NDJSON receipt adapter. No Procmon/WFP parser or execution permission.
use super::{
    hash,
    opencode_native_files::FactSource,
    opencode_paths::*,
    opencode_probe::{ObservationClass, ObservationReview},
    Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
pub const MAX_INPUT: usize = 16 * 1024 * 1024;
pub const MAX_EVENTS: usize = 65536;
pub const MAX_LINE: usize = 16 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Header {
    pub schema: String,
    pub source: FactSource,
    pub run: String,
    pub attempt: String,
    pub generation: String,
    pub capture: String,
    pub count: u64,
    pub body_sha256: String,
    pub started: u64,
    pub stopped: u64,
    pub closed: u64,
    pub root: ProcessIdentity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tail {
    pub schema: String,
    pub count: u64,
    pub body_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Process {
    pub at: u64,
    pub process: ProcessIdentity,
    pub parent: Option<ProcessIdentity>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Write {
    pub at: u64,
    pub process: ProcessIdentity,
    pub path: String,
    pub kind: WriteKind,
    pub open_access: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Network {
    pub at: u64,
    pub process: ProcessIdentity,
    pub protocol: String,
    pub address: String,
    pub port: u16,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct End {
    pub at: u64,
    pub class: ObservationClass,
    pub records: u64,
    pub loss: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "event", content = "data", deny_unknown_fields)]
pub enum Event {
    Process(Process),
    Write(Write),
    Network(Network),
    End(End),
}
/// Typed SYNTHETIC ownership records. No booleans supplied by an export can grant capture ownership.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OwnershipKind {
    NoPriorCapture,
    Started,
    Stopped,
    OutputsClosed,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnershipReceipt {
    pub source: FactSource,
    pub run: String,
    pub attempt: String,
    pub generation: String,
    pub capture: String,
    pub kind: OwnershipKind,
    pub at: u64,
}
/// Already held by the caller; the export cannot choose the expected run/generation/target.
pub struct Binding<'a> {
    pub run: &'a str,
    pub attempt: &'a str,
    pub capture: &'a str,
    pub generation: &'a str,
    pub ownership: &'a [OwnershipReceipt],
    pub model_port: u16,
    pub target: &'a ProcessIdentity,
}
pub struct ParsedObservation {
    pub review: ObservationReview,
    pub retained_events: Vec<Event>,
    pub input_sha256: String,
}
pub fn native_parser(_: &[u8]) -> Result<ParsedObservation> {
    Err("BLOCKED_NATIVE_OBSERVATION_SCHEMA_LOSS_COVERAGE".into())
}
fn bounded_string(s: &str) -> Result<()> {
    if s.is_empty() || s.len() > 4096 || s.chars().any(|c| c.is_control()) {
        return Err("OBS_STRING".into());
    }
    Ok(())
}
/// Lexical depth bound before serde; braces in strings do not consume nesting.
fn depth(line: &[u8]) -> Result<()> {
    let (mut n, mut quoted, mut escaped) = (0u32, false, false);
    for &c in line {
        if quoted {
            if escaped {
                escaped = false
            } else if c == b'\\' {
                escaped = true
            } else if c == b'"' {
                quoted = false
            }
        } else {
            match c {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    n = n.checked_add(1).ok_or("OBS_DEPTH")?;
                    if n > 16 {
                        return Err("OBS_DEPTH".into());
                    }
                }
                b'}' | b']' => n = n.checked_sub(1).ok_or("OBS_DEPTH")?,
                _ => (),
            }
        }
    }
    if quoted || n != 0 {
        return Err("OBS_TRUNCATED".into());
    }
    Ok(())
}
fn decode<T: serde::de::DeserializeOwned>(l: &[u8]) -> Result<T> {
    if l.is_empty() || l.len() > MAX_LINE {
        return Err("OBS_LINE_BOUND".into());
    }
    depth(l)?;
    serde_json::from_slice(l).map_err(|_| "OBS_SCHEMA_UNKNOWN_DUPLICATE_OR_TYPE".into())
}
fn event_time(e: &Event) -> u64 {
    match e {
        Event::Process(x) => x.at,
        Event::Write(x) => x.at,
        Event::Network(x) => x.at,
        Event::End(x) => x.at,
    }
}
/// Every body byte (including each LF) is covered by the header and terminator hash.
pub fn parse(
    bytes: &[u8],
    b: &Binding<'_>,
    r: &FixtureReservation,
    after: TreeSnapshot,
) -> Result<ParsedObservation> {
    if bytes.len() > MAX_INPUT || !bytes.ends_with(b"\n") {
        return Err("OBS_INPUT_BOUND_OR_TRUNCATED".into());
    }
    let lines: Vec<&[u8]> = bytes[..bytes.len() - 1].split(|c| *c == b'\n').collect();
    if lines.len() < 2 || lines.len() > MAX_EVENTS + 2 {
        return Err("OBS_EVENT_BOUND".into());
    }
    let h: Header = decode(lines[0])?;
    let tail: Tail = decode(lines[lines.len() - 1])?;
    if h.source != FactSource::Injected {
        return Err("BLOCKED_NATIVE_OBSERVATION_SCHEMA_LOSS_COVERAGE".into());
    }
    r.validate(FactSource::Injected)?;
    for s in [
        &h.schema,
        &h.run,
        &h.attempt,
        &h.generation,
        &h.capture,
        &h.body_sha256,
    ] {
        bounded_string(s)?;
    }
    if h.schema != "NB3_SYNTHETIC_V1"
        || tail.schema != "NB3_END_V1"
        || h.count != tail.count
        || h.count != u64::try_from(lines.len() - 2).map_err(|_| "OBS_COUNT")?
        || h.body_sha256 != tail.body_sha256
        || h.run != b.run
        || h.attempt != b.attempt
        || h.capture != b.capture
        || h.generation != b.generation
        || r.prepared.as_ref().map(|p| p.generation.as_str()) != Some(b.generation)
        || h.root != *b.target
        || b.model_port == 0
    {
        return Err("OBS_BINDING".into());
    }
    if h.started >= h.stopped
        || h.stopped
            .checked_sub(h.started)
            .is_none_or(|t| t > 180 * 10_000_000)
        || h.closed < h.stopped
    {
        return Err("OBS_TIME".into());
    }
    if b.ownership.len() != 4 {
        return Err("OBS_OWNERSHIP_MISSING".into());
    }
    for kind in [
        OwnershipKind::NoPriorCapture,
        OwnershipKind::Started,
        OwnershipKind::Stopped,
        OwnershipKind::OutputsClosed,
    ] {
        let records: Vec<_> = b.ownership.iter().filter(|x| x.kind == kind).collect();
        if records.len() != 1 {
            return Err("OBS_OWNERSHIP_AMBIGUOUS".into());
        }
        let x = records[0];
        if x.source != FactSource::Injected
            || x.run != b.run
            || x.attempt != b.attempt
            || x.capture != b.capture
            || x.generation != b.generation
        {
            return Err("OBS_OWNERSHIP_BINDING".into());
        }
        let time_ok = match kind {
            OwnershipKind::NoPriorCapture => x.at <= h.started,
            OwnershipKind::Started => x.at == h.started,
            OwnershipKind::Stopped => x.at == h.stopped,
            OwnershipKind::OutputsClosed => x.at == h.closed,
        };
        if !time_ok {
            return Err("OBS_OWNERSHIP_TIME".into());
        }
    }
    let start = lines[0].len() + 1;
    let end = bytes.len() - lines.last().unwrap().len() - 1;
    if hash(&bytes[start..end]) != h.body_sha256 {
        return Err("OBS_BODY_HASH".into());
    }
    let mut events = Vec::new();
    let mut processes = BTreeMap::new();
    let mut ends = BTreeMap::new();
    let mut counts = [0u64; 3];
    let mut edge_count = 0usize;
    for l in &lines[1..lines.len() - 1] {
        let e: Event = decode(l)?;
        let at = event_time(&e);
        if at < h.started || at > h.stopped {
            return Err("OBS_EVENT_TIME".into());
        }
        match &e {
            Event::Process(p) => {
                p.process.validate()?;
                if p.process.created < h.started
                    || p.process.created > p.at
                    || processes
                        .insert(p.process.clone(), p.parent.clone())
                        .is_some()
                {
                    return Err("OBS_PROCESS".into());
                }
                if let Some(parent) = &p.parent {
                    parent.validate()?;
                    edge_count = edge_count.checked_add(1).ok_or("OBS_EDGE_BOUND")?;
                    if edge_count > 4096 {
                        return Err("OBS_EDGE_BOUND".into());
                    }
                }
                counts[0] = counts[0].checked_add(1).ok_or("OBS_COUNT")?;
            }
            Event::Write(w) => {
                w.process.validate()?;
                if w.process.created > w.at {
                    return Err("OBS_PROCESS_TIME".into());
                }
                if w.path.is_empty()
                    || w.path.encode_utf16().count() > 2048
                    || w.path.chars().any(|c| c.is_control())
                {
                    return Err("OBS_PATH_BOUND".into());
                }
                counts[1] = counts[1].checked_add(1).ok_or("OBS_COUNT")?;
            }
            Event::Network(n) => {
                n.process.validate()?;
                if n.process.created > n.at {
                    return Err("OBS_PROCESS_TIME".into());
                }
                bounded_string(&n.protocol)?;
                bounded_string(&n.address)?;
                counts[2] = counts[2].checked_add(1).ok_or("OBS_COUNT")?;
                if counts[2] > 4096 {
                    return Err("OBS_NETWORK_BOUND".into());
                }
            }
            Event::End(e) => {
                let key = match e.class {
                    ObservationClass::ProcessTree => 0,
                    ObservationClass::FileWriteAds => 1,
                    ObservationClass::NetworkAttempts => 2,
                };
                if ends.insert(key, e.clone()).is_some() {
                    return Err("OBS_CLASS_DUPLICATE".into());
                }
            }
        }
        events.push(e);
    }
    if processes.get(b.target) != Some(&None) {
        return Err("OBS_ROOT".into());
    }
    let mut members = BTreeSet::from([b.target.clone()]);
    let mut pending: Vec<_> = processes
        .iter()
        .filter_map(|(p, parent)| parent.as_ref().map(|q| (p.clone(), q.clone())))
        .collect();
    // All declared process edges belong to the target closure. Unrelated writer identities are retained separately.
    while !pending.is_empty() {
        let n = pending.len();
        pending.retain(|(p, q)| {
            if members.contains(q) && p != q && p.created >= q.created {
                members.insert(p.clone());
                false
            } else {
                true
            }
        });
        if n == pending.len() {
            return Err("OBS_ANCESTRY".into());
        }
    }
    let mut writes = Vec::new();
    let mut descendants = Vec::new();
    for (p, parent) in &processes {
        if let Some(q) = parent {
            descendants.push(ProcessEdge {
                process: p.clone(),
                parent: q.clone(),
            })
        }
    }
    for e in &events {
        match e {
            Event::Write(w) => {
                let canonical = w
                    .path
                    .strip_prefix(&format!("{}\\", ROOT))
                    .map(|p| p.replace('\\', "/"));
                let looks_inside = w
                    .path
                    .to_ascii_lowercase()
                    .starts_with(&ROOT.to_ascii_lowercase())
                    || w.path
                        .to_ascii_lowercase()
                        .starts_with(&format!(r"\\?\{}", ROOT).to_ascii_lowercase());
                if members.contains(&w.process) {
                    if writes.len() >= 64 {
                        return Err("OBS_WRITE_BOUND".into());
                    }
                    let p = canonical
                        .filter(|s| super::opencode_native_files::relative(s) && !s.contains('~'))
                        .unwrap_or("UNKNOWN_PATH".into());
                    let kind = if w.open_access.is_some_and(|m| m & 0x500D0116 != 0) {
                        WriteKind::Unknown
                    } else {
                        w.kind.clone()
                    };
                    writes.push(FileWriteFact {
                        path: p,
                        process: w.process.clone(),
                        kind,
                    });
                } else if looks_inside {
                    return Err("OBS_FOREIGN_FIXTURE_WRITE".into());
                }
            }
            Event::Network(n) if members.contains(&n.process) => {
                if n.protocol != "TCP" || n.address != "127.0.0.1" || n.port != b.model_port {
                    return Err("OBS_NETWORK_DENIED".into());
                }
            }
            _ => (),
        }
    }
    for key in 0..3 {
        let e = ends.get(&key).ok_or("OBS_COVERAGE_UNKNOWN")?;
        if e.loss != 0 || e.records != counts[key] || e.at != h.stopped {
            return Err("OBS_LOSS_OR_COUNT".into());
        }
    }
    let classes = ends.values().map(|e| e.class.clone()).collect();
    let review = ObservationReview {
        source: Some(FactSource::Injected),
        exclusive_vm: true,
        same_generation: h.generation == b.generation,
        no_prior_capture: true,
        start_receipt: true,
        stop_receipt: true,
        outputs_closed: h.closed >= h.stopped,
        classes,
        tree: AgentTreeReceipt {
            after,
            descendants,
            writes,
        },
        pml_loss: Some(ends[&1].loss),
        etl_loss: Some(ends[&2].loss),
        pml_bytes: bytes.len() as u64,
        etl_bytes: bytes.len() as u64,
    };
    review.validate()?;
    review.validate_source(FactSource::Injected)?;
    Ok(ParsedObservation {
        review,
        retained_events: events,
        input_sha256: hash(bytes),
    })
}
