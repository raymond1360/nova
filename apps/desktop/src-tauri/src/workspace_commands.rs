//! Lock order: run reservations -> main DB mutex -> SQLite write transaction.
//! Never hold these locks across probes, processes, awaits or filesystem work.
use crate::{
    db::Database,
    storage::workspace::{Request, WorkspaceError},
    AppState,
};
use std::{collections::HashMap, sync::Mutex};
use tauri::State;
use tokio_util::sync::CancellationToken;

pub(crate) fn reserve<T>(
    runs: &Mutex<HashMap<String, CancellationToken>>,
    db: &Mutex<Database>,
    task_id: &str,
    token: CancellationToken,
    check: impl FnOnce(&Database) -> Result<T, String>,
) -> Result<T, String> {
    let mut runs = runs.lock().map_err(|_| "ACTIVITY_UNKNOWN".to_string())?;
    if runs.contains_key(task_id) {
        return Err("EXECUTION_IN_PROGRESS".into());
    }
    let db = db.lock().map_err(|_| "ACTIVITY_UNKNOWN".to_string())?;
    let result = check(&db)?;
    runs.insert(task_id.to_string(), token);
    Ok(result)
}
pub(crate) fn execute(
    runs: &Mutex<HashMap<String, CancellationToken>>,
    db: &Mutex<Database>,
    request: &Request,
) -> Result<serde_json::Value, WorkspaceError> {
    let unknown = || WorkspaceError {
        code: "ACTIVITY_UNKNOWN".into(),
        current_revision: None,
    };
    let runs = runs.lock().map_err(|_| unknown())?;
    let db = db.lock().map_err(|_| unknown())?;
    db.workspace(request, &runs.keys().cloned().collect::<Vec<_>>())
}
#[tauri::command]
pub fn requirement_workspace(
    request: Request,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, WorkspaceError> {
    execute(&state.core_loop_cancel, &state.db, &request)
}
#[tauri::command]
pub fn get_planning_capability() -> serde_json::Value {
    serde_json::json!({"status":"blocked","reason":"PLANNING_NOT_IMPLEMENTED","ai_calls":0})
}

#[cfg(test)]
mod tests;
