use super::Result;
use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize, Deserializer, Serialize,
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub const REPORT_LIMIT: usize = 2 * 1024 * 1024;
pub fn hash(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn hex(s: &str, n: usize) -> bool {
    s.len() == n
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
}
pub fn relative(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 512
        && !s.contains(['\\', ':', '*', '?', '[', ']', '\0'])
        && s.split('/')
            .all(|p| !p.is_empty() && p != "." && p != ".." && !p.ends_with(['.', ' ']))
}
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> std::result::Result<Option<T>, D::Error> {
    Option::deserialize(d)
}

// Parse recursively before typed decoding: Value alone would silently retain the last duplicate key.
struct Unique(Value);
impl<'de> Deserialize<'de> for Unique {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Unique;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("bounded JSON")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut m: M,
            ) -> std::result::Result<Unique, M::Error> {
                let mut out = serde_json::Map::new();
                while let Some(k) = m.next_key::<String>()? {
                    if out.contains_key(&k) {
                        return Err(de::Error::custom("duplicate key"));
                    }
                    out.insert(k, m.next_value::<Unique>()?.0);
                }
                Ok(Unique(Value::Object(out)))
            }
            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut s: S,
            ) -> std::result::Result<Unique, S::Error> {
                let mut v = vec![];
                while let Some(x) = s.next_element::<Unique>()? {
                    v.push(x.0)
                }
                Ok(Unique(Value::Array(v)))
            }
            fn visit_str<E: de::Error>(self, s: &str) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::String(s.into())))
            }
            fn visit_string<E: de::Error>(self, s: String) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::String(s)))
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Bool(b)))
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Unique, E> {
                Ok(Unique(Value::Null))
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> std::result::Result<Unique, E> {
                Ok(Unique(n.into()))
            }
            fn visit_u64<E: de::Error>(self, n: u64) -> std::result::Result<Unique, E> {
                Ok(Unique(n.into()))
            }
            fn visit_f64<E: de::Error>(self, n: f64) -> std::result::Result<Unique, E> {
                serde_json::Number::from_f64(n)
                    .map(|v| Unique(Value::Number(v)))
                    .ok_or_else(|| de::Error::custom("number"))
            }
        }
        d.deserialize_any(V)
    }
}
pub fn json<T: serde::de::DeserializeOwned>(b: &[u8]) -> Result<T> {
    if b.is_empty() || b.len() > REPORT_LIMIT {
        return Err("INPUT_BOUND");
    }
    let u: Unique = serde_json::from_slice(b).map_err(|_| "SCHEMA_REJECTED")?;
    fn bounds(v: &Value) -> bool {
        match v {
            Value::String(s) => s.len() <= 2048,
            Value::Array(a) => a.len() <= 256 && a.iter().all(bounds),
            Value::Object(m) => m.len() <= 64 && m.values().all(bounds),
            _ => true,
        }
    }
    if !bounds(&u.0) {
        return Err("INPUT_BOUND");
    }
    serde_json::from_value(u.0).map_err(|_| "SCHEMA_REJECTED")
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Baseline {
    pub mode: String,
    #[serde(deserialize_with = "nullable")]
    pub base_sha: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: String,
    pub package_id: String,
    pub repo_path: String,
    pub title: String,
    pub goal: String,
    pub allowed_paths: Vec<String>,
    pub acceptance_items: Vec<String>,
    pub pm_decision_ref: String,
    pub report_roots: Vec<String>,
    pub required_reports: Vec<ReportType>,
    pub baseline: Baseline,
    pub eol_policy: String,
}
impl Manifest {
    pub fn validate(&self) -> Result<()> {
        if self.schema != "nova.sh1.package/1"
            || !id(&self.package_id)
            || self.title.trim().is_empty()
            || self.goal.trim().is_empty()
            || self.pm_decision_ref.trim().is_empty()
        {
            return Err("MANIFEST_REJECTED");
        }
        if !["true", "input", "false"].contains(&self.eol_policy.as_str()) {
            return Err("EOL_POLICY_REQUIRED");
        }
        match self.baseline.mode.as_str() {
            "PRE_WORK" if self.baseline.base_sha.is_none() => (),
            "DECLARED" if self.baseline.base_sha.as_ref().is_some_and(|s| hex(s, 40)) => (),
            _ => return Err("BASELINE_REJECTED"),
        }
        let paths: BTreeSet<_> = self.allowed_paths.iter().collect();
        let folded: BTreeSet<_> = self
            .allowed_paths
            .iter()
            .map(|s| s.to_lowercase())
            .collect();
        if paths.is_empty()
            || paths.len() > 64
            || paths.len() != self.allowed_paths.len()
            || folded.len() != paths.len()
            || !paths.iter().all(|s| relative(s))
        {
            return Err("SCOPE_REJECTED");
        }
        if self.report_roots.is_empty()
            || self.report_roots.len() > 8
            || self.acceptance_items.is_empty()
            || self.acceptance_items.len() > 64
        {
            return Err("MANIFEST_REJECTED");
        }
        let required: BTreeSet<_> = self.required_reports.iter().copied().collect();
        if required != BTreeSet::from([ReportType::Development, ReportType::Review, ReportType::Ci])
            || required.len() != self.required_reports.len()
        {
            return Err("REQUIRED_SLOTS_REJECTED");
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReportType {
    Development,
    Review,
    Ci,
    PmDecision,
}
impl ReportType {
    pub fn role(self) -> &'static str {
        match self {
            Self::Development => "development",
            Self::Review => "review",
            Self::Ci => "ci",
            Self::PmDecision => "pm",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Conclusion {
    Pass,
    Fail,
    Blocked,
    NotRun,
    Recorded,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Severity {
    Blocker,
    High,
    Medium,
    Low,
    Info,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum IssueStatus {
    Open,
    Closed,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Issue {
    pub id: String,
    pub severity: Severity,
    pub blocking: bool,
    pub status: IssueStatus,
    pub first_seen_sha: String,
    #[serde(deserialize_with = "nullable")]
    pub closed_sha: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CiRun {
    pub url: String,
    pub claimed_head_sha: String,
    pub claimed_conclusion: Conclusion,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Claims {
    #[serde(deserialize_with = "nullable")]
    pub author: Option<String>,
    pub role: String,
    #[serde(deserialize_with = "nullable")]
    pub fresh_session: Option<bool>,
    #[serde(deserialize_with = "nullable")]
    pub model: Option<String>,
    pub ci_runs: Vec<CiRun>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolution {
    pub delivery_identity: String,
    pub report_type: ReportType,
    pub candidate_receipt_ids: Vec<String>,
    pub selected_receipt_id: String,
    #[serde(deserialize_with = "nullable")]
    pub supersedes_resolution_receipt_id: Option<String>,
    pub reason: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub package_id: String,
    pub stage: String,
    #[serde(deserialize_with = "nullable")]
    pub supersedes_sha: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub pm_decision_ref: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub decision_kind: Option<String>,
    #[serde(deserialize_with = "nullable")]
    pub slot_resolution: Option<Resolution>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceReport {
    pub receipt_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Transcriber {
    pub name: String,
    pub role: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub schema: String,
    pub report_type: ReportType,
    pub claimed_delivery_sha: String,
    pub claimed_conclusion: Conclusion,
    pub issues: Vec<Issue>,
    pub context: Context,
    pub claims: Claims,
    #[serde(deserialize_with = "nullable")]
    pub source_report: Option<SourceReport>,
    #[serde(deserialize_with = "nullable")]
    pub transcribed_by: Option<Transcriber>,
}
impl Report {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(bytes).map_err(|_| "SCHEMA_REJECTED")?;
        let marker = "```nova-handoff-v1";
        let payload = if text.trim_start().starts_with('{') {
            text.trim()
        } else {
            if text.matches(marker).count() != 1 {
                return Err("FORMAT_REQUIRED");
            }
            let rest = text.split_once(marker).ok_or("FORMAT_REQUIRED")?.1;
            if !rest.starts_with(['\r', '\n']) {
                return Err("SCHEMA_REJECTED");
            }
            rest.split_once("```").ok_or("SCHEMA_REJECTED")?.0.trim()
        };
        if bytes.len() > REPORT_LIMIT {
            return Err("INPUT_BOUND");
        }
        let r: Self = json(payload.as_bytes())?;
        r.validate()?;
        Ok(r)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema != "nova.scheduler.report/1"
            || !hex(&self.claimed_delivery_sha, 40)
            || !id(&self.context.package_id)
            || self.claims.role != self.report_type.role()
        {
            return Err("SCHEMA_REJECTED");
        }
        if !["INITIAL", "REPAIR", "REFERENCE"].contains(&self.context.stage.as_str()) {
            return Err("SCHEMA_REJECTED");
        }
        if self.context.stage == "REPAIR"
            && (self.report_type != ReportType::Development
                || !self
                    .context
                    .supersedes_sha
                    .as_ref()
                    .is_some_and(|s| hex(s, 40))
                || !self
                    .context
                    .pm_decision_ref
                    .as_ref()
                    .is_some_and(|s| !s.trim().is_empty()))
        {
            return Err("REPAIR_REFERENCE_REQUIRED");
        }
        let mut ids = BTreeSet::new();
        if self.issues.len() > 64 {
            return Err("INPUT_BOUND");
        }
        for i in &self.issues {
            if !id(&i.id)
                || !ids.insert(&i.id)
                || !hex(&i.first_seen_sha, 40)
                || (i.status == IssueStatus::Open) != i.closed_sha.is_none()
                || i.closed_sha.as_ref().is_some_and(|s| !hex(s, 40))
            {
                return Err("ISSUE_REJECTED");
            }
            if matches!(i.severity, Severity::Blocker | Severity::High) && !i.blocking {
                return Err("SEVERITY_BLOCKING_CONTRADICTION");
            }
        }
        if let Some(src) = &self.source_report {
            if !id(&src.receipt_id)
                || !self
                    .transcribed_by
                    .as_ref()
                    .is_some_and(|t| !t.name.trim().is_empty() && t.role == "transcriber")
            {
                return Err("DERIVATION_REJECTED");
            }
        } else if self.transcribed_by.is_some() {
            return Err("DERIVATION_REJECTED");
        }
        for ci in &self.claims.ci_runs {
            let url = url::Url::parse(&ci.url).map_err(|_| "CI_CLAIM_REJECTED")?;
            if url.scheme() != "https"
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || ci.claimed_head_sha != self.claimed_delivery_sha
            {
                return Err("CI_CLAIM_REJECTED");
            }
        }
        if self.report_type == ReportType::PmDecision {
            if !self
                .claims
                .author
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            {
                return Err("PM_AUTHOR_CLAIM_REQUIRED");
            }
            if !self
                .context
                .pm_decision_ref
                .as_ref()
                .is_some_and(|s| !s.trim().is_empty())
            {
                return Err("PM_REFERENCE_REQUIRED");
            }
            match self.context.decision_kind.as_deref() {
                Some(
                    "CONTINUE_MATERIALS" | "PAUSE" | "CANCEL" | "REPAIR_REFERENCE"
                    | "CLOSURE_REFERENCE",
                ) if self.context.slot_resolution.is_none() => (),
                Some("SLOT_RESOLUTION") if self.context.slot_resolution.is_some() => (),
                _ => return Err("PM_KIND_REJECTED"),
            }
            if let Some(s) = &self.context.slot_resolution {
                let unique: BTreeSet<_> = s.candidate_receipt_ids.iter().collect();
                if !hex(&s.delivery_identity, 64)
                    || s.report_type == ReportType::PmDecision
                    || unique.len() != s.candidate_receipt_ids.len()
                    || unique.len() < 2
                    || unique.len() > 64
                    || !unique.contains(&s.selected_receipt_id)
                    || !unique.iter().all(|v| id(v))
                    || s.reason.trim().is_empty()
                {
                    return Err("RESOLUTION_REJECTED");
                }
            }
        } else if self.context.decision_kind.is_some() || self.context.slot_resolution.is_some() {
            return Err("PM_KIND_REJECTED");
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TreeEntry {
    pub path: String,
    pub mode: String,
    pub object: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FileFact {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub identity: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub head: String,
    pub parent: String,
    pub base: String,
    pub base_source: String,
    pub root_identity: String,
    pub git_dir: String,
    pub tree: Vec<TreeEntry>,
    pub files: Vec<FileFact>,
    pub ancestors: Vec<String>,
    pub attributes: Value,
    pub outside_scope_notice: String,
    pub observed_utc: String,
    pub delivery_identity: String,
    pub envelope_id: String,
}
impl Envelope {
    pub fn bind(&mut self, m: &Manifest) -> Result<()> {
        if !hex(&self.head, 40)
            || !hex(&self.parent, 40)
            || !hex(&self.base, 40)
            || !self.ancestors.contains(&self.base)
            || !self.ancestors.contains(&self.head)
        {
            return Err("VERSION_UNKNOWN");
        }
        self.tree.sort_by(|a, b| a.path.cmp(&b.path));
        self.delivery_identity = hash(
            &serde_json::to_vec(&(
                &m.package_id,
                &self.root_identity,
                &self.base,
                &self.head,
                &m.allowed_paths,
                &self.tree,
            ))
            .map_err(|_| "ENCODING")?,
        );
        self.envelope_id.clear();
        self.envelope_id = hash(&serde_json::to_vec(self).map_err(|_| "ENCODING")?);
        Ok(())
    }
    pub fn validate_report(&self, m: &Manifest, r: &Report) -> Result<()> {
        if r.context.package_id != m.package_id || r.claimed_delivery_sha != self.head {
            return Err("VERSION_MISMATCH");
        }
        for i in &r.issues {
            let first = self
                .ancestors
                .iter()
                .position(|v| v == &i.first_seen_sha)
                .ok_or("ISSUE_ANCESTRY_UNKNOWN")?;
            if let Some(closed) = &i.closed_sha {
                let c = self
                    .ancestors
                    .iter()
                    .position(|v| v == closed)
                    .ok_or("ISSUE_ANCESTRY_UNKNOWN")?;
                if c > first {
                    return Err("ISSUE_ANCESTRY_UNKNOWN");
                }
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Source {
    ToolGitRead,
    ToolFileHash,
    HumanAttested,
}
impl Source {
    pub fn base(s: &str) -> Result<Self> {
        match s {
            "TOOL_GIT_READ" => Ok(Self::ToolGitRead),
            "HUMAN_ATTESTED" => Ok(Self::HumanAttested),
            _ => Err("STORE_INCONSISTENT"),
        }
    }
}
pub fn attributed<T: Serialize>(value: T, source: Source) -> Value {
    serde_json::json!({"value":value,"source":source})
}
pub fn issue_map(r: &Report) -> BTreeMap<&str, &Issue> {
    r.issues.iter().map(|i| (i.id.as_str(), i)).collect()
}

// Separate AR0 bounds; V1 parser and DTOs remain unchanged.
pub fn relay_json<T: serde::de::DeserializeOwned>(b: &[u8], limit: usize) -> Result<T> {
    if b.is_empty() || b.len() > limit {
        return Err("BOUND");
    }
    let u: Unique = serde_json::from_slice(b).map_err(|_| "SCHEMA_REJECTED")?;
    fn valid(v: &Value, d: usize) -> bool {
        if d > 32 {
            return false;
        }
        match v {
            Value::String(s) => s.len() <= 65536,
            Value::Array(a) => a.len() <= 256 && a.iter().all(|v| valid(v, d + 1)),
            Value::Object(m) => {
                m.len() <= 128 && m.iter().all(|(k, v)| k.len() <= 256 && valid(v, d + 1))
            }
            _ => true,
        }
    }
    if !valid(&u.0, 0) {
        return Err("BOUND");
    }
    serde_json::from_value(u.0).map_err(|_| "SCHEMA_REJECTED")
}
pub const RELAY_SIGNATURE: &str = "pub fn normalize_label(s: &str) -> String {";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayLimits {
    pub stage_ms: u64,
    pub total_ms: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RelayPlan {
    pub schema: String,
    pub package_id: String,
    pub registration_revision: i64,
    pub input_base_sha: String,
    pub input_delivery_identity: String,
    pub target_path: String,
    pub base_sha256: String,
    pub goal: String,
    pub constraints: Vec<String>,
    pub examples: Vec<String>,
    pub develop_profile: String,
    pub review_profile: String,
    pub model_develop: String,
    pub model_review: String,
    pub pm_decision_ref: String,
    pub limits: RelayLimits,
    pub mock: bool,
}
impl RelayPlan {
    pub fn validate(&self) -> Result<()> {
        if self.schema != "nova.relay.plan/1"
            || !id(&self.package_id)
            || self.registration_revision < 0
            || !hex(&self.input_base_sha, 40)
            || !hex(&self.input_delivery_identity, 64)
            || self.target_path != "pilot/src/normalize_label.rs"
            || !hex(&self.base_sha256, 64)
            || self.goal.is_empty()
            || self.goal.len() > 4096
            || self.constraints.len() > 16
            || self.examples.len() > 16
            || self
                .constraints
                .iter()
                .chain(&self.examples)
                .any(|x| x.len() > 1024)
            || self.develop_profile != "codex-exec-0.162.1"
            || self.review_profile != "claude-print-2.1.197"
            || [&self.model_develop, &self.model_review]
                .iter()
                .any(|x| x.is_empty() || x.len() > 128 || x.contains(['\0', '\r', '\n']))
            || self.pm_decision_ref.trim().is_empty()
            || self.pm_decision_ref.len() > 2048
            || self.limits.stage_ms == 0
            || self.limits.stage_ms > 300000
            || self.limits.total_ms > 600000
            || self.limits.total_ms < 60000
        {
            return Err("SCHEMA_REJECTED");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Proposal {
    pub schema: String,
    pub target_path: String,
    pub old_text: String,
    pub new_text: String,
    pub rationale: String,
}
impl Proposal {
    pub fn validate(&self, p: &RelayPlan) -> Result<()> {
        self.validate_target(&p.target_path)
    }
    pub fn validate_target(&self, target: &str) -> Result<()> {
        if self.schema != "nova.relay.proposal/1"
            || self.target_path != target
            || self.old_text.is_empty()
            || self.old_text.len() > 8192
            || self.new_text.len() > 16384
            || self.rationale.len() > 2048
            || self.old_text.contains(['\0', '\r'])
            || self.new_text.contains(['\0', '\r'])
        {
            return Err("SCHEMA_REJECTED");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewIssue {
    pub id: String,
    pub severity: Severity,
    pub blocking: bool,
    pub location: String,
    pub summary: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewClaim {
    pub schema: String,
    pub base_sha256: String,
    pub candidate_sha256: String,
    pub verdict: String,
    pub issues: Vec<ReviewIssue>,
    pub unknowns: Vec<String>,
}
impl ReviewClaim {
    pub fn validate(&self, b: &str, c: &str) -> Result<()> {
        if self.schema != "nova.relay.review/1"
            || self.base_sha256 != b
            || self.candidate_sha256 != c
        {
            return Err("BINDING_MISMATCH");
        }
        if !["PASS", "CHANGES_REQUIRED", "BLOCKED"].contains(&self.verdict.as_str())
            || self.issues.len() > 32
            || self.unknowns.len() > 16
            || self.unknowns.iter().any(|s| s.len() > 512)
        {
            return Err("SCHEMA_REJECTED");
        }
        let mut seen = BTreeSet::new();
        for i in &self.issues {
            let n = i.id.strip_prefix("R-").ok_or("SCHEMA_REJECTED")?;
            if n.is_empty()
                || n.len() > 3
                || !n.bytes().all(|b| b.is_ascii_digit())
                || !seen.insert(&i.id)
                || i.location.len() > 256
                || i.summary.len() > 1024
            {
                return Err("SCHEMA_REJECTED");
            }
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RelayRole {
    Develop,
    Review,
}
impl RelayRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Develop => "DEVELOP",
            Self::Review => "REVIEW",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub attempt_id: String,
    pub role: RelayRole,
    pub state: String,
    pub state_version: i64,
    pub input_identity: String,
    pub parent_attempt_id: Option<String>,
    pub profile_id: String,
    pub exe_sha256: String,
    pub argv_sha256: String,
    pub env_names_sha256: String,
    pub started_utc: Option<String>,
    pub ended_utc: Option<String>,
    pub pid: Option<u32>,
    pub process_creation_utc: Option<String>,
    pub exit_code: Option<i64>,
    pub terminal: Option<String>,
    pub reclaim: Option<String>,
    pub session_id: Option<String>,
    pub stdout_bytes: Option<u64>,
    pub stdout_sha256: Option<String>,
    pub stderr_bytes: Option<u64>,
    pub stderr_sha256: Option<String>,
    pub result_sha256: Option<String>,
    // SQLite carrier only: model claims are projected separately from tool facts.
    #[serde(skip_serializing, default)]
    pub model_claim_json: Option<String>,
    pub usage_json: Option<String>,
}
// RP1 is a separate wire contract. RelayPlan above is deliberately unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealPlanCore {
    pub schema: String,
    pub run_mode: String,
    pub package_id: String,
    pub registration_revision: i64,
    pub input_base_sha: String,
    pub input_delivery_identity: String,
    pub target_path: String,
    pub base_sha256: String,
    pub goal: String,
    pub constraints: Vec<String>,
    pub examples: Vec<String>,
    pub develop_profile: String,
    pub review_profile: String,
    pub model_develop: String,
    pub model_review: String,
    pub pm_decision_ref: String,
    pub limits: RelayLimits,
}
impl RealPlanCore {
    /// An internal common-data view, never encoded as a real plan or accepted by old run.
    pub fn common(&self) -> RelayPlan {
        RelayPlan {
            schema: "nova.relay.plan/1".into(),
            package_id: self.package_id.clone(),
            registration_revision: self.registration_revision,
            input_base_sha: self.input_base_sha.clone(),
            input_delivery_identity: self.input_delivery_identity.clone(),
            target_path: self.target_path.clone(),
            base_sha256: self.base_sha256.clone(),
            goal: self.goal.clone(),
            constraints: self.constraints.clone(),
            examples: self.examples.clone(),
            develop_profile: self.develop_profile.clone(),
            review_profile: self.review_profile.clone(),
            model_develop: self.model_develop.clone(),
            model_review: self.model_review.clone(),
            pm_decision_ref: self.pm_decision_ref.clone(),
            limits: self.limits.clone(),
            mock: false,
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.common().validate()?;
        if self.schema != "nova.relay.plan-core/2"
            || self.run_mode != "REAL_PILOT_ONCE"
            || !real_model(&self.model_develop)
            || !real_model(&self.model_review)
        {
            return Err("SCHEMA_REJECTED");
        }
        Ok(())
    }
}
pub fn real_model(s: &str) -> bool {
    let b = s.as_bytes();
    if !(3..=64).contains(&b.len())
        || !b[0].is_ascii_lowercase()
        || !b.iter().any(u8::is_ascii_digit)
    {
        return false;
    }
    let mut sep = false;
    let mut had_sep = false;
    for &c in b {
        if c == b'.' || c == b'-' {
            if sep {
                return false;
            }
            sep = true;
            had_sep = true;
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            sep = false;
        } else {
            return false;
        }
    }
    !sep && had_sep && !["default", "sonnet", "opus", "haiku", "best", "opusplan"].contains(&s)
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedController {
    pub source_sha: String,
    pub build_receipt_sha256: String,
    pub exe_sha256: String,
    pub exe_bytes: u64,
    pub cargo_profile: String,
    pub features: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedRoots {
    pub source_registration: String,
    pub workspace: String,
    pub run_root: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedTool {
    pub role: RelayRole,
    pub profile_id: String,
    pub exe_path: String,
    pub exe_bytes: u64,
    pub exe_sha256: String,
    pub model: String,
    pub auth_category: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedBilling {
    pub route: String,
    pub extra_usage_allowed: bool,
    pub hard_cap_claimed: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealPilotApproval {
    pub schema: String,
    pub attestation: String,
    pub approval_id: String,
    pub pm_task_ref: String,
    pub approver_note: String,
    pub issued_utc: String,
    pub expires_utc: String,
    pub plan_core_sha256: String,
    pub controller: ApprovedController,
    pub roots: ApprovedRoots,
    pub tools: Vec<ApprovedTool>,
    pub limits: RelayLimits,
    pub billing: ApprovedBilling,
    pub single_use: bool,
}
impl RealPilotApproval {
    pub fn check_time(&self, now: chrono::DateTime<chrono::Utc>) -> Result<()> {
        let parse = |s: &str| {
            if !s.ends_with('Z') {
                return Err("APPROVAL_TIME");
            }
            chrono::DateTime::parse_from_rfc3339(s)
                .map(|v| v.with_timezone(&chrono::Utc))
                .map_err(|_| "APPROVAL_TIME")
        };
        let issued = parse(&self.issued_utc)?;
        let expires = parse(&self.expires_utc)?;
        if expires <= issued
            || expires - issued > chrono::Duration::hours(24)
            || now < issued - chrono::Duration::minutes(5)
            || now >= expires
        {
            return Err("APPROVAL_EXPIRED");
        }
        Ok(())
    }
    pub fn validate(&self, c: &RealPlanCore) -> Result<()> {
        c.validate()?;
        if self.schema != "nova.relay.real-approval/1"
            || self.attestation != "HUMAN_ATTESTED"
            || !hex(&self.approval_id, 32)
            || self.pm_task_ref.trim().is_empty()
            || self.pm_task_ref.len() > 512
            || self.approver_note.len() > 256
            || !self.single_use
            || self.billing.route != "SUBSCRIPTION_ONLY"
            || self.billing.hard_cap_claimed
            || self.tools.len() != 2
            || !hex(&self.controller.source_sha, 40)
            || !hex(&self.controller.build_receipt_sha256, 64)
            || !hex(&self.controller.exe_sha256, 64)
            || self.controller.exe_bytes == 0
            || self.controller.cargo_profile != "dev"
            || self.controller.features != ["scheduler-validation"]
            || self.plan_core_sha256 != hash(relay_encoded(c)?.as_bytes())
            || self.limits.stage_ms != c.limits.stage_ms
            || self.limits.total_ms != c.limits.total_ms
        {
            return Err("APPROVAL_MISMATCH");
        }
        for (i, role) in [RelayRole::Develop, RelayRole::Review].iter().enumerate() {
            let t = &self.tools[i];
            if t.role != *role
                || t.profile_id.as_str()
                    != if i == 0 {
                        c.develop_profile.as_str()
                    } else {
                        c.review_profile.as_str()
                    }
                || t.model.as_str()
                    != if i == 0 {
                        c.model_develop.as_str()
                    } else {
                        c.model_review.as_str()
                    }
                || t.auth_category
                    != if i == 0 {
                        "CODEX_CHATGPT"
                    } else {
                        "CLAUDE_AI_SUBSCRIPTION"
                    }
            {
                return Err("APPROVAL_MISMATCH");
            }
        }
        for p in [
            &self.roots.source_registration,
            &self.roots.workspace,
            &self.roots.run_root,
        ] {
            super::store::absolute(p.as_ref())?;
            super::store::safe_location(p.as_ref())?;
        }
        if self.roots.workspace == self.roots.run_root
            || self.roots.workspace == self.roots.source_registration
            || self.roots.run_root == self.roots.source_registration
        {
            return Err("APPROVAL_MISMATCH");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildReceipt {
    pub schema: String,
    pub source_sha: String,
    pub source_claim_source: String,
    pub clean: bool,
    pub cargo_profile: String,
    pub features: Vec<String>,
    pub cargo_command: Vec<String>,
    pub exe_bytes: u64,
    pub exe_sha256: String,
    pub exe_identity_source: String,
}
impl BuildReceipt {
    pub fn validate(&self, a: &RealPilotApproval, raw: &[u8]) -> Result<()> {
        if self.schema != "nova.relay.build-receipt/1"
            || self.source_claim_source != "HUMAN_ATTESTED"
            || self.exe_identity_source != "TOOL_FILE_HASH"
            || !self.clean
            || self.source_sha != a.controller.source_sha
            || self.cargo_profile != "dev"
            || self.features != a.controller.features
            || self.exe_bytes != a.controller.exe_bytes
            || self.exe_sha256 != a.controller.exe_sha256
            || hash(raw) != a.controller.build_receipt_sha256
            || self.cargo_command
                != [
                    "cargo",
                    "build",
                    "--offline",
                    "--locked",
                    "--features",
                    "scheduler-validation",
                    "--bin",
                    "nova_scheduler_relay",
                ]
        {
            return Err("BUILD_RECEIPT_MISMATCH");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RealPlanRecord {
    pub schema: String,
    pub core: RealPlanCore,
    pub approval: RealPilotApproval,
    pub approval_file_sha256: String,
    pub build_receipt_sha256: String,
}
impl RealPlanRecord {
    pub fn validate(&self) -> Result<()> {
        if relay_encoded(self)?.len() > 65536 {
            return Err("BOUND");
        }
        if self.schema != "nova.relay.plan/2"
            || !hex(&self.approval_file_sha256, 64)
            || self.build_receipt_sha256 != self.approval.controller.build_receipt_sha256
        {
            return Err("BINDING_MISMATCH");
        }
        self.approval
            .validate(&self.core)
            .map_err(|_| "BINDING_MISMATCH")
    }
}
#[derive(Clone, Debug)]
pub enum PlanMode {
    Mock(RelayPlan),
    RealPilot(RealPlanRecord),
}
impl PlanMode {
    pub fn common(&self) -> RelayPlan {
        match self {
            Self::Mock(p) => p.clone(),
            Self::RealPilot(p) => p.core.common(),
        }
    }
    pub fn is_real(&self) -> bool {
        matches!(self, Self::RealPilot(_))
    }
    pub fn record(&self) -> Option<&RealPlanRecord> {
        if let Self::RealPilot(r) = self {
            Some(r)
        } else {
            None
        }
    }
}
/// Mode-bearing driver input. Common v1-shaped data is only an internal compatibility view.
pub struct PlanView<'a> {
    pub mode: &'a PlanMode,
    common: RelayPlan,
}
impl<'a> PlanView<'a> {
    pub fn new(mode: &'a PlanMode) -> Self {
        Self {
            common: mode.common(),
            mode,
        }
    }
}
impl std::ops::Deref for PlanView<'_> {
    type Target = RelayPlan;
    fn deref(&self) -> &RelayPlan {
        &self.common
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CleanupReceipt {
    pub root_exited_before: bool,
    pub active_before: Option<u32>,
    pub terminated: bool,
    pub job_confirmed: bool,
    pub readers_finished: bool,
}
impl CleanupReceipt {
    pub fn diagnostics(&self) -> Vec<String> {
        let mut v = vec![];
        if self.root_exited_before && self.active_before.is_some_and(|n| n > 0) {
            v.push("DESCENDANT_TERMINATED".into());
        }
        if self.active_before.is_none() {
            v.push("CLEANUP_STATE_UNKNOWN".into());
        }
        v
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EligibilityReceipt {
    pub exit_code: Option<i64>,
    pub category: String,
    pub stdout_bytes: u64,
    pub stderr_bytes: u64,
}

pub fn relay_encoded<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|_| "ENCODING")
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ApiSource {
    InjectedTest,
}
impl ApiSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InjectedTest => "INJECTED_TEST",
        }
    }
}

// API injected contract is deliberately separate from CLI wire types.
pub const API_FIXTURE: &[u8] =
    b"pub fn normalize_label(s: &str) -> String {\n    s.to_string()\n}\n";
pub const API_TARGET: &str = "pilot/src/normalize_label.rs";
pub const API_H: &str = "3333333333333333333333333333333333333333";
pub const API_B: &str = "1111111111111111111111111111111111111111";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiLimits {
    pub stage_ms: u64,
    pub total_ms: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiPlanRequest {
    pub schema: String,
    pub mode: String,
    pub package_id: String,
    pub registration_revision: i64,
    pub input_base_sha: String,
    pub target_path: String,
    pub base_sha256: String,
    pub goal: String,
    pub constraints: Vec<String>,
    pub examples: Vec<String>,
    pub protocol: String,
    pub model_develop: String,
    pub model_review: String,
    pub limits: ApiLimits,
    pub pm_decision_ref: String,
}
impl ApiPlanRequest {
    pub fn validate(&self) -> Result<()> {
        if self.mode != "API_INJECTED_ONLY" {
            return Err("REAL_API_NOT_AUTHORIZED");
        }
        if self.schema != "nova.scheduler.api-plan-request/1"
            || !id(&self.package_id)
            || self.registration_revision != 0
            || self.input_base_sha != API_H
            || self.target_path != API_TARGET
            || self.base_sha256 != hash(API_FIXTURE)
            || self.protocol != "API_JSON_INJECTED_V1"
            || self.model_develop != "injected-develop-v1"
            || self.model_review != "injected-review-v1"
            || self.goal.is_empty()
            || self.goal.len() > 4096
            || self.constraints.len() > 16
            || self.examples.len() > 16
            || self
                .constraints
                .iter()
                .chain(&self.examples)
                .any(|s| s.len() > 1024)
            || self.pm_decision_ref.trim().is_empty()
            || self.pm_decision_ref.len() > 2048
            || self.limits.stage_ms == 0
            || self.limits.stage_ms > 45000
            || self.limits.total_ms < 25000
            || self.limits.total_ms > 120000
            || self.limits.input_tokens == 0
            || self.limits.input_tokens > 8192
            || self.limits.output_tokens == 0
            || self.limits.output_tokens > 4096
        {
            return Err("SCHEMA_REJECTED");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiPlan {
    pub schema: String,
    pub mode: String,
    pub package_id: String,
    pub registration_revision: i64,
    pub input_base_sha: String,
    pub input_delivery_identity: String,
    pub target_path: String,
    pub base_sha256: String,
    pub goal: String,
    pub constraints: Vec<String>,
    pub examples: Vec<String>,
    pub protocol: String,
    pub model_develop: String,
    pub model_review: String,
    pub limits: ApiLimits,
    pub pm_decision_ref: String,
    pub plan_request_sha256: String,
}
impl ApiPlan {
    pub fn bind(r: ApiPlanRequest, delivery: String, request_hash: String) -> Self {
        Self {
            schema: "nova.scheduler.api-plan/1".into(),
            mode: r.mode,
            package_id: r.package_id,
            registration_revision: r.registration_revision,
            input_base_sha: r.input_base_sha,
            input_delivery_identity: delivery,
            target_path: r.target_path,
            base_sha256: r.base_sha256,
            goal: r.goal,
            constraints: r.constraints,
            examples: r.examples,
            protocol: r.protocol,
            model_develop: r.model_develop,
            model_review: r.model_review,
            limits: r.limits,
            pm_decision_ref: r.pm_decision_ref,
            plan_request_sha256: request_hash,
        }
    }
    pub fn request(&self) -> ApiPlanRequest {
        ApiPlanRequest {
            schema: "nova.scheduler.api-plan-request/1".into(),
            mode: self.mode.clone(),
            package_id: self.package_id.clone(),
            registration_revision: self.registration_revision,
            input_base_sha: self.input_base_sha.clone(),
            target_path: self.target_path.clone(),
            base_sha256: self.base_sha256.clone(),
            goal: self.goal.clone(),
            constraints: self.constraints.clone(),
            examples: self.examples.clone(),
            protocol: self.protocol.clone(),
            model_develop: self.model_develop.clone(),
            model_review: self.model_review.clone(),
            limits: self.limits.clone(),
            pm_decision_ref: self.pm_decision_ref.clone(),
        }
    }
    pub fn validate(&self) -> Result<()> {
        self.request().validate()?;
        if self.schema != "nova.scheduler.api-plan/1"
            || !hex(&self.input_delivery_identity, 64)
            || !hex(&self.plan_request_sha256, 64)
        {
            return Err("BINDING_MISMATCH");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiRequestMeta {
    pub schema: String,
    pub protocol: String,
    pub role: RelayRole,
    pub plan_id: String,
    pub input_sha256: String,
    pub body_sha256: String,
    pub body_bytes: u64,
    pub logical_endpoint: String,
    pub requested_model: String,
    pub source: String,
    pub parent_attempt_id: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiFacts {
    pub schema: String,
    pub source: String,
    pub dispatch_invoked: bool,
    pub response_status: Option<u16>,
    pub request_bytes: u64,
    pub response_bytes: u64,
    pub response_sha256: Option<String>,
    pub transport_state: String,
    pub local_cancel_requested: bool,
    pub local_resources_released: bool,
    pub error_code: String,
    pub unknowns: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApiAttempt {
    pub attempt_id: String,
    pub role: RelayRole,
    pub state: String,
    pub state_version: i64,
    pub input_identity: String,
    pub parent_attempt_id: Option<String>,
    pub meta: ApiRequestMeta,
    pub request_sha256: String,
    pub facts: Option<ApiFacts>,
    pub model_claim: Option<serde_json::Value>,
    pub binding: Option<serde_json::Value>,
    pub evidence: Option<serde_json::Value>,
}
/// Duplicate-key rejection is shared; API has stricter independent traversal limits.
pub fn api_json<T: serde::de::DeserializeOwned>(b: &[u8], cap: usize) -> Result<T> {
    let v: serde_json::Value = relay_json(b, cap)?;
    fn walk(v: &serde_json::Value, d: usize, n: &mut usize) -> Result<()> {
        if d > 16 {
            return Err("BOUND");
        };
        *n = n.checked_add(1).ok_or("BOUND")?;
        if *n > 2048 {
            return Err("BOUND");
        }
        match v {
            serde_json::Value::String(s) if s.len() > 65536 => Err("BOUND"),
            serde_json::Value::Array(a) => {
                for x in a {
                    walk(x, d + 1, n)?
                }
                Ok(())
            }
            serde_json::Value::Object(m) => {
                for (k, x) in m {
                    if k.len() > 4096 {
                        return Err("BOUND");
                    };
                    walk(x, d + 1, n)?
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    walk(&v, 0, &mut 0)?;
    serde_json::from_value(v).map_err(|_| "SCHEMA_REJECTED")
}

// A real request is never accepted by the injected initializer.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeepSeekPlan {
    pub schema: String,
    pub package_id: String,
    pub declared_source_sha: String,
    pub snapshot: String,
    pub goal: String,
    pub proxy_policy: String,
    pub review_rule: String,
}
impl DeepSeekPlan {
    pub fn validate(&self) -> Result<()> {
        if self.schema != "nova.deepseek.plan/1" || !id(&self.package_id)
            || !hex(&self.declared_source_sha, 40) || self.snapshot.is_empty()
            || self.snapshot.len() > 8192 || self.snapshot.contains(['\0','\r'])
            || !self.snapshot.lines().any(|l|l==RELAY_SIGNATURE)
            || self.goal.trim().is_empty() || self.goal.len() > 4096
            || self.proxy_policy != "EXPLICIT_DIRECT" || self.review_rule != "CANDIDATE_DIFF_PROPOSAL_V1"
        { return Err("REAL_PLAN_REJECTED"); }
        Ok(())
    }
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        Ok(hash(&serde_json::to_vec(self).map_err(|_| "ENCODING")?))
    }
}
/// Created only by the local explicit approval operation, never from model output or a plan bool.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeepSeekApproval {
    plan_sha256: String,
    snapshot_sha256: String,
    approval_id: String,
    endpoint: String,
    model: String,
    max_requests: u8,
    no_money_limit: bool,
    wire_policy_sha256: String,
}
fn deepseek_policy_hash()->String {hash(b"DEEPSEEK_CHAT_JSON_V1|deepseek-flash|thinking=disabled|stream=false|max_tokens=4096|json_object|requests=2|request=65536|response=131072|stage=45000|total=120000|CANDIDATE_DIFF_PROPOSAL_V1|NO_MONEY_LIMIT")}
impl DeepSeekApproval {
    pub(crate) fn local_confirm(plan: &DeepSeekPlan, confirmed_digest: &str) -> Result<Self> {
        let digest = plan.digest()?;
        if digest != confirmed_digest { return Err("APPROVAL_REQUIRED"); }
        Ok(Self { plan_sha256: digest, snapshot_sha256: hash(plan.snapshot.as_bytes()),
            approval_id: uuid::Uuid::new_v4().to_string(),
            endpoint: "https://api.deepseek.com/chat/completions".into(), model: "deepseek-flash".into(),
            max_requests: 2, no_money_limit: true,wire_policy_sha256:deepseek_policy_hash() })
    }
    pub fn validate(&self, plan: &DeepSeekPlan) -> Result<()> {
        if self.plan_sha256 != plan.digest()? || self.snapshot_sha256 != hash(plan.snapshot.as_bytes())
            || uuid::Uuid::parse_str(&self.approval_id).is_err()
            || self.endpoint != "https://api.deepseek.com/chat/completions" || self.model != "deepseek-flash"
            || self.max_requests != 2 || !self.no_money_limit || self.wire_policy_sha256!=deepseek_policy_hash() { return Err("APPROVAL_REQUIRED"); }
        Ok(())
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeepSeekRecord {
    pub plan: DeepSeekPlan,
    pub approval: DeepSeekApproval,
    pub core: ApiPlan,
    pub develop_input_sha256: String,
}
impl DeepSeekRecord {
    pub fn validate(&self) -> Result<()> {
        self.approval.validate(&self.plan)?;
        let expected = self.plan.core(self.core.input_delivery_identity.clone())?;
        if self.develop_input_sha256 != hash(&super::relay::api_develop_input(&self.core,self.plan.snapshot.as_bytes())?) || relay_encoded(&expected)? != relay_encoded(&self.core)? { return Err("BINDING_MISMATCH"); }
        Ok(())
    }
}
impl DeepSeekPlan {
    pub fn core(&self, delivery: String) -> Result<ApiPlan> {
        self.validate()?;
        Ok(ApiPlan { schema:"nova.scheduler.api-plan/1".into(), mode:"REAL_API".into(),
            package_id:self.package_id.clone(), registration_revision:0,
            input_base_sha:self.declared_source_sha.clone(),input_delivery_identity:delivery,
            target_path:API_TARGET.into(),base_sha256:hash(self.snapshot.as_bytes()),goal:self.goal.clone(),
            constraints:vec!["Proposal only; never execute tools or write source".into()],examples:vec![],
            protocol:"DEEPSEEK_CHAT_JSON_V1".into(),model_develop:"deepseek-flash".into(),model_review:"deepseek-flash".into(),
            limits:ApiLimits{stage_ms:45000,total_ms:120000,input_tokens:8192,output_tokens:4096},
            pm_decision_ref:"LOCAL_EXPLICIT_APPROVAL".into(),plan_request_sha256:self.digest()? })
    }
}

impl Envelope {
    pub(crate) fn bind_human_snapshot(&mut self,m:&Manifest)->Result<()> {
        if self.base_source!="HUMAN_ATTESTED" || self.outside_scope_notice!="HUMAN_SNAPSHOT_NOT_GIT_SAMPLE"
            || !hex(&self.head,40) || self.base!=self.head || !self.parent.is_empty()
            || !self.tree.is_empty() || !self.ancestors.is_empty() {return Err("VERSION_UNKNOWN");}
        self.delivery_identity=hash(&serde_json::to_vec(&("HUMAN_SNAPSHOT_V1",&m.package_id,&self.root_identity,&self.head,&m.allowed_paths,&self.files)).map_err(|_|"ENCODING")?);
        self.envelope_id.clear();self.envelope_id=hash(&serde_json::to_vec(self).map_err(|_|"ENCODING")?);Ok(())
    }
}
