//! New ordinary tests are PURE INJECTION: no socket, randomness, process/Job or filesystem call.
use super::{
    opencode_native_files::*, opencode_native_process::*, opencode_nonce::*, opencode_paths::*,
    opencode_probe::*,
};
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
fn secrets() -> RunSecrets {
    RunSecrets {
        model: RunNonce::injected(|b| {
            b.fill(0x11);
            Ok(())
        })
        .unwrap(),
        control: RunNonce::injected(|b| {
            b.fill(0x22);
            Ok(())
        })
        .unwrap(),
    }
}
fn env_rows(b: &EnvironmentBlock) -> BTreeMap<String, String> {
    String::from_utf16(b.units())
        .unwrap()
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|s| {
            let (k, v) = s.split_once('=').unwrap();
            (k.into(), v.into())
        })
        .collect()
}
#[test]
fn environment_uppercase_ordinal_puts_letters_before_underscore() {
    let s = secrets();
    let b = EnvironmentBlock::encode(
        vec![("_A", "1"), ("z", "2"), ("a", "3")],
        &s,
        0,
        LaunchRole::GitPreflight,
    )
    .unwrap();
    let names: String = String::from_utf16(b.units()).unwrap();
    assert!(names.starts_with("a=3\0z=2\0_A=1\0"));
}
#[test]
fn environment_duplicate_names_nul_and_illegal_keys_are_rejected() {
    let s = secrets();
    for values in [
        vec![("a", "1"), ("A", "2")],
        vec![("", "v")],
        vec![("=C:", "v")],
        vec![("A-B", "v")],
        vec![("A", "v\0")],
        vec![("opencode_server_password", "override")],
    ] {
        assert!(EnvironmentBlock::encode(values, &s, 1, LaunchRole::OpenCode).is_err())
    }
}
#[test]
fn environment_bound_rejects_instead_of_truncating() {
    let s = secrets();
    let huge = "x".repeat(32768);
    assert!(EnvironmentBlock::encode(vec![("A", &huge)], &s, 0, LaunchRole::GitPreflight).is_err());
}
#[test]
fn git_environment_is_secret_free_from_empty_input() {
    let b = git_environment().unwrap();
    let rows = env_rows(&b);
    assert_eq!(rows.get("GIT_CONFIG_NOSYSTEM").unwrap(), "1");
    assert!(!rows.contains_key("OPENCODE_SERVER_PASSWORD"));
    assert!(!rows.contains_key("OPENCODE_CONFIG_CONTENT"));
    assert!(!rows.contains_key("GIT_CONFIG_COUNT"));
    assert!(b.units().ends_with(&[0, 0]));
}
#[test]
fn model_overlay_is_valid_json_without_substitution_directives() {
    let s = secrets();
    let b = opencode_environment(&s, 1234).unwrap();
    let rows = env_rows(&b);
    let j: serde_json::Value = serde_json::from_str(&rows["OPENCODE_CONFIG_CONTENT"]).unwrap();
    assert_eq!(
        j["provider"]["nova"]["options"]["baseURL"],
        "http://127.0.0.1:1234/v1"
    );
    let m = j["provider"]["nova"]["options"]["apiKey"].as_str().unwrap();
    assert_eq!(m.len(), 64);
    assert_ne!(m, rows["OPENCODE_SERVER_PASSWORD"]);
    assert!(!rows["OPENCODE_CONFIG_CONTENT"].contains("{env:"));
    assert!(!rows["OPENCODE_CONFIG_CONTENT"].contains("{file:"));
}
#[test]
fn equal_purpose_secrets_and_zero_port_refuse() {
    let s = RunSecrets {
        model: RunNonce::injected(|b| {
            b.fill(1);
            Ok(())
        })
        .unwrap(),
        control: RunNonce::injected(|b| {
            b.fill(1);
            Ok(())
        })
        .unwrap(),
    };
    assert!(opencode_environment(&s, 1).is_err());
    assert!(opencode_environment(&secrets(), 0).is_err());
}
#[test]
fn independent_environment_buffers_do_not_alias() {
    let s = secrets();
    let a = opencode_environment(&s, 123).unwrap();
    let b = opencode_environment(&s, 456).unwrap();
    assert_ne!(a.units(), b.units());
    assert!(env_rows(&a)["OPENCODE_CONFIG_CONTENT"].contains(":123/v1"));
}
#[test]
fn command_quoting_preserves_empty_spaces_quotes_and_trailing_backslashes() {
    assert_eq!(quote_arg("").unwrap(), "\"\"");
    assert_eq!(quote_arg("two words").unwrap(), "\"two words\"");
    assert_eq!(quote_arg("a\"b").unwrap(), "\"a\\\"b\"");
    assert_eq!(quote_arg("a\\").unwrap(), "\"a\\\\\"");
    assert!(quote_arg("x\0").is_err());
    assert!(quote_arg("x\ny").is_err());
}
#[test]
fn fixed_images_never_allow_shell_batch_or_arbitrary_target() {
    for p in [
        r"D:\NovaM0BInput\git.cmd",
        r"C:\Windows\System32\cmd.exe",
        r"D:\NovaM0BInput\other.exe",
    ] {
        assert!(FrozenImage {
            role: LaunchRole::GitPreflight,
            path: p.into(),
            sha256: "a".repeat(64)
        }
        .validate()
        .is_err())
    }
    assert!(FrozenImage {
        role: LaunchRole::GitPreflight,
        path: r"D:\NovaM0BInput\git\cmd\git.exe".into(),
        sha256: "a".repeat(64)
    }
    .validate()
    .is_ok());
}
#[test]
fn git_five_seconds_contains_cleanup_and_reader_reserves() {
    let d = DeadlinePlan::git(100, 30100, 120100, 240100).unwrap();
    assert_eq!(d.chain, 3100);
    assert_eq!(d.cleanup_end, 4100);
    assert_eq!(d.reader_end, 5100);
    assert_eq!(d.cleanup_at(3150), 4100);
}
#[test]
fn nested_deadlines_and_exhausted_budget_refuse_start() {
    let d = DeadlinePlan::git(100, 4500, 4000, 3800).unwrap();
    assert_eq!(d.reader_end, 3800);
    assert_eq!(d.chain, 1800);
    assert!(DeadlinePlan::git(100, 2100, 10000, 20000).is_err());
    assert!(DeadlinePlan::git(u64::MAX, 1, 2, 3).is_err());
    let d = DeadlinePlan::opencode(100, 20100, 10000, 9000).unwrap();
    assert_eq!(d.chain, 5100);
    assert_eq!(d.reader_end, 9000);
}
struct FakeProcess {
    base: Instant,
    ms: u64,
    create_ms: u64,
    termination_ms: u64,
    identity_ok: bool,
    exit_after: u64,
    fail: &'static str,
    calls: Vec<&'static str>,
}
impl FakeProcess {
    fn new() -> Self {
        Self {
            base: Instant::now(),
            ms: 0,
            create_ms: 0,
            termination_ms: 0,
            identity_ok: true,
            exit_after: 0,
            fail: "",
            calls: vec![],
        }
    }
    fn hit(&mut self, n: &'static str) -> super::Result<()> {
        self.calls.push(n);
        if self.fail == n {
            Err("injected API failure".into())
        } else {
            Ok(())
        }
    }
}
impl ProcessOps for FakeProcess {
    fn now(&self) -> Instant {
        self.base + Duration::from_millis(self.ms)
    }
    fn pause(&mut self) {
        self.ms += 1;
    }
    fn create_suspended(&mut self) -> super::Result<()> {
        self.ms += self.create_ms;
        self.hit("create")
    }
    fn validate_identity(&mut self) -> super::Result<()> {
        self.hit("identity")?;
        if self.identity_ok {
            Ok(())
        } else {
            Err("identity UNKNOWN".into())
        }
    }
    fn assign(&mut self) -> super::Result<()> {
        self.hit("assign")
    }
    fn flags_and_active(&mut self) -> super::Result<(u32, u32)> {
        self.hit("query")?;
        Ok((
            0x2000,
            if (self.calls.contains(&"resume") && self.ms >= self.exit_after)
                || self.calls.contains(&"stop-job")
            {
                0
            } else {
                1
            },
        ))
    }
    fn resume(&mut self) -> super::Result<()> {
        self.hit("resume")
    }
    fn terminate_suspended(&mut self) -> super::Result<()> {
        self.ms += self.termination_ms;
        self.hit("terminate")
    }
    fn terminate_job(&mut self) -> super::Result<()> {
        self.hit("stop-job")
    }
    fn exited(&mut self) -> super::Result<bool> {
        self.hit("exit")?;
        Ok(self.ms >= self.exit_after)
    }
}
fn until(p: &mut FakeProcess, chain: u64, cleanup: u64) -> std::result::Result<(), LaunchFailure> {
    let a = p.base + Duration::from_millis(chain);
    let b = p.base + Duration::from_millis(cleanup);
    launch_verified_until(p, a, b)
}
#[test]
fn late_create_uses_reserved_cleanup_never_resumes() {
    let mut p = FakeProcess::new();
    p.create_ms = 3100;
    let e = until(&mut p, 3000, 4000).unwrap_err();
    assert!(e.direct_cleanup_confirmed());
    assert!(!p.calls.contains(&"resume"));
    assert_eq!(p.ms, 3100);
}
#[test]
fn late_beyond_outer_is_unknown_not_cleaned() {
    let mut p = FakeProcess::new();
    p.create_ms = 5100;
    let e = until(&mut p, 3000, 4000).unwrap_err();
    assert!(!e.direct_cleanup_confirmed());
    assert_eq!(e.cleanup.unwrap().error, Some(StopError::Deadline));
    assert!(!p.calls.contains(&"resume"));
}
#[test]
fn cleanup_poll_does_not_regain_budget() {
    let mut p = FakeProcess::new();
    p.identity_ok = false;
    p.exit_after = 5000;
    let e = until(&mut p, 3000, 4000).unwrap_err();
    assert!(!e.direct_cleanup_confirmed());
    assert_eq!(p.ms, 1000);
    assert!(!p.calls.contains(&"resume"));
}
#[test]
fn no_remaining_budget_never_creates() {
    let mut p = FakeProcess::new();
    assert!(until(&mut p, 0, 10).is_err());
    assert!(p.calls.is_empty());
}
#[test]
fn identity_unknown_blocks_assign_and_resume() {
    let mut p = FakeProcess::new();
    p.identity_ok = false;
    let e = until(&mut p, 3000, 4000).unwrap_err();
    assert!(!p.calls.contains(&"assign"));
    assert!(!p.calls.contains(&"resume"));
    assert!(e.direct_cleanup_confirmed());
}
#[test]
fn launch_checks_identity_both_before_assign_and_before_resume() {
    let mut p = FakeProcess::new();
    launch_verified(&mut p).unwrap();
    assert_eq!(
        p.calls,
        ["create", "identity", "assign", "query", "identity", "resume"]
    );
}
#[test]
fn failed_termination_and_exit_unknown_never_claim_success() {
    for f in ["terminate", "exit"] {
        let mut p = FakeProcess::new();
        p.identity_ok = false;
        p.fail = f;
        let e = launch_verified(&mut p).unwrap_err();
        assert!(!e.direct_cleanup_confirmed());
        assert!(!p.calls.contains(&"resume"));
    }
}
#[test]
fn access_time_is_fact_but_not_equality_requirement() {
    let a = injected_tree();
    let mut b = a.clone();
    for f in &mut b.files {
        f.access = 77;
    }
    assert!(review_tree(&a.files, &b.files, &"a".repeat(40), "probe-sid", false).is_ok());
    let r = AgentTreeReceipt {
        after: b,
        descendants: vec![],
        writes: vec![],
    };
    assert!(review_agent_tree(
        &a,
        &r,
        &injected_preflight().fixture,
        &injected_target(),
        FactSource::Injected
    )
    .is_ok());
}
#[test]
fn nonmarker_creation_write_and_change_still_reject() {
    let a = injected_tree();
    for field in ["creation", "write", "change"] {
        let mut b = a.clone();
        let f = b.files.iter_mut().find(|f| f.path == "Logo.tsx").unwrap();
        match field {
            "creation" => f.creation += 1,
            "write" => f.write += 1,
            _ => f.change += 1,
        };
        assert!(review_tree(&a.files, &b.files, &"a".repeat(40), "probe-sid", true).is_err());
    }
}
#[test]
fn marker_creation_not_forgiven_but_declared_write_change_are() {
    let a = injected_tree();
    let mut b = a.clone();
    let f = b
        .files
        .iter_mut()
        .find(|f| f.path == ".git/opencode")
        .unwrap();
    f.access += 1;
    f.write += 1;
    f.change += 1;
    assert!(review_tree(&a.files, &b.files, &"a".repeat(40), "probe-sid", true).is_ok());
    b.files
        .iter_mut()
        .find(|f| f.path == ".git/opencode")
        .unwrap()
        .creation += 1;
    assert!(review_tree(&a.files, &b.files, &"a".repeat(40), "probe-sid", true).is_err());
}
#[test]
fn frozen_policy_and_preparation_must_bind_not_follow_current_fact() {
    let r = injected_preflight().fixture;
    let mut s = injected_tree();
    s.files[0].dacl = "other".into();
    assert!(validate_snapshot(&s, &r, FactSource::Injected).is_err());
    let mut bad = r.clone();
    bad.prepared.as_mut().unwrap().policy_sha256 = "0".repeat(64);
    assert!(validate_snapshot(&injected_tree(), &bad, FactSource::Injected).is_err());
}
#[test]
fn missing_policy_receipt_and_unknown_enumeration_reject() {
    let s = injected_tree();
    let mut r = injected_preflight().fixture;
    r.policy = None;
    assert!(validate_snapshot(&s, &r, FactSource::Injected).is_err());
    let r = injected_preflight().fixture;
    for f in [0, 1, 2, 3, 4] {
        let mut s = s.clone();
        match f {
            0 => s.enumeration = None,
            1 => s.enumeration.as_mut().unwrap().error = Some("UNKNOWN".into()),
            2 => s.enumeration.as_mut().unwrap().streams_ended -= 1,
            3 => s.enumeration.as_mut().unwrap().finished = 10001,
            _ => s.enumeration.as_mut().unwrap().directories_ended = 0,
        };
        assert!(validate_snapshot(&s, &r, FactSource::Injected).is_err());
    }
}
#[test]
fn actual_owner_ads_identity_and_new_entries_do_not_redefine_expectation() {
    let r = injected_preflight().fixture;
    for i in 0..4 {
        let mut s = injected_tree();
        match i {
            0 => s.files[0].owner = "other".into(),
            1 => s.files[0].ads.push(("hidden".into(), vec![1])),
            2 => s.files[0].identity = "same-path-new-id".into(),
            _ => {
                let mut e = s.files[0].clone();
                e.path = "new-file".into();
                s.files.push(e)
            }
        };
        assert!(validate_snapshot(&s, &r, FactSource::Injected).is_err());
    }
}
#[test]
fn wrong_root_filesystem_and_source_refuse() {
    let mut r = injected_preflight().fixture;
    r.prepared.as_mut().unwrap().filesystem = "FAT32".into();
    assert!(validate_snapshot(&injected_tree(), &r, FactSource::Injected).is_err());
    let mut r = injected_preflight().fixture;
    r.prepared.as_mut().unwrap().source = FactSource::NativePrepared;
    assert!(validate_snapshot(&injected_tree(), &r, FactSource::Injected).is_err());
}
#[test]
fn acl_free_capacity_is_not_evidence_but_ace_bytes_are() {
    let a = [2, 0, 8, 0, 0, 0, 0, 0];
    let b = [2, 0, 12, 0, 0, 0, 0, 0, 99, 98, 97, 96];
    assert_eq!(
        canonical_acl(&a, 8, 4).unwrap(),
        canonical_acl(&b, 8, 4).unwrap()
    );
    let mut c = b;
    c[0] = 4;
    assert_ne!(
        canonical_acl(&a, 8, 4).unwrap(),
        canonical_acl(&c, 8, 4).unwrap()
    );
    assert!(canonical_acl(&b, 10, 4).is_err());
}
#[test]
fn acl_ace_truncation_and_control_changes_refuse() {
    let a = [2, 0, 12, 0, 1, 0, 0, 0, 0, 0, 4, 0];
    let mut b = a;
    b[10] = 8;
    assert!(canonical_acl(&b, 12, 4).is_err());
    assert_ne!(
        canonical_acl(&a, 12, 4).unwrap(),
        canonical_acl(&a, 12, 0x1004).unwrap()
    );
}
#[test]
fn fixed_path_depth_alias_and_ads_syntax_are_closed() {
    for s in [
        "../escape",
        "a//b",
        "a/../b",
        "a:stream",
        "a\\b",
        "a/.",
        "a ",
        "a\0b",
    ] {
        assert!(!relative(s))
    }
    assert!(relative(".git/opencode"));
    assert!(!relative(&vec!["a"; 33].join("/")));
}
#[test]
fn collection_counter_short_read_or_late_receipt_refuse() {
    let r = injected_preflight().fixture;
    for i in 0..3 {
        let mut s = injected_tree();
        match i {
            0 => s.enumeration.as_mut().unwrap().default_bytes += 1,
            1 => s.files[2].bytes.pop().map(|_| ()).unwrap(),
            _ => s.enumeration.as_mut().unwrap().root_identity = "changed".into(),
        };
        assert!(validate_snapshot(&s, &r, FactSource::Injected).is_err());
    }
}
#[test]
fn native_expectations_cannot_accept_injected_identities() {
    let mut p = injected_policy();
    p.source = FactSource::NativePrepared;
    assert!(p.validate().is_err());
    let mut r = injected_prepared();
    r.source = FactSource::NativePrepared;
    assert!(r.validate(&injected_policy()).is_err());
}
#[test]
#[ignore = "Windows native parent/token query requires separate authorization"]
fn native_parent_policy_receipt() {
    assert_eq!(
        super::opencode_native_process::native::parent_policy().unwrap(),
        0
    );
}

