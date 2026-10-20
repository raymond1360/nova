#[cfg(all(feature = "wp2a-desktop-validation", not(debug_assertions)))]
compile_error!("wp2a-desktop-validation is debug-only and must never ship");
#[cfg(feature = "wp2a-desktop-validation")]
mod isolated_validation;
#[cfg(feature = "wp2a-desktop-validation")]
pub use isolated_validation::run_validation;

mod api;
mod app_paths;
mod commands;
mod core_loop;
mod credential;
mod db;
mod error;
mod models;
mod runtime;
mod security;
mod storage;
mod validation;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::Manager;
use tokio_util::sync::CancellationToken;

use credential::CredentialStore;
#[cfg(not(feature = "wp2a-desktop-validation"))]
use credential::SystemCredentialStore;
use db::Database;
use models::RecoverySummary;
use runtime::detection_slot::DetectionSlot;

pub struct AppState {
    db: Arc<Mutex<Database>>,
    client: reqwest::Client,
    credentials: Arc<dyn CredentialStore>,
    active_cancel: Arc<Mutex<Option<CancellationToken>>>,
    /// Phase 3 remediation (Major 4): one cancellation handle per
    /// currently-running core-loop task, keyed by `task_id`. Distinct from
    /// `active_cancel` above (that one is chat-only). Only ever holds an
    /// entry for a task that is genuinely mid-run; `commands::start_core_loop`
    /// inserts it just before spawning the loop and removes it once the loop
    /// returns (success, failure, or cancellation), so a stale entry can
    /// never outlive its run.
    core_loop_cancel: Arc<Mutex<HashMap<String, CancellationToken>>>,
    app_data_dir: PathBuf,
    detection_cancel: DetectionSlot,
    /// Computed once, synchronously, during `setup()` — before any Tauri
    /// command can run — by `Database::run_startup_recovery`. Never
    /// recomputed afterward: the whole point is a single, idempotent scan
    /// at process start, not a live/repeatedly-polled value.
    startup_recovery_summary: Option<RecoverySummary>,
}

impl AppState {
    fn new(
        db: Database,
        credentials: Arc<dyn CredentialStore>,
        app_data_dir: PathBuf,
        startup_recovery_summary: Option<RecoverySummary>,
    ) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .expect("reqwest client");
        Self {
            db: Arc::new(Mutex::new(db)),
            client,
            credentials,
            active_cancel: Arc::new(Mutex::new(None)),
            core_loop_cancel: Arc::new(Mutex::new(HashMap::new())),
            app_data_dir,
            detection_cancel: DetectionSlot::new(),
            startup_recovery_summary,
        }
    }
}

pub fn run() {
    #[cfg(feature = "wp2a-desktop-validation")]
    isolated_validation::require_active();
    #[allow(unused_mut)]
    let mut context = tauri::generate_context!();
    #[cfg(feature = "wp2a-desktop-validation")]
    {
        context.config_mut().app.windows.clear();
        context.config_mut().identifier = "com.sdkmax.nova.wp2a.validation".into();
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(feature = "wp2a-desktop-validation")]
            { return isolated_validation::setup(app); }
            #[cfg(not(feature = "wp2a-desktop-validation"))]
            {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let db_path = app_data_dir.join("nova.sqlite3");
            let db = Database::open(&db_path)?;
            db.migrate()?;
            db.ensure_default_provider()?;
            // Runs before any command is dispatched: sweeps any
            // crash-orphaned `started` actions to `uncertain` and, if the
            // last-active project had a task mid-flight, moves it to
            // `interrupted` — never re-running a command, a write, or an
            // AI call. See `storage::recovery`.
            let startup_recovery_summary = db.run_startup_recovery()?;
            app.manage(AppState::new(
                db,
                Arc::new(SystemCredentialStore::new()),
                app_data_dir,
                startup_recovery_summary,
            ));
            Ok(())
            }
        })
        .invoke_handler({
            let real: Arc<dyn Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync> = std::sync::Arc::new(tauri::generate_handler![
            commands::initialize_app,
            commands::get_provider,
            commands::save_provider,
            commands::delete_provider_credential,
            commands::fetch_models,
            commands::test_provider_connection,
            commands::fetch_token_usage,
            commands::get_setting,
            commands::set_setting,
            commands::list_conversations,
            commands::create_conversation,
            commands::delete_conversation,
            commands::list_messages,
            commands::send_chat_message,
            commands::cancel_active_chat,
            commands::detect_runtime_environment,
            commands::cancel_environment_detection,
            commands::check_project_directory_access,
            commands::check_project_directory_readable,
            commands::create_project,
            commands::list_projects,
            commands::get_project,
            commands::set_last_active_project,
            commands::get_last_active_project,
            commands::project_directory_state,
            commands::get_project_memory,
            commands::update_project_memory_field,
            commands::list_memory_revisions,
            commands::create_task,
            commands::list_tasks_for_project,
            commands::transition_task,
            commands::list_task_state_events,
            commands::record_user_acceptance,
            commands::get_startup_recovery_summary,
            commands::resolve_recovery_checkpoint,
            commands::start_core_loop,
            commands::get_verify_only_eligibility,
            commands::verify_existing_changes,
            commands::cancel_core_loop,
            commands::get_core_loop_readiness,
            commands::get_pending_round,
            commands::detect_test_command,
            commands::confirm_task_test_command,
            commands::list_task_reviews,
            commands::resolve_cli_binary_path,
            commands::get_cli_binary_trust_status,
            commands::confirm_cli_binary_path,
            commands::confirm_no_tests_applicable,
            commands::get_no_tests_confirmation_needed,
            commands::confirm_no_build_applicable,
            commands::get_no_build_confirmation_needed,
            commands::accept_task_result,
            commands::resync_task_documents,
            commands::get_task_evidence_overview,
            commands::get_completion_precheck,
            commands::generate_requirement_snapshot,
            commands::list_task_self_checks,
            commands::list_task_documents,
            commands::read_task_document,
            ]);
            move |invoke| {
                #[cfg(feature = "wp2a-desktop-validation")]
                { isolated_validation::dispatch(invoke, real.clone()) }
                #[cfg(not(feature = "wp2a-desktop-validation"))]
                { real(invoke) }
            }
        })
        .build(context)
        .expect("error while building Nova")
        .run(|app_handle, event| {
            // Major 4: on app shutdown, every core-loop task still in
            // flight must be cancelled — never left as an orphaned CLI
            // process outliving the Nova process itself. `RunEvent::Exit`
            // fires once, right before the process actually terminates.
            if let tauri::RunEvent::Exit = event {
                let state = app_handle.state::<AppState>();
                let tokens: Vec<CancellationToken> = state
                    .core_loop_cancel
                    .lock()
                    .unwrap()
                    .values()
                    .cloned()
                    .collect();
                for token in tokens {
                    token.cancel();
                }
            }
        });
}
