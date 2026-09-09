//! Provider HTTP boundary tests.
//!
//! The AI providers speak three real HTTP dialects: OpenAI chat
//! completions, Anthropic messages, and the Ollama generate API. Each test
//! points a provider at a local scripted HTTP server through
//! `ProviderConfig::base_url` and serves realistic wire-format responses,
//! so request envelopes, auth headers, status-to-error mapping
//! (429/401/5xx), and response parsing all run over real HTTP without
//! calling the paid APIs.

use std::collections::VecDeque;
use std::sync::Arc;

use gitforge_ai::{
    AiError, AiProvider, ChangeType, FileChange, FindingCategory, OpenAiProvider, ProviderConfig,
    ProviderType, ReviewRequest, ReviewResponse, Severity,
};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// A request the provider sent to the scripted server.
#[derive(Debug, Clone)]
struct RecordedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl RecordedRequest {
    fn header(&self, name: &str) -> &str {
        self.headers
            .iter()
            .find(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
            .unwrap_or_else(|| panic!("request carried no {name} header"))
    }
}

/// A local HTTP server that hands out one scripted response per connection
/// and records every request it receives.
struct ScriptedServer {
    addr: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Drop for ScriptedServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

impl ScriptedServer {
    async fn spawn(responses: Vec<(u16, String)>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind scripted provider server");
        let addr = listener.local_addr().expect("scripted server address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&requests);
        let mut queue = VecDeque::from(responses);
        let handle = tokio::spawn(async move {
            while let Some((status, body)) = queue.pop_front() {
                let Ok((stream, _peer)) = listener.accept().await else {
                    break;
                };
                serve_one(stream, status, body, Arc::clone(&recorded)).await;
            }
        });
        Self {
            addr,
            requests,
            handle,
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    async fn last_request(&self) -> RecordedRequest {
        self.requests
            .lock()
            .await
            .last()
            .cloned()
            .expect("server received a request")
    }
}

/// Serve one scripted response, recording the request it answers.
async fn serve_one(
    stream: tokio::net::TcpStream,
    status: u16,
    body: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
) {
    let mut reader = BufReader::new(stream);

    let mut request_line = String::new();
    if reader.read_line(&mut request_line).await.is_err() {
        return;
    }
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).await.is_err() {
            return;
        }
        let line = line.trim_end().to_string();
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            if name == "content-length" {
                content_length = value.trim().parse().unwrap_or(0);
            }
            headers.push((name, value.trim().to_string()));
        }
    }

    let mut body_bytes = vec![0u8; content_length];
    if content_length > 0 && reader.read_exact(&mut body_bytes).await.is_err() {
        return;
    }
    requests.lock().await.push(RecordedRequest {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body_bytes).to_string(),
    });

    let mut stream = reader.into_inner();
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "OK",
    };
    let response = format!(
        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    if stream.write_all(response.as_bytes()).await.is_err() {
        return;
    }
    let _ = stream.shutdown().await;
}

// ─── Shared fixtures ─────────────────────────────────────────────────────

/// Review JSON a model would place inside its message content, exercising
/// both the mapped labels and the fallback for unknown ones.
const REVIEW_JSON: &str = r#"{
  "summary": "Small refactor with one critical issue",
  "overall_score": 82,
  "findings": [
    {
      "file": "src/lib.rs",
      "line_start": 10,
      "line_end": 15,
      "severity": "critical",
      "category": "security",
      "title": "Hard-coded credential",
      "description": "The token is embedded in source.",
      "suggestion": "Load it from the environment.",
      "code_snippet": "let token = \"harness\";"
    },
    {
      "file": "src/other.rs",
      "line_start": null,
      "line_end": null,
      "severity": "some-new-severity",
      "category": "mystery",
      "title": "Unmapped severity and category",
      "description": "Unknown labels must fall back safely."
    }
  ]
}"#;

const OPENAI_KEY_ENV: &str = "GITFORGE_TEST_OPENAI_WIRE_KEY";
const ANTHROPIC_KEY_ENV: &str = "GITFORGE_TEST_ANTHROPIC_WIRE_KEY";

