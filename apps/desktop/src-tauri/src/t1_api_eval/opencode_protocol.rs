use super::{contract::Patch, Result, PRE, PRE_HASH};
use serde::Deserialize;
use std::collections::BTreeSet;
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permission {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub permission: String,
    pub patterns: Vec<String>,
    pub metadata: serde_json::Value,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPart {
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub tool: String,
    pub status: String,
    pub input: serde_json::Value,
}
pub struct Association {
    pub session: String,
    seen: BTreeSet<String>,
}
impl Association {
    pub fn new(session: &str) -> Self {
        Self {
            session: session.into(),
            seen: BTreeSet::new(),
        }
    }
    pub fn input(
        &mut self,
        p: &Permission,
        mut get: impl FnMut() -> Result<Vec<ToolPart>>,
    ) -> Result<serde_json::Value> {
        let started = std::time::Instant::now();
        if p.session_id != self.session
            || !["read", "edit"].contains(&p.permission.as_str())
            || [p.id.as_str(), p.message_id.as_str(), p.call_id.as_str()]
                .iter()
                .any(|s| s.is_empty() || s.len() > 128)
            || !self.seen.insert(p.id.clone())
        {
            return Err("unknown/duplicate permission association".into());
        }
        for _ in 0..3 {
            if started.elapsed() > std::time::Duration::from_millis(1500) {
                return Err("input total deadline".into());
            }
            let parts = get()?;
            if started.elapsed() > std::time::Duration::from_millis(1500) {
                return Err("input API returned late".into());
            }
            let m: Vec<_> = parts.iter().filter(|t| t.call_id == p.call_id).collect();
            if m.len() != 1 {
                return Err("missing/ambiguous tool".into());
            }
            let t = m[0];
            if t.session_id != p.session_id
                || t.message_id != p.message_id
                || t.tool != p.permission
            {
                return Err("cross-session/message/tool".into());
            }
            if t.status == "pending" || t.input.as_object().is_some_and(|m| m.is_empty()) {
                continue;
            }
            if t.status != "running" {
                return Err("late tool".into());
            }
            let path = t
                .input
                .get("filePath")
                .and_then(|v| v.as_str())
                .ok_or("missing filePath")?;
            super::opencode_paths::target(path, &p.patterns, false)?;
            if p.permission == "read" {
                if t.input
                    .as_object()
                    .ok_or("input object")?
                    .keys()
                    .any(|k| !matches!(k.as_str(), "filePath" | "offset" | "limit"))
                    || t.input.get("offset").is_some_and(|v| v.as_u64() != Some(1))
                    || t.input
                        .get("limit")
                        .is_some_and(|v| v.as_u64() != Some(2000))
                {
                    return Err("read bounds".into());
                }
            } else {
                if p.metadata.get("filepath").and_then(|v| v.as_str()) != Some(path) {
                    return Err("edit metadata path mismatch".into());
                }
                Self::patch(&p.id, &t.input)?;
            }
            return Ok(t.input.clone());
        }
        Err("bounded reread exhausted".into())
    }
    pub fn patch(id: &str, v: &serde_json::Value) -> Result<Patch> {
        if v.as_object().ok_or("input object")?.keys().any(|k| {
            !matches!(
                k.as_str(),
                "filePath" | "oldString" | "newString" | "replaceAll"
            )
        }) {
            return Err("unknown edit field".into());
        }
        if v.get("replaceAll").is_some_and(|x| x != false) {
            return Err("replaceAll/type".into());
        }
        let p = Patch {
            run_id: "M0B".into(),
            role: "developer".into(),
            attempt_id: id.into(),
            target_id: "logo".into(),
            expected_sha256: PRE_HASH.into(),
            old_literal: v["oldString"].as_str().ok_or("oldString")?.into(),
            new_literal: v["newString"].as_str().ok_or("newString")?.into(),
            reason: "observed edit proposal".into(),
            replace_all: false,
        };
        p.validate("M0B", id, PRE)?;
        Ok(p)
    }
}
pub fn sse(tool: &str, id: &str, input: &serde_json::Value) -> Result<Vec<u8>> {
    if !["read", "edit"].contains(&tool) {
        return Err("fixed tool".into());
    }
    let chunk = serde_json::json!({"id":"nova_mock","object":"chat.completion.chunk","created":0,"model":"nova_t1_fixture","choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":id,"type":"function","function":{"name":tool,"arguments":input.to_string()}}]},"finish_reason":null}]});
    let end = serde_json::json!({"id":"nova_mock","object":"chat.completion.chunk","created":0,"model":"nova_t1_fixture","choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":1,"completion_tokens":0,"total_tokens":1}});
    Ok(format!("data: {chunk}\n\ndata: {end}\n\ndata: [DONE]\n\n").into_bytes())
}

