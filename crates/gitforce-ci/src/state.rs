//! Job state machine

use gitforce_common::{JobId, JobStatus, Result, RunnerId};

/// Job state machine for managing job lifecycle
#[derive(Debug, Clone)]
pub struct JobStateMachine {
    job_id: JobId,
    status: JobStatus,
    runner_id: Option<RunnerId>,
    exit_code: Option<i32>,
    error_message: Option<String>,
}

impl JobStateMachine {
    /// Create a new state machine for a job
    pub fn new(job_id: JobId) -> Self {
        Self {
            job_id,
            status: JobStatus::Pending,
            runner_id: None,
            exit_code: None,
            error_message: None,
        }
    }

    /// Get current status
    pub fn status(&self) -> JobStatus {
        self.status
    }

    /// Get runner ID if assigned
    pub fn runner_id(&self) -> Option<RunnerId> {
        self.runner_id
    }

    /// Get exit code if finished
    pub fn exit_code(&self) -> Option<i32> {
        self.exit_code
    }

    /// Get error message if failed
    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    /// Transition to queued state
    pub fn queue(&mut self) -> Result<()> {
        self.ensure_valid_transition(JobStatus::Queued)?;
        self.status = JobStatus::Queued;
        tracing::debug!("job {} queued", self.job_id);
        Ok(())
    }

    /// Transition to assigned state
    pub fn assign(&mut self, runner_id: RunnerId) -> Result<()> {
        self.ensure_valid_transition(JobStatus::Assigned)?;
        self.status = JobStatus::Assigned;
        self.runner_id = Some(runner_id);
        tracing::debug!("job {} assigned to runner {}", self.job_id, runner_id);
        Ok(())
    }

    /// Transition to running state
    pub fn start(&mut self) -> Result<()> {
        self.ensure_valid_transition(JobStatus::Running)?;
        self.status = JobStatus::Running;
        tracing::debug!("job {} started", self.job_id);
        Ok(())
    }

    /// Transition to succeeded state
    pub fn succeed(&mut self, exit_code: i32) -> Result<()> {
        self.ensure_valid_transition(JobStatus::Succeeded)?;
        self.status = JobStatus::Succeeded;
        self.exit_code = Some(exit_code);
        tracing::info!("job {} succeeded with exit code {}", self.job_id, exit_code);
        Ok(())
    }

    /// Transition to failed state
    pub fn fail(&mut self, exit_code: i32, error: String) -> Result<()> {
        self.ensure_valid_transition(JobStatus::Failed)?;
        self.status = JobStatus::Failed;
        self.exit_code = Some(exit_code);
        self.error_message = Some(error);
        tracing::error!(
            "job {} failed with exit code {}: {}",
            self.job_id,
            exit_code,
            self.error_message.as_ref().unwrap()
        );
        Ok(())
    }

    /// Transition to cancelled state
    pub fn cancel(&mut self) -> Result<()> {
        self.ensure_valid_transition(JobStatus::Cancelled)?;
        self.status = JobStatus::Cancelled;
        tracing::warn!("job {} cancelled", self.job_id);
        Ok(())
    }

    /// Transition to timed out state
    pub fn timeout(&mut self) -> Result<()> {
        self.ensure_valid_transition(JobStatus::TimedOut)?;
        self.status = JobStatus::TimedOut;
        tracing::error!("job {} timed out", self.job_id);
        Ok(())
    }

    /// Check if the job is in a terminal state
    pub fn is_terminal(&self) -> bool {
        self.status.is_terminal()
    }

    /// Get a summary of the job state
    pub fn summary(&self) -> JobStateSummary {
        JobStateSummary {
            job_id: self.job_id,
            status: self.status,
            runner_id: self.runner_id,
            exit_code: self.exit_code,
            error_message: self.error_message.clone(),
        }
    }

    fn ensure_valid_transition(&self, target: JobStatus) -> Result<()> {
        let valid = match (&self.status, target) {
            // From pending
            (JobStatus::Pending, JobStatus::Queued) => true,
            (JobStatus::Pending, JobStatus::Cancelled) => true,

            // From queued
            (JobStatus::Queued, JobStatus::Assigned) => true,
            (JobStatus::Queued, JobStatus::Cancelled) => true,
            (JobStatus::Queued, JobStatus::Failed) => true,

            // From assigned
            (JobStatus::Assigned, JobStatus::Running) => true,
            (JobStatus::Assigned, JobStatus::Cancelled) => true,

            // From running
            (JobStatus::Running, JobStatus::Succeeded) => true,
            (JobStatus::Running, JobStatus::Failed) => true,
            (JobStatus::Running, JobStatus::TimedOut) => true,
            (JobStatus::Running, JobStatus::Cancelled) => true,

            // Terminal states
            _ => false,
        };

        if !valid {
            return Err(gitforce_common::Error::invalid_input(format!(
                "invalid state transition from {:?} to {:?}",
                self.status, target
            )));
        }

        Ok(())
    }
}

