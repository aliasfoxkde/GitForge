//! Fail-closed reconciler for containers abandoned by lost runner attempts.
//!
//! A GitForge runner can be terminated (supervisor interruption, OOM, host
//! reboot) after creating a sandbox container but before the normal destroy
//! path ran. `DockerSandbox::remove_job_containers` only handles a *known*
//! job ID; it cannot discover an attempt whose queue row or runner process
//! was lost. This module provides a bounded, ownership-strict reconciler:
//!
//! 1. **Ownership** — only containers carrying the exact label
//!    `com.gitforce.managed=true` *and* a well-formed `com.gitforce.job_id`
//!    (a parseable UUID) are ever considered. Names, images, age, and exit
//!    codes are never ownership evidence.
//! 2. **State correlation** — the caller supplies the authoritative set of
//!    active/claimed job IDs. A container belonging to an active job is
//!    always retained. An unknown job is a *candidate*, not proof of
//!    abandonment; the grace period and deletion policy decide the rest.
//! 3. **Grace period** — a container must have been in a non-running state
//!    for at least `grace` before it is eligible. Running containers are
//!    never removed regardless of metadata age.
//! 4. **Concurrency** — removal is idempotent; Docker 404 (already gone)
//!    and 409 (concurrent teardown in progress) are treated as successful
//!    no-ops so a race with the normal teardown never fails.
//! 5. **Dry run** — `census` is read-only and reports the decision
//!    (`retain`/`eligible`) with the reason for every owned container.
//! 6. **Deletion authority** — removal happens only when
//!    `ReconcilerPolicy::deletion_enabled` is true. The default policy is
//!    census-only.
//! 7. **Receipt** — the resulting [`ReconcileReport`] serializes counts,
//!    container IDs, decisions, and outcomes. It never contains logs,
//!    tokens, or environment values.

use bollard::query_parameters::{ListContainersOptions, RemoveContainerOptions};
use chrono::DateTime;
use gitforge_common::{Error, Result};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

/// The exact managed-ownership label GitForge stamps on sandbox containers.
pub const MANAGED_LABEL: &str = "com.gitforce.managed";
/// Label carrying the owning job ID (UUID string).
pub const JOB_ID_LABEL: &str = "com.gitforce.job_id";

/// Why a container was retained or deemed eligible.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "decision", content = "reason")]
pub enum Decision {
    /// Exact managed label mismatch — not ours, never touched.
    RetainNotOwned,
    /// Missing or malformed job label — fail-closed, never touched.
    RetainMalformedLabel,
    /// Container is still running; age is irrelevant.
    RetainRunning,
    /// Owning job is currently active/claimed per authoritative state.
    RetainActiveJob,
    /// Non-running for less than the configured grace period.
    RetainGracePeriod,
    /// Deletion policy is disabled; eligible in principle, not removed.
    RetainPolicyDisabled,
    /// Abandoned per policy: unknown job, past grace. Subject to removal
    /// only when deletion is enabled.
    Eligible,
}

impl Decision {
    pub fn is_eligible(&self) -> bool {
        matches!(self, Decision::Eligible)
    }
}

/// One container observed during a census pass.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContainerRecord {
    /// Docker container ID (64-hex or short form as reported by the API).
    pub id: String,
    /// Labels as reported by the runtime (may be empty).
    pub labels: HashMap<String, String>,
    /// True while the container is running.
    pub running: bool,
    /// Millisecond timestamp of when the container was observed to exit
    /// (`FinishedAt` from the Docker inspect/list state), if known.
    pub exited_at_ms: Option<i64>,
    /// Wall-clock age in milliseconds of the container itself
    /// (`Created` timestamp), if known.
    pub created_at_ms: Option<i64>,
}

/// Timestamps injected by the caller so classification is deterministic
/// and unit-testable.
#[derive(Debug, Clone, Copy)]
pub struct Now {
    /// Current wall-clock time in milliseconds.
    pub now_ms: i64,
}

