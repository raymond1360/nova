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
    pub received_delta: bool,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseParseError {
    message: String,
    events: Vec<StreamEvent>,
}

impl SseParseError {
    fn emitted_delta(&self) -> bool {
        self.events
            .iter()
            .any(|event| matches!(event, StreamEvent::Delta(_)))
    }
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
                    && !detail.received_delta =>
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
            received_delta: false,
        })?;
    let request_id = response_request_id(response.headers());
    if !response.status().is_success() {
        return Err(map_api_error(response, request_id).await);
    }

    let mut parser = SseParser::default();
    let mut stream = response.bytes_stream();
    let mut saw_done = false;
    let mut saw_delta = false;
    loop {
        let item = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                return Err(ApiErrorDetail {
                    status: None,
                    code: Some("user_canceled".to_string()),
                    message: "生成已停止，已保留收到的内容".to_string(),
                    request_id,
                    received_delta: saw_delta,
                });
            }
            item = stream.next() => item,
        };
        let Some(item) = item else {
            break;
        };
        let bytes = item.map_err(|error| ApiErrorDetail {
            status: None,
            code: None,
            message: map_network_error(error).public_message(),
            request_id: request_id.clone(),
            received_delta: saw_delta,
        })?;
        let text = String::from_utf8_lossy(&bytes);
        let events = match parser.push(&text) {
            Ok(events) => events,
            Err(error) => {
                let emitted_delta = error.emitted_delta();
                for event in error.events {
                    if let StreamEvent::Delta(delta) = event {
                        saw_delta = true;
                        on_chunk(StreamChunk {
                            delta,
                            request_id: request_id.clone(),
                        });
                    }
                }
                return Err(ApiErrorDetail {
                    status: None,
                    code: Some("sse_parse_error".to_string()),
                    message: error.message,
                    request_id: request_id.clone(),
                    received_delta: saw_delta || emitted_delta,
                });
            }
        };
        for event in events {
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
            message: "回答中断，可重新生成".to_string(),
            request_id,
            received_delta: saw_delta,
        });
    }
    Ok(request_id)
}

#[derive(Default)]
pub struct SseParser {
    buffer: String,
}

