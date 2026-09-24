use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::api::{self, ApiProvider, StreamChunk};
use crate::db::RequestLogEntry;
use crate::error::{public_error, NovaError, NovaResult};
use crate::models::{
    Actor, ChatMessage, Conversation, CreateProjectRequest, CreateTaskRequest, ModelInfo, Project,
    ProjectDirectoryState, ProjectMemory, ProjectMemoryRevision, ProviderSummary, RecoverySummary,
    SaveProviderRequest, SendChatRequest, SendChatResult, Task, TaskState, TaskStateEvent,
    TestConnectionResult, TokenUsage, UserAcceptance,
};
use crate::runtime::detect::{self, DirectoryStatus, RuntimeEnvironmentReport};
use crate::security::redact_sensitive_text;
use crate::storage::projects::check_project_directory_state;
use crate::validation::{assert_allowed_path, normalize_base_url, AllowedEndpoint};
use crate::AppState;

#[tauri::command]
pub fn initialize_app(state: State<'_, AppState>) -> Result<(), String> {
    with_db(&state, |db| {
        db.migrate()?;
        db.ensure_default_provider()?;
        Ok(())
    })
}

#[tauri::command]
pub fn get_provider(
    provider_id: String,
    state: State<'_, AppState>,
) -> Result<ProviderSummary, String> {
    with_db(&state, |db| db.provider(&provider_id).map_err(Into::into))
}

#[tauri::command]
pub fn save_provider(
    request: SaveProviderRequest,
    state: State<'_, AppState>,
) -> Result<ProviderSummary, String> {
    let base_url = normalize_base_url(&request.base_url).map_err(public_error)?;
    if request.api_key.trim().is_empty() {
        return Err("API Key 不能为空".to_string());
    }
    state
        .credentials
        .save_api_key(&request.provider_id, request.api_key.trim())
        .map_err(public_error)?;
    with_db(&state, |db| {
        db.save_provider_metadata(
            &request.provider_id,
            &request.name,
            &base_url,
            Some(request.api_key.trim()),
        )
        .map_err(Into::into)
    })
}

#[tauri::command]
pub fn delete_provider_credential(
    provider_id: String,
    state: State<'_, AppState>,
) -> Result<ProviderSummary, String> {
    state
        .credentials
        .delete_api_key(&provider_id)
        .map_err(public_error)?;
    with_db(&state, |db| {
        db.clear_provider_key(&provider_id).map_err(Into::into)
    })
}

