//! Product command surface; network is reachable only after separate explicit approvals.
use crate::{
    api_only::{self as a, config, transport},
    AppState,
};
use serde_json::Value;
use tauri::State;
use tokio_util::sync::CancellationToken;
#[tauri::command]
pub fn api_only_catalog() -> Value {
    serde_json::json!([
     {"provider":"sdkmax","base_url":"https://api.sdkmax.com/v1","protocol":"openai_chat","adapter_ready":false},
     {"provider":"openai","base_url":"https://api.openai.com/v1","protocol":"openai_responses","adapter_ready":false},
     {"provider":"claude","base_url":"https://api.anthropic.com/v1","protocol":"anthropic_messages","adapter_ready":false},
     {"provider":"kimi","base_url":"https://api.moonshot.ai/v1","protocol":"openai_chat","adapter_ready":false},
     {"provider":"deepseek","base_url":"https://api.deepseek.com/v1","protocol":"deepseek_chat","adapter_ready":true},
     {"provider":"gemini","base_url":"https://generativelanguage.googleapis.com/v1beta","protocol":"gemini_content","adapter_ready":false},
     {"provider":"openai_compatible","base_url":null,"protocol":"openai_chat","adapter_ready":false}
    ])
}
#[tauri::command]
pub fn api_only_configs(state: State<'_, AppState>) -> Result<Vec<a::ConfigView>, String> {
    config::list_configs(&*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?)
        .map_err(str::to_string)
}
#[tauri::command]
pub fn api_only_save_config(
    config: a::Config,
    key: Option<String>,
    state: State<'_, AppState>,
) -> Result<a::ConfigView, String> {
    config::save_config(
        &*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?,
        state.credentials.as_ref(),
        config,
        key,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_only_delete_key(
    id: String,
    state: State<'_, AppState>,
) -> Result<a::ConfigView, String> {
    config::delete_key(
        &*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?,
        state.credentials.as_ref(),
        &id,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_only_prepare(
    task_id: String,
    config_id: String,
    paths: Vec<String>,
    tests: Vec<a::FixedTest>,
    allow_repair: bool,
    state: State<'_, AppState>,
) -> Result<a::Plan, String> {
    if a::pm::managed(&*state.db.lock().map_err(|_| "API_BUSY")?, &task_id).map_err(str::to_string)? { return Err("API_PM_COORDINATOR_REQUIRED".into()); }
    a::prepare_scoped(
        &*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?,
        state.credentials.as_ref(),
        &task_id,
        &config_id,
        paths,
        tests,
        allow_repair,
        None,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_only_approve(
    id: String,
    digest: String,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let task=a::status(&*state.db.lock().map_err(|_| "API_BUSY")?, &id).map_err(str::to_string)?;
    if a::pm::managed(&*state.db.lock().map_err(|_| "API_BUSY")?, task["task_id"].as_str().ok_or("API_TASK_INVALID")?).map_err(str::to_string)? { return Err("API_PM_COORDINATOR_REQUIRED".into()); }
    a::approve(
        &*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?,
        &id,
        &digest,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_only_status(id: String, state: State<'_, AppState>) -> Result<Value, String> {
    a::status(&*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?, &id).map_err(str::to_string)
}
#[tauri::command]
pub async fn api_only_test_connection(
    id: String,
    explicit_network: bool,
    state: State<'_, AppState>,
) -> Result<transport::Connectivity, String> {
    if !explicit_network {
        return Err("API_NETWORK_APPROVAL_REQUIRED".into());
    }
    let db_path = state.app_data_dir.join("nova.sqlite3");
    let vault = state.credentials.clone();
    tokio::task::spawn_blocking(move || {
        let db =
            crate::db::Database::open(&db_path).map_err(|_| "API_DATABASE_ERROR".to_string())?;
        transport::test_connection(
            &db,
            vault.as_ref(),
            &id,
            true,
            &mut a::DeepSeekHttp,
            &CancellationToken::new(),
        )
        .map_err(str::to_string)
    })
    .await
    .map_err(|_| "API_CONNECTION_RESULT_UNKNOWN".to_string())?
}
struct Slot {
    map: std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, CancellationToken>>>,
    task: String,
}
impl Drop for Slot {
    fn drop(&mut self) {
        if let Ok(mut map) = self.map.lock() {
            map.remove(&self.task);
        }
    }
}
#[tauri::command]
pub async fn api_only_execute(id: String, state: State<'_, AppState>) -> Result<Value, String> {
    let task = a::status(&*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?, &id)
        .map_err(str::to_string)?["task_id"]
        .as_str()
        .ok_or("API_TASK_INVALID")?
        .to_string();
    let cancel = CancellationToken::new();
    {
        let mut map = state.core_loop_cancel.lock().map_err(|_| "API_BUSY")?;
        if !map.is_empty() {
            return Err("API_SERIAL_TASK_BUSY".into());
        };
        map.insert(task.clone(), cancel.clone());
    }
    let slot = Slot {
        map: state.core_loop_cancel.clone(),
        task,
    };
    let db_path = state.app_data_dir.join("nova.sqlite3");
    let vault = state.credentials.clone();
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let db =
            crate::db::Database::open(&db_path).map_err(|_| "API_DATABASE_ERROR".to_string())?;
        a::task_loop::execute_loop(
            &db,
            &id,
            vault.as_ref(),
            &mut a::DeepSeekHttp,
            &mut a::FixedTestRunner,
            &cancel,
        )
        .map_err(str::to_string)
    })
    .await
    .map_err(|_| "API_RESULT_UNKNOWN_NO_REPLAY".to_string())?
}

#[tauri::command]
pub fn api_only_task_report(task_id: String, state: State<'_, AppState>) -> Result<Value, String> {
    a::task_loop::report(
        &*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?,
        &task_id,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_only_repair(
    parent: String,
    paths: Vec<String>,
    tests: Vec<a::FixedTest>,
    config_id: String,
    state: State<'_, AppState>,
) -> Result<a::Plan, String> {
    let task=a::status(&*state.db.lock().map_err(|_| "API_BUSY")?, &parent).map_err(str::to_string)?;
    if a::pm::managed(&*state.db.lock().map_err(|_| "API_BUSY")?, task["task_id"].as_str().ok_or("API_TASK_INVALID")?).map_err(str::to_string)? { return Err("API_PM_COORDINATOR_REQUIRED".into()); }
    a::task_loop::repair(
        &*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?,
        state.credentials.as_ref(),
        &parent,
        paths,
        tests,
        &config_id,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_only_user_decision(
    id: String,
    accepted: bool,
    note: String,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    a::task_loop::user_decision_with_vault(
        &*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?,
        state.credentials.as_ref(),
        &id,
        accepted,
        &note,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_only_cleanup_status(state: State<'_, AppState>) -> Result<Value, String> {
    config::cleanup_status(&*state.db.lock().map_err(|_| "API_DATABASE_BUSY")?)
        .map_err(str::to_string)
}

#[tauri::command]
pub fn api_pm_prepare(
    request: a::pm::Request,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    a::pm::prepare(
        &*state.db.lock().map_err(|_| "API_BUSY")?,
        state.credentials.as_ref(),
        request,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_pm_status(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Option<Value>, String> {
    a::pm::latest(&*state.db.lock().map_err(|_| "API_BUSY")?, &project_id).map_err(str::to_string)
}
#[tauri::command]
pub fn api_pm_confirm(id: String, state: State<'_, AppState>) -> Result<Value, String> {
    a::pm::confirm(
        &*state.db.lock().map_err(|_| "API_BUSY")?,
        state.credentials.as_ref(),
        &id,
    )
    .map_err(str::to_string)
}
#[tauri::command]
pub fn api_pm_approve(
    id: String,
    digest: String,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    a::pm::approve(&*state.db.lock().map_err(|_| "API_BUSY")?, &id, &digest).map_err(str::to_string)
}
#[tauri::command]
pub async fn api_pm_run(
    id: String,
    planning_digest: Option<String>,
    explicit_network: bool,
    state: State<'_, AppState>,
) -> Result<Value, String> {
    let cancel = CancellationToken::new();
    {
        let mut map = state.core_loop_cancel.lock().map_err(|_| "API_BUSY")?;
        if !map.is_empty() {
            return Err("API_BUSY".into());
        }
        map.insert(id.clone(), cancel.clone());
    }
    let slot = Slot {
        map: state.core_loop_cancel.clone(),
        task: id.clone(),
    };
    let path = state.app_data_dir.join("nova.sqlite3");
    let vault = state.credentials.clone();
    tokio::task::spawn_blocking(move || {
        let _slot = slot;
        let db = crate::db::Database::open(&path).map_err(|_| "API_DATABASE_ERROR".to_string())?;
        match planning_digest {
            Some(d) => a::pm::plan(
                &db,
                vault.as_ref(),
                &id,
                &d,
                explicit_network,
                &mut a::DeepSeekHttp,
                &cancel,
            ),
            None => a::pm::execute(
                &db,
                vault.as_ref(),
                &id,
                &mut a::DeepSeekHttp,
                &mut a::DeepSeekHttp,
                &mut a::FixedTestRunner,
                &cancel,
            ),
        }
        .map_err(str::to_string)
    })
    .await
    .map_err(|_| "API_RESULT_UNKNOWN_NO_REPLAY".to_string())?
}
#[tauri::command]
pub fn api_pm_cancel(id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Some(c) = state
        .core_loop_cancel
        .lock()
        .map_err(|_| "API_BUSY")?
        .get(&id)
    {
        c.cancel();
        return Ok(());
    }
    a::pm::cancel_pending(&*state.db.lock().map_err(|_| "API_BUSY")?, &id).map_err(str::to_string)
}
