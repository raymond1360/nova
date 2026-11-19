//! SELF-1 offline material routing. No CLI/model/test execution and no Task transitions.
use super::{
    contract::*,
    policy,
    store::{read_file, Guard, Store},
    Result,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBinding {
    pub uuid: String,
    pub source: String,
    pub reference: String,
}
impl SessionBinding {
    fn validate(&self) -> Result<()> {
        if uuid::Uuid::parse_str(&self.uuid)
            .map(|u| u.to_string() != self.uuid)
            .unwrap_or(true)
            || !["EXPLICIT_SELECTION", "SYNTHETIC_CLI_RECEIPT", "TOOL_CLI_RECEIPT"].contains(&self.source.as_str())
            || self.reference.trim().is_empty()
        {
            return Err("SELF_SESSION_REJECTED");
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Stage {
    pub stage: String,
    pub developer: Option<SessionBinding>,
    pub reviewer: Option<SessionBinding>,
}
impl Stage {
    fn explicit_input(&self)->Result<()> {
        if [&self.developer,&self.reviewer].iter().any(|s|s.as_ref().is_some_and(|b|b.source!="EXPLICIT_SELECTION")){return Err("SELF_SESSION_REJECTED");}self.validate()
    }
    fn validate(&self) -> Result<()> {
        if !id(&self.stage) {
            return Err("SELF_STAGE_REJECTED");
        }
        if let Some(d) = &self.developer {
            d.validate()?;
        }
        if let Some(r) = &self.reviewer {
            r.validate()?;
            if self.developer.as_ref().is_some_and(|d| r.uuid == d.uuid) {
                return Err("SELF_ROLE_COLLISION");
            }
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LogInput {
    pub command: Vec<String>,
    pub delivery_sha: String,
    pub stdout: PathBuf,
    pub stdout_sha256: String,
    pub stderr: PathBuf,
    pub stderr_sha256: String,
    pub declared_exit: Option<i32>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeliveryInput {
    pub stage: String,
    pub package: String,
    pub base_sha: String,
    pub head_sha: String,
    pub goal: String,
    pub acceptance: Vec<String>,
    pub allowed_paths: Vec<String>,
    pub required_commands: Vec<Vec<String>>,
    pub logs: Vec<LogInput>,
    pub developer_report: Report,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewInput {
    pub stage: String,
    pub package: String,
    pub head_sha: String,
    pub packet_sha256: String,
    pub route_id: String,
    pub reviewer_uuid: String,
    pub report: Report,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub schema: String,
    pub input: DeliveryInput,
    pub root_identity: String,
    pub files: BTreeMap<String, Value>,
    pub committed_diff: String,
    pub dirty_paths: Vec<String>,
    pub logs: Vec<Value>,
    pub missing: Vec<String>,
    pub claim_source: String,
    pub worktree_comparison: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Package {
    pub packet: Packet,
    pub packet_sha256: String,
    pub route_id: Option<String>,
    pub review: Option<ReviewInput>,
    pub uncertain: Option<String>,
    pub repairs: u32,
    pub repair_approval: Option<Report>,
    pub prior_reviews: Vec<Report>,
    pub prior_packets: Vec<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct State {
    pub schema: String,
    pub stage: Stage,
    pub active: Option<Package>,
    pub closed: Vec<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub self2: Option<super::self_codex::Journal>,
}
fn encode<T: Serialize>(v: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(v).map_err(|_| "ENCODING")
}
fn decode<T: serde::de::DeserializeOwned>(v: Value) -> Result<T> {
    serde_json::from_value(v).map_err(|_| "SELF_STATE_REJECTED")
}
pub(crate) fn load(s: &Store) -> Result<(i64, State)> {
    let (rev, v) = s.self_read()?;
    let st: State = decode(v.ok_or("SELF_UNINITIALIZED")?)?;
    st.stage.validate()?;
    if let Some(p) = &st.active {
        if hash(&encode(&p.packet)?) != p.packet_sha256 || p.packet.input.stage != st.stage.stage {
            return Err("SELF_PACKET_TAMPERED");
        }
    }
    if (st.self2.is_none() && st.schema != "nova.self-state/1") || (st.self2.is_some() && st.schema != "nova.self-state/2") {
        return Err("SELF_STATE_REJECTED");
    }
    Ok((rev, st))
}
pub(crate) fn verify_packet(s: &Store, p: &Package) -> Result<()> {
    if hash(&encode(&p.packet)?) != p.packet_sha256 {
        return Err("SELF_PACKET_TAMPERED");
    }
    let snap = s.snapshot()?;
    let (current, _) = material(Path::new(&snap.manifest.repo_path), &p.packet.input)?;
    if encode(&current)? != encode(&p.packet)? {
        return Err("SELF_MATERIAL_DRIFT");
    }
    Ok(())
}
pub(crate) fn save(s: &Store, rev: i64, st: &State) -> Result<()> {
    s.self_cas(rev, &serde_json::to_value(st).map_err(|_| "ENCODING")?)
}
fn adapter_settled(st:&State)->Result<()> {
 if st.self2.as_ref().is_some_and(|j|j.records.iter().any(|r|r.outcome!="COMPLETE")){return Err("SELF2_RECONCILE_REQUIRED");}Ok(())
}
fn same_stage(st: &State, stage: &str) -> Result<()> {
    if st.stage.stage != stage {
        Err("SELF_STAGE_MISMATCH")
    } else {
        Ok(())
    }
}
fn active<'a>(st: &'a mut State, package: &str) -> Result<&'a mut Package> {
    let p = st.active.as_mut().ok_or("SELF_NO_PACKAGE")?;
    if p.packet.input.package != package {
        return Err("SELF_PACKAGE_MISMATCH");
    }
    if p.uncertain.is_some() {
        return Err("SELF_RECONCILE_REQUIRED");
    }
    Ok(p)
}

// Only object/metadata reads: NEVER status, working-tree diff, checkout, hash-object,
// textconv or clean filters. Local Git config cannot turn these builtins into aliases.
// env_clear + no replacements/lazy fetch/external diff; bounded object material only.
struct Git {
    root: PathBuf,
    _pin: Guard,
}
impl Git {
    fn new(root: &Path) -> Result<Self> {
        Ok(Self {
            root: root.into(),
            _pin: Guard::open(root, true)?,
        })
    }
    fn call(&self, args: &[&str]) -> Result<Vec<u8>> {
        self._pin.verify()?;
        #[cfg(windows)]
        let exe = "C:/Program Files/Git/cmd/git.exe";
        #[cfg(not(windows))]
        let exe = "/usr/bin/git";
        let mut c = Command::new(exe);
        c.env_clear()
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env(
                "GIT_CONFIG_GLOBAL",
                if cfg!(windows) { "NUL" } else { "/dev/null" },
            )
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_TERMINAL_PROMPT", "0");
        if let Some(sr) = std::env::var_os("SystemRoot") {
            c.env("SystemRoot", sr);
        }
        c.args([
            "--no-pager",
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "core.attributesFile=NUL",
            "-c",
            "diff.external=",
            "-C",
        ])
        .arg(&self.root)
        .args(args)
        .stdin(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            c.creation_flags(0x08000000);
        }
        let out = c.output().map_err(|_| "SELF_GIT_ERROR")?;
        if !out.status.success() {
            return Err("SELF_GIT_ERROR");
        }
        if out.stdout.len() > 2 * 1024 * 1024 || out.stderr.len() > 16384 {
            return Err("INPUT_BOUND");
        }
        self._pin.verify()?;
        Ok(out.stdout)
    }
    fn text(&self, args: &[&str]) -> Result<String> {
        String::from_utf8(self.call(args)?).map_err(|_| "SELF_UTF8")
    }
}
fn lines0(bytes: &[u8]) -> Result<Vec<String>> {
    bytes
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8(b.to_vec()).map_err(|_| "SELF_UTF8"))
        .collect()
}
fn blob_id(b: &[u8]) -> String {
    let mut d = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    d.update(format!("blob {}\0", b.len()).as_bytes());
    d.update(b);
    d.finish()
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn material(root: &Path, input: &DeliveryInput) -> Result<(Packet, Envelope)> {
    material_for(root, input, true)
}
pub(crate) fn material_for(root: &Path, input: &DeliveryInput, delivered: bool) -> Result<(Packet, Envelope)> {
    if !id(&input.package)
        || !id(&input.stage)
        || !hex(&input.base_sha, 40)
        || !hex(&input.head_sha, 40)
        || input.goal.trim().is_empty()
        || input.acceptance.is_empty()
        || input.allowed_paths.is_empty()
        || input.allowed_paths.len() > 64
        || input.logs.len() > 32
        || input.required_commands.len() > 32
    {
        return Err("SELF_INPUT_REJECTED");
    }
    let allowed: BTreeSet<_> = input.allowed_paths.iter().collect();
    if allowed.len() != input.allowed_paths.len() || !allowed.iter().all(|p| relative(p)) {
        return Err("SCOPE_REJECTED");
    }
    input.developer_report.validate()?;
    if input.developer_report.report_type != ReportType::Development
        || (delivered && input.developer_report.claimed_conclusion != Conclusion::Pass)
        || input.developer_report.context.package_id != input.package
        || input.developer_report.claimed_delivery_sha != input.head_sha
    {
        return Err("SELF_DEVELOPMENT_NOT_SUCCESSFUL");
    }
    let g = Git::new(root)?;
    if g.text(&["rev-parse", "HEAD"])?.trim() != input.head_sha {
        return Err("SELF_SHA_MISMATCH");
    }
    g.call(&[
        "merge-base",
        "--is-ancestor",
        &input.base_sha,
        &input.head_sha,
    ])?;
    let changes = lines0(&g.call(&[
        "diff-tree",
        "--no-commit-id",
        "--name-only",
        "--no-renames",
        "-r",
        "-z",
        &input.base_sha,
        &input.head_sha,
    ])?)?;
    if changes.iter().any(|p| !allowed.contains(p)) {
        return Err("SCOPE_REJECTED");
    }
    let committed_diff = g.text(&[
        "diff-tree",
        "--no-commit-id",
        "--no-ext-diff",
        "--no-textconv",
        "--no-renames",
        "-p",
        &input.base_sha,
        &input.head_sha,
    ])?;
    let tree = g.call(&["ls-tree", "-r", "-z", &input.head_sha])?;
    let mut dirty = vec![];
    let mut tracked = BTreeSet::new();
    let mut entries = vec![];
    for line in lines0(&tree)? {
        let (meta, path) = line.split_once('\t').ok_or("SELF_GIT_TREE")?;
        let fields: Vec<_> = meta.split(' ').collect();
        if fields.len() != 3 || !relative(path) || !fields[0].starts_with("100") {
            return Err("SELF_UNSUPPORTED_TREE");
        }
        tracked.insert(path.to_string());
        entries.push(TreeEntry {
            path: path.into(),
            mode: fields[0].into(),
            object: fields[2].into(),
        });
        let file = root.join(path);
        super::store::safe_location(&file)?;
        let b = if file.exists() {
            read_file(&file, 16 * 1024 * 1024)?.0
        } else {
            Vec::new()
        };
        // Explicit CRLF checkout equivalence only for cleanliness; packet keeps raw bytes/hash.
        let equal = file.exists()
            && (blob_id(&b) == fields[2]
                || (b.contains(&b'\r')
                    && !b.contains(&0)
                    && std::str::from_utf8(&b).is_ok()
                    && blob_id(
                        &b.windows(2)
                            .enumerate()
                            .filter_map(|(i, w)| if w == b"\r\n" { None } else { Some(b[i]) })
                            .chain(b.last().copied())
                            .collect::<Vec<_>>(),
                    ) == fields[2]));
        if !equal {
            if !allowed.contains(&path.to_string()) {
                return Err("SCOPE_REJECTED");
            }
            dirty.push(path.to_string());
        }
    }
    for path in lines0(&g.call(&["ls-files", "--others", "--exclude-standard", "-z"])?)? {
        if !allowed.contains(&path) {
            return Err("SCOPE_REJECTED");
        }
        dirty.push(path);
    }
    let mut files = BTreeMap::new();
    let mut facts = vec![];
    for path in &input.allowed_paths {
        super::store::safe_location(&root.join(path))?;
        let (b, identity) = read_file(&root.join(path), 131072)?;
        let text = String::from_utf8(b.clone()).map_err(|_| "SELF_UTF8")?;
        let committed = if tracked.contains(path) {
            Some(g.text(&["show", &format!("{}:{path}", input.head_sha)])?)
        } else {
            None
        };
        facts.push(FileFact {
            path: path.clone(),
            bytes: b.len() as u64,
            sha256: hash(&b),
            identity: identity.clone(),
        });
        files.insert(path.clone(),json!({"source":"TOOL_FILE_READ","identity":identity,"sha256":hash(&b),"text":text,"committed_text":committed,"uncommitted_change":dirty.contains(path)}));
    }
    let mut logs = vec![];
    let mut missing = vec![];
    let mut seen = BTreeSet::new();
    for l in &input.logs {
        if l.delivery_sha != input.head_sha
            || l.command.is_empty()
            || !input.required_commands.contains(&l.command)
            || !seen.insert(l.command.clone())
        {
            return Err("SELF_LOG_SCOPE_REJECTED");
        }
        super::store::safe_location(&l.stdout)?;
        super::store::safe_location(&l.stderr)?;
        let out = read_file(&l.stdout, 262144)?.0;
        let err = read_file(&l.stderr, 262144)?.0;
        if hash(&out) != l.stdout_sha256 || hash(&err) != l.stderr_sha256 {
            return Err("SELF_LOG_TAMPERED");
        }
        logs.push(json!({"command":l.command,"source":"TOOL_READ_LOG_BYTES","stdout":String::from_utf8(out).map_err(|_|"SELF_UTF8")?,"stderr":String::from_utf8(err).map_err(|_|"SELF_UTF8")?,"declared_exit":l.declared_exit,"exit_source":"EXTERNAL_DECLARATION_NOT_REEXECUTED","delivery_sha":l.delivery_sha}));
    }
    for command in &input.required_commands {
        match input.logs.iter().find(|l| &l.command == command) {
            None => missing.push(format!("NOT RUN: {command:?}")),
            Some(l) if l.declared_exit != Some(0) => {
                missing.push(format!("RESULT_NOT_SUCCESSFUL_OR_UNKNOWN: {command:?}"))
            }
            _ => (),
        }
    }
    // Re-sample identity, HEAD and every exposed source/log before returning.
    if g.text(&["rev-parse", "HEAD"])?.trim() != input.head_sha {
        return Err("SELF_SHA_MISMATCH");
    }
    for fact in &facts {
        let (b, i) = read_file(&root.join(&fact.path), 131072)?;
        if i != fact.identity || hash(&b) != fact.sha256 {
            return Err("SELF_MATERIAL_DRIFT");
        }
    }
    for l in &input.logs {
        if hash(&read_file(&l.stdout, 262144)?.0) != l.stdout_sha256
            || hash(&read_file(&l.stderr, 262144)?.0) != l.stderr_sha256
        {
            return Err("SELF_LOG_TAMPERED");
        }
    }
    let mut env=Envelope{head:input.head_sha.clone(),parent:input.head_sha.clone(),base:input.base_sha.clone(),base_source:"HUMAN_ATTESTED".into(),root_identity:g._pin.identity.clone(),git_dir:g.text(&["rev-parse","--absolute-git-dir"])?.trim().into(),tree:entries,files:facts,ancestors:g.text(&["rev-list","--max-count=4096",&input.head_sha])?.lines().map(str::to_string).collect(),attributes:json!({"content_filters_executed":false}),outside_scope_notice:"Cumulative committed diff plus byte-inspected worktree; no intermediate-history scope claim".into(),observed_utc:chrono::Utc::now().to_rfc3339(),delivery_identity:String::new(),envelope_id:String::new()};
    let packet = Packet {
        schema: "nova.self-review-packet/1".into(),
        input: input.clone(),
        root_identity: g._pin.identity.clone(),
        files,
        committed_diff,
        dirty_paths: dirty,
        logs,
        missing,
        claim_source: "DEVELOPER_MODEL_OR_HUMAN_CLAIM_NOT_TOOL_VERIFICATION".into(),
        worktree_comparison:"TEXT_CHECKOUT_CRLF_TO_LF_EQUIVALENCE; CAPTURED_SOURCE_RETAINS_EXACT_RAW_UTF8_BYTES_AND_HASH".into(),
    };
    // Caller binds the same real Envelope through existing SH1 identity machinery.
    env.delivery_identity.clear();
    Ok((packet, env))
}
fn manifest(root: &Path, input: &DeliveryInput, reports: &Path) -> Manifest {
    Manifest {
        schema: "nova.sh1.package/1".into(),
        package_id: input.package.clone(),
        repo_path: root.to_string_lossy().into(),
        title: "Nova SELF-1 offline handoff".into(),
        goal: input.goal.clone(),
        allowed_paths: input.allowed_paths.clone(),
        acceptance_items: input.acceptance.clone(),
        pm_decision_ref: "EXPLICIT_SELF1_INPUT".into(),
        report_roots: vec![reports.to_string_lossy().into()],
        required_reports: vec![ReportType::Development, ReportType::Review, ReportType::Ci],
        baseline: Baseline {
            mode: "DECLARED".into(),
            base_sha: Some(input.base_sha.clone()),
        },
        eol_policy: "false".into(),
    }
}
/// One canonical owned store per physical project. Changing run/output directories
/// cannot reset package tombstones, review binding or the repair counter.
/// Moving a project/storage is unsupported; never silently migrate it.
pub fn store_path(repo: &Path) -> Result<PathBuf> {
    let g = Guard::open(repo, true)?;
    Ok(g.path
        .parent()
        .ok_or("PATH_REJECTED")?
        .join(format!("ns1-{}", &hash(g.identity.as_bytes())[..32])))
}
pub fn init(
    root: &Path,
    repo: &Path,
    reports: &Path,
    stage: Stage,
    input: DeliveryInput,
) -> Result<Store> {
    stage.explicit_input()?;
    if stage.stage != input.stage {
        return Err("SELF_STAGE_MISMATCH");
    }
    let (_, mut env) = material(repo, &input)?;
    let m = manifest(repo, &input, reports);
    env.bind(&m)?;
    let s = Store::self_init(root, &m, &env)?;
    let st = State {
        schema: "nova.self-state/1".into(),
        stage,
        active: None,
        closed: vec![],
        self2: None,
    };
    save(&s, 0, &st)?;
    collect(&s, input)?;
    Ok(s)
}
pub fn init_work(root: &Path, repo: &Path, reports: &Path, stage: Stage, input: DeliveryInput) -> Result<Store> {
    stage.explicit_input()?;
    if stage.stage != input.stage || input.developer_report.claimed_conclusion != Conclusion::NotRun || !input.logs.is_empty() {
        return Err("SELF_WORK_INPUT_REJECTED");
    }
    let (_, mut env) = material_for(repo, &input, false)?;
    let m = manifest(repo, &input, reports);
    env.bind(&m)?;
    let s = Store::self_init(root, &m, &env)?;
    save(&s, 0, &State { schema: "nova.self-state/1".into(), stage, active: None, closed: vec![], self2: None })?;
    Ok(s)
}
pub fn bind(s: &Store, stage: &str, reviewer: SessionBinding) -> Result<()> {
    if reviewer.source != "EXPLICIT_SELECTION" {return Err("SELF_SESSION_REJECTED");}
    let (rev, mut st) = load(s)?;
    adapter_settled(&st)?;
    same_stage(&st, stage)?;
    if st.stage.reviewer.is_some() {
        return Err("SELF_ALREADY_BOUND");
    }
    st.stage.reviewer = Some(reviewer);
    st.stage.validate()?;
    save(s, rev, &st)
}
pub fn bind_developer(s: &Store, stage: &str, developer: SessionBinding) -> Result<()> {
    if developer.source != "EXPLICIT_SELECTION" {return Err("SELF_SESSION_REJECTED");}
    let (rev, mut st) = load(s)?;
    adapter_settled(&st)?;
    same_stage(&st, stage)?;
    if st.stage.developer.is_some() {
        return Err("SELF_ALREADY_BOUND");
    }
    st.stage.developer = Some(developer);
    st.stage.validate()?;
    save(s, rev, &st)
}
pub fn collect(s: &Store, input: DeliveryInput) -> Result<()> {
    let (rev, mut st) = load(s)?;
    adapter_settled(&st)?;
    same_stage(&st, &input.stage)?;
    manifest(
        Path::new(&s.snapshot()?.manifest.repo_path),
        &input,
        Path::new(&s.snapshot()?.manifest.report_roots[0]),
    )
    .validate()?;
    let mut repairs = 0;
    let mut prior_reviews = vec![];
    let mut prior_packets = vec![];
    if st.closed.iter().any(|v| v["package"] == input.package) {
        return Err("SELF_DUPLICATE_PACKAGE");
    }
    if let Some(p) = &st.active {
        if p.packet.input.package != input.package {
            return Err("SELF_PACKAGE_IN_FLIGHT");
        }
        if p.uncertain.is_some() {
            return Err("SELF_RECONCILE_REQUIRED");
        }
        let approval = p
            .repair_approval
            .as_ref()
            .ok_or("SELF_REPAIR_APPROVAL_REQUIRED")?;
        if input.head_sha == p.packet.input.head_sha
            || input.base_sha != p.packet.input.base_sha
            || input.allowed_paths != p.packet.input.allowed_paths
            || input.goal != p.packet.input.goal
            || input.acceptance != p.packet.input.acceptance
            || input.required_commands != p.packet.input.required_commands
            || input.developer_report.context.stage != "REPAIR"
            || input.developer_report.context.supersedes_sha.as_deref()
                != Some(&p.packet.input.head_sha)
            || input.developer_report.context.pm_decision_ref != approval.context.pm_decision_ref
        {
            return Err("SELF_REPAIR_SCOPE_REJECTED");
        }
        repairs = p.repairs;
        prior_reviews = p.prior_reviews.clone();
        prior_reviews.push(
            p.review
                .as_ref()
                .ok_or("SELF_REVIEW_REQUIRED")?
                .report
                .clone(),
        );
        prior_packets = p.prior_packets.clone();
        prior_packets.push(p.packet_sha256.clone());
    }
    let snapshot = s.snapshot()?;
    let (packet, mut env) = material(Path::new(&snapshot.manifest.repo_path), &input)?;
    if env.root_identity != s.audit_envelope()?.root_identity {
        return Err("ROOT_MISMATCH");
    }
    env.bind(&manifest(
        Path::new(&snapshot.manifest.repo_path),
        &input,
        Path::new(&snapshot.manifest.report_roots[0]),
    ))?;
    env.validate_report(
        &manifest(
            Path::new(&snapshot.manifest.repo_path),
            &input,
            Path::new(&snapshot.manifest.report_roots[0]),
        ),
        &input.developer_report,
    )?;
    let digest = hash(&encode(&packet)?);
    st.active = Some(Package {
        packet,
        packet_sha256: digest,
        route_id: None,
        review: None,
        uncertain: None,
        repairs,
        repair_approval: None,
        prior_reviews,
        prior_packets,
    });
    save(s, rev, &st)
}
pub fn route(s: &Store, stage: &str, package: &str) -> Result<Value> {
    let (rev, mut st) = load(s)?;
    adapter_settled(&st)?;
    same_stage(&st, stage)?;
    if st.stage.developer.is_none() {
        return Err("SELF_DEVELOPER_UNBOUND");
    }
    let reviewer = st
        .stage
        .reviewer
        .as_ref()
        .ok_or("SELF_REVIEWER_UNBOUND")?
        .uuid
        .clone();
    let p = active(&mut st, package)?;
    verify_packet(s, p)?;
    if p.route_id.is_some() || p.review.is_some() {
        return Err("SELF_ROUTE_ALREADY_RECORDED");
    }
    if !p.packet.missing.is_empty() {
        return Err("SELF_REQUIRED_FACTS_MISSING");
    }
    p.route_id = Some(uuid::Uuid::new_v4().to_string());
    let out = json!({"schema":"nova.self-route/1","stage":stage,"package":package,"head_sha":p.packet.input.head_sha,"packet_sha256":p.packet_sha256,"reviewer_uuid":reviewer,"route_id":p.route_id,"external_action_executed":false,"instruction":"Read complete review packet, actual source/diff and command evidence; return version-bound review. No writes or execution authorized."});
    save(s, rev, &st)?;
    Ok(out)
}
pub fn review(s: &Store, input: ReviewInput) -> Result<()> {
    let (rev, mut st) = load(s)?;
    adapter_settled(&st)?;
    same_stage(&st, &input.stage)?;
    if st.stage.reviewer.as_ref().map(|r| r.uuid.as_str()) != Some(input.reviewer_uuid.as_str()) {
        return Err("SELF_REVIEWER_MISMATCH");
    }
    let p = active(&mut st, &input.package)?;
    verify_packet(s, p)?;
    if p.review.is_some()
        || p.route_id.as_deref() != Some(input.route_id.as_str())
        || input.head_sha != p.packet.input.head_sha
        || input.packet_sha256 != p.packet_sha256
    {
        return Err("SELF_REVIEW_BINDING_REJECTED");
    }
    input.report.validate()?;
    if input.report.report_type != ReportType::Review
        || input.report.context.package_id != input.package
        || input.report.claimed_delivery_sha != input.head_sha
    {
        return Err("SELF_REVIEW_BINDING_REJECTED");
    }
    if let Some(old) = p.prior_reviews.last() {
        policy::validate_history(old, &input.report)?;
    }
    let snap = s.snapshot()?;
    let (_, env) = material(Path::new(&snap.manifest.repo_path), &p.packet.input)?;
    env.validate_report(
        &manifest(
            Path::new(&snap.manifest.repo_path),
            &p.packet.input,
            Path::new(&snap.manifest.report_roots[0]),
        ),
        &input.report,
    )?;
    p.review = Some(input);
    save(s, rev, &st)
}
pub fn approve_repair(s: &Store, stage: &str, package: &str, approval: Report) -> Result<()> {
    let (rev, mut st) = load(s)?;
    adapter_settled(&st)?;
    same_stage(&st, stage)?;
    let p = active(&mut st, package)?;
    approval.validate()?;
    if p.repairs != 0 || p.repair_approval.is_some() {
        return Err("SELF_REPAIR_LIMIT");
    }
    let review = &p.review.as_ref().ok_or("SELF_REVIEW_REQUIRED")?.report;
    if review.claimed_conclusion == Conclusion::Pass
        && !review
            .issues
            .iter()
            .any(|i| i.blocking && i.status == IssueStatus::Open)
    {
        return Err("SELF_REPAIR_NOT_REQUIRED");
    }
    if approval.report_type != ReportType::PmDecision
        || approval.context.decision_kind.as_deref() != Some("REPAIR_REFERENCE")
        || approval.context.package_id != package
        || approval.claimed_delivery_sha != p.packet.input.head_sha
    {
        return Err("SELF_PM_APPROVAL_REQUIRED");
    }
    p.repairs = 1;
    p.repair_approval = Some(approval);
    save(s, rev, &st)
}
pub fn uncertain(s: &Store, stage: &str, package: &str, state: &str) -> Result<()> {
    let (rev, mut st) = load(s)?;
    same_stage(&st, stage)?;
    if !["STARTED", "UNKNOWN"].contains(&state) {
        return Err("SELF_STATE_REJECTED");
    }
    let p = active(&mut st, package)?;
    p.uncertain = Some(state.into());
    save(s, rev, &st)
}
pub fn close(s: &Store, stage: &str, package: &str, approval: Report) -> Result<()> {
    let (rev, mut st) = load(s)?;
    adapter_settled(&st)?;
    same_stage(&st, stage)?;
    let p = active(&mut st, package)?;
    approval.validate()?;
    if approval.report_type != ReportType::PmDecision
        || approval.context.decision_kind.as_deref() != Some("CLOSURE_REFERENCE")
        || approval.context.package_id != package
        || approval.claimed_delivery_sha != p.packet.input.head_sha
    {
        return Err("SELF_PM_APPROVAL_REQUIRED");
    }
    let summary = summary_package(p);
    let package_record = p.clone();
    st.closed.push(json!({"package":package,"package_record":package_record,"summary":summary,"approval":approval,"semantics":"PM_CLOSURE_REFERENCE_NOT_TASK_COMPLETION_OR_USER_ACCEPTANCE"}));
    if let Some(journal)=st.self2.take(){st.closed.last_mut().ok_or("SELF_NO_PACKAGE")?["self2_journal"]=serde_json::to_value(journal).map_err(|_|"ENCODING")?;st.schema="nova.self-state/1".into();}
    st.active = None;
    save(s, rev, &st)
}
fn summary_package(p: &Package) -> Value {
    let result = if p.uncertain.is_some() {
        "STOP_RECONCILE"
    } else if p.review.is_some() {
        "AWAITING_PM_AND_USER"
    } else if !p.packet.missing.is_empty() {
        "MISSING_FACTS"
    } else if p.route_id.is_some() {
        "REVIEW_RESULT_PENDING"
    } else {
        "READY_FOR_OFFLINE_REVIEW_ROUTING"
    };
    json!({"package":p.packet.input.package,"head_sha":p.packet.input.head_sha,"packet_sha256":p.packet_sha256,"status":result,"missing":p.packet.missing,"review_claim":p.review,"claim_source":"MODEL_OR_HUMAN_DECLARATION_NOT_MACHINE_VERIFICATION","manual_repairs":p.repairs,"automatic_repairs":0,"test_exit_semantics":"IMPORTED_DECLARATION; FILE_BYTES_TOOL_VERIFIED; NO_TESTS_EXECUTED_BY_SELF1","ci":"NOT RUN BY SELF1","user_acceptance":"PENDING","external_action_executed":false})
}
pub fn summary(s: &Store) -> Result<Value> {
    let (rev, st) = load(s)?;
    Ok(
        json!({"schema":"nova.self-summary/1","revision":rev,"binding_status":if st.stage.developer.is_none() || st.stage.reviewer.is_none(){"UNBOUND"}else{"EXPLICITLY_SELECTED_NOT_RUNTIME_VERIFIED"},"stage":st.stage,"active":st.active.as_ref().map(summary_package),"closed":st.closed,"execution_authorized":false,"real_codex_started":false}),
    )
}
pub fn export(s: &Store, output: &Path) -> Result<()> {
    let (_, st) = load(s)?;
    let p = st.active.as_ref().ok_or("SELF_NO_PACKAGE")?;
    let packet = encode(&p.packet)?;
    if hash(&packet) != p.packet_sha256 {
        return Err("SELF_PACKET_TAMPERED");
    }
    // Replay collection without storing: catches changed HEAD, source and imported log files.
    let snap = s.snapshot()?;
    let (current, _) = material(Path::new(&snap.manifest.repo_path), &p.packet.input)?;
    if encode(&current)? != packet {
        return Err("SELF_MATERIAL_DRIFT");
    }
    let parent = Guard::open(output.parent().ok_or("PATH_REJECTED")?, true)?;
    if super::store::inside(output, Path::new(&snap.manifest.repo_path)) {
        return Err("SELF_OUTPUT_IN_REPO");
    }
    fs::create_dir(output).map_err(|_| "CREATE_NEW_REJECTED")?;
    let data = serde_json::to_string_pretty(&p.packet).map_err(|_| "ENCODING")?;
    let readme=format!("# Nova offline review packet\n\nReview session: {:?} (unbound means no routing).\nStage: {}\nPacket SHA256 (compact JSON): {}\n\nRead full review-packet.json below. Test exits are imported declarations; missing checks are NOT RUN. No Codex was started.\n\n```json\n{}\n```\n",st.stage.reviewer,st.stage.stage,p.packet_sha256,data);
    for (name, b) in [
        ("review-packet.json", packet),
        ("REVIEW.md", readme.into_bytes()),
        ("summary.json", encode(&summary(s)?)?),
    ] {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(output.join(name))
            .map_err(|_| "EVIDENCE_CREATE_NEW")?;
        f.write_all(&b)
            .and_then(|_| f.sync_all())
            .map_err(|_| "EVIDENCE_WRITE")?;
    }
    parent.verify()?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) struct F {
        pub(crate) dir: PathBuf,
        pub(crate) repo: PathBuf,
        pub(crate) logs: PathBuf,
        pub(crate) input: DeliveryInput,
        pub(crate) stage: Stage,
    }
    impl F {
        fn git(&self, a: &[&str]) -> String {
            let o = Command::new(if cfg!(windows) {
                "C:/Program Files/Git/cmd/git.exe"
            } else {
                "git"
            })
            .arg("-C")
            .arg(&self.repo)
            .args(a)
            .output()
            .unwrap();
            assert!(o.status.success(), "{:?}", o.stderr);
            String::from_utf8(o.stdout).unwrap().trim().into()
        }
        pub(crate) fn new() -> Self {
            let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/self1-tests");
            fs::create_dir_all(&base).unwrap();
            let dir = base.join(uuid::Uuid::new_v4().to_string());
            fs::create_dir(&dir).unwrap();
            let repo = dir.join("repo");
            let logs = dir.join("logs");
            fs::create_dir(&repo).unwrap();
            fs::create_dir(&logs).unwrap();
            let mut f=Self{dir,repo,logs,input:serde_json::from_value(json!({"stage":"stage-one","package":"pkg-one","base_sha":"a".repeat(40),"head_sha":"a".repeat(40),"goal":"Actual offline fixture","acceptance":["one change"],"allowed_paths":["a.txt"],"required_commands":[["fixed-test"]],"logs":[],"developer_report":report("pkg-one",&"a".repeat(40),ReportType::Development,Conclusion::Pass)})).unwrap(),stage:Stage{stage:"stage-one".into(),developer:Some(SessionBinding{uuid:uuid::Uuid::new_v4().to_string(),source:"EXPLICIT_SELECTION".into(),reference:"SYNTHETIC OFFLINE fixture developer".into()}),reviewer:Some(SessionBinding{uuid:uuid::Uuid::new_v4().to_string(),source:"EXPLICIT_SELECTION".into(),reference:"SYNTHETIC OFFLINE fixture reviewer".into()})}};
            f.git(&["init", "-q"]);
            fs::write(f.repo.join("a.txt"), "before\n").unwrap();
            f.commit();
            f.input.base_sha = f.git(&["rev-parse", "HEAD"]);
            fs::write(f.repo.join("a.txt"), "after\n").unwrap();
            f.commit();
            f.input.head_sha = f.git(&["rev-parse", "HEAD"]);
            f.input.developer_report.claimed_delivery_sha = f.input.head_sha.clone();
            fs::write(
                f.logs.join("out"),
                "SYNTHETIC imported test receipt, not executed\n",
            )
            .unwrap();
            fs::write(f.logs.join("err"), "").unwrap();
            f.input.logs.push(LogInput {
                command: vec!["fixed-test".into()],
                delivery_sha: f.input.head_sha.clone(),
                stdout: f.logs.join("out"),
                stdout_sha256: hash(&fs::read(f.logs.join("out")).unwrap()),
                stderr: f.logs.join("err"),
                stderr_sha256: hash(b""),
                declared_exit: Some(0),
            });
            f
        }
        fn commit(&self) {
            self.git(&["add", "a.txt"]);
            self.git(&[
                "-c",
                "user.name=SELF fixture",
                "-c",
                "user.email=fixture@invalid",
                "-c",
                "core.hooksPath=NUL",
                "commit",
                "-qm",
                "fixture",
            ]);
        }
        pub(crate) fn init(&self) -> Store {
            init(
                &store_path(&self.repo).unwrap(),
                &self.repo,
                &self.logs,
                self.stage.clone(),
                self.input.clone(),
            )
            .unwrap()
        }
        fn review(&self, s: &Store, c: Conclusion) -> ReviewInput {
            let rt = route(s, "stage-one", "pkg-one").unwrap();
            ReviewInput {
                stage: "stage-one".into(),
                package: "pkg-one".into(),
                head_sha: self.input.head_sha.clone(),
                packet_sha256: rt["packet_sha256"].as_str().unwrap().into(),
                route_id: rt["route_id"].as_str().unwrap().into(),
                reviewer_uuid: self.stage.reviewer.as_ref().unwrap().uuid.clone(),
                report: report("pkg-one", &self.input.head_sha, ReportType::Review, c),
            }
        }
    }
    pub(crate) fn report(package: &str, sha: &str, kind: ReportType, c: Conclusion) -> Report {
        Report {
            schema: "nova.scheduler.report/1".into(),
            report_type: kind,
            claimed_delivery_sha: sha.into(),
            claimed_conclusion: c,
            issues: vec![],
            context: Context {
                package_id: package.into(),
                stage: "INITIAL".into(),
                supersedes_sha: None,
                pm_decision_ref: None,
                decision_kind: None,
                slot_resolution: None,
            },
            claims: Claims {
                author: Some("SYNTHETIC offline claim".into()),
                role: kind.role().into(),
                fresh_session: None,
                model: None,
                ci_runs: vec![],
            },
            source_report: None,
            transcribed_by: None,
        }
    }
    fn pm(f: &F, kind: &str) -> Report {
        let mut r = report(
            "pkg-one",
            &f.input.head_sha,
            ReportType::PmDecision,
            Conclusion::Recorded,
        );
        r.context.pm_decision_ref = Some("EXPLICIT SYNTHETIC PM".into());
        r.context.decision_kind = Some(kind.into());
        r
    }
    #[test]
    fn self1_real_bytes_packet_route_claim_and_summary() {
        let f = F::new();
        let s = f.init();
        let r = f.review(&s, Conclusion::Pass);
        review(&s, r).unwrap();
        export(&s, &f.dir.join("export")).unwrap();
        let st = summary(&s).unwrap();
        assert_eq!(st["active"]["status"], "AWAITING_PM_AND_USER");
        assert_eq!(st["real_codex_started"], false);
        assert_eq!(
            load(&s).unwrap().1.active.unwrap().packet.files["a.txt"]["text"],
            "after\n"
        );
    }
    #[test]
    fn self1_unbound_roles_and_fixed_stage_identity() {
        let mut unbound = F::new();
        unbound.stage.developer = None;
        unbound.stage.reviewer = None;
        let u = unbound.init();
        assert_eq!(
            route(&u, "stage-one", "pkg-one").unwrap_err(),
            "SELF_DEVELOPER_UNBOUND"
        );
        let mut f = F::new();
        f.stage.reviewer = None;
        let s = f.init();
        assert_eq!(
            route(&s, "stage-one", "pkg-one").unwrap_err(),
            "SELF_REVIEWER_UNBOUND"
        );
        assert_eq!(
            bind(&s, "other", f.stage.developer.clone().unwrap()).unwrap_err(),
            "SELF_STAGE_MISMATCH"
        );
        assert_eq!(
            bind(&s, "stage-one", f.stage.developer.clone().unwrap()).unwrap_err(),
            "SELF_ROLE_COLLISION"
        );
        let r = SessionBinding {
            uuid: uuid::Uuid::new_v4().to_string(),
            source: "EXPLICIT_SELECTION".into(),
            reference: "SYNTHETIC".into(),
        };
        bind(&s, "stage-one", r.clone()).unwrap();
        assert_eq!(bind(&s, "stage-one", r).unwrap_err(), "SELF_ALREADY_BOUND");
    }
    #[test]
    fn self1_out_of_order_duplicate_wrong_uuid_sha_and_packet() {
        let f = F::new();
        let s = f.init();
        let mut r = f.review(&s, Conclusion::Pass);
        let good = r.clone();
        r.reviewer_uuid = f.stage.developer.as_ref().unwrap().uuid.clone();
        assert_eq!(review(&s, r).unwrap_err(), "SELF_REVIEWER_MISMATCH");
        for key in [0, 1, 2, 3] {
            let mut r = good.clone();
            match key {
                0 => r.head_sha = "b".repeat(40),
                1 => r.packet_sha256 = "b".repeat(64),
                2 => r.route_id = "wrong".into(),
                _ => r.stage = "wrong".into(),
            };
            assert!(review(&s, r).is_err());
        }
        review(&s, good.clone()).unwrap();
        assert!(review(&s, good).is_err());
        assert!(route(&s, "stage-one", "pkg-one").is_err());
    }
    #[test]
    fn self1_scope_failed_development_and_missing_checks() {
        let mut f = F::new();
        let mut i = f.input.clone();
        i.head_sha = "b".repeat(40);
        assert!(material(&f.repo, &i).is_err());
        i = f.input.clone();
        i.developer_report.claimed_conclusion = Conclusion::Fail;
        assert_eq!(
            material(&f.repo, &i).unwrap_err(),
            "SELF_DEVELOPMENT_NOT_SUCCESSFUL"
        );
        fs::write(f.repo.join("outside"), "out").unwrap();
        assert_eq!(material(&f.repo, &f.input).unwrap_err(), "SCOPE_REJECTED");
        fs::remove_file(f.repo.join("outside")).unwrap();
        f.input.logs.clear();
        let s = f.init();
        assert!(summary(&s).unwrap()["active"]["missing"][0]
            .as_str()
            .unwrap()
            .starts_with("NOT RUN"));
        assert_eq!(
            route(&s, "stage-one", "pkg-one").unwrap_err(),
            "SELF_REQUIRED_FACTS_MISSING"
        );
    }
    #[test]
    fn self1_log_source_and_export_drift_rejected() {
        let f = F::new();
        let s = f.init();
        fs::write(f.logs.join("out"), "tampered").unwrap();
        assert_eq!(
            route(&s, "stage-one", "pkg-one").unwrap_err(),
            "SELF_LOG_TAMPERED"
        );
        assert!(export(&s, &f.dir.join("bad-export")).is_err());
        assert!(!f.dir.join("bad-export").exists());
        let f = F::new();
        let s = f.init();
        fs::write(f.repo.join("a.txt"), "drift\n").unwrap();
        assert_eq!(
            route(&s, "stage-one", "pkg-one").unwrap_err(),
            "SELF_MATERIAL_DRIFT"
        );
    }
    #[test]
    fn self1_started_unknown_reopen_never_replays() {
        for state in ["STARTED", "UNKNOWN"] {
            let f = F::new();
            let s = f.init();
            uncertain(&s, "stage-one", "pkg-one", state).unwrap();
            drop(s);
            let s = Store::self_open(&store_path(&f.repo).unwrap()).unwrap();
            assert_eq!(
                route(&s, "stage-one", "pkg-one").unwrap_err(),
                "SELF_RECONCILE_REQUIRED"
            );
            assert_eq!(
                collect(&s, f.input.clone()).unwrap_err(),
                "SELF_RECONCILE_REQUIRED"
            );
            assert_eq!(summary(&s).unwrap()["active"]["status"], "STOP_RECONCILE");
        }
    }
    #[test]
    fn self1_single_package_and_cas_two_connections() {
        let alt = F::new();
        assert!(init(
            &alt.dir.join("alternate"),
            &alt.repo,
            &alt.logs,
            alt.stage.clone(),
            alt.input.clone()
        )
        .is_err());
        assert!(!alt.dir.join("alternate").exists());
        let f = F::new();
        let s = f.init();
        let s2 = Store::self_open(&store_path(&f.repo).unwrap()).unwrap();
        let (rev, st) = load(&s).unwrap();
        uncertain(&s, "stage-one", "pkg-one", "STARTED").unwrap();
        assert_eq!(save(&s2, rev, &st).unwrap_err(), "CAS_CONFLICT");
        let f = F::new();
        let s = f.init();
        let mut i = f.input.clone();
        i.package = "other".into();
        i.developer_report.context.package_id = i.package.clone();
        assert_eq!(collect(&s, i).unwrap_err(), "SELF_PACKAGE_IN_FLIGHT");
    }
    #[test]
    fn self1_one_explicit_repair_same_reviewer_after_reopen() {
        let mut f = F::new();
        let s = f.init();
        assert!(approve_repair(&s, "stage-one", "pkg-one", pm(&f, "REPAIR_REFERENCE")).is_err());
        let r = f.review(&s, Conclusion::Fail);
        review(&s, r).unwrap();
        approve_repair(&s, "stage-one", "pkg-one", pm(&f, "REPAIR_REFERENCE")).unwrap();
        assert_eq!(
            approve_repair(&s, "stage-one", "pkg-one", pm(&f, "REPAIR_REFERENCE")).unwrap_err(),
            "SELF_REPAIR_LIMIT"
        );
        drop(s);
        let s = Store::self_open(&store_path(&f.repo).unwrap()).unwrap();
        let old = f.input.head_sha.clone();
        fs::write(f.repo.join("a.txt"), "repaired\n").unwrap();
        f.commit();
        f.input.head_sha = f.git(&["rev-parse", "HEAD"]);
        f.input.logs[0].delivery_sha = f.input.head_sha.clone();
        f.input.developer_report.claimed_delivery_sha = f.input.head_sha.clone();
        f.input.developer_report.context.stage = "REPAIR".into();
        f.input.developer_report.context.supersedes_sha = Some(old);
        f.input.developer_report.context.pm_decision_ref = Some("EXPLICIT SYNTHETIC PM".into());
        collect(&s, f.input.clone()).unwrap();
        let r = f.review(&s, Conclusion::Pass);
        review(&s, r).unwrap();
        assert_eq!(summary(&s).unwrap()["active"]["manual_repairs"], 1);
        assert_eq!(
            load(&s).unwrap().1.stage.reviewer.unwrap().uuid,
            f.stage.reviewer.unwrap().uuid
        );
    }
    #[test]
    fn self1_fixed_reviewer_next_package_and_old_store_no_migration() {
        let mut f = F::new();
        let s = f.init();
        let r = f.review(&s, Conclusion::Pass);
        review(&s, r).unwrap();
        close(&s, "stage-one", "pkg-one", pm(&f, "CLOSURE_REFERENCE")).unwrap();
        assert_eq!(
            collect(&s, f.input.clone()).unwrap_err(),
            "SELF_DUPLICATE_PACKAGE"
        );
        f.input.package = "pkg-two".into();
        f.input.developer_report.context.package_id = "pkg-two".into();
        collect(&s, f.input.clone()).unwrap();
        let r = route(&s, "stage-one", "pkg-two").unwrap();
        assert_eq!(r["reviewer_uuid"], f.stage.reviewer.unwrap().uuid);
        assert!(Store::open(&store_path(&f.repo).unwrap()).is_err());
        let (packet, mut env) = material(&f.repo, &f.input).unwrap();
        let m = manifest(&f.repo, &packet.input, &f.logs);
        env.bind(&m).unwrap();
        let old = f.dir.join("old");
        drop(Store::init(&old, &m, &env).unwrap());
        assert!(Store::self_open(&old).is_err());
        assert!(Store::open(&old).is_ok());
    }
    #[test]
    fn self1_git_filters_not_invoked_by_object_collection() {
        let f = F::new();
        let mark = f.dir.join("FORBIDDEN");
        f.git(&[
            "config",
            "core.fsmonitor",
            &format!("echo bad > {}", mark.display()),
        ]);
        f.git(&[
            "config",
            "filter.side.clean",
            &format!("echo bad > {}", mark.display()),
        ]);
        f.git(&[
            "config",
            "filter.side.process",
            &format!("echo bad > {}", mark.display()),
        ]);
        fs::write(f.repo.join(".git/info/attributes"), "*.txt filter=side\n").unwrap();
        fs::write(f.repo.join("a.txt"), "dirty!\n").unwrap();
        let (p, _) = material(&f.repo, &f.input).unwrap();
        assert_eq!(p.dirty_paths, vec!["a.txt"]);
        assert!(!mark.exists());
    }
}
