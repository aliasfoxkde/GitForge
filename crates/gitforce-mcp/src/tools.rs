//! Tool definitions for the GitForge MCP server

use serde::{Deserialize, Serialize};

/// Result from any tool call
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl ToolResult {
    pub fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: None,
        }
    }

    pub fn with_data<T: Serialize>(message: impl Into<String>, data: T) -> Self {
        Self {
            success: true,
            message: message.into(),
            data: Some(serde_json::to_value(data).unwrap()),
        }
    }

    pub fn err(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: message.into(),
            data: None,
        }
    }
}

/// Options for `ci_run`
#[derive(Debug, Clone, Deserialize)]
pub struct CiRunOptions {
    /// Repository in "owner/repo" format
    #[serde(default)]
    pub repo: Option<String>,
    /// Branch to run CI on
    #[serde(default)]
    pub branch: Option<String>,
    /// Run only affected packages (delta analysis)
    #[serde(default = "default_true")]
    pub delta: bool,
    /// Override execution scope: "trivial", "fast", "standard", "full", "heavy"
    #[serde(default)]
    pub scope: Option<String>,
    /// Comma-separated list of workflow names to run
    #[serde(default)]
    pub workflows: Option<String>,
    /// GitHub token (defaults to GITHUB_TOKEN env var)
    #[serde(default)]
    pub token: Option<String>,
}

fn default_true() -> bool {
    true
}

/// CI run job status
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiJob {
    pub id: String,
    pub name: String,
    pub status: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub conclusion: Option<String>,
    pub logs_url: Option<String>,
}

/// Status of a CI run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiStatus {
    pub run_id: u64,
    pub repo: String,
    pub branch: String,
    pub commit: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub jobs: Vec<CiJob>,
    pub triggered_by: String,
    pub run_at: String,
    pub url: String,
}

/// Result of delta analysis
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaResult {
    pub file_count: usize,
    pub lines_changed: usize,
    pub languages: Vec<String>,
    pub affected_packages: Vec<String>,
    pub execution_scope: String,
    pub workflows_to_run: Vec<String>,
    pub docs_only: bool,
    pub config_only: bool,
    pub ci_changed: bool,
    pub summary: String,
}

/// Repository info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoInfo {
    pub name: String,
    pub full_name: String,
    pub default_branch: String,
    pub visibility: String,
    pub ci_enabled: bool,
}

/// Repository configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepoConfig {
    pub default_branch: String,
    pub coverage_threshold: u32,
    pub languages: Vec<String>,
    pub workflows_enabled: Vec<String>,
    pub require_code_owner_review: bool,
    pub required_status_checks: Vec<String>,
}

/// Security scan result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityScanResult {
    pub category: String,
    pub finding_count: usize,
    pub severity_counts: SeverityCounts,
    pub findings: Vec<SecurityFinding>,
    pub scan_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeverityCounts {
    pub critical: usize,
    pub high: usize,
    pub medium: usize,
    pub low: usize,
    pub info: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityFinding {
    pub pattern_id: String,
    pub category: String,
    pub severity: String,
    pub file: String,
    pub line: Option<usize>,
    pub description: String,
    pub fix_suggestion: Option<String>,
}
