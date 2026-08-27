//! Versioned, bounded execution receipts shared by GitForge consumers.

use gitforge_common::{JobId, PipelineRunId, RepoId, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The receipt schema version exchanged with Control Center and other local
/// consumers. Increment this when a breaking field/meaning change is made.
pub const RECEIPT_VERSION: u32 = 2;
/// Prevent relational metadata from becoming an unbounded log/artifact sink.
pub const MAX_LOG_BYTES: u64 = 64 * 1024;
pub const MAX_ARTIFACT_BYTES: u64 = 256 * 1024 * 1024;

/// URI scheme for job receipt references
pub const JOB_RECEIPT_URI_SCHEME: &str = "gitforge";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiptStatus {
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogReceipt {
    /// Stable local storage identifier, never an arbitrary filesystem path.
    pub uri: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArtifactReceipt {
    pub name: String,
    /// Stable local storage identifier, never an arbitrary filesystem path.
    pub uri: String,
    pub sha256: String,
    pub bytes: u64,
    pub media_type: Option<String>,
}

/// Extended job receipt with complete provenance chain.
///
/// Captures: exact SHA (base + head), workspace path, run ID, log file paths,
/// artifact paths, and cryptographic integrity signature.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JobReceipt {
    pub receipt_version: u32,
    pub work_request_id: Option<String>,
    pub pipeline_run_id: PipelineRunId,
    pub job_id: JobId,
    pub repository_id: Option<RepoId>,
    /// Base SHA before any changes (merge-base)
    pub base_sha: Option<String>,
    /// Head SHA after changes (the commit being tested)
    pub head_sha: Option<String>,
    /// Workspace path where the job executed
    pub workspace_path: Option<String>,
    /// Run identifier for this execution
    pub run_id: Option<String>,
    pub status: ReceiptStatus,
    pub commands: Vec<String>,
    pub working_directory: Option<String>,
    pub exit_code: Option<i32>,
    pub changed_paths: Vec<String>,
    pub started_at: chrono::DateTime<chrono::Utc>,
    pub completed_at: chrono::DateTime<chrono::Utc>,
    /// SHA-256 of the concatenated artifact checksums (empty string if no artifacts)
    pub output_sha: String,
    /// Total bytes of all artifacts
    pub output_bytes: u64,
    /// Stable URI for this receipt (gitforge://job/{job_id})
    pub stable_uri: String,
    /// URIs for log files produced by this job
    pub log_uri: Vec<String>,
    /// URIs for artifacts produced by this job
    pub artifact_uri: Vec<String>,
    pub logs: Option<LogReceipt>,
    pub artifacts: Vec<ArtifactReceipt>,
    pub error: Option<String>,
    /// SHA-256 signature of the canonical receipt JSON for integrity verification
    pub receipt_signature: Option<String>,
}

impl JobReceipt {
    /// Validate the storage/transport invariants before persistence.
    pub fn validate(&self) -> Result<()> {
        if self.receipt_version != RECEIPT_VERSION {
            return Err(gitforge_common::Error::invalid_input(format!(
                "unsupported job receipt version {}",
                self.receipt_version
            )));
        }
        if let Some(base_sha) = &self.base_sha {
            if base_sha.is_empty() {
                return Err(gitforge_common::Error::invalid_input(
                    "job receipt base_sha must not be empty when present",
                ));
            }
            validate_digest(base_sha)?;
        }
        if let Some(head_sha) = &self.head_sha {
            if !head_sha.is_empty() {
                validate_digest(head_sha)?;
            }
        }
        if self.completed_at < self.started_at {
            return Err(gitforge_common::Error::invalid_input(
                "job receipt completed_at must not be before started_at",
            ));
        }
        // output_sha is empty when there are no artifacts
        if !self.output_sha.is_empty() {
            validate_digest(&self.output_sha)?;
        }
        if let Some(log) = &self.logs {
            validate_digest(&log.sha256)?;
            if log.bytes > MAX_LOG_BYTES {
                return Err(gitforge_common::Error::invalid_input(format!(
                    "job log exceeds {} byte limit",
                    MAX_LOG_BYTES
                )));
            }
        }
        for artifact in &self.artifacts {
            validate_digest(&artifact.sha256)?;
            if artifact.bytes > MAX_ARTIFACT_BYTES {
                return Err(gitforge_common::Error::invalid_input(format!(
                    "artifact {} exceeds {} byte limit",
                    artifact.name, MAX_ARTIFACT_BYTES
                )));
            }
        }
        // Validate receipt signature if present
        if let Some(sig) = &self.receipt_signature {
            validate_digest(sig)?;
        }
        Ok(())
    }

    /// Compute SHA-256 signature of this receipt for integrity verification.
    ///
    /// Signs the canonical JSON representation of the receipt (excluding the
    /// signature field itself to allow self-referential verification).
    pub fn compute_signature(&self) -> String {
        // Clone without signature for canonical serialization
        let receipt_for_signing = Self {
            receipt_signature: None,
            ..self.clone()
        };
        let canonical =
            serde_json::to_string(&receipt_for_signing).unwrap_or_else(|_| "{}".to_string());
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Verify the receipt's cryptographic integrity.
    ///
    /// Returns Ok(()) if the receipt has no signature (unsigned receipts are valid
    /// but unauthenticated), or if the computed signature matches the stored one.
    /// Returns Err if the signature does not match.
    pub fn verify_signature(&self) -> Result<()> {
        match &self.receipt_signature {
            Some(sig) => {
                let computed = self.compute_signature();
                if computed == *sig {
                    Ok(())
                } else {
                    Err(gitforge_common::Error::invalid_input(
                        "receipt signature mismatch - receipt may have been tampered with",
                    ))
                }
            }
            None => Ok(()), // Unsigned receipts are valid but unauthenticated
        }
    }
}

fn validate_digest(digest: &str) -> Result<()> {
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(gitforge_common::Error::invalid_input(
            "receipt checksum must be a SHA-256 hex digest",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> JobReceipt {
        let started = chrono::Utc::now();
        let completed = started + chrono::Duration::seconds(10);
        // Valid 64-char hex strings for SHA-256 digests
        let base = "a".repeat(64);
        let head = "b".repeat(64);
        let output = "c".repeat(64);
        let log_sha = "d".repeat(64);
        let artifact_sha = "e".repeat(64);
        JobReceipt {
            receipt_version: RECEIPT_VERSION,
            work_request_id: Some("wr-1".into()),
            pipeline_run_id: PipelineRunId::new(),
            job_id: JobId::new(),
            repository_id: Some(RepoId::new()),
            base_sha: Some(base),
            head_sha: Some(head),
            workspace_path: Some("/workspace/abc123".into()),
            run_id: Some("run-001".into()),
            status: ReceiptStatus::Succeeded,
            commands: vec!["cargo test".into()],
            working_directory: Some("/workspace".into()),
            exit_code: Some(0),
            changed_paths: vec!["src/lib.rs".into()],
            started_at: started,
            completed_at: completed,
            output_sha: output,
            output_bytes: 20,
            stable_uri: "gitforge://job/test-job".into(),
            log_uri: vec!["gitforge://log/test-job".into()],
            artifact_uri: vec!["gitforge://artifact/test-job/report.json".into()],
            logs: Some(LogReceipt {
                uri: "gitforge://log/test-job".into(),
                sha256: log_sha,
                bytes: 12,
            }),
            artifacts: vec![ArtifactReceipt {
                name: "report.json".into(),
                uri: "gitforge://artifact/test-job/report.json".into(),
                sha256: artifact_sha,
                bytes: 20,
                media_type: Some("application/json".into()),
            }],
            error: None,
            receipt_signature: None,
        }
    }

    #[test]
    fn validates_and_round_trips_versioned_receipt() {
        let value = receipt();
        value.validate().unwrap();
        let json = serde_json::to_string(&value).unwrap();
        let decoded: JobReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, value);
    }

    #[test]
    fn rejects_invalid_checksum() {
        let mut value = receipt();
        value.logs.as_mut().unwrap().sha256 = "bad".into();
        assert!(value.validate().is_err());
    }

    #[test]
    fn rejects_oversized_log() {
        let mut value = receipt();
        value.logs.as_mut().unwrap().bytes = MAX_LOG_BYTES + 1;
        assert!(value.validate().is_err());
    }

    #[test]
    fn rejects_unknown_version() {
        let mut value = receipt();
        value.receipt_version = RECEIPT_VERSION + 1;
        assert!(value.validate().is_err());
    }

    #[test]
    fn rejects_completed_before_started() {
        let mut value = receipt();
        let earlier = value.started_at - chrono::Duration::seconds(1);
        value.completed_at = earlier;
        assert!(value.validate().is_err());
    }

    #[test]
    fn accepts_empty_output_sha_with_no_artifacts() {
        let mut value = receipt();
        value.output_sha = String::new();
        value.artifacts.clear();
        assert!(value.validate().is_ok());
    }

    #[test]
    fn computes_and_verifies_signature() {
        let value = receipt();
        let signature = value.compute_signature();
        // Signature should be a valid SHA-256 hex digest
        assert_eq!(signature.len(), 64);
        assert!(signature.chars().all(|c| c.is_ascii_hexdigit()));

        // Verify the signature
        let mut signed_receipt = value;
        signed_receipt.receipt_signature = Some(signature.clone());
        assert!(signed_receipt.verify_signature().is_ok());
    }

    #[test]
    fn detect_tampered_receipt() {
        let value = receipt();
        let signature = value.compute_signature();
        let mut signed_receipt = value;
        signed_receipt.receipt_signature = Some(signature);

        // Tamper with the receipt
        signed_receipt.exit_code = Some(1);

        // Verification should fail
        assert!(signed_receipt.verify_signature().is_err());
    }

    #[test]
    fn accepts_unsigned_receipt() {
        let value = receipt();
        // No signature set
        assert!(value.verify_signature().is_ok());
    }

    #[test]
    fn validates_head_sha_digest() {
        let mut value = receipt();
        value.head_sha = Some("bad".to_string());
        assert!(value.validate().is_err());
    }

    #[test]
    fn validates_receipt_signature_digest() {
        let mut value = receipt();
        value.receipt_signature = Some("bad".to_string());
        assert!(value.validate().is_err());
    }
}
