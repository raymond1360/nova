//! Inputs simulate locked Permission V1 / running ToolPart fields; not an OpenCode client.
use super::{hash, oracle, Result, PRE, PRE_HASH};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    pub run_id: String,
    pub role: String,
    pub attempt_id: String,
    pub target_id: String,
    pub expected_sha256: String,
    pub old_literal: String,
    pub new_literal: String,
    pub reason: String,
}
impl Patch {
    pub fn validate(&self, run: &str, attempt: &str, pre: &[u8]) -> Result<Vec<u8>> {
        if self.run_id != run
            || self.attempt_id != attempt
            || !["developer", "fix"].contains(&self.role.as_str())
            || self.target_id != "logo"
            || self.expected_sha256 != PRE_HASH
            || hash(pre) != self.expected_sha256
            || self.old_literal != "alt=\"Payload Logo\""
            || self.new_literal != "alt=\"Northstar Studio logo\""
            || self.reason.is_empty()
            || self.reason.len() > 1024
        {
            return Err("fixed patch contract rejected".into());
        }
        oracle(pre)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Permission {
    pub source: String,
    pub id: String,
    pub session_id: String,
    pub permission: String,
    pub patterns: Vec<String>,
    pub metadata: serde_json::Value,
    pub message_id: String,
    pub call_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolPart {
    pub source: String,
    pub session_id: String,
    pub message_id: String,
    pub call_id: String,
    pub tool: String,
    pub state: String,
    pub input: serde_json::Value,
}
#[derive(Debug)]
pub struct SyntheticGate {
    pub run_id: String,
    pub session_id: String,
    pub admission_open: bool,
    pub abort_requested: bool,
    pub abort_acknowledged: bool,
    pub idle: bool,
    pub control_http: u32,
    pub reject_sent: bool,
    pub reject_replied: bool,
    pub seen: BTreeSet<String>,
}
impl SyntheticGate {
    pub fn new(run: &str, session: &str) -> Self {
        Self {
            run_id: run.into(),
            session_id: session.into(),
            admission_open: true,
            abort_requested: false,
            abort_acknowledged: false,
            idle: false,
            control_http: 0,
            reject_sent: false,
            reject_replied: false,
            seen: BTreeSet::new(),
        }
    }
    pub fn stop(&mut self) {
        self.admission_open = false;
        self.abort_requested = true;
    }
    /// At most three synthetic GETs; no permission reply while input remains pending.
    pub fn proposal(
        &mut self,
        p: &Permission,
        mut get: impl FnMut() -> Result<Vec<ToolPart>>,
    ) -> Result<Patch> {
        let result = self.proposal_inner(p, &mut get);
        self.admission_open = false;
        if result.is_err() {
            self.abort_requested = true;
        }
        result
    }
    fn proposal_inner(
        &mut self,
        p: &Permission,
        get: &mut impl FnMut() -> Result<Vec<ToolPart>>,
    ) -> Result<Patch> {
        if !self.admission_open
            || p.source != "SYNTHETIC"
            || p.session_id != self.session_id
            || p.id.is_empty()
            || p.message_id.is_empty()
            || p.call_id.is_empty()
            || p.permission != "edit"
            || p.patterns != ["Logo.tsx"]
            || !self.seen.insert(p.id.clone())
        {
            return Err("permission association rejected".into());
        }
        for _ in 0..3 {
            self.control_http += 1;
            let parts = get()?;
            let matches: Vec<_> = parts.iter().filter(|t| t.call_id == p.call_id).collect();
            if matches.len() != 1 {
                return Err("missing/ambiguous tool call".into());
            }
            let t = matches[0];
            if t.source != "SYNTHETIC"
                || t.session_id != p.session_id
                || t.message_id != p.message_id
                || t.tool != "edit"
            {
                return Err("cross session/message or wrong tool".into());
            }
            if t.state == "pending" || t.input.as_object().is_some_and(|x| x.is_empty()) {
                continue;
            }
            if t.state != "running" {
                return Err("tool not running".into());
            }
            #[derive(Deserialize)]
            #[serde(deny_unknown_fields)]
            struct Edit {
                #[serde(rename = "filePath")]
                path: String,
                #[serde(rename = "oldString")]
                old: String,
                #[serde(rename = "newString")]
                new: String,
                #[serde(rename = "replaceAll", default)]
                all: bool,
            }
            let e: Edit = serde_json::from_value(t.input.clone()).map_err(|e| e.to_string())?;
            if e.path != "Logo.tsx" || e.all {
                return Err("nonfixed edit".into());
            }
            let patch = Patch {
                run_id: self.run_id.clone(),
                role: "developer".into(),
                attempt_id: p.id.clone(),
                target_id: "logo".into(),
                expected_sha256: PRE_HASH.into(),
                old_literal: e.old,
                new_literal: e.new,
                reason: "SYNTHETIC edit proposal".into(),
            };
            patch.validate(&self.run_id, &p.id, PRE)?;
            return Ok(patch);
        }
        Err("bounded input reread exhausted; reject and abort".into())
    }
    pub fn reject(&mut self) -> Result<&'static [u8]> {
        if self.admission_open || self.reject_sent {
            return Err("reject state invalid".into());
        }
        self.reject_sent = true;
        self.control_http += 1;
        Ok(br#"{"reply":"reject"}"#)
    }
    pub fn rejected(&mut self, id: &str) -> Result<()> {
        if !self.reject_sent || !self.seen.contains(id) || self.reject_replied {
            return Err("late/duplicate reply".into());
        }
        self.reject_replied = true;
        Ok(())
    }
    /// Covers deny-without-permission-event and tool failures; independent Nova stop path.
    pub fn unexpected_tool_error(&mut self) {
        self.stop();
    }
    pub fn abort(&mut self) -> Result<()> {
        if !self.abort_requested || self.abort_acknowledged {
            return Err("abort state invalid".into());
        }
        self.control_http += 1;
        self.abort_acknowledged = true;
        Ok(())
    }
    pub fn confirm_idle(&mut self) {
        self.idle = true;
    }
    pub fn cas_allowed(&self) -> bool {
        !self.admission_open
            && self.reject_sent
            && self.reject_replied
            && self.idle
            && !self.abort_requested
    }
}
