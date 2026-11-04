use super::{contract::*, policy::*, store::*, Result};
use crate::models::TaskState;
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn sha(c: char) -> String {
    std::iter::repeat_n(c, 40).collect()
}
fn manifest(repo: &Path, reports: &Path) -> Manifest {
    Manifest {
        schema: "nova.sh1.package/1".into(),
        package_id: "fixture-package".into(),
        repo_path: repo.to_string_lossy().into(),
        title: "Fixture handoff".into(),
        goal: "One bounded edit".into(),
        allowed_paths: vec!["Logo.tsx".into()],
        acceptance_items: vec!["bounded result".into()],
        pm_decision_ref: "fixture-pm-ref".into(),
        report_roots: vec![reports.to_string_lossy().into()],
        required_reports: vec![ReportType::Development, ReportType::Review, ReportType::Ci],
        baseline: Baseline {
            mode: "DECLARED".into(),
            base_sha: Some(sha('a')),
        },
        eol_policy: "false".into(),
    }
}
fn report(kind: ReportType) -> Report {
    Report {
        schema: "nova.scheduler.report/1".into(),
        report_type: kind,
        claimed_delivery_sha: sha('b'),
        claimed_conclusion: Conclusion::Pass,
        issues: vec![],
        context: Context {
            package_id: "fixture-package".into(),
            stage: "INITIAL".into(),
            supersedes_sha: None,
            pm_decision_ref: None,
            decision_kind: None,
            slot_resolution: None,
        },
        claims: Claims {
            author: Some("Fixture author claim".into()),
            role: kind.role().into(),
            fresh_session: None,
            model: None,
            ci_runs: vec![],
        },
        source_report: None,
        transcribed_by: None,
    }
}
fn bytes(r: &Report) -> Vec<u8> {
    serde_json::to_vec(r).unwrap()
}
fn candidate(kind: ReportType) -> Candidate {
    Candidate {
        receipt_id: format!("r-{}", kind.role()),
        report: report(kind),
    }
}
fn issue(blocking: bool) -> Issue {
    Issue {
        id: "N-1".into(),
        severity: Severity::Low,
        blocking,
        status: IssueStatus::Open,
        first_seen_sha: sha('a'),
        closed_sha: None,
    }
}
fn all() -> Vec<Candidate> {
    vec![
        candidate(ReportType::Development),
        candidate(ReportType::Review),
        candidate(ReportType::Ci),
    ]
}
fn project_all(c: &[Candidate], r: &[Candidate], invalid: bool) -> Projection {
    project(
        &[ReportType::Development, ReportType::Review, ReportType::Ci],
        c,
        r,
        invalid,
    )
}

