//! Scheduling policies

use async_trait::async_trait;
use gitforce_common::{JobId, Result, RunnerId};
use gitforce_db::models::Runner;

/// Scheduling policy trait
#[async_trait]
pub trait SchedulingPolicy: Send + Sync {
    /// Select the best runner for a job
    async fn select_runner(
        &self,
        job_id: JobId,
        runners: &[Runner],
    ) -> Option<RunnerId>;
}

/// Simple round-robin / least-loaded policy
pub struct SimplePolicy;

impl SimplePolicy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SimplePolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SchedulingPolicy for SimplePolicy {
    async fn select_runner(
        &self,
        _job_id: JobId,
        runners: &[Runner],
    ) -> Option<RunnerId> {
        // Filter to only online runners with capacity
        let available: Vec<_> = runners
            .iter()
            .filter(|r| r.status == "online" && r.capacity > 0)
            .collect();

        if available.is_empty() {
            return None;
        }

        // Select the runner with most available capacity
        // In a real implementation, we'd track actual load
        let best = available
            .iter()
            .max_by_key(|r| r.capacity)?;

        Some(best.id)
    }
}

/// Priority-based policy
pub struct PriorityPolicy;

impl PriorityPolicy {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PriorityPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SchedulingPolicy for PriorityPolicy {
    async fn select_runner(
        &self,
        job_id: JobId,
        runners: &[Runner],
    ) -> Option<RunnerId> {
        // Use simple policy for now
        SimplePolicy::new().select_runner(job_id, runners).await
    }
}
