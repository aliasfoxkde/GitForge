//! Ownership and reclamation rules for the runner's warm container pool.
//!
//! The pool is a pre-warm cache of containers: the runner keeps a small
//! number of spare containers per image so a job does not pay container
//! creation latency. Those containers are created with a placeholder job id
//! that has no row in the store, and they are never handed a real job id
//! afterwards — Docker labels are immutable, so a container that leaves the
//! pool keeps the placeholder label for the rest of its life.
//!
//! That leaves the pool invisible to every other cleanup path in the tree:
//! `remove_job_containers` scopes by exact label equality with a real job
//! id, and the general reconciler retains every running container. When a
//! runner process dies or restarts, its in-memory pool map is lost while the
//! containers it created keep running — they leak, and nothing in the tree
//! can reach them (GF-17).
//!
//! This module is the pool's own reclamation contract: a pooled container
//! is owned by the pool, it is identified by the labels set at creation
//! time, and it is reclaimable only when every ownership fact is
//! established. The decision is a pure function so it can be tested
//! without a Docker daemon; the removal loop is a thin shell around it.

use crate::reconciler::{ContainerRecord, ContainerSource, RemovalOutcome};
use gitforge_common::{Error, Result};
use std::collections::HashMap;
use std::time::Duration;

/// Label marking a container as belonging to a warm pool.
pub const POOL_LABEL: &str = "com.gitforce.pool";

/// Label naming the pool owner that created the container.
pub const POOL_OWNER_LABEL: &str = "com.gitforce.pool.owner";

/// Label naming the pool generation. One generation per runner process,
/// so a container from a previous process is distinguishable from one the
/// current process owns.
pub const POOL_GENERATION_LABEL: &str = "com.gitforce.pool.generation";

/// Label naming the PID of the runner process that created the container.
pub const POOL_PID_LABEL: &str = "com.gitforce.pool.pid";

/// The identity of the pool that owns a set of containers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolIdentity {
    /// Stable owner of the pool (runner identity). Two pools with
    /// different owners never reclaim each other's containers.
    pub owner: String,
    /// One generation per runner process. Containers from another
    /// generation belong to a process that is gone.
    pub generation: String,
    /// PID of the owning process, used to tell "the process that made this
    /// is gone" from "another live process made this".
    pub pid: u32,
}

impl PoolIdentity {
    /// Build an identity for the current process.
    pub fn new(owner: impl Into<String>, generation: impl Into<String>) -> Self {
        Self {
            owner: owner.into(),
            generation: generation.into(),
            pid: std::process::id(),
        }
    }
}

/// Policy for reclaiming pooled containers.
#[derive(Debug, Clone)]
pub struct PoolPolicy {
    /// How long a pooled container may sit before it is eligible for
    /// reclamation. Default: 30 minutes.
    pub ttl: Duration,
    /// Upper bound on containers removed per pass. Bounded blast radius.
    /// Default: 16.
    pub max_removals: usize,
}

impl Default for PoolPolicy {
    fn default() -> Self {
        Self {
            ttl: Duration::from_secs(1800),
            max_removals: 16,
        }
    }
}

impl PoolPolicy {
    /// Fail-closed validation: the TTL and the blast radius must be
    /// positive.
    pub fn validate(&self) -> Result<()> {
        if self.ttl.is_zero() {
            return Err(Error::sandbox("pool policy rejected: ttl must be positive"));
        }
        if self.max_removals == 0 {
            return Err(Error::sandbox(
                "pool policy rejected: max removals must be positive",
            ));
        }
        Ok(())
    }
}

/// The decision for one container against the pool's reclamation rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoolDecision {
    /// The container is not a pooled container at all.
    NotPool,
    /// The container belongs to a different pool owner.
    RetainOtherOwner,
    /// The container belongs to this process's own pool.
    RetainCurrentGeneration,
    /// The creating process is still alive, so the container may be
    /// serving a job right now.
    RetainOwnerAlive,
    /// The creating process's PID cannot be checked, so ownership cannot
    /// be established.
    RetainUnknownOwner,
    /// The container's age is unknown, so its TTL cannot be applied.
    RetainUnknownAge,
    /// The container is still inside its TTL.
    RetainWithinTtl,
    /// The container is an orphan of a dead pool generation and may be
    /// removed.
    Reclaim,
}

/// What is known about the process that created a container.
pub type OwnerState = dyn Fn(u32) -> Option<bool> + Send + Sync;