struct InjectedIo {
    cost: u64,
    fail: bool,
    started: bool,
}
impl GitIo<FakeProcess> for InjectedIo {
    fn start(&mut self, _: &mut FakeProcess) -> super::Result<()> {
        self.started = true;
        Ok(())
    }
    fn overflow(&self) -> bool {
        false
    }
    fn finish(&mut self, p: &mut FakeProcess, deadline: Instant) -> super::Result<Vec<u8>> {
        p.ms += self.cost;
        if self.fail || p.now() >= deadline {
            Err("injected reader late UNKNOWN".into())
        } else {
            Ok(b"fixed-root\n".to_vec())
        }
    }
}
#[test]
fn actual_git_skeleton_uses_one_clock_for_execution_and_reader_total() {
    let mut p = FakeProcess::new();
    p.create_ms = 100;
    p.exit_after = 800;
    let plan = DeadlinePlan::git(0, 30000, 120000, 240000).unwrap();
    let mut io = InjectedIo {
        cost: 700,
        fail: false,
        started: false,
    };
    let base = p.base;
    let r = supervise_git(&mut p, &mut io, base, plan).unwrap();
    assert!(r.natural);
    assert!(io.started);
    assert_eq!(p.ms, 1500);
    assert!(p.ms <= 5000);
    assert_eq!(r.job.active, Some(0));
}
#[test]
fn actual_git_skeleton_late_reader_is_not_success() {
    let mut p = FakeProcess::new();
    p.exit_after = 100;
    let plan = DeadlinePlan::git(0, 30000, 120000, 240000).unwrap();
    let mut io = InjectedIo {
        cost: 5000,
        fail: false,
        started: false,
    };
    let base = p.base;
    let r = supervise_git(&mut p, &mut io, base, plan).unwrap_err();
    assert!(!r.readers_confirmed);
    assert!(p.ms > 5000);
    assert_eq!(r.stop.unwrap().active, Some(0));
}
#[test]
fn actual_git_timeout_keeps_cleanup_within_reserved_five_seconds() {
    let mut p = FakeProcess::new();
    p.exit_after = 3500;
    let plan = DeadlinePlan::git(0, 30000, 120000, 240000).unwrap();
    let mut io = InjectedIo {
        cost: 400,
        fail: false,
        started: false,
    };
    let base = p.base;
    let r = supervise_git(&mut p, &mut io, base, plan).unwrap_err();
    assert!(p.calls.contains(&"stop-job"));
    assert_eq!(p.ms, 3900);
    assert!(p.ms <= 5000);
    assert!(r.readers_confirmed);
    assert_eq!(r.stop.unwrap().active, Some(0));
}
#[test]
fn content_read_identity_size_write_and_change_checked_by_native_shared_logic() {
    let a = ReadStamp {
        identity: "original".into(),
        size: 7,
        write: 1,
        change: 2,
    };
    for i in 0..4 {
        let mut b = a.clone();
        match i {
            0 => b.identity = "replacement".into(),
            1 => b.size += 1,
            2 => b.write += 1,
            _ => b.change += 1,
        };
        assert!(unchanged_read(&a, &b).is_err())
    }
    assert!(unchanged_read(&a, &a).is_ok());
}
#[test]
fn acl_effective_ace_bytes_not_discarded() {
    let a = [2, 0, 12, 0, 1, 0, 0, 0, 0, 0, 4, 0];
    let mut b = a;
    b[9] = 1;
    assert_ne!(
        canonical_acl(&a, 12, 4).unwrap(),
        canonical_acl(&b, 12, 4).unwrap()
    );
}