/// Policy controlling reconciler behavior. Constructed explicitly; there
/// is no ambient configuration so service policy is auditable.
#[derive(Debug, Clone)]
pub struct ReconcilerPolicy {
    /// Master switch for deletion. When false the reconciler is
    /// census-only and never issues a remove call. Default: false
    /// (census-only) until the policy is qualified.
    pub deletion_enabled: bool,
    /// Minimum time a container must have been non-running before it can
    /// be eligible for removal. Default: 1 hour. Must be positive.
    pub grace: Duration,
    /// Upper bound on containers removed per reconcile pass. Bounded
    /// blast radius. Default: 64.
    pub max_removals: usize,
    /// Per-Docker-call timeout so one hung API call cannot stall job
    /// admission. Default: 10 seconds.
    pub call_timeout: Duration,
}

impl Default for ReconcilerPolicy {
    fn default() -> Self {
        Self {
            deletion_enabled: false,
            grace: Duration::from_secs(3600),
            max_removals: 64,
            call_timeout: Duration::from_secs(10),
        }
    }
}

impl ReconcilerPolicy {
    /// Fail-closed validation: all timing and blast-radius limits must be
    /// positive.
    pub fn validate(&self) -> Result<()> {
        if self.grace.is_zero() {
            return Err(Error::sandbox(
                "reconciler policy rejected: grace period must be positive",
            ));
        }
        if self.max_removals == 0 {
            return Err(Error::sandbox(
                "reconciler policy rejected: max_removals must be positive",
            ));
        }
        if self.call_timeout.is_zero() {
            return Err(Error::sandbox(
                "reconciler policy rejected: call timeout must be positive",
            ));
        }
        Ok(())
    }
}

/// Pure decision function: classify one container against policy and the
/// authoritative active-job set. No I/O; the same inputs always produce
/// the same decision.
pub fn classify(
    record: &ContainerRecord,
    active_jobs: &HashSet<String>,
    policy: &ReconcilerPolicy,
    now: Now,
) -> Decision {
    // 1. Ownership: exact managed label, exact value "true".
    if record.labels.get(MANAGED_LABEL).map(|value| value.as_str()) != Some("true") {
        return Decision::RetainNotOwned;
    }
    // 2. Valid job identity: present and parseable as a UUID.
    let Some(raw_job) = record.labels.get(JOB_ID_LABEL) else {
        return Decision::RetainMalformedLabel;
    };
    if uuid::Uuid::parse_str(raw_job).is_err() {
        return Decision::RetainMalformedLabel;
    }
    // 3. State correlation: never remove an active job's container.
    if active_jobs.contains(raw_job) {
        return Decision::RetainActiveJob;
    }
    // 4. Never remove a running container.
    if record.running {
        return Decision::RetainRunning;
    }
    // 5. Grace period: must be non-running for at least `grace`.
    //    Fail-closed: an unknown exit time never qualifies.
    let Some(exited_at) = record.exited_at_ms else {
        return Decision::RetainGracePeriod;
    };
    let non_running_for = now.now_ms.saturating_sub(exited_at);
    if non_running_for < policy.grace.as_millis() as i64 {
        return Decision::RetainGracePeriod;
    }
    // 6. Deletion authority.
    if !policy.deletion_enabled {
        return Decision::RetainPolicyDisabled;
    }
    Decision::Eligible
}

/// Outcome of one removal attempt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemovalOutcome {
    Removed,
    /// 404/409 race: the container was already removed or is being
    /// removed by the normal teardown. Idempotent success.
    AlreadyGone,
    Failed,
}

/// A single census/reconcile entry in the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportEntry {
    pub container_id: String,
    pub job_id: Option<String>,
    pub running: bool,
    /// Age of the exited container in ms, when computable.
    pub exited_age_ms: Option<i64>,
    pub decision: Decision,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub removal: Option<RemovalOutcome>,
}

/// Durable report for a reconcile pass. Serializes without environment
/// values, tokens, or logs — only IDs, decisions, and outcomes.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReconcileReport {
    pub runtime: String,
    pub policy_deletion_enabled: bool,
    pub policy_grace_secs: u64,
    pub active_job_count: usize,
    /// Hash of the active-job snapshot (FNV-1a over sorted IDs) so the
    /// receipt is comparable across runs without embedding job data.
    pub active_jobs_hash: String,
    pub candidates_seen: usize,
    pub eligible: usize,
    pub removed: usize,
    pub already_gone: usize,
    pub failed: usize,
    pub entries: Vec<ReportEntry>,
}

