//! Provider-neutral publication of terminal GitForge receipts.

use crate::{JobReceipt, ReceiptStatus};
use anyhow::{anyhow, Context, Result};
use chrono::{DateTime, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerminalPublication {
    pub owner: String,
    pub repository: String,
    pub check_name: String,
    pub ref_name: String,
    pub head_sha: String,
    pub receipt: JobReceipt,
}

impl TerminalPublication {
    pub fn new(
        owner: impl Into<String>,
        repository: impl Into<String>,
        check_name: impl Into<String>,
        ref_name: impl Into<String>,
        head_sha: impl Into<String>,
        receipt: JobReceipt,
    ) -> Result<Self> {
        let publication = Self {
            owner: owner.into(),
            repository: repository.into(),
            check_name: check_name.into(),
            ref_name: ref_name.into(),
            head_sha: head_sha.into(),
            receipt,
        };
        publication
            .receipt
            .validate()
            .map_err(|error| anyhow!("invalid GitForge receipt: {error}"))?;
        if publication.owner.is_empty()
            || publication.repository.is_empty()
            || publication.check_name.is_empty()
            || publication.ref_name.is_empty()
            || publication.head_sha.is_empty()
        {
            return Err(anyhow!("GitHub publication fields must not be empty"));
        }
        if publication.owner.contains('/') || publication.repository.contains('/') {
            return Err(anyhow!(
                "GitHub owner and repository must be one path segment"
            ));
        }
        if !publication
            .head_sha
            .chars()
            .all(|character| character.is_ascii_hexdigit())
        {
            return Err(anyhow!("GitHub head SHA must be hexadecimal"));
        }
        Ok(publication)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationResponse {
    pub provider_id: Option<String>,
}

#[derive(Clone)]
pub struct GithubPublisher {
    client: reqwest::Client,
    base_url: String,
    token: String,
}

impl GithubPublisher {
    pub fn new(base_url: impl Into<String>, token: String, timeout: Duration) -> Result<Self> {
        if token.is_empty() {
            return Err(anyhow!("GitHub publisher token must not be empty"));
        }
        let base_url = base_url.into().trim_end_matches('/').to_owned();
        if base_url.is_empty() {
            return Err(anyhow!("GitHub publisher base URL must not be empty"));
        }
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .build()
            .context("build GitHub publisher HTTP client")?;
        Ok(Self {
            client,
            base_url,
            token,
        })
    }

    pub async fn publish(&self, publication: &TerminalPublication) -> Result<PublicationResponse> {
        let url = format!(
            "{}/repos/{}/{}/check-runs",
            self.base_url, publication.owner, publication.repository
        );
        let response = self
            .client
            .post(url)
            .header(ACCEPT, "application/vnd.github+json")
            .header(AUTHORIZATION, format!("Bearer {}", self.token))
            .header(CONTENT_TYPE, "application/json")
            .header(USER_AGENT, "gitforge")
            .json(&CheckRunRequest::from(publication))
            .send()
            .await
            .context("publish GitForge receipt to GitHub")?;
        let status = response.status();
        let payload: GithubResponse = response
            .json()
            .await
            .context("decode GitHub publication response")?;
        if !status.is_success() {
            return Err(anyhow!(
                "GitHub publication failed with HTTP {}: {}",
                status,
                payload.message.unwrap_or_else(|| "unknown error".into())
            ));
        }
        Ok(PublicationResponse {
            provider_id: payload.id.map(|id| id.to_string()),
        })
    }
}

#[derive(Debug, Serialize)]
struct CheckRunRequest<'a> {
    name: &'a str,
    head_sha: &'a str,
    status: &'static str,
    conclusion: &'static str,
    started_at: DateTime<Utc>,
    completed_at: DateTime<Utc>,
    output: CheckOutput<'a>,
}

#[derive(Debug, Serialize)]
struct CheckOutput<'a> {
    title: &'static str,
    summary: &'a str,
    text: Option<&'a str>,
}

impl<'a> From<&'a TerminalPublication> for CheckRunRequest<'a> {
    fn from(publication: &'a TerminalPublication) -> Self {
        let conclusion = match publication.receipt.status {
            ReceiptStatus::Succeeded => "success",
            ReceiptStatus::Failed => "failure",
            ReceiptStatus::TimedOut => "timed_out",
            ReceiptStatus::Cancelled => "cancelled",
        };
        Self {
            name: &publication.check_name,
            head_sha: &publication.head_sha,
            status: "completed",
            conclusion,
            started_at: publication.receipt.started_at,
            completed_at: publication.receipt.completed_at,
            output: CheckOutput {
                title: "GitForge job receipt",
                summary: publication
                    .receipt
                    .error
                    .as_deref()
                    .unwrap_or("GitForge job completed"),
                text: publication
                    .receipt
                    .logs
                    .as_ref()
                    .map(|log| log.uri.as_str()),
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct GithubResponse {
    id: Option<u64>,
    message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RECEIPT_VERSION;
    use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
    use gitforge_common::{JobId, PipelineRunId, RepoId};
    use serde_json::Value;
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn publishes_successful_receipt_as_github_check_run() {
        let requests = Arc::new(Mutex::new(Vec::<Value>::new()));
        let app =
            Router::new()
                .route(
                    "/repos/acme/widget/check-runs",
                    post(
                        |State(requests): State<Arc<Mutex<Vec<Value>>>>,
                         Json(body): Json<Value>| async move {
                            requests.lock().unwrap().push(body);
                            (StatusCode::CREATED, Json(serde_json::json!({"id": 42})))
                        },
                    ),
                )
                .with_state(requests.clone());
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let receipt = test_receipt(ReceiptStatus::Succeeded);
        let publication = TerminalPublication::new(
            "acme",
            "widget",
            "build",
            "refs/heads/main",
            "b".repeat(64),
            receipt,
        )
        .unwrap();
        let publisher = GithubPublisher::new(
            format!("http://{address}"),
            "test-token".into(),
            std::time::Duration::from_secs(2),
        )
        .unwrap();

        let response = publisher.publish(&publication).await.unwrap();

        assert_eq!(response.provider_id, Some("42".into()));
        let body = &requests.lock().unwrap()[0];
        assert_eq!(body["name"], "build");
        assert_eq!(body["head_sha"], "b".repeat(64));
        assert_eq!(body["status"], "completed");
        assert_eq!(body["conclusion"], "success");
    }

    #[test]
    fn rejects_invalid_publication_identity_and_token() {
        let receipt = test_receipt(ReceiptStatus::Failed);
        assert!(TerminalPublication::new(
            "acme/widget",
            "widget",
            "build",
            "main",
            "a",
            receipt.clone()
        )
        .is_err());
        assert!(
            TerminalPublication::new("acme", "widget", "build", "main", "not-a-sha", receipt)
                .is_err()
        );
        assert!(
            GithubPublisher::new("http://127.0.0.1:1", String::new(), Duration::from_secs(1))
                .is_err()
        );
    }

    #[tokio::test]
    async fn reports_non_success_github_response() {
        let app = Router::new().route(
            "/repos/acme/widget/check-runs",
            post(|| async {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(serde_json::json!({"message": "rate limited"})),
                )
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let publication = TerminalPublication::new(
            "acme",
            "widget",
            "build",
            "main",
            "a".repeat(64),
            test_receipt(ReceiptStatus::TimedOut),
        )
        .unwrap();
        let publisher = GithubPublisher::new(
            format!("http://{address}"),
            "test-token".into(),
            Duration::from_secs(2),
        )
        .unwrap();
        let error = publisher
            .publish(&publication)
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("HTTP 429"));
        assert!(error.contains("rate limited"));
    }

    fn test_receipt(status: ReceiptStatus) -> JobReceipt {
        let started_at = chrono::Utc::now();
        JobReceipt {
            receipt_version: RECEIPT_VERSION,
            work_request_id: Some("request-1".into()),
            pipeline_run_id: PipelineRunId::new(),
            job_id: JobId::new(),
            repository_id: Some(RepoId::new()),
            base_sha: None,
            head_sha: Some("b".repeat(64)),
            workspace_path: None,
            run_id: Some("run-1".into()),
            status,
            commands: vec!["cargo test".into()],
            working_directory: None,
            exit_code: Some(0),
            changed_paths: vec![],
            started_at,
            completed_at: started_at,
            output_sha: String::new(),
            output_bytes: 0,
            stable_uri: "gitforge://job/test".into(),
            log_uri: vec![],
            artifact_uri: vec![],
            logs: None,
            artifacts: vec![],
            error: None,
            receipt_signature: None,
        }
    }
}
