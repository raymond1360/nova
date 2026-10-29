//! Inputs simulate locked Permission V1 / running ToolPart fields; not an OpenCode client.
use super::budget::Budget;
use super::{hash, oracle, Result, PRE, PRE_HASH};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

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
    pub replace_all: bool,
}
impl Patch {
    pub fn validate(&self, run: &str, attempt: &str, pre: &[u8]) -> Result<Vec<u8>> {
        if self.run_id != run
            || self.attempt_id != attempt
            || !["developer", "fix"].contains(&self.role.as_str())
            || self.target_id != "logo"
            || self.expected_sha256 != PRE_HASH
            || hash(pre) != self.expected_sha256
            || self.replace_all
            || self.old_literal.is_empty()
            || self.old_literal.len() > 1024
            || self.new_literal.len() > 1024
            || self.old_literal.contains(['\0', '\r'])
            || self.new_literal.contains(['\0', '\r'])
            || self.reason.is_empty()
            || self.reason.len() > 1024
        {
            return Err("fixed patch contract rejected".into());
        }
        let old = self.old_literal.as_bytes();
        let matches: Vec<_> = pre
            .windows(old.len())
            .enumerate()
            .filter(|(_, b)| *b == old)
            .map(|(i, _)| i)
            .collect();
        if matches.len() != 1 {
            return Err("edit must match exactly once".into());
        }
        let at = matches[0];
        let candidate = [
            &pre[..at],
            self.new_literal.as_bytes(),
            &pre[at + old.len()..],
        ]
        .concat();
        if candidate.len() > 4096 || candidate != oracle(pre)? {
            return Err("Agent edit does not satisfy independent oracle".into());
        }
        Ok(candidate)
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
    ledger: Arc<Mutex<Budget>>,
    fixture_root: PathBuf,
    fixture_target: PathBuf,
    pub abort_acknowledged: bool,
    pub idle: bool,
    pub control_http: u32,
    pub reject_sent: bool,
    pub reject_replied: bool,
    pub seen: BTreeSet<String>,
}
impl SyntheticGate {
    pub fn new(run: &str, session: &str, root: &Path, ledger: Arc<Mutex<Budget>>) -> Result<Self> {
        if !root.is_absolute() {
            return Err("fixture root must be absolute".into());
        }
        let fixture_root = std::fs::canonicalize(root).map_err(|e| e.to_string())?;
        let fixture_target = fixture_root.join("Logo.tsx");
        super::cas::validate_final_path(&fixture_target, &fixture_root)?;
        Ok(Self {
            run_id: run.into(),
            session_id: session.into(),
            ledger,
            fixture_root,
            fixture_target,
            abort_acknowledged: false,
            idle: false,
            control_http: 0,
            reject_sent: false,
            reject_replied: false,
            seen: BTreeSet::new(),
        })
    }
    pub fn admission_open(&self) -> bool {
        self.ledger
            .lock()
            .map(|b| !b.closed && !b.cancelled && !b.abort_requested)
            .unwrap_or(false)
    }
    pub fn abort_requested(&self) -> bool {
        self.ledger
            .lock()
            .map(|b| b.abort_requested)
            .unwrap_or(true)
    }
    fn close(&self, abort: bool) -> Result<()> {
        self.ledger
            .lock()
            .map_err(|_| "poisoned ledger")?
            .close(abort);
        Ok(())
    }
    pub fn stop(&mut self) {
        let _ = self.close(true);
    }

    /// At most three synthetic GETs; no permission reply while input remains pending.
    pub fn proposal(
        &mut self,
        p: &Permission,
        mut get: impl FnMut() -> Result<Vec<ToolPart>>,
    ) -> Result<Patch> {
        let result = self.proposal_inner(p, &mut get);
        self.close(result.is_err())?;
        result
    }
    fn proposal_inner(
        &mut self,
        p: &Permission,
        get: &mut impl FnMut() -> Result<Vec<ToolPart>>,
    ) -> Result<Patch> {
        if !self.admission_open()
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
            if !Path::new(&e.path).is_absolute()
                || Path::new(&e.path) != self.fixture_target
                || e.all
            {
                return Err("absolute fixture mapping rejected".into());
            }
            super::cas::validate_final_path(Path::new(&e.path), &self.fixture_root)?;
            if p.patterns != ["Logo.tsx"] {
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
                replace_all: e.all,
            };
            patch.validate(&self.run_id, &p.id, PRE)?;
            return Ok(patch);
        }
        Err("bounded input reread exhausted; reject and abort".into())
    }
    pub fn reject(&mut self) -> Result<&'static [u8]> {
        if self.admission_open() || self.reject_sent {
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
        if !self.abort_requested() || self.abort_acknowledged {
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
        !self.admission_open()
            && self.reject_sent
            && self.reject_replied
            && self.idle
            && !self.abort_requested()
    }
}
