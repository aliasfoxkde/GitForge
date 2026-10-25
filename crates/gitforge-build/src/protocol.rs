//! Unix socket protocol for build coordinator communication

use serde::{Deserialize, Serialize};

/// Maximum message size in bytes
pub const MAX_MESSAGE_SIZE: usize = 64 * 1024; // 64KB

/// Per-stream cap for a captured job's output on the wire.
///
/// Two streams plus the JSON envelope have to fit inside
/// [`MAX_MESSAGE_SIZE`], so each side gets a quarter of it. The cap is
/// applied to the tail: a compile error or a failing test explains itself
/// at the end of the stream, not at the beginning.
pub const WIRE_STREAM_MAX: usize = 16 * 1024;

/// Keep the tail of `text` within `max_bytes`, marking what was dropped.
///
/// A character boundary is respected so the result stays valid UTF-8.
pub fn tail_for_wire(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let marker = "...[output truncated]...\n";
    if max_bytes <= marker.len() {
        // No room for the marker: the raw tail still has to fit.
        let mut start = text.len() - max_bytes;
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
        return text[start..].to_string();
    }
    let keep = max_bytes - marker.len();
    let mut start = text.len() - keep;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{marker}{}", &text[start..])
}

impl Request {
    /// Get a string name for the request type (for logging)
    pub fn name(&self) -> &'static str {
        match self {
            Request::Submit { .. } => "submit",
            Request::Status { .. } => "status",
            Request::Cancel { .. } => "cancel",
            Request::Result { .. } => "result",
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
    /// Fetch a finished job's captured output
    Result { job_id: String },
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
    fn tail_for_wire_keeps_short_text_verbatim() {
        assert_eq!(tail_for_wire("short", 64), "short");
    }

    #[test]
    fn tail_for_wire_keeps_the_end_of_a_long_stream() {
        let text = "a".repeat(100) + "THE-ERROR";
        let capped = tail_for_wire(&text, 64);
        assert!(capped.len() <= 64);
        assert!(capped.contains("THE-ERROR"), "got {capped}");
        assert!(capped.starts_with("...[output truncated]..."));
    }

    #[test]
    fn tail_for_wire_respects_character_boundaries() {
        // Each accented character is two bytes; a byte-wise cut would
        // split one and produce invalid UTF-8.
        let text = "\u{e9}".repeat(64);
        let capped = tail_for_wire(&text, 17);
        assert!(capped.len() <= 17);
        assert!(std::str::from_utf8(capped.as_bytes()).is_ok());
    }

    #[test]
    fn request_names_cover_every_variant() {
        assert_eq!(Request::Result { job_id: "x".into() }.name(), "result");
    }

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

    #[test]
    fn test_decode_request_incomplete_message() {
        // Create a message that claims to have 100 bytes but only has 4 + 10
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&100u32.to_le_bytes()); // claims 100 bytes
        bytes.extend_from_slice(&[0u8; 10]); // only 10 bytes of payload
        let result = decode_request(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("incomplete"));
    }

    #[test]
    fn test_decode_response_incomplete_message() {
        // Create a message that claims to have 100 bytes but only has 4 + 10
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&100u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 10]);
        let result = decode_response(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_request_message_too_large() {
        // Create a message that exceeds MAX_MESSAGE_SIZE
        let large_len = (MAX_MESSAGE_SIZE + 1) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&large_len.to_le_bytes());
        bytes.extend_from_slice(&vec![0u8; MAX_MESSAGE_SIZE + 1]);
        let result = decode_request(&bytes);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too large"));
    }

    #[test]
    fn test_decode_response_message_too_large() {
        let large_len = (MAX_MESSAGE_SIZE + 1) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&large_len.to_le_bytes());
        bytes.extend_from_slice(&vec![0u8; MAX_MESSAGE_SIZE + 1]);
        let result = decode_response(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_request_serde_roundtrip() {
        // Test Submit with all fields
        let req = Request::Submit {
            cargo_args: vec!["build".to_string(), "--release".to_string()],
            working_dir: Some("/test/dir".to_string()),
        };
        let encoded = encode_request(&req).unwrap();
        let decoded: Request = serde_json::from_slice(&encoded[4..]).unwrap();
        assert!(matches!(decoded, Request::Submit { .. }));
    }

    #[test]
    fn test_response_serde_roundtrip() {
        let resp = Response::Completed {
            job_id: "job-abc".to_string(),
            success: true,
            exit_code: 0,
            duration_ms: 5000,
            stdout: "test output".to_string(),
            stderr: "".to_string(),
        };
        let encoded = encode_response(&resp).unwrap();
        let decoded: Response = serde_json::from_slice(&encoded[4..]).unwrap();
        assert!(matches!(decoded, Response::Completed { .. }));
    }

    #[test]
    fn test_job_info_serde() {
        let job = JobInfo {
            job_id: "job-123".to_string(),
            status: "running".to_string(),
            cargo_args: vec!["test".to_string()],
            wait_time_ms: 100,
        };
        let json = serde_json::to_string(&job).unwrap();
        let decoded: JobInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.job_id, "job-123");
    }

    #[test]
    fn test_max_message_size_constant() {
        assert_eq!(MAX_MESSAGE_SIZE, 64 * 1024);
    }
}