impl SseParser {
    pub fn push(&mut self, chunk: &str) -> Result<Vec<StreamEvent>, SseParseError> {
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
                serde_json::from_str(&data).map_err(|_| SseParseError {
                    message: "SSE data 不是有效 JSON".to_string(),
                    events: events.clone(),
                })?;
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
        received_delta: false,
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
        received_delta: false,
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
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use super::*;

    enum MockResponse {
        CloseImmediately,
        Body(String),
        HeadersThenSleep(Duration),
    }

    struct MockServer {
        base_url: String,
        requests: Arc<AtomicUsize>,
    }

    fn chat_message(content: &str) -> ChatMessage {
        ChatMessage {
            id: "user-1".to_string(),
            conversation_id: "conversation-1".to_string(),
            role: "user".to_string(),
            content: content.to_string(),
            provider_id: "sdkmax".to_string(),
            model_id: "gpt-test".to_string(),
            status: "complete".to_string(),
            request_id: None,
            created_at: "2026-08-30T00:00:00Z".to_string(),
        }
    }

    fn sse_delta(text: &str) -> String {
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n")
    }

    fn start_mock_server(responses: Vec<MockResponse>) -> MockServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let responses = Arc::new(Mutex::new(responses));
        let request_counter = requests.clone();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                drain_headers(&mut stream);
                request_counter.fetch_add(1, Ordering::SeqCst);
                let response = responses.lock().unwrap().remove(0);
                match response {
                    MockResponse::CloseImmediately => {}
                    MockResponse::Body(body) => write_sse_response(&mut stream, &body),
                    MockResponse::HeadersThenSleep(delay) => {
                        write_headers(&mut stream);
                        thread::sleep(delay);
                    }
                }
                if responses.lock().unwrap().is_empty() {
                    break;
                }
            }
        });
        MockServer {
            base_url: format!("http://{}", addr),
            requests,
        }
    }

    fn drain_headers(stream: &mut TcpStream) {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut buffer = [0_u8; 1024];
        let mut collected = Vec::new();
        while !collected.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            collected.extend_from_slice(&buffer[..read]);
        }
    }

    fn write_headers(stream: &mut TcpStream) {
        stream
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        stream.flush().unwrap();
    }

    fn write_sse_response(stream: &mut TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    async fn run_stream(
        server: &MockServer,
        chunks: Arc<Mutex<Vec<String>>>,
    ) -> Result<Option<String>, ApiErrorDetail> {
        stream_chat(
            reqwest::Client::new(),
            ApiProvider {
                base_url: server.base_url.clone(),
                api_key: "test-key".to_string(),
            },
            "gpt-test".to_string(),
            vec![chat_message("hello")],
            CancellationToken::new(),
            move |chunk| chunks.lock().unwrap().push(chunk.delta),
        )
        .await
    }

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

    #[tokio::test]
    async fn streams_normal_delta_and_done() {
        let body = format!("{}data: [DONE]\n\n", sse_delta("hello"));
        let server = start_mock_server(vec![MockResponse::Body(body)]);
        let chunks = Arc::new(Mutex::new(Vec::new()));
        let result = run_stream(&server, chunks.clone()).await.unwrap();
        assert_eq!(result, None);
        assert_eq!(*chunks.lock().unwrap(), vec!["hello".to_string()]);
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_once_when_temporary_failure_has_no_delta() {
        let body = format!("{}data: [DONE]\n\n", sse_delta("retry-ok"));
        let server = start_mock_server(vec![
            MockResponse::CloseImmediately,
            MockResponse::Body(body),
        ]);
        let chunks = Arc::new(Mutex::new(Vec::new()));
        let result = run_stream(&server, chunks.clone()).await.unwrap();
        assert_eq!(result, None);
        assert_eq!(*chunks.lock().unwrap(), vec!["retry-ok".to_string()]);
        assert_eq!(server.requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn does_not_retry_when_stream_breaks_after_delta() {
        let body = sse_delta("partial");
        let server = start_mock_server(vec![MockResponse::Body(body)]);
        let chunks = Arc::new(Mutex::new(Vec::new()));
        let error = run_stream(&server, chunks.clone()).await.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("sse_incomplete"));
        assert!(error.received_delta);
        assert_eq!(error.message, "回答中断，可重新生成");
        assert_eq!(*chunks.lock().unwrap(), vec!["partial".to_string()]);
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn malformed_sse_after_delta_does_not_retry() {
        let body = format!("{}data: not-json\n\n", sse_delta("partial"));
        let server = start_mock_server(vec![MockResponse::Body(body)]);
        let chunks = Arc::new(Mutex::new(Vec::new()));
        let error = run_stream(&server, chunks.clone()).await.unwrap_err();
        assert_eq!(error.code.as_deref(), Some("sse_parse_error"));
        assert!(error.received_delta);
        assert_eq!(*chunks.lock().unwrap(), vec!["partial".to_string()]);
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn blocking_stream_can_be_cancelled_immediately_and_does_not_retry() {
        let server =
            start_mock_server(vec![MockResponse::HeadersThenSleep(Duration::from_secs(5))]);
        let chunks = Arc::new(Mutex::new(Vec::new()));
        let cancel = CancellationToken::new();
        let cancel_for_task = cancel.clone();
        let chunks_for_task = chunks.clone();
        let base_url = server.base_url.clone();
        let task = tokio::spawn(async move {
            stream_chat(
                reqwest::Client::new(),
                ApiProvider {
                    base_url,
                    api_key: "test-key".to_string(),
                },
                "gpt-test".to_string(),
                vec![chat_message("hello")],
                cancel_for_task,
                move |chunk| chunks_for_task.lock().unwrap().push(chunk.delta),
            )
            .await
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        cancel.cancel();
        let error = tokio::time::timeout(Duration::from_millis(500), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.code.as_deref(), Some("user_canceled"));
        assert_eq!(server.requests.load(Ordering::SeqCst), 1);
        assert!(chunks.lock().unwrap().is_empty());
    }
}