#[test]
#[ignore = "fixed helper execution requires separate native authorization"]
fn native_helper_secret_free_environment_and_eof() {
    super::opencode_native_process::native::helper_fixture(false, git_environment().unwrap())
        .unwrap();
}
#[test]
#[ignore = "fixed helper Job/process-tree termination requires separate native authorization"]
fn native_helper_delegated_job_timeout_and_readers() {
    super::opencode_native_process::native::helper_fixture(true, git_environment().unwrap())
        .unwrap();
}
#[test]
#[ignore = "own NTFS file/ADS/security IO requires separate native authorization"]
fn native_file_content_ads_and_security_facts() {
    super::opencode_native_files::native::read_fixed_native_fixture().unwrap();
}

#[test]
fn reader_reclaim_is_one_second_even_when_outer_has_more_time() {
    let mut p = FakeProcess::new();
    p.exit_after = 100;
    let plan = DeadlinePlan::git(0, 30000, 120000, 240000).unwrap();
    let mut io = InjectedIo {
        cost: 1100,
        fail: false,
        started: false,
    };
    let origin = p.base;
    let r = supervise_git(&mut p, &mut io, origin, plan).unwrap_err();
    assert!(!r.readers_confirmed);
    assert!(p.ms < 5000);
}
#[test]
fn parallel_injected_environment_builds_have_no_shared_secret_buffer() {
    let handles: Vec<_> = (0..2)
        .map(|n| {
            std::thread::spawn(move || {
                let s = secrets();
                let b = opencode_environment(&s, 31001 + n).unwrap();
                env_rows(&b).remove("OPENCODE_CONFIG_CONTENT").unwrap()
            })
        })
        .collect();
    let mut outputs = handles.into_iter().map(|h| h.join().unwrap());
    let a = outputs.next().unwrap();
    let b = outputs.next().unwrap();
    assert!(a.contains("31001"));
    assert!(b.contains("31002"));
    assert_ne!(a, b);
}