/// Summary of job state
#[derive(Debug, Clone)]
pub struct JobStateSummary {
    pub job_id: JobId,
    pub status: JobStatus,
    pub runner_id: Option<RunnerId>,
    pub exit_code: Option<i32>,
    pub error_message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_job_state_transitions() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        assert_eq!(state.status(), JobStatus::Pending);

        state.queue().unwrap();
        assert_eq!(state.status(), JobStatus::Queued);

        state.assign(RunnerId::new()).unwrap();
        assert_eq!(state.status(), JobStatus::Assigned);

        state.start().unwrap();
        assert_eq!(state.status(), JobStatus::Running);

        state.succeed(0).unwrap();
        assert_eq!(state.status(), JobStatus::Succeeded);
        assert_eq!(state.exit_code(), Some(0));
    }

    #[test]
    fn test_invalid_transition() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        // Can't go from pending to running directly
        assert!(state.start().is_err());
    }

    #[test]
    fn test_fail_transition() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        state.queue().unwrap();
        state.assign(RunnerId::new()).unwrap();
        state.start().unwrap();
        state.fail(1, "build error".to_string()).unwrap();

        assert_eq!(state.status(), JobStatus::Failed);
        assert_eq!(state.exit_code(), Some(1));
        assert_eq!(state.error_message(), Some("build error"));
    }

    #[test]
    fn test_cancel_from_pending() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        state.cancel().unwrap();
        assert_eq!(state.status(), JobStatus::Cancelled);
    }

    #[test]
    fn test_cancel_from_queued() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        state.queue().unwrap();
        state.cancel().unwrap();
        assert_eq!(state.status(), JobStatus::Cancelled);
    }

    #[test]
    fn test_cancel_from_assigned() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        state.queue().unwrap();
        state.assign(RunnerId::new()).unwrap();
        state.cancel().unwrap();
        assert_eq!(state.status(), JobStatus::Cancelled);
    }

    #[test]
    fn test_cancel_from_running() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        state.queue().unwrap();
        state.assign(RunnerId::new()).unwrap();
        state.start().unwrap();
        state.cancel().unwrap();
        assert_eq!(state.status(), JobStatus::Cancelled);
    }

    #[test]
    fn test_timeout_transition() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        state.queue().unwrap();
        state.assign(RunnerId::new()).unwrap();
        state.start().unwrap();
        state.timeout().unwrap();

        assert_eq!(state.status(), JobStatus::TimedOut);
    }

    #[test]
    fn test_runner_id_accessor() {
        let job_id = JobId::new();
        let runner_id = RunnerId::new();
        let mut state = JobStateMachine::new(job_id);

        assert!(state.runner_id().is_none());

        state.queue().unwrap();
        state.assign(runner_id).unwrap();

        assert_eq!(state.runner_id(), Some(runner_id));
    }

    #[test]
    fn test_is_terminal() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        assert!(!state.is_terminal());

        state.queue().unwrap();
        assert!(!state.is_terminal());

        state.assign(RunnerId::new()).unwrap();
        assert!(!state.is_terminal());

        state.start().unwrap();
        assert!(!state.is_terminal());

        state.succeed(0).unwrap();
        assert!(state.is_terminal());
    }

    #[test]
    fn test_summary() {
        let job_id = JobId::new();
        let runner_id = RunnerId::new();
        let mut state = JobStateMachine::new(job_id);

        state.queue().unwrap();
        state.assign(runner_id).unwrap();
        state.start().unwrap();
        state.succeed(0).unwrap();

        let summary = state.summary();
        assert_eq!(summary.job_id, job_id);
        assert_eq!(summary.status, JobStatus::Succeeded);
        assert_eq!(summary.runner_id, Some(runner_id));
        assert_eq!(summary.exit_code, Some(0));
        assert!(summary.error_message.is_none());
    }

    #[test]
    fn test_summary_with_error() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);

        state.queue().unwrap();
        state.assign(RunnerId::new()).unwrap();
        state.start().unwrap();
        state.fail(1, "test error".to_string()).unwrap();

        let summary = state.summary();
        assert_eq!(summary.error_message, Some("test error".to_string()));
    }

    #[test]
    fn test_pending_to_cancelled() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);
        // Direct cancel from pending
        state.cancel().unwrap();
        assert_eq!(state.status(), JobStatus::Cancelled);
    }

    #[test]
    fn test_cannot_assign_from_pending() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);
        // Can't assign directly from pending - must queue first
        assert!(state.assign(RunnerId::new()).is_err());
    }

    #[test]
    fn test_cannot_fail_from_pending() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);
        // Can't fail directly from pending - must go through queue/assigned/running
        assert!(state.fail(1, "error".to_string()).is_err());
    }

    #[test]
    fn test_cannot_succeed_from_pending() {
        let job_id = JobId::new();
        let mut state = JobStateMachine::new(job_id);
        // Can't succeed directly from pending
        assert!(state.succeed(0).is_err());
    }
}