#[test]
fn strict_duplicate_keys_at_all_depths() {
    let r = String::from_utf8(bytes(&report(ReportType::Review))).unwrap();
    let duplicate = r.replace(
        "\"role\":\"review\"",
        "\"role\":\"review\",\"role\":\"review\"",
    );
    assert!(Report::parse(duplicate.as_bytes()).is_err());
    assert!(json::<Value>(br#"{"a":{"x":1,"x":2}}"#).is_err());
}
#[test]
fn strict_unknown_fields_and_source_spoof_rejected() {
    for key in ["verified", "source", "authorization"] {
        let mut v = serde_json::to_value(report(ReportType::Review)).unwrap();
        v[key] = json!("TOOL_FILE_HASH");
        assert!(Report::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    }
}
#[test]
fn required_nullable_fields_cannot_be_omitted() {
    let mut v = serde_json::to_value(report(ReportType::Review)).unwrap();
    for field in ["source_report", "transcribed_by"] {
        let mut x = v.clone();
        x.as_object_mut().unwrap().remove(field);
        assert!(Report::parse(&serde_json::to_vec(&x).unwrap()).is_err());
    }
    v["claims"].as_object_mut().unwrap().remove("fresh_session");
    assert!(Report::parse(&serde_json::to_vec(&v).unwrap()).is_err());
}
#[test]
fn report_fence_is_exactly_one_and_legacy_is_index_only() {
    assert_eq!(
        Report::parse(b"legacy PASS").unwrap_err(),
        "FORMAT_REQUIRED"
    );
    let b = String::from_utf8(bytes(&report(ReportType::Review))).unwrap();
    let block = format!("# report\n```nova-handoff-v1\n{b}\n```\n");
    assert!(Report::parse(block.as_bytes()).is_ok());
    assert!(Report::parse(format!("{block}{block}").as_bytes()).is_err());
}
#[test]
fn schema_missing_blocking_is_not_false() {
    let mut r = report(ReportType::Review);
    r.issues.push(issue(false));
    let mut v = serde_json::to_value(r).unwrap();
    v["issues"][0].as_object_mut().unwrap().remove("blocking");
    assert!(Report::parse(&serde_json::to_vec(&v).unwrap()).is_err());
}
#[test]
fn low_blocking_and_high_nonblocking_are_distinct() {
    let mut r = report(ReportType::Review);
    r.issues.push(issue(true));
    assert!(r.validate().is_ok());
    r.issues[0].severity = Severity::High;
    r.issues[0].blocking = false;
    assert_eq!(r.validate(), Err("SEVERITY_BLOCKING_CONTRADICTION"));
}
#[test]
fn manifest_requires_explicit_eol_and_unique_exact_scope() {
    let m = manifest(Path::new("E:/repo"), Path::new("E:/reports"));
    for p in ["auto", "TRUE", ""] {
        let mut x = m.clone();
        x.eol_policy = p.into();
        assert!(x.validate().is_err())
    }
    let mut x = m.clone();
    x.allowed_paths.push("logo.tsx".into());
    assert!(x.validate().is_err());
    x.allowed_paths = vec!["../x".into()];
    assert!(x.validate().is_err());
}
#[test]
fn all_six_slot_orders_have_the_same_projection() {
    let c = all();
    for order in [
        [0, 1, 2],
        [0, 2, 1],
        [1, 0, 2],
        [1, 2, 0],
        [2, 0, 1],
        [2, 1, 0],
    ] {
        let ordered = order.map(|i| c[i].clone());
        let p = project_all(&ordered, &[], false);
        assert!(p.complete);
        assert_eq!(p.selected.len(), 3);
    }
}
#[test]
fn early_review_and_ci_do_not_invent_development() {
    let c = all();
    let p = project_all(&c[1..], &[], false);
    assert!(!p.has_development);
    assert!(!p.complete);
    assert!(p.missing.contains(&ReportType::Development));
}
#[test]
fn nonblocking_open_pass_stays_open_in_projection() {
    let mut c = all();
    c[1].report.issues.push(issue(false));
    let p = project_all(&c, &[], false);
    assert!(p.complete);
    assert_eq!(p.nonblocking_open, vec!["N-1"]);
    assert_eq!(c[1].report.issues[0].status, IssueStatus::Open);
}
#[test]
fn low_blocking_open_or_claimed_failure_prevents_readiness() {
    let mut c = all();
    c[1].report.issues.push(issue(true));
    assert!(!project_all(&c, &[], false).complete);
    c[1].report.issues.clear();
    c[2].report.claimed_conclusion = Conclusion::Fail;
    assert!(!project_all(&c, &[], false).complete);
}
#[test]
fn same_slot_never_uses_last_report() {
    let mut c = all();
    let mut other = c[1].clone();
    other.receipt_id = "r-review-2".into();
    other.report.claimed_conclusion = Conclusion::Fail;
    c.push(other);
    assert_eq!(
        project_all(&c, &[], false).conflicts,
        vec![ReportType::Review]
    );
    c.reverse();
    assert!(!project_all(&c, &[], false).complete);
}
#[test]
fn exact_resolution_cannot_cover_a_late_candidate() {
    let mut c = all();
    let mut x = c[1].clone();
    x.receipt_id = "r-review-2".into();
    c.push(x);
    let mut pm = candidate(ReportType::PmDecision);
    pm.report.context.slot_resolution = Some(Resolution {
        delivery_identity: "a".repeat(64),
        report_type: ReportType::Review,
        candidate_receipt_ids: vec!["r-review".into(), "r-review-2".into()],
        selected_receipt_id: "r-review".into(),
        supersedes_resolution_receipt_id: None,
        reason: "PM claim".into(),
    });
    assert!(project_all(&c, &[pm.clone()], false).complete);
    let mut late = c[1].clone();
    late.receipt_id = "r-review-3".into();
    c.push(late);
    assert!(!project_all(&c, &[pm], false).complete);
}
#[test]
fn product_once_policy_and_internal_zero_are_separate() {
    assert!(product_auto_allowed(0));
    assert!(!product_auto_allowed(1));
    assert!(!product_auto_allowed(u64::MAX));
    assert!(!internal_auto_allowed());
}
#[test]
fn no_new_delivery_invalidates_even_all_pass_slots() {
    assert!(!project_all(&all(), &[], true).complete);
}
#[test]
fn ci_empty_issues_do_not_close_review_blocker() {
    let mut c = all();
    c[1].report.issues.push(issue(true));
    assert!(!project_all(&c, &[], false).complete);
    assert!(c[2].report.issues.is_empty());
}
#[test]
fn missing_author_does_not_get_promoted_by_hash() {
    let mut c = all();
    c[1].report.claims.author = None;
    assert!(project_all(&c, &[], false)
        .blocking
        .contains(&"AUTHOR_CLAIM_MISSING".into()));
}
#[test]
fn review_issue_history_cannot_silently_drop_or_rewrite() {
    let mut old = report(ReportType::Review);
    old.issues.push(issue(false));
    let mut new = old.clone();
    new.issues.clear();
    assert_eq!(validate_history(&old, &new), Err("ISSUE_HISTORY_GAP"));
    new = old.clone();
    new.issues[0].first_seen_sha = sha('b');
    assert!(validate_history(&old, &new).is_err());
}
#[test]
fn unknown_issue_ancestry_never_auto_closes() {
    let m = manifest(Path::new("E:/repo"), Path::new("E:/reports"));
    let e = envelope(&m, "root");
    let mut r = report(ReportType::Review);
    r.issues.push(issue(false));
    r.issues[0].first_seen_sha = sha('0');
    assert_eq!(e.validate_report(&m, &r), Err("ISSUE_ANCESTRY_UNKNOWN"));
}
#[test]
fn phase_mapping_reuses_existing_states_and_rejects_completed() {
    for s in [
        TaskState::Draft,
        TaskState::AwaitingConfirmation,
        TaskState::Confirmed,
        TaskState::Queued,
        TaskState::Running,
        TaskState::Verifying,
        TaskState::SafeFixing,
        TaskState::PreviewReady,
        TaskState::AwaitingUserAcceptance,
        TaskState::NeedsUserAction,
        TaskState::Interrupted,
        TaskState::Cancelled,
    ] {
        assert!(phase(s).is_ok())
    }
    assert!(phase(TaskState::Completed).is_err());
    assert!(phase(TaskState::Failed).is_err());
}
#[test]
fn envelope_audit_noise_does_not_change_delivery_identity() {
    let m = manifest(Path::new("E:/repo"), Path::new("E:/reports"));
    let mut e = envelope(&m, "root");
    let d = e.delivery_identity.clone();
    let audit = e.envelope_id.clone();
    e.outside_scope_notice = "UNKNOWN".into();
    e.observed_utc = "another-time".into();
    e.bind(&m).unwrap();
    assert_eq!(d, e.delivery_identity);
    assert_ne!(audit, e.envelope_id);
    e.head = sha('c');
    e.ancestors.insert(0, sha('c'));
    e.bind(&m).unwrap();
    assert_ne!(d, e.delivery_identity);
}
#[test]
fn derived_requires_transcriber_not_author_substitution() {
    let mut r = report(ReportType::Review);
    r.source_report = Some(SourceReport {
        receipt_id: "source".into(),
    });
    assert!(r.validate().is_err());
    r.transcribed_by = Some(Transcriber {
        name: "human".into(),
        role: "transcriber".into(),
    });
    assert!(r.validate().is_ok());
    r.source_report = None;
    assert!(r.validate().is_err());
}

fn envelope(m: &Manifest, root: &str) -> Envelope {
    let mut e = Envelope {
        head: sha('b'),
        parent: sha('a'),
        base: sha('a'),
        base_source: "HUMAN_ATTESTED".into(),
        root_identity: root.into(),
        git_dir: Path::new(&m.repo_path)
            .join(".git")
            .to_string_lossy()
            .into(),
        tree: vec![TreeEntry {
            path: "Logo.tsx".into(),
            mode: "100644".into(),
            object: sha('c'),
        }],
        files: vec![],
        ancestors: vec![sha('b'), sha('a')],
        attributes: json!([]),
        outside_scope_notice: "INJECTED".into(),
        observed_utc: "INJECTED".into(),
        delivery_identity: String::new(),
        envelope_id: String::new(),
    };
    e.bind(m).unwrap();
    e
}
#[cfg(windows)]
struct Fixture {
    root: PathBuf,
    repo: PathBuf,
    reports: PathBuf,
    m: Manifest,
    e: Envelope,
}
#[cfg(windows)]
impl Fixture {
    fn new() -> Self {
        let base = std::env::var_os("NOVA_SH1_TEST_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!("sh1-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let repo = root.join("repo");
        let reports = root.join("reports");
        fs::create_dir(&repo).unwrap();
        fs::create_dir(&reports).unwrap();
        fs::create_dir(repo.join(".git")).unwrap();
        fs::write(repo.join("Logo.tsx"), b"fixture").unwrap();
        let m = manifest(&repo, &reports);
        let e = envelope(&m, &Guard::open(&repo, true).unwrap().identity);
        Self {
            root,
            repo,
            reports,
            m,
            e,
        }
    }
    fn store(&self) -> Store {
        Store::init(&self.root.join("owned"), &self.m, &self.e).unwrap()
    }
    fn import(&self, s: &Store, r: &Report) -> Result<Value> {
        self.import_env(s, r, &self.e)
    }
    fn import_env(&self, s: &Store, r: &Report, e: &Envelope) -> Result<Value> {
        let b = bytes(r);
        let p = self.reports.join(format!("{}.json", hash(&b)));
        if !p.exists() {
            fs::write(&p, &b).unwrap()
        }
        s.import(
            &s.snapshot()?,
            e,
            &Import {
                path: p.to_string_lossy().into(),
                bytes: b,
                report: Some(r.clone()),
            },
        )
    }
}
#[cfg(windows)]
#[test]
fn actual_owned_store_guard_migrations_and_transition_side_effects() {
    let f = Fixture::new();
    let s = f.store();
    let status = s.status(&f.e).unwrap();
    assert_eq!(status["task_state"], "queued");
    let backups = fs::read_dir(f.root.join("owned/backups")).unwrap().count();
    assert_eq!(backups, 11);
    for t in [ReportType::Review, ReportType::Ci, ReportType::Development] {
        f.import(&s, &report(t)).unwrap();
    }
    let status = s.status(&f.e).unwrap();
    assert_eq!(status["task_state"], "awaiting_user_acceptance");
    assert_eq!(status["result_generation"], 1);
    assert!(status["projection"]["complete"].as_bool().unwrap());
    assert!(Store::open(&f.root.join("owned")).is_ok());
}
#[cfg(windows)]
#[test]
fn actual_idempotent_import_and_stale_revision_do_not_duplicate() {
    let f = Fixture::new();
    let s = f.store();
    let r = report(ReportType::Development);
    let snap = s.snapshot().unwrap();
    let first = f.import(&s, &r).unwrap();
    let second = f.import(&s, &r).unwrap();
    assert_eq!(first["receipt_id"], second["receipt_id"]);
    assert_eq!(second["idempotent"], true);
    let other = report(ReportType::Ci);
    let b = bytes(&other);
    assert_eq!(
        s.import(
            &snap,
            &f.e,
            &Import {
                path: "unused".into(),
                bytes: b,
                report: Some(other)
            }
        )
        .unwrap_err(),
        "CONCURRENT_REVISION"
    );
    assert_eq!(s.snapshot().unwrap().revision, 1);
}
#[cfg(windows)]
#[test]
fn actual_no_new_delivery_and_repair_only_clear_invalidation() {
    let f = Fixture::new();
    let s = f.store();
    for t in [ReportType::Development, ReportType::Review, ReportType::Ci] {
        f.import(&s, &report(t)).unwrap();
    }
    let mut repair = report(ReportType::Development);
    repair.context.stage = "REPAIR".into();
    repair.context.supersedes_sha = Some(sha('b'));
    repair.context.pm_decision_ref = Some("pm-manual".into());
    f.import(&s, &repair).unwrap();
    assert!(s.snapshot().unwrap().invalidated);
    assert_eq!(s.snapshot().unwrap().manual_rounds, 1);
    assert_eq!(s.snapshot().unwrap().phase, "PM_DECISION_REQUIRED");
    f.import(&s, &repair).unwrap();
    assert!(s.snapshot().unwrap().invalidated);
    let mut new = f.e.clone();
    new.head = sha('d');
    new.ancestors.insert(0, sha('d'));
    new.bind(&f.m).unwrap();
    repair.claimed_delivery_sha = sha('d');
    f.import_env(&s, &repair, &new).unwrap();
    assert!(!s.snapshot().unwrap().invalidated);
    assert_eq!(s.snapshot().unwrap().manual_rounds, 2);
    assert_eq!(s.snapshot().unwrap().phase, "MATERIAL_COLLECTION");
}
#[cfg(windows)]
#[test]
fn actual_slot_conflict_resolution_then_explicit_continue() {
    let f = Fixture::new();
    let s = f.store();
    for t in [ReportType::Development, ReportType::Review, ReportType::Ci] {
        f.import(&s, &report(t)).unwrap();
    }
    let mut second = report(ReportType::Review);
    second.claims.author = Some("second claim".into());
    f.import(&s, &second).unwrap();
    assert_eq!(s.snapshot().unwrap().phase, "PM_DECISION_REQUIRED");
    let status = s.status(&f.e).unwrap();
    let ids = status["receipts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["claims"]["value"]["report_type"] == "REVIEW")
        .map(|v| v["receipt_id"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    let mut pm = report(ReportType::PmDecision);
    pm.context.decision_kind = Some("SLOT_RESOLUTION".into());
    pm.context.pm_decision_ref = Some("pm-select".into());
    pm.context.slot_resolution = Some(Resolution {
        delivery_identity: f.e.delivery_identity.clone(),
        report_type: ReportType::Review,
        candidate_receipt_ids: ids.clone(),
        selected_receipt_id: ids[0].clone(),
        supersedes_resolution_receipt_id: None,
        reason: "PM selection claim".into(),
    });
    f.import(&s, &pm).unwrap();
    assert_eq!(s.snapshot().unwrap().phase, "PM_DECISION_REQUIRED");
    pm.context.decision_kind = Some("CONTINUE_MATERIALS".into());
    pm.context.slot_resolution = None;
    f.import(&s, &pm).unwrap();
    assert_eq!(s.snapshot().unwrap().phase, "AWAITING_PM");
    assert_eq!(s.status(&f.e).unwrap()["result_generation"], 2);
}
#[cfg(windows)]
#[test]
fn actual_closure_reference_freezes_new_writes_not_completed() {
    let f = Fixture::new();
    let s = f.store();
    for t in [ReportType::Development, ReportType::Review, ReportType::Ci] {
        f.import(&s, &report(t)).unwrap();
    }
    let mut pm = report(ReportType::PmDecision);
    pm.context.decision_kind = Some("CLOSURE_REFERENCE".into());
    pm.context.pm_decision_ref = Some("closed-claim".into());
    f.import(&s, &pm).unwrap();
    assert!(s.snapshot().unwrap().closed);
    let mut new = report(ReportType::Review);
    new.claims.author = Some("new".into());
    assert_eq!(f.import(&s, &new).unwrap_err(), "REGISTRATION_CLOSED");
    assert_eq!(
        s.status(&f.e).unwrap()["task_state"],
        "awaiting_user_acceptance"
    );
    assert_eq!(f.import(&s, &pm).unwrap()["idempotent"], true);
}
#[cfg(windows)]
#[test]
fn actual_index_only_receipt_and_single_derived_link() {
    let f = Fixture::new();
    let s = f.store();
    let p = f.reports.join("legacy.md");
    fs::write(&p, b"original report bytes").unwrap();
    let result = s
        .import(
            &s.snapshot().unwrap(),
            &f.e,
            &Import {
                path: p.to_string_lossy().into(),
                bytes: b"original report bytes".to_vec(),
                report: None,
            },
        )
        .unwrap();
    let mut r = report(ReportType::Review);
    r.source_report = Some(SourceReport {
        receipt_id: result["receipt_id"].as_str().unwrap().into(),
    });
    r.transcribed_by = Some(Transcriber {
        name: "transcriber".into(),
        role: "transcriber".into(),
    });
    let derived = f.import(&s, &r).unwrap();
    let mut bad = report(ReportType::Ci);
    bad.source_report = Some(SourceReport {
        receipt_id: derived["receipt_id"].as_str().unwrap().into(),
    });
    bad.transcribed_by = r.transcribed_by.clone();
    assert_eq!(f.import(&s, &bad).unwrap_err(), "DERIVATION_REJECTED");
    assert_eq!(
        s.status(&f.e).unwrap()["receipts"][0]["receipt_id"],
        result["receipt_id"]
    );
    fs::write(&p, b"changed").unwrap();
    assert_eq!(s.status(&f.e).unwrap()["report_identity_problem"], true);
}
#[cfg(windows)]
#[test]
fn actual_file_handle_rejects_replacement_write_and_hardlink() {
    let f = Fixture::new();
    let p = f.repo.join("Logo.tsx");
    let g = Guard::open(&p, false).unwrap();
    assert!(fs::write(&p, b"other").is_err());
    assert!(fs::rename(&p, f.repo.join("moved")).is_err());
    g.verify().unwrap();
    drop(g);
    let alias = f.repo.join("alias.txt");
    fs::hard_link(&p, &alias).unwrap();
    assert!(Guard::open(&p, false).is_err());
    assert!(Guard::open(&alias, false).is_err());
}
#[cfg(windows)]
#[test]
fn actual_existing_unknown_and_repository_store_roots_rejected() {
    let f = Fixture::new();
    assert!(Store::open(&f.root).is_err());
    assert!(Store::init(&f.repo.join("bad"), &f.m, &f.e).is_err());
    let _s = f.store();
    assert!(Store::init(&f.root.join("owned"), &f.m, &f.e).is_err());
}
#[cfg(windows)]
#[test]
fn actual_partial_initialization_never_auto_repairs() {
    let f = Fixture::new();
    for point in 1..=4 {
        let p = f.root.join(format!("partial-{point}"));
        assert_eq!(
            super::store::test_init_fault(&p, &f.m, &f.e, point).err(),
            Some("INJECTED_INIT_FAILURE")
        );
        assert!(Store::open(&p).is_err());
        assert!(Store::init(&p, &f.m, &f.e).is_err());
    }
}
#[cfg(windows)]
#[test]
fn actual_foreign_keys_enforced_and_product_fact_tables_empty() {
    let f = Fixture::new();
    let s = f.store();
    super::store::test_constraints(&s).unwrap();
}

#[cfg(windows)]
#[test]
fn actual_two_connection_compare_and_swap_has_one_winner() {
    use std::sync::{Arc, Barrier};
    let f = Fixture::new();
    let _s = f.store();
    let barrier = Arc::new(Barrier::new(2));
    let mut threads = vec![];
    for index in 0..2 {
        let root = f.root.join("owned");
        let reports = f.reports.clone();
        let b = barrier.clone();
        let e = f.e.clone();
        threads.push(std::thread::spawn(move || {
            let s = Store::open(&root).unwrap();
            let snapshot = s.snapshot().unwrap();
            let mut r = report(ReportType::Development);
            r.claims.author = Some(format!("author-{index}"));
            let bytes = bytes(&r);
            let path = reports.join(format!("concurrent-{index}.json"));
            fs::write(&path, &bytes).unwrap();
            b.wait();
            s.import(
                &snapshot,
                &e,
                &Import {
                    path: path.to_string_lossy().into(),
                    bytes,
                    report: Some(r),
                },
            )
        }));
    }
    let results = threads
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err("CONCURRENT_REVISION")))
            .count(),
        1
    );
    assert_eq!(_s.snapshot().unwrap().revision, 1);
}
#[cfg(windows)]
#[test]
fn injected_import_failure_rolls_back_real_receipt_and_state() {
    let f = Fixture::new();
    let s = f.store();
    let r = report(ReportType::Development);
    let b = bytes(&r);
    let path = f.reports.join("failed.json");
    fs::write(&path, &b).unwrap();
    let input = Import {
        path: path.to_string_lossy().into(),
        bytes: b,
        report: Some(r),
    };
    assert_eq!(
        super::store::test_fail_import(&s, &f.e, &input).unwrap_err(),
        "INJECTED_IMPORT_FAILURE"
    );
    assert_eq!(s.snapshot().unwrap().revision, 0);
    assert_eq!(s.snapshot().unwrap().phase, "HANDOFF_READY");
    assert!(s.status(&f.e).unwrap()["receipts"]
        .as_array()
        .unwrap()
        .is_empty());
    s.import(&s.snapshot().unwrap(), &f.e, &input).unwrap();
}
#[cfg(windows)]
#[test]
fn actual_ownership_marker_schema_and_identity_changes_rejected() {
    let f = Fixture::new();
    let s = f.store();
    drop(s);
    let path = f.root.join("owned/owner.json");
    let b = fs::read(&path).unwrap();
    let mut v: Value = serde_json::from_slice(&b).unwrap();
    v["db_identity"] = json!("wrong");
    fs::write(&path, serde_json::to_vec(&v).unwrap()).unwrap();
    assert!(Store::open(&f.root.join("owned")).is_err());
}
#[cfg(windows)]
#[test]
fn actual_pause_then_continue_requires_reference_and_keeps_materials() {
    let f = Fixture::new();
    let s = f.store();
    f.import(&s, &report(ReportType::Development)).unwrap();
    let mut pm = report(ReportType::PmDecision);
    pm.context.decision_kind = Some("PAUSE".into());
    pm.context.pm_decision_ref = Some("pause-claim".into());
    f.import(&s, &pm).unwrap();
    assert_eq!(s.snapshot().unwrap().phase, "PAUSED");
    pm.context.decision_kind = Some("CONTINUE_MATERIALS".into());
    f.import(&s, &pm).unwrap();
    assert_eq!(s.snapshot().unwrap().phase, "MATERIAL_COLLECTION");
    assert_eq!(s.snapshot().unwrap().manual_rounds, 0);
}