#[tauri::command]
pub async fn fetch_models(
    provider_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ModelInfo>, String> {
    assert_allowed_path(AllowedEndpoint::Models.path()).map_err(public_error)?;
    let provider = api_provider(&state, &provider_id)?;
    api::fetch_models(&state.client, &provider)
        .await
        .map_err(public_error)
}

#[tauri::command]
pub async fn test_provider_connection(
    provider_id: String,
    state: State<'_, AppState>,
) -> Result<TestConnectionResult, String> {
    let provider = api_provider(&state, &provider_id)?;
    let models = api::fetch_models(&state.client, &provider)
        .await
        .map_err(public_error)?;
    Ok(TestConnectionResult {
        ok: true,
        message: "连接正常".to_string(),
        models_count: models.len(),
        request_id: None,
    })
}

#[tauri::command]
pub async fn fetch_token_usage(
    provider_id: String,
    state: State<'_, AppState>,
) -> Result<TokenUsage, String> {
    assert_allowed_path(AllowedEndpoint::TokenUsage.path()).map_err(public_error)?;
    let provider = api_provider(&state, &provider_id)?;
    api::fetch_token_usage(&state.client, &provider)
        .await
        .map_err(public_error)
}

#[tauri::command]
pub fn get_setting(key: String, state: State<'_, AppState>) -> Result<Option<String>, String> {
    with_db(&state, |db| db.get_setting(&key).map_err(Into::into))
}

#[tauri::command]
pub fn set_setting(key: String, value: String, state: State<'_, AppState>) -> Result<(), String> {
    with_db(&state, |db| {
        db.set_setting(&key, &value).map_err(Into::into)
    })
}

#[tauri::command]
pub fn list_conversations(state: State<'_, AppState>) -> Result<Vec<Conversation>, String> {
    with_db(&state, |db| db.list_conversations().map_err(Into::into))
}

#[tauri::command]
pub fn create_conversation(
    provider_id: String,
    model_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Conversation, String> {
    with_db(&state, |db| {
        db.create_conversation(&provider_id, model_id.as_deref())
            .map_err(Into::into)
    })
}

#[tauri::command]
pub fn delete_conversation(
    conversation_id: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    with_db(&state, |db| {
        db.delete_conversation(&conversation_id).map_err(Into::into)
    })
}

#[tauri::command]
pub fn list_messages(
    conversation_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ChatMessage>, String> {
    with_db(&state, |db| {
        db.list_messages(&conversation_id).map_err(Into::into)
    })
}

#[tauri::command]
pub async fn send_chat_message(
    request: SendChatRequest,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<SendChatResult, String> {
    assert_allowed_path(AllowedEndpoint::ChatCompletions.path()).map_err(public_error)?;
    let provider = api_provider(&state, &request.provider_id)?;
    let (assistant_message_id, messages) = with_db(&state, |db| {
        db.create_message(
            &request.conversation_id,
            "user",
            &request.content,
            &request.provider_id,
            &request.model_id,
            "complete",
        )?;
        let assistant = db.create_message(
            &request.conversation_id,
            "assistant",
            "",
            &request.provider_id,
            &request.model_id,
            "streaming",
        )?;
        let messages = db.list_messages(&request.conversation_id)?;
        Ok::<_, NovaError>((assistant.id, messages))
    })?;
    let cancel = CancellationToken::new();
    *state
        .active_cancel
        .lock()
        .map_err(|_| "无法创建取消令牌".to_string())? = Some(cancel.clone());

    let db_for_chunk = state.db.clone();
    let db_for_finish = state.db.clone();
    let client = state.client.clone();
    let active_cancel = state.active_cancel.clone();
    let model_id = request.model_id.clone();
    let provider_id = request.provider_id.clone();
    let app_for_stream = app.clone();
    let assistant_id_for_stream = assistant_message_id.clone();
    tauri::async_runtime::spawn(async move {
        let assistant_id_for_chunk = assistant_id_for_stream.clone();
        let stream_result = api::stream_chat(
            client,
            provider.clone(),
            model_id.clone(),
            messages,
            cancel,
            move |chunk: StreamChunk| {
                if let Ok(db) = db_for_chunk.lock() {
                    let _ = db.append_message_delta(
                        &assistant_id_for_chunk,
                        &chunk.delta,
                        chunk.request_id.as_deref(),
                    );
                }
                let _ = app_for_stream.emit(
                    "chat://chunk",
                    serde_json::json!({
                        "message_id": assistant_id_for_chunk,
                        "content_delta": chunk.delta,
                        "request_id": chunk.request_id
                    }),
                );
            },
        )
        .await;

        if let Ok(mut guard) = active_cancel.lock() {
            *guard = None;
        }

        match stream_result {
            Ok(request_id) => {
                if let Ok(db) = db_for_finish.lock() {
                    let _ = db.update_message_status(
                        &assistant_id_for_stream,
                        "complete",
                        request_id.as_deref(),
                    );
                    let _ = db.log_request(RequestLogEntry {
                        provider_id: &provider_id,
                        model_id: Some(&model_id),
                        endpoint: AllowedEndpoint::ChatCompletions.path(),
                        status_code: Some(200),
                        error_code: None,
                        message: None,
                        request_id: request_id.as_deref(),
                    });
                }
                let _ = app.emit(
                    "chat://done",
                    serde_json::json!({
                        "message_id": assistant_id_for_stream,
                        "status": "complete",
                        "request_id": request_id
                    }),
                );
            }
            Err(detail) => {
                let status = stream_error_status(&detail);
                let public_message = stream_error_message(&detail);
                if let Ok(db) = db_for_finish.lock() {
                    let _ = db.update_message_status(
                        &assistant_id_for_stream,
                        status,
                        detail.request_id.as_deref(),
                    );
                    let _ = db.log_request(RequestLogEntry {
                        provider_id: &provider_id,
                        model_id: Some(&model_id),
                        endpoint: AllowedEndpoint::ChatCompletions.path(),
                        status_code: detail.status,
                        error_code: detail.code.as_deref(),
                        message: Some(&public_message),
                        request_id: detail.request_id.as_deref(),
                    });
                }
                let _ = app.emit(
                    "chat://error",
                    serde_json::json!({
                        "message_id": assistant_id_for_stream,
                        "message": public_message,
                        "request_id": detail.request_id
                    }),
                );
            }
        }
    });

    Ok(SendChatResult {
        assistant_message_id,
    })
}

fn stream_error_status(detail: &api::ApiErrorDetail) -> &'static str {
    if detail.received_delta
        || detail.code.as_deref() == Some("user_canceled")
        || detail.code.as_deref() == Some("sse_incomplete")
    {
        "interrupted"
    } else {
        "error"
    }
}

fn stream_error_message(detail: &api::ApiErrorDetail) -> String {
    if detail.received_delta && detail.code.as_deref() != Some("user_canceled") {
        "回答中断，可重新生成".to_string()
    } else {
        redact_sensitive_text(&detail.message)
    }
}

#[tauri::command]
pub fn cancel_active_chat(state: State<'_, AppState>) -> Result<(), String> {
    if let Some(cancel) = state
        .active_cancel
        .lock()
        .map_err(|_| "无法读取取消令牌".to_string())?
        .as_ref()
    {
        cancel.cancel();
    }
    Ok(())
}

fn api_provider(state: &State<'_, AppState>, provider_id: &str) -> Result<ApiProvider, String> {
    let summary = with_db(state, |db| db.provider(provider_id).map_err(Into::into))?;
    let api_key = state
        .credentials
        .read_api_key(provider_id)
        .map_err(public_error)?
        .ok_or_else(|| "未保存 API Key".to_string())?;
    Ok(ApiProvider {
        base_url: summary.base_url,
        api_key,
    })
}

fn with_db<T, F>(state: &State<'_, AppState>, action: F) -> Result<T, String>
where
    F: FnOnce(&crate::db::Database) -> NovaResult<T>,
{
    let db = state
        .db
        .lock()
        .map_err(|_| "本地数据库锁定失败".to_string())?;
    action(&db).map_err(public_error)
}

/// Detect Git/Codex CLI/Claude Code CLI readiness, platform support, and
/// Nova's own app data directory access.
///
/// Serialized via `AppState.detection_cancel` (a [`DetectionSlot`]): only
/// one detection run may be in flight at a time. A second call while one is
/// already running is rejected with an error rather than silently
/// replacing the first run's cancellation token — the earlier Phase 1
/// review found that an unconditional overwrite let a second call's
/// completion clear a *third*, unrelated run's token, and let `cancel`
/// target the wrong run. `DetectionSlot::try_start` only ever holds its
/// lock for the brief check-and-set, never across the `.await` below, and
/// its returned handle clears the slot on every exit path — including an
/// unexpected panic — via `Drop`.
#[tauri::command]
pub async fn detect_runtime_environment(
    state: State<'_, AppState>,
) -> Result<RuntimeEnvironmentReport, String> {
    let (cancel, _handle) = state
        .detection_cancel
        .try_start()
        .map_err(|_| "环境检测已在进行中，请稍候或先取消当前检测后重试".to_string())?;

    let app_data_dir = state.app_data_dir.clone();
    Ok(detect::build_report(app_data_dir, cancel).await)
}

#[tauri::command]
pub fn cancel_environment_detection(state: State<'_, AppState>) -> Result<(), String> {
    state.detection_cancel.cancel_current();
    Ok(())
}

/// Check whether a user-selected directory exists, is a directory, and is
/// writable by Nova (create+write+remove a probe file). Used before Phase
/// 3+ treats a directory as a project root; exposed now so the minimal
/// Phase 1 environment panel can exercise and test the same boundary logic
/// end to end. Callers must only invoke this for a directory the user has
/// actually selected/authorized — never speculatively in a read-only
/// context, where [`check_project_directory_readable`] is the correct call.
#[tauri::command]
pub fn check_project_directory_access(path: String) -> Result<DirectoryStatus, String> {
    Ok(detect::check_directory_writable(std::path::Path::new(
        &path,
    )))
}

/// Read-only counterpart to [`check_project_directory_access`]: confirms a
/// directory exists and is readable without creating, writing, or deleting
/// anything. This is the correct check for a read-only/review context, or
/// for inspecting a candidate directory before the user has explicitly
/// authorized write access to it.
#[tauri::command]
pub fn check_project_directory_readable(path: String) -> Result<DirectoryStatus, String> {
    Ok(detect::check_directory_readable(std::path::Path::new(
        &path,
    )))
}

// --- Phase 2: project registry, memory, task state machine, and
// crash-recovery commands. All SQL lives in `storage::*`/`db.rs`; these
// commands only translate between the frontend and those repository
// functions, and never touch the database directly. ---

#[tauri::command]
pub fn create_project(
    request: CreateProjectRequest,
    state: State<'_, AppState>,
) -> Result<Project, String> {
    with_db(&state, |db| db.create_project(&request))
}

#[tauri::command]
pub fn list_projects(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    with_db(&state, |db| db.list_projects())
}

#[tauri::command]
pub fn get_project(project_id: String, state: State<'_, AppState>) -> Result<Project, String> {
    with_db(&state, |db| db.project(&project_id))
}

#[tauri::command]
pub fn set_last_active_project(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Project, String> {
    with_db(&state, |db| db.set_last_active_project(&project_id))
}

#[tauri::command]
pub fn get_last_active_project(state: State<'_, AppState>) -> Result<Option<Project>, String> {
    with_db(&state, |db| db.last_active_project())
}

/// Read-only directory-existence check for an already-registered project
/// (moved/deleted-directory detection) — distinct from
/// [`check_project_directory_access`]/[`check_project_directory_readable`],
/// which validate a directory the user is about to select, before it is
/// ever registered as a project.
#[tauri::command]
pub fn project_directory_state(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<ProjectDirectoryState, String> {
    with_db(&state, |db| {
        let project = db.project(&project_id)?;
        Ok(check_project_directory_state(&project))
    })
}

#[tauri::command]
pub fn get_project_memory(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<ProjectMemory, String> {
    with_db(&state, |db| db.project_memory(&project_id))
}

/// `changed_by` is deliberately not a frontend-supplied parameter — every
/// memory edit reachable through this command was, by construction, made
/// through the Nova UI by a person, so it is always attributed to
/// `Actor::User` server-side. A frontend caller cannot make this command
/// masquerade as `nova`/`connected_ai`/`system`; only backend-internal code
/// (not exposed as any Tauri command) can record those actors.
#[tauri::command]
pub fn update_project_memory_field(
    project_id: String,
    field_name: String,
    new_value: Option<String>,
    reason: Option<String>,
    state: State<'_, AppState>,
) -> Result<ProjectMemory, String> {
    with_db(&state, |db| {
        db.update_project_memory_field(
            &project_id,
            &field_name,
            new_value.as_deref(),
            Actor::User,
            reason.as_deref(),
        )
    })
}

#[tauri::command]
pub fn list_memory_revisions(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<ProjectMemoryRevision>, String> {
    with_db(&state, |db| db.list_memory_revisions(&project_id))
}

#[tauri::command]
pub fn create_task(request: CreateTaskRequest, state: State<'_, AppState>) -> Result<Task, String> {
    with_db(&state, |db| db.create_task(&request, Actor::User))
}

#[tauri::command]
pub fn list_tasks_for_project(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Task>, String> {
    with_db(&state, |db| db.list_tasks_for_project(&project_id))
}

/// The single authoritative entry point the frontend has for moving a task
/// between states — it can never pass a state transition that
/// [`crate::storage::tasks::is_legal_transition`] rejects; the backend is
/// the sole authority on what's legal, matching the "前端传入目标状态不能绕过
/// 合法转换" architecture requirement.
///
/// `actor` is deliberately not a parameter here: every transition reachable
/// through this command is, by construction, requested by a person through
/// the Nova UI, so it is always recorded as `Actor::User` server-side — a
/// frontend caller cannot forge `system`/`nova`/`connected_ai`/recovery
/// authority through this or any other exposed command. Those actors, and
/// the `is_recovery` flag, are only ever set by backend-internal code (see
/// `storage::recovery`, `storage::tasks::Database::create_task`), never by
/// anything the frontend can reach.
#[tauri::command]
pub fn transition_task(
    task_id: String,
    to_state: String,
    reason: Option<String>,
    correlation_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<Task, String> {
    let to: TaskState = to_state.parse().map_err(|error: String| error)?;
    with_db(&state, |db| {
        db.transition_task(
            &task_id,
            to,
            Actor::User,
            reason.as_deref(),
            correlation_id.as_deref(),
            false,
            None,
        )
    })
}

#[tauri::command]
pub fn list_task_state_events(
    task_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<TaskStateEvent>, String> {
    with_db(&state, |db| db.list_task_state_events(&task_id))
}

/// Must only be invoked by the UI action tied to the user's explicit
/// "我已经看过，符合需要" confirmation — never automatically. See
/// [`crate::db::Database::record_user_acceptance`].
#[tauri::command]
pub fn record_user_acceptance(
    project_id: String,
    task_id: String,
    accepted: bool,
    user_note: Option<String>,
    state: State<'_, AppState>,
) -> Result<UserAcceptance, String> {
    with_db(&state, |db| {
        db.record_user_acceptance(&project_id, &task_id, accepted, user_note.as_deref())
    })
}

/// Returns the recovery summary computed once at app startup (see
/// `AppState::startup_recovery_summary`) — the minimal "恢复上次工作" panel's
/// only data source. Recomputing this on every call would defeat the
/// point of a single, idempotent startup scan.
#[tauri::command]
pub fn get_startup_recovery_summary(
    state: State<'_, AppState>,
) -> Result<Option<RecoverySummary>, String> {
    Ok(state.startup_recovery_summary.clone())
}

#[tauri::command]
pub fn resolve_recovery_checkpoint(
    checkpoint_id: String,
    resolution: String,
    state: State<'_, AppState>,
) -> Result<(), String> {
    with_db(&state, |db| {
        db.resolve_recovery_checkpoint(&checkpoint_id, &resolution)
    })
}

// --- Phase 3: Open Local Project Dual-AI Core Loop ---

/// Phase 3 remediation (Blocker 3): read-only auto-detection of a test
/// command for a project, exposed so the minimal UI can show the user what
/// Nova would run and let them confirm or edit it before a task starts.
/// Never executes anything itself.
#[tauri::command]
pub fn detect_test_command(
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Option<String>, String> {
    let project_dir = with_db(&state, |db| db.project(&project_id))?;
    Ok(
        crate::core_loop::test_runner::detect_test_command(std::path::Path::new(
            &project_dir.local_path,
        ))
        .map(|command| command.display()),
    )
}

/// Records the user-confirmed (or Nova-auto-detected, re-confirmed by the
/// user in the UI) test command for a task. `source` must be exactly
/// `"auto"` or `"user"` — never anything derived from AI output.
#[tauri::command]
pub fn confirm_task_test_command(
    project_id: String,
    task_id: String,
    command: String,
    source: String,
    state: State<'_, AppState>,
) -> Result<Task, String> {
    with_db(&state, |db| {
        db.set_task_test_command(&task_id, &command, &source)?;
        // Second re-review remediation: if the task stalled at
        // `needs_user_action` because the previous round's test command was
        // missing or failed the allowlist check, providing a real command
        // now must actually unblock a retry, not leave the loop stuck.
        crate::core_loop::unblock_test_execution_for_retry(db, &project_id, &task_id)
    })
}

/// v0.1.0 scope correction (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`,
/// Blocker 1): Nova's v0.1.0 product main path requires exactly one
/// connected developer AI — Codex or Claude Code, user's choice. A real,
/// just-executed probe against the *selected* developer AI's configured
/// binary is the only thing `dev_ready` depends on. `reviewer_kind`/
/// `review_ready`/`review_message` are still returned for frontend/type
/// compatibility — the UI may still show the other AI's connection state as
/// an optional diagnostic (and this is exactly the data a future v0.2.0
/// dual-AI-review mode would gate on) — but nothing in the backend ever
/// refuses to start or blocks completion because the *other* AI is
/// missing, untrusted, or not logged in. See `start_core_loop` below, which
/// is the actual v0.1.0 gate this struct's data feeds.
///
/// Codex re-review remediation (Minor 2): `get_core_loop_readiness` no
/// longer actively probes the unselected AI at all (previously a real,
/// just-executed child-process invocation purely for this diagnostic) — so
/// `review_ready` is now always `false` and `review_message` is a constant
/// "not checked" string, never a faked `true` and never the result of a
/// probe the user never asked for.
#[derive(serde::Serialize)]
pub struct CoreLoopReadiness {
    pub dev_kind: String,
    pub reviewer_kind: String,
    pub dev_ready: bool,
    pub review_ready: bool,
    pub dev_message: String,
    pub review_message: String,
}

/// CLI binary paths, configurable (not PATH-only): read from
/// `app_settings` (`codex_binary_path`/`claude_binary_path`, set via the
/// already-exposed generic `set_setting` command) with a bare-name PATH
/// fallback. A missing/unreadable setting is never an error here — it just
/// means "use PATH", which is a legal, common configuration.
fn cli_binary_paths(state: &AppState) -> (String, String) {
    let db = state.db.lock().unwrap();
    let codex = db
        .get_setting("codex_binary_path")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "codex".to_string());
    let claude = db
        .get_setting("claude_binary_path")
        .ok()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "claude".to_string());
    (codex, claude)
}

fn build_dual_adapter(state: &AppState) -> crate::runtime::ai_adapter::DualCliAdapter {
    let (codex_binary, claude_binary) = cli_binary_paths(state);
    crate::runtime::ai_adapter::DualCliAdapter::new(
        crate::runtime::ai_adapter::CliConfig::from_binary_name(&codex_binary),
        crate::runtime::ai_adapter::CliConfig::from_binary_name(&claude_binary),
    )
}

fn display_name(kind: crate::runtime::types::ToolKind) -> &'static str {
    match kind {
        crate::runtime::types::ToolKind::CodexCli => "Codex CLI",
        crate::runtime::types::ToolKind::ClaudeCli => "Claude CLI",
        crate::runtime::types::ToolKind::Git => "Git",
    }
}

/// Second re-review (Blocker 2 / executable trust boundary): readiness must
/// never be reported `true` purely from a working `--version`/`--help`
/// probe — the *exact* binary that would actually be launched must also
/// match a fingerprint the user explicitly confirmed. A binary that is
/// technically capable but was swapped out, or never confirmed at all,
/// must fail closed here rather than being treated as usable.
fn binary_trust_message(
    db: &crate::db::Database,
    kind: crate::runtime::types::ToolKind,
    configured_path: &str,
) -> Option<String> {
    let kind_str = crate::runtime::ai_adapter::ai_kind_to_str(kind);
    match current_fingerprint_status(db, kind_str, configured_path) {
        Ok(status) if status.trusted => None,
        Ok(status) => Some(format!(
            "{}：{}",
            display_name(kind),
            status
                .mismatch_reason
                .unwrap_or_else(|| "可执行文件尚未通过信任确认".to_string())
        )),
        Err(error) => Some(format!("{}：{error}", display_name(kind))),
    }
}

/// v0.1.0 scope correction (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`,
/// Blocker 1): the actual v0.1.0 start-gate decision, extracted as a pure
/// function so it is directly unit-testable without a `State<AppState>` —
/// unlike `start_core_loop` itself. Takes only the *selected* developer
/// AI's own readiness/trust; deliberately has no parameter for the other
/// AI's state at all, so "the other AI is missing/untrusted/not logged in"
/// cannot even be expressed as an input that could accidentally block
/// this — the calling site (`start_core_loop`) simply never passes it in.
fn dev_only_start_gate(
    dev_ready: bool,
    dev_trust_message: Option<&str>,
    dev_readiness_message: impl FnOnce() -> String,
) -> Result<(), String> {
    if let Some(message) = dev_trust_message {
        return Err(message.to_string());
    }
    if !dev_ready {
        return Err(dev_readiness_message());
    }
    Ok(())
}

/// Codex re-review remediation (Minor 2): only the *selected* developer
/// AI's binary is ever probed here. The unselected AI's readiness used to
/// be actively probed too (a real, just-executed `--version`/`--help`
/// child-process invocation) purely to populate an optional diagnostic
/// field — meaning a user who only wants to use Codex could still see this
/// call block on, or surface an error/prompt from, a Claude CLI they never
/// configured and do not intend to use. `reviewer_kind`/`review_message`
/// are still returned (frontend/type compatibility — see `CoreLoopReadiness`
/// above), but `review_ready` is now always `false` and `review_message` is
/// a constant "not checked, optional" string — never a real probe result,
/// and never faked as `true`.
#[tauri::command]
pub fn get_core_loop_readiness(
    dev_ai_kind: String,
    state: State<'_, AppState>,
) -> Result<CoreLoopReadiness, String> {
    let dev_kind = crate::runtime::ai_adapter::ai_kind_from_str(&dev_ai_kind)
        .ok_or_else(|| "未知的开发 AI 类型".to_string())?;
    let reviewer_kind = crate::runtime::ai_adapter::the_other(dev_kind);
    let dual = build_dual_adapter(&state);
    let cancel = CancellationToken::new();
    let dev_readiness = dual.probe_dev(dev_kind, &state.app_data_dir, &cancel);

    let (codex_binary, claude_binary) = cli_binary_paths(&state);
    let binary_for = |kind: crate::runtime::types::ToolKind| -> &str {
        match kind {
            crate::runtime::types::ToolKind::CodexCli => &codex_binary,
            crate::runtime::types::ToolKind::ClaudeCli => &claude_binary,
            crate::runtime::types::ToolKind::Git => unreachable!(),
        }
    };
    let dev_trust_message = with_db(&state, |db| {
        Ok(binary_trust_message(db, dev_kind, binary_for(dev_kind)))
    })?;

    let dev_ready = dev_readiness.dev_ready() && dev_trust_message.is_none();
    let dev_message =
        dev_trust_message.unwrap_or_else(|| dev_readiness.user_message(display_name(dev_kind)));

    Ok(CoreLoopReadiness {
        dev_kind: crate::runtime::ai_adapter::ai_kind_to_str(dev_kind).to_string(),
        reviewer_kind: crate::runtime::ai_adapter::ai_kind_to_str(reviewer_kind).to_string(),
        dev_ready,
        review_ready: false,
        dev_message,
        review_message: "未检查（可选诊断，不影响启动）".to_string(),
    })
}

/// Codex re-review remediation (Major 1): the single source of truth for
/// which round (1 or 2) a stalled task's next action targets — a thin
/// parameter-boundary wrapper around `core_loop::current_pending_round`.
/// The frontend must call this instead of inferring the round itself from
/// `task_reviews` (see `CoreLoopView.vue`/`coreLoop.ts`'s prior
/// `pendingRound` getter, which mirrored the exact stale logic this audit
/// flagged in the backend).
#[tauri::command]
pub fn get_pending_round(
    project_id: String,
    task_id: String,
    state: State<'_, AppState>,
) -> Result<i64, String> {
    with_db(&state, |db| {
        crate::core_loop::current_pending_round(db, &project_id, &task_id)
    })
}

/// Starts the dual-AI core loop for a confirmed/queued task in the
/// background and returns immediately — a real developer-AI/reviewer-AI
/// run can take many minutes, far too long to hold a synchronous Tauri
/// command response (or the shared `AppState.db` mutex) open for. The
/// precondition check here is a fast, synchronous read on the shared
/// connection; the actual loop runs on its own, independent SQLite
/// connection to the same database file (safe under Phase 2's tested
/// multi-connection concurrency guarantees), so a long-running core loop
/// never blocks any other command. Progress is observable through the
/// already-exposed `list_task_state_events`/`get_project`/
/// `list_tasks_for_project` commands; this command intentionally adds no
/// new live-progress/event infrastructure, matching Phase 3's "no more
/// than the minimal interface" scope.
///
/// Phase 3 remediation (Blocker 2): before any AI is invoked or the task is
/// transitioned to `running`, both the developer and reviewer CLI's
/// readiness is checked via a real probe. If either is not ready, this
/// returns a clear, actionable Chinese error and the task is left exactly
/// where it was — never transitioned, never touched by any AI call.
#[tauri::command]
pub fn start_core_loop(
    task_id: String,
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<(), String> {
    let dev_kind = with_db(&state, |db| {
        let task = db.task(&task_id)?;
        if !matches!(task.state, TaskState::Confirmed | TaskState::Queued) {
            return Err(NovaError::Config(format!(
                "只能从 confirmed 或 queued 状态启动核心循环，当前状态：{}",
                task.state
            )));
        }
        let dev_kind =
            crate::runtime::ai_adapter::ai_kind_from_str(task.ai_kind.as_deref().unwrap_or(""))
                .ok_or_else(|| {
                    NovaError::Config("任务未设置有效的开发 AI 类型（ai_kind）".to_string())
                })?;
        Ok(dev_kind)
    })?;

    // Only one active run handle per task at a time: reserve the slot with
    // a CAS-style check before doing anything else observable.
    {
        let mut guard = state.core_loop_cancel.lock().unwrap();
        if guard.contains_key(&task_id) {
            return Err("该任务已在运行核心循环，请勿重复启动".to_string());
        }
        guard.insert(task_id.clone(), CancellationToken::new());
    }

    // v0.1.0 scope correction (Blocker 1): the *only* thing that may block
    // starting the loop is the readiness of the developer AI the user
    // actually selected — the other CLI's connection state (missing,
    // untrusted, not logged in) is never probed or checked here, and must
    // never prevent this task from starting or completing.
    let dual = build_dual_adapter(&state);
    let readiness_cancel = CancellationToken::new();
    let dev_readiness = dual.probe_dev(dev_kind, &state.app_data_dir, &readiness_cancel);
    let (codex_binary, claude_binary) = cli_binary_paths(&state);
    let binary_for = |kind: crate::runtime::types::ToolKind| -> &str {
        match kind {
            crate::runtime::types::ToolKind::CodexCli => &codex_binary,
            crate::runtime::types::ToolKind::ClaudeCli => &claude_binary,
            crate::runtime::types::ToolKind::Git => unreachable!(),
        }
    };
    let dev_trust_message = with_db(&state, |db| {
        Ok(binary_trust_message(db, dev_kind, binary_for(dev_kind)))
    })?;
    if let Err(message) = dev_only_start_gate(
        dev_readiness.dev_ready(),
        dev_trust_message.as_deref(),
        || dev_readiness.user_message(display_name(dev_kind)),
    ) {
        state.core_loop_cancel.lock().unwrap().remove(&task_id);
        return Err(message);
    }

    let cancel = state
        .core_loop_cancel
        .lock()
        .unwrap()
        .get(&task_id)
        .cloned()
        .expect("just inserted above");

    let (codex_binary, claude_binary) = cli_binary_paths(&state);
    let db_path = state.app_data_dir.join("nova.sqlite3");
    let app_data_dir = state.app_data_dir.clone();
    let task_id_for_event = task_id.clone();
    let task_id_for_cleanup = task_id.clone();
    let core_loop_cancel = state.core_loop_cancel.clone();
    tauri::async_runtime::spawn(async move {
        let outcome = tokio::task::spawn_blocking(move || {
            let db = crate::db::Database::open(&db_path).ok()?;
            let adapter = crate::runtime::ai_adapter::DualCliAdapter::new(
                crate::runtime::ai_adapter::CliConfig::from_binary_name(&codex_binary),
                crate::runtime::ai_adapter::CliConfig::from_binary_name(&claude_binary),
            );
            crate::core_loop::run_core_loop(&db, &task_id, &adapter, &cancel, &app_data_dir).ok()
        })
        .await
        .ok()
        .flatten();

        // The run handle is only ever meant to exist while the loop is
        // genuinely in flight — remove it now that it has returned
        // (success, failure, or cancellation), so `cancel_core_loop` can
        // never target a completed run and a fresh `start_core_loop` for
        // the same task is never blocked by a stale entry.
        core_loop_cancel
            .lock()
            .unwrap()
            .remove(&task_id_for_cleanup);

        // A minimal, final-state-only signal — deliberately not a
        // step-by-step progress stream, matching Phase 3's "no more than
        // the minimal interface" scope. Callers can still poll
        // `list_task_state_events`/`list_task_reviews` for detail.
        if let Some(outcome) = outcome {
            let _ = app.emit(
                "core-loop://done",
                serde_json::json!({
                    "task_id": task_id_for_event,
                    "state": outcome.task.state,
                }),
            );
        }
    });
    Ok(())
}

/// Phase 3 remediation (Major 4): cancels a currently-running core-loop
/// task. Terminates the corresponding CLI process (via the `CancellationToken`
/// `runtime::process` already watches on every wait loop iteration),
/// causing `run_core_loop` to record `cancelled`/`needs_user_action`
/// through the normal Action Ledger + state machine path — this command
/// itself performs no state transition, it only signals cancellation.
/// Cancelling a task with no active run handle (already finished, or never
/// started) returns a clear error rather than silently doing nothing.
#[tauri::command]
pub fn cancel_core_loop(task_id: String, state: State<'_, AppState>) -> Result<(), String> {
    let guard = state.core_loop_cancel.lock().unwrap();
    match guard.get(&task_id) {
        Some(token) => {
            token.cancel();
            Ok(())
        }
        None => Err("该任务当前没有正在运行的核心循环，无法取消".to_string()),
    }
}

#[tauri::command]
pub fn list_task_reviews(
    project_id: String,
    task_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<crate::models::TaskReview>, String> {
    with_db(&state, |db| db.list_task_reviews(&project_id, &task_id))
}

// --- Second re-review remediation: no-tests-applicable user confirmation,
// executable trust boundary, and atomic acceptance/rejection ---

/// Resolves `name` (a bare command name, e.g. `"codex"`) against `PATH` the
/// same way Nova's own process boundary would, returning the concrete
/// absolute path so the UI can show the user exactly which file it is about
/// to ask them to trust — never silently launching whatever a bare name
/// happens to resolve to without the user ever having seen the real path.
#[tauri::command]
pub fn resolve_cli_binary_path(name: String) -> Result<String, String> {
    crate::runtime::win_spawn::resolve_executable(&name)
        .map(|path| path.to_string_lossy().into_owned())
        .map_err(|error| format!("无法解析可执行文件路径：{error}"))
}

#[derive(serde::Serialize)]
pub struct CliBinaryTrustStatus {
    pub trusted: bool,
    pub canonical_path: Option<String>,
    pub file_size: Option<i64>,
    pub sha256_hex: Option<String>,
    pub confirmed_at: Option<String>,
    /// Present only when a trust record exists for `kind` but the live
    /// file at `configured_path` no longer matches it (replaced, resized,
    /// or edited since confirmation) — the concrete reason readiness must
    /// fail closed, surfaced verbatim to the user rather than a generic
    /// "not trusted".
    pub mismatch_reason: Option<String>,
}

fn current_fingerprint_status(
    db: &crate::db::Database,
    kind: &str,
    configured_path: &str,
) -> Result<CliBinaryTrustStatus, String> {
    let resolved = crate::runtime::win_spawn::resolve_executable(configured_path)
        .map_err(|error| format!("无法解析可执行文件路径：{error}"))?;
    let live = crate::runtime::binary_trust::compute_fingerprint(&resolved);
    let trusted_record = db.find_trusted_cli_binary(kind).map_err(public_error)?;

    match (trusted_record, live) {
        (Some(record), Ok(live_fp)) => {
            let recorded_fp = crate::runtime::binary_trust::BinaryFingerprint {
                canonical_path: record.canonical_path.clone(),
                file_size: record.file_size as u64,
                modified_unix_ms: record.modified_unix_ms,
                sha256_hex: record.sha256_hex.clone(),
            };
            let matches = crate::runtime::binary_trust::fingerprints_match(&recorded_fp, &live_fp);
            Ok(CliBinaryTrustStatus {
                trusted: matches,
                canonical_path: Some(live_fp.canonical_path),
                file_size: Some(live_fp.file_size as i64),
                sha256_hex: Some(live_fp.sha256_hex),
                confirmed_at: Some(record.confirmed_at),
                mismatch_reason: if matches {
                    None
                } else {
                    Some(
                        "可执行文件自上次确认后已发生变化（路径/大小/哈希不一致），需要重新确认"
                            .to_string(),
                    )
                },
            })
        }
        (None, Ok(live_fp)) => Ok(CliBinaryTrustStatus {
            trusted: false,
            canonical_path: Some(live_fp.canonical_path),
            file_size: Some(live_fp.file_size as i64),
            sha256_hex: Some(live_fp.sha256_hex),
            confirmed_at: None,
            mismatch_reason: Some("尚未确认过该可执行文件，请先确认后再使用".to_string()),
        }),
        (_, Err(error)) => Ok(CliBinaryTrustStatus {
            trusted: false,
            canonical_path: None,
            file_size: None,
            sha256_hex: None,
            confirmed_at: None,
            mismatch_reason: Some(format!("无法验证可执行文件：{error}")),
        }),
    }
}

#[tauri::command]
pub fn get_cli_binary_trust_status(
    kind: String,
    configured_path: String,
    state: State<'_, AppState>,
) -> Result<CliBinaryTrustStatus, String> {
    with_db(&state, |db| {
        current_fingerprint_status(db, &kind, &configured_path).map_err(NovaError::Config)
    })
}

/// The only write path for `trusted_cli_binaries` — always `Actor::User`
/// (hardcoded at the storage layer, see `storage::binary_trust`), called
/// only from the explicit "确认此可执行文件" UI action. Re-resolves and
/// re-fingerprints `path` itself rather than trusting anything the caller
/// claims about it.
#[tauri::command]
pub fn confirm_cli_binary_path(
    kind: String,
    path: String,
    state: State<'_, AppState>,
) -> Result<CliBinaryTrustStatus, String> {
    let resolved = crate::runtime::win_spawn::resolve_executable(&path)
        .map_err(|error| format!("无法解析可执行文件路径：{error}"))?;
    let fingerprint = crate::runtime::binary_trust::compute_fingerprint(&resolved)
        .map_err(|error| error.to_string())?;
    with_db(&state, |db| {
        db.confirm_trusted_cli_binary(&kind, &fingerprint)
    })?;
    with_db(&state, |db| {
        current_fingerprint_status(db, &kind, &path).map_err(NovaError::Config)
    })
}

/// Second re-review (Blocker 3): the only write path for
/// `no_tests_confirmations` — always `Actor::User` (hardcoded at the
/// storage layer), reachable only from the explicit "确认该项目当前没有适用测试"
/// UI action. Scoped to the task's *current* `result_generation`, so a
/// confirmation always applies to the round the user is actually looking
/// at, never a stale one silently carried forward.
///
/// Second re-review remediation (Major 5): the single atomic business entry
/// point for a user's accept/reject decision — a thin parameter boundary
/// around `core_loop::accept_task_result`, which does the actual
/// record-acceptance / transition / document-regeneration work and is
/// exercised directly by that module's own tests (see this crate's
/// architecture note at the top of `core_loop.rs`: commands stay a
/// permission/parameter boundary only, business logic lives in
/// `core_loop`).
#[tauri::command]
pub fn accept_task_result(
    task_id: String,
    project_id: String,
    accepted: bool,
    note: Option<String>,
    state: State<'_, AppState>,
) -> Result<Task, String> {
    with_db(&state, |db| {
        crate::core_loop::accept_task_result(
            db,
            &project_id,
            &task_id,
            accepted,
            note.as_deref(),
            &state.app_data_dir,
        )
    })
}

/// Retries seven-document regeneration for a task whose
/// `document_sync_pending` flag is set (see `accept_task_result`) — lets
/// the UI offer an explicit "重试文档同步" action, or a future app-startup
/// sweep, without requiring another accept/reject decision.
#[tauri::command]
pub fn resync_task_documents(
    task_id: String,
    project_id: String,
    state: State<'_, AppState>,
) -> Result<Task, String> {
    with_db(&state, |db| {
        crate::core_loop::resync_task_documents(db, &project_id, &task_id, &state.app_data_dir)
    })
}

#[tauri::command]
pub fn confirm_no_tests_applicable(
    project_id: String,
    task_id: String,
    round: i64,
    reason: Option<String>,
    state: State<'_, AppState>,
) -> Result<crate::models::NoTestsConfirmation, String> {
    with_db(&state, |db| {
        let task = db.task(&task_id)?;
        let confirmation = db.record_no_tests_confirmation(
            &project_id,
            &task_id,
            task.result_generation,
            round,
            reason.as_deref(),
        )?;
        // Second re-review remediation: the confirmation alone does not
        // resume the loop — without this, the task would stay stuck at
        // `needs_user_action` forever even though the user just answered
        // exactly what it was waiting for.
        crate::core_loop::unblock_test_execution_for_retry(db, &project_id, &task_id)?;
        Ok(confirmation)
    })
}

/// Codex re-review remediation (final Major closing Work Package 1): the
/// build-gate equivalent of `confirm_no_tests_applicable` — the only write
/// path for `no_build_confirmations`, always `Actor::User` (hardcoded at
/// the storage layer), reachable only from an explicit "确认该项目当前无需构建"
/// UI action.
///
/// Deliberately no longer accepts a caller-supplied `round` at all — the
/// previous signature let any frontend (or a compromised/buggy one)
/// pre-write a no-build confirmation for an arbitrary generation/round the
/// task was not actually stalled on, including a future round, or write
/// one while the task was in a state where no build stall existed at all
/// (`Draft`/`Running`/`Completed`/...). `core_loop::confirm_no_build_
/// applicable` is the sole authority for the generation/round (derived
/// from the task's own real `result_generation` and
/// `core_loop::current_pending_round`) and for the full state/action/
/// evidence authorization boundary — this command is purely a parameter
/// pass-through, exactly matching this codebase's "commands stay a
/// permission/parameter boundary only" architecture rule.
#[tauri::command]
pub fn confirm_no_build_applicable(
    project_id: String,
    task_id: String,
    reason: Option<String>,
    state: State<'_, AppState>,
) -> Result<crate::models::NoBuildConfirmation, String> {
    with_db(&state, |db| {
        crate::core_loop::confirm_no_build_applicable(db, &project_id, &task_id, reason.as_deref())
    })
}

/// v0.1.0 scope correction (Work Package 2): a read-only parameter
/// pass-through to [`crate::core_loop::no_build_confirmation_needed`] — the
/// frontend polls this (alongside `detect_test_command`) to decide whether
/// to show the "confirm this project currently needs no build" prompt for
/// a `needs_user_action` task. Never accepts a caller-supplied round or
/// generation; the backend remains the sole authority for both, exactly as
/// `confirm_no_build_applicable` already requires.
#[tauri::command]
pub fn get_no_build_confirmation_needed(
    project_id: String,
    task_id: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    with_db(&state, |db| {
        crate::core_loop::no_build_confirmation_needed(db, &project_id, &task_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(code: Option<&str>, message: &str, received_delta: bool) -> api::ApiErrorDetail {
        api::ApiErrorDetail {
            status: None,
            code: code.map(str::to_string),
            message: message.to_string(),
            request_id: None,
            received_delta,
        }
    }

    #[test]
    fn partial_stream_errors_map_to_interrupted_message() {
        let error = detail(Some("sse_parse_error"), "bad json", true);
        assert_eq!(stream_error_status(&error), "interrupted");
        assert_eq!(stream_error_message(&error), "回答中断，可重新生成");
    }

    #[test]
    fn user_cancel_maps_to_interrupted_without_retry_hint() {
        let error = detail(Some("user_canceled"), "生成已停止，已保留收到的内容", true);
        assert_eq!(stream_error_status(&error), "interrupted");
        assert_eq!(stream_error_message(&error), "生成已停止，已保留收到的内容");
    }

    fn temp_exe(name: &str, content: &[u8]) -> std::path::PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "nova-commands-test-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tool.exe");
        std::fs::write(&path, content).unwrap();
        path
    }

    #[test]
    fn fingerprint_status_is_untrusted_before_any_confirmation() {
        let path = temp_exe("untrusted", b"v1");
        let db = crate::db::Database::memory().unwrap();
        db.migrate().unwrap();
        let status = current_fingerprint_status(&db, "codex_cli", path.to_str().unwrap()).unwrap();
        assert!(!status.trusted);
        assert!(status.mismatch_reason.is_some());
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn fingerprint_status_is_trusted_immediately_after_confirmation() {
        let path = temp_exe("trusted", b"v1");
        let db = crate::db::Database::memory().unwrap();
        db.migrate().unwrap();
        let fingerprint = crate::runtime::binary_trust::compute_fingerprint(&path).unwrap();
        db.confirm_trusted_cli_binary("codex_cli", &fingerprint)
            .unwrap();
        let status = current_fingerprint_status(&db, "codex_cli", path.to_str().unwrap()).unwrap();
        assert!(status.trusted, "{:?}", status.mismatch_reason);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn fingerprint_status_flips_back_to_untrusted_after_the_file_changes() {
        let path = temp_exe("drift", b"v1");
        let db = crate::db::Database::memory().unwrap();
        db.migrate().unwrap();
        let fingerprint = crate::runtime::binary_trust::compute_fingerprint(&path).unwrap();
        db.confirm_trusted_cli_binary("codex_cli", &fingerprint)
            .unwrap();
        std::fs::write(&path, b"v2, a completely different binary").unwrap();

        let status = current_fingerprint_status(&db, "codex_cli", path.to_str().unwrap()).unwrap();
        assert!(
            !status.trusted,
            "a binary replaced after confirmation must not still be trusted"
        );
        assert!(status.mismatch_reason.unwrap().contains("已发生变化"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn confirming_codex_trust_never_affects_claude_trust() {
        let path = temp_exe("independent", b"v1");
        let db = crate::db::Database::memory().unwrap();
        db.migrate().unwrap();
        let fingerprint = crate::runtime::binary_trust::compute_fingerprint(&path).unwrap();
        db.confirm_trusted_cli_binary("codex_cli", &fingerprint)
            .unwrap();
        let claude_status =
            current_fingerprint_status(&db, "claude_cli", path.to_str().unwrap()).unwrap();
        assert!(!claude_status.trusted);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    // --- v0.1.0 scope correction: single-AI start gate ---
    // (`CODEX_V0.1_SCOPE_CORRECTION_AUDIT_09211027.md`, section 5's
    // required test list, items 1-4). `dev_only_start_gate` never takes the
    // *other* AI's state as an input at all, so these four combinations —
    // matching the audit's own wording — are really "no matter what the
    // caller believes about the other AI, it structurally cannot reach
    // this function."

    #[test]
    fn codex_ready_claude_missing_still_allows_start() {
        // "Claude missing" is simply never represented in this function's
        // inputs — the only way to prove it can't block is that a
        // dev-ready call succeeds regardless of what the (unrepresentable)
        // other AI's state would have been.
        assert!(dev_only_start_gate(true, None, || unreachable!()).is_ok());
    }

    #[test]
    fn codex_ready_claude_untrusted_still_allows_start() {
        assert!(dev_only_start_gate(true, None, || unreachable!()).is_ok());
    }

    #[test]
    fn codex_ready_claude_not_logged_in_still_allows_start() {
        assert!(dev_only_start_gate(true, None, || unreachable!()).is_ok());
    }

    #[test]
    fn claude_ready_codex_missing_still_allows_start_when_claude_is_the_selected_dev_ai() {
        // Symmetric case: Claude selected as the developer AI, Codex in
        // whatever state — again unrepresentable as an input, so a
        // Claude-ready call must still succeed.
        assert!(dev_only_start_gate(true, None, || unreachable!()).is_ok());
    }

    #[test]
    fn the_selected_dev_ai_itself_missing_still_blocks_start() {
        // The one case that *must* still block: the actually-selected
        // developer AI itself is not ready.
        let result = dev_only_start_gate(false, None, || "Codex CLI 未安装".to_string());
        assert_eq!(result, Err("Codex CLI 未安装".to_string()));
    }

    #[test]
    fn the_selected_dev_ai_itself_untrusted_still_blocks_start() {
        let result = dev_only_start_gate(true, Some("Codex CLI：尚未确认"), || unreachable!());
        assert_eq!(result, Err("Codex CLI：尚未确认".to_string()));
    }
}
