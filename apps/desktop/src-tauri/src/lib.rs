mod api;
mod commands;
mod credential;
mod db;
mod error;
mod models;
mod runtime;
mod security;
mod validation;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tauri::Manager;
use tokio_util::sync::CancellationToken;

use credential::{CredentialStore, SystemCredentialStore};
use db::Database;
use runtime::detection_slot::DetectionSlot;

pub struct AppState {
    db: Arc<Mutex<Database>>,
    client: reqwest::Client,
    credentials: Arc<dyn CredentialStore>,
    active_cancel: Arc<Mutex<Option<CancellationToken>>>,
    app_data_dir: PathBuf,
    detection_cancel: DetectionSlot,
}

impl AppState {
    fn new(db: Database, credentials: Arc<dyn CredentialStore>, app_data_dir: PathBuf) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(45))
            .build()
            .expect("reqwest client");
        Self {
            db: Arc::new(Mutex::new(db)),
            client,
            credentials,
            active_cancel: Arc::new(Mutex::new(None)),
            app_data_dir,
            detection_cancel: DetectionSlot::new(),
        }
    }
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&app_data_dir)?;
            let db_path = app_data_dir.join("nova.sqlite3");
            let db = Database::open(&db_path)?;
            db.migrate()?;
            db.ensure_default_provider()?;
            app.manage(AppState::new(
                db,
                Arc::new(SystemCredentialStore::new()),
                app_data_dir,
            ));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Nova");
}
