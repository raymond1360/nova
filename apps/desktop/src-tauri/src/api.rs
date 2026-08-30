use futures_util::StreamExt;
use reqwest::header::HeaderMap;
use serde::Deserialize;
use serde_json::json;
use tokio_util::sync::CancellationToken;

use crate::error::{NovaError, NovaResult};
use crate::models::{ChatMessage, ModelInfo, TokenUsage};
use crate::security::{mask_request_id, redact_sensitive_text};
use crate::validation::{endpoint_url, AllowedEndpoint};

#[derive(Debug, Clone)]
pub struct ApiProvider {
    pub base_url: String,
    pub api_key: String,
}

#[derive(Debug, Clone)]
pub struct ApiErrorDetail {
    pub status: Option<u16>,
    pub code: Option<String>,
    pub message: String,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StreamChunk {
    pub delta: String,
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    Delta(String),
    Done,
}

pub async fn fetch_models(
    client: &reqwest::Client,
    provider: &ApiProvider,
) -> NovaResult<Vec<ModelInfo>> {
    let url = endpoint_url(&provider.base_url, AllowedEndpoint::Models)?;
    let response = client
        .get(url)
        .bearer_auth(&provider.api_key)
        .send()
        .await
        .map_err(map_network_error)?;
    let request_id = response_request_id(response.headers());
    if !response.status().is_success() {
        return Err(NovaError::Api(
            map_api_error(response, request_id).await.message,
        ));
    }
    let payload = response
        .json::<ModelsResponse>()
        .await
        .map_err(|_| NovaError::Api("模型接口返回结构异常".to_string()))?;
    Ok(payload
        .data
        .into_iter()
        .filter(|item| !item.id.trim().is_empty())
        .map(|item| ModelInfo {
            id: item.id,
            object: item.object,
            owned_by: item.owned_by,
        })
        .collect())
}

pub async fn fetch_token_usage(
    client: &reqwest::Client,
    provider: &ApiProvider,
) -> NovaResult<TokenUsage> {
    let url = endpoint_url(&provider.base_url, AllowedEndpoint::TokenUsage)?;
    let response = client
        .get(url)
        .bearer_auth(&provider.api_key)
        .send()
        .await
        .map_err(map_network_error)?;
    let request_id = response_request_id(response.headers());
    if !response.status().is_success() {
        return Err(NovaError::Api(
            map_api_error(response, request_id).await.message,
        ));
    }
    let value = response
        .json::<serde_json::Value>()
        .await
        .map_err(|_| NovaError::Api("Token 用量接口返回结构异常".to_string()))?;
    parse_token_usage(value)
}

pub async fn stream_chat<F>(
    client: reqwest::Client,
    provider: ApiProvider,
    model_id: String,
    messages: Vec<ChatMessage>,
    cancel: CancellationToken,
    mut on_chunk: F,
) -> Result<Option<String>, ApiErrorDetail>
where
    F: FnMut(StreamChunk) + Send + 'static,
{
    let body_messages = messages
        .into_iter()
        .filter(|message| matches!(message.role.as_str(), "user" | "assistant" | "system"))
        .filter(|message| !(message.role == "assistant" && message.content.trim().is_empty()))
        .map(|message| json!({ "role": message.role, "content": message.content }))
        .collect::<Vec<_>>();
    let body = json!({
        "model": model_id,
        "messages": body_messages,
        "stream": true
    });

    let mut attempts = 0;
    loop {
        attempts += 1;
        match stream_chat_once(&client, &provider, &body, cancel.clone(), &mut on_chunk).await {
            Err(detail)
                if attempts == 1
                    && detail.status.is_none()
                    && !cancel.is_cancelled()
                    && detail.code.as_deref() != Some("stream_started") =>
            {
                continue;
            }
            result => return result,
        }
    }
}

async fn stream_chat_once<F>(
    client: &reqwest::Client,
    provider: &ApiProvider,
    body: &serde_json::Value,
    cancel: CancellationToken,
    on_chunk: &mut F,
) -> Result<Option<String>, ApiErrorDetail>
where
    F: FnMut(StreamChunk) + Send + 'static,
{
    let url = endpoint_url(&provider.base_url, AllowedEndpoint::ChatCompletions)
        .map_err(to_api_detail)?;
    let response = client
        .post(url)
        .bearer_auth(&provider.api_key)
        .json(body)
        .send()
        .await
        .map_err(|error| ApiErrorDetail {
            status: None,
            code: None,
            message: map_network_error(error).public_message(),
            request_id: None,
        })?;
    let request_id = response_request_id(response.headers());
    if !response.status().is_success() {
        return Err(map_api_error(response, request_id).await);
    }

    let mut parser = SseParser::default();
    let mut stream = response.bytes_stream();
    let mut saw_done = false;
    let mut saw_delta = false;
    while let Some(item) = stream.next().await {
        if cancel.is_cancelled() {
            return Err(ApiErrorDetail {
                status: None,
                code: Some("user_canceled".to_string()),
                message: "生成已停止，已保留收到的内容".to_string(),
                request_id,
            });
        }
        let bytes = item.map_err(|error| ApiErrorDetail {
            status: None,
            code: if saw_delta {
                Some("stream_started".to_string())
            } else {
                None
            },
            message: map_network_error(error).public_message(),
            request_id: request_id.clone(),
        })?;
        let text = String::from_utf8_lossy(&bytes);
        for event in parser.push(&text).map_err(|message| ApiErrorDetail {
            status: None,
            code: Some("sse_parse_error".to_string()),
            message,
            request_id: request_id.clone(),
        })? {
            match event {
                StreamEvent::Delta(delta) => {
                    saw_delta = true;
                    on_chunk(StreamChunk {
                        delta,
                        request_id: request_id.clone(),
                    });
                }
                StreamEvent::Done => {
                    saw_done = true;
                    break;
                }
            }
        }
        if saw_done {
            break;
        }
    }
    if !saw_done {
        return Err(ApiErrorDetail {
            status: None,
            code: Some("sse_incomplete".to_string()),
            message: "流式响应中断，已保留收到的内容".to_string(),
            request_id,
        });
    }
    Ok(request_id)
}

#[derive(Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamEvent>, String> {
        self.buffer.push_str(chunk);
        let normalized = self.buffer.replace("\r\n", "\n");
        let mut parts = normalized
            .split("\n\n")
            .map(str::to_string)
            .collect::<Vec<_>>();
        self.buffer = parts.pop().unwrap_or_default();
        let mut events = Vec::new();
        for part in parts {
            let data_lines = part
                .lines()
                .filter_map(|line| line.strip_prefix("data:").map(str::trim))
                .collect::<Vec<_>>();
            if data_lines.is_empty() {
                continue;
            }
            let data = data_lines.join("\n");
            if data == "[DONE]" {
                events.push(StreamEvent::Done);
                continue;
            }
            let value: serde_json::Value =
                serde_json::from_str(&data).map_err(|_| "SSE data 不是有效 JSON".to_string())?;
            if let Some(delta) = value
                .pointer("/choices/0/delta/content")
                .and_then(serde_json::Value::as_str)
            {
                events.push(StreamEvent::Delta(delta.to_string()));
            }
        }
        Ok(events)
    }
}