/// Look up whether a PID is alive. Returns `None` when the answer cannot
/// be established (for example when the runner cannot see the PID
/// namespace its container was created in), which keeps the caller
/// fail-safe: an unknown owner is never reclaimed.
pub fn process_alive(pid: u32) -> Option<bool> {
    // Our own PID must be visible for the namespace to be shared with the
    // processes we are asked about.
    if !std::path::Path::new("/proc/self").exists() {
        return None;
    }
    let path = format!("/proc/{pid}");
    if std::path::Path::new(&path).exists() {
        Some(true)
    } else {
        Some(false)
    }
}

/// Pure decision function: classify one container against the pool's
/// reclamation rules. No I/O; the same inputs always produce the same
/// decision.
pub fn classify_pool_container(
    record: &ContainerRecord,
    identity: &PoolIdentity,
    policy: &PoolPolicy,
    now_ms: i64,
    owner_state: &OwnerState,
) -> PoolDecision {
    // 1. Is it a pooled container at all?
    if record.labels.get(POOL_LABEL).map(String::as_str) != Some("true") {
        return PoolDecision::NotPool;
    }
    // 2. Ownership: only our own pool's containers are ours to reclaim.
    match record.labels.get(POOL_OWNER_LABEL) {
        Some(owner) if owner == &identity.owner => {}
        _ => return PoolDecision::RetainOtherOwner,
    }
    // 3. Generation: our own generation is in use by definition.
    match record.labels.get(POOL_GENERATION_LABEL) {
        Some(generation) if generation == &identity.generation => {
            return PoolDecision::RetainCurrentGeneration;
        }
        Some(_) => {}
        // A container with no generation cannot be attributed to a dead
        // process, so it is retained.
        None => return PoolDecision::RetainUnknownOwner,
    }
    // 4. The creating process: a live PID may be serving a job right now.
    let Some(raw_pid) = record.labels.get(POOL_PID_LABEL) else {
        return PoolDecision::RetainUnknownOwner;
    };
    let Ok(pid) = raw_pid.parse::<u32>() else {
        return PoolDecision::RetainUnknownOwner;
    };
    match owner_state(pid) {
        Some(true) => return PoolDecision::RetainOwnerAlive,
        None => return PoolDecision::RetainUnknownOwner,
        Some(false) => {}
    }
    // 5. Age: an unknown age cannot be TTL-checked.
    let Some(created_at_ms) = record.created_at_ms else {
        return PoolDecision::RetainUnknownAge;
    };
    let age_ms = now_ms.saturating_sub(created_at_ms);
    if age_ms < policy.ttl.as_millis() as i64 {
        return PoolDecision::RetainWithinTtl;
    }
    PoolDecision::Reclaim
}

/// Outcome of one reclamation pass.
#[derive(Debug, Clone, Default)]
pub struct PoolReclaimReport {
    /// Containers removed, by container id.
    pub reclaimed: Vec<String>,
    /// Containers that were considered and kept, by decision.
    pub retained: Vec<(String, PoolDecision)>,
    /// Containers whose removal failed.
    pub failed: Vec<(String, String)>,
    /// True when the pass stopped early because the blast radius bound
    /// was reached.
    pub bounded: bool,
}

/// Reclaim the pooled containers orphaned by a previous runner process.
///
/// The pass is idempotent: a container that is already gone is reported as
/// reclaimed without an error, and a container whose removal fails is
/// recorded rather than retried forever in the same pass.
pub async fn reclaim_orphaned_pool_containers<S: ContainerSource>(
    source: &S,
    identity: &PoolIdentity,
    policy: &PoolPolicy,
    now_ms: i64,
    owner_state: &OwnerState,
) -> Result<PoolReclaimReport> {
    policy.validate()?;
    let mut report = PoolReclaimReport::default();
    let containers = source.list_containers().await?;

    for record in containers {
        let decision = classify_pool_container(&record, identity, policy, now_ms, owner_state);
        if decision != PoolDecision::Reclaim {
            if decision != PoolDecision::NotPool {
                report.retained.push((record.id.clone(), decision));
            }
            continue;
        }
        if report.reclaimed.len() >= policy.max_removals {
            report.bounded = true;
            report
                .retained
                .push((record.id.clone(), PoolDecision::Reclaim));
            continue;
        }
        match source.remove_container(&record.id).await {
            Ok(RemovalOutcome::Removed) | Ok(RemovalOutcome::AlreadyGone) => {
                report.reclaimed.push(record.id.clone());
            }
            Ok(RemovalOutcome::Failed) => {
                report
                    .failed
                    .push((record.id.clone(), "removal refused".to_string()));
            }
            Err(error) => {
                report.failed.push((record.id.clone(), error.to_string()));
            }
        }
    }
    Ok(report)
}

