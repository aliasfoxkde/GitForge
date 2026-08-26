//! Unix socket client for build daemon communication
//!
//! This module provides a mockable interface for communicating with the build daemon.

use crate::{encode_request, Request, Response};
use anyhow::Result;

/// Default socket path
pub const DEFAULT_SOCKET: &str = "/tmp/gitforge-build.sock";

/// Result type for daemon operations
pub type DaemonResult = Result<Response>;

/// Trait for submitting jobs to the daemon - enables mocking in tests
#[async_trait::async_trait]
pub trait JobSubmitter: Send + Sync {
    /// Submit a job to the daemon
    async fn submit_job(
        &self,
        socket_path: &str,
        cargo_args: Vec<String>,
        working_dir: Option<String>,
    ) -> DaemonResult;

    /// List all jobs
    async fn list_jobs(&self, socket_path: &str) -> DaemonResult;

    /// Get daemon stats
    async fn get_stats(&self, socket_path: &str) -> DaemonResult;

    /// Cancel a queued or running job.
    async fn cancel_job(&self, socket_path: &str, job_id: String) -> DaemonResult;
}

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;

/// Real implementation using Unix sockets
pub struct UnixSocketClient;

impl UnixSocketClient {
    pub fn new() -> Self {
        Self
    }
}

impl Default for UnixSocketClient {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl JobSubmitter for UnixSocketClient {
    async fn submit_job(
        &self,
        socket_path: &str,
        cargo_args: Vec<String>,
        working_dir: Option<String>,
    ) -> DaemonResult {
        let mut stream = UnixStream::connect(socket_path).await?;

        let request = Request::Submit {
            cargo_args,
            working_dir,
        };
        let request_bytes = encode_request(&request)?;
        stream.write_all(&request_bytes).await?;
        stream.shutdown().await?;

        let response = read_response(&mut stream).await?;
        Ok(response)
    }

    async fn list_jobs(&self, socket_path: &str) -> DaemonResult {
        let mut stream = UnixStream::connect(socket_path).await?;

        let request = Request::List;
        let request_bytes = encode_request(&request)?;
        stream.write_all(&request_bytes).await?;
        stream.shutdown().await?;

        let response = read_response(&mut stream).await?;
        Ok(response)
    }

    async fn get_stats(&self, socket_path: &str) -> DaemonResult {
        let mut stream = UnixStream::connect(socket_path).await?;

        let request = Request::Stats;
        let request_bytes = encode_request(&request)?;
        stream.write_all(&request_bytes).await?;
        stream.shutdown().await?;

        let response = read_response(&mut stream).await?;
        Ok(response)
    }

    async fn cancel_job(&self, socket_path: &str, job_id: String) -> DaemonResult {
        let mut stream = UnixStream::connect(socket_path).await?;
        let request = Request::Cancel { job_id };
        stream.write_all(&encode_request(&request)?).await?;
        stream.shutdown().await?;
        Ok(read_response(&mut stream).await?)
    }
}

/// Read a response from the stream
async fn read_response(stream: &mut UnixStream) -> Result<Response> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = u32::from_le_bytes(len_buf) as usize;
    let mut response_bytes = vec![0u8; len];
    stream.read_exact(&mut response_bytes).await?;
    let response: Response = serde_json::from_slice(&response_bytes)?;
    Ok(response)
}

/// Mock client for testing
pub struct MockClient {
    pub submit_response: Response,
    pub list_response: Response,
    pub stats_response: Response,
}

impl MockClient {
    pub fn new(
        submit_response: Response,
        list_response: Response,
        stats_response: Response,
    ) -> Self {
        Self {
            submit_response,
            list_response,
            stats_response,
        }
    }

    pub fn with_submit_response(submit_response: Response) -> Self {
        Self {
            submit_response,
            list_response: Response::JobList { jobs: vec![] },
            stats_response: Response::Stats {
                running_count: 0,
                queued_count: 0,
                completed_count: 0,
                max_concurrent: 0,
            },
        }
    }
}