/// Decode the locked PermissionV1 wire shape, then normalize explicitly.
/// The normalized structs above are never advertised as raw upstream JSON.
pub fn permission_wire(v: &serde_json::Value) -> Result<Permission> {
    let field = |key: &str| -> Result<String> {
        v[key]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .map(str::to_string)
            .ok_or(format!("permission field {key}"))
    };
    let o = v.as_object().ok_or("permission object")?;
    if o.keys().any(|k| {
        ![
            "id",
            "sessionID",
            "permission",
            "patterns",
            "metadata",
            "always",
            "tool",
        ]
        .contains(&k.as_str())
    }) || !v["always"].is_array()
        || !v["metadata"].is_object()
    {
        return Err("PermissionV1 schema".into());
    }
    let tool = v["tool"].as_object().ok_or("missing permission.tool")?;
    if tool.len() != 2 {
        return Err("ambiguous permission.tool".into());
    }
    let patterns: Vec<String> =
        serde_json::from_value(v["patterns"].clone()).map_err(|_| "patterns")?;
    Ok(Permission {
        id: field("id")?,
        session_id: field("sessionID")?,
        permission: field("permission")?,
        patterns,
        message_id: v["tool"]["messageID"].as_str().ok_or("messageID")?.into(),
        call_id: v["tool"]["callID"].as_str().ok_or("callID")?.into(),
        metadata: v["metadata"].clone(),
    })
}
pub fn tool_wire(v: &serde_json::Value) -> Result<ToolPart> {
    if v["type"] != "tool" || !v["state"]["input"].is_object() {
        return Err("ToolPart wire schema".into());
    }
    let f = |k: &str| -> Result<String> {
        v[k].as_str()
            .filter(|s| !s.is_empty() && s.len() <= 128)
            .map(str::to_string)
            .ok_or(format!("ToolPart {k}"))
    };
    Ok(ToolPart {
        session_id: f("sessionID")?,
        message_id: f("messageID")?,
        call_id: f("callID")?,
        tool: f("tool")?,
        status: v["state"]["status"].as_str().ok_or("state.status")?.into(),
        input: v["state"]["input"].clone(),
    })
}
/// Read permission once, actual completed ToolPart, then edit proposal. No Agent write.
pub struct PermissionFlow {
    association: Association,
    read: Option<Permission>,
    read_done: bool,
    edit: bool,
}
impl PermissionFlow {
    pub fn new(session: &str) -> Self {
        Self {
            association: Association::new(session),
            read: None,
            read_done: false,
            edit: false,
        }
    }
    pub fn permission(
        &mut self,
        state: &std::sync::Arc<super::opencode_gate::RunState>,
        p: &Permission,
        get: impl FnMut() -> Result<Vec<ToolPart>>,
    ) -> Result<Option<Patch>> {
        let result = (|| {
            let input = self.association.input(p, get)?;
            match p.permission.as_str() {
                "read" if self.read.is_none() && !self.edit => {
                    self.read = Some(p.clone());
                    Ok(None)
                }
                "edit" if self.read_done && !self.edit => {
                    self.edit = true;
                    let patch = Association::patch(&p.id, &input)?;
                    Ok(Some(patch))
                }
                _ => Err("permission order/ticket".into()),
            }
        })();
        if result.is_err() {
            state.stop();
        }
        result
    }
    pub fn completed_read(
        &mut self,
        state: &std::sync::Arc<super::opencode_gate::RunState>,
        part: &ToolPart,
    ) -> Result<()> {
        let result = (|| {
            let r = self.read.as_ref().ok_or("completion without read")?;
            if self.read_done
                || part.status != "completed"
                || part.tool != "read"
                || part.session_id != r.session_id
                || part.call_id != r.call_id
                || part.message_id != r.message_id
            {
                return Err("unassociated/late read completion".into());
            }
            super::opencode_paths::target(
                part.input["filePath"]
                    .as_str()
                    .ok_or("read completion path")?,
                &r.patterns,
                false,
            )?;
            state.read_completed(
                std::time::Instant::now() + std::time::Duration::from_millis(100),
            )?;
            self.read_done = true;
            Ok(())
        })();
        if result.is_err() {
            state.stop();
        }
        result
    }
}
/// Finite control route vocabulary; no arbitrary URL or provider forwarding.
#[derive(Clone, Copy)]
pub enum Control {
    Health,
    Path,
    Project,
    Agent,
    Permissions,
    Status,
    Message,
    Event,
    Session,
    Prompt,
    Reply,
    Abort,
}
pub struct ControlBudget {
    pub received: u32,
    pub health: u32,
    pub event_open: bool,
}
impl ControlBudget {
    pub fn new() -> Self {
        Self {
            received: 0,
            health: 0,
            event_open: false,
        }
    }
    pub fn reserve(&mut self, route: Control) -> Result<()> {
        if matches!(route, Control::Event) {
            if self.event_open {
                return Err("event reconnect forbidden".into());
            }
            self.event_open = true;
            return Ok(());
        }
        self.received = self.received.checked_add(1).ok_or("control overflow")?;
        if self.received > 40 {
            return Err("control limit".into());
        }
        if matches!(route, Control::Health) {
            self.health += 1;
            if self.health > 5 {
                return Err("health limit".into());
            }
        }
        Ok(())
    }
}
/// Stateful bounded SSE framing. Caller owns a single connection and total deadline.
pub struct EventStream {
    data: Vec<u8>,
    total: usize,
}
impl EventStream {
    pub fn new() -> Self {
        Self {
            data: vec![],
            total: 0,
        }
    }
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<serde_json::Value>> {
        self.total = self
            .total
            .checked_add(bytes.len())
            .ok_or("event overflow")?;
        if self.total > 4 * 1024 * 1024 {
            return Err("event stream bound".into());
        }
        self.data.extend_from_slice(bytes);
        let mut out = vec![];
        while let Some(end) = self.data.windows(2).position(|w| w == b"\n\n") {
            if end > 256 * 1024 {
                return Err("event bound".into());
            }
            let frame = std::str::from_utf8(&self.data[..end]).map_err(|_| "event encoding")?;
            if !frame.starts_with("data: ") || frame[6..].contains('\n') {
                return Err("unknown SSE frame".into());
            }
            out.push(serde_json::from_str(&frame[6..]).map_err(|_| "event JSON")?);
            self.data.drain(..end + 2);
        }
        if self.data.len() > 256 * 1024 {
            return Err("event pending bound".into());
        }
        Ok(out)
    }
    pub fn end(self) -> Result<()> {
        if self.data.is_empty() {
            Ok(())
        } else {
            Err("event stream truncated".into())
        }
    }
}
