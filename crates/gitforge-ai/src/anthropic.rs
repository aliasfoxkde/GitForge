//! Anthropic (Claude) AI provider implementation

use super::{
    AiError, AiProvider, FindingCategory, ProviderConfig, ProviderType, ReviewFinding,
    ReviewRequest, ReviewResponse, Severity,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Anthropic API endpoints
const ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";

/// Anthropic provider for Claude-based code review
pub struct AnthropicProvider {
    api_key: String,
    base_url: String,
    client: Client,
    model: String,
}

impl AnthropicProvider {
    /// Create a new Anthropic provider
    pub fn new(config: ProviderConfig) -> Result<Self, AiError> {
        let api_key = std::env::var(&config.api_key_env).map_err(|_| {
            AiError::Config(format!(
                "Environment variable {} not set",
                config.api_key_env
            ))
        })?;

        Ok(Self {
            api_key,
            base_url: config
                .base_url
                .unwrap_or_else(|| ANTHROPIC_BASE_URL.to_string()),
            client: Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|e| AiError::Config(format!("Failed to create HTTP client: {}", e)))?,
            model: "claude-3-5-sonnet-20241022".to_string(),
        })
    }

    /// Set the model to use
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

#[async_trait]
impl AiProvider for AnthropicProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Anthropic
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn health_check(&self) -> Result<(), AiError> {
        // Simple check - try to get model info
        let url = format!("{}/v1/messages", self.base_url);

        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 10,
            "messages": [{"role": "user", "content": "hi"}]
        });

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Network(format!("Health check failed: {}", e)))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(AiError::Auth("API key validation failed".to_string()))
        }
    }

    async fn generate_review(&self, request: &ReviewRequest) -> Result<ReviewResponse, AiError> {
        let url = format!("{}/v1/messages", self.base_url);

        let prompt = self.build_review_prompt(request);

        let body = AnthropicRequest {
            model: self.model.clone(),
            max_tokens: request.config.max_tokens,
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
        };

        let response = self
            .client
            .post(&url)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Network(format!("Request failed: {}", e)))?;

        if response.status() == 429 {
            return Err(AiError::RateLimit);
        }

        if response.status() == 401 {
            return Err(AiError::Auth("Invalid API key".to_string()));
        }

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AiError::Api(format!("API error: {}", error_text)));
        }

        let api_response: AnthropicResponse = response
            .json()
            .await
            .map_err(|e| AiError::Parse(format!("Failed to parse response: {}", e)))?;

        self.parse_review_response(api_response, request)
    }
}

impl AnthropicProvider {
    fn build_review_prompt(&self, request: &ReviewRequest) -> String {
        let mut prompt = format!(
            r#"You are an expert code reviewer. Review the following code changes in repository "{}" on branch "{}".
"#,
            request.repo_name, request.branch
        );

        if let Some(base) = &request.base_branch {
            prompt.push_str(&format!(
                r#"Compare against branch "{}".
"#,
                base
            ));
        }

        if !request.context.is_empty() {
            prompt.push_str(&format!(
                r#"Context: {}

"#,
                request.context
            ));
        }

        prompt.push_str(&format!(
            r#"Review the changes and provide:
1. A brief summary of the overall changes
2. A score from 0-100 for code quality (higher is better)
3. Specific findings with severity (critical/high/medium/low/info), category, file, line numbers, and descriptions

Files changed ({} total):
"#,
            request.files.len()
        ));

        for (i, file) in request.files.iter().enumerate() {
            prompt.push_str(&format!(
                r#"
--- File {}: {} ({})
```{}
{}
```
"#,
                i + 1,
                file.path,
                serde_json::to_string(&file.change_type).unwrap_or_default(),
                file.language.as_deref().unwrap_or("text"),
                file.diff
            ));
        }

        prompt.push_str(
            r#"

Provide your review in JSON format:
{
  "summary": "Brief summary of changes and overall assessment",
  "overall_score": 0-100,
  "findings": [
    {
      "file": "path/to/file.ext",
      "line_start": 10,
      "line_end": 15,
      "severity": "high",
      "category": "security",
      "title": "Brief title",
      "description": "Detailed description of the issue",
      "suggestion": "Optional suggested fix",
      "code_snippet": "Optional relevant code snippet"
    }
  ]
}

JSON Response:
"#,
        );

        prompt
    }

