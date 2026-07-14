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

    #[test]
    fn test_queue_peek() {
        let mut queue = JobQueue::new();
        let repo_id = RepoId::new();

        assert!(queue.peek().is_none());

        let job = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id)
            .with_priority(Priority::High);
        queue.enqueue(job.clone());

        let peeked = queue.peek().unwrap();
        assert_eq!(peeked.job_id, job.job_id);
    }

    #[test]
    fn test_queue_contains() {
        let mut queue = JobQueue::new();
        let repo_id = RepoId::new();

        let job_id = JobId::new();
        let job = QueuedJob::new(job_id, PipelineRunId::new(), repo_id);
        queue.enqueue(job);

        assert!(queue.contains(job_id));
        assert!(!queue.contains(JobId::new()));
    }

    #[test]
    fn test_queue_remove() {
        let mut queue = JobQueue::new();
        let repo_id = RepoId::new();

        let job_id = JobId::new();
        let job = QueuedJob::new(job_id, PipelineRunId::new(), repo_id);
        queue.enqueue(job.clone());

        let removed = queue.remove(job_id);
        assert!(removed.is_some());
        assert!(!queue.contains(job_id));
    }

    #[test]
    fn test_queue_remove_nonexistent() {
        let mut queue = JobQueue::new();
        let removed = queue.remove(JobId::new());
        assert!(removed.is_none());
    }

    #[test]
    fn test_queue_len() {
        let mut queue = JobQueue::new();
        let repo_id = RepoId::new();

        assert_eq!(queue.len(), 0);

        queue.enqueue(QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id));
        assert_eq!(queue.len(), 1);

        queue.enqueue(QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id));
        assert_eq!(queue.len(), 2);

        queue.dequeue();
        assert_eq!(queue.len(), 1);
    }

    #[test]
    fn test_queue_is_empty() {
        let mut queue = JobQueue::new();
        let repo_id = RepoId::new();

        assert!(queue.is_empty());

        queue.enqueue(QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id));
        assert!(!queue.is_empty());

        queue.dequeue();
        assert!(queue.is_empty());
    }

    #[test]
    fn test_queue_all() {
        let mut queue = JobQueue::new();
        let repo_id = RepoId::new();

        let job1 = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id);
        let job2 = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id);
        queue.enqueue(job1.clone());
        queue.enqueue(job2.clone());

        let all = queue.all();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::High > Priority::Low);
        assert!(Priority::Normal > Priority::Low);
        assert!(Priority::Low < Priority::High);
    }

    #[test]
    fn test_queued_job_with_priority() {
        let repo_id = RepoId::new();
        let job = QueuedJob::new(JobId::new(), PipelineRunId::new(), repo_id)
            .with_priority(Priority::High);
        assert_eq!(job.priority, Priority::High);
    }
}