#[test]
fn termination_api_time_consumes_the_same_one_second_cleanup_window() {
    let mut p = FakeProcess::new();
    p.identity_ok = false;
    p.termination_ms = 1100;
    p.exit_after = 2000;
    let e = until(&mut p, 3000, 4000).unwrap_err();
    assert_eq!(p.ms, 1100);
    assert!(!e.direct_cleanup_confirmed());
    assert_eq!(e.cleanup.unwrap().error, Some(StopError::Deadline));
    assert!(!p.calls.contains(&"resume"));
    assert!(!p.calls.contains(&"exit"));
}

/// Synthetic NativePrepared-shaped material for source-check tests, NOT native IO evidence.
pub(super) fn n1_native_fixture() -> (FixtureReservation, TreeSnapshot) {
    let mut r = injected_preflight().fixture;
    let mut tree = injected_tree();
    let sid = "S-1-5-21-1001";
    let acl = "0004:0200080000000000";
    r.sid = sid.into();
    for f in &mut tree.files {
        f.owner = sid.into();
        f.dacl = acl.into();
        f.identity = format!("test-native:{}", f.path);
    }
    r.root.identity = "test-native:.".into();
    r.logo_identity = "test-native:Logo.tsx".into();
    r.git_marker_identity = "test-native:.git/opencode".into();
    let p = r.policy.as_mut().unwrap();
    p.source = FactSource::NativePrepared;
    p.sid = sid.into();
    for e in &mut p.entries {
        e.owner = sid.into();
        e.dacl = acl.into();
    }
    let digest = p.digest().unwrap();
    let prepared = r.prepared.as_mut().unwrap();
    prepared.source = FactSource::NativePrepared;
    prepared.policy_sha256 = digest;
    for (path, id) in &mut prepared.identities {
        *id = format!("test-native:{path}");
    }
    let proof = tree.enumeration.as_mut().unwrap();
    proof.source = FactSource::NativePrepared;
    proof.root_identity = r.root.identity.clone();
    (r, tree)
}
#[test]
fn n1_checker_binds_expected_source_in_both_directions_and_retains_valid_paths() {
    let i = injected_preflight().fixture;
    let it = injected_tree();
    let (n, nt) = n1_native_fixture();
    assert!(validate_collected_snapshot(&it, &i, true, FactSource::Injected).is_ok());
    assert!(validate_collected_snapshot(&nt, &n, true, FactSource::NativePrepared).is_ok());
    assert!(validate_collected_snapshot(&it, &i, true, FactSource::NativePrepared).is_err());
    assert!(validate_collected_snapshot(&nt, &n, true, FactSource::Injected).is_err());
    assert!(validate_snapshot(&nt, &n, FactSource::NativePrepared).is_ok());
    assert!(n.validate(FactSource::Injected).is_err());
    assert!(i.validate(FactSource::NativePrepared).is_err());
}
#[test]
fn n1_checker_rejects_each_mismatching_source_without_changing_other_contracts() {
    for wrong in 0..3 {
        let (mut r, mut tree) = n1_native_fixture();
        match wrong {
            0 => r.policy.as_mut().unwrap().source = FactSource::Injected,
            1 => r.prepared.as_mut().unwrap().source = FactSource::Injected,
            _ => tree.enumeration.as_mut().unwrap().source = FactSource::Injected,
        }
        assert!(validate_collected_snapshot(&tree, &r, true, FactSource::NativePrepared).is_err());
    }
}
#[test]
fn n1_review_rejects_wrong_after_source_even_with_valid_native_before() {
    let (r, before) = n1_native_fixture();
    let mut receipt = injected_observation().tree;
    receipt.after = before.clone();
    assert!(review_agent_tree(
        &before,
        &receipt,
        &r,
        &injected_target(),
        FactSource::NativePrepared
    )
    .is_ok());
    receipt.after.enumeration.as_mut().unwrap().source = FactSource::Injected;
    assert!(review_agent_tree(
        &before,
        &receipt,
        &r,
        &injected_target(),
        FactSource::NativePrepared
    )
    .is_err());
    receipt.after = injected_tree();
    assert!(review_agent_tree(
        &before,
        &receipt,
        &r,
        &injected_target(),
        FactSource::NativePrepared
    )
    .is_err());
}
