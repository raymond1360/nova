//! Non-default DEBUG ONLY desktop validation entry. Never part of normal release.
use crate::{
    credential::CredentialStore,
    db::Database,
    error::{NovaError, NovaResult},
    models::*,
    AppState,
};
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::Duration,
};
use tauri::{ipc::Invoke, Manager};
#[derive(Clone, Debug)]
struct Config {
    root: PathBuf,
    data: PathBuf,
    fake: PathBuf,
    git: PathBuf,
    fake_sha: String,
}
static CONFIG: OnceLock<Config> = OnceLock::new();
static DELAY: Mutex<Option<Value>> = Mutex::new(None);
pub(crate) fn require_active() {
    assert!(CONFIG.get().is_some(),"Use nova_wp2a_validation --root <fresh absolute directory>; normal entry forbidden with validation feature");
}
fn config() -> &'static Config {
    CONFIG.get().expect("validation root not initialized")
}
fn log(name: &str, value: &Value) {
    if let Some(c) = CONFIG.get() {
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(c.root.join(name))
            .unwrap();
        writeln!(f, "{value}").unwrap();
    }
}
fn reject_reparse(path: &Path) -> Result<(), String> {
    for ancestor in path.ancestors() {
        let meta = fs::symlink_metadata(ancestor).map_err(|e| e.to_string())?;
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            if meta.file_attributes() & 0x400 != 0 {
                return Err("reparse path refused".into());
            }
        }
        if meta.file_type().is_symlink() {
            return Err("symlink refused".into());
        }
    }
    Ok(())
}
fn prepare(root: &Path, fake: &Path, git: &Path) -> Result<Config, String> {
    if !root.is_absolute() || !fake.is_absolute() || !git.is_absolute() {
        return Err("all paths must be explicit absolute paths".into());
    }
    if root.exists() {
        return Err("validation root must not exist: fresh database required".into());
    }
    let parent = root.parent().ok_or("missing parent")?;
    reject_reparse(parent)?;
    let parent = parent.canonicalize().map_err(|e| e.to_string())?;
    if !parent
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .starts_with("wp2a-desktop-validation-")
    {
        return Err("root must be under explicit audit validation directory".into());
    }
    reject_reparse(fake)?;
    reject_reparse(git)?;
    if fake.file_name().and_then(|s| s.to_str()) != Some("nova_wp2a_fake_cli.exe") {
        return Err("only the named fixture binary is accepted".into());
    }
    if !fs::read(fake)
        .map_err(|e| e.to_string())?
        .windows(b"NOVA_WP2A_FAKE_CLI_NO_NETWORK_NO_SPAWN_V1".len())
        .any(|w| w == b"NOVA_WP2A_FAKE_CLI_NO_NETWORK_NO_SPAWN_V1")
    {
        return Err("fixture identity marker missing".into());
    }
    fs::create_dir(root).map_err(|e| e.to_string())?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let data = root.join("app-data");
    let tools = root.join("tools");
    fs::create_dir(&data).map_err(|e| e.to_string())?;
    fs::create_dir(&tools).map_err(|e| e.to_string())?;
    let copied = tools.join("nova_wp2a_fake_cli.exe");
    fs::copy(fake, &copied).map_err(|e| e.to_string())?;
    fs::write(root.join("ISOLATED-VALIDATION.json"),json!({"test_fixture":true,"root":root,"data":data,"database":data.join("nova.sqlite3"),"fake_cli":copied,"webview":data.join("webview"),"no_real_ai":true}).to_string()).map_err(|e|e.to_string())?;
    let fake_sha = crate::runtime::binary_trust::compute_fingerprint(&copied)
        .map_err(|e| e.to_string())?
        .sha256_hex;
    Ok(Config {
        root,
        data,
        fake: copied.canonicalize().map_err(|e| e.to_string())?,
        git: git.canonicalize().map_err(|e| e.to_string())?,
        fake_sha,
    })
}
pub fn run_validation() {
    let args: Vec<_> = std::env::args().collect();
    let get = |flag: &str| {
        args.windows(2)
            .find(|a| a[0] == flag)
            .map(|a| PathBuf::from(&a[1]))
            .expect(flag)
    };
    let c = prepare(&get("--root"), &get("--fake-cli"), &get("--git"))
        .expect("isolation preflight refused");
    println!(
        "ISOLATION_PREFLIGHT {}",
        fs::read_to_string(c.root.join("ISOLATED-VALIDATION.json")).unwrap()
    );
    fs::write(c.root.join("empty-gitconfig"), "").unwrap();
    // Supplemental Git config isolation, not the database isolation mechanism.
    std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    let git_config = c
        .root
        .join("empty-gitconfig")
        .to_string_lossy()
        .replace(char::from(92), "/")
        .trim_start_matches("//?/")
        .to_string();
    std::env::set_var("GIT_CONFIG_GLOBAL", git_config);
    CONFIG.set(c).unwrap();
    crate::run();
}
struct NoCredentials;
impl CredentialStore for NoCredentials {
    fn save_api_key(&self, _: &str, _: &str) -> NovaResult<()> {
        Err(NovaError::Credential(
            "validation: system credentials and external APIs disabled".into(),
        ))
    }
    fn read_api_key(&self, _: &str) -> NovaResult<Option<String>> {
        Err(NovaError::Credential(
            "validation: system credentials and external APIs disabled".into(),
        ))
    }
    fn delete_api_key(&self, _: &str) -> NovaResult<()> {
        Err(NovaError::Credential(
            "validation: credential operations disabled".into(),
        ))
    }
}
pub(crate) fn guard_process(
    request: &mut crate::runtime::types::ExecutionRequest,
) -> Result<(), String> {
    let Some(c) = CONFIG.get() else {
        #[cfg(test)]
        {
            return Ok(());
        }
        #[cfg(not(test))]
        {
            return Err("validation runtime uninitialized".into());
        }
    };
    process_policy(c, request)
}
fn process_policy(
    c: &Config,
    request: &mut crate::runtime::types::ExecutionRequest,
) -> Result<(), String> {
    if request.program == "git" {
        request.program = c.git.to_string_lossy().into_owned();
    }
    let cwd = request.cwd.canonicalize().map_err(|e| e.to_string())?;
    let program = if request.program == "git" {
        c.git.clone()
    } else {
        PathBuf::from(&request.program)
            .canonicalize()
            .map_err(|_| "validation: non-pinned executable blocked")?
    };
    let fake_unchanged = crate::runtime::binary_trust::compute_fingerprint(&c.fake)
        .map_err(|e| e.to_string())?
        .sha256_hex
        == c.fake_sha;
    let allowed =
        cwd.starts_with(&c.root) && fake_unchanged && (program == c.fake || program == c.git);
    log(
        "process-boundary.jsonl",
        &json!({"program":program,"cwd":cwd,"args":request.args,"allowed":allowed}),
    );
    if allowed {
        Ok(())
    } else {
        Err("validation: real CLI or process outside isolated root blocked".into())
    }
}
pub(crate) fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let c = config();
    let resolved_default = app.path().app_data_dir()?;
    if c.data == resolved_default || c.data.join("nova.sqlite3").exists() {
        return Err("unexpected data path or pre-existing database".into());
    }
    reject_reparse(&c.data).map_err(std::io::Error::other)?;
    let db = Database::open(&c.data.join("nova.sqlite3"))?;
    db.migrate()?;
    db.ensure_default_provider()?;
    seed(&db, c)?;
    log(
        "startup-proof.jsonl",
        &json!({"resolved_database":c.data.join("nova.sqlite3"),"resolved_webview":c.data.join("webview"),"distinct_identifier_default":resolved_default,"fake_cli":c.fake,"fake_sha256":c.fake_sha,"git":c.git,"real_credentials":false,"all_paths_checked_before_database_open":true}),
    );
    app.manage(AppState::new(
        db,
        Arc::new(NoCredentials),
        c.data.clone(),
        None,
    ));
    tauri::WebviewWindowBuilder::new(
        app,
        "main",
        tauri::WebviewUrl::External("http://127.0.0.1:1420".parse()?),
    )
    .title("Nova — WP2A ISOLATED VALIDATION (FAKE CLI)")
    .inner_size(1180., 900.)
    .data_directory(c.data.join("webview"))
    .initialization_script(include_str!("../tests/fixtures/wp2a_validation_driver.js"))
    .on_navigation(|url| url.host_str() == Some("127.0.0.1") || url.scheme() == "about")
    .build()?;
    Ok(())
}
fn snapshot(db: &Database) -> Value {
    let mut result = serde_json::Map::new();
    for table in [
        "tasks",
        "confirmed_requirement_items",
        "self_checks",
        "task_actions",
        "test_evidence",
        "build_evidence",
        "no_tests_confirmations",
        "no_build_confirmations",
        "user_acceptance",
        "task_state_events",
    ] {
        let mut stmt = match db
            .conn()
            .prepare(&format!("SELECT * FROM {table} ORDER BY rowid"))
        {
            Ok(s) => s,
            Err(e) => {
                result.insert(table.into(), json!({"read_error":e.to_string()}));
                continue;
            }
        };
        let cols: Vec<_> = stmt.column_names().iter().map(|s| s.to_string()).collect();
        let rows = stmt
            .query_map([], |row| {
                let mut object = serde_json::Map::new();
                for (i, col) in cols.iter().enumerate() {
                    use rusqlite::types::ValueRef;
                    let value = match row.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(v) => json!(v),
                        ValueRef::Real(v) => json!(v),
                        ValueRef::Text(v) => json!(String::from_utf8_lossy(v)),
                        ValueRef::Blob(_) => json!("BLOB"),
                    };
                    object.insert(col.clone(), value);
                }
                Ok(Value::Object(object))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        result.insert(table.into(), json!(rows));
    }
    Value::Object(result)
}
#[tauri::command]
fn validation_control(
    action: String,
    payload: Value,
    state: tauri::State<'_, AppState>,
) -> Result<Value, String> {
    let c = config();
    match action.as_str() {
        "snapshot" => {
            let db = state.db.lock().unwrap();
            Ok(snapshot(&db))
        }
        "checkpoint" => {
            let db = state.db.lock().unwrap();
            let value = json!({"checkpoint":payload,"database":snapshot(&db)});
            log("checkpoints.jsonl", &value);
            Ok(json!(true))
        }
        "ack" => Ok(json!(c
            .root
            .join(format!("ack-{}", payload["name"].as_str().ok_or("name")?))
            .is_file())),
        "delay" => {
            *DELAY.lock().unwrap() = Some(payload);
            Ok(json!(true))
        }
        "break_reads" => {
            let db = state.db.lock().unwrap();
            let sql = if payload["restore"] == true {
                "ALTER TABLE validation_hidden_self_checks RENAME TO self_checks"
            } else {
                "ALTER TABLE self_checks RENAME TO validation_hidden_self_checks"
            };
            db.conn().execute_batch(sql).map_err(|e| e.to_string())?;
            Ok(json!(true))
        }
        "advance_generation" => {
            let db = state.db.lock().unwrap();
            let id = payload["task"].as_str().ok_or("task")?;
            for s in [
                TaskState::SafeFixing,
                TaskState::Verifying,
                TaskState::PreviewReady,
                TaskState::AwaitingUserAcceptance,
            ] {
                db.transition_task(
                    id,
                    s,
                    Actor::User,
                    Some("ISOLATED FIXTURE generation change"),
                    None,
                    false,
                    None,
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(json!(true))
        }
        "finish" => {
            log("driver-result.jsonl", &payload);
            Ok(json!(true))
        }
        _ => Err("unknown validation control".into()),
    }
}
pub(crate) fn dispatch(
    invoke: Invoke<tauri::Wry>,
    real: Arc<dyn Fn(Invoke<tauri::Wry>) -> bool + Send + Sync>,
) -> bool {
    let name = invoke.message.command().to_string();
    log(
        "ipc.jsonl",
        &json!({"command":name,"payload":format!("{:?}",invoke.message.payload())}),
    );
    if name == "validation_control" {
        let handler: Box<dyn Fn(Invoke<tauri::Wry>) -> bool> =
            Box::new(tauri::generate_handler![validation_control]);
        return handler(invoke);
    }
    // Preserve real command wrappers/registration, but deny unrelated network,
    // credential and arbitrary project/configuration mutation surfaces in test mode.
    let allowed = [
        "initialize_app",
        "get_provider",
        "get_setting",
        "list_conversations",
        "create_conversation",
        "list_messages",
        "list_projects",
        "get_project",
        "set_last_active_project",
        "get_last_active_project",
        "project_directory_state",
        "get_project_memory",
        "list_memory_revisions",
        "create_task",
        "list_tasks_for_project",
        "transition_task",
        "list_task_state_events",
        "record_user_acceptance",
        "get_startup_recovery_summary",
        "start_core_loop",
        "cancel_core_loop",
        "get_core_loop_readiness",
        "get_pending_round",
        "detect_test_command",
        "confirm_task_test_command",
        "list_task_reviews",
        "get_cli_binary_trust_status",
        "confirm_no_tests_applicable",
        "get_no_tests_confirmation_needed",
        "confirm_no_build_applicable",
        "get_no_build_confirmation_needed",
        "accept_task_result",
        "resync_task_documents",
        "get_task_evidence_overview",
        "get_completion_precheck",
        "generate_requirement_snapshot",
        "list_task_self_checks",
        "list_task_documents",
        "read_task_document",
    ];
    if !allowed.contains(&name.as_str()) && !name.starts_with("plugin:event|") {
        invoke
            .resolver
            .reject("isolated validation: unrelated/unsafe command blocked");
        return true;
    }
    let delayed = {
        let mut d = DELAY.lock().unwrap();
        if d.as_ref().is_some_and(|v| {
            v["command"] == name
                && v["task"]
                    .as_str()
                    .is_none_or(|t| format!("{:?}", invoke.message.payload()).contains(t))
        }) {
            d.take()
        } else {
            None
        }
    };
    if let Some(d) = delayed {
        let ms = d["ms"].as_u64().unwrap_or(500).min(5000);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            real(invoke);
        });
        return true;
    }
    real(invoke)
}
fn seed(db: &Database, c: &Config) -> NovaResult<()> {
    let project = c.root.join("project");
    fs::create_dir(&project)?;
    for args in [
        vec!["init", "-q"],
        vec!["config", "user.email", "fixture@invalid.example"],
        vec!["config", "user.name", "WP2A Fixture"],
    ] {
        assert!(std::process::Command::new(&c.git)
            .args(args)
            .current_dir(&project)
            .status()?
            .success());
    }
    fs::write(
        project.join("README.md"),
        "ISOLATED WP2A FIXTURE; no real AI\n",
    )?;
    assert!(std::process::Command::new(&c.git)
        .args(["add", "README.md"])
        .current_dir(&project)
        .status()?
        .success());
    assert!(std::process::Command::new(&c.git)
        .args(["commit", "-qm", "fixture"])
        .current_dir(&project)
        .status()?
        .success());
    let p = db.create_project(&CreateProjectRequest {
        name: "WP2A ISOLATED FIXTURE".into(),
        local_path: project.to_string_lossy().into_owned(),
        source_type: "local".into(),
        source_url: None,
        purpose: None,
        long_term_goal: None,
        user_preferences: None,
        prohibitions: None,
    })?;
    db.set_setting("last_active_project_id", &p.id)?;
    for kind in ["codex_cli", "claude_cli"] {
        db.set_setting(
            if kind == "codex_cli" {
                "codex_binary_path"
            } else {
                "claude_binary_path"
            },
            &c.fake.to_string_lossy(),
        )?;
        let fp = crate::runtime::binary_trust::compute_fingerprint(&c.fake)
            .map_err(|e| NovaError::Config(e.to_string()))?;
        db.confirm_trusted_cli_binary(kind, &fp)?;
    }
    let mut fixtures = Vec::new();
    for (title, mode) in [
        ("01 NORMAL FAKE CLI", "complete"),
        ("02 MISSING SELF CHECK", "missing"),
        ("03 INVALID SELF CHECK", "invalid"),
        ("04 INCOMPLETE SELF CHECK", "incomplete"),
        ("05 SNAPSHOT BACKFILL", "backfill"),
        ("06 CASE C RECOVERY", "casec"),
        ("07 OLD ATTEMPT HISTORY", "old"),
        ("08 FROZEN HISTORY", "frozen"),
        ("09 GENERATION READ FAILURES", "generation"),
    ] {
        let t = db.create_task(
            &CreateTaskRequest {
                project_id: p.id.clone(),
                title: title.into(),
                goal: Some(format!("SCENARIO:{mode}\nExport fixture\nExport fixture")),
                in_scope: Some("CSV".into()),
                not_in_scope: Some("cloud".into()),
                done_criteria: Some("UTF8".into()),
                ai_kind: Some("codex_cli".into()),
            },
            Actor::User,
        )?;
        for s in [TaskState::AwaitingConfirmation, TaskState::Confirmed] {
            db.transition_task(
                &t.id,
                s,
                Actor::User,
                Some("ISOLATED FIXTURE setup"),
                None,
                false,
                None,
            )?;
        }
        db.record_no_tests_confirmation(&p.id, &t.id, 0, 1, 1, Some("FIXTURE no tests"))?;
        db.record_no_build_confirmation(&p.id, &t.id, 0, 1, 1, Some("FIXTURE no build"))?;
        if mode == "backfill" {
            db.conn().execute(
                "DELETE FROM confirmed_requirement_items WHERE task_id=?1",
                [&t.id],
            )?;
        }
        if ["casec", "old", "frozen", "generation"].contains(&mode) {
            let dk = format!("dev-{}-round-1", t.id);
            db.plan_action(&t.id, "ai_development", &dk)?;
            db.start_action(&t.id, &dk, false)?;
            db.finish_action(&t.id, &dk, ActionStatus::Succeeded, None, None, 1)?;
            for s in [TaskState::Queued, TaskState::Running, TaskState::Verifying] {
                db.transition_task(
                    &t.id,
                    s,
                    Actor::User,
                    Some("ISOLATED FIXTURE evidence"),
                    None,
                    false,
                    None,
                )?;
            }
            use crate::storage::evidence::{RecordBuildEvidenceRequest, RecordTestEvidenceRequest};
            db.record_test_evidence(&RecordTestEvidenceRequest {
                project_id: &p.id,
                task_id: &t.id,
                result_generation: 0,
                round: 1,
                result_attempt: 1,
                test_command_redacted: None,
                status: "passed",
                passed_count: 1,
                failed_count: 0,
                skipped_count: 0,
                exit_code: Some(0),
                reason: Some("FIXTURE NOT REAL TEST EXECUTION"),
                stdout_ref: None,
                stderr_ref: None,
            })?;
            let casec = mode == "casec";
            db.record_build_evidence(&RecordBuildEvidenceRequest {
                project_id: &p.id,
                task_id: &t.id,
                result_generation: 0,
                round: 1,
                result_attempt: 1,
                build_command_redacted: None,
                status: if casec {
                    "insufficient_evidence"
                } else {
                    "passed"
                },
                authorization_source: None,
                exit_code: None,
                reason: Some("FIXTURE NOT REAL BUILD EXECUTION"),
                stdout_ref: None,
                stderr_ref: None,
            })?;
            let coverage: Vec<_> = db
                .list_confirmed_requirement_items(&t.id)?
                .iter()
                .map(|i| json!({"item":format!("{}:{}",i.field,i.item_index),"status":"covered"}))
                .collect();
            let output = format!(
                "===NOVA_SELF_CHECK_START===\n{}\n===NOVA_SELF_CHECK_END===",
                json!({"requirement_coverage":coverage,"check_results":["FIXTURE self-check, not AI"],"unfinished_items":[],"blocking_issues":[],"self_reported_status":"self_reported_complete"})
            );
            db.record_self_check(&crate::storage::self_check::RecordSelfCheckRequest {
                project_id: &p.id,
                task_id: &t.id,
                round: 1,
                result_attempt: 1,
                parse_status: "valid",
                self_reported_status: Some("self_reported_complete"),
                requirement_coverage_json: Some(&serde_json::to_string(&coverage).unwrap()),
                check_results_json: Some("[\"FIXTURE, not AI\"]"),
                unfinished_items_json: Some("[]"),
                blocking_issues_json: Some("[]"),
                raw_block_redacted: &output,
            })?;
            if casec {
                let key = format!("build-{}-generation-0-round-1", t.id);
                db.plan_action(&t.id, "build_execution", &key)?;
                db.start_action(&t.id, &key, false)?;
                db.finish_action(
                    &t.id,
                    &key,
                    ActionStatus::Failed,
                    None,
                    Some("build detection insufficient, awaiting user confirmation"),
                    1,
                )?;
                db.reset_failed_action_for_retry(&t.id, &key)?;
                db.transition_task(
                    &t.id,
                    TaskState::NeedsUserAction,
                    Actor::User,
                    None,
                    None,
                    false,
                    None,
                )?;
            } else {
                for st in [TaskState::PreviewReady, TaskState::AwaitingUserAcceptance] {
                    db.transition_task(
                        &t.id,
                        st,
                        Actor::User,
                        Some("SYNTHETIC FIXTURE result"),
                        None,
                        false,
                        None,
                    )?;
                }
            }
            if mode == "old" {
                db.conn().execute("UPDATE task_actions SET attempt_number=2 WHERE task_id=?1 AND idempotency_key=?2",rusqlite::params![t.id,dk])?;
            }
            if mode == "frozen" {
                db.conn().execute(
                    "UPDATE tasks SET legacy_classification='unknown_insufficient' WHERE id=?1",
                    [&t.id],
                )?;
            }
            crate::core_loop::documents::generate_result_documents(db, &p.id, &t.id, &c.data)?;
        }
        fixtures.push(json!({"id":t.id,"title":title,"mode":mode,"origin":"isolated_validation::seed, explicit synthetic fixture, not real AI"}));
    }
    fs::write(
        c.root.join("fixtures.json"),
        json!({"project":p.id,"fixtures":fixtures}).to_string(),
    )?;
    fs::write(
        c.root.join("database-before.json"),
        snapshot(db).to_string(),
    )?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn isolation_refuses_relative_existing_and_unmarked_parent() {
        let t = std::env::temp_dir();
        assert!(prepare(Path::new("relative"), &t, &t).is_err());
        assert!(prepare(&t, &t, &t).is_err());
        let root = t.join(format!("nova-isolation-{}", uuid::Uuid::new_v4()));
        assert!(prepare(&root, &t, &t).is_err());
        assert!(!root.exists());
    }
    #[test]
    fn release_entry_is_feature_gated_and_frontend_is_unmodified() {
        let manifest = include_str!("../Cargo.toml");
        assert!(manifest.contains("required-features = [\"wp2a-desktop-validation\"]"));
        assert!(include_str!("lib.rs").contains("not(debug_assertions)"));
        assert!(include_str!("../tests/fixtures/wp2a_validation_driver.js").contains("DOM"));
    }
    #[test]
    fn process_policy_pins_fake_hash_cwd_and_denies_real_cli_names() {
        let parent = std::env::temp_dir().join(format!(
            "wp2a-desktop-validation-policy-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&parent).unwrap();
        let fake = parent.join("nova_wp2a_fake_cli.exe");
        fs::copy(std::env::current_exe().unwrap(), &fake).unwrap();
        let c = prepare(
            &parent.join("fresh-run"),
            &fake,
            &std::env::current_exe().unwrap(),
        )
        .unwrap();
        let request = |program: String, cwd: PathBuf| {
            crate::runtime::types::ExecutionRequest::new(program, vec![], cwd, Some(c.root.clone()))
        };
        assert!(process_policy(
            &c,
            &mut request(c.fake.to_string_lossy().into_owned(), c.root.clone())
        )
        .is_ok());
        assert!(process_policy(&c, &mut request("codex".into(), c.root.clone())).is_err());
        assert!(process_policy(&c, &mut request("claude".into(), c.root.clone())).is_err());
        assert!(process_policy(
            &c,
            &mut request(c.fake.to_string_lossy().into_owned(), parent.clone())
        )
        .is_err());
        fs::write(&c.fake, "changed fixture binary").unwrap();
        assert!(process_policy(
            &c,
            &mut request(c.fake.to_string_lossy().into_owned(), c.root.clone())
        )
        .is_err());
    }
}
