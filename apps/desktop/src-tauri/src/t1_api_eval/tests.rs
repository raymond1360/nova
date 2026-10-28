use super::*;
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("nova-m0a-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn file(&self) -> PathBuf {
        let p = self.0.join("Logo.tsx");
        fs::write(&p, PRE).unwrap();
        p
    }
    fn tx(&self, p: &Path) -> journal::Transaction {
        let t =
            journal::Transaction::prepare(&self.0.join("tx"), PRE, &oracle(PRE).unwrap()).unwrap();
        cas::reserve_identity(p, &t).unwrap();
        t
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn permission() -> contract::Permission {
    contract::Permission {
        source: "SYNTHETIC".into(),
        id: "a".into(),
        session_id: "s".into(),
        permission: "edit".into(),
        patterns: vec!["Logo.tsx".into()],
        metadata: serde_json::json!({"diff":"intentionally lossy, never parsed"}),
        message_id: "m".into(),
        call_id: "c".into(),
    }
}
fn part() -> contract::ToolPart {
    contract::ToolPart {
        source: "SYNTHETIC".into(),
        session_id: "s".into(),
        message_id: "m".into(),
        call_id: "c".into(),
        tool: "edit".into(),
        state: "running".into(),
        input: serde_json::json!({"filePath":"Logo.tsx","oldString":"alt=\"Payload Logo\"","newString":"alt=\"Northstar Studio logo\""}),
    }
}
#[test]
fn oracle_is_exact_contract() {
    assert_eq!(PRE.len(), 786);
    assert_eq!(hash(PRE), PRE_HASH);
    let p = oracle(PRE).unwrap();
    assert_eq!(p.len(), 795);
    assert_eq!(hash(&p), POST_HASH);
    assert!(oracle(b"bad").is_err());
}
#[test]
fn pending_reread_reject_without_message_then_idle() {
    let mut g = contract::SyntheticGate::new("r", "s");
    let mut reads = 0;
    let p = g
        .proposal(&permission(), || {
            reads += 1;
            let mut t = part();
            if reads < 3 {
                t.state = "pending".into();
                t.input = serde_json::json!({});
            }
            Ok(vec![t])
        })
        .unwrap();
    assert_eq!(reads, 3);
    assert_eq!(p.validate("r", "a", PRE).unwrap(), oracle(PRE).unwrap());
    assert!(!g.admission_open);
    assert!(!g.cas_allowed());
    assert_eq!(g.reject().unwrap(), br#"{"reply":"reject"}"#);
    g.rejected("a").unwrap();
    assert!(!g.cas_allowed());
    g.confirm_idle();
    assert!(g.cas_allowed());
    assert!(g.reject().is_err());
    assert!(g.rejected("a").is_err());
}
#[test]
fn empty_input_exhaustion_aborts() {
    let mut g = contract::SyntheticGate::new("r", "s");
    assert!(g
        .proposal(&permission(), || {
            let mut t = part();
            t.input = serde_json::json!({});
            Ok(vec![t])
        })
        .is_err());
    assert_eq!(g.control_http, 3);
    assert!(g.abort_requested);
    g.abort().unwrap();
    assert!(g.abort().is_err());
    assert!(!g.cas_allowed());
}
#[test]
fn mismatched_permissions_never_read_or_admit() {
    for mutation in 0..6 {
        let mut p = permission();
        match mutation {
            0 => p.source = "LIVE".into(),
            1 => p.session_id = "other".into(),
            2 => p.permission = "bash".into(),
            3 => p.patterns = vec!["other".into()],
            4 => p.id.clear(),
            _ => p.call_id.clear(),
        };
        let mut g = contract::SyntheticGate::new("r", "s");
        assert!(g
            .proposal(&p, || panic!("GET must be unreachable"))
            .is_err());
        assert!(g.abort_requested);
    }
}
#[test]
fn tool_input_ambiguity_and_wrong_role_are_rejected() {
    for n in 0..10 {
        let mut g = contract::SyntheticGate::new("r", "s");
        let mut t = part();
        match n {
            0 => t.tool = "write".into(),
            1 => t.session_id = "other".into(),
            2 => t.message_id = "other".into(),
            3 => t.source = "LIVE".into(),
            4 => t.input["replaceAll"] = true.into(),
            5 => t.input["filePath"] = "../Logo.tsx".into(),
            6 => t.input["newString"] = "alt=\"wrong\"".into(),
            7 => t.state = "completed".into(),
            _ => (),
        };
        assert!(g
            .proposal(&permission(), || Ok(if n == 8 {
                vec![]
            } else if n == 9 {
                vec![t.clone(), t.clone()]
            } else {
                vec![t.clone()]
            }))
            .is_err());
        assert!(!g.admission_open);
    }
}
#[test]
fn patch_binds_run_attempt_hash_role_and_exact_bytes() {
    let mut g = contract::SyntheticGate::new("r", "s");
    let p = g.proposal(&permission(), || Ok(vec![part()])).unwrap();
    for n in 0..8 {
        let mut q = p.clone();
        match n {
            0 => q.run_id = "other".into(),
            1 => q.attempt_id = "late".into(),
            2 => q.role = "review".into(),
            3 => q.target_id = "other".into(),
            4 => q.expected_sha256 = "bad".into(),
            5 => q.new_literal.push('\n'),
            6 => q.new_literal = q.new_literal.replace("logo", "Logo"),
            _ => q.old_literal = "Payload".into(),
        };
        assert!(q.validate("r", "a", PRE).is_err());
    }
}
#[test]
fn deny_without_permission_event_uses_admission_and_abort() {
    let mut g = contract::SyntheticGate::new("r", "s");
    g.unexpected_tool_error();
    assert!(!g.admission_open);
    assert!(g.abort_requested);
    g.abort().unwrap();
    g.confirm_idle();
    assert!(!g.cas_allowed());
    assert!(g.proposal(&permission(), || panic!()).is_err());
}
#[test]
fn cancelled_or_late_permission_cannot_reopen() {
    let mut g = contract::SyntheticGate::new("r", "s");
    g.stop();
    assert!(g.proposal(&permission(), || panic!()).is_err());
    assert!(g.rejected("unknown").is_err());
}
#[test]
fn simulated_budget_exact_eight_and_ninth_recorded() {
    let mut b = budget::Budget::mock(1).unwrap();
    for i in 0..8 {
        b.reserve(&i.to_string(), 1500).unwrap();
        b.settle(&i.to_string(), Some(1500)).unwrap();
    }
    assert_eq!(b.committed_tokens(), 12000);
    assert!(b.reserve("nine", 1).is_err());
    assert_eq!((b.received, b.admitted, b.denied), (9, 8, 1));
    assert!(b.abort_requested);
}
#[test]
fn unknown_usage_and_duplicate_settlement_keep_reservation() {
    let mut b = budget::Budget::mock(0).unwrap();
    b.reserve("a", 12000).unwrap();
    assert!(b.settle("a", None).is_err());
    assert_eq!(b.committed_tokens(), 12000);
    assert!(b.settle("a", Some(0)).is_err());
    assert!(b.reserve("b", 1).is_err());
}
#[test]
fn cancellation_late_usage_and_excess_usage_are_uncertain() {
    for cancel in [false, true] {
        let mut b = budget::Budget::mock(0).unwrap();
        b.reserve("a", 4).unwrap();
        if cancel {
            b.cancel();
        }
        assert!(b.settle("a", Some(5)).is_err());
        assert_eq!(b.committed_tokens(), if cancel { 4 } else { 5 });
        assert!(b.closed);
    }
}
#[test]
fn repairs_zero_one_and_duplicate_reserve() {
    for limit in 0..=1 {
        let mut b = budget::Budget::mock(limit).unwrap();
        for _ in 0..limit {
            b.repair().unwrap();
        }
        assert!(b.repair().is_err());
    }
    let mut b = budget::Budget::mock(0).unwrap();
    b.reserve("a", 1).unwrap();
    b.settle("a", Some(1)).unwrap();
    assert!(b.reserve("a", 1).is_err());
    assert!(budget::Budget::mock(2).is_err());
}
#[test]
fn concurrent_reserve_is_serialized() {
    let b = Arc::new(Mutex::new(budget::Budget::mock(0).unwrap()));
    let hs: Vec<_> = (0..4)
        .map(|i| {
            let b = b.clone();
            std::thread::spawn(move || b.lock().unwrap().reserve(&i.to_string(), 12000).is_ok())
        })
        .collect();
    assert_eq!(
        hs.into_iter()
            .filter(|h| h.thread().id() != std::thread::current().id())
            .map(|h| h.join().unwrap() as usize)
            .sum::<usize>(),
        1
    );
    let b = b.lock().unwrap();
    assert_eq!(b.received, 4);
    assert_eq!(b.admitted, 1);
    assert!(b.committed_tokens() <= 12000);
}
#[test]
fn loopback_parser_rejects_aliases_and_bypass() {
    for s in [
        "http://localhost:123/model",
        "https://127.0.0.1:123/model",
        "http://127.1:123/model",
        "http://2130706433:123/model",
        "http://[::1]:123/model",
        "http://127.0.0.1:123@evil/model",
        "http://127.0.0.1:123/model?url=x",
        "http://127.0.0.1:0123/model",
        "http://127.0.0.1:0/model",
        "http://8.8.8.8:123/model",
    ] {
        assert!(transport::MockEndpoint::parse(s).is_err(), "{s}");
    }
    assert!(transport::MockEndpoint::parse("http://127.0.0.1:123/model").is_ok());
}
#[tokio::test]
async fn broker_admitted_only_in_process_and_denial_byte_constant() {
    let ledger = Arc::new(Mutex::new(budget::Budget::mock(0).unwrap()));
    let broker = transport::MockBroker::bind(ledger.clone()).unwrap();
    let endpoint = broker.endpoint.clone();
    let h = std::thread::spawn(move || {
        broker.serve_one().unwrap();
        broker.serve_one().unwrap();
    });
    let a = endpoint.post("a", 10).await.unwrap();
    assert_eq!(a.status(), 200);
    assert_eq!(a.bytes().await.unwrap().as_ref(), transport::MOCK_BODY);
    let b = endpoint.post("a", 10).await.unwrap();
    assert_eq!(b.status(), 400);
    assert_eq!(b.headers()["content-type"], "application/json");
    assert!(!b.headers().contains_key("retry-after"));
    assert_eq!(b.bytes().await.unwrap().as_ref(), transport::DENIAL_BODY);
    h.join().unwrap();
    let l = ledger.lock().unwrap();
    assert_eq!((l.received, l.admitted, l.denied), (2, 1, 1));
}
#[tokio::test]
async fn redirect_is_not_followed() {
    use std::net::TcpListener;
    let dest = TcpListener::bind("127.0.0.1:0").unwrap();
    dest.set_nonblocking(true).unwrap();
    let s = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = s.local_addr().unwrap();
    let to = dest.local_addr().unwrap();
    let h = std::thread::spawn(move || {
        let (mut c, _) = s.accept().unwrap();
        c.set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let mut buf = [0; 1024];
        c.read(&mut buf).unwrap();
        write!(c,"HTTP/1.1 302 Found\r\nLocation: http://{to}/model\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
    });
    let e = transport::MockEndpoint::parse(&format!("http://{addr}/model")).unwrap();
    let r = e.post("a", 1).await.unwrap();
    assert_eq!(r.status(), 302);
    h.join().unwrap();
    assert!(dest.accept().is_err());
}
#[test]
fn denial_is_constant_not_retry_regex_input() {
    assert_eq!(transport::DENIAL_BODY,br#"{"error":{"message":"NOVA_ADMISSION_DENIED","type":"invalid_request_error","code":"nova_admission_denied"}}"#);
    assert!(!transport::DENIAL_BODY.iter().any(u8::is_ascii_digit));
    let text = String::from_utf8_lossy(transport::DENIAL_BODY).to_lowercase();
    for keyword in [
        "overloaded",
        "exhausted",
        "rate",
        "unavailable",
        "capacity",
        "server_error",
        "internal_error",
    ] {
        assert!(!text.contains(keyword));
    }
    assert_eq!(
        transport::DENIAL_HEADER,
        b"HTTP/1.1 400 Bad Request\r\nContent-Type: application/json\r\nConnection: close\r\n"
    );
}

#[cfg(windows)]
#[test]
fn same_handle_cas_receipt_and_explicit_recovery() {
    let d = Temp::new();
    let p = d.file();
    let tx = d.tx(&p);
    let r = cas::apply(&p, PRE, &oracle(PRE).unwrap(), &tx, cas::Fault::None).unwrap();
    assert_eq!(r.post_sha256, POST_HASH);
    assert_eq!(tx.status().unwrap(), "COMMITTED_HISTORICAL");
    assert_eq!(fs::read(&p).unwrap(), oracle(PRE).unwrap());
    assert!(cas::apply(&p, PRE, &oracle(PRE).unwrap(), &tx, cas::Fault::None).is_err());
    let reopened = journal::Transaction::reopen(&d.0.join("tx")).unwrap();
    cas::recover(&p, &reopened).unwrap();
    assert_eq!(fs::read(&p).unwrap(), PRE);
    assert!(cas::recover(&p, &reopened).is_err());
}
#[cfg(windows)]
#[test]
fn sharing_allows_read_but_rejects_writers_and_delete() {
    use std::os::windows::fs::OpenOptionsExt;
    let d = Temp::new();
    let p = d.file();
    let locked = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .share_mode(1)
        .open(&p)
        .unwrap();
    assert!(fs::OpenOptions::new()
        .read(true)
        .share_mode(3)
        .open(&p)
        .is_ok());
    assert!(fs::OpenOptions::new()
        .write(true)
        .share_mode(7)
        .open(&p)
        .is_err());
    assert!(fs::remove_file(&p).is_err());
    drop(locked);
    assert!(fs::OpenOptions::new().write(true).open(&p).is_ok());
}
#[cfg(windows)]
#[test]
fn concurrent_writer_blocks_cas_before_intent() {
    let d = Temp::new();
    let p = d.file();
    let tx = d.tx(&p);
    let _writer = fs::OpenOptions::new().write(true).open(&p).unwrap();
    assert!(cas::apply(&p, PRE, &oracle(PRE).unwrap(), &tx, cas::Fault::None).is_err());
    assert!(!tx.root.join("mutation-intent.json").exists());
}
#[cfg(windows)]
#[test]
fn stale_hash_and_same_bytes_replacement_rejected() {
    for replace in [false, true] {
        let d = Temp::new();
        let p = d.file();
        let tx = d.tx(&p);
        if replace {
            let other = d.0.join("new");
            fs::write(&other, PRE).unwrap();
            fs::remove_file(&p).unwrap();
            fs::rename(other, &p).unwrap();
        } else {
            fs::write(&p, b"user edit").unwrap();
        }
        assert!(cas::apply(&p, PRE, &oracle(PRE).unwrap(), &tx, cas::Fault::None).is_err());
        assert!(!tx.root.join("mutation-intent.json").exists());
    }
}
#[cfg(windows)]
#[test]
fn uncertain_faults_never_create_success_receipt() {
    for fault in [
        cas::Fault::ShortWrite,
        cas::Fault::Flush,
        cas::Fault::Receipt,
        cas::Fault::CrashAfterIntent,
    ] {
        let d = Temp::new();
        let p = d.file();
        let tx = d.tx(&p);
        assert!(cas::apply(&p, PRE, &oracle(PRE).unwrap(), &tx, fault)
            .unwrap_err()
            .contains("UNCERTAIN"));
        assert!(!tx.root.join("receipt.json").exists());
        assert_eq!(tx.status().unwrap(), "UNCERTAIN");
        assert!(tx.root.join("mutation-intent.json").exists());
        if fault == cas::Fault::ShortWrite || fault == cas::Fault::CrashAfterIntent {
            assert!(cas::recover(&p, &tx).is_err());
        } else {
            cas::recover(&p, &tx).unwrap();
        }
    }
}
#[cfg(windows)]
#[test]
fn user_edit_after_commit_cannot_be_restored() {
    let d = Temp::new();
    let p = d.file();
    let tx = d.tx(&p);
    cas::apply(&p, PRE, &oracle(PRE).unwrap(), &tx, cas::Fault::None).unwrap();
    fs::write(&p, b"new user data").unwrap();
    assert!(cas::recover(&p, &tx).is_err());
    assert_eq!(fs::read(&p).unwrap(), b"new user data");
}
#[test]
fn journal_create_new_and_corruption_refusal() {
    let d = Temp::new();
    let p = d.0.join("x");
    journal::write_new(&p, b"first").unwrap();
    assert!(journal::write_new(&p, b"overwrite").is_err());
    assert!(journal::write_new(&d.0.join("absent/x"), b"x").is_err());
    assert_eq!(fs::read(p).unwrap(), b"first");
    let tx = journal::Transaction::prepare(&d.0.join("tx"), PRE, &oracle(PRE).unwrap()).unwrap();
    assert!(journal::Transaction::prepare(&tx.root, PRE, PRE).is_err());
    fs::write(tx.root.join("preimage.bin"), b"partial").unwrap();
    assert!(journal::Transaction::reopen(&tx.root).is_err());
}
#[cfg(windows)]
#[test]
fn ntfs_tree_includes_hidden_and_directory_ads_and_refuses_overwrite() {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{SetFileAttributesW, FILE_ATTRIBUTE_HIDDEN};
    let d = Temp::new();
    let root = d.0.join("project");
    fs::create_dir(&root).unwrap();
    let p = root.join("Logo.tsx");
    fs::write(&p, PRE).unwrap();
    let hidden = root.join(".hidden");
    fs::write(&hidden, b"hidden").unwrap();
    let wide: Vec<_> = hidden.as_os_str().encode_wide().chain(Some(0)).collect();
    assert_ne!(
        unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN) },
        0
    );
    fs::write(format!("{}:file-note", p.display()), b"file ADS").unwrap();
    fs::write(format!("{}:dir-note", root.display()), b"dir ADS").unwrap();
    let out = d.0.join("before.json");
    let m = cas::snapshot(&root, &out).unwrap();
    assert!(m.complete, "{:?}", m.errors);
    assert_eq!(m.filesystem, "NTFS");
    assert_eq!(m.entries.len(), 3);
    assert!(m
        .entries
        .iter()
        .any(|e| e.path == Path::new(".hidden") && e.attributes & FILE_ATTRIBUTE_HIDDEN != 0));
    assert_eq!(m.entries.iter().map(|e| e.streams.len()).sum::<usize>(), 2);
    assert!(cas::snapshot(&root, &out).is_err());
    assert!(cas::snapshot(&root, &root.join("manifest.json")).is_err());
    fs::write(root.join("new-file"), b"unexpected").unwrap();
    let a = cas::snapshot(&root, &d.0.join("after.json")).unwrap();
    assert!(cas::verify_tree_change(&m, &a, Path::new("Logo.tsx"), &oracle(PRE).unwrap()).is_err());
}
#[cfg(windows)]
#[test]
fn tree_detects_delete_and_changed_ads() {
    for ads in [false, true] {
        let d = Temp::new();
        let root = d.0.join("project");
        fs::create_dir(&root).unwrap();
        let p = root.join("Logo.tsx");
        fs::write(&p, PRE).unwrap();
        let m = cas::snapshot(&root, &d.0.join("b.json")).unwrap();
        assert!(m.complete);
        if ads {
            fs::write(format!("{}:new", p.display()), b"x").unwrap();
        } else {
            fs::remove_file(&p).unwrap();
        }
        let a = cas::snapshot(&root, &d.0.join("a.json")).unwrap();
        assert!(cas::verify_tree_change(&m, &a, Path::new("Logo.tsx"), PRE).is_err());
    }
}
#[cfg(windows)]
#[test]
fn hardlinks_are_not_empty_success() {
    let d = Temp::new();
    let root = d.0.join("project");
    fs::create_dir(&root).unwrap();
    let p = root.join("Logo.tsx");
    fs::write(&p, PRE).unwrap();
    fs::hard_link(&p, root.join("alias")).unwrap();
    let m = cas::snapshot(&root, &d.0.join("m.json")).unwrap();
    assert!(!m.complete);
    assert_eq!(m.status, "NOT ENUMERATED");
    assert!(!m.errors.is_empty());
}
#[cfg(windows)]
#[test]
fn junction_is_rejected_without_following_it() {
    let d = Temp::new();
    let root = d.0.join("project");
    let outside = d.0.join("outside");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("untouched"), b"outside sentinel").unwrap();
    let link = root.join("junction");
    // Fixed system utility only to create a junction in this test's owned UUID directory.
    // No Agent, shell command text, user-controlled command, probe or policy change.
    let system = std::env::var_os("SystemRoot").unwrap();
    let output = std::process::Command::new(PathBuf::from(system).join("System32").join("cmd.exe"))
        .env_clear()
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "junction setup failed: {:?}",
        output
    );
    let m = cas::snapshot(&root, &d.0.join("reparse.json")).unwrap();
    assert!(!m.complete);
    assert!(m.errors.iter().any(|e| e.contains("reparse")));
    assert_eq!(
        fs::read(outside.join("untouched")).unwrap(),
        b"outside sentinel"
    );
    fs::remove_dir(link).unwrap();
}
#[test]
fn admission_closed_on_deny_or_reject_blocks_next_model_attempt() {
    for error_without_event in [true, false] {
        let mut g = contract::SyntheticGate::new("r", "s");
        let mut b = budget::Budget::mock(0).unwrap();
        b.reserve("initial", 100).unwrap();
        b.settle("initial", Some(1)).unwrap();
        if error_without_event {
            g.unexpected_tool_error();
        } else {
            g.proposal(&permission(), || Ok(vec![part()])).unwrap();
            g.reject().unwrap();
        }
        b.closed = !g.admission_open;
        assert!(b.reserve("subsequent", 100).is_err());
        assert_eq!((b.received, b.admitted, b.denied), (2, 1, 1));
        assert!(b.abort_requested);
        // A denied configuration does not supply a permission event or prove Agent idle.
        if error_without_event {
            assert!(g.abort_requested);
            assert!(!g.reject_sent);
            g.abort().unwrap();
            assert!(!g.cas_allowed());
        }
    }
}
#[cfg(windows)]
#[test]
fn receipt_create_new_failure_remains_uncertain() {
    let d = Temp::new();
    let p = d.file();
    let tx = d.tx(&p);
    journal::write_new(&tx.root.join("receipt.json"), b"existing evidence").unwrap();
    assert!(
        cas::apply(&p, PRE, &oracle(PRE).unwrap(), &tx, cas::Fault::None)
            .unwrap_err()
            .contains("UNCERTAIN")
    );
    assert_eq!(
        fs::read(tx.root.join("receipt.json")).unwrap(),
        b"existing evidence"
    );
}
#[cfg(windows)]
#[test]
fn fixed_mock_pipeline() {
    let result = mock_run().unwrap();
    let root = PathBuf::from(result["root"].as_str().unwrap());
    assert_eq!(result["post_sha256"], POST_HASH);
    assert_eq!(result["M0_B"], "BLOCKED");
    fs::remove_dir_all(root).unwrap();
}