/// The pool labels for a container, as a map. Used by the sandbox to
/// stamp pooled containers and by tests to build records.
pub fn pool_labels(identity: &PoolIdentity) -> HashMap<String, String> {
    HashMap::from([
        (POOL_LABEL.to_string(), "true".to_string()),
        (POOL_OWNER_LABEL.to_string(), identity.owner.clone()),
        (
            POOL_GENERATION_LABEL.to_string(),
            identity.generation.clone(),
        ),
        (POOL_PID_LABEL.to_string(), identity.pid.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> PoolIdentity {
        PoolIdentity {
            owner: "runner@host".to_string(),
            generation: "gen-current".to_string(),
            pid: 4242,
        }
    }

    fn record(
        id: &str,
        labels: HashMap<String, String>,
        created_at_ms: Option<i64>,
    ) -> ContainerRecord {
        ContainerRecord {
            id: id.to_string(),
            labels,
            running: true,
            exited_at_ms: None,
            created_at_ms,
        }
    }

    fn pooled(labels: Vec<(&str, &str)>) -> HashMap<String, String> {
        labels
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// A container source that records removals instead of talking to a
    /// Docker daemon.
    struct FakeSource {
        containers: Vec<ContainerRecord>,
        removed: std::sync::Mutex<Vec<String>>,
    }

    impl FakeSource {
        fn new(containers: Vec<ContainerRecord>) -> Self {
            Self {
                containers,
                removed: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl ContainerSource for FakeSource {
        async fn list_containers(&self) -> Result<Vec<ContainerRecord>> {
            Ok(self.containers.clone())
        }

        async fn remove_container(&self, id: &str) -> Result<RemovalOutcome> {
            self.removed
                .lock()
                .expect("fake source lock")
                .push(id.to_string());
            Ok(RemovalOutcome::Removed)
        }
    }

    /// A pooled container from a dead generation, older than the TTL.
    fn orphan(id: &str, age_ms: i64) -> ContainerRecord {
        record(
            id,
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "runner@host"),
                (POOL_GENERATION_LABEL, "gen-old"),
                (POOL_PID_LABEL, "1"),
            ]),
            Some(NOW - age_ms),
        )
    }

    fn dead_owner(_pid: u32) -> Option<bool> {
        Some(false)
    }

    fn live_owner(_pid: u32) -> Option<bool> {
        Some(true)
    }

    fn unknown_owner(_pid: u32) -> Option<bool> {
        None
    }

    const NOW: i64 = 10_000_000;

    #[test]
    fn a_non_pooled_container_is_not_ours_to_judge() {
        let record = record(
            "c1",
            pooled(vec![("com.gitforce.managed", "true")]),
            Some(NOW - 10_000),
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &dead_owner,
        );
        assert_eq!(decision, PoolDecision::NotPool);
    }

    #[test]
    fn another_owners_pool_is_never_reclaimed() {
        let record = record(
            "c1",
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "other@host"),
                (POOL_GENERATION_LABEL, "gen-old"),
                (POOL_PID_LABEL, "1"),
            ]),
            Some(NOW - 10_000_000),
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &dead_owner,
        );
        assert_eq!(decision, PoolDecision::RetainOtherOwner);
    }

    #[test]
    fn our_own_generation_is_retained_even_when_old() {
        let record = record(
            "c1",
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "runner@host"),
                (POOL_GENERATION_LABEL, "gen-current"),
                (POOL_PID_LABEL, "4242"),
            ]),
            Some(NOW - 100_000_000),
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &live_owner,
        );
        assert_eq!(decision, PoolDecision::RetainCurrentGeneration);
    }

    #[test]
    fn a_live_creating_process_keeps_its_container() {
        let record = record(
            "c1",
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "runner@host"),
                (POOL_GENERATION_LABEL, "gen-old"),
                (POOL_PID_LABEL, "99"),
            ]),
            Some(NOW - 100_000_000),
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &live_owner,
        );
        assert_eq!(decision, PoolDecision::RetainOwnerAlive);
    }

    #[test]
    fn an_unknown_owner_is_retained() {
        let record = record(
            "c1",
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "runner@host"),
                (POOL_GENERATION_LABEL, "gen-old"),
                (POOL_PID_LABEL, "99"),
            ]),
            Some(NOW - 100_000_000),
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &unknown_owner,
        );
        assert_eq!(decision, PoolDecision::RetainUnknownOwner);
    }

    #[test]
    fn a_container_without_a_pid_is_retained() {
        let record = record(
            "c1",
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "runner@host"),
                (POOL_GENERATION_LABEL, "gen-old"),
            ]),
            Some(NOW - 100_000_000),
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &dead_owner,
        );
        assert_eq!(decision, PoolDecision::RetainUnknownOwner);
    }

    #[test]
    fn a_container_inside_its_ttl_is_retained() {
        let record = record(
            "c1",
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "runner@host"),
                (POOL_GENERATION_LABEL, "gen-old"),
                (POOL_PID_LABEL, "99"),
            ]),
            Some(NOW - 60_000),
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &dead_owner,
        );
        assert_eq!(decision, PoolDecision::RetainWithinTtl);
    }

    #[test]
    fn an_orphan_past_its_ttl_is_reclaimed() {
        let record = record(
            "c1",
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "runner@host"),
                (POOL_GENERATION_LABEL, "gen-old"),
                (POOL_PID_LABEL, "99"),
            ]),
            Some(NOW - 100_000_000),
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &dead_owner,
        );
        assert_eq!(decision, PoolDecision::Reclaim);
    }

    #[test]
    fn an_unknown_age_is_retained() {
        let record = record(
            "c1",
            pooled(vec![
                (POOL_LABEL, "true"),
                (POOL_OWNER_LABEL, "runner@host"),
                (POOL_GENERATION_LABEL, "gen-old"),
                (POOL_PID_LABEL, "99"),
            ]),
            None,
        );
        let decision = classify_pool_container(
            &record,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &dead_owner,
        );
        assert_eq!(decision, PoolDecision::RetainUnknownAge);
    }

    #[test]
    fn a_zero_ttl_policy_is_refused() {
        let policy = PoolPolicy {
            ttl: Duration::ZERO,
            max_removals: 4,
        };
        assert!(policy.validate().is_err());
    }

    #[tokio::test]
    async fn a_pass_reclaims_only_orphans_of_a_dead_generation() {
        let source = FakeSource::new(vec![
            orphan("orphan-old", 10_000_000),
            record(
                "live-owner",
                pooled(vec![
                    (POOL_LABEL, "true"),
                    (POOL_OWNER_LABEL, "runner@host"),
                    (POOL_GENERATION_LABEL, "gen-old"),
                    (POOL_PID_LABEL, "2"),
                ]),
                Some(NOW - 10_000_000),
            ),
            record(
                "other-owner",
                pooled(vec![
                    (POOL_LABEL, "true"),
                    (POOL_OWNER_LABEL, "other@host"),
                    (POOL_GENERATION_LABEL, "gen-old"),
                    (POOL_PID_LABEL, "1"),
                ]),
                Some(NOW - 10_000_000),
            ),
        ]);
        let owner_state: &OwnerState = &|pid: u32| {
            if pid == 1 {
                Some(false)
            } else {
                Some(true)
            }
        };
        let report = reclaim_orphaned_pool_containers(
            &source,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &owner_state,
        )
        .await
        .expect("reclaim pass");
        assert_eq!(report.reclaimed, vec!["orphan-old".to_string()]);
        assert_eq!(
            source.removed.lock().expect("fake source lock").as_slice(),
            &["orphan-old".to_string()]
        );
    }

    #[tokio::test]
    async fn a_pass_is_bounded_by_the_blast_radius() {
        let containers: Vec<ContainerRecord> = (0..5)
            .map(|index| orphan(&format!("orphan-{index}"), 10_000_000))
            .collect();
        let source = FakeSource::new(containers);
        let policy = PoolPolicy {
            ttl: Duration::from_secs(1),
            max_removals: 2,
        };
        let owner_state: &OwnerState = &dead_owner;
        let report =
            reclaim_orphaned_pool_containers(&source, &identity(), &policy, NOW, &owner_state)
                .await
                .expect("reclaim pass");
        assert_eq!(report.reclaimed.len(), 2);
        assert!(report.bounded);
        assert_eq!(
            source.removed.lock().expect("fake source lock").len(),
            2,
            "the pass must not remove more than the bound allows"
        );
    }

    #[tokio::test]
    async fn an_unknown_owner_state_reclaims_nothing() {
        let source = FakeSource::new(vec![orphan("orphan", 10_000_000)]);
        let owner_state: &OwnerState = &unknown_owner;
        let report = reclaim_orphaned_pool_containers(
            &source,
            &identity(),
            &PoolPolicy::default(),
            NOW,
            &owner_state,
        )
        .await
        .expect("reclaim pass");
        assert!(report.reclaimed.is_empty());
        assert!(source.removed.lock().expect("fake source lock").is_empty());
    }
}
