//! AI Provider Interface for GitForge
//!
//! This crate provides a trait-based abstraction for AI code review providers,
//! supporting Anthropic (Claude), OpenAI, and local OLLAMA.
//!
//! # Example
//!
//! ```rust,ignore
//! use gitforge_ai::{AiProvider, AnthropicProvider, ReviewRequest};
//!
//! let provider = AnthropicProvider::new()?;
//! let response = provider.generate_review(ReviewRequest::new(diff)).await?;
//! ```

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// AI provider errors
#[derive(Debug, Clone, thiserror::Error)]
pub enum AiError {
    #[error("API error: {0}")]
    Api(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Rate limit exceeded")]
    RateLimit,

    #[error("Context length exceeded: {0}")]
    ContextLength(usize),

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Parse error: {0}")]
    Parse(String),
}

/// Supported AI providers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Ollama,
}

impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderType::Anthropic => write!(f, "anthropic"),
            ProviderType::OpenAI => write!(f, "openai"),
            ProviderType::Ollama => write!(f, "ollama"),
        }
    }
}

/// Review request configuration
#[derive(Debug, Clone)]
pub struct ReviewConfig {
    /// Provider to use
    pub provider: ProviderType,
    /// Model name
    pub model: String,
    /// Maximum tokens in response
    pub max_tokens: u32,
    /// Temperature (0-1)
    pub temperature: f32,
    /// Cost budget per review in cents
    pub max_cost_cents: u32,
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            provider: ProviderType::Anthropic,
            model: "claude-3-5-sonnet-20241022".to_string(),
            max_tokens: 4096,
            temperature: 0.3,
            max_cost_cents: 50, // $0.50 default
        }
    }
}

/// A single file change in a review request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    /// File path relative to repository root
    pub path: String,
    /// Change type (added, modified, deleted)
    pub change_type: ChangeType,
    /// Unified diff content
    pub diff: String,
    /// Programming language (auto-detected or specified)
    pub language: Option<String>,
}

/// Change type for a file
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeType {
    Added,
    Modified,
    Deleted,
}

/// Review request sent to AI provider
#[derive(Debug, Clone)]
pub struct ReviewRequest {
    /// Repository name
    pub repo_name: String,
    /// Branch name
    pub branch: String,
    /// Base branch for comparison
    pub base_branch: Option<String>,
    /// List of file changes
    pub files: Vec<FileChange>,
    /// Additional context (commit messages, PR description, etc.)
    pub context: String,
    /// Configuration for this review
    pub config: ReviewConfig,
}

impl ReviewRequest {
    /// Create a new review request
    pub fn new(
        repo_name: impl Into<String>,
        branch: impl Into<String>,
        files: Vec<FileChange>,
    ) -> Self {
        Self {
            repo_name: repo_name.into(),
            branch: branch.into(),
            base_branch: None,
            files,
            context: String::new(),
            config: ReviewConfig::default(),
        }
    }

    /// Set the base branch
    pub fn with_base_branch(mut self, base: impl Into<String>) -> Self {
        self.base_branch = Some(base.into());
        self
    }

    /// Set additional context
    pub fn with_context(mut self, context: impl Into<String>) -> Self {
        self.context = context.into();
        self
    }

    /// Set the review configuration
    pub fn with_config(mut self, config: ReviewConfig) -> Self {
        self.config = config;
        self
    }
}

/// Severity level for review findings
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    High,
    Medium,
    Low,
    Info,
}

/// Category of review finding
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingCategory {
    Security,
    Bug,
    Performance,
    Style,
    Documentation,
    Complexity,
    BestPractice,
}

/// A single finding from the review
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    /// File path where the finding applies
    pub file: String,
    /// Starting line number
    pub line_start: Option<u32>,
    /// Ending line number
    pub line_end: Option<u32>,
    /// Severity of the finding
    pub severity: Severity,
    /// Category of the finding
    pub category: FindingCategory,
    /// Title/summary of the finding
    pub title: String,
    /// Detailed description
    pub description: String,
    /// Suggested fix (if applicable)
    pub suggestion: Option<String>,
    /// Code snippet (if applicable)
    pub code_snippet: Option<String>,
}

/// Review response from AI provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewResponse {
    /// Summary of the review
    pub summary: String,
    /// All findings from the review
    pub findings: Vec<ReviewFinding>,
    /// Overall score (0-100)
    pub overall_score: u32,
    /// Estimated cost in cents
    pub cost_cents: u32,
    /// Tokens used
    pub tokens_used: u32,
    /// Provider used
    pub provider: ProviderType,
    /// Model used
    pub model: String,
}

impl ReviewResponse {
    /// Check if any critical or high severity findings exist
    pub fn has_critical_findings(&self) -> bool {
        self.findings
            .iter()
            .any(|f| matches!(f.severity, Severity::Critical | Severity::High))
    }

    /// Get findings by severity
    pub fn findings_by_severity(&self, severity: Severity) -> Vec<&ReviewFinding> {
        self.findings
            .iter()
            .filter(|f| f.severity == severity)
            .collect()
    }

    /// Get findings by category
    pub fn findings_by_category(&self, category: FindingCategory) -> Vec<&ReviewFinding> {
        self.findings
            .iter()
            .filter(|f| f.category == category)
            .collect()
    }
}

/// Cost tracking information
#[derive(Debug, Clone, Default)]
pub struct CostTracking {
    /// Total cost in cents
    pub total_cents: u32,
    /// Total tokens used
    pub total_tokens: u32,
    /// Reviews performed
    pub reviews_count: u32,
}

