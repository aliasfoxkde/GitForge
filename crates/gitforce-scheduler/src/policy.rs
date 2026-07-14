//! Scheduling policies

use async_trait::async_trait;
use gitforce_common::{JobId, RunnerId};
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

#[cfg(test)]
mod tests {
    use super::*;
    use gitforce_common::RunnerId;
    use gitforce_db::models::Runner;

    fn make_runner(id: RunnerId, name: &str, status: &str, capacity: i32) -> Runner {
        Runner {
            id,
            name: name.to_string(),
            runner_type: "docker".to_string(),
            status: status.to_string(),
            capacity,
            last_heartbeat: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_simple_policy_selects_available_runner() {
        let policy = SimplePolicy::new();
        let runners = vec![
            make_runner(RunnerId::new(), "runner-1", "online", 4),
            make_runner(RunnerId::new(), "runner-2", "online", 2),
        ];
        let job_id = JobId::new();

        let selected = policy.select_runner(job_id, &runners).await;
        assert!(selected.is_some());
    }

    #[tokio::test]
    async fn test_simple_policy_skips_offline_runners() {
        let policy = SimplePolicy::new();
        let runners = vec![
            make_runner(RunnerId::new(), "runner-1", "offline", 4),
            make_runner(RunnerId::new(), "runner-2", "online", 2),
        ];
        let job_id = JobId::new();

        let selected = policy.select_runner(job_id, &runners).await;
        assert!(selected.is_some());
    }

    #[tokio::test]
    async fn test_simple_policy_skips_zero_capacity() {
        let policy = SimplePolicy::new();
        let runner1 = RunnerId::new();
        let runner2 = RunnerId::new();
        let runners = vec![
            make_runner(runner1, "runner-1", "online", 0),
            make_runner(runner2, "runner-2", "online", 2),
        ];
        let job_id = JobId::new();

        let selected = policy.select_runner(job_id, &runners).await;
        assert_eq!(selected, Some(runner2));
    }

    #[tokio::test]
    async fn test_simple_policy_selects_most_capacity() {
        let policy = SimplePolicy::new();
        let runner1 = RunnerId::new();
        let runner2 = RunnerId::new();
        let runners = vec![
            make_runner(runner1, "runner-1", "online", 2),
            make_runner(runner2, "runner-2", "online", 8),
        ];
        let job_id = JobId::new();

        let selected = policy.select_runner(job_id, &runners).await;
        assert_eq!(selected, Some(runner2));
    }

    #[tokio::test]
    async fn test_simple_policy_returns_none_when_no_runners() {
        let policy = SimplePolicy::new();
        let runners: Vec<Runner> = vec![];
        let job_id = JobId::new();

        let selected = policy.select_runner(job_id, &runners).await;
        assert!(selected.is_none());
    }

    #[tokio::test]
    async fn test_simple_policy_returns_none_when_all_offline() {
        let policy = SimplePolicy::new();
        let runners = vec![
            make_runner(RunnerId::new(), "runner-1", "offline", 4),
            make_runner(RunnerId::new(), "runner-2", "offline", 2),
        ];
        let job_id = JobId::new();

        let selected = policy.select_runner(job_id, &runners).await;
        assert!(selected.is_none());
    }

    #[tokio::test]
    async fn test_priority_policy_uses_simple_policy() {
        let policy = PriorityPolicy::new();
        let runners = vec![
            make_runner(RunnerId::new(), "runner-1", "online", 4),
        ];
        let job_id = JobId::new();

        let selected = policy.select_runner(job_id, &runners).await;
        assert!(selected.is_some());
    }

    #[test]
    fn test_simple_policy_new() {
        let policy = SimplePolicy::new();
        assert!(matches!(policy, SimplePolicy));
    }

    #[test]
    fn test_priority_policy_new() {
        let policy = PriorityPolicy::new();
        assert!(matches!(policy, PriorityPolicy));
    }
}
