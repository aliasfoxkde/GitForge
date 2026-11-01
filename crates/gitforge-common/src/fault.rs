//! Failure-injection fault kinds (M10).
//!
//! A `fault:` key on a pipeline job asks the RUNNER to simulate a failure
//! instead of executing the job's steps. The surface is inert unless the
//! deployment enabled fault injection (see `crate::rollout::RolloutFlags`)
//! and the parser rejects the key outright when it is disabled, so a stale
//! or hard-off deployment can never silently grade a green commit red.

use serde::{Deserialize, Serialize};

/// Failure mode a job asks the runner to simulate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// Simulate the job deadline elapsing: graded as timed out, exit -1.
    Timeout,
    /// Simulate a plain code failure: graded failed with exit code 1.
    #[serde(rename = "nonzero-exit")]
    NonZeroExit,
    /// Simulate a cgroup OOM kill: graded failed with exit code 137 (128 +
    /// SIGKILL), matching how a container memory kill presents.
    Oom,
}

impl Fault {
    pub fn as_str(self) -> &'static str {
        match self {
            Fault::Timeout => "timeout",
            Fault::NonZeroExit => "nonzero-exit",
            Fault::Oom => "oom",
        }
    }

    /// Parse the YAML spelling of a fault kind. Only the documented,
    /// lowercase forms are accepted — no aliases, no casing tolerance, so a
    /// typo is a loud parse error.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "timeout" => Some(Fault::Timeout),
            "nonzero-exit" => Some(Fault::NonZeroExit),
            "oom" => Some(Fault::Oom),
            _ => None,
        }
    }

    /// Every fault kind, in documentation order.
    pub fn all() -> [Fault; 3] {
        [Fault::Timeout, Fault::NonZeroExit, Fault::Oom]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_faults() {
        for fault in Fault::all() {
            assert_eq!(Fault::parse(fault.as_str()), Some(fault));
        }
    }

    #[test]
    fn rejects_unknown_fault_values() {
        assert_eq!(Fault::parse("segfault"), None);
        assert_eq!(Fault::parse(""), None);
        assert_eq!(Fault::parse("TIMEOUT"), None);
        assert_eq!(Fault::parse("non_zero_exit"), None);
    }

    #[test]
    fn serde_round_trip_is_snake_case() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            fault: Fault,
        }
        let parsed: Wrapper =
            serde_yaml::from_str("fault: nonzero-exit").expect("nonzero-exit yaml");
        assert_eq!(parsed.fault, Fault::NonZeroExit);
        let serialized = serde_yaml::to_string(&Wrapper { fault: Fault::Oom }).expect("serialize");
        assert!(serialized.contains("oom"), "yaml spelling: {serialized}");
    }
}
