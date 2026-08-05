//! OpenAI provider implementation

use super::{
    AiError, AiProvider, FindingCategory, ProviderConfig, ProviderType, ReviewFinding,
    ReviewRequest, ReviewResponse, Severity,
};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// OpenAI API endpoints
const OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// OpenAI provider for GPT-based code review
pub struct OpenAiProvider {
    api_key: String,
    base_url: String,
    client: Client,
    model: String,
    organization: Option<String>,
}

impl OpenAiProvider {
    /// Create a new OpenAI provider
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
                .unwrap_or_else(|| OPENAI_BASE_URL.to_string()),
            client: Client::builder()
                .timeout(Duration::from_secs(60))
                .build()
                .map_err(|e| AiError::Config(format!("Failed to create HTTP client: {}", e)))?,
            model: "gpt-4o".to_string(),
            organization: config.organization,
        })
    }

    /// Set the model to use
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

#[async_trait]
impl AiProvider for OpenAiProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::OpenAI
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn health_check(&self) -> Result<(), AiError> {
        let url = format!("{}/models", self.base_url);

        let mut request = self.client.get(&url).bearer_auth(&self.api_key);

        if let Some(ref org) = self.organization {
            request = request.header("OpenAI-Organization", org);
        }

        let response = request
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
        let url = format!("{}/chat/completions", self.base_url);

        let prompt = self.build_review_prompt(request);

        let body = OpenAiRequest {
            model: self.model.clone(),
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt,
            }],
            temperature: 0.3,
            max_tokens: request.config.max_tokens,
        };

        let mut req_builder = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body);

        if let Some(ref org) = self.organization {
            req_builder = req_builder.header("OpenAI-Organization", org);
        }

        let response = req_builder
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

        let api_response: OpenAiResponse = response
            .json()
            .await
            .map_err(|e| AiError::Parse(format!("Failed to parse response: {}", e)))?;

        self.parse_review_response(api_response, request)
    }
}

impl OpenAiProvider {
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
```
{}
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
        response: OpenAiResponse,
        _request: &ReviewRequest,
    ) -> Result<ReviewResponse, AiError> {
        let content = response
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .ok_or_else(|| AiError::Parse("No content in response".to_string()))?;

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

        let usage = response.usage.unwrap_or_default();
        let cost_cents =
            calculate_openai_cost(&self.model, usage.prompt_tokens, usage.completion_tokens);

        Ok(ReviewResponse {
            summary: parsed.summary,
            findings,
            overall_score: parsed.overall_score,
            cost_cents,
            tokens_used: usage.prompt_tokens + usage.completion_tokens,
            provider: ProviderType::OpenAI,
            model: self.model.clone(),
        })
    }
}

// OpenAI API types
#[derive(Debug, Serialize)]
struct OpenAiRequest {
    model: String,
    messages: Vec<Message>,
    temperature: f32,
    max_tokens: u32,
}

#[derive(Debug, Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OpenAiResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct Choice {
    index: u32,
    message: ResponseMessage,
    finish_reason: String,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ResponseMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize, Default)]
#[allow(dead_code)]
struct Usage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
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

/// Calculate cost for OpenAI API
/// Based on GPT-4o pricing (~$5/MTok input, $15/MTok output)
fn calculate_openai_cost(model: &str, prompt_tokens: u32, completion_tokens: u32) -> u32 {
    let (input_cost_per_mtok, output_cost_per_mtok) = if model.contains("gpt-4o-mini") {
        (0.15, 0.60) // $0.15/M input, $0.60/M output
    } else if model.contains("gpt-4o") {
        (5.0, 15.0) // $5/M input, $15/M output
    } else if model.contains("gpt-4-turbo") {
        (10.0, 30.0) // $10/M input, $30/M output
    } else {
        (0.50, 1.50) // Default fallback
    };

    let input_cost = (prompt_tokens as f64 / 1_000_000.0) * input_cost_per_mtok;
    let output_cost = (completion_tokens as f64 / 1_000_000.0) * output_cost_per_mtok;

    ((input_cost + output_cost) * 100.0) as u32 // Convert to cents
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cost_calculation() {
        // 1000 input + 500 output tokens for GPT-4o
        let cost = calculate_openai_cost("gpt-4o", 1000, 500);
        // (0.001 * 5.0) + (0.0005 * 15.0) = 0.005 + 0.0075 = 0.0125 dollars = ~1 cent
        assert!(cost >= 1);
        assert!(cost <= 2);
    }
}
