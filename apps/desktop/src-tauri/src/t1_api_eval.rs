//! Nova-only SYNTHETIC harness. No Agent, credentials, application state or live transport.
pub mod budget;
pub mod cas;
pub mod contract;
pub mod journal;
pub mod transport;
pub mod opencode_gate;
pub mod opencode_paths;
pub mod opencode_protocol;
pub mod opencode_broker;
pub mod opencode_probe;
#[cfg(test)]
mod opencode_probe_tests;

use ring::digest::{digest, SHA256};
use std::path::Path;

pub const PRE: &[u8] = include_bytes!("../../../../tools/t1-api-eval/fixtures/Logo.tsx");
pub const PRE_HASH: &str = "0821ae3bbb7e9d2085e9ae9031b4b7744bc191c84a6b7dda6b618b7c1b6ed58a";
pub const POST_HASH: &str = "68fad18b5f5e14a50b0024e050d9b4d63612091a3d126de11326b3f19f909518";
pub type Result<T> = std::result::Result<T, String>;
pub fn hash(bytes: &[u8]) -> String {
    digest(&SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn oracle(pre: &[u8]) -> Result<Vec<u8>> {
    if hash(pre) != PRE_HASH {
        return Err("preimage identity mismatch".into());
    }
    let old = b"alt=\"Payload Logo\"";
    let matches: Vec<_> = pre
        .windows(old.len())
        .enumerate()
        .filter(|(_, b)| *b == old)
        .map(|(i, _)| i)
        .collect();
    if matches.len() != 1 {
        return Err("ambiguous oracle".into());
    }
    let at = matches[0];
    Ok([
        &pre[..at],
        b"alt=\"Northstar Studio logo\"",
        &pre[at + old.len()..],
    ]
    .concat())
}

/// Fixed mock run only. The runner never accepts a target/project/network argument.
pub fn mock_run() -> Result<serde_json::Value> {
    let root = std::env::temp_dir().join(format!("nova-t1-m0a-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).map_err(|e| e.to_string())?;
    let project = root.join("project");
    std::fs::create_dir(&project).map_err(|e| e.to_string())?;
    let target = project.join("Logo.tsx");
    journal::write_new(&target, PRE)?;
    let evidence = root.join("evidence");
    std::fs::create_dir(&evidence).map_err(|e| e.to_string())?;
    let ledger = std::sync::Arc::new(std::sync::Mutex::new(budget::Budget::mock(0)?));
    let broker = transport::MockBroker::bind(ledger.clone())?;
    let endpoint = broker.endpoint.clone();
    let server = std::thread::spawn(move || broker.serve_one());
    let response = tokio::runtime::Runtime::new()
        .map_err(|e| e.to_string())?
        .block_on(async {
            let r = endpoint.post("synthetic-developer", 1500).await?;
            if r.status() != 200 {
                return Err("mock admission failed".into());
            }
            r.bytes()
                .await
                .map(|b| b.to_vec())
                .map_err(|e| e.to_string())
        });
    server.join().map_err(|_| "mock server panic")??;
    journal::write_new(&evidence.join("mock-response.json"), &response?)?;
    let mut gate = contract::SyntheticGate::new(
        "SYNTHETIC-run",
        "SYNTHETIC-session",
        &project,
        ledger.clone(),
    )?;
    let permission = contract::Permission {
        source: "SYNTHETIC".into(),
        id: "edit-attempt".into(),
        session_id: "SYNTHETIC-session".into(),
        permission: "edit".into(),
        patterns: vec!["Logo.tsx".into()],
        metadata: serde_json::json!({"diff":"not a patch source"}),
        message_id: "message".into(),
        call_id: "call".into(),
    };
    let mut reads = 0;
    let patch=gate.proposal(&permission,|| {
        reads+=1;
        Ok(vec![contract::ToolPart {source:"SYNTHETIC".into(),session_id:permission.session_id.clone(),message_id:permission.message_id.clone(),call_id:permission.call_id.clone(),tool:"edit".into(),
            state:if reads==1 {"pending"}else{"running"}.into(),input:if reads==1{serde_json::json!({})}else{serde_json::json!({"filePath":std::fs::canonicalize(&target).map_err(|e|e.to_string())?,"oldString":"alt=\"Payload Logo\"","newString":"alt=\"Northstar Studio logo\""})} }])
    })?;
    journal::write_new(
        &evidence.join("synthetic-permission.json"),
        &serde_json::to_vec_pretty(&permission).unwrap(),
    )?;
    journal::write_new(&evidence.join("reject-body.json"), gate.reject()?)?;
    gate.rejected("edit-attempt")?;
    gate.confirm_idle();
    if !gate.cas_allowed() {
        return Err("SYNTHETIC stop acknowledgement missing".into());
    }
    journal::write_new(
        &evidence.join("synthetic-patch.json"),
        &serde_json::to_vec_pretty(&patch).unwrap(),
    )?;
    journal::write_new(
        &evidence.join("simulated-budget.json"),
        &serde_json::to_vec_pretty(&*ledger.lock().map_err(|_| "ledger poison")?).unwrap(),
    )?;
    let before = cas::snapshot(&project, &evidence.join("before.json"))?;
    if !before.complete {
        return Err("pre-CAS tree NOT ENUMERATED; mutation refused".into());
    }
    let post = patch.validate("SYNTHETIC-run", "edit-attempt", PRE)?;
    let tx = journal::Transaction::prepare(&evidence.join("transaction"), PRE, &post)?;
    cas::reserve_identity(&target, gate.fixture_root(), &tx)?;
    let receipt = cas::apply(
        &target,
        gate.fixture_root(),
        PRE,
        &post,
        &tx,
        cas::Fault::None,
    )?;
    let after = cas::snapshot(&project, &evidence.join("after.json"))?;
    cas::verify_tree_change(&before, &after, Path::new("Logo.tsx"), &post)?;
    journal::write_new(
        &evidence.join("mock-result.json"),
        &serde_json::to_vec_pretty(&receipt).unwrap(),
    )?;
    Ok(
        serde_json::json!({"scope":"Nova-side SYNTHETIC mock only", "root":root,
       "post_sha256":receipt.post_sha256, "M0_B":"BLOCKED", "live":"NOT AUTHORIZED"}),
    )
}

#[cfg(test)]
mod tests;