fn review_request() -> ReviewRequest {
    ReviewRequest::new(
        "harness-repo",
        "feature/provider-tests",
        vec![FileChange {
            path: "src/lib.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: "-fn old() {}\n+fn new() {}\n".to_string(),
            language: Some("rust".to_string()),
        }],
    )
    .with_context("provider http harness")
}

fn openai_provider(base_url: &str) -> OpenAiProvider {
    std::env::set_var(OPENAI_KEY_ENV, "harness-openai-key");
    OpenAiProvider::new(ProviderConfig {
        api_key_env: OPENAI_KEY_ENV.to_string(),
        base_url: Some(base_url.to_string()),
        organization: None,
    })
    .expect("construct openai provider")
    .with_model("gpt-4o")
}

/// Assert a provider call failed and return the error for message checks.
fn expect_error<T>(result: Result<T, AiError>) -> AiError {
    match result {
        Ok(_) => panic!("expected an error, got an ok result"),
        Err(err) => err,
    }
}

fn assert_review_shape(response: &ReviewResponse, expected_provider: ProviderType, model: &str) {
    assert_eq!(response.summary, "Small refactor with one critical issue");
    assert_eq!(response.overall_score, 82);
    assert_eq!(response.provider, expected_provider);
    assert_eq!(response.model, model);
    assert!(response.has_critical_findings());

    assert_eq!(response.findings.len(), 2);
    let critical = &response.findings[0];
    assert_eq!(critical.severity, Severity::Critical);
    assert_eq!(critical.category, FindingCategory::Security);
    assert_eq!(critical.file, "src/lib.rs");
    assert_eq!(critical.line_start, Some(10));
    assert_eq!(critical.line_end, Some(15));
    assert_eq!(
        critical.suggestion.as_deref(),
        Some("Load it from the environment.")
    );

    // Unknown labels fall back instead of failing the review.
    let unknown = &response.findings[1];
    assert_eq!(unknown.severity, Severity::Info);
    assert_eq!(unknown.category, FindingCategory::BestPractice);
    assert_eq!(unknown.line_start, None);
    assert_eq!(unknown.suggestion, None);
}

