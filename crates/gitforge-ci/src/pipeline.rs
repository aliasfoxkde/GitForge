//! Pipeline definition and parsing

use gitforge_common::{PipelineId, RepoId};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const DEFAULT_TIMEOUT_SECS: u64 = 300;
pub const MIN_TIMEOUT_SECS: u64 = 5;
pub const MAX_TIMEOUT_SECS: u64 = 24 * 60 * 60;

/// Parse a human-readable job timeout into bounded seconds.
///
/// Supported forms are plain seconds (`900`) and a single suffix of `s`,
/// `m`, or `h` (`45m`). Missing values use the safe legacy default. Compound
/// durations and zero/unbounded values are rejected so the scheduler and
/// runner share one deterministic contract.
pub fn parse_timeout_secs(value: Option<&str>) -> Result<u64, String> {
    let Some(raw) = value else {
        return Ok(DEFAULT_TIMEOUT_SECS);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("timeout must not be empty".to_string());
    }
    let (number, multiplier) = match raw.chars().last() {
        Some('s') => (&raw[..raw.len() - 1], 1),
        Some('m') => (&raw[..raw.len() - 1], 60),
        Some('h') => (&raw[..raw.len() - 1], 60 * 60),
        Some(last) if last.is_ascii_digit() => (raw, 1),
        _ => {
            return Err(format!(
                "invalid timeout '{}': use seconds, s, m, or h",
                raw
            ))
        }
    };
    let amount: u64 = number
        .parse()
        .map_err(|_| format!("invalid timeout '{}': expected a positive integer", raw))?;
    let seconds = amount
        .checked_mul(multiplier)
        .ok_or_else(|| format!("timeout '{}' overflows", raw))?;
    if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&seconds) {
        return Err(format!(
            "timeout '{}' must be between {} seconds and {} seconds",
            raw, MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS
        ));
    }
    Ok(seconds)
}

/// Pipeline trigger event
#[derive(Debug, Clone)]
pub struct PipelineTriggerEvent {
    pub pipeline_id: PipelineId,
    pub repo_id: RepoId,
    pub commit_hash: String,
    pub trigger_type: TriggerType,
    pub ref_name: Option<String>,
    pub actor_id: Option<gitforge_common::UserId>,
}

impl PipelineTriggerEvent {
    pub fn new(
        pipeline_id: PipelineId,
        repo_id: RepoId,
        commit_hash: String,
        trigger_type: TriggerType,
    ) -> Self {
        Self {
            pipeline_id,
            repo_id,
            commit_hash,
            trigger_type,
            ref_name: None,
            actor_id: None,
        }
    }

    pub fn with_ref(mut self, ref_name: String) -> Self {
        self.ref_name = Some(ref_name);
        self
    }

    pub fn with_actor(mut self, actor_id: gitforge_common::UserId) -> Self {
        self.actor_id = Some(actor_id);
        self
    }
}

/// Trigger type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerType {
    Push,
    Tag,
    PullRequest,
    Manual,
}

impl TriggerType {
    pub fn as_str(&self) -> &'static str {
        match self {
            TriggerType::Push => "push",
            TriggerType::Tag => "tag",
            TriggerType::PullRequest => "pull_request",
            TriggerType::Manual => "manual",
        }
    }
}

/// Pipeline definition (loaded from .gitforce.yml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDefinition {
    pub name: String,
    pub version: String,
    pub trigger_on: Vec<TriggerType>,
    pub environment: HashMap<String, String>,
    pub jobs: Vec<JobDefinition>,
}

impl PipelineDefinition {
    /// Parse pipeline definition from YAML
    pub fn parse(yaml: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(yaml)
    }

    /// Convert to YAML
    pub fn to_yaml(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }
}

/// Job definition within a pipeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobDefinition {
    pub name: String,
    pub image: String,
    #[serde(default)]
    pub needs: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    pub steps: Vec<StepDefinition>,
    pub timeout: Option<String>,
    pub retry: Option<u32>,
}

impl JobDefinition {
    /// Check if this job has any dependencies
    pub fn has_dependencies(&self) -> bool {
        !self.needs.is_empty()
    }

    pub fn timeout_secs(&self) -> Result<u64, String> {
        parse_timeout_secs(self.timeout.as_deref())
    }
}

/// Step definition within a job
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepDefinition {
    pub name: String,
    pub run: String,
    pub env: Option<HashMap<String, String>>,
    pub working_directory: Option<String>,
    pub condition: Option<String>,
}

impl StepDefinition {
    /// Get environment variables for this step
    pub fn get_env(&self) -> &HashMap<String, String> {
        self.env.as_ref().unwrap_or(&EMPTY_ENV)
    }
}

