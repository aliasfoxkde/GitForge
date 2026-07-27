//! Unix socket protocol for build coordinator communication

use serde::{Deserialize, Serialize};

/// Maximum message size in bytes
const MAX_MESSAGE_SIZE: usize = 64 * 1024; // 64KB

impl Request {
    /// Get a string name for the request type (for logging)
    pub fn name(&self) -> &'static str {
        match self {
            Request::Submit { .. } => "submit",
            Request::Status { .. } => "status",
            Request::Cancel { .. } => "cancel",
            Request::List => "list",
            Request::Stats => "stats",
            Request::Shutdown => "shutdown",
        }
    }
}

/// Request message from client to daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    /// Submit a new build job
    Submit {
        cargo_args: Vec<String>,
        working_dir: Option<String>,
    },
    /// Check job status
    Status { job_id: String },
    /// Cancel a running job
    Cancel { job_id: String },
    /// List all jobs
    List,
    /// Get daemon stats
    Stats,
    /// Shutdown the daemon
    Shutdown,
}

/// Response message from daemon to client
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Response {
    /// Job submitted successfully
    Submitted { job_id: String },
    /// Job completed
    Completed {
        job_id: String,
        success: bool,
        exit_code: i32,
        duration_ms: u64,
        stdout: String,
        stderr: String,
    },
    /// Job status
    Status {
        job_id: String,
        status: String,
        wait_time_ms: u64,
    },
    /// List of all jobs
    JobList { jobs: Vec<JobInfo> },
    /// Daemon stats
    Stats {
        running_count: usize,
        queued_count: usize,
        completed_count: u64,
        max_concurrent: usize,
    },
    /// Error response
    Error { message: String },
    /// Shutdown acknowledgment
    Shutdown,
}

/// Information about a job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInfo {
    pub job_id: String,
    pub status: String,
    pub cargo_args: Vec<String>,
    pub wait_time_ms: u64,
}

/// Encode a request message
pub fn encode_request(req: &Request) -> anyhow::Result<Vec<u8>> {
    let json = serde_json::to_vec(req)?;
    let len = json.len() as u32;
    let mut bytes = Vec::with_capacity(4 + json.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&json);
    Ok(bytes)
}

/// Decode a request message
pub fn decode_request(bytes: &[u8]) -> anyhow::Result<Request> {
    if bytes.len() < 4 {
        anyhow::bail!("message too short");
    }
    let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() < 4 + len {
        anyhow::bail!(
            "incomplete message: expected {} bytes, got {}",
            len,
            bytes.len() - 4
        );
    }
    if bytes.len() > 4 + MAX_MESSAGE_SIZE {
        anyhow::bail!("message too large");
    }
    let json = &bytes[4..4 + len];
    let req = serde_json::from_slice(json)?;
    Ok(req)
}

/// Encode a response message
pub fn encode_response(resp: &Response) -> anyhow::Result<Vec<u8>> {
    let json = serde_json::to_vec(resp)?;
    let len = json.len() as u32;
    let mut bytes = Vec::with_capacity(4 + json.len());
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&json);
    Ok(bytes)
}

/// Decode a response message
pub fn decode_response(bytes: &[u8]) -> anyhow::Result<Response> {
    if bytes.len() < 4 {
        anyhow::bail!("message too short");
    }
    let len = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if bytes.len() < 4 + len {
        anyhow::bail!("incomplete message");
    }
    if bytes.len() > 4 + MAX_MESSAGE_SIZE {
        anyhow::bail!("message too large");
    }
    let json = &bytes[4..4 + len];
    let resp = serde_json::from_slice(json)?;
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_decode_request() {
        let req = Request::Submit {
            cargo_args: vec!["test".to_string(), "--workspace".to_string()],
            working_dir: Some("/path/to/project".to_string()),
        };
        let encoded = encode_request(&req).unwrap();
        let decoded = decode_request(&encoded).unwrap();
        assert!(matches!(decoded, Request::Submit { .. }));
    }

    #[test]
    fn test_encode_decode_response() {
        let resp = Response::Stats {
            running_count: 1,
            queued_count: 2,
            completed_count: 100,
            max_concurrent: 2,
        };
        let encoded = encode_response(&resp).unwrap();
        let decoded = decode_response(&encoded).unwrap();
        assert!(matches!(decoded, Response::Stats { .. }));
    }

    #[test]
    fn test_request_names() {
        assert_eq!(
            Request::Submit {
                cargo_args: vec![],
                working_dir: None
            }
            .name(),
            "submit"
        );
        assert_eq!(
            Request::Status {
                job_id: "123".to_string()
            }
            .name(),
            "status"
        );
        assert_eq!(
            Request::Cancel {
                job_id: "123".to_string()
            }
            .name(),
            "cancel"
        );
        assert_eq!(Request::List.name(), "list");
        assert_eq!(Request::Stats.name(), "stats");
        assert_eq!(Request::Shutdown.name(), "shutdown");
    }

    #[test]
    fn test_encode_decode_all_request_types() {
        let requests = vec![
            Request::Submit {
                cargo_args: vec!["build".to_string()],
                working_dir: None,
            },
            Request::Status {
                job_id: "test-123".to_string(),
            },
            Request::Cancel {
                job_id: "test-456".to_string(),
            },
            Request::List,
            Request::Stats,
            Request::Shutdown,
        ];

        for req in requests {
            let encoded = encode_request(&req).unwrap();
            let decoded = decode_request(&encoded).unwrap();
            // Check type matches
            match (&req, &decoded) {
                (Request::Submit { .. }, Request::Submit { .. }) => (),
                (Request::Status { .. }, Request::Status { .. }) => (),
                (Request::Cancel { .. }, Request::Cancel { .. }) => (),
                (Request::List, Request::List) => (),
                (Request::Stats, Request::Stats) => (),
                (Request::Shutdown, Request::Shutdown) => (),
                _ => panic!("type mismatch"),
            }
        }
    }

    #[test]
    fn test_encode_decode_all_response_types() {
        let responses = vec![
            Response::Submitted {
                job_id: "test-123".to_string(),
            },
            Response::Completed {
                job_id: "test-456".to_string(),
                success: true,
                exit_code: 0,
                duration_ms: 100,
                stdout: "OK".to_string(),
                stderr: "".to_string(),
            },
            Response::Error {
                message: "test error".to_string(),
            },
            Response::Status {
                job_id: "test-789".to_string(),
                status: "running".to_string(),
                wait_time_ms: 50,
            },
            Response::JobList { jobs: vec![] },
            Response::Stats {
                running_count: 1,
                queued_count: 2,
                completed_count: 100,
                max_concurrent: 2,
            },
            Response::Shutdown,
        ];

        for resp in responses {
            let encoded = encode_response(&resp).unwrap();
            let decoded = decode_response(&encoded).unwrap();
            // Just verify decode succeeded
            assert!(matches!(decoded, _));
        }
    }

    #[test]
    fn test_decode_request_too_short() {
        let result = decode_request(&[0, 0]);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_response_too_short() {
        let result = decode_response(&[0, 0]);
        assert!(result.is_err());
    }

    #[test]
    fn test_encode_request_empty_args() {
        let req = Request::Submit {
            cargo_args: vec![],
            working_dir: None,
        };
        let encoded = encode_request(&req).unwrap();
        let decoded = decode_request(&encoded).unwrap();
        assert!(matches!(decoded, Request::Submit { cargo_args, .. } if cargo_args.is_empty()));
    }
}