fn parse_token_usage(value: serde_json::Value) -> NovaResult<TokenUsage> {
    let data = value.get("data").unwrap_or(&value);
    let quota = |keys: &[&str]| -> f64 {
        keys.iter()
            .find_map(|key| data.get(key).and_then(serde_json::Value::as_f64))
            .unwrap_or(0.0)
    };
    Ok(TokenUsage {
        name: data
            .get("name")
            .or_else(|| data.get("token_name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("当前 Token")
            .to_string(),
        total_used: quota(&["total_used", "used_quota", "used"]),
        total_available: quota(&[
            "total_available",
            "remain_quota",
            "available_quota",
            "available",
        ]),
        unlimited_quota: data
            .get("unlimited_quota")
            .or_else(|| data.get("unlimited"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
        expires_at: data
            .get("expires_at")
            .or_else(|| data.get("expired_time"))
            .and_then(|value| {
                if value.is_null() {
                    None
                } else if let Some(text) = value.as_str() {
                    Some(text.to_string())
                } else {
                    Some(value.to_string())
                }
            }),
    })
}

async fn map_api_error(response: reqwest::Response, request_id: Option<String>) -> ApiErrorDetail {
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    let value = serde_json::from_str::<serde_json::Value>(&text).ok();
    let code = value
        .as_ref()
        .and_then(|value| value.pointer("/error/code").or_else(|| value.get("code")))
        .and_then(|value| {
            value
                .as_str()
                .map(str::to_string)
                .or_else(|| value.as_i64().map(|number| number.to_string()))
        });
    let raw_message = value
        .as_ref()
        .and_then(|value| {
            value
                .pointer("/error/message")
                .or_else(|| value.get("message"))
        })
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&text);
    ApiErrorDetail {
        status: Some(status),
        code: code.clone(),
        message: classify_error(status, code.as_deref(), raw_message),
        request_id: request_id.map(|id| mask_request_id(&id)),
    }
}

fn map_network_error(error: reqwest::Error) -> NovaError {
    if error.is_timeout() {
        NovaError::Network("请求超时".to_string())
    } else if error.is_connect() {
        NovaError::Network("无法连接 SDKMAX".to_string())
    } else {
        NovaError::Network(redact_sensitive_text(&error.to_string()))
    }
}

fn classify_error(status: u16, code: Option<&str>, message: &str) -> String {
    match (status, code) {
        (401, _) => "API Key 无效或已过期".to_string(),
        (403, _) => "账号、Token 或模型权限不可用".to_string(),
        (429, Some("insufficient_user_quota")) => {
            "余额不足，请前往 SDKMAX 处理账户额度".to_string()
        }
        (429, _) => "请求过于频繁，请稍后重试".to_string(),
        (_, Some("model_not_found")) => "模型不存在或当前 Token 不可调用".to_string(),
        (_, Some("get_channel_failed")) => "SDKMAX 暂时没有可用渠道，请稍后重试".to_string(),
        (_, Some("insufficient_user_quota")) => "余额不足，请前往 SDKMAX 处理账户额度".to_string(),
        _ => {
            let cleaned = redact_sensitive_text(message);
            if cleaned.is_empty() {
                format!("SDKMAX 返回错误：HTTP {status}")
            } else {
                cleaned
            }
        }
    }
}

fn response_request_id(headers: &HeaderMap) -> Option<String> {
    for name in ["x-request-id", "x-oneapi-request-id", "request-id"] {
        if let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) {
            return Some(mask_request_id(value));
        }
    }
    None
}

fn to_api_detail(error: NovaError) -> ApiErrorDetail {
    ApiErrorDetail {
        status: None,
        code: None,
        message: error.public_message(),
        request_id: None,
    }
}

#[derive(Debug, Deserialize)]
struct ModelsResponse {
    data: Vec<ModelItem>,
}

#[derive(Debug, Deserialize)]
struct ModelItem {
    id: String,
    object: Option<String>,
    owned_by: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_openai_model_list() {
        let payload =
            r#"{"object":"list","data":[{"id":"gpt-a","object":"model","owned_by":"sdkmax"}]}"#;
        let response: ModelsResponse = serde_json::from_str(payload).unwrap();
        assert_eq!(response.data[0].id, "gpt-a");
    }

    #[test]
    fn parses_sse_across_chunks_and_done() {
        let mut parser = SseParser::default();
        let first = parser
            .push("data: {\"choices\":[{\"delta\":{\"content\":\"你")
            .unwrap();
        assert!(first.is_empty());
        let second = parser.push("好\"}}]}\n\ndata: [DONE]\n\n").unwrap();
        assert_eq!(
            second,
            vec![StreamEvent::Delta("你好".to_string()), StreamEvent::Done]
        );
    }

    #[test]
    fn rejects_invalid_sse_json() {
        let mut parser = SseParser::default();
        assert!(parser.push("data: not-json\n\n").is_err());
    }

    #[test]
    fn maps_known_errors() {
        assert_eq!(classify_error(401, None, ""), "API Key 无效或已过期");
        assert_eq!(
            classify_error(429, Some("insufficient_user_quota"), ""),
            "余额不足，请前往 SDKMAX 处理账户额度"
        );
        assert_eq!(
            classify_error(500, Some("get_channel_failed"), ""),
            "SDKMAX 暂时没有可用渠道，请稍后重试"
        );
    }

    #[test]
    fn parses_token_usage_shapes() {
        let usage = parse_token_usage(json!({
            "success": true,
            "data": {
                "name": "nova-desktop",
                "used_quota": 1.0,
                "remain_quota": 9.0,
                "unlimited_quota": false,
                "expires_at": null
            }
        }))
        .unwrap();
        assert_eq!(usage.name, "nova-desktop");
        assert_eq!(usage.total_available, 9.0);
    }
}
