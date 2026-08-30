use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

use crate::api::{self, ApiProvider, StreamChunk};
use crate::error::{public_error, NovaError, NovaResult};
use crate::models::{
    ChatMessage, Conversation, ModelInfo, ProviderSummary, SaveProviderRequest, SendChatRequest,
    SendChatResult, TestConnectionResult, TokenUsage,
};
use crate::security::redact_sensitive_text;
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
                    let _ = db.log_request(
                        &provider_id,
                        Some(&model_id),
                        AllowedEndpoint::ChatCompletions.path(),
                        Some(200),
                        None,
                        None,
                        request_id.as_deref(),
                    );
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
                let status = if detail.code.as_deref() == Some("user_canceled")
                    || detail.code.as_deref() == Some("sse_incomplete")
                {
                    "interrupted"
                } else {
                    "error"
                };
                let public_message = redact_sensitive_text(&detail.message);
                if let Ok(db) = db_for_finish.lock() {
                    let _ = db.update_message_status(
                        &assistant_id_for_stream,
                        status,
                        detail.request_id.as_deref(),
                    );
                    let _ = db.log_request(
                        &provider_id,
                        Some(&model_id),
                        AllowedEndpoint::ChatCompletions.path(),
                        detail.status,
                        detail.code.as_deref(),
                        Some(&public_message),
                        detail.request_id.as_deref(),
                    );
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
