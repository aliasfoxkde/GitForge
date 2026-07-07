//! Time utilities for GitForce
//!
//! Provides timezone-aware datetime types and utilities.

use chrono::{DateTime as ChronoDateTime, Utc};
use serde::{Deserialize, Serialize};

/// GitForge's standard timestamp type (UTC)
pub type DateTime = ChronoDateTime<Utc>;

/// Get current UTC timestamp
pub fn now() -> DateTime {
    Utc::now()
}

/// Get current Unix timestamp in milliseconds
pub fn unix_timestamp_ms() -> i64 {
    Utc::now().timestamp_millis()
}

/// Get current Unix timestamp in seconds
pub fn unix_timestamp() -> i64 {
    Utc::now().timestamp()
}

/// Calculate duration between two timestamps in milliseconds
pub fn duration_ms(start: DateTime, end: DateTime) -> i64 {
    (end - start).num_milliseconds()
}

/// Check if a timestamp is older than the given duration
pub fn is_older_than(timestamp: DateTime, duration: chrono::Duration) -> bool {
    Utc::now() - timestamp > duration
}

/// Duration shorthand constructors
pub mod duration {
    use chrono::Duration;

    pub fn seconds(s: i64) -> Duration {
        Duration::seconds(s)
    }

    pub fn minutes(m: i64) -> Duration {
        Duration::minutes(m)
    }

    pub fn hours(h: i64) -> Duration {
        Duration::hours(h)
    }

    pub fn days(d: i64) -> Duration {
        Duration::days(d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_now() {
        let ts = now();
        assert!(ts <= Utc::now());
    }

    #[test]
    fn test_unix_timestamp() {
        let ts = unix_timestamp();
        assert!(ts > 0);
        assert_eq!(ts, Utc::now().timestamp());
    }

    #[test]
    fn test_unix_timestamp_ms() {
        let ts = unix_timestamp_ms();
        assert!(ts > 0);
    }

    #[test]
    fn test_duration_helpers() {
        assert_eq!(duration::seconds(60), duration::seconds(60));
        assert_eq!(duration::minutes(5), duration::minutes(5));
        assert_eq!(duration::hours(2), duration::hours(2));
        assert_eq!(duration::days(1), duration::days(1));
    }

    #[test]
    fn test_is_older_than() {
        let old = Utc::now() - chrono::Duration::hours(2);
        let recent = Utc::now() - chrono::Duration::minutes(5);

        assert!(is_older_than(old, chrono::Duration::hours(1)));
        assert!(!is_older_than(recent, chrono::Duration::hours(1)));
    }
}
