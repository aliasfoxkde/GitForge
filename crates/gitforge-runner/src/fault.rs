//! Runner-side fault simulation (M10).
//!
//! When a job carries a `fault:` request (parsed only on deployments with
//! the fault-injection rollout flag enabled), the runner grades the job
//! from the fault instead of executing any step. The mapping from fault
//! kind to outcome is a pure decision core: no environment, no clock, no
//! I/O — callers supply what the outcome needs as data.

use gitforge_common::Fault;

/// A simulated job outcome. Mirrors the fields of `JobResult` that grading
/// depends on, so the runner can synthesize a faithful result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FaultOutcome {
    pub success: bool,
    pub exit_code: i32,
    pub timed_out: bool,
}

/// Pure rule: what outcome does this fault simulate?
pub fn fault_outcome(fault: Fault) -> FaultOutcome {
    match fault {
        // The job deadline elapsing: the runner's kill path grades exit -1
        // with `timed_out` set, and the receipt lands as TimedOut.
        Fault::Timeout => FaultOutcome {
            success: false,
            exit_code: -1,
            timed_out: true,
        },
        // A plain code failure: red X on the commit.
        Fault::NonZeroExit => FaultOutcome {
            success: false,
            exit_code: 1,
            timed_out: false,
        },
        // A cgroup OOM kill presents as SIGKILL: 128 + 9 = 137.
        Fault::Oom => FaultOutcome {
            success: false,
            exit_code: 137,
            timed_out: false,
        },
    }
}

/// Pure rule: the diagnostic line the runner reports for a simulated fault.
/// Names the injection explicitly so a green-eyes review of the run log can
/// never mistake it for an organic failure.
pub fn fault_error(fault: Fault) -> String {
    format!(
        "fault injection: simulated '{}' (M10 failure injection; the job's \
         steps were not executed)",
        fault.as_str()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeout_fault_grades_as_timed_out() {
        let outcome = fault_outcome(Fault::Timeout);
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, -1);
        assert!(outcome.timed_out);
    }

    #[test]
    fn nonzero_exit_fault_grades_as_code_failure() {
        let outcome = fault_outcome(Fault::NonZeroExit);
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, 1);
        assert!(!outcome.timed_out);
    }

    #[test]
    fn oom_fault_grades_as_sigkill_exit_code() {
        let outcome = fault_outcome(Fault::Oom);
        assert!(!outcome.success);
        assert_eq!(outcome.exit_code, 137);
        assert!(!outcome.timed_out);
    }

    #[test]
    fn fault_error_names_the_injection() {
        for fault in Fault::all() {
            let error = fault_error(fault);
            assert!(error.contains("fault injection"), "{error}");
            assert!(error.contains(fault.as_str()), "{error}");
            assert!(error.contains("not executed"), "{error}");
        }
    }
}
