//! Seeded value generators.
//!
//! Tests that need "some string" or "some timestamp" should reach for
//! these instead of `uuid::Uuid::new_v4()` inline, so a failing test can
//! replay the exact inputs: every generator is derived from a caller
//! [`Seed`], and the same seed produces the same values forever.

use std::sync::atomic::{AtomicU64, Ordering};

/// Opaque generator seed. Cheap to copy; derive more generators from one
/// seed with [`Seed::spawn`] so each gets an independent counter without
/// sharing sequence state.
#[derive(Debug, Clone)]
pub struct Seed {
    prefix: String,
    counter: std::sync::Arc<AtomicU64>,
}

impl Seed {
    /// A seed from a fixed prefix. Deterministic across runs.
    pub fn new(prefix: impl Into<String>) -> Self {
        Self {
            prefix: prefix.into(),
            counter: std::sync::Arc::new(AtomicU64::new(0)),
        }
    }

    /// Derive an independent generator from this one. The child's values
    /// are namespaced under the parent's prefix, so two children can never
    /// collide with each other or with the parent.
    pub fn spawn(&self, suffix: &str) -> Self {
        Self::new(format!("{}-{suffix}", self.prefix))
    }

    /// The seed's namespace prefix.
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// A unique, readable, deterministic name: `prefix-0`, `prefix-1`, ...
    pub fn next_name(&self) -> String {
        format!(
            "{}-{}",
            self.prefix,
            self.counter.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// A deterministic v4-shaped uuid derived from the seed's namespace
    /// and counter (not random — the same seed replays the same uuids).
    pub fn next_uuid(&self) -> uuid::Uuid {
        let name = self.next_name();
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, name.as_bytes())
    }

    /// A timestamp `offset_ms` after the Unix epoch — fixed base, so
    /// assertions on ordering and formatting never depend on the wall
    /// clock (the environment-as-data rule: the clock is an input).
    pub fn timestamp(offset_ms: i64) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::from_timestamp_millis(1_700_000_000_000 + offset_ms)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_sequential_and_namespaced() {
        let seed = Seed::new("t");
        assert_eq!(seed.next_name(), "t-0");
        assert_eq!(seed.next_name(), "t-1");
        let child = seed.spawn("b");
        assert_eq!(child.next_name(), "t-b-0");
        assert_eq!(seed.next_name(), "t-2");
    }

    #[test]
    fn uuids_replay_for_the_same_seed() {
        let a = Seed::new("replay");
        let b = Seed::new("replay");
        assert_eq!(a.next_uuid(), b.next_uuid());
        assert_ne!(a.next_uuid(), a.next_uuid());
    }

    #[test]
    fn timestamps_are_fixed_base_plus_offset() {
        assert_eq!(
            Seed::timestamp(1_500).to_rfc3339(),
            "2023-11-14T22:13:21.500+00:00"
        );
        assert!(Seed::timestamp(10) < Seed::timestamp(20));
    }
}