#[async_trait::async_trait]
impl JobSubmitter for MockClient {
    async fn submit_job(
        &self,
        _socket_path: &str,
        _cargo_args: Vec<String>,
        _working_dir: Option<String>,
    ) -> DaemonResult {
        Ok(self.submit_response.clone())
    }

    async fn list_jobs(&self, _socket_path: &str) -> DaemonResult {
        Ok(self.list_response.clone())
    }

    async fn get_stats(&self, _socket_path: &str) -> DaemonResult {
        Ok(self.stats_response.clone())
    }

    async fn cancel_job(&self, _socket_path: &str, _job_id: String) -> DaemonResult {
        Ok(Response::Error {
            message: "cancel unavailable in mock client".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Response;

    #[tokio::test]
    async fn test_mock_client_submit() {
        let client = MockClient::with_submit_response(Response::Submitted {
            job_id: "test-job-123".to_string(),
        });

        let result = client
            .submit_job("/fake", vec!["build".to_string()], None)
            .await;
        assert!(result.is_ok());

        if let Ok(Response::Submitted { job_id }) = result {
            assert_eq!(job_id, "test-job-123");
        }
    }

    #[tokio::test]
    async fn test_mock_client_submit_error() {
        let client = MockClient::with_submit_response(Response::Error {
            message: "test error".to_string(),
        });

        let result = client.submit_job("/fake", vec![], None).await;
        assert!(result.is_ok());

        if let Ok(Response::Error { message }) = result {
            assert_eq!(message, "test error");
        }
    }

    #[tokio::test]
    async fn test_mock_client_list_empty() {
        let client = MockClient::with_submit_response(Response::Submitted {
            job_id: "test".to_string(),
        });

        let result = client.list_jobs("/fake").await;
        assert!(result.is_ok());

        if let Ok(Response::JobList { jobs }) = result {
            assert!(jobs.is_empty());
        }
    }

    #[tokio::test]
    async fn test_mock_client_list_with_jobs() {
        use crate::JobInfo;

        let client = MockClient::new(
            Response::Submitted {
                job_id: "test".to_string(),
            },
            Response::JobList {
                jobs: vec![JobInfo {
                    job_id: "job-1".to_string(),
                    status: "running".to_string(),
                    cargo_args: vec!["build".to_string()],
                    wait_time_ms: 100,
                }],
            },
            Response::Stats {
                running_count: 1,
                queued_count: 0,
                completed_count: 10,
                max_concurrent: 2,
            },
        );

        let result = client.list_jobs("/fake").await;
        assert!(result.is_ok());

        if let Ok(Response::JobList { jobs }) = result {
            assert_eq!(jobs.len(), 1);
            assert_eq!(jobs[0].job_id, "job-1");
        }
    }

    #[tokio::test]
    async fn test_mock_client_stats() {
        let client = MockClient::new(
            Response::Submitted {
                job_id: "test".to_string(),
            },
            Response::JobList { jobs: vec![] },
            Response::Stats {
                running_count: 2,
                queued_count: 5,
                completed_count: 100,
                max_concurrent: 4,
            },
        );

        let result = client.get_stats("/fake").await;
        assert!(result.is_ok());

        if let Ok(Response::Stats {
            running_count,
            queued_count,
            completed_count,
            max_concurrent,
        }) = result
        {
            assert_eq!(running_count, 2);
            assert_eq!(queued_count, 5);
            assert_eq!(completed_count, 100);
            assert_eq!(max_concurrent, 4);
        }
    }

    #[test]
    fn test_default_socket_constant() {
        assert_eq!(DEFAULT_SOCKET, "/tmp/gitforge-build.sock");
    }

    #[test]
    fn test_mock_client_creation() {
        let client = MockClient::with_submit_response(Response::Submitted {
            job_id: "new".to_string(),
        });
        assert!(matches!(client.submit_response, Response::Submitted { .. }));
    }
}
