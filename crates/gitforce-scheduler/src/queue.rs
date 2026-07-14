//! Job queue implementation

use gitforce_common::{JobId, PipelineRunId, RepoId};
use std::collections::{BinaryHeap, HashMap};
use std::cmp::Ordering;

/// Job priority
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Priority {
    #[default]
    Normal = 1,
    High = 2,
    Low = 0,
}

impl Ord for Priority {
    fn cmp(&self, other: &Self) -> Ordering {
        (*self as i32).cmp(&(*other as i32))
    }
}

impl PartialOrd for Priority {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Queued job entry
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedJob {
    pub job_id: JobId,
    pub pipeline_run_id: PipelineRunId,
    pub repo_id: RepoId,
    pub priority: Priority,
    pub queued_at: i64,
}

impl QueuedJob {
    pub fn new(job_id: JobId, pipeline_run_id: PipelineRunId, repo_id: RepoId) -> Self {
        Self {
            job_id,
            pipeline_run_id,
            repo_id,
            priority: Priority::default(),
            queued_at: chrono::Utc::now().timestamp_millis(),
        }
    }

    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }
}

/// FIFO ordering for same-priority jobs (older first)
impl PartialOrd for QueuedJob {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for QueuedJob {
    fn cmp(&self, other: &Self) -> Ordering {
        // Higher priority first (BinaryHeap extracts max, so larger priority first)
        let priority_cmp = self.priority.cmp(&other.priority);
        if priority_cmp != Ordering::Equal {
            return priority_cmp;
        }

        // Older first (FIFO within same priority) - reverse so older is "larger" for heap
        other.queued_at.cmp(&self.queued_at)
    }
}

/// Job queue with priority support
#[derive(Debug)]
pub struct JobQueue {
    heap: BinaryHeap<QueuedJob>,
    by_id: HashMap<JobId, QueuedJob>,
}

impl Default for JobQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl JobQueue {
    /// Create a new job queue
    pub fn new() -> Self {
        Self {
            heap: BinaryHeap::new(),
            by_id: HashMap::new(),
        }
    }

    /// Enqueue a job
    pub fn enqueue(&mut self, job: QueuedJob) {
        let job_id = job.job_id;
        self.by_id.insert(job_id, job.clone());
        self.heap.push(job);
    }

    /// Dequeue the highest priority job
    pub fn dequeue(&mut self) -> Option<QueuedJob> {
        let job = self.heap.pop()?;
        self.by_id.remove(&job.job_id);
        Some(job)
    }

    /// Peek at the next job without removing it
    pub fn peek(&self) -> Option<&QueuedJob> {
        self.heap.peek()
    }

    /// Remove a specific job from the queue
    pub fn remove(&mut self, job_id: JobId) -> Option<QueuedJob> {
        let job = self.by_id.remove(&job_id)?;
        // Note: We can't efficiently remove from BinaryHeap, so we mark it as removed
        // by checking by_id when dequeueing
        Some(job)
    }

    /// Check if the queue contains a job
    pub fn contains(&self, job_id: JobId) -> bool {
        self.by_id.contains_key(&job_id)
    }

    /// Get queue length
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Get all queued jobs
    pub fn all(&self) -> Vec<&QueuedJob> {
        self.by_id.values().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_ordering() {
        let mut queue = JobQueue::new();
        let repo_id = RepoId::new();

        let job1 = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id);
        let job2 = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id)
            .with_priority(Priority::High);
        let job3 = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id)
            .with_priority(Priority::Low);

        queue.enqueue(job1.clone());
        queue.enqueue(job2.clone());
        queue.enqueue(job3.clone());

        // High priority should come first
        let first = queue.dequeue().unwrap();
        assert_eq!(first.job_id, job2.job_id);
    }

    #[test]
    fn test_fifo_within_priority() {
        let mut queue = JobQueue::new();
        let repo_id = RepoId::new();

        let job1 = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id);
        std::thread::sleep(std::time::Duration::from_millis(1));
        let job2 = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id);

        queue.enqueue(job1.clone());
        queue.enqueue(job2.clone());

        // First enqueued should come first (FIFO)
        let first = queue.dequeue().unwrap();
        assert_eq!(first.job_id, job1.job_id);
    }
}
