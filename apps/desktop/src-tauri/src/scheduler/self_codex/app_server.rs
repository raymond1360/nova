//! Fixed three-turn adapter. No production capability issuer; never an arbitrary RPC launcher.
#[cfg(windows)]
use super::super::cli_adapter::AppServerChannel;
use super::*;
use std::collections::BTreeSet;

// Constructors remain test-only until actual tool permissions and quiescence have been admitted.
pub(crate) struct TransportBinding {
    spec_sha256: String,
    plan_sha256: String,
    root: PathBuf,
    immutable: String,
    before: Vec<u8>,
    after: Vec<u8>,
    #[cfg(test)]
    case: String,
}
fn spec_hash(s: &Spec) -> Result<String> {
    Ok(hash(&bytes(
        &json!({"exe":s.exe,"sha256":s.exe_sha256,"role":s.role,"profile":s.profile,"cwd":s.cwd,"argv":s.argv,"env":s.env,"session":s.session}),
    )?))
}
impl TransportBinding {
    pub(crate) fn check(&self, spec: &Spec) -> Result<()> {
        if spec.profile != "SELF2_APP_SERVER"
            || spec.cwd != self.root
            || spec_hash(spec)? != self.spec_sha256
        {
            return Err("APP_SERVER_BINDING_REJECTED");
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn test_fault(&self) -> Option<&'static str> {
        if self.case == "partial" {
            Some("duplex-half")
        } else {
            None
        }
    }
}
fn modify(s: &Store, f: impl FnOnce(&mut Value) -> Result<()>) -> Result<()> {
    let (rev, mut state) = h::load(s)?;
    let value = state
        .self2
        .as_mut()
        .ok_or("SELF2_UNINITIALIZED")?
        .app_server
        .as_mut()
        .ok_or("APP_SERVER_STATE_MISSING")?;
    f(value)?;
    h::save(s, rev, &state)
}
fn app_state(s: &Store) -> Result<Value> {
    journal(s)?.app_server.ok_or("APP_SERVER_STATE_MISSING")
}
fn launch_attempt(spec: &Spec) -> Attempt {
    Attempt {
        attempt_id: uuid::Uuid::new_v4().to_string(),
        role: RelayRole::Develop,
        state: "LAUNCHING".into(),
        state_version: 0,
        input_identity: spec.argv_sha256(),
        parent_attempt_id: None,
        profile_id: "SELF2_APP_SERVER".into(),
        exe_sha256: spec.exe_sha256.clone(),
        argv_sha256: spec.argv_sha256(),
        env_names_sha256: spec.env_names_sha256(),
        started_utc: Some(chrono::Utc::now().to_rfc3339()),
        ended_utc: None,
        pid: None,
        process_creation_utc: None,
        exit_code: None,
        terminal: None,
        reclaim: None,
        session_id: None,
        stdout_bytes: None,
        stdout_sha256: None,
        stderr_bytes: None,
        stderr_sha256: None,
        result_sha256: None,
        model_claim_json: None,
        usage_json: None,
    }
}
#[cfg(windows)]
struct Adapter<'a> {
    store: &'a Store,
    binding: &'a TransportBinding,
    io: AppServerChannel,
    next_id: u64,
    round: usize,
    developer: Option<String>,
    reviewer: Option<String>,
    review_session: Option<String>,
    review_packet: Option<String>,
    pid: u32,
    created: String,
    end: Instant,
}
#[cfg(windows)]
impl Adapter<'_> {
    fn deadline(&self) -> Instant {
        #[cfg(test)]
        if self.binding.case == "timeout" || self.binding.case == "cancel" {
            return self.end.min(Instant::now() + Duration::from_millis(500));
        }
        self.end.min(Instant::now() + Duration::from_secs(60))
    }
    fn reserve_send(&mut self, method: &str, params: &Value, id: Option<u64>) -> Result<Vec<u8>> {
        if ![
            "initialize",
            "initialized",
            "config/read",
            "thread/start",
            "thread/resume",
            "thread/loaded/list",
            "turn/start",
        ]
        .contains(&method)
        {
            return Err("APP_SERVER_METHOD_REJECTED");
        }
        let value = if let Some(id) = id {
            json!({"id":id,"method":method,"params":params})
        } else {
            json!({"method":method})
        };
        let mut wire = bytes(&value)?;
        wire.push(b'\n');
        if wire.len() > INPUT_CAP {
            return Err("APP_SERVER_INPUT_BOUND");
        }
        let digest = hash(&wire);
        let expected = journal(self.store)?.plan_sha256;
        modify(self.store, |a| {
            if a["status"] != "ACTIVE" || a["plan_sha256"] != expected {
                return Err("APP_SERVER_RECONCILE_REQUIRED");
            }
            let rows = a["sends"]
                .as_array_mut()
                .ok_or("APP_SERVER_STATE_REJECTED")?;
            if rows.len() >= 16 || rows.last().is_some_and(|r| r["state"] != "COMPLETE") {
                return Err("APP_SERVER_RECONCILE_REQUIRED");
            }
            if method == "turn/start"
                && rows.iter().filter(|v| v["method"] == "turn/start").count() >= 3
            {
                return Err("APP_SERVER_TURN_LIMIT");
            }
            rows.push(json!({"id":id,"method":method,"request_sha256":digest,"state":"STARTED","round":self.round,"thread":params["threadId"]}));
            Ok(())
        })?;
        Ok(wire)
    }
    fn sent(&mut self, response: &Value) -> Result<()> {
        modify(self.store, |a| {
            let row = a["sends"]
                .as_array_mut()
                .and_then(|r| r.last_mut())
                .ok_or("APP_SERVER_STATE_REJECTED")?;
            if row["state"] != "STARTED" {
                return Err("APP_SERVER_RECONCILE_REQUIRED");
            }
            row["state"] = json!("COMPLETE");
            row["response_sha256"] = json!(hash(&bytes(response)?));
            Ok(())
        })
    }
    fn rpc(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let wire = self.reserve_send(method, &params, Some(id))?;
        let end = self.deadline();
        self.io.send(&wire, end)?;
        let v: Value = relay_json(&self.io.receive(end)?, 1024 * 1024)?;
        if v["id"] != id
            || v.get("error").is_some()
            || v.get("method").is_some()
            || v.get("result").is_none()
        {
            return Err("APP_SERVER_RPC_ORDER_REJECTED");
        }
        self.sent(&v)?;
        Ok(v["result"].clone())
    }
    fn ready(&mut self) -> Result<()> {
        self.rpc("initialize",json!({"clientInfo":{"name":"nova_fixed_canary","version":"1"},"capabilities":{"experimentalApi":true}}))?;
        let wire = self.reserve_send("initialized", &Value::Null, None)?;
        self.io.send(&wire, self.deadline())?;
        self.sent(&json!({"notification_written":true}))?;
        self.check_config()
    }
    fn check_config(&mut self) -> Result<()> {
        let p = journal(self.store)?.plan;
        let r = self.rpc(
            "config/read",
            json!({"includeLayers":false,"cwd":p.project_root}),
        )?;
        let c = &r["config"];
        if c["model_provider"] != "openai"
            || c["model"] != p.model
            || c["project_doc_max_bytes"] != 0
            || c["mcp_servers"].as_object().is_none_or(|v| !v.is_empty())
        {
            return Err("APP_SERVER_CONFIGURATION_REJECTED");
        }
        for name in [
            "plugins",
            "hooks",
            "plugin_hooks",
            "multi_agent",
            "multi_agent_v2",
        ] {
            if c["features"][name] != false {
                return Err("APP_SERVER_CONFIGURATION_REJECTED");
            }
        }
        // This is a finite observed projection, NOT a full tool registry or immutable cloud proof.
        Ok(())
    }
    fn thread(&mut self, review: bool, resume: bool) -> Result<String> {
        let p = journal(self.store)?.plan;
        let expected = self.reviewer.clone();
        let mut params = json!({"model":p.model,"modelProvider":"openai","cwd":p.project_root,
            "runtimeWorkspaceRoots":[],"approvalPolicy":"never","approvalsReviewer":"user","sandbox":if review {"read-only"}else{"workspace-write"},
            "config":{"windows.sandbox":"unelevated","sandbox_workspace_write.network_access":false,
                "features.plugins":false,"features.hooks":false,"features.multi_agent":false,"project_doc_max_bytes":0}});
        if resume {
            let loaded = self.rpc("thread/loaded/list", json!({"limit":3}))?;
            let expected_ids = BTreeSet::from([
                self.developer
                    .clone()
                    .ok_or("APP_SERVER_WARM_THREAD_REQUIRED")?,
                expected.clone().ok_or("APP_SERVER_WARM_THREAD_REQUIRED")?,
            ]);
            let ids = loaded["data"]
                .as_array()
                .ok_or("APP_SERVER_WARM_THREAD_REQUIRED")?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_string)
                        .ok_or("APP_SERVER_WARM_THREAD_REQUIRED")
                })
                .collect::<Result<BTreeSet<_>>>()?;
            if ids != expected_ids || loaded["nextCursor"] != Value::Null {
                return Err("APP_SERVER_WARM_THREAD_REQUIRED");
            }
        }
        let method = if resume {
            params["threadId"] = json!(expected
                .as_deref()
                .ok_or("APP_SERVER_WARM_THREAD_REQUIRED")?);
            params["excludeTurns"] = json!(true);
            "thread/resume"
        } else {
            params["dynamicTools"] = json!([]);
            params["ephemeral"] = json!(false);
            "thread/start"
        };
        let r = self.rpc(method, params)?;
        let id = r["thread"]["id"].as_str().ok_or("SELF2_UUID_MISSING")?;
        let session = r["thread"]["sessionId"]
            .as_str()
            .ok_or("SELF2_UUID_MISSING")?;
        if !uuid_ok(id) || !uuid_ok(session) || r["thread"]["status"]["type"] != "idle" {
            return Err("APP_SERVER_THREAD_REJECTED");
        }
        let sandbox = if review {
            json!({"type":"readOnly","networkAccess":false})
        } else {
            json!({"type":"workspaceWrite","writableRoots":[],"networkAccess":false,"excludeTmpdirEnvVar":true,"excludeSlashTmp":true})
        };
        if r["model"] != p.model
            || r["modelProvider"] != "openai"
            || r["cwd"]
                .as_str()
                .is_none_or(|v| Path::new(v) != p.project_root)
            || r["runtimeWorkspaceRoots"] != json!([])
            || r["approvalPolicy"] != "never"
            || r["approvalsReviewer"] != "user"
            || r.get("activePermissionProfile") != Some(&Value::Null)
            || r["sandbox"] != sandbox
            || r["reasoningEffort"] != "low"
            || r["serviceTier"] != "default"
            || r["disabledPluginIds"] != json!([])
        {
            return Err("APP_SERVER_THREAD_PERMISSION_REJECTED");
        }
        if resume {
            if expected.as_deref() != Some(id)
                || self.review_session.as_deref() != Some(session)
                || !self.io.only_server()
            {
                return Err("APP_SERVER_WARM_THREAD_REQUIRED");
            }
        } else if review {
            if self.developer.as_deref() == Some(id) {
                return Err("SELF_ROLE_COLLISION");
            }
            self.reviewer = Some(id.into());
            self.review_session = Some(session.into());
        } else {
            self.developer = Some(id.into());
        }
        Ok(id.into())
    }
    fn material(&self, input: &Value) -> Result<()> {
        let j = journal(self.store)?;
        let p = j.plan;
        if j.plan_sha256 != self.binding.plan_sha256
            || p.project_root != self.binding.root
            || canary_tree(&p.project_root)? != self.binding.immutable
            || input["head_sha"] != p.input.head_sha
            || input["stage"] != p.input.stage
            || input["package"] != p.input.package
        {
            return Err("APP_SERVER_MATERIAL_DRIFT");
        }
        let (raw, _) = super::super::store::read_file(&p.project_root.join("a.txt"), 131072)?;
        if raw
            != if self.round == 0 {
                &self.binding.before
            } else {
                &self.binding.after
            }
            .as_slice()
        {
            return Err("APP_SERVER_FILE_DRIFT");
        }
        if self.round > 0 {
            let state = h::load(self.store)?.1;
            let a = state.active.ok_or("SELF_NO_PACKAGE")?;
            h::verify_packet(self.store, &a)?;
            if input["packet_sha256"] != a.packet_sha256
                || self
                    .review_packet
                    .as_ref()
                    .is_some_and(|r| r != &a.packet_sha256)
            {
                return Err("APP_SERVER_PACKET_MISMATCH");
            }
        }
        Ok(())
    }
    fn turn(&mut self, input: &[u8], spec: &Spec) -> Result<Vec<u8>> {
        if self.round >= 3 {
            return Err("APP_SERVER_TURN_LIMIT");
        }
        let plan = journal(self.store)?.plan;
        let role = if self.round == 0 {
            Role::Develop
        } else {
            Role::Review
        };
        if spec.exe != plan.cli
            || spec.exe_sha256 != plan.cli_sha256
            || spec.cwd != plan.project_root
            || spec.profile != role.profile()
        {
            return Err("APP_SERVER_BINDING_REJECTED");
        }
        let mut v: Value = relay_json(input, INPUT_CAP)?;
        let review = self.round > 0;
        #[cfg(test)]
        if self.binding.case == "packet" && review {
            v["packet_sha256"] = json!("0".repeat(64));
        }
        if spec.role
            != if review {
                RelayRole::Review
            } else {
                RelayRole::Develop
            }
            || (self.round < 2 && !spec.session.is_empty())
            || (self.round == 2 && self.reviewer.as_deref() != Some(spec.session.as_str()))
        {
            return Err("APP_SERVER_ROLE_REJECTED");
        }
        self.material(&v)?;
        if self.round > 0 && !self.io.only_server() {
            return Err("APP_SERVER_D1_NOT_QUIESCENT");
        }
        self.check_config()?;
        let id = self.thread(review, self.round == 2)?;
        self.material(&v)?; // Thread creation/resume is not a zero-side-effect operation.
        if review {
            self.review_packet = Some(
                v["packet_sha256"]
                    .as_str()
                    .ok_or("APP_SERVER_PACKET_MISMATCH")?
                    .into(),
            );
        }
        let before = super::super::store::read_file(&self.binding.root.join("a.txt"), 131072)?;
        let request_id = self.next_id;
        self.next_id += 1;
        let params = json!({"threadId":id,"input":[{"type":"text","text":std::str::from_utf8(input).map_err(|_|"ENCODING")?}],
            "cwd":self.binding.root,"approvalPolicy":"never","outputSchema":output_schema()});
        let wire = self.reserve_send("turn/start", &params, Some(request_id))?;
        let end = self.deadline();
        self.io.send(&wire, end)?;
        let ack: Value = relay_json(&self.io.receive(end)?, 1024 * 1024)?;
        if ack["id"] != request_id
            || ack.get("error").is_some()
            || ack["result"]["turn"]["status"] != "inProgress"
        {
            return Err("APP_SERVER_TURN_ACK_REJECTED");
        }
        let tid = ack["result"]["turn"]["id"]
            .as_str()
            .ok_or("APP_SERVER_TURN_ACK_REJECTED")?
            .to_string();
        let mut active = BTreeSet::new();
        let mut seen = BTreeSet::new();
        let mut message = None;
        let mut started = false;
        loop {
            let event: Value = relay_json(&self.io.receive(end)?, 1024 * 1024)?;
            if event.get("id").is_some() || event["params"]["threadId"] != id {
                return Err("APP_SERVER_EVENT_BINDING_REJECTED");
            }
            let p = &event["params"];
            match event["method"].as_str() {
                Some("turn/started") if !started && p["turn"]["id"] == tid => started = true,
                Some("item/started") if started && p["turnId"] == tid => {
                    let key = p["item"]["id"].as_str().ok_or("APP_SERVER_ITEM_REJECTED")?;
                    if ![
                        "agentMessage",
                        "reasoning",
                        "commandExecution",
                        "fileChange",
                    ]
                    .contains(&p["item"]["type"].as_str().unwrap_or(""))
                        || (review && p["item"]["type"] == "fileChange")
                        || !seen.insert(key.to_string())
                        || !active.insert(key.to_string())
                    {
                        return Err("APP_SERVER_ITEM_REJECTED");
                    }
                }
                Some("item/completed") if started && p["turnId"] == tid => {
                    let key = p["item"]["id"].as_str().ok_or("APP_SERVER_ITEM_REJECTED")?;
                    if !active.remove(key) {
                        return Err("APP_SERVER_ITEM_ORDER_REJECTED");
                    }
                    match p["item"]["type"].as_str() {
                        Some("agentMessage") if message.is_none() => {
                            message = Some(
                                p["item"]["text"]
                                    .as_str()
                                    .ok_or("SELF2_RESULT_MISSING")?
                                    .to_string(),
                            )
                        }
                        Some("reasoning") => {}
                        Some("fileChange") if !review => {}
                        Some("commandExecution")
                            if p["item"]["status"] == "completed" && p["item"]["exitCode"] == 0 => {
                        }
                        _ => return Err("APP_SERVER_ITEM_REJECTED"),
                    }
                }
                Some("turn/completed")
                    if started && p["turn"]["id"] == tid && p["turn"]["status"] == "completed" =>
                {
                    let (flags, count, exited) = self.io.job_sample();
                    modify(self.store, |a| {
                        a["quiescence_sample"] = json!({"flags":flags,"active_processes":count,"root_exited":exited,"active_items":active.len(),"round":self.round});
                        Ok(())
                    })?;
                    #[cfg(test)]
                    modify(self.store, |a| {
                        a["synthetic_job_members"] = json!(self.io.job_names());
                        Ok(())
                    })?;
                    if !active.is_empty() || !self.io.only_server() {
                        return Err("APP_SERVER_D1_NOT_QUIESCENT");
                    }
                    self.sent(&event)?;
                    break;
                }
                _ => return Err("APP_SERVER_EVENT_ORDER_REJECTED"),
            }
        }
        if canary_tree(&self.binding.root)? != self.binding.immutable {
            return Err("APP_SERVER_MATERIAL_DRIFT");
        }
        let after = super::super::store::read_file(&self.binding.root.join("a.txt"), 131072)?;
        if after.0 != self.binding.after || (review && after != before) {
            return Err("APP_SERVER_FILE_DRIFT");
        }
        let mut out = vec![];
        for e in [
            json!({"type":"thread.started","thread_id":id}),
            json!({"type":"turn.started"}),
            json!({"type":"item.completed","item":{"type":"agent_message","text":message.ok_or("SELF2_RESULT_MISSING")?}}),
            json!({"type":"turn.completed","usage":{"source":"APP_SERVER_EVENT; BILLING_NOT_OBSERVED"}}),
        ] {
            out.extend(bytes(&e)?);
            out.push(b'\n')
        }
        self.round += 1;
        Ok(out)
    }
}
#[cfg(windows)]
impl sealed::Sealed for Adapter<'_> {}
#[cfg(windows)]
impl Driver for Adapter<'_> {
    fn app_server_transport(&self) -> bool {
        true
    }
    fn invoke(
        &mut self,
        s: &Spec,
        b: &[u8],
        permit: &LaunchPermit,
        _: Instant,
        running: &mut dyn FnMut(u32, String) -> Result<()>,
    ) -> Execution {
        let mut e = Execution::failed("APP_SERVER_STOPPED");
        e.created = false;
        e.pid = Some(self.pid);
        e.process_creation_utc = Some(self.created.clone());
        e.reclaim = "ACTIVE_JOB".into();
        if !permit.claim_once() || running(self.pid, self.created.clone()).is_err() {
            return e;
        }
        match self.turn(b, s) {
            Ok(out) => {
                e.terminal = "TURN_COMPLETED".into();
                e.stdout = out
            }
            Err(code) => {
                e.terminal = code.into();
                e.unknowns.push("APP_SERVER_NO_REPLAY".into())
            }
        }
        e
    }
}
// Offline driver capability is issued only by synthetic tests below. No production run/approval path.
#[cfg(windows)]
fn execute(s: &Store, binding: &TransportBinding, spec: Spec) -> Result<Value> {
    binding.check(&spec)?;
    let (rev, mut state) = h::load(s)?;
    let j = state.self2.as_mut().ok_or("SELF2_UNINITIALIZED")?;
    if j.app_server.is_some()
        || !j.records.is_empty()
        || j.canary.is_some()
        || !j.plan.commands.is_empty()
        || j.plan_sha256 != binding.plan_sha256
        || state.stage.developer.is_some()
        || state.stage.reviewer.is_some()
    {
        return Err("APP_SERVER_ALREADY_RESERVED");
    }
    j.app_server = Some(
        json!({"schema":"nova.self2.app-server/1","status":"STARTED","plan_sha256":j.plan_sha256,"spec_sha256":binding.spec_sha256,
        "sends":[],"turns_limit":3,"real_permissions":"NOT VERIFIED; SYNTHETIC DRIVER ONLY"}),
    );
    let permit = s.self2_launch(
        rev,
        &serde_json::to_value(&state).map_err(|_| "ENCODING")?,
        launch_attempt(&spec),
    )?;
    let end = Instant::now() + Duration::from_secs(90);
    let mut pid = 0;
    let mut created = String::new();
    let io = match AppServerChannel::open(binding, &spec, &permit, end, &mut |p, c| {
        pid = p;
        created = c.clone();
        modify(s, |a| {
            a["pid"] = json!(p);
            a["creation"] = json!(c);
            a["status"] = json!("ACTIVE");
            Ok(())
        })
    }) {
        Ok(io) => io,
        Err(e) => {
            modify(s, |a| {
                a["status"] = json!("UNKNOWN");
                a["error"] = json!(e);
                Ok(())
            })?;
            return Err(e);
        }
    };
    #[cfg(test)]
    if binding.case == "cancel" {
        let cancel = io.cancel_handle();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            cancel.store(true, std::sync::atomic::Ordering::Release);
        });
    }
    let mut driver = Adapter {
        store: s,
        binding,
        io,
        next_id: 1,
        round: 0,
        developer: None,
        reviewer: None,
        review_session: None,
        review_packet: None,
        pid,
        created,
        end,
    };
    let result = (|| {
        driver.ready()?;
        let p = journal(s)?.plan;
        run(s, &mut driver, &p.cli, &p.cli_sha256, false)?;
        let st = h::load(s)?.1;
        let active = st.active.as_ref().ok_or("SELF_NO_PACKAGE")?;
        let review = active
            .review
            .as_ref()
            .ok_or("SELF2_CANARY_REVIEW_REQUIRED")?
            .clone();
        if review.report.claimed_conclusion != Conclusion::Pass {
            return Err("SELF2_CANARY_REVIEW_NOT_PASS");
        }
        let input = bytes(
            &json!({"schema":"nova.self2.input/1","role":"REVIEW","stage":p.input.stage,"package":p.input.package,
            "head_sha":p.input.head_sha,"packet_sha256":active.packet_sha256,"material":active.packet,"instruction":"Recheck SAME packet; keep R1 conclusion; read-only.","result_contract":output_schema()}),
        )?;
        let old_report = bytes(&review)?;
        let role_spec = Spec {
            role: RelayRole::Review,
            profile: Role::Review.profile(),
            exe: p.cli.clone(),
            exe_sha256: p.cli_sha256.clone(),
            argv: vec![],
            env: BTreeMap::new(),
            cwd: p.project_root.clone(),
            session: review.reviewer_uuid.clone(),
        };
        let dir = h::store_path(&p.project_root)?.join("backups/self2");
        let e = invoke(
            s,
            &mut driver,
            Role::Review,
            1,
            role_spec,
            input,
            Some(review.reviewer_uuid.clone()),
            &dir,
        )?;
        let (_, r) = accept_saved(
            s,
            &e,
            Role::Review,
            Some(&review.reviewer_uuid),
            &p,
            &active.packet_sha256,
        )?;
        if r.claimed_conclusion != Conclusion::Pass
            || old_report != bytes(&h::load(s)?.1.active.unwrap().review.unwrap())?
        {
            return Err("SELF2_CANARY_REVIEW_CHANGED");
        }
        if driver.round != 3 {
            return Err("APP_SERVER_TURN_LIMIT");
        }
        #[cfg(test)]
        {
            let before = bytes(&h::load(s)?)?;
            assert_eq!(driver.turn(b"", &spec), Err("APP_SERVER_TURN_LIMIT"));
            assert_eq!(before, bytes(&h::load(s)?)?);
        }
        Ok(())
    })();
    let cleanup = driver.io.finish();
    let close_ok =
        cleanup.natural && cleanup.exit_code == Some(0) && cleanup.reclaim == "CONFIRMED";
    modify(s, |a| {
        a["status"] = json!(if result.is_ok() && close_ok {
            "COMPLETE"
        } else {
            "UNKNOWN"
        });
        a["rounds_completed"] = json!(driver.round);
        a["error"] = json!(result.err().or(if close_ok {
            None
        } else {
            Some("APP_SERVER_CLOSE_NOT_VERIFIED")
        }));
        a["process_exit"] = json!({"created":cleanup.created,"pid":cleanup.pid,"creation":cleanup.process_creation_utc,
            "natural":cleanup.natural,"exit_code":cleanup.exit_code,"reclaim":cleanup.reclaim,
            "stdout_bytes":cleanup.stdout.len(),"stdout_sha256":hash(&cleanup.stdout),"stderr_bytes":cleanup.stderr.len(),"stderr_sha256":hash(&cleanup.stderr)});
        a["cleanup"] = serde_json::to_value(&cleanup.cleanup).map_err(|_| "ENCODING")?;
        if let Some(row) = a["sends"].as_array_mut().and_then(|r| r.last_mut()) {
            if row["state"] == "STARTED" {
                row["state"] = json!("UNKNOWN")
            }
        }
        Ok(())
    })?;
    result?;
    if !close_ok {
        return Err("APP_SERVER_CLOSE_NOT_VERIFIED");
    }
    Ok(
        json!({"schema":"nova.self2.app-server-summary/1","server":app_state(s)?,"self1":h::summary(s)?,
        "real_tool_permissions":"NOT RUN","extra_tool_closure":"NOT VERIFIED","background_quiescence":"SAMPLED_JOB_AND_EVENT_FACTS_ONLY",
        "native_admission":"CLOSED","user_acceptance":"PENDING"}),
    )
}
#[cfg(test)]
pub(crate) mod tests;
