//! API client for GitForge CLI

use anyhow::Result;
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// Login response from the API
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub token: String,
    pub token_type: String,
    pub expires_in: i64,
}

/// Auth status response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthStatusResponse {
    pub authenticated: bool,
    pub user_id: Option<String>,
    pub username: Option<String>,
    pub role: Option<String>,
    pub message: Option<String>,
}

/// Login request body
#[derive(Debug, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

/// Repository response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoResponse {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub visibility: String,
    pub git_path: String,
    pub created_at: String,
    pub updated_at: String,
}

/// Pipeline response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResponse {
    pub id: String,
    pub name: String,
    pub repo_id: String,
    pub enabled: bool,
}

/// Pipeline run response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRunResponse {
    pub id: String,
    pub pipeline_id: String,
    pub status: String,
    pub commit_hash: String,
    pub triggered_by: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// Job response
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResponse {
    pub id: String,
    pub name: String,
    pub status: String,
    pub runner_id: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
}

/// Runner response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunnerResponse {
    pub id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub runner_type: String,
    pub status: String,
    pub capacity: i32,
    pub last_heartbeat: Option<String>,
}

/// Create repo request
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateRepoRequest {
    pub name: String,
    pub visibility: Option<String>,
}

/// GitForge API client
#[derive(Clone)]
pub struct ApiClient {
    base_url: String,
    token: Option<String>,
    http: Client,
}

impl ApiClient {
    /// Create a new client
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        Self {
            base_url: base_url.to_string(),
            token,
            http: Client::new(),
        }
    }

    /// Set auth token
    pub fn with_token(mut self, token: String) -> Self {
        self.token = Some(token);
        self
    }

    /// GET request
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        Ok(resp.json().await?)
    }

    /// POST request with JSON body
    pub async fn post<T: Serialize, R: DeserializeOwned>(&self, path: &str, body: &T) -> Result<R> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url).json(body);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        Ok(resp.json().await?)
    }

    /// DELETE request
    pub async fn delete(&self, path: &str) -> Result<()> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.delete(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        req.send().await?;
        Ok(())
    }

    /// Login with username and password
    pub async fn login(&self, username: &str, password: &str) -> Result<LoginResponse> {
        let url = format!("{}/auth/login", self.base_url);
        let body = LoginRequest {
            username: username.to_string(),
            password: password.to_string(),
        };

        let resp = self.http.post(&url).json(&body).send().await?;
        let status = resp.status();

        if !status.is_success() {
            let error_text = resp.text().await.unwrap_or_default();
            anyhow::bail!("Login failed: {} - {}", status, error_text);
        }

        let login_resp: LoginResponse = resp.json().await?;
        Ok(login_resp)
    }

    /// Check authentication status
    pub async fn auth_status(&self) -> Result<AuthStatusResponse> {
        let url = format!("{}/auth/status", self.base_url);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        let auth_resp: AuthStatusResponse = resp.json().await?;
        Ok(auth_resp)
    }

    /// List repositories
    pub async fn list_repos(&self) -> Result<Vec<RepoResponse>> {
        let url = format!("{}/api/repos", self.base_url);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to list repos: {}", resp.status());
        }
        let repos: Vec<RepoResponse> = resp.json().await?;
        Ok(repos)
    }

    /// Get repository by ID
    pub async fn get_repo(&self, id: &str) -> Result<RepoResponse> {
        let url = format!("{}/api/repos/{}", self.base_url, id);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to get repo: {}", resp.status());
        }
        let repo: RepoResponse = resp.json().await?;
        Ok(repo)
    }

    /// Create a repository
    pub async fn create_repo(
        &self,
        name: &str,
        visibility: Option<String>,
    ) -> Result<RepoResponse> {
        let url = format!("{}/api/repos", self.base_url);
        let body = CreateRepoRequest {
            name: name.to_string(),
            visibility,
        };

        let mut req = self.http.post(&url).json(&body);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to create repo: {}", resp.status());
        }
        let repo: RepoResponse = resp.json().await?;
        Ok(repo)
    }

    /// Delete a repository
    pub async fn delete_repo(&self, id: &str) -> Result<()> {
        let url = format!("{}/api/repos/{}", self.base_url, id);
        let mut req = self.http.delete(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to delete repo: {}", resp.status());
        }
        Ok(())
    }

    /// List pipelines
    pub async fn list_pipelines(&self) -> Result<Vec<PipelineResponse>> {
        let url = format!("{}/api/pipelines", self.base_url);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to list pipelines: {}", resp.status());
        }
        let pipelines: Vec<PipelineResponse> = resp.json().await?;
        Ok(pipelines)
    }

    /// Get pipeline by ID
    pub async fn get_pipeline(&self, id: &str) -> Result<PipelineResponse> {
        let url = format!("{}/api/pipelines/{}", self.base_url, id);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to get pipeline: {}", resp.status());
        }
        let pipeline: PipelineResponse = resp.json().await?;
        Ok(pipeline)
    }

    /// List pipeline runs
    pub async fn list_pipeline_runs(&self) -> Result<Vec<PipelineRunResponse>> {
        let url = format!("{}/api/pipeline-runs", self.base_url);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to list pipeline runs: {}", resp.status());
        }
        let runs: Vec<PipelineRunResponse> = resp.json().await?;
        Ok(runs)
    }

    /// List runners
    pub async fn list_runners(&self) -> Result<Vec<RunnerResponse>> {
        let url = format!("{}/api/runners", self.base_url);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to list runners: {}", resp.status());
        }
        let runners: Vec<RunnerResponse> = resp.json().await?;
        Ok(runners)
    }

    /// Get runner by ID
    pub async fn get_runner(&self, id: &str) -> Result<RunnerResponse> {
        let url = format!("{}/api/runners/{}", self.base_url, id);
        let mut req = self.http.get(&url);

        if let Some(token) = &self.token {
            req = req.header("Authorization", format!("Bearer {}", token));
        }

        let resp = req.send().await?;
        if !resp.status().is_success() {
            anyhow::bail!("Failed to get runner: {}", resp.status());
        }
        let runner: RunnerResponse = resp.json().await?;
        Ok(runner)
    }
}