impl CostTracking {
    /// Add a review's cost
    pub fn add_review(&mut self, cost_cents: u32, tokens: u32) {
        self.total_cents += cost_cents;
        self.total_tokens += tokens;
        self.reviews_count += 1;
    }

    /// Check if we're within budget
    pub fn within_budget(&self, max_cents: u32) -> bool {
        self.total_cents <= max_cents
    }
}

/// AI Provider trait for code review
#[async_trait]
pub trait AiProvider: Send + Sync {
    /// Get the provider type
    fn provider_type(&self) -> ProviderType;

    /// Get the model name
    fn model(&self) -> &str;

    /// Generate a code review
    async fn generate_review(&self, request: &ReviewRequest) -> Result<ReviewResponse, AiError>;

    /// Check if the provider is properly configured (has valid API key, etc.)
    async fn health_check(&self) -> Result<(), AiError>;
}

/// Configuration for AI providers
#[derive(Debug, Clone, Default)]
pub struct ProviderConfig {
    /// API key environment variable name
    pub api_key_env: String,
    /// Base URL (for custom endpoints)
    pub base_url: Option<String>,
    /// Organization ID (for OpenAI)
    pub organization: Option<String>,
}

impl ProviderConfig {
    /// Create config for Anthropic
    pub fn anthropic() -> Self {
        Self {
            api_key_env: "ANTHROPIC_API_KEY".to_string(),
            base_url: None,
            organization: None,
        }
    }

    /// Create config for OpenAI
    pub fn openai() -> Self {
        Self {
            api_key_env: "OPENAI_API_KEY".to_string(),
            base_url: None,
            organization: None,
        }
    }

    /// Create config for Ollama (local)
    pub fn ollama(base_url: impl Into<String>) -> Self {
        Self {
            api_key_env: String::new(), // Ollama doesn't need API key
            base_url: Some(base_url.into()),
            organization: None,
        }
    }
}

/// Provider factory for creating AI providers
pub struct AiProviderFactory;

impl AiProviderFactory {
    /// Create an Anthropic provider
    pub fn create_anthropic(config: ProviderConfig) -> Result<AnthropicProvider, AiError> {
        AnthropicProvider::new(config)
    }

    /// Create an OpenAI provider
    pub fn create_openai(config: ProviderConfig) -> Result<OpenAiProvider, AiError> {
        OpenAiProvider::new(config)
    }

    /// Create an Ollama provider
    pub fn create_ollama(config: ProviderConfig) -> Result<OllamaProvider, AiError> {
        OllamaProvider::new(config)
    }
}

// Re-export implementations
mod anthropic;
mod mock;
mod ollama;
mod openai;

pub use anthropic::AnthropicProvider;
pub use mock::{MockAiProvider, MockProviderConfig};
pub use ollama::OllamaProvider;
pub use openai::OpenAiProvider;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_review_config_default() {
        let config = ReviewConfig::default();
        assert_eq!(config.provider, ProviderType::Anthropic);
        assert!(config.max_cost_cents > 0);
    }

    #[test]
    fn test_review_request_builder() {
        let files = vec![FileChange {
            path: "src/main.rs".to_string(),
            change_type: ChangeType::Modified,
            diff: "".to_string(),
            language: Some("rust".to_string()),
        }];

        let request = ReviewRequest::new("test-repo", "main", files)
            .with_base_branch("develop")
            .with_context("Bug fix for issue #123");

        assert_eq!(request.repo_name, "test-repo");
        assert_eq!(request.branch, "main");
        assert_eq!(request.base_branch, Some("develop".to_string()));
        assert!(!request.context.is_empty());
    }

    #[test]
    fn test_review_response_critical_findings() {
        let response = ReviewResponse {
            summary: "Test review".to_string(),
            findings: vec![
                ReviewFinding {
                    file: "src/main.rs".to_string(),
                    line_start: Some(10),
                    line_end: Some(15),
                    severity: Severity::Critical,
                    category: FindingCategory::Security,
                    title: "SQL Injection".to_string(),
                    description: "Found SQL injection".to_string(),
                    suggestion: None,
                    code_snippet: None,
                },
                ReviewFinding {
                    file: "src/main.rs".to_string(),
                    line_start: Some(20),
                    line_end: Some(25),
                    severity: Severity::Low,
                    category: FindingCategory::Style,
                    title: "Style issue".to_string(),
                    description: "Style violation".to_string(),
                    suggestion: None,
                    code_snippet: None,
                },
            ],
            overall_score: 75,
            cost_cents: 25,
            tokens_used: 1000,
            provider: ProviderType::Anthropic,
            model: "claude-3-5-sonnet".to_string(),
        };

        assert!(response.has_critical_findings());
        assert_eq!(response.findings_by_severity(Severity::Critical).len(), 1);
        assert_eq!(
            response
                .findings_by_category(FindingCategory::Security)
                .len(),
            1
        );
    }

    #[test]
    fn test_cost_tracking() {
        let mut tracking = CostTracking::default();
        assert!(tracking.within_budget(100));

        tracking.add_review(30, 500);
        assert_eq!(tracking.total_cents, 30);
        assert_eq!(tracking.total_tokens, 500);
        assert_eq!(tracking.reviews_count, 1);

        tracking.add_review(20, 300);
        assert!(tracking.within_budget(100)); // 50 total
        assert!(!tracking.within_budget(40)); // exceeds
    }
}