// ─── OpenAI ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn test_openai_health_check_sends_bearer_and_organization() {
    let server =
        ScriptedServer::spawn(vec![(200, r#"{"object":"list","data":[]}"#.to_string())]).await;
    let provider = {
        std::env::set_var(OPENAI_KEY_ENV, "harness-openai-key");
        OpenAiProvider::new(ProviderConfig {
            api_key_env: OPENAI_KEY_ENV.to_string(),
            base_url: Some(server.base_url()),
            organization: Some("org-harness".to_string()),
        })
        .expect("construct openai provider")
    };

    provider
        .health_check()
        .await
        .expect("health check against a healthy endpoint");

    let request = server.last_request().await;
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/models");
    assert_eq!(request.header("authorization"), "Bearer harness-openai-key");
    assert_eq!(request.header("openai-organization"), "org-harness");
}

#[tokio::test]
async fn test_openai_health_check_rejects_invalid_key() {
    let server = ScriptedServer::spawn(vec![(401, r#"{"error":{}}"#.to_string())]).await;
    let provider = openai_provider(&server.base_url());

    let err = expect_error(provider.health_check().await);
    assert!(
        matches!(err, AiError::Auth(ref message) if message.contains("validation failed")),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_openai_generate_review_parses_real_envelope() {
    let envelope = serde_json::json!({
        "id": "chatcmpl-harness",
        "object": "chat.completion",
        "created": 1_757_400_000u64,
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": REVIEW_JSON},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 1000, "completion_tokens": 500, "total_tokens": 1500}
    })
    .to_string();
    let server = ScriptedServer::spawn(vec![(200, envelope)]).await;
    let provider = openai_provider(&server.base_url());

    let response = provider
        .generate_review(&review_request())
        .await
        .expect("review from scripted openai endpoint");

    assert_review_shape(&response, ProviderType::OpenAI, "gpt-4o");
    assert_eq!(response.tokens_used, 1500);
    // (1000/1e6 * $5 + 500/1e6 * $15) * 100 = 1.25 cents, truncated to 1.
    assert_eq!(response.cost_cents, 1);

    let request = server.last_request().await;
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/chat/completions");
    assert_eq!(request.header("authorization"), "Bearer harness-openai-key");
    let sent: serde_json::Value =
        serde_json::from_str(&request.body).expect("parse sent request body");
    assert_eq!(sent["model"], "gpt-4o");
    assert_eq!(sent["max_tokens"], 4096);
    assert_eq!(sent["temperature"], 0.3);
    let prompt = sent["messages"][0]["content"].as_str().expect("prompt");
    assert!(prompt.contains("harness-repo"));
    assert!(prompt.contains("src/lib.rs"));
}

#[tokio::test]
async fn test_openai_generate_review_maps_error_statuses() {
    for (status, expected) in [(429, "rate limit"), (401, "auth"), (500, "api")] {
        let body = serde_json::json!({"error": {"message": "upstream says no"}}).to_string();
        let server = ScriptedServer::spawn(vec![(status, body)]).await;
        let provider = openai_provider(&server.base_url());

        let err = expect_error(provider.generate_review(&review_request()).await);
        match (status, &err) {
            (429, AiError::RateLimit) => {}
            (401, AiError::Auth(_)) => {}
            (500, AiError::Api(message)) => {
                assert!(message.contains("upstream says no"), "message: {message}");
            }
            _ => panic!("status {status} mapped to the wrong error: {err:?}"),
        }
        let _ = expected;
    }
}

#[tokio::test]
async fn test_openai_generate_review_rejects_unparsable_content() {
    let envelope = serde_json::json!({
        "id": "chatcmpl-harness",
        "object": "chat.completion",
        "created": 1u64,
        "model": "gpt-4o",
        "choices": [{
            "index": 0,
            "message": {"role": "assistant", "content": "the diff looks fine, ship it"},
            "finish_reason": "stop"
        }],
        "usage": {"prompt_tokens": 10, "completion_tokens": 5, "total_tokens": 15}
    })
    .to_string();
    let server = ScriptedServer::spawn(vec![(200, envelope)]).await;
    let provider = openai_provider(&server.base_url());

    let err = expect_error(provider.generate_review(&review_request()).await);
    assert!(
        matches!(err, AiError::Parse(ref message) if message.contains("Failed to parse review JSON")),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_openai_generate_review_rejects_empty_choices() {
    let envelope = serde_json::json!({
        "id": "chatcmpl-harness",
        "object": "chat.completion",
        "created": 1u64,
        "model": "gpt-4o",
        "choices": []
    })
    .to_string();
    let server = ScriptedServer::spawn(vec![(200, envelope)]).await;
    let provider = openai_provider(&server.base_url());

    let err = expect_error(provider.generate_review(&review_request()).await);
    assert!(
        matches!(err, AiError::Parse(ref message) if message.contains("No content in response")),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_openai_provider_requires_configured_key_env() {
    let env_name = "GITFORGE_TEST_OPENAI_KEY_DEFINITELY_UNSET";
    std::env::remove_var(env_name);

    let err = expect_error(OpenAiProvider::new(ProviderConfig {
        api_key_env: env_name.to_string(),
        base_url: None,
        organization: None,
    }));
    assert!(
        matches!(err, AiError::Config(ref message) if message.contains(env_name)),
        "unexpected error: {err:?}"
    );
}

// ─── Anthropic ───────────────────────────────────────────────────────────

fn anthropic_provider(base_url: &str) -> gitforge_ai::AnthropicProvider {
    std::env::set_var(ANTHROPIC_KEY_ENV, "harness-anthropic-key");
    gitforge_ai::AnthropicProvider::new(ProviderConfig {
        api_key_env: ANTHROPIC_KEY_ENV.to_string(),
        base_url: Some(base_url.to_string()),
        organization: None,
    })
    .expect("construct anthropic provider")
    .with_model("claude-3-5-sonnet-20241022")
}

/// The message object the real Anthropic API returns.
fn anthropic_message(content: serde_json::Value) -> String {
    serde_json::json!({
        "id": "msg_harness",
        "type": "message",
        "role": "assistant",
        "model": "claude-3-5-sonnet-20241022",
        "content": content,
        "stop_reason": "end_turn",
        "usage": {"input_tokens": 1000, "output_tokens": 500}
    })
    .to_string()
}

#[tokio::test]
async fn test_anthropic_health_check_sends_api_key_headers() {
    let server = ScriptedServer::spawn(vec![(
        200,
        anthropic_message(serde_json::json!([
            {"type": "text", "text": "hi"}
        ])),
    )])
    .await;
    let provider = anthropic_provider(&server.base_url());

    provider
        .health_check()
        .await
        .expect("health check against a healthy endpoint");

    let request = server.last_request().await;
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/messages");
    assert_eq!(request.header("x-api-key"), "harness-anthropic-key");
    assert_eq!(request.header("anthropic-version"), "2023-06-01");
}

#[tokio::test]
async fn test_anthropic_health_check_rejects_invalid_key() {
    let server = ScriptedServer::spawn(vec![(403, r#"{"error":{}}"#.to_string())]).await;
    let provider = anthropic_provider(&server.base_url());

    let err = expect_error(provider.health_check().await);
    assert!(
        matches!(err, AiError::Auth(ref message) if message.contains("validation failed")),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_anthropic_generate_review_parses_real_envelope() {
    let server = ScriptedServer::spawn(vec![(
        200,
        anthropic_message(serde_json::json!([
            {"type": "text", "text": REVIEW_JSON}
        ])),
    )])
    .await;
    let provider = anthropic_provider(&server.base_url());

    let response = provider
        .generate_review(&review_request())
        .await
        .expect("review from scripted anthropic endpoint");

    assert_review_shape(
        &response,
        ProviderType::Anthropic,
        "claude-3-5-sonnet-20241022",
    );
    assert_eq!(response.tokens_used, 1500);
    // Sonnet: (1000/1e6 * $3 + 500/1e6 * $15) * 100 = 1.05, truncated to 1.
    assert_eq!(response.cost_cents, 1);

    let request = server.last_request().await;
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/v1/messages");
    assert_eq!(request.header("x-api-key"), "harness-anthropic-key");
    let sent: serde_json::Value =
        serde_json::from_str(&request.body).expect("parse sent request body");
    assert_eq!(sent["model"], "claude-3-5-sonnet-20241022");
    assert_eq!(sent["max_tokens"], 4096);
    let prompt = sent["messages"][0]["content"].as_str().expect("prompt");
    assert!(prompt.contains("harness-repo"));
}

#[tokio::test]
async fn test_anthropic_generate_review_maps_error_statuses() {
    for status in [429u16, 401, 500] {
        let body = serde_json::json!({"type": "error", "error": {"message": "nope"}}).to_string();
        let server = ScriptedServer::spawn(vec![(status, body)]).await;
        let provider = anthropic_provider(&server.base_url());

        let err = expect_error(provider.generate_review(&review_request()).await);
        match (status, &err) {
            (429, AiError::RateLimit) => {}
            (401, AiError::Auth(_)) => {}
            (500, AiError::Api(message)) => {
                assert!(message.contains("nope"), "message: {message}");
            }
            _ => panic!("status {status} mapped to the wrong error: {err:?}"),
        }
    }
}

#[tokio::test]
async fn test_anthropic_generate_review_rejects_response_without_text() {
    // An error content block carries no reviewable text.
    let server = ScriptedServer::spawn(vec![(
        200,
        anthropic_message(serde_json::json!([
            {"type": "error", "text": "overloaded_error"}
        ])),
    )])
    .await;
    let provider = anthropic_provider(&server.base_url());

    let err = expect_error(provider.generate_review(&review_request()).await);
    assert!(
        matches!(err, AiError::Parse(ref message) if message.contains("No text content")),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_anthropic_generate_review_rejects_unparsable_content() {
    let server = ScriptedServer::spawn(vec![(
        200,
        anthropic_message(serde_json::json!([
            {"type": "text", "text": "looks good to me"}
        ])),
    )])
    .await;
    let provider = anthropic_provider(&server.base_url());

    let err = expect_error(provider.generate_review(&review_request()).await);
    assert!(
        matches!(err, AiError::Parse(ref message) if message.contains("Failed to parse review JSON")),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_anthropic_provider_requires_configured_key_env() {
    let env_name = "GITFORGE_TEST_ANTHROPIC_KEY_DEFINITELY_UNSET";
    std::env::remove_var(env_name);

    let err = expect_error(gitforge_ai::AnthropicProvider::new(ProviderConfig {
        api_key_env: env_name.to_string(),
        base_url: None,
        organization: None,
    }));
    assert!(
        matches!(err, AiError::Config(ref message) if message.contains(env_name)),
        "unexpected error: {err:?}"
    );
}

// ─── Ollama ──────────────────────────────────────────────────────────────

fn ollama_provider(base_url: &str) -> gitforge_ai::OllamaProvider {
    gitforge_ai::OllamaProvider::new(ProviderConfig::ollama(base_url))
        .expect("construct ollama provider")
        .with_model("qwen3:8b")
}

fn ollama_response(response: &str) -> String {
    serde_json::json!({
        "model": "qwen3:8b",
        "response": response,
        "done": true,
        "context": [1, 2, 3],
        "total_duration": 1_000_000u64,
        "load_duration": 100u64,
        "prompt_eval_count": 10u32,
        "eval_count": 20u32,
        "eval_duration": 5u64
    })
    .to_string()
}

#[tokio::test]
async fn test_ollama_health_check_reflects_daemon_status() {
    let healthy = ScriptedServer::spawn(vec![(200, r#"{"models":[]}"#.to_string())]).await;
    ollama_provider(&healthy.base_url())
        .health_check()
        .await
        .expect("health check against a running ollama");
    let request = healthy.last_request().await;
    assert_eq!(request.method, "GET");
    assert_eq!(request.path, "/api/tags");

    let down = ScriptedServer::spawn(vec![(503, String::new())]).await;
    let err = expect_error(ollama_provider(&down.base_url()).health_check().await);
    assert!(
        matches!(err, AiError::Api(ref message) if message.contains("not available")),
        "unexpected error: {err:?}"
    );
}

#[tokio::test]
async fn test_ollama_generate_review_parses_real_envelope() {
    let server = ScriptedServer::spawn(vec![(200, ollama_response(REVIEW_JSON))]).await;
    let provider = ollama_provider(&server.base_url());

    let response = provider
        .generate_review(&review_request())
        .await
        .expect("review from scripted ollama endpoint");

    assert_review_shape(&response, ProviderType::Ollama, "qwen3:8b");
    assert_eq!(response.cost_cents, 0, "local inference has no API cost");
    assert_eq!(response.tokens_used, (REVIEW_JSON.len() / 4) as u32);

    let request = server.last_request().await;
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/api/generate");
    let sent: serde_json::Value =
        serde_json::from_str(&request.body).expect("parse sent request body");
    assert_eq!(sent["model"], "qwen3:8b");
    assert_eq!(sent["stream"], false);
    assert_eq!(sent["options"]["temperature"], 0.3);
    assert_eq!(sent["options"]["num_predict"], 4096);
}

#[tokio::test]
async fn test_ollama_generate_review_falls_back_to_raw_prose() {
    let prose = "The change looks fine overall; no blocking issues were found.";
    let server = ScriptedServer::spawn(vec![(200, ollama_response(prose))]).await;
    let provider = ollama_provider(&server.base_url());

    let response = provider
        .generate_review(&review_request())
        .await
        .expect("prose review from scripted ollama endpoint");

    assert_eq!(response.summary, prose);
    assert!(response.findings.is_empty());
    assert_eq!(response.overall_score, 50);
    assert_eq!(response.provider, ProviderType::Ollama);
}

#[tokio::test]
async fn test_ollama_generate_review_maps_error_and_parse_failures() {
    let server = ScriptedServer::spawn(vec![(
        503,
        "model runner has unexpectedly stopped".to_string(),
    )])
    .await;
    let err = expect_error(
        ollama_provider(&server.base_url())
            .generate_review(&review_request())
            .await,
    );
    assert!(
        matches!(err, AiError::Api(ref message) if message.contains("unexpectedly stopped")),
        "unexpected error: {err:?}"
    );

    let malformed = ScriptedServer::spawn(vec![(200, "not json at all".to_string())]).await;
    let err = expect_error(
        ollama_provider(&malformed.base_url())
            .generate_review(&review_request())
            .await,
    );
    assert!(
        matches!(err, AiError::Parse(ref message) if message.contains("Failed to parse response")),
        "unexpected error: {err:?}"
    );
}
