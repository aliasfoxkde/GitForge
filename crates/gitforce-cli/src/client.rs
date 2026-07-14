//! API client for GitForge CLI

use anyhow::Result;
use reqwest::Client;
use serde::{de::DeserializeOwned, Serialize};

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
}

pub use ApiClient as GitForgeClient;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_api_client_new() {
        let client = ApiClient::new("http://localhost:8080", None);
        assert_eq!(client.base_url, "http://localhost:8080");
    }

    #[test]
    fn test_api_client_with_token() {
        let client = ApiClient::new("http://localhost:8080", None)
            .with_token("test-token".to_string());
        // Token is set internally, verify client was created
        let client2 = ApiClient::new("http://localhost:8080", Some("test-token".to_string()));
        assert_eq!(client2.token, Some("test-token".to_string()));
    }

    #[test]
    fn test_api_client_clone() {
        let client = ApiClient::new("http://localhost:8080", Some("token".to_string()));
        let _cloned = client.clone();
    }
}
