//! Deployment rollout flags (M10).
//!
//! A rollout flag gates an opt-in surface (today: fault injection) per
//! deployment. The decision core takes the environment as DATA: the raw
//! environment value is read once at the edge (service startup) and passed
//! in, so the core is pure and testable without touching process-global
//! state.

/// Rollout switches for surfaces that ship dark.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RolloutFlags {
    /// Fault injection (`fault:` job key and the runner's simulation path).
    pub fault_injection: bool,
}

impl RolloutFlags {
    /// Core rule for the `GITFORGE_ENABLE_FAULT_INJECTION` value. Only an
    /// exact opt-in spelling enables the surface; unset, empty, garbage,
    /// and `0`/`false` all keep it off (default off).
    pub fn fault_injection_enabled(env_value: Option<&str>) -> bool {
        matches!(
            env_value.map(str::trim),
            Some("1") | Some("true") | Some("yes")
        )
    }

    /// Edge adapter: derive the flags from one env lookup result.
    pub fn from_env_value(fault_injection: Option<&str>) -> Self {
        Self {
            fault_injection: Self::fault_injection_enabled(fault_injection),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fault_injection_is_off_by_default() {
        assert!(!RolloutFlags::fault_injection_enabled(None));
        assert!(!RolloutFlags::fault_injection_enabled(Some("")));
        assert_eq!(RolloutFlags::default(), RolloutFlags::from_env_value(None));
    }

    #[test]
    fn fault_injection_requires_explicit_opt_in() {
        for value in ["1", "true", "yes", " 1 "] {
            assert!(
                RolloutFlags::fault_injection_enabled(Some(value)),
                "must enable: {value}"
            );
        }
    }

    #[test]
    fn fault_injection_ignores_garbage_and_negatives() {
        for value in ["0", "false", "no", "on", "enabled", "True"] {
            assert!(
                !RolloutFlags::fault_injection_enabled(Some(value)),
                "must stay off: {value}"
            );
        }
    }
}
