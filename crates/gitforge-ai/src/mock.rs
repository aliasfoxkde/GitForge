//! Mock AI Provider for testing

use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::*;

/// Configuration for mock provider behavior
#[derive(Debug, Clone)]
pub struct MockProviderConfig {
    /// Whether health_check should succeed
    pub health_check_success: bool,
    /// Whether generate_review should succeed
    pub review_success: bool,
    /// Response to return on successful review
    pub mock_response: Option<ReviewResponse>,
    /// Error to return on failed review
    pub error_to_return: Option<AiError>,
    /// Number of reviews before rate limit (None = unlimited)
    pub reviews_before_rate_limit: Option<usize>,
    /// Call count
    pub call_count: Arc<RwLock<usize>>,
}

impl Default for MockProviderConfig {
    fn default() -> Self {
        Self {
            health_check_success: true,
            review_success: true,
            mock_response: None,
            error_to_return: None,
            reviews_before_rate_limit: None,
            call_count: Arc::new(RwLock::new(0)),
        }
    }
}

impl MockProviderConfig {
    /// Create a mock that always succeeds
    pub fn success() -> Self {
        Self::default()
    }

    /// Create a mock that always fails
    pub fn failure(error: AiError) -> Self {
        Self {
            health_check_success: false,
            review_success: false,
            error_to_return: Some(error),
            ..Default::default()
        }
    }

    /// Create a mock that returns rate limit after N calls
    pub fn rate_limited_after(calls: usize) -> Self {
        Self {
            reviews_before_rate_limit: Some(calls),
            ..Default::default()
        }
    }

    /// Set a custom response
    pub fn with_response(mut self, response: ReviewResponse) -> Self {
        self.mock_response = Some(response);
        self
    }
}

/// Mock AI provider for testing
pub struct MockAiProvider {
    config: MockProviderConfig,
}

impl MockAiProvider {
    /// Create a new mock provider
    pub fn new(config: MockProviderConfig) -> Self {
        Self { config }
    }

    /// Create a mock that always succeeds
    pub fn success() -> Self {
        Self::new(MockProviderConfig::success())
    }

    /// Create a mock that always fails
    pub fn failure(error: AiError) -> Self {
        Self::new(MockProviderConfig::failure(error))
    }

    /// Get the number of times generate_review was called
    pub async fn call_count(&self) -> usize {
        *self.config.call_count.read().await
    }
}

impl Default for MockAiProvider {
    fn default() -> Self {
        Self::success()
    }
}

#[async_trait]
impl AiProvider for MockAiProvider {
    fn provider_type(&self) -> ProviderType {
        ProviderType::Anthropic
    }

    fn model(&self) -> &str {
        "mock-model"
    }

    async fn generate_review(&self, _request: &ReviewRequest) -> Result<ReviewResponse, AiError> {
        // Increment call count
        {
            let mut count = self.config.call_count.write().await;
            *count += 1;
        }

        // Check rate limit
        if let Some(limit) = self.config.reviews_before_rate_limit {
            let count = *self.config.call_count.read().await;
            if count > limit {
                return Err(AiError::RateLimit);
            }
        }

        // Return configured response or default
        if self.config.review_success {
            if let Some(response) = &self.config.mock_response {
                Ok(response.clone())
            } else {
                // Return a default mock response
                Ok(ReviewResponse {
                    summary: "Mock review summary".to_string(),
                    findings: vec![
                        ReviewFinding {
                            file: "src/mock.rs".to_string(),
                            line_start: Some(1),
                            line_end: Some(10),
                            severity: Severity::Info,
                            category: FindingCategory::BestPractice,
                            title: "Mock finding".to_string(),
                            description: "This is a mock finding for testing".to_string(),
                            suggestion: Some("Consider reviewing this in production".to_string()),
                            code_snippet: Some("fn mock() {}".to_string()),
                        },
                    ],
                    overall_score: 100,
                    cost_cents: 1,
                    tokens_used: 100,
                    provider: ProviderType::Anthropic,
                    model: "mock-model".to_string(),
                })
            }
        } else if let Some(error) = &self.config.error_to_return {
            Err(error.clone())
        } else {
            Err(AiError::Api("Mock API error".to_string()))
        }
    }