    fn parse_review_response(
        &self,
        response: AnthropicResponse,
        _request: &ReviewRequest,
    ) -> Result<ReviewResponse, AiError> {
        // Extract the text content from the response
        let content = response
            .content
            .into_iter()
            .find_map(|block| {
                if let ContentBlock::Text(text) = block {
                    Some(text.text)
                } else {
                    None
                }
            })
            .ok_or_else(|| AiError::Parse("No text content in response".to_string()))?;

        // Parse the JSON from the response
        let parsed: ParsedReviewResponse = serde_json::from_str(&content).map_err(|e| {
            AiError::Parse(format!(
                "Failed to parse review JSON: {}\n\nContent:\n{}",
                e, content
            ))
        })?;

        let findings = parsed
            .findings
            .into_iter()
            .map(|f| ReviewFinding {
                file: f.file,
                line_start: f.line_start,
                line_end: f.line_end,
                severity: match f.severity.as_str() {
                    "critical" => Severity::Critical,
                    "high" => Severity::High,
                    "medium" => Severity::Medium,
                    "low" => Severity::Low,
                    _ => Severity::Info,
                },
                category: match f.category.as_str() {
                    "security" => FindingCategory::Security,
                    "bug" => FindingCategory::Bug,
                    "performance" => FindingCategory::Performance,
                    "style" => FindingCategory::Style,
                    "documentation" => FindingCategory::Documentation,
                    "complexity" => FindingCategory::Complexity,
                    _ => FindingCategory::BestPractice,
                },
                title: f.title,
                description: f.description,
                suggestion: f.suggestion,
                code_snippet: f.code_snippet,
            })
            .collect();

        // Estimate cost based on tokens (Anthropic pricing)
        let input_tokens = response.usage.input_tokens;
        let output_tokens = response.usage.output_tokens;
        let cost_cents = calculate_anthropic_cost(&self.model, input_tokens, output_tokens);

        Ok(ReviewResponse {
            summary: parsed.summary,
            findings,
            overall_score: parsed.overall_score,
            cost_cents,
            tokens_used: input_tokens + output_tokens,
            provider: ProviderType::Anthropic,
            model: self.model.clone(),
        })
    }
}

// Anthropic API types
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct AnthropicResponse {
    id: String,
    type_: String,
    role: String,
    content: Vec<ContentBlock>,
    model: String,
    usage: Usage,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
#[serde(tag = "type")]
enum ContentBlock {
    #[serde(rename = "text")]
    Text(TextBlock),
    #[serde(rename = "error")]
    Error(TextBlock),
}

#[derive(Debug, Deserialize)]
struct TextBlock {
    text: String,
}

#[derive(Debug, Deserialize)]
struct Usage {
    input_tokens: u32,
    output_tokens: u32,
}

// Internal parsing types
#[derive(Debug, Deserialize)]
struct ParsedReviewResponse {
    summary: String,
    overall_score: u32,
    findings: Vec<ParsedFinding>,
}

#[derive(Debug, Deserialize)]
struct ParsedFinding {
    file: String,
    line_start: Option<u32>,
    line_end: Option<u32>,
    severity: String,
    category: String,
    title: String,
    description: String,
    suggestion: Option<String>,
    code_snippet: Option<String>,
}

/// Calculate cost for Anthropic API
/// Based on Claude 3.5 Sonnet pricing (~$3/MTok input, $15/MTok output)
fn calculate_anthropic_cost(model: &str, input_tokens: u32, output_tokens: u32) -> u32 {
    let input_cost_per_mtok = if model.contains("haiku") {
        0.25 // $0.25/M token for Haiku
    } else if model.contains("sonnet") {
        3.0 // $3/M token for Sonnet
    } else {
        15.0 // $15/M token for Opus
    };

    let output_cost_per_mtok = if model.contains("haiku") {
        1.25
    } else if model.contains("sonnet") {
        15.0
    } else {
        75.0
    };

    let input_cost = (input_tokens as f64 / 1_000_000.0) * input_cost_per_mtok;
    let output_cost = (output_tokens as f64 / 1_000_000.0) * output_cost_per_mtok;

    ((input_cost + output_cost) * 100.0) as u32 // Convert to cents
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChangeType, FileChange, ReviewRequest};

    #[test]
    fn test_cost_calculation() {
        // 1000 input + 500 output tokens for Sonnet
        let cost = calculate_anthropic_cost("claude-3-5-sonnet-20241022", 1000, 500);
        // (0.001 * 3.0) + (0.0005 * 15.0) = 0.003 + 0.0075 = 0.0105 dollars = ~1 cent
        assert!(cost >= 1);
        assert!(cost <= 2);
    }

    #[test]
    fn test_build_review_prompt() {
        // Prompt construction is offline and must not depend on a developer's
        // live credential. The provider only needs a syntactically valid test
        // configuration here; network calls are covered separately.
        let test_key_env = "GITFORGE_TEST_ANTHROPIC_KEY";
        std::env::set_var(test_key_env, "test-anthropic-key");
        let config = ProviderConfig {
            api_key_env: test_key_env.to_string(),
            ..ProviderConfig::anthropic()
        };
        let provider = AnthropicProvider::new(config)
            .unwrap()
            .with_model("claude-3-5-sonnet-20241022");

        let files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: "fn main() { println!(\"hello\"); }".to_string(),
            language: Some("rust".to_string()),
        }];

        let request = ReviewRequest::new("test-repo", "feature/test", files)
            .with_base_branch("main")
            .with_context("Adding hello world");

        let prompt = provider.build_review_prompt(&request);

        assert!(prompt.contains("test-repo"));
        assert!(prompt.contains("feature/test"));
        assert!(prompt.contains("main"));
        assert!(prompt.contains("Adding hello world"));
        assert!(prompt.contains("src/main.rs"));
    }
}
