//! Execution lanes (#338): scheduling classes a job belongs to.
//!
//! A lane is a durable property of a job, persisted on the `jobs` row and
//! consulted by the assigner's admission walk. Lanes are scheduling policy
//! over the one durable job/lease model — not a separate queue.

use serde::{Deserialize, Serialize};

/// Scheduling lane for a pipeline job.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    /// Formatting, metadata, changed-file classification, targeted lints.
    Fast,
    /// Ordinary builds and affected test suites.
    #[default]
    Standard,
    /// Full workspace tests, coverage, fuzzing, releases.
    Heavy,
    /// Cache warming, graph refresh, GC, compaction.
    Background,
}

impl Lane {
    pub fn as_str(self) -> &'static str {
        match self {
            Lane::Fast => "fast",
            Lane::Standard => "standard",
            Lane::Heavy => "heavy",
            Lane::Background => "background",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "fast" => Some(Lane::Fast),
            "standard" => Some(Lane::Standard),
            "heavy" => Some(Lane::Heavy),
            "background" => Some(Lane::Background),
            _ => None,
        }
    }

    /// Every lane, in dispatch-enumeration order.
    pub fn all_lanes() -> [Lane; 4] {
        [Lane::Fast, Lane::Standard, Lane::Heavy, Lane::Background]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_all_lanes() {
        for lane in [Lane::Fast, Lane::Standard, Lane::Heavy, Lane::Background] {
            assert_eq!(Lane::from_str(lane.as_str()), Some(lane));
        }
    }

    #[test]
    fn rejects_unknown_lane_values() {
        assert_eq!(Lane::from_str("urgent"), None);
        assert_eq!(Lane::from_str(""), None);
        assert_eq!(Lane::from_str("FAST"), None);
    }

    #[test]
    fn default_is_standard() {
        assert_eq!(Lane::default(), Lane::Standard);
    }

    #[test]
    fn serde_round_trip() {
        #[derive(Serialize, Deserialize)]
        struct Wrapper {
            #[serde(default)]
            lane: Lane,
        }
        let parsed: Wrapper = serde_yaml::from_str("lane: heavy").expect("heavy yaml");
        assert_eq!(parsed.lane, Lane::Heavy);
        let parsed: Wrapper = serde_yaml::from_str("{}").expect("empty yaml");
        assert_eq!(parsed.lane, Lane::Standard);
    }
}