    async fn health_check(&self) -> Result<(), AiError> {
        if self.config.health_check_success {
            Ok(())
        } else if let Some(error) = &self.config.error_to_return {
            Err(error.clone())
        } else {
            Err(AiError::Config("Mock health check failed".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_provider_success() {
        let provider = MockAiProvider::success();

        let files = vec![FileChange {
            path: "test.rs".to_string(),
            change_type: ChangeType::Added,
            diff: "+fn test() {}".to_string(),
            language: Some("rust".to_string()),
        }];

        let request = ReviewRequest::new("test-repo", "main", files);
        let result = provider.generate_review(&request).await;

        assert!(result.is_ok());
        let response = result.unwrap();
        assert_eq!(response.summary, "Mock review summary");
        assert!(!response.findings.is_empty());
    }

    #[tokio::test]
    async fn test_mock_provider_failure() {
        let provider = MockAiProvider::failure(AiError::RateLimit);

        let files = vec![FileChange {
            path: "test.rs".to_string(),
            change_type: ChangeType::Added,
            diff: "+fn test() {}".to_string(),
            language: Some("rust".to_string()),
        }];

        let request = ReviewRequest::new("test-repo", "main", files);
        let result = provider.generate_review(&request).await;

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AiError::RateLimit));
    }

    #[tokio::test]
    async fn test_mock_provider_rate_limit() {
        let provider = MockAiProvider::new(MockProviderConfig::rate_limited_after(2));

        let files = vec![FileChange {
            path: "test.rs".to_string(),
            change_type: ChangeType::Added,
            diff: "+fn test() {}".to_string(),
            language: Some("rust".to_string()),
        }];

        let request = ReviewRequest::new("test-repo", "main", files);

        // First two should succeed
        assert!(provider.generate_review(&request).await.is_ok());
        assert!(provider.generate_review(&request).await.is_ok());

        // Third should fail with rate limit
        let result = provider.generate_review(&request).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), AiError::RateLimit));
    }

    #[tokio::test]
    async fn test_mock_provider_call_count() {
        let provider = MockAiProvider::success();

        assert_eq!(provider.call_count().await, 0);

        let files = vec![FileChange {
            path: "test.rs".to_string(),
            change_type: ChangeType::Added,
            diff: "+fn test() {}".to_string(),
            language: Some("rust".to_string()),
        }];

        let request = ReviewRequest::new("test-repo", "main", files);

        provider.generate_review(&request).await.unwrap();
        assert_eq!(provider.call_count().await, 1);

        provider.generate_review(&request).await.unwrap();
        assert_eq!(provider.call_count().await, 2);
    }

    #[tokio::test]
    async fn test_mock_provider_custom_response() {
        let custom_response = ReviewResponse {
            summary: "Custom summary".to_string(),
            findings: vec![],
            overall_score: 50,
            cost_cents: 10,
            tokens_used: 500,
            provider: ProviderType::OpenAI,
            model: "gpt-4".to_string(),
        };

        let provider = MockAiProvider::new(
            MockProviderConfig::success().with_response(custom_response.clone()),
        );

        let files = vec![FileChange {
            path: "test.rs".to_string(),
            change_type: ChangeType::Added,
            diff: "+fn test() {}".to_string(),
            language: Some("rust".to_string()),
        }];

        let request = ReviewRequest::new("test-repo", "main", files);
        let result = provider.generate_review(&request).await.unwrap();

        assert_eq!(result.summary, "Custom summary");
        assert_eq!(result.overall_score, 50);
        assert_eq!(result.provider, ProviderType::OpenAI);
    }

    #[tokio::test]
    async fn test_mock_provider_health_check() {
        let provider = MockAiProvider::success();
        assert!(provider.health_check().await.is_ok());

        let failing_provider = MockAiProvider::failure(AiError::Auth("invalid key".to_string()));
        assert!(failing_provider.health_check().await.is_err());
    }
}
