//! Ollama local provider implementation

use super::{AiError, AiProvider, ProviderConfig, ProviderType, ReviewFinding, ReviewRequest, ReviewResponse, Severity, FindingCategory};
use async_trait::async_trait;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Ollama API endpoints
const OLLAMA_BASE_URL: &str = "http://localhost:11434";

/// Ollama provider for local LLM-based code review
pub struct OllamaProvider {
    base_url: String,
    client: Client,
    model: String,
}

impl OllamaProvider {
    /// Create a new Ollama provider
    pub fn new(config: ProviderConfig) -> Result<Self, AiError> {
        Ok(Self {
            base_url: config.base_url.unwrap_or_else(|| OLLAMA_BASE_URL.to_string()),
            client: Client::builder()
                .timeout(Duration::from_secs(120)) // Longer timeout for local models
                .build()
                .map_err(|e| AiError::Config(format!("Failed to create HTTP client: {}", e)))?,
            model: "codellama".to_string(),
        })
    }

    /// Set the model to use
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }
}

#[async_trait]
impl AiProvider for OllamaProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Ollama
    }

    fn model(&self) -> &str {
        &self.model
    }

    async fn health_check(&self) -> Result<(), AiError> {
        let url = format!("{}/api/tags", self.base_url);

        let response = self.client
            .get(&url)
            .send()
            .await
            .map_err(|e| AiError::Network(format!("Health check failed: {}", e)))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(AiError::Api("Ollama not available".to_string()))
        }
    }

    async fn generate_review(&self, request: &ReviewRequest) -> Result<ReviewResponse, AiError> {
        let url = format!("{}/api/generate", self.base_url);

        let prompt = self.build_review_prompt(request);

        let body = OllamaRequest {
            model: self.model.clone(),
            prompt,
            stream: false,
            options: OllamaOptions {
                temperature: 0.3,
                num_predict: request.config.max_tokens as i32,
            },
        };

        let response = self.client
            .post(&url)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| AiError::Network(format!("Request failed: {}", e)))?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_default();
            return Err(AiError::Api(format!("API error: {}", error_text)));
        }

        let api_response: OllamaResponse = response
            .json()
            .await
            .map_err(|e| AiError::Parse(format!("Failed to parse response: {}", e)))?;

        self.parse_review_response(api_response, request)
    }
}

impl OllamaProvider {
    fn build_review_prompt(&self, request: &ReviewRequest) -> String {
        let mut prompt = format!(
            r#"You are an expert code reviewer. Review the following code changes in repository "{}" on branch "{}".
"#,
            request.repo_name, request.branch
        );

        if let Some(base) = &request.base_branch {
            prompt.push_str(&format!(r#"Compare against branch "{}".
"#,
            base));
        }

        if !request.context.is_empty() {
            prompt.push_str(&format!(r#"Context: {}

"#,
            request.context));
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

        prompt.push_str(r#"

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
"#);

        prompt
    }

    fn parse_review_response(
        &self,
        response: OllamaResponse,
        _request: &ReviewRequest,
    ) -> Result<ReviewResponse, AiError> {
        let content = response.response;

        // Try to parse as JSON, but if it fails, create a simple response
        let parsed: Result<ParsedReviewResponse, _> = serde_json::from_str(&content);

        match parsed {
            Ok(parsed) => {
                let findings = parsed.findings.into_iter().map(|f| {
                    ReviewFinding {
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
                    }
                }).collect();

                // Ollama doesn't provide token counts, estimate based on response length
                let tokens_used = (content.len() / 4) as u32; // Rough estimate

                Ok(ReviewResponse {
                    summary: parsed.summary,
                    findings,
                    overall_score: parsed.overall_score,
                    cost_cents: 0, // Local - no API cost
                    tokens_used,
                    provider: ProviderType::Ollama,
                    model: self.model.clone(),
                })
            }
            Err(_) => {
                // If parsing fails, create a simple response with the raw content
                Ok(ReviewResponse {
                    summary: content.chars().take(200).collect(),
                    findings: vec![],
                    overall_score: 50,
                    cost_cents: 0, // Local - no API cost
                    tokens_used: (content.len() / 4) as u32,
                    provider: ProviderType::Ollama,
                    model: self.model.clone(),
                })
            }
        }
    }
}

// Ollama API types
#[derive(Debug, Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
    options: OllamaOptions,
}

#[derive(Debug, Serialize)]
struct OllamaOptions {
    temperature: f32,
    num_predict: i32,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct OllamaResponse {
    model: String,
    response: String,
    done: bool,
    context: Option<Vec<i32>>,
    total_duration: Option<u64>,
    load_duration: Option<u64>,
    prompt_eval_count: Option<u32>,
    eval_count: Option<u32>,
    eval_duration: Option<u64>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ChangeType, FileChange, ReviewRequest};

    #[test]
    fn test_build_review_prompt() {
        let provider = OllamaProvider::new(ProviderConfig::ollama("http://localhost:11434"))
            .unwrap()
            .with_model("codellama");

        let files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: "fn main() {}".to_string(),
            language: Some("rust".to_string()),
        }];

        let request = ReviewRequest::new("test-repo", "feature/test", files);

        let prompt = provider.build_review_prompt(&request);

        assert!(prompt.contains("test-repo"));
        assert!(prompt.contains("feature/test"));
    }
}
