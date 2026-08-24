//! Restricted read-only client for the GitForge API.

use reqwest::Url;
use serde_json::Value;
use std::time::Duration;

/// Errors returned by the restricted API client.
#[derive(Debug, thiserror::Error)]
pub enum ApiClientError {
    #[error("invalid API base URL: {0}")]
    InvalidBaseUrl(String),
    #[error("API host is not allowlisted: {0}")]
    HostNotAllowed(String),
    #[error("API request failed")]
    Request(#[source] reqwest::Error),
    #[error("API returned HTTP status {0}")]
    HttpStatus(u16),
    #[error("API returned invalid JSON")]
    InvalidJson(#[source] serde_json::Error),
}

/// Read-only GitForge API client with an explicit host allowlist.
pub struct ApiClient {
    client: reqwest::Client,
    base_url: Url,
    bearer_token: Option<String>,
}

impl ApiClient {
    /// Construct a client. An empty allowlist is rejected deliberately.
    pub fn new(
        base_url: &str,
        bearer_token: Option<String>,
        allowed_hosts: &[String],
        timeout: Duration,
    ) -> Result<Self, ApiClientError> {
        if allowed_hosts.is_empty() {
            return Err(ApiClientError::HostNotAllowed(
                "empty allowlist".to_string(),
            ));
        }

        let url = Url::parse(base_url)
            .map_err(|error| ApiClientError::InvalidBaseUrl(error.to_string()))?;
        let host = url
            .host_str()
            .ok_or_else(|| ApiClientError::InvalidBaseUrl("missing host".to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(ApiClientError::InvalidBaseUrl(
                "scheme must be http or https".to_string(),
            ));
        }
        if url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(ApiClientError::InvalidBaseUrl(
                "credentials, query parameters, and fragments are not allowed".to_string(),
            ));
        }
        if !allowed_hosts.iter().any(|allowed| allowed == host) {
            return Err(ApiClientError::HostNotAllowed(host.to_string()));
        }

        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .map_err(ApiClientError::Request)?;

        Ok(Self {
            client,
            base_url: url,
            bearer_token,
        })
    }

    /// Construct from an environment variable without exposing its value.
    pub fn from_env(
        base_url: &str,
        token_env: Option<&str>,
        allowed_hosts: &[String],
        timeout: Duration,
    ) -> Result<Self, ApiClientError> {
        let token = token_env.and_then(|name| std::env::var(name).ok());
        Self::new(base_url, token, allowed_hosts, timeout)
    }

    fn endpoint(&self, path: &str) -> Result<Url, ApiClientError> {
        let mut url = self.base_url.clone();
        let base_path = url.path().trim_end_matches('/');
        url.set_path(&format!("{base_path}{path}"));
        Ok(url)
    }

    async fn get_json(&self, path: &str) -> Result<Value, ApiClientError> {
        let url = self.endpoint(path)?;
        let mut request = self.client.get(url);
        if let Some(token) = &self.bearer_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(ApiClientError::Request)?;
        let status = response.status();
        if !status.is_success() {
            return Err(ApiClientError::HttpStatus(status.as_u16()));
        }
        response
            .json::<Value>()
            .await
            .map_err(ApiClientError::Request)
    }

    /// Fetch the public API health document.
    pub async fn health(&self) -> Result<Value, ApiClientError> {
        self.get_json("/health").await
    }

    /// Fetch repositories visible to the authenticated API principal.
    pub async fn repositories(&self) -> Result<Value, ApiClientError> {
        self.get_json("/api/repos").await
    }

    /// Fetch pipeline definitions visible to the authenticated principal.
    pub async fn pipelines(&self) -> Result<Value, ApiClientError> {
        self.get_json("/api/pipelines").await
    }

    /// Fetch pipeline runs visible to the authenticated principal.
    pub async fn pipeline_runs(&self) -> Result<Value, ApiClientError> {
        self.get_json("/api/pipeline-runs").await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowed() -> Vec<String> {
        vec!["127.0.0.1".to_string()]
    }

    #[test]
    fn rejects_empty_allowlist() {
        let result = ApiClient::new("http://127.0.0.1:8080", None, &[], Duration::from_secs(1));

        assert!(matches!(
            result,
            Err(ApiClientError::HostNotAllowed(message)) if message == "empty allowlist"
        ));
    }

    #[test]
    fn rejects_non_allowlisted_host() {
        let result = ApiClient::new(
            "http://localhost:8080",
            None,
            &allowed(),
            Duration::from_secs(1),
        );

        assert!(matches!(
            result,
            Err(ApiClientError::HostNotAllowed(host)) if host == "localhost"
        ));
    }

    #[test]
    fn rejects_url_fragments() {
        let result = ApiClient::new(
            "http://127.0.0.1:8080/base#fragment",
            None,
            &allowed(),
            Duration::from_secs(1),
        );

        assert!(matches!(
            result,
            Err(ApiClientError::InvalidBaseUrl(message))
                if message.contains("fragments")
        ));
    }

    #[tokio::test]
    async fn health_uses_allowlisted_endpoint() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(200)
            .with_body(r#"{"status":"healthy"}"#)
            .create_async()
            .await;
        let client = ApiClient::new(
            &server.url(),
            None,
            &["127.0.0.1".to_string()],
            Duration::from_secs(1),
        )
        .expect("valid client");

        let health = client.health().await.expect("health response");

        assert_eq!(health["status"], "healthy");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn authenticated_endpoint_sends_bearer_token() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/api/repos")
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let client = ApiClient::new(
            &server.url(),
            Some("test-token".to_string()),
            &["127.0.0.1".to_string()],
            Duration::from_secs(1),
        )
        .expect("valid client");

        assert_eq!(
            client.repositories().await.expect("repos"),
            serde_json::json!([])
        );
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn upstream_status_error_does_not_expose_body() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/health")
            .with_status(503)
            .with_body("secret upstream detail")
            .create_async()
            .await;
        let client = ApiClient::new(
            &server.url(),
            Some("secret-token".to_string()),
            &["127.0.0.1".to_string()],
            Duration::from_secs(1),
        )
        .expect("valid client");

        let error = client.health().await.expect_err("503 must fail");
        let message = error.to_string();
        assert!(message.contains("503"));
        assert!(!message.contains("secret"));
        mock.assert_async().await;
    }
}