lazy_static::lazy_static! {
    static ref EMPTY_ENV: HashMap<String, String> = HashMap::new();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_pipeline() {
        let yaml = r#"
name: test-pipeline
version: "1.0"
trigger_on:
  - push
environment:
  RUST_BACKTRACE: "1"
jobs:
  - name: build
    image: rust:latest
    steps:
      - name: build
        run: cargo build
"#;

        let pipeline = PipelineDefinition::parse(yaml).unwrap();
        assert_eq!(pipeline.name, "test-pipeline");
        assert_eq!(pipeline.jobs.len(), 1);
        assert_eq!(pipeline.jobs[0].name, "build");
    }

    #[test]
    fn test_parse_timeout_secs_bounds_and_units() {
        assert_eq!(parse_timeout_secs(None).unwrap(), DEFAULT_TIMEOUT_SECS);
        assert_eq!(parse_timeout_secs(Some("5s")).unwrap(), 5);
        assert_eq!(parse_timeout_secs(Some("45m")).unwrap(), 2700);
        assert_eq!(parse_timeout_secs(Some("1h")).unwrap(), 3600);
        assert_eq!(parse_timeout_secs(Some("900")).unwrap(), 900);
        assert!(parse_timeout_secs(Some("0s")).is_err());
        assert!(parse_timeout_secs(Some("2d")).is_err());
        assert!(parse_timeout_secs(Some("86401")).is_err());
    }

    #[test]
    fn test_trigger_type() {
        assert_eq!(TriggerType::Push.as_str(), "push");
        assert_eq!(TriggerType::Tag.as_str(), "tag");
        assert_eq!(TriggerType::PullRequest.as_str(), "pull_request");
        assert_eq!(TriggerType::Manual.as_str(), "manual");
    }

    #[test]
    fn test_trigger_type_variants() {
        assert!(matches!(TriggerType::Push, TriggerType::Push));
        assert!(matches!(TriggerType::Tag, TriggerType::Tag));
        assert!(matches!(TriggerType::PullRequest, TriggerType::PullRequest));
        assert!(matches!(TriggerType::Manual, TriggerType::Manual));
    }

    #[test]
    fn test_pipeline_definition_to_yaml() {
        let def = PipelineDefinition {
            name: "test".to_string(),
            version: "1.0".to_string(),
            trigger_on: vec![TriggerType::Push],
            environment: HashMap::new(),
            jobs: vec![],
        };
        let yaml = def.to_yaml().unwrap();
        assert!(yaml.contains("test"));
    }

    #[test]
    fn test_job_definition_has_dependencies() {
        let job = JobDefinition {
            name: "build".to_string(),
            image: "rust:latest".to_string(),
            needs: vec!["dep".to_string()],
            env: HashMap::new(),
            steps: vec![],
            timeout: None,
            retry: None,
        };
        assert!(job.has_dependencies());

        let job2 = JobDefinition {
            name: "build".to_string(),
            image: "rust:latest".to_string(),
            needs: vec![],
            env: HashMap::new(),
            steps: vec![],
            timeout: None,
            retry: None,
        };
        assert!(!job2.has_dependencies());
    }

    #[test]
    fn test_step_definition_get_env() {
        let mut env = HashMap::new();
        env.insert("RUST_BACKTRACE".to_string(), "1".to_string());

        let step = StepDefinition {
            name: "build".to_string(),
            run: "cargo build".to_string(),
            env: Some(env),
            working_directory: None,
            condition: None,
        };
        assert_eq!(step.get_env().get("RUST_BACKTRACE"), Some(&"1".to_string()));
    }

    #[test]
    fn test_step_definition_get_env_empty() {
        let step = StepDefinition {
            name: "build".to_string(),
            run: "cargo build".to_string(),
            env: None,
            working_directory: None,
            condition: None,
        };
        assert!(step.get_env().is_empty());
    }

    #[test]
    fn test_pipeline_trigger_event_new() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        );
        assert_eq!(event.commit_hash, "abc123");
        assert!(event.ref_name.is_none());
        assert!(event.actor_id.is_none());
    }

    #[test]
    fn test_pipeline_trigger_event_with_ref() {
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        )
        .with_ref("refs/heads/main".to_string());

        assert_eq!(event.ref_name, Some("refs/heads/main".to_string()));
    }

    #[test]
    fn test_pipeline_trigger_event_with_actor() {
        let user_id = gitforge_common::UserId::new();
        let event = PipelineTriggerEvent::new(
            PipelineId::new(),
            RepoId::new(),
            "abc123".to_string(),
            TriggerType::Push,
        )
        .with_actor(user_id);

        assert_eq!(event.actor_id, Some(user_id));
    }
}