pub use ApiClient as GitForgeClient;

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestResponse {
        pub id: String,
        pub name: String,
    }

    #[derive(Debug, Serialize, Deserialize)]
    struct TestRequest {
        pub value: String,
    }

    #[tokio::test]
    async fn test_api_client_get_success() {
        let mut mock_server = mockito::Server::new_async().await;
        let m = mock_server
            .mock("GET", "/api/test")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"123","name":"test"}"#)
            .create();

        let url = mock_server.url();
        let client = ApiClient::new(&url, Some("token".to_string()));
        let result: TestResponse = client.get("/api/test").await.unwrap();
        assert_eq!(result.id, "123");
        assert_eq!(result.name, "test");
        m.assert();
    }

    #[tokio::test]
    async fn test_api_client_post_success() {
        let mut mock_server = mockito::Server::new_async().await;
        let m = mock_server
            .mock("POST", "/api/test")
            .with_status(201)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"456","name":"created"}"#)
            .create();

        let url = mock_server.url();
        let client = ApiClient::new(&url, Some("token".to_string()));
        let request = TestRequest {
            value: "test".to_string(),
        };
        let result: TestResponse = client.post("/api/test", &request).await.unwrap();
        assert_eq!(result.id, "456");
        m.assert();
    }

    #[tokio::test]
    async fn test_api_client_delete_success() {
        let mut mock_server = mockito::Server::new_async().await;
        let m = mock_server
            .mock("DELETE", "/api/test/123")
            .with_status(204)
            .create();

        let url = mock_server.url();
        let client = ApiClient::new(&url, Some("token".to_string()));
        client.delete("/api/test/123").await.unwrap();
        m.assert();
    }

    #[tokio::test]
    async fn test_api_client_get_with_auth_header() {
        let mut mock_server = mockito::Server::new_async().await;
        let m = mock_server
            .mock("GET", "/api/protected")
            .match_header("Authorization", "Bearer my-secret-token")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"1","name":"authed"}"#)
            .create();

        let url = mock_server.url();
        let client = ApiClient::new(&url, Some("my-secret-token".to_string()));
        let result: TestResponse = client.get("/api/protected").await.unwrap();
        assert_eq!(result.id, "1");
        m.assert();
    }

    #[tokio::test]
    async fn test_api_client_get_without_token() {
        let mut mock_server = mockito::Server::new_async().await;
        let m = mock_server
            .mock("GET", "/api/public")
            .match_header("Authorization", mockito::Matcher::Missing)
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"1","name":"public"}"#)
            .create();

        let url = mock_server.url();
        let client = ApiClient::new(&url, None);
        let result: TestResponse = client.get("/api/public").await.unwrap();
        assert_eq!(result.id, "1");
        m.assert();
    }

    #[test]
    fn test_api_client_new() {
        let client = ApiClient::new("http://localhost:42780", None);
        assert_eq!(client.base_url, "http://localhost:42780");
    }

    #[test]
    fn test_api_client_with_token() {
        let client =
            ApiClient::new("http://localhost:42780", None).with_token("test-token".to_string());
        // Token is set internally, verify client was created
        let client2 = ApiClient::new("http://localhost:42780", Some("test-token".to_string()));
        assert_eq!(client.token, client2.token);
    }

    #[test]
    fn test_api_client_clone() {
        let client = ApiClient::new("http://localhost:42780", Some("token".to_string()));
        let _cloned = client.clone();
    }

    #[test]
    fn test_api_client_base_url() {
        let client = ApiClient::new("http://localhost:9090", None);
        assert_eq!(client.base_url, "http://localhost:9090");
    }

    #[test]
    fn test_api_client_token_none() {
        let client = ApiClient::new("http://localhost:42780", None);
        assert!(client.token.is_none());
    }

    #[test]
    fn test_api_client_with_token_method() {
        let client =
            ApiClient::new("http://localhost:42780", None).with_token("my-token".to_string());
        assert_eq!(client.token, Some("my-token".to_string()));
    }

    #[test]
    fn test_api_client_clone_preserves_url() {
        let client = ApiClient::new("http://custom:8080", Some("token".to_string()));
        let cloned = client.clone();
        assert_eq!(cloned.base_url, client.base_url);
    }

    #[test]
    fn test_api_client_clone_preserves_token() {
        let client = ApiClient::new("http://localhost:42780", Some("secret-token".to_string()));
        let cloned = client.clone();
        assert_eq!(cloned.token, client.token);
    }

    #[test]
    fn test_api_client_different_tokens() {
        let client1 = ApiClient::new("http://localhost:42780", Some("token1".to_string()));
        let client2 = ApiClient::new("http://localhost:42780", Some("token2".to_string()));
        assert_ne!(client1.token, client2.token);
    }

    #[test]
    fn test_api_client_with_empty_token() {
        let client = ApiClient::new("http://localhost:42780", Some("".to_string()));
        assert!(client.token.is_some());
        assert_eq!(client.token.unwrap(), "");
    }

    #[test]
    fn test_gitforge_client_is_api_client() {
        // GitForgeClient is an alias for ApiClient
        let client = GitForgeClient::new("http://localhost:42780", None);
        assert_eq!(client.base_url, "http://localhost:42780");
    }
}
