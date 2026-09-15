//! Authoritative registry of jobs currently being executed by this process.
//!
//! This is the **single shared state seam** between the runner's job
//! admission path and the abandoned-container reconciler. Both paths acquire
//! the same internal mutex so that the reconciler can never delete a
//! container for a job that has been concurrently claimed (or, conversely,
//! can never observe a job as inactive while admission is in the middle of
//! recording it as active).
//!
//! # Concurrency contract
//!
//! - [`ActiveJobRegistry::try_claim`] and [`ActiveJobRegistry::release`]
//!   acquire the mutex for their entire critical section. They never block
//!   on network I/O.
//! - [`ActiveJobRegistry::lock`] returns a guard that the reconciler holds
//!   for the duration of a reconcile pass so that admission is serialized
//!   with the reconciler's Docker calls. This eliminates the
//!   "snapshot-then-claim-then-delete" race.
//! - [`ActiveJobSnapshot`] is the read-side view returned from
//!   [`ActiveJobRegistry::snapshot`]. It borrows the registry's mutex; the
//!   caller is responsible for keeping it alive only as long as the snapshot
//!   remains valid.

use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::{Mutex, MutexGuard};

/// Authoritative set of currently-active job IDs.
///
/// Cheap to clone — the inner mutex is reference-counted and shared.
#[derive(Clone, Default)]
pub struct ActiveJobRegistry {
    inner: Arc<Mutex<HashSet<String>>>,
}

impl ActiveJobRegistry {
    /// Build an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire the registry's lock. Used by the reconciler to hold the lock
    /// across its Docker operations so admission cannot interleave.
    pub async fn lock(&self) -> ActiveJobGuard<'_> {
        let guard = self.inner.lock().await;
        ActiveJobGuard { guard }
    }

    /// Take a read-only snapshot under the lock. The returned guard borrows
    /// the registry and must be dropped (or awaited on) before admission can
    /// proceed.
    pub async fn snapshot(&self) -> ActiveJobSnapshot<'_> {
        let guard = self.inner.lock().await;
        ActiveJobSnapshot { guard }
    }

    /// Atomically claim a job ID. Returns `true` if newly claimed, `false` if
    /// it was already active.
    pub async fn try_claim(&self, job_id: &str) -> bool {
        let mut guard = self.inner.lock().await;
        guard.insert(job_id.to_string())
    }

    /// Release a job ID. Returns `true` if it was active.
    pub async fn release(&self, job_id: &str) -> bool {
        let mut guard = self.inner.lock().await;
        guard.remove(job_id)
    }

    /// Convenience predicate used by the runner's admission loop. Locks the
    /// registry briefly to check whether the given job is currently active.
    pub async fn is_active(&self, job_id: &str) -> bool {
        self.inner.lock().await.contains(job_id)
    }
}

/// RAII guard returned by [`ActiveJobRegistry::lock`].
pub struct ActiveJobGuard<'a> {
    guard: MutexGuard<'a, HashSet<String>>,
}

impl<'a> ActiveJobGuard<'a> {
    /// True if the given job ID is currently active.
    pub fn contains(&self, job_id: &str) -> bool {
        self.guard.contains(job_id)
    }

    /// Snapshot the current active set. O(n) copy; useful only when the
    /// caller needs a stable, owned view.
    pub fn snapshot(&self) -> HashSet<String> {
        self.guard.clone()
    }

    /// Number of active jobs.
    pub fn len(&self) -> usize {
        self.guard.len()
    }

    /// True if no jobs are active.
    pub fn is_empty(&self) -> bool {
        self.guard.is_empty()
    }

    /// Iterate over the active job IDs.
    pub fn iter(&self) -> impl Iterator<Item = &String> {
        self.guard.iter()
    }
}

/// Read-only view of the active-job set, borrowed under the registry's
/// mutex.
pub struct ActiveJobSnapshot<'a> {
    guard: MutexGuard<'a, HashSet<String>>,
}

impl<'a> ActiveJobSnapshot<'a> {
    /// True if the given job ID is currently active.
    pub fn contains(&self, job_id: &str) -> bool {
        self.guard.contains(job_id)
    }

    /// Snapshot the current active set. O(n) copy.
    pub fn to_set(&self) -> HashSet<String> {
        self.guard.clone()
    }

    /// Number of active jobs.
    pub fn len(&self) -> usize {
        self.guard.len()
    }

    /// True if no jobs are active.
    pub fn is_empty(&self) -> bool {
        self.guard.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn registry_starts_empty() {
        let registry = ActiveJobRegistry::new();
        let snapshot = registry.snapshot().await;
        assert!(snapshot.is_empty());
        assert_eq!(snapshot.len(), 0);
    }

    #[tokio::test]
    async fn try_claim_is_atomic_and_idempotent() {
        let registry = ActiveJobRegistry::new();
        let id = uuid::Uuid::new_v4().to_string();

        assert!(registry.try_claim(&id).await, "first claim must succeed");
        assert!(
            !registry.try_claim(&id).await,
            "duplicate claim must return false"
        );

        let snapshot = registry.snapshot().await;
        assert!(snapshot.contains(&id));
        assert_eq!(snapshot.len(), 1);
        // Drop the snapshot (and its mutex guard) before calling `release`,
        // which needs to acquire the same mutex. Holding the guard across the
        // `release` call would deadlock with ourselves.
        drop(snapshot);

        assert!(registry.release(&id).await);
        let snapshot = registry.snapshot().await;
        assert!(!snapshot.contains(&id));
        assert!(snapshot.is_empty());
    }

    #[tokio::test]
    async fn lock_guard_sees_current_state() {
        let registry = ActiveJobRegistry::new();
        let id = uuid::Uuid::new_v4().to_string();
        registry.try_claim(&id).await;

        let guard = registry.lock().await;
        assert!(guard.contains(&id));
        assert_eq!(guard.len(), 1);
        assert!(!guard.is_empty());

        let owned: Vec<String> = guard.iter().cloned().collect();
        assert_eq!(owned, vec![id.clone()]);

        let snapshot = guard.snapshot();
        assert!(snapshot.contains(&id));
    }

    #[tokio::test]
    async fn release_returns_false_when_unknown() {
        let registry = ActiveJobRegistry::new();
        assert!(!registry.release("not-a-job").await);
    }
}