/// Abstract container operations so the reconcile engine is testable
/// without a Docker daemon.
#[async_trait::async_trait]
pub trait ContainerSource: Send + Sync {
    /// List candidate containers (the implementation may pre-filter on
    /// the managed label for efficiency, but classification re-verifies).
    async fn list_containers(&self) -> Result<Vec<ContainerRecord>>;
    /// Remove one container. Must tolerate 404/409 as idempotent success
    /// ([`RemovalOutcome::AlreadyGone`]).
    async fn remove_container(&self, id: &str) -> Result<RemovalOutcome>;
}

fn fnv1a_hex(input: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in input.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// The reconcile engine. Holds policy and source; every method is bounded
/// and idempotent.
pub struct Reconciler<S: ContainerSource> {
    source: Arc<S>,
    policy: ReconcilerPolicy,
}

impl<S: ContainerSource> Reconciler<S> {
    pub fn new(source: Arc<S>, policy: ReconcilerPolicy) -> Self {
        Self { source, policy }
    }

    pub fn policy(&self) -> &ReconcilerPolicy {
        &self.policy
    }

    /// Read-only census: lists owned containers and reports decisions
    /// without issuing any remove call.
    pub async fn census(&self, active_jobs: &HashSet<String>, now: Now) -> Result<ReconcileReport> {
        self.policy().validate()?;
        let containers = self.source.list_containers().await?;
        let mut report = Self::base_report(active_jobs);
        report.policy_deletion_enabled = self.policy.deletion_enabled;
        report.policy_grace_secs = self.policy.grace.as_secs();
        report.runtime = "docker".to_string();

        for record in containers {
            let decision = classify(&record, active_jobs, &self.policy, now);
            let job_id = record.labels.get(JOB_ID_LABEL).cloned();
            let exited_age_ms = record.exited_at_ms.map(|t| now.now_ms.saturating_sub(t));
            report.candidates_seen += 1;
            if decision.is_eligible() {
                report.eligible += 1;
            }
            report.entries.push(ReportEntry {
                container_id: record.id,
                job_id,
                running: record.running,
                exited_age_ms,
                decision,
                removal: None,
            });
        }
        Ok(report)
    }

    /// Reconcile pass: census followed by bounded removal of eligible
    /// containers (only when `deletion_enabled`).
    pub async fn reconcile(
        &self,
        active_jobs: &HashSet<String>,
        now: Now,
    ) -> Result<ReconcileReport> {
        let mut report = self.census(active_jobs, now).await?;
        if !self.policy.deletion_enabled {
            return Ok(report);
        }
        for entry in report
            .entries
            .iter_mut()
            .filter(|e| e.decision.is_eligible())
            .take(self.policy.max_removals)
        {
            let outcome = self.source.remove_container(&entry.container_id).await?;
            match outcome {
                RemovalOutcome::Removed => report.removed += 1,
                RemovalOutcome::AlreadyGone => report.already_gone += 1,
                RemovalOutcome::Failed => report.failed += 1,
            }
            entry.removal = Some(outcome);
        }
        Ok(report)
    }

    fn base_report(active_jobs: &HashSet<String>) -> ReconcileReport {
        let mut sorted: Vec<&String> = active_jobs.iter().collect();
        sorted.sort();
        ReconcileReport {
            runtime: "docker".to_string(),
            policy_deletion_enabled: bool::default(),
            policy_grace_secs: u64::default(),
            active_job_count: active_jobs.len(),
            active_jobs_hash: fnv1a_hex(
                &sorted
                    .iter()
                    .map(|value| value.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            ),
            candidates_seen: 0,
            eligible: 0,
            removed: 0,
            already_gone: 0,
            failed: 0,
            entries: Vec::new(),
        }
    }
}

/// Docker-backed [`ContainerSource`] built on the same bollard connection
/// the sandbox uses. Listing pre-filters on the managed label (an
/// efficiency measure only — [`classify`] re-verifies ownership).
pub struct DockerContainerSource {
    docker: bollard::Docker,
    call_timeout: Duration,
}

impl DockerContainerSource {
    /// Connect to the local Docker daemon. Fails if Docker is not
    /// reachable — the reconciler never runs against a stub.
    pub fn connect() -> Result<Self> {
        Self::connect_with_timeout(Duration::from_secs(10))
    }

    /// Connect with a bounded timeout applied to every Docker API operation.
    pub fn connect_with_timeout(call_timeout: Duration) -> Result<Self> {
        if call_timeout.is_zero() {
            return Err(Error::sandbox(
                "reconciler Docker call timeout must be positive",
            ));
        }
        let docker = bollard::Docker::connect_with_local_defaults()
            .map_err(|e| Error::sandbox(format!("reconciler docker connect failed: {}", e)))?;
        Ok(Self {
            docker,
            call_timeout,
        })
    }

    pub fn new(docker: bollard::Docker) -> Self {
        Self::new_with_timeout(docker, Duration::from_secs(10))
    }

    pub fn new_with_timeout(docker: bollard::Docker, call_timeout: Duration) -> Self {
        Self {
            docker,
            call_timeout,
        }
    }
}

#[async_trait::async_trait]
impl ContainerSource for DockerContainerSource {
    async fn list_containers(&self) -> Result<Vec<ContainerRecord>> {
        let mut filters = HashMap::new();
        filters.insert("label".to_string(), vec![format!("{MANAGED_LABEL}=true")]);
        let containers = tokio::time::timeout(
            self.call_timeout,
            self.docker.list_containers(Some(ListContainersOptions {
                all: true,
                filters: Some(filters),
                ..Default::default()
            })),
        )
        .await
        .map_err(|_| Error::sandbox("reconciler list timed out"))?
        .map_err(|e| Error::sandbox(format!("reconciler list failed: {}", e)))?;

        let mut records = Vec::with_capacity(containers.len());
        for c in containers {
            let running = matches!(
                c.state,
                Some(bollard::models::ContainerSummaryStateEnum::RUNNING)
            );
            let created_at_ms = c.created.map(|s| s * 1000);
            let exited_at_ms = if running {
                None
            } else {
                // The list response does not contain authoritative finish
                // time. Never infer it from Created: an old container may
                // have exited moments ago. Missing or malformed state is
                // intentionally retained by the classifier.
                match c.id.as_deref() {
                    Some(id) => match tokio::time::timeout(
                        self.call_timeout,
                        self.docker.inspect_container(id, None),
                    )
                    .await
                    {
                        Ok(Ok(details)) => details
                            .state
                            .and_then(|state| state.finished_at)
                            .and_then(|raw| DateTime::parse_from_rfc3339(&raw).ok())
                            .map(|timestamp| timestamp.timestamp_millis()),
                        _ => None,
                    },
                    None => None,
                }
            };
            records.push(ContainerRecord {
                id: c.id.unwrap_or_default(),
                labels: c.labels.unwrap_or_default(),
                running,
                exited_at_ms,
                created_at_ms,
            });
        }
        Ok(records)
    }

    async fn remove_container(&self, id: &str) -> Result<RemovalOutcome> {
        match tokio::time::timeout(
            self.call_timeout,
            self.docker.remove_container(
                id,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            ),
        )
        .await
        {
            Err(_) => Ok(RemovalOutcome::Failed),
            Ok(Ok(())) => Ok(RemovalOutcome::Removed),
            Ok(Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404 | 409,
                ..
            })) => Ok(RemovalOutcome::AlreadyGone),
            Ok(Err(e)) => {
                tracing::warn!(%id, "reconciler remove failed: {}", e);
                Ok(RemovalOutcome::Failed)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ReconcilerPolicy {
        ReconcilerPolicy {
            deletion_enabled: true,
            grace: Duration::from_secs(3600),
            ..ReconcilerPolicy::default()
        }
    }

    fn owned(job: &str, running: bool, exited_age_ms: Option<i64>) -> ContainerRecord {
        ContainerRecord {
            id: format!("cid-{job}"),
            labels: HashMap::from([
                (MANAGED_LABEL.to_string(), "true".to_string()),
                (JOB_ID_LABEL.to_string(), job.to_string()),
            ]),
            running,
            exited_at_ms: exited_age_ms.map(|age| 10_000_000 - age),
            created_at_ms: Some(9_000_000),
        }
    }

    const NOW_MS: i64 = 10_000_000;

    fn now() -> Now {
        Now { now_ms: NOW_MS }
    }

    #[test]
    fn non_gitforge_container_is_never_owned() {
        // No managed label at all.
        let plain = ContainerRecord {
            id: "plain".into(),
            labels: HashMap::new(),
            running: false,
            exited_at_ms: Some(0),
            created_at_ms: Some(0),
        };
        assert_eq!(
            classify(&plain, &HashSet::new(), &policy(), now()),
            Decision::RetainNotOwned
        );

        // Managed label with the wrong value.
        let wrong_value = ContainerRecord {
            labels: HashMap::from([
                (MANAGED_LABEL.to_string(), "1".to_string()),
                (JOB_ID_LABEL.to_string(), uuid::Uuid::new_v4().to_string()),
            ]),
            ..plain.clone()
        };
        assert_eq!(
            classify(&wrong_value, &HashSet::new(), &policy(), now()),
            Decision::RetainNotOwned
        );

        // Name-prefix ownership is not ownership: a record that looks like
        // a GitForge container by id alone (no labels) is retained.
        let name_only = ContainerRecord {
            id: "gitforge-job-abc".into(),
            ..plain.clone()
        };
        assert_eq!(
            classify(&name_only, &HashSet::new(), &policy(), now()),
            Decision::RetainNotOwned
        );
    }

    #[test]
    fn malformed_or_missing_job_label_fails_closed() {
        let plain = ContainerRecord {
            id: "x".into(),
            labels: HashMap::from([(MANAGED_LABEL.to_string(), "true".to_string())]),
            running: false,
            exited_at_ms: Some(0),
            created_at_ms: Some(0),
        };
        assert_eq!(
            classify(&plain, &HashSet::new(), &policy(), now()),
            Decision::RetainMalformedLabel
        );
        let bad = ContainerRecord {
            labels: HashMap::from([
                (MANAGED_LABEL.to_string(), "true".to_string()),
                (
                    JOB_ID_LABEL.to_string(),
                    "gitforge-job-not-a-uuid".to_string(),
                ),
            ]),
            ..plain.clone()
        };
        assert_eq!(
            classify(&bad, &HashSet::new(), &policy(), now()),
            Decision::RetainMalformedLabel
        );
    }

    #[test]
    fn running_container_never_removed_even_when_ancient() {
        let rec = owned(
            &uuid::Uuid::new_v4().to_string(),
            true,
            Some(10 * 365 * 24 * 3600 * 1000),
        );
        assert_eq!(
            classify(&rec, &HashSet::new(), &policy(), now()),
            Decision::RetainRunning
        );
    }

    #[test]
    fn active_job_container_retained() {
        let job = uuid::Uuid::new_v4().to_string();
        let rec = owned(&job, false, Some(10 * 365 * 24 * 3600 * 1000));
        let active = HashSet::from([job.clone()]);
        assert_eq!(
            classify(&rec, &active, &policy(), now()),
            Decision::RetainActiveJob
        );
    }

    #[test]
    fn grace_period_enforced() {
        let job = uuid::Uuid::new_v4().to_string();
        // Exited 59 minutes ago: inside grace.
        let fresh = owned(&job, false, Some(59 * 60 * 1000));
        assert_eq!(
            classify(&fresh, &HashSet::new(), &policy(), now()),
            Decision::RetainGracePeriod
        );
        // Exited exactly at grace: eligible.
        let boundary = owned(&job, false, Some(3600 * 1000));
        assert_eq!(
            classify(&boundary, &HashSet::new(), &policy(), now()),
            Decision::Eligible
        );
        // Unknown exit time: fail closed.
        let unknown = owned(&job, false, None);
        assert_eq!(
            classify(&unknown, &HashSet::new(), &policy(), now()),
            Decision::RetainGracePeriod
        );
    }

    #[test]
    fn deletion_disabled_yields_policy_retention_not_eligible() {
        let job = uuid::Uuid::new_v4().to_string();
        let rec = owned(&job, false, Some(10 * 365 * 24 * 3600 * 1000));
        let census_only = ReconcilerPolicy {
            deletion_enabled: false,
            ..policy()
        };
        assert_eq!(
            classify(&rec, &HashSet::new(), &census_only, now()),
            Decision::RetainPolicyDisabled
        );
    }

    #[test]
    fn policy_validation_rejects_zero_grace() {
        let bad = ReconcilerPolicy {
            grace: Duration::ZERO,
            ..policy()
        };
        assert!(bad.validate().is_err());
        assert!(policy().validate().is_ok());
    }

    /// Mock Docker API: records remove calls and can simulate 404/409.
    struct MockSource {
        containers: std::sync::Mutex<Vec<ContainerRecord>>,
        removed: std::sync::Mutex<Vec<String>>,
        /// Error statuses to return once per ID before succeeding.
        race_errors: std::sync::Mutex<HashMap<String, u16>>,
    }

    impl MockSource {
        fn new(containers: Vec<ContainerRecord>) -> Self {
            Self {
                containers: std::sync::Mutex::new(containers),
                removed: std::sync::Mutex::new(Vec::new()),
                race_errors: std::sync::Mutex::new(HashMap::new()),
            }
        }

        fn removed_ids(&self) -> Vec<String> {
            self.removed.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl ContainerSource for MockSource {
        async fn list_containers(&self) -> Result<Vec<ContainerRecord>> {
            Ok(self.containers.lock().unwrap().clone())
        }

        async fn remove_container(&self, id: &str) -> Result<RemovalOutcome> {
            // Simulate 404/409 races: configured IDs fail once with the
            // status, then succeed — proving idempotent handling.
            if let Some(status) = self.race_errors.lock().unwrap().remove(id) {
                if status == 404 || status == 409 {
                    return Ok(RemovalOutcome::AlreadyGone);
                }
                return Ok(RemovalOutcome::Failed);
            }
            self.removed.lock().unwrap().push(id.to_string());
            Ok(RemovalOutcome::Removed)
        }
    }

    #[tokio::test]
    async fn census_is_read_only() {
        let job = uuid::Uuid::new_v4().to_string();
        let eligible = owned(&job, false, Some(10 * 365 * 24 * 3600 * 1000));
        let unrelated = ContainerRecord {
            id: "unrelated".into(),
            labels: HashMap::new(),
            running: true,
            exited_at_ms: None,
            created_at_ms: None,
        };
        let source = Arc::new(MockSource::new(vec![eligible, unrelated]));
        let reconciler = Reconciler::new(source.clone(), policy());
        let report = reconciler.census(&HashSet::new(), now()).await.unwrap();

        assert_eq!(report.candidates_seen, 2);
        assert_eq!(report.eligible, 1);
        // Census never mutates.
        assert!(source.removed_ids().is_empty());
        assert!(report.entries.iter().all(|e| e.removal.is_none()));
    }

    #[tokio::test]
    async fn reconcile_removes_only_eligible_owned_containers() {
        let abandoned = uuid::Uuid::new_v4().to_string();
        let active_job = uuid::Uuid::new_v4().to_string();
        let fresh = uuid::Uuid::new_v4().to_string();
        let containers = vec![
            owned(&abandoned, false, Some(10 * 365 * 24 * 3600 * 1000)),
            owned(&active_job, false, Some(10 * 365 * 24 * 3600 * 1000)),
            owned(&fresh, false, Some(30 * 1000)),
        ];
        let source = Arc::new(MockSource::new(containers));
        let reconciler = Reconciler::new(source.clone(), policy());
        let active = HashSet::from([active_job.clone()]);
        let report = reconciler.reconcile(&active, now()).await.unwrap();

        assert_eq!(report.removed, 1);
        assert_eq!(source.removed_ids(), vec![format!("cid-{abandoned}")]);
    }

    #[tokio::test]
    async fn reconcile_without_deletion_authority_never_removes() {
        let job = uuid::Uuid::new_v4().to_string();
        let source = Arc::new(MockSource::new(vec![owned(
            &job,
            false,
            Some(10 * 365 * 24 * 3600 * 1000),
        )]));
        let reconciler = Reconciler::new(
            source.clone(),
            ReconcilerPolicy {
                deletion_enabled: false,
                ..policy()
            },
        );
        let report = reconciler.reconcile(&HashSet::new(), now()).await.unwrap();
        assert_eq!(report.removed, 0);
        assert_eq!(report.eligible, 0);
        assert!(source.removed_ids().is_empty());
    }

    #[test]
    fn policy_rejects_zero_call_timeout() {
        let policy = ReconcilerPolicy {
            call_timeout: Duration::ZERO,
            ..policy()
        };
        assert!(policy.validate().is_err());
    }

    #[tokio::test]
    async fn docker_404_409_races_are_idempotent_success() {
        let job = uuid::Uuid::new_v4().to_string();
        let source = MockSource::new(vec![owned(&job, false, Some(10 * 365 * 24 * 3600 * 1000))]);
        let cid = format!("cid-{job}");
        source.race_errors.lock().unwrap().insert(cid.clone(), 409);
        let source = Arc::new(source);
        let reconciler = Reconciler::new(source.clone(), policy());
        let report = reconciler.reconcile(&HashSet::new(), now()).await.unwrap();

        assert_eq!(report.removed, 0);
        assert_eq!(report.already_gone, 1);
        assert_eq!(report.failed, 0);
        // The racing teardown removed it — reconciler did not retry blindly.
        assert!(source.removed_ids().is_empty());
    }

    #[tokio::test]
    async fn second_run_is_idempotent() {
        let job = uuid::Uuid::new_v4().to_string();
        let source = Arc::new(MockSource::new(vec![owned(
            &job,
            false,
            Some(10 * 365 * 24 * 3600 * 1000),
        )]));
        let reconciler = Reconciler::new(source.clone(), policy());

        let first = reconciler.reconcile(&HashSet::new(), now()).await.unwrap();
        assert_eq!(first.removed, 1);

        // After the first pass the container is gone; the second pass sees
        // nothing and removes nothing.
        source.containers.lock().unwrap().clear();
        let second = reconciler.reconcile(&HashSet::new(), now()).await.unwrap();
        assert_eq!(second.candidates_seen, 0);
        assert_eq!(second.removed, 0);
        assert_eq!(source.removed_ids().len(), 1, "removed exactly once total");
    }

    #[tokio::test]
    async fn max_removals_bounds_blast_radius() {
        let containers: Vec<_> = (0..10)
            .map(|_| {
                let job = uuid::Uuid::new_v4().to_string();
                owned(&job, false, Some(10 * 365 * 24 * 3600 * 1000))
            })
            .collect();
        let source = Arc::new(MockSource::new(containers));
        let bounded = ReconcilerPolicy {
            max_removals: 3,
            ..policy()
        };
        bounded.validate().unwrap();
        let reconciler = Reconciler::new(source.clone(), bounded);
        let report = reconciler.reconcile(&HashSet::new(), now()).await.unwrap();
        assert_eq!(report.removed, 3);
        assert_eq!(report.eligible, 10);
        assert_eq!(source.removed_ids().len(), 3);
    }

    #[test]
    fn report_serializes_without_sensitive_data() {
        let report = ReconcileReport {
            runtime: "docker".into(),
            policy_deletion_enabled: true,
            policy_grace_secs: 3600,
            active_job_count: 1,
            active_jobs_hash: "abc123".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&report).unwrap();
        assert!(!json.contains("token"));
        assert!(!json.contains("env"));
        assert!(json.contains("active_jobs_hash"));
        assert!(json.contains("policy_grace_secs"));
    }

    #[tokio::test]
    async fn docker_runtime_census_canary_is_read_only() {
        if std::env::var("GITFORGE_RECONCILER_DOCKER_CANARY").as_deref() != Ok("1") {
            return;
        }
        let source = DockerContainerSource::connect_with_timeout(Duration::from_secs(5)).unwrap();
        let records = source.list_containers().await.unwrap();
        assert!(records.iter().all(|record| {
            record.labels.get(MANAGED_LABEL).map(String::as_str) == Some("true")
        }));
        assert!(records
            .iter()
            .all(|record| !record.running || record.exited_at_ms.is_none()));
    }
}
