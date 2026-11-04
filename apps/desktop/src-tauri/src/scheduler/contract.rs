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
