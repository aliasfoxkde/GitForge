//! Pipeline definition and parsing

use gitforge_common::{Fault, JobCache, PipelineId, RepoId, RolloutFlags, VolumeSpec};
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
        _ => return Err(format!("invalid timeout '{raw}': use seconds, s, m, or h")),
    };
    let amount: u64 = number
        .parse()
        .map_err(|_| format!("invalid timeout '{raw}': expected a positive integer"))?;
    let seconds = amount
        .checked_mul(multiplier)
        .ok_or_else(|| format!("timeout '{raw}' overflows"))?;
    if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&seconds) {
        return Err(format!(
            "timeout '{raw}' must be between {MIN_TIMEOUT_SECS} seconds and {MAX_TIMEOUT_SECS} seconds"
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

/// Pipeline definition (loaded from `.gitforge.yml`; legacy `.gitforce.yml`
/// accepted)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineDefinition {
    pub name: String,
    pub version: String,
    pub trigger_on: Vec<TriggerType>,
    /// Optional: pipelines that need no shared environment omit the key.
    /// Absent in older pipelines; an empty map is the safe default.
    #[serde(default)]
    pub environment: HashMap<String, String>,
    pub jobs: Vec<JobDefinition>,
}

impl PipelineDefinition {
    /// Parse a pipeline definition with the deployment's rollout flags.
    ///
    /// The flags are data on purpose: the decision (accept or reject the
    /// `fault:` surface) never reads the environment itself — the edge
    /// (`RolloutFlags::from_env`, service startup) reads it once.
    pub fn parse_with_flags(yaml: &str, flags: &RolloutFlags) -> Result<Self, serde_yaml::Error> {
        let value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
        reject_unknown_keys(&value, flags.fault_injection).map_err(|message| {
            use serde::de::Error as _;
            serde_yaml::Error::custom(message)
        })?;
        serde_yaml::from_value(value)
    }

    /// Parse pipeline definition from YAML. Fault injection is a rollout
    /// surface, so the default (flag-off) parse REJECTS a `fault:` key with
    /// a clear error instead of silently dropping or honouring it.
    pub fn parse(yaml: &str) -> Result<Self, serde_yaml::Error> {
        Self::parse_with_flags(yaml, &RolloutFlags::default())
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
    /// Skip this job entirely when the shell expression evaluates to false.
    /// The expression is evaluated in a bash subshell with access to CI-change
    /// variables written by the `detect-changes` job:
    ///   CHANGES_INCLUDE_CODE      — non-doc Rust/source files changed
    ///   CHANGES_INCLUDE_DOCS      — documentation files changed
    ///   CHANGES_INCLUDE_CRATES     — specific gitforge-* crates changed
    ///   CHANGES_INCLUDE_TESTS      — test files changed
    ///   CHANGES_INCLUDE_INFRA      — infrastructure/docker/scripts changed
    ///   CHANGES_FILE_COUNT        — total number of changed files
    /// When absent the job always runs. When the expression is false the job
    /// transitions to Skipped (terminal, counted as succeeded for deps).
    #[serde(default)]
    pub skip_if: Option<String>,
    /// Host mounts this job needs (cache directories, tmpfs scratch).
    /// Absent in older pipelines; an empty list is the safe default.
    #[serde(default)]
    pub volumes: Vec<VolumeSpec>,
    /// Compiled-artifact cache mounted into the job's container. The DAG
    /// builder expands it into a bind mount plus the environment variable
    /// that points the toolchain at it, so every downstream hop carries
    /// plain volumes and env.
    #[serde(default)]
    pub cache: Option<JobCache>,
    /// Scheduling lane (#338): fast / standard / heavy / background. The
    /// assigner uses it for per-lane admission and reserved fast-lane
    /// capacity. Absent means Standard — pre-lane pipelines keep their
    /// legacy scheduling behavior exactly.
    #[serde(default)]
    pub lane: gitforge_common::Lane,
    /// Failure-injection request (M10). Only parsed when the deployment
    /// enabled fault injection (`RolloutFlags::fault_injection`); the
    /// unknown-key gate rejects the key outright otherwise. `None` keeps
    /// legacy behavior: the runner executes the steps for real.
    #[serde(default)]
    pub fault: Option<Fault>,
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

const PIPELINE_KEYS: &[&str] = &["name", "version", "trigger_on", "environment", "jobs"];
const JOB_KEYS: &[&str] = &[
    "name", "image", "needs", "env", "steps", "timeout", "retry", "skip_if", "volumes", "cache",
    "lane", "fault",
];

/// Reject YAML keys the parser would otherwise silently drop. A stale binary
/// (deployed before a field existed) degrades the committed pipeline — a
/// dropped `cache:` cold-builds every job, a dropped `skip_if:` runs jobs
/// that should have been skipped — so failing loudly is the safe behavior.
/// The error names the offending key and points at deploy skew.
fn reject_unknown_keys(value: &serde_yaml::Value, allow_fault: bool) -> Result<(), String> {
    fn check(
        mapping: &serde_yaml::Mapping,
        allowed: &[&str],
        what: &str,
        job_name: Option<&str>,
    ) -> Result<(), String> {
        for key in mapping.keys() {
            if let serde_yaml::Value::String(k) = key {
                if !allowed.contains(&k.as_str()) {
                    let label = match job_name {
                        Some(name) => format!("job '{name}'"),
                        None => what.to_string(),
                    };
                    if what == "job" && k == "fault" {
                        let label = job_name.map_or_else(|| what.to_string(), str::to_owned);
                        return Err(format!(
                            "{label} sets 'fault' but fault injection is disabled on this \
                             deployment: set GITFORGE_ENABLE_FAULT_INJECTION=1 on the API \
                             and CI services to enable the rollout flag, or remove the \
                             'fault' key before committing the pipeline"
                        ));
                    }
                    return Err(format!(
                        "unknown {what} key '{k}' in {label}: the deployed binary predates \
                         this pipeline field, or the key is misspelled — refusing to \
                         silently ignore it"
                    ));
                }
            }
        }
        Ok(())
    }
    let Some(mapping) = value.as_mapping() else {
        return Ok(());
    };
    check(mapping, PIPELINE_KEYS, "pipeline", None)?;
    let Some(jobs) = mapping.get(serde_yaml::Value::from("jobs")) else {
        return Ok(());
    };
    if let Some(jobs) = jobs.as_sequence() {
        for (idx, job) in jobs.iter().enumerate() {
            let Some(job) = job.as_mapping() else {
                continue;
            };
            let name = job
                .get(serde_yaml::Value::from("name"))
                .and_then(|v| v.as_str())
                .map_or_else(|| format!("#{idx}"), str::to_owned);
            let allowed: &[&str] = if allow_fault {
                JOB_KEYS
            } else {
                JOB_KEYS_NO_FAULT
            };
            check(job, allowed, "job", Some(&name))?;
            if !allow_fault {
                if let Some(fault_val) = job.get(serde_yaml::Value::from("fault")) {
                    return Err(format!(
                        "job '{name}' sets 'fault' ({fault_val:?}) but fault injection is \
                         disabled on this deployment: set GITFORGE_ENABLE_FAULT_INJECTION=1 \
                         on the API and CI services to enable the rollout flag, or remove \
                         the 'fault' key before committing the pipeline"
                    ));
                }
            }
        }
    }
    Ok(())
}

/// Job keys accepted when fault injection is disabled: `fault` is excluded
/// so a fault-bearing pipeline fails validation with the standard
/// unknown-key error plus a rollout-specific hint.
const JOB_KEYS_NO_FAULT: &[&str] = &[
    "name", "image", "needs", "env", "steps", "timeout", "retry", "skip_if", "volumes", "cache",
    "lane",
];

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
    fn test_parse_rejects_unknown_job_key() {
        // A binary from before `cache` existed must fail loudly, not
        // silently run the degraded pipeline.
        let yaml = r#"
name: stale-binary
version: "1.0"
trigger_on: [push]
jobs:
  - name: clippy
    image: dsc-ci-rust:10
    steps:
      - name: clippy
        run: cargo clippy
    caches:
      source: "{cache_root}/cargo/{image}"
"#;
        let error = PipelineDefinition::parse(yaml).unwrap_err().to_string();
        assert!(
            error.contains("unknown"),
            "error mentions unknown key: {error}"
        );
        assert!(error.contains("caches"), "error names the key: {error}");
        assert!(error.contains("clippy"), "error names the job: {error}");
    }

    #[test]
    fn test_parse_rejects_unknown_pipeline_key() {
        let yaml = r#"
name: typo
version: "1.0"
trigger_on: [push]
enviroment: {}
jobs: []
"#;
        let error = PipelineDefinition::parse(yaml).unwrap_err().to_string();
        assert!(error.contains("enviroment"), "error names the key: {error}");
    }

    #[test]
    fn test_parse_accepts_all_documented_keys() {
        let yaml = r#"
name: full
version: "1.0"
trigger_on: [push, pull_request]
environment: {}
jobs:
  - name: job
    image: img:1
    needs: []
    env: {}
    steps:
      - name: step
        run: echo hi
    timeout: 45m
    retry: 1
    skip_if: 'false'
    volumes: []
    cache:
      source: "{cache_root}/x"
"#;
        let pipeline = PipelineDefinition::parse(yaml).unwrap();
        assert_eq!(pipeline.jobs.len(), 1);
        assert!(pipeline.jobs[0].cache.is_some());
    }

    #[test]
    fn test_parse_job_with_cache_and_volumes() {
        let yaml = r#"
name: cached
version: "1.0"
trigger_on:
  - push
environment: {}
jobs:
  - name: test
    image: dsc-ci-rust:10
    cache:
      source: "{cache_root}/cargo/{image}"
      mount_path: /cargo-target
    volumes:
      - type: tmpfs
        destination: /scratch
        size: 4G
    steps:
      - name: run
        run: cargo test
"#;

        let pipeline = PipelineDefinition::parse(yaml).unwrap();
        let job = &pipeline.jobs[0];
        let cache = job.cache.as_ref().expect("cache block");
        assert_eq!(cache.source, "{cache_root}/cargo/{image}");
        assert_eq!(cache.mount_path, "/cargo-target");
        // The env var defaults to the one every cargo job wants.
        assert_eq!(cache.env_var, "CARGO_TARGET_DIR");
        assert_eq!(job.volumes.len(), 1);
        assert_eq!(
            job.volumes[0].resolved_type(),
            gitforge_common::VolumeType::Tmpfs
        );
        assert_eq!(job.volumes[0].destination, "/scratch");
    }

    #[test]
    fn test_parse_job_without_cache_has_no_volumes() {
        let yaml = r#"
name: plain
version: "1.0"
trigger_on:
  - push
environment: {}
jobs:
  - name: test
    image: dsc-ci-rust:10
    steps:
      - name: run
        run: cargo test
"#;

        let pipeline = PipelineDefinition::parse(yaml).unwrap();
        assert!(pipeline.jobs[0].cache.is_none());
        assert!(pipeline.jobs[0].volumes.is_empty());
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
            skip_if: None,
            volumes: Vec::new(),
            cache: None,
            lane: Default::default(),
            fault: None,
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
            skip_if: None,
            volumes: Vec::new(),
            cache: None,
            lane: Default::default(),
            fault: None,
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

    // --- fault injection rollout gate (M10) ------------------------------

    const FAULT_YAML: &str = r#"
name: faulty
version: "1.0"
trigger_on: [push]
jobs:
  - name: build
    image: img:1
    fault: oom
    steps:
      - name: build
        run: cargo build
"#;

    #[test]
    fn test_parse_rejects_fault_key_when_injection_disabled() {
        // Default flags: fault injection is OFF. The key must be rejected
        // with a clear, actionable error - never silently dropped.
        let error = PipelineDefinition::parse(FAULT_YAML)
            .unwrap_err()
            .to_string();
        assert!(error.contains("fault"), "error names the key: {error}");
        assert!(
            error.contains("GITFORGE_ENABLE_FAULT_INJECTION"),
            "error explains the rollout gate: {error}"
        );
    }

    #[test]
    fn test_parse_accepts_fault_key_when_injection_enabled() {
        let flags = RolloutFlags {
            fault_injection: true,
        };
        let pipeline = PipelineDefinition::parse_with_flags(FAULT_YAML, &flags).unwrap();
        assert_eq!(pipeline.jobs[0].fault, Some(Fault::Oom));
    }

    #[test]
    fn test_parse_with_disabled_flags_rejects_fault_key() {
        assert!(
            PipelineDefinition::parse_with_flags(FAULT_YAML, &RolloutFlags::default()).is_err()
        );
    }

    #[test]
    fn test_parse_fault_all_kinds_and_rejects_bad_value() {
        let flags = RolloutFlags {
            fault_injection: true,
        };
        for fault in Fault::all() {
            let yaml = format!(
                "name: f\nversion: \"1.0\"\ntrigger_on: [push]\njobs:\n  - name: j\n    image: i\n    fault: {}\n    steps: []\n",
                fault.as_str()
            );
            let pipeline = PipelineDefinition::parse_with_flags(&yaml, &flags).unwrap();
            assert_eq!(pipeline.jobs[0].fault, Some(fault));
        }
        let bad = "name: f\nversion: \"1.0\"\ntrigger_on: [push]\njobs:\n  - name: j\n    image: i\n    fault: segfault\n    steps: []\n";
        assert!(PipelineDefinition::parse_with_flags(bad, &flags).is_err());
    }

    #[test]
    fn test_parse_without_fault_key_is_unaffected_by_flag() {
        let plain = "name: p\nversion: \"1.0\"\ntrigger_on: [push]\njobs:\n  - name: j\n    image: i\n    steps: []\n";
        let on = PipelineDefinition::parse_with_flags(
            plain,
            &RolloutFlags {
                fault_injection: true,
            },
        )
        .unwrap();
        let off = PipelineDefinition::parse_with_flags(plain, &RolloutFlags::default()).unwrap();
        assert!(on.jobs[0].fault.is_none());
        assert!(off.jobs[0].fault.is_none());
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
