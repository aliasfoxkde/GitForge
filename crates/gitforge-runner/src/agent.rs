//! Runner agent

use crate::executor::{ExecutableJob, JobExecutor, JobStep};
use gitforge_common::{Error, JobId, PipelineRunId, Result, RunnerId};
use gitforge_db::models::Runner;
use gitforge_sandbox::{DockerSandbox, OutputSink, OutputStream, StepResult};
use gitforge_storage::ArtifactReceipt;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use tokio::fs;
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, Duration};

/// Runner configuration
#[derive(Clone)]
pub struct RunnerConfig {
    /// Scheduler URL for job fetching
    pub scheduler_url: String,
    /// Runner name
    pub name: String,
    /// Runner type
    pub runner_type: String,
    /// Capacity (number of concurrent jobs)
    pub capacity: i32,
    /// Heartbeat interval in seconds
    pub heartbeat_interval_secs: u64,
    /// Job fetch interval in seconds
    pub fetch_interval_secs: u64,
    /// Bearer token used for scheduler service authentication.
    pub scheduler_token: Option<String>,
    /// Registration attempts against an unreachable scheduler before giving
    /// up. Covers the compose race where a runner starts before the
    /// scheduler's listener is up.
    pub register_attempts: u32,
    /// Initial delay in seconds between registration attempts. Doubles after
    /// every failed attempt up to [`REGISTER_BACKOFF_CAP_SECS`].
    pub register_backoff_secs: u64,
    /// Whether registration failure may fall back to standalone execution.
    /// Defaults to `false`: a runner that cannot register exits instead of
    /// appearing healthy while it can never receive scheduler jobs.
    pub allow_standalone: bool,
}

/// Upper bound for the exponential registration backoff.
const REGISTER_BACKOFF_CAP_SECS: u64 = 30;

/// Sleep out one registration backoff step, doubling the delay for the next
/// attempt. A zero delay (used by tests) stays zero.
async fn wait_registration_backoff(backoff: &mut u64) {
    tokio::time::sleep(Duration::from_secs(*backoff)).await;
    *backoff = next_registration_backoff(*backoff);
}

/// Compute the next registration backoff step: double the current delay,
/// capped at [`REGISTER_BACKOFF_CAP_SECS`].
fn next_registration_backoff(current: u64) -> u64 {
    current.saturating_mul(2).min(REGISTER_BACKOFF_CAP_SECS)
}

impl fmt::Debug for RunnerConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RunnerConfig")
            .field("scheduler_url", &self.scheduler_url)
            .field("name", &self.name)
            .field("runner_type", &self.runner_type)
            .field("capacity", &self.capacity)
            .field("heartbeat_interval_secs", &self.heartbeat_interval_secs)
            .field("fetch_interval_secs", &self.fetch_interval_secs)
            .field(
                "scheduler_token",
                &self.scheduler_token.as_ref().map(|_| "<redacted>"),
            )
            .field("register_attempts", &self.register_attempts)
            .field("register_backoff_secs", &self.register_backoff_secs)
            .field("allow_standalone", &self.allow_standalone)
            .finish()
    }
}

impl Default for RunnerConfig {
    fn default() -> Self {
        // Hardcoded safe defaults — used only by existing tests that construct
        // RunnerConfig directly without env vars.
        Self {
            scheduler_url: "http://localhost:42781".to_string(),
            name: "runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 2,
            heartbeat_interval_secs: 30,
            fetch_interval_secs: 5,
            scheduler_token: None,
            register_attempts: 6,
            register_backoff_secs: 1,
            allow_standalone: false,
        }
    }
}

impl RunnerConfig {
    /// Parse runner configuration from an iterator of environment variable
    /// (key, value) pairs. This is a pure helper that makes the parsing logic
    /// fully testable without touching the process environment.
    ///
    /// The following keys are read; all others are ignored:
    /// - `GITFORGE_SCHEDULER_URL` (required)
    /// - `GITFORGE_RUNNER_NAME` (optional, default: `"runner"`)
    /// - `GITFORGE_RUNNER_CAPACITY` (optional, default: `2`)
    /// - `GITFORGE_HEARTBEAT_INTERVAL` (optional, default: `30`)
    /// - `GITFORGE_FETCH_INTERVAL` (optional, default: `5`)
    /// - `GITFORGE_SCHEDULER_TOKEN` (optional, default: `None`)
    /// - `GITFORGE_REGISTER_ATTEMPTS` (optional, default: `6`)
    /// - `GITFORGE_REGISTER_BACKOFF_SECS` (optional, default: `1`)
    ///
    /// # Errors
    ///
    /// Returns an error if `GITFORGE_SCHEDULER_URL` is missing or empty, or
    /// if any numeric variable is present but fails to parse or is not positive.
    fn parse_from_iter<I, K, V>(iter: I) -> Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: AsRef<str>,
    {
        let mut scheduler_url: Option<String> = None;
        let mut name: Option<String> = None;
        let mut capacity: Option<i32> = None;
        let mut heartbeat_interval_secs: Option<u64> = None;
        let mut fetch_interval_secs: Option<u64> = None;
        let mut scheduler_token: Option<Option<String>> = None;
        let mut register_attempts: Option<u32> = None;
        let mut register_backoff_secs: Option<u64> = None;
        let mut allow_standalone: Option<bool> = None;

        for (key, value) in iter {
            let key = key.as_ref();
            let value = value.as_ref();
            match key {
                "GITFORGE_SCHEDULER_URL" => {
                    let v = value.trim().to_string();
                    if !v.is_empty() {
                        scheduler_url = Some(v);
                    }
                }
                "GITFORGE_RUNNER_NAME" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        name = Some(v.to_string());
                    }
                }
                "GITFORGE_RUNNER_CAPACITY" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        let parsed: i64 = v.parse().map_err(|_| {
                            Error::invalid_input("GITFORGE_RUNNER_CAPACITY must be a valid integer")
                        })?;
                        if parsed <= 0 {
                            return Err(Error::invalid_input(format!(
                                "GITFORGE_RUNNER_CAPACITY must be a positive integer (got {})",
                                parsed
                            )));
                        }
                        capacity = Some(parsed as i32);
                    }
                }
                "GITFORGE_HEARTBEAT_INTERVAL" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        let parsed: i64 = v.parse().map_err(|_| {
                            Error::invalid_input(
                                "GITFORGE_HEARTBEAT_INTERVAL must be a valid integer",
                            )
                        })?;
                        if parsed <= 0 {
                            return Err(Error::invalid_input(format!(
                                "GITFORGE_HEARTBEAT_INTERVAL must be a positive integer (got {})",
                                parsed
                            )));
                        }
                        heartbeat_interval_secs = Some(parsed as u64);
                    }
                }
                "GITFORGE_FETCH_INTERVAL" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        let parsed: i64 = v.parse().map_err(|_| {
                            Error::invalid_input("GITFORGE_FETCH_INTERVAL must be a valid integer")
                        })?;
                        if parsed <= 0 {
                            return Err(Error::invalid_input(format!(
                                "GITFORGE_FETCH_INTERVAL must be a positive integer (got {})",
                                parsed
                            )));
                        }
                        fetch_interval_secs = Some(parsed as u64);
                    }
                }
                "GITFORGE_SCHEDULER_TOKEN" => {
                    let v = value.trim();
                    scheduler_token = Some(if v.is_empty() {
                        None
                    } else {
                        Some(v.to_string())
                    });
                }
                "GITFORGE_REGISTER_ATTEMPTS" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        let parsed: i64 = v.parse().map_err(|_| {
                            Error::invalid_input(
                                "GITFORGE_REGISTER_ATTEMPTS must be a valid integer",
                            )
                        })?;
                        if parsed <= 0 {
                            return Err(Error::invalid_input(format!(
                                "GITFORGE_REGISTER_ATTEMPTS must be a positive integer (got {})",
                                parsed
                            )));
                        }
                        register_attempts = Some(parsed as u32);
                    }
                }
                "GITFORGE_REGISTER_BACKOFF_SECS" => {
                    let v = value.trim();
                    if !v.is_empty() {
                        let parsed: i64 = v.parse().map_err(|_| {
                            Error::invalid_input(
                                "GITFORGE_REGISTER_BACKOFF_SECS must be a valid integer",
                            )
                        })?;
                        if parsed < 0 {
                            return Err(Error::invalid_input(format!(
                                "GITFORGE_REGISTER_BACKOFF_SECS must not be negative (got {})",
                                parsed
                            )));
                        }
                        register_backoff_secs = Some(parsed as u64);
                    }
                }
                "GITFORGE_RUNNER_STANDALONE" => {
                    let v = value.trim().to_ascii_lowercase();
                    if !v.is_empty() {
                        allow_standalone = Some(match v.as_str() {
                            "allow" | "true" | "1" => true,
                            "deny" | "false" | "0" => false,
                            other => {
                                return Err(Error::invalid_input(format!(
                                    "GITFORGE_RUNNER_STANDALONE must be allow or deny (got {})",
                                    other
                                )))
                            }
                        });
                    }
                }
                _ => {}
            }
        }

        let scheduler_url = scheduler_url.ok_or_else(|| {
            Error::invalid_input(
                "GITFORGE_SCHEDULER_URL is not set or is empty; \
                 the runner requires a scheduler URL to register with.\n\
                 Hint: set GITFORGE_SCHEDULER_URL=http://localhost:42781 (or your CI address)",
            )
        })?;

        Ok(Self {
            scheduler_url,
            name: name.unwrap_or_else(|| "runner".to_string()),
            runner_type: "docker".to_string(),
            capacity: capacity.unwrap_or(2),
            heartbeat_interval_secs: heartbeat_interval_secs.unwrap_or(30),
            fetch_interval_secs: fetch_interval_secs.unwrap_or(5),
            scheduler_token: scheduler_token.unwrap_or(None),
            register_attempts: register_attempts.unwrap_or(6),
            register_backoff_secs: register_backoff_secs.unwrap_or(1),
            allow_standalone: allow_standalone.unwrap_or(false),
        })
    }

    /// Load runner configuration from the environment, then validate.
    ///
    /// **Required:**
    /// - `GITFORGE_SCHEDULER_URL` — scheduler HTTP endpoint (e.g. `http://localhost:42781`).
    ///   If unset or empty, the runner exits immediately with an error at startup.
    ///
    /// **Optional** (safe defaults when absent):
    /// - `GITFORGE_RUNNER_NAME`        — runner display name (default: `"runner"`)
    /// - `GITFORGE_RUNNER_CAPACITY`   — concurrent job slots (default: `2`)
    /// - `GITFORGE_HEARTBEAT_INTERVAL` — heartbeat seconds (default: `30`)
    /// - `GITFORGE_FETCH_INTERVAL`    — job-poll seconds (default: `5`)
    ///
    /// **Optional credentials** (no default — runner runs unauthenticated if unset):
    /// - `GITFORGE_SCHEDULER_TOKEN`  — bearer token for scheduler API
    ///
    /// **Optional policy:**
    /// - `GITFORGE_RUNNER_STANDALONE` — `deny` (default) makes registration
    ///   failure fatal at startup; `allow` restores legacy standalone
    ///   fallback where an unreachable scheduler still permits local
    ///   execution.
    ///
    /// # Errors
    ///
    /// Returns an error if `GITFORGE_SCHEDULER_URL` is missing or empty.
    /// Numeric parse failures also cause startup to fail fast.
    pub fn from_env() -> Result<Self> {
        Self::parse_from_iter(std::env::vars())
    }
}

#[cfg(test)]
mod config_tests {
    use super::*;

    /// Helper: construct a Vec of (key, value) from an iterator of Option pairs,
    /// similar to what `std::env::vars()` would return but pure and isolated.
    fn env<'a>(
        vars: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    ) -> Vec<(String, String)> {
        vars.into_iter()
            .filter_map(|(k, v)| v.map(|v| (k.to_string(), v.to_string())))
            .collect()
    }

    /// Helper: empty env slice (no GITFORGE_ vars at all)
    fn empty_env() -> Vec<(String, String)> {
        Vec::new()
    }

    /// Helper: error message contains a substring (used for deterministic assertions)
    fn err_contains(err: &gitforge_common::Error, needle: &str) -> bool {
        err.message.contains(needle)
    }

    // ── Valid / complete ─────────────────────────────────────────────────────

    #[test]
    fn test_parse_from_iter_valid_complete() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://ci:42781")),
            ("GITFORGE_RUNNER_NAME", Some("prod-runner-01")),
            ("GITFORGE_RUNNER_CAPACITY", Some("4")),
            ("GITFORGE_HEARTBEAT_INTERVAL", Some("60")),
            ("GITFORGE_FETCH_INTERVAL", Some("10")),
            ("GITFORGE_SCHEDULER_TOKEN", Some("secret-token")),
        ]);
        let cfg = RunnerConfig::parse_from_iter(vars).expect("valid env should parse");
        assert_eq!(cfg.scheduler_url, "http://ci:42781");
        assert_eq!(cfg.name, "prod-runner-01");
        assert_eq!(cfg.capacity, 4);
        assert_eq!(cfg.heartbeat_interval_secs, 60);
        assert_eq!(cfg.fetch_interval_secs, 10);
        assert_eq!(cfg.scheduler_token.as_deref(), Some("secret-token"));
        assert_eq!(cfg.runner_type, "docker");
    }

    // ── Optional fields absent → defaults ───────────────────────────────────

    #[test]
    fn test_parse_from_iter_optional_defaults() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_RUNNER_NAME", None),
            ("GITFORGE_RUNNER_CAPACITY", None),
            ("GITFORGE_HEARTBEAT_INTERVAL", None),
            ("GITFORGE_FETCH_INTERVAL", None),
            ("GITFORGE_SCHEDULER_TOKEN", None),
        ]);
        let cfg = RunnerConfig::parse_from_iter(vars).expect("valid env should parse");
        assert_eq!(cfg.scheduler_url, "http://localhost:42781");
        assert_eq!(cfg.name, "runner");
        assert_eq!(cfg.capacity, 2);
        assert_eq!(cfg.heartbeat_interval_secs, 30);
        assert_eq!(cfg.fetch_interval_secs, 5);
        assert!(cfg.scheduler_token.is_none());
    }

    // ── Missing required GITFORGE_SCHEDULER_URL ─────────────────────────────

    #[test]
    fn test_parse_from_iter_missing_scheduler_url() {
        let vars = empty_env();
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("missing GITFORGE_SCHEDULER_URL should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_SCHEDULER_URL"));
    }

    #[test]
    fn test_parse_from_iter_empty_scheduler_url() {
        let vars = env([("GITFORGE_SCHEDULER_URL", Some(""))]);
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("empty GITFORGE_SCHEDULER_URL should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_SCHEDULER_URL"));
    }

    // ── Non-numeric GITFORGE_RUNNER_CAPACITY ───────────────────────────────

    #[test]
    fn test_parse_from_iter_invalid_capacity() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_RUNNER_CAPACITY", Some("not-a-number")),
        ]);
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("invalid GITFORGE_RUNNER_CAPACITY should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_RUNNER_CAPACITY"));
    }

    // ── Non-numeric GITFORGE_HEARTBEAT_INTERVAL ────────────────────────────

    #[test]
    fn test_parse_from_iter_invalid_heartbeat_interval() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_HEARTBEAT_INTERVAL", Some("not-an-int")),
        ]);
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("invalid GITFORGE_HEARTBEAT_INTERVAL should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_HEARTBEAT_INTERVAL"));
    }

    // ── GITFORGE_FETCH_INTERVAL negative ───────────────────────────────────

    #[test]
    fn test_parse_from_iter_negative_fetch_interval() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_FETCH_INTERVAL", Some("-5")),
        ]);
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("negative GITFORGE_FETCH_INTERVAL should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_FETCH_INTERVAL"));
        assert!(err_contains(&err, "positive integer"));
    }

    // ── Zero values ─────────────────────────────────────────────────────────

    #[test]
    fn test_parse_from_iter_zero_capacity() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_RUNNER_CAPACITY", Some("0")),
        ]);
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("zero GITFORGE_RUNNER_CAPACITY should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_RUNNER_CAPACITY"));
        assert!(err_contains(&err, "positive integer"));
    }

    #[test]
    fn test_parse_from_iter_negative_capacity() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_RUNNER_CAPACITY", Some("-3")),
        ]);
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("negative GITFORGE_RUNNER_CAPACITY should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_RUNNER_CAPACITY"));
        assert!(err_contains(&err, "positive integer"));
    }

    #[test]
    fn test_parse_from_iter_zero_heartbeat_interval() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_HEARTBEAT_INTERVAL", Some("0")),
        ]);
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("zero GITFORGE_HEARTBEAT_INTERVAL should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_HEARTBEAT_INTERVAL"));
        assert!(err_contains(&err, "positive integer"));
    }

    #[test]
    fn test_parse_from_iter_zero_fetch_interval() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_FETCH_INTERVAL", Some("0")),
        ]);
        let result = RunnerConfig::parse_from_iter(vars);
        let err = result.expect_err("zero GITFORGE_FETCH_INTERVAL should fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_FETCH_INTERVAL"));
        assert!(err_contains(&err, "positive integer"));
    }

    // ── Whitespace-only values (empty after trim) ────────────────────────────

    #[test]
    fn test_parse_from_iter_whitespace_only_values() {
        let vars = env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_RUNNER_NAME", Some("   ")),
            ("GITFORGE_RUNNER_CAPACITY", Some("  ")),
            ("GITFORGE_HEARTBEAT_INTERVAL", Some("  ")),
            ("GITFORGE_FETCH_INTERVAL", Some("  ")),
            ("GITFORGE_SCHEDULER_TOKEN", Some("   ")),
        ]);
        let cfg = RunnerConfig::parse_from_iter(vars)
            .expect("whitespace-only values should be treated as absent");
        assert_eq!(cfg.name, "runner");
        assert_eq!(cfg.capacity, 2);
        assert_eq!(cfg.heartbeat_interval_secs, 30);
        assert_eq!(cfg.fetch_interval_secs, 5);
        assert!(cfg.scheduler_token.is_none());
    }

    // ── Ignored keys (no effect) ────────────────────────────────────────────

    #[test]
    fn test_parse_from_iter_ignores_unknown_keys() {
        let mut vars = env([("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781"))]);
        // Add arbitrary non-GITFORGE_ vars — they must be ignored without error
        vars.push(("HOME".to_string(), "/home/test".to_string()));
        vars.push(("PATH".to_string(), "/usr/bin".to_string()));
        vars.push((
            "GITFORGE_UNKNOWN_VAR".to_string(),
            "should be ignored".to_string(),
        ));
        let cfg = RunnerConfig::parse_from_iter(vars).expect("unknown keys should be ignored");
        assert_eq!(cfg.scheduler_url, "http://localhost:42781");
    }

    // ── Default() still works ───────────────────────────────────────────────

    #[test]
    fn test_default_returns_safe_defaults() {
        let cfg = RunnerConfig::default();
        assert_eq!(cfg.scheduler_url, "http://localhost:42781");
        assert_eq!(cfg.name, "runner");
        assert_eq!(cfg.runner_type, "docker");
        assert_eq!(cfg.capacity, 2);
        assert_eq!(cfg.heartbeat_interval_secs, 30);
        assert_eq!(cfg.fetch_interval_secs, 5);
        assert!(cfg.scheduler_token.is_none());
    }

    #[test]
    fn test_standalone_policy_defaults_to_fail_closed() {
        let cfg = RunnerConfig::parse_from_iter(env(vec![(
            "GITFORGE_SCHEDULER_URL",
            Some("http://localhost:42781"),
        )]))
        .unwrap();
        assert!(!cfg.allow_standalone, "standalone fallback must be opt-in");
    }

    #[test]
    fn test_standalone_policy_accepts_allow_and_deny() {
        for (raw, expected) in [
            ("allow", true),
            ("ALLOW", true),
            ("true", true),
            ("1", true),
            ("deny", false),
            ("false", false),
            ("0", false),
        ] {
            let cfg = RunnerConfig::parse_from_iter(vec![
                (
                    "GITFORGE_SCHEDULER_URL".to_string(),
                    "http://localhost:42781".to_string(),
                ),
                ("GITFORGE_RUNNER_STANDALONE".to_string(), raw.to_string()),
            ])
            .unwrap_or_else(|err| panic!("valid value {raw} rejected: {err}"));
            assert_eq!(cfg.allow_standalone, expected, "value {raw}");
        }
    }

    #[test]
    fn test_standalone_policy_rejects_unknown_values() {
        let err = RunnerConfig::parse_from_iter(vec![
            (
                "GITFORGE_SCHEDULER_URL".to_string(),
                "http://localhost:42781".to_string(),
            ),
            (
                "GITFORGE_RUNNER_STANDALONE".to_string(),
                "maybe".to_string(),
            ),
        ])
        .unwrap_err();
        assert!(
            err.to_string().contains("GITFORGE_RUNNER_STANDALONE"),
            "unexpected error: {err}"
        );
    }

    // ── GITFORGE_REGISTER_ATTEMPTS / GITFORGE_REGISTER_BACKOFF_SECS ─────────

    #[test]
    fn test_parse_registration_retry_defaults() {
        let cfg = RunnerConfig::parse_from_iter(env([(
            "GITFORGE_SCHEDULER_URL",
            Some("http://localhost:42781"),
        )]))
        .unwrap();
        assert_eq!(cfg.register_attempts, 6);
        assert_eq!(cfg.register_backoff_secs, 1);
    }

    #[test]
    fn test_parse_registration_retry_overrides() {
        let cfg = RunnerConfig::parse_from_iter(env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_REGISTER_ATTEMPTS", Some("10")),
            ("GITFORGE_REGISTER_BACKOFF_SECS", Some("4")),
        ]))
        .unwrap();
        assert_eq!(cfg.register_attempts, 10);
        assert_eq!(cfg.register_backoff_secs, 4);
    }

    #[test]
    fn test_parse_registration_attempts_rejects_zero_and_negative() {
        for bad in ["0", "-1"] {
            let err = RunnerConfig::parse_from_iter(env([
                ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
                ("GITFORGE_REGISTER_ATTEMPTS", Some(bad)),
            ]))
            .expect_err("non-positive attempts must fail");
            assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
            assert!(err_contains(&err, "GITFORGE_REGISTER_ATTEMPTS"), "{err}");
            assert!(err_contains(&err, "positive integer"), "{err}");
        }
    }

    #[test]
    fn test_parse_registration_attempts_rejects_non_numeric() {
        let err = RunnerConfig::parse_from_iter(env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_REGISTER_ATTEMPTS", Some("many")),
        ]))
        .expect_err("non-numeric attempts must fail");
        assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
        assert!(err_contains(&err, "GITFORGE_REGISTER_ATTEMPTS"), "{err}");
        assert!(err_contains(&err, "valid integer"), "{err}");
    }

    #[test]
    fn test_parse_registration_backoff_rejects_negative_and_non_numeric() {
        for (bad, needle) in [("-3", "must not be negative"), ("soon", "valid integer")] {
            let err = RunnerConfig::parse_from_iter(env([
                ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
                ("GITFORGE_REGISTER_BACKOFF_SECS", Some(bad)),
            ]))
            .expect_err("invalid backoff must fail");
            assert_eq!(err.kind, gitforge_common::ErrorKind::InvalidInput);
            assert!(
                err_contains(&err, "GITFORGE_REGISTER_BACKOFF_SECS"),
                "{err}"
            );
            assert!(err_contains(&err, needle), "value {bad}: {err}");
        }
    }

    #[test]
    fn test_parse_registration_backoff_allows_zero() {
        // Zero backoff is meaningful: it retries immediately and is what the
        // retry tests use to stay fast.
        let cfg = RunnerConfig::parse_from_iter(env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_REGISTER_BACKOFF_SECS", Some("0")),
        ]))
        .unwrap();
        assert_eq!(cfg.register_backoff_secs, 0);
    }

    #[test]
    fn test_parse_registration_blank_values_use_defaults() {
        let cfg = RunnerConfig::parse_from_iter(env([
            ("GITFORGE_SCHEDULER_URL", Some("http://localhost:42781")),
            ("GITFORGE_REGISTER_ATTEMPTS", Some("  ")),
            ("GITFORGE_REGISTER_BACKOFF_SECS", Some("  ")),
        ]))
        .unwrap();
        assert_eq!(cfg.register_attempts, 6);
        assert_eq!(cfg.register_backoff_secs, 1);
    }
}

/// Job assignment from scheduler
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobAssignment {
    /// Job ID
    pub job_id: String,
    /// Job name
    pub name: String,
    /// Pipeline run ID
    pub pipeline_run_id: String,
    /// Commands to execute
    pub commands: Vec<String>,
    #[serde(default = "default_job_image")]
    pub image: String,
    /// Working directory
    pub working_dir: Option<String>,
    /// Maximum seconds allowed for each command. Defaults for old scheduler
    /// responses are applied during deserialization.
    #[serde(default = "default_job_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_job_timeout_secs() -> u64 {
    300
}

fn default_job_image() -> String {
    "rust:latest".to_string()
}

/// Runner agent that fetches and executes jobs
#[derive(Clone)]
pub struct RunnerAgent {
    config: RunnerConfig,
    client: Client,
    runner: Option<Runner>,
    #[allow(dead_code)]
    sandbox: Arc<DockerSandbox>,
    executor: Arc<JobExecutor>,
    is_running: Arc<RwLock<bool>>,
}

impl RunnerAgent {
    /// Create a new runner agent
    pub async fn new(config: RunnerConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| Error::internal(format!("failed to create HTTP client: {}", e)))?;

        let sandbox = DockerSandbox::connect_required().await?;
        let executor = JobExecutor::new().await?;

        Ok(Self {
            config,
            client,
            runner: None,
            sandbox: Arc::new(sandbox),
            executor: Arc::new(executor),
            is_running: Arc::new(RwLock::new(false)),
        })
    }

    /// Register with the scheduler via HTTP.
    ///
    /// An unreachable scheduler is retried with exponential backoff — a
    /// runner started alongside a restarting control plane (compose race)
    /// must tolerate a listener that is not up yet. Credential and policy
    /// rejections are never retried: a bad token or a refused registration
    /// does not heal by asking again.
    pub async fn register(&mut self) -> Result<RunnerId> {
        let mut runner = Runner::new(
            self.config.name.clone(),
            gitforge_db::models::RunnerType::Docker,
            self.config.capacity,
        );

        let register_url = format!("{}/runners", self.config.scheduler_url);
        let request = serde_json::json!({
            "name": runner.name,
            "type": runner.runner_type,
            "capacity": runner.capacity,
        });

        let attempts = self.config.register_attempts.max(1);
        let mut backoff = self.config.register_backoff_secs;

        for attempt in 1..=attempts {
            let mut register_request = self.client.post(&register_url).json(&request);
            if let Some(token) = &self.config.scheduler_token {
                register_request = register_request.bearer_auth(token);
            }
            match register_request.send().await {
                Ok(response) if response.status().is_success() => {
                    if let Ok(payload) = response.json::<serde_json::Value>().await {
                        if let Some(id) = payload["id"]
                            .as_str()
                            .and_then(|value| uuid::Uuid::parse_str(value).ok())
                        {
                            runner.id = RunnerId::from(id);
                        }
                    }
                    tracing::info!("registered runner {} with scheduler", runner.id);
                    self.runner = Some(runner.clone());
                    return Ok(runner.id);
                }
                Ok(response) => {
                    let status = response.status();
                    // Credentials do not heal by retrying.
                    if status == reqwest::StatusCode::UNAUTHORIZED
                        || status == reqwest::StatusCode::FORBIDDEN
                    {
                        return Err(Error::internal(format!(
                            "scheduler authentication rejected registration: {status}"
                        )));
                    }
                    // SERVICE_UNAVAILABLE means the scheduler is not ready to
                    // answer, which is exactly the transient case.
                    if status == reqwest::StatusCode::SERVICE_UNAVAILABLE && attempt < attempts {
                        tracing::warn!(
                            attempt,
                            attempts,
                            retry_in_secs = backoff,
                            %status,
                            "scheduler not ready; retrying registration"
                        );
                        wait_registration_backoff(&mut backoff).await;
                        continue;
                    }
                    if !self.config.allow_standalone {
                        return Err(Error::internal(format!(
                            "scheduler rejected registration with status {status}; \
                             refusing to run standalone (set GITFORGE_RUNNER_STANDALONE=allow \
                             to override)"
                        )));
                    }
                    tracing::warn!(
                        "scheduler returned {status} for registration, running in standalone mode"
                    );
                    break;
                }
                Err(error) => {
                    if attempt == attempts {
                        if !self.config.allow_standalone {
                            return Err(Error::internal(format!(
                                "failed to register with scheduler after {attempt} attempts: \
                                 {error}; refusing to run standalone \
                                 (set GITFORGE_RUNNER_STANDALONE=allow to override)"
                            )));
                        }
                        tracing::warn!(
                            "failed to register with scheduler after {attempt} attempts: {error}. \
                             Running in standalone mode."
                        );
                        break;
                    }
                    tracing::warn!(
                        attempt,
                        attempts,
                        retry_in_secs = backoff,
                        %error,
                        "scheduler unreachable; retrying registration"
                    );
                    wait_registration_backoff(&mut backoff).await;
                }
            }
        }

        self.runner = Some(runner.clone());
        Ok(runner.id)
    }

    /// Start the runner agent loop
    pub async fn run(&self) -> Result<()> {
        *self.is_running.write().await = true;

        let runner = self
            .runner
            .as_ref()
            .ok_or_else(|| Error::internal("runner not registered"))?;

        let runner_id = runner.id;
        tracing::info!("runner {} starting", runner_id);

        // Start heartbeat loop
        let heartbeat_runner_id = runner_id;
        let heartbeat_interval = self.config.heartbeat_interval_secs;
        let heartbeat_client = self.client.clone();
        let heartbeat_url = self.config.scheduler_url.clone();
        let heartbeat_token = self.config.scheduler_token.clone();
        let is_running = self.is_running.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(heartbeat_interval));
            loop {
                ticker.tick().await;
                if !*is_running.read().await {
                    tracing::debug!("heartbeat loop stopping");
                    break;
                }
                tracing::debug!("runner {} sending heartbeat", heartbeat_runner_id);
                let url = format!(
                    "{}/runners/{}/heartbeat",
                    heartbeat_url, heartbeat_runner_id
                );
                let mut heartbeat_request = heartbeat_client.post(&url);
                if let Some(token) = &heartbeat_token {
                    heartbeat_request = heartbeat_request.bearer_auth(token);
                }
                if let Err(e) = heartbeat_request.send().await {
                    tracing::trace!("heartbeat failed: {}", e);
                }
            }
        });

        // Start job fetch loop
        let fetch_interval = self.config.fetch_interval_secs;
        let fetch_client = self.client.clone();
        let fetch_url = self.config.scheduler_url.clone();
        let fetch_runner_id = runner_id;
        let fetch_token = self.config.scheduler_token.clone();
        let is_running = self.is_running.clone();
        let executor = self.executor.clone();
        let active_jobs: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let active_jobs_for_loop = active_jobs.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(fetch_interval));
            loop {
                ticker.tick().await;
                if !*is_running.read().await {
                    tracing::debug!("job fetch loop stopping");
                    break;
                }
                tracing::debug!("runner checking for jobs...");

                let jobs_url = format!("{}/jobs/pending?runner_id={}", fetch_url, fetch_runner_id);
                let mut fetch_request = fetch_client.get(&jobs_url);
                if let Some(token) = &fetch_token {
                    fetch_request = fetch_request.bearer_auth(token);
                }
                match fetch_request.send().await {
                    Ok(response) => {
                        if response.status().is_success() {
                            if let Ok(jobs) = response.json::<Vec<JobAssignment>>().await {
                                for job in jobs {
                                    tracing::debug!(
                                        "received job assignment: {} ({})",
                                        job.name,
                                        job.job_id
                                    );
                                    // Do not claim a job that this process is
                                    // already executing. Claiming first would
                                    // rotate the durable lease and fence the
                                    // original execution, causing its live-log
                                    // and completion requests to return 409.
                                    {
                                        let active = active_jobs_for_loop.lock().await;
                                        if active.contains(&job.job_id) {
                                            tracing::debug!(
                                                "job {} is already executing locally; ignoring duplicate assignment",
                                                job.job_id
                                            );
                                            continue;
                                        }
                                    }
                                    let Some(lease_token) = Self::claim_job(
                                        &fetch_client,
                                        &fetch_url,
                                        &job.job_id,
                                        fetch_runner_id,
                                        fetch_token.as_deref(),
                                    )
                                    .await
                                    else {
                                        tracing::warn!("unable to claim job {}", job.job_id);
                                        continue;
                                    };
                                    {
                                        let mut active = active_jobs_for_loop.lock().await;
                                        if !active.insert(job.job_id.clone()) {
                                            tracing::warn!("job {} is already executing locally; skipping duplicate assignment", job.job_id);
                                            continue;
                                        }
                                    }
                                    tracing::info!(
                                        "accepted job assignment: {} ({})",
                                        job.name,
                                        job.job_id
                                    );
                                    // Execute concurrently so the fetch loop
                                    // remains responsive and cancellation can
                                    // be observed while the sandbox runs.
                                    let executor = executor.clone();
                                    let client = fetch_client.clone();
                                    let url = fetch_url.clone();
                                    let token = fetch_token.clone();
                                    let active_jobs = active_jobs_for_loop.clone();
                                    let active_job_id = job.job_id.clone();
                                    tokio::spawn(async move {
                                        Self::execute_job(
                                            &executor,
                                            &job,
                                            &client,
                                            &url,
                                            fetch_runner_id,
                                            &lease_token,
                                            token.as_deref(),
                                        )
                                        .await;
                                        active_jobs.lock().await.remove(&active_job_id);
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::trace!("job fetch failed: {}", e);
                    }
                }
            }
        });

        // Keep running until stopped
        while *self.is_running.read().await {
            tokio::time::sleep(Duration::from_secs(1)).await;
        }

        Ok(())
    }

    /// Stop the runner agent
    /// If force is true, cancel all running jobs immediately.
    /// Otherwise, wait for jobs to complete gracefully.
    pub async fn stop(&self, force: bool) {
        *self.is_running.write().await = false;

        if force {
            tracing::info!("force stopping - cancelling all active jobs");
            self.executor.cancel_all_jobs().await;
        }

        let runner_id = self
            .runner
            .as_ref()
            .map(|r| r.id.to_string())
            .unwrap_or_default();
        tracing::info!("runner {} stopped", runner_id);
    }

    /// Wait for all active jobs to complete within the given timeout
    pub async fn wait_for_jobs_complete(&self, timeout_duration: tokio::time::Duration) -> bool {
        self.executor.wait_for_jobs_complete(timeout_duration).await
    }

    /// Check if agent is running
    pub async fn is_running(&self) -> bool {
        *self.is_running.read().await
    }

    /// Execute a job assignment
    async fn claim_job(
        client: &Client,
        scheduler_url: &str,
        job_id: &str,
        runner_id: RunnerId,
        scheduler_token: Option<&str>,
    ) -> Option<String> {
        let url = format!("{}/jobs/{}/claim", scheduler_url, job_id);
        let mut request = client
            .post(url)
            .json(&serde_json::json!({"runner_id": runner_id.to_string()}));
        if let Some(token) = scheduler_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }
        response
            .json::<serde_json::Value>()
            .await
            .ok()?
            .get("lease_token")
            .and_then(|token| token.as_str())
            .map(ToOwned::to_owned)
    }

    /// Execute a job assignment
    async fn execute_job(
        executor: &Arc<JobExecutor>,
        assignment: &JobAssignment,
        client: &Client,
        scheduler_url: &str,
        runner_id: RunnerId,
        lease_token: &str,
        scheduler_token: Option<&str>,
    ) {
        let job_id = match uuid::Uuid::parse_str(&assignment.job_id) {
            Ok(id) => JobId::from(id),
            Err(_) => {
                tracing::error!("invalid job_id: {}", assignment.job_id);
                return;
            }
        };

        // Convert assignment to ExecutableJob
        let pipeline_run_id = uuid::Uuid::parse_str(&assignment.pipeline_run_id)
            .map(PipelineRunId::from)
            .unwrap_or_else(|_| PipelineRunId::new());

        let executable = ExecutableJob {
            job_id,
            pipeline_run_id,
            repository_id: None,
            base_sha: None,
            image: assignment.image.clone(),
            steps: assignment
                .commands
                .iter()
                .map(|cmd| JobStep {
                    name: "run".to_string(),
                    run: cmd.clone(),
                    env: None,
                    working_directory: assignment.working_dir.clone(),
                })
                .collect(),
            env: std::collections::HashMap::new(),
            working_dir: assignment.working_dir.clone(),
            timeout_secs: assignment.timeout_secs.clamp(5, 24 * 60 * 60),
        };

        tracing::info!("executing job {} in container", assignment.job_id);

        let started_url = format!("{}/jobs/{}/started", scheduler_url, assignment.job_id);
        let mut started_request = client.post(&started_url).json(&serde_json::json!({
            "runner_id": runner_id.to_string(),
            "lease_token": lease_token,
        }));
        if let Some(token) = scheduler_token {
            started_request = started_request.bearer_auth(token);
        }
        let started = match started_request.send().await {
            Ok(response) => {
                let status = response.status();
                if status.is_success() {
                    true
                } else {
                    let body = response.text().await.unwrap_or_default();
                    tracing::error!(
                        job_id = %assignment.job_id,
                        %status,
                        response = %body,
                        "failed to mark job started"
                    );
                    false
                }
            }
            Err(error) => {
                tracing::error!(
                    job_id = %assignment.job_id,
                    error = %error,
                    "failed to reach scheduler while marking job started"
                );
                false
            }
        };
        if !started {
            return;
        }

        let cancellation_client = client.clone();
        let cancellation_url = scheduler_url.to_string();
        let cancellation_job_id = assignment.job_id.clone();
        let cancellation_executor = executor.clone();
        let cancellation_token = scheduler_token.map(ToOwned::to_owned);
        // Set when the scheduler says the job's durable outcome was already
        // decided while this execution was still running: an operator
        // cancellation, or restart recovery failing the in-flight row. The
        // lease is gone in both cases, so post-execution reporting can only
        // produce rejected requests.
        let orphaned = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let orphaned_watch = orphaned.clone();
        let cancellation_watch = tokio::spawn(async move {
            let endpoint = format!(
                "{}/jobs/{}/cancelled",
                cancellation_url, cancellation_job_id
            );
            let mut probe_failures = 0u8;
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                let mut request = cancellation_client.get(&endpoint);
                if let Some(token) = &cancellation_token {
                    request = request.bearer_auth(token);
                }
                match request.send().await {
                    Ok(response) if response.status().is_success() => {
                        probe_failures = 0;
                        let cancelled = response
                            .json::<serde_json::Value>()
                            .await
                            .ok()
                            .and_then(|payload| payload["cancelled"].as_bool())
                            .unwrap_or(false);
                        if cancelled {
                            orphaned_watch.store(true, std::sync::atomic::Ordering::Relaxed);
                            if let Ok(job_id) = uuid::Uuid::parse_str(&cancellation_job_id) {
                                let job_id = JobId::from(job_id);
                                if let Err(error) = cancellation_executor.cancel(&job_id).await {
                                    tracing::warn!(%error, %job_id, "failed to destroy cancelled sandbox");
                                }
                            }
                            break;
                        }
                    }
                    Ok(response) => {
                        probe_failures = probe_failures.saturating_add(1);
                        tracing::warn!(status = %response.status(), attempt = probe_failures, "job cancellation probe rejected");
                    }
                    Err(error) => {
                        probe_failures = probe_failures.saturating_add(1);
                        tracing::warn!(%error, attempt = probe_failures, "job cancellation probe failed");
                    }
                }
                if probe_failures >= 3 {
                    tracing::error!(
                        "cancellation probe unavailable repeatedly; stopping local job sandbox"
                    );
                    if let Ok(job_id) = uuid::Uuid::parse_str(&cancellation_job_id) {
                        let _ = cancellation_executor.cancel(&JobId::from(job_id)).await;
                    }
                    break;
                }
            }
        });

        // Execute the job. Output is sent to the scheduler while the sandbox
        // is running; the bounded sink applies network backpressure and never
        // changes the job's success result if observability is degraded.
        let live_logs = Arc::new(LiveLogSink::new(
            client,
            scheduler_url,
            &assignment.job_id,
            runner_id,
            lease_token,
            scheduler_token,
        ));
        let result = executor
            .execute_with_output(executable, Some(live_logs.clone()))
            .await;
        cancellation_watch.abort();

        tracing::info!(
            "job {} completed: success={}, exit_code={}",
            assignment.job_id,
            result.success,
            result.exit_code
        );

        if orphaned.load(std::sync::atomic::Ordering::Relaxed) {
            // The scheduler finalized this job while we were executing it
            // (operator cancellation, or restart recovery re-queuing the
            // row and failing the in-flight execution). The lease no longer
            // exists, so log chunks, artifacts, and a completion POST would
            // all be rejected 409; stop here instead of writing noise.
            tracing::warn!(
                job_id = %assignment.job_id,
                "job outcome was decided by the scheduler mid-execution; \
                 skipping log, artifact, and completion reporting"
            );
            return;
        }

        let protocol = RunnerProtocol {
            client,
            scheduler_url,
            job_id: &assignment.job_id,
            runner_id,
            lease_token,
            scheduler_token,
        };
        if !live_logs.sent_any() || live_logs.failed() {
            // Nothing — or not everything — reached the scheduler live, so
            // re-upload the full step output while the lease is still valid.
            // Chunks append by sequence; a degraded stream may duplicate the
            // prefix it did deliver, which beats a silently truncated log.
            if let Err(error) = report_log_chunks(&protocol, &result.step_results).await {
                tracing::warn!(%error, job_id = %assignment.job_id, "failed to stream job logs");
            }
        }

        let uploaded_artifacts = match report_artifacts(
            &protocol,
            result.workspace_path.as_deref(),
            &result.artifacts,
        )
        .await
        {
            Ok(artifacts) => artifacts,
            Err(error) => {
                tracing::warn!(%error, job_id = %assignment.job_id, "failed to upload job artifacts");
                result
                    .artifacts
                    .iter()
                    .filter_map(|artifact| serde_json::to_value(artifact).ok())
                    .collect()
            }
        };

        // Report completion to scheduler with full results
        let complete_url = format!("{}/jobs/{}/complete", scheduler_url, assignment.job_id);

        // Build step results for reporting
        let step_results_json: Vec<serde_json::Value> = result
            .step_results
            .iter()
            .map(|sr| {
                serde_json::json!({
                    "exit_code": sr.exit_code,
                    // Output is already streamed to the durable log ledger.
                    // Keep the completion receipt bounded so a scanner that
                    // emits megabytes cannot make the completion request fail
                    // and leave the assignment eligible for re-execution.
                    "stdout": bounded_receipt_text(&sr.stdout),
                    "stderr": bounded_receipt_text(&sr.stderr),
                })
            })
            .collect();

        let complete_request = serde_json::json!({
            "contract_version": "harness.job.v1",
            "runner_id": runner_id.to_string(),
            "lease_token": lease_token,
            "success": result.success,
            "exit_code": result.exit_code,
            "error": result.error,
            "step_results": step_results_json,
            "artifacts": uploaded_artifacts,
        });

        let mut complete_request_builder = client.post(&complete_url).json(&complete_request);
        if let Some(token) = scheduler_token {
            complete_request_builder = complete_request_builder.bearer_auth(token);
        }
        match complete_request_builder.send().await {
            Ok(response) if response.status().is_success() => {}
            Ok(response) => {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                tracing::error!(
                    job_id = %assignment.job_id,
                    runner_id = %runner_id,
                    status = %status,
                    response_body = %body,
                    "scheduler rejected job completion"
                );
            }
            Err(error) => tracing::error!("failed to report job completion: {}", error),
        }
    }
}

const MAX_RECEIPT_STREAM_BYTES: usize = 64 * 1024;

fn bounded_receipt_text(value: &str) -> String {
    if value.len() <= MAX_RECEIPT_STREAM_BYTES {
        return value.to_owned();
    }
    let mut end = MAX_RECEIPT_STREAM_BYTES;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[output truncated; full output is available in the job log ledger]",
        &value[..end]
    )
}

#[cfg(test)]
mod receipt_tests {
    use super::{bounded_receipt_text, MAX_RECEIPT_STREAM_BYTES};

    #[test]
    fn receipt_output_is_bounded_and_marked() {
        let output = "x".repeat(MAX_RECEIPT_STREAM_BYTES + 100);
        let receipt = bounded_receipt_text(&output);
        assert!(receipt.len() < MAX_RECEIPT_STREAM_BYTES + 100);
        assert!(receipt.contains("output truncated"));
    }

    #[test]
    fn receipt_output_preserves_small_output() {
        assert_eq!(bounded_receipt_text("ok"), "ok");
    }
}

struct LiveLogSink {
    client: Client,
    endpoint: String,
    runner_id: RunnerId,
    lease_token: String,
    scheduler_token: Option<String>,
    sent_chunks: std::sync::atomic::AtomicUsize,
    failed_delivery: std::sync::atomic::AtomicBool,
}

impl LiveLogSink {
    fn new(
        client: &Client,
        scheduler_url: &str,
        job_id: &str,
        runner_id: RunnerId,
        lease_token: &str,
        scheduler_token: Option<&str>,
    ) -> Self {
        Self {
            client: client.clone(),
            endpoint: format!("{scheduler_url}/jobs/{job_id}/logs"),
            runner_id,
            lease_token: lease_token.to_string(),
            scheduler_token: scheduler_token.map(ToOwned::to_owned),
            sent_chunks: std::sync::atomic::AtomicUsize::new(0),
            failed_delivery: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn sent_any(&self) -> bool {
        self.sent_chunks.load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    fn failed(&self) -> bool {
        self.failed_delivery
            .load(std::sync::atomic::Ordering::Relaxed)
    }
}

#[async_trait::async_trait]
impl OutputSink for LiveLogSink {
    async fn on_output(&self, stream: OutputStream, chunk: Vec<u8>) -> gitforge_common::Result<()> {
        let label = match stream {
            OutputStream::Stdout => "stdout",
            OutputStream::Stderr => "stderr",
        };
        let text = String::from_utf8_lossy(&chunk);
        for part in utf8_chunks(&text, 60 * 1024) {
            let mut request = self.client.post(&self.endpoint).json(&serde_json::json!({
                "contract_version": "harness.job.v1",
                "runner_id": self.runner_id.to_string(),
                "lease_token": self.lease_token,
                "chunk": format!("[{label}]\n{part}"),
            }));
            if let Some(token) = &self.scheduler_token {
                request = request.bearer_auth(token);
            }
            match request.send().await {
                Ok(response) if response.status().is_success() => {
                    self.sent_chunks
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                }
                Ok(response) => {
                    self.failed_delivery
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(status = %response.status(), "scheduler rejected live log chunk");
                }
                Err(error) => {
                    self.failed_delivery
                        .store(true, std::sync::atomic::Ordering::Relaxed);
                    tracing::warn!(%error, "live log delivery failed");
                }
            }
        }
        Ok(())
    }
}

/// Upload final step output in bounded, UTF-8-safe chunks before completion.
/// The scheduler persists each chunk under the runner lease; this provides a
/// durable tail immediately before the terminal receipt while the sandbox
/// streaming interface is still being expanded.
struct RunnerProtocol<'a> {
    client: &'a Client,
    scheduler_url: &'a str,
    job_id: &'a str,
    runner_id: RunnerId,
    lease_token: &'a str,
    scheduler_token: Option<&'a str>,
}

async fn report_log_chunks(
    protocol: &RunnerProtocol<'_>,
    step_results: &[StepResult],
) -> anyhow::Result<()> {
    let endpoint = format!("{}/jobs/{}/logs", protocol.scheduler_url, protocol.job_id);
    for (index, result) in step_results.iter().enumerate() {
        let mut output = String::new();
        if !result.stdout.is_empty() {
            output.push_str(&format!("[step {index} stdout]\n{}\n", result.stdout));
        }
        if !result.stderr.is_empty() {
            output.push_str(&format!("[step {index} stderr]\n{}\n", result.stderr));
        }
        for chunk in utf8_chunks(&output, 60 * 1024) {
            let mut request = protocol.client.post(&endpoint).json(&serde_json::json!({
                "contract_version": "harness.job.v1",
                "runner_id": protocol.runner_id.to_string(),
                "lease_token": protocol.lease_token,
                "chunk": chunk,
            }));
            if let Some(token) = protocol.scheduler_token {
                request = request.bearer_auth(token);
            }
            let response = request.send().await?;
            if !response.status().is_success() {
                anyhow::bail!("scheduler rejected log append: {}", response.status());
            }
        }
    }
    Ok(())
}

fn utf8_chunks(value: &str, max_bytes: usize) -> Vec<&str> {
    if value.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < value.len() {
        let mut end = (start + max_bytes).min(value.len());
        while end > start && !value.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = value[start..]
                .char_indices()
                .nth(1)
                .map(|(offset, _)| start + offset)
                .unwrap_or(value.len());
        }
        chunks.push(&value[start..end]);
        start = end;
    }
    chunks
}

async fn report_artifacts(
    protocol: &RunnerProtocol<'_>,
    workspace_path: Option<&str>,
    artifacts: &[ArtifactReceipt],
) -> anyhow::Result<Vec<serde_json::Value>> {
    let Some(workspace_path) = workspace_path else {
        return Ok(Vec::new());
    };
    if artifacts.is_empty() {
        return Ok(Vec::new());
    }
    let artifact_root = fs::canonicalize(Path::new(workspace_path).join("artifacts")).await?;
    let endpoint = format!(
        "{}/jobs/{}/artifacts",
        protocol.scheduler_url, protocol.job_id
    );
    let mut uploaded = Vec::with_capacity(artifacts.len());
    for artifact in artifacts {
        let path = artifact_root.join(&artifact.name);
        let canonical = fs::canonicalize(&path).await?;
        if !canonical.starts_with(&artifact_root) {
            anyhow::bail!("artifact path escapes artifact directory");
        }
        let data = fs::read(&canonical).await?;
        let checksum = sha256_hex(&data);
        if checksum != artifact.sha256 {
            anyhow::bail!("artifact checksum changed before upload: {}", artifact.name);
        }
        let mut request = protocol
            .client
            .post(&endpoint)
            .header("x-runner-id", protocol.runner_id.to_string())
            .header("x-lease-token", protocol.lease_token)
            .header("x-artifact-name", &artifact.name)
            .header("x-artifact-sha256", &checksum)
            .body(data);
        if let Some(token) = protocol.scheduler_token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await?;
        if !response.status().is_success() {
            anyhow::bail!("scheduler rejected artifact upload: {}", response.status());
        }
        uploaded.push(response.json::<serde_json::Value>().await?);
    }
    Ok(uploaded)
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_utf8_chunks_preserve_boundaries() {
        let value = "ééé";
        let chunks = utf8_chunks(value, 3);
        assert_eq!(chunks.concat(), value);
        assert!(chunks.iter().all(|chunk| chunk.len() <= 3));
    }

    #[tokio::test]
    async fn test_runner_creation() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[tokio::test]
    async fn test_runner_config_default() {
        let config = RunnerConfig::default();
        assert_eq!(config.name, "runner");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[tokio::test]
    async fn test_runner_register_no_scheduler() {
        // Registration failure is fatal by default: a runner must not appear
        // healthy when it cannot reach the scheduler that assigns it work.
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(), // Invalid URL
            register_attempts: 1, // no retries: keeps this test instantaneous
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        let result = agent.register().await;
        assert!(
            result.is_err(),
            "fail-closed default must reject standalone"
        );
        assert!(agent.runner.is_none());
    }

    #[tokio::test]
    async fn test_runner_register_unreachable_scheduler_allows_standalone() {
        // Legacy standalone fallback remains available behind an explicit
        // policy opt-in.
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            register_attempts: 1, // no retries: keeps this test instantaneous
            allow_standalone: true,
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        let result = agent.register().await;
        assert!(result.is_ok());
        assert!(agent.runner.is_some());
    }

    // ── Registration retry/backoff ──────────────────────────────────────────

    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Reason phrases for the statuses served by [`spawn_status_server`].
    /// reqwest parses the numeric status only, but a well-formed response
    /// needs a phrase.
    fn reason_phrase(status: u16) -> &'static str {
        match status {
            201 => "Created",
            401 => "Unauthorized",
            500 => "Internal Server Error",
            503 => "Service Unavailable",
            _ => "Status",
        }
    }

    /// Serve a scripted sequence of HTTP statuses from a local listener and
    /// count accepted connections. The last status repeats for any further
    /// connections. Every response body is `{"id":"<uuid>"}` so a success
    /// status also exercises runner-id adoption.
    async fn spawn_status_server(statuses: &[u16]) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("test listener binds");
        let addr = listener.local_addr().expect("local addr");
        let connections = Arc::new(AtomicUsize::new(0));
        let counter = connections.clone();
        let statuses = statuses.to_vec();
        assert!(!statuses.is_empty(), "at least one status is required");
        tokio::spawn(async move {
            let mut served = 0usize;
            while let Ok((mut socket, _)) = listener.accept().await {
                let status = statuses[served.min(statuses.len() - 1)];
                served += 1;
                counter.fetch_add(1, Ordering::Relaxed);
                // Drain the request head before answering so the client's
                // write never races our response.
                let mut head = [0u8; 2048];
                let _ = socket.read(&mut head).await;
                let body = format!("{{\"id\":\"{}\"}}", uuid::Uuid::new_v4());
                let response = format!(
                    "HTTP/1.1 {status} {}\r\n\
                     Content-Type: application/json\r\n\
                     Content-Length: {}\r\n\
                     Connection: close\r\n\
                     \r\n{body}",
                    reason_phrase(status),
                    body.len(),
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
        });
        (format!("http://{addr}"), connections)
    }

    #[tokio::test]
    async fn test_registration_retries_unavailable_scheduler_then_succeeds() {
        // The compose race: the scheduler is up but not ready (503) while the
        // runner is already registering. Registration must retry with backoff
        // and adopt the runner id once the scheduler accepts.
        let (url, connections) = spawn_status_server(&[503, 503, 201]).await;
        let config = RunnerConfig {
            scheduler_url: url,
            register_attempts: 5,
            register_backoff_secs: 0,
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        let runner_id = agent
            .register()
            .await
            .expect("registration must outlive 503s");
        assert!(agent.runner.is_some());
        assert_eq!(connections.load(Ordering::Relaxed), 3);
        assert_eq!(agent.runner.as_ref().map(|r| r.id), Some(runner_id));
    }

    #[tokio::test]
    async fn test_registration_auth_rejection_is_not_retried() {
        // A rejected token does not heal by asking again: exactly one attempt
        // is made and the error names the authentication failure.
        let (url, connections) = spawn_status_server(&[401]).await;
        let config = RunnerConfig {
            scheduler_url: url,
            register_attempts: 5,
            register_backoff_secs: 0,
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        let error = agent
            .register()
            .await
            .expect_err("auth rejection must fail closed");
        assert!(
            error.to_string().contains("authentication rejected"),
            "unexpected error: {error}"
        );
        assert_eq!(connections.load(Ordering::Relaxed), 1);
    }

    #[tokio::test]
    async fn test_registration_exhausts_transport_retries() {
        // Nothing listens on the reserved port, so every attempt is a
        // transport error; the surfaced error must report the exhausted
        // attempt count.
        let reserved = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("reserve a port");
        let port = reserved.local_addr().expect("local addr").port();
        drop(reserved);

        let config = RunnerConfig {
            scheduler_url: format!("http://127.0.0.1:{port}"),
            register_attempts: 3,
            register_backoff_secs: 0,
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        let error = agent
            .register()
            .await
            .expect_err("unreachable scheduler must exhaust retries");
        assert!(
            error.to_string().contains("after 3 attempts"),
            "unexpected error: {error}"
        );
        assert!(agent.runner.is_none());
    }

    #[tokio::test]
    async fn test_registration_policy_rejection_falls_back_only_when_allowed() {
        // A 500 is a scheduler refusal, not "not ready yet": it is never
        // retried, it fails closed by default, and standalone fallback
        // requires the explicit opt-in.
        let (url, connections) = spawn_status_server(&[500]).await;

        let deny = RunnerConfig {
            scheduler_url: url.clone(),
            register_attempts: 5,
            register_backoff_secs: 0,
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(deny).await.unwrap();
        let error = agent.register().await.expect_err("500 must fail closed");
        assert!(
            error.to_string().contains("refusing to run standalone"),
            "unexpected error: {error}"
        );
        assert_eq!(connections.load(Ordering::Relaxed), 1);

        let allow = RunnerConfig {
            scheduler_url: url,
            register_attempts: 5,
            register_backoff_secs: 0,
            allow_standalone: true,
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(allow).await.unwrap();
        agent
            .register()
            .await
            .expect("standalone opt-in tolerates a rejected registration");
        assert!(agent.runner.is_some());
        assert_eq!(connections.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn test_next_registration_backoff_doubles_and_caps() {
        assert_eq!(next_registration_backoff(0), 0);
        assert_eq!(next_registration_backoff(1), 2);
        assert_eq!(next_registration_backoff(2), 4);
        assert_eq!(next_registration_backoff(25), 30);
        assert_eq!(next_registration_backoff(30), 30);
        assert_eq!(next_registration_backoff(u64::MAX), 30);
    }

    #[tokio::test]
    async fn test_runner_agent_stop() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        agent.stop(false).await;
        // No panic means success
    }

    #[test]
    fn test_runner_config_custom_values() {
        let config = RunnerConfig {
            scheduler_url: "http://custom:8081".to_string(),
            name: "custom-runner".to_string(),
            runner_type: "kubernetes".to_string(),
            capacity: 5,
            heartbeat_interval_secs: 60,
            fetch_interval_secs: 10,
            scheduler_token: None,
            register_attempts: 3,
            register_backoff_secs: 2,
            allow_standalone: false,
        };
        assert_eq!(config.name, "custom-runner");
        assert_eq!(config.capacity, 5);
        assert_eq!(config.heartbeat_interval_secs, 60);
        assert_eq!(config.fetch_interval_secs, 10);
        assert_eq!(config.register_attempts, 3);
        assert_eq!(config.register_backoff_secs, 2);
    }

    #[test]
    fn test_job_assignment_serialization() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string(), "cargo test".to_string()],
            image: "rust:latest".to_string(),
            working_dir: Some("/workspace".to_string()),
            timeout_secs: 300,
        };

        let json = serde_json::to_string(&assignment).unwrap();
        let deserialized: JobAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.job_id, "job-123");
        assert_eq!(deserialized.commands.len(), 2);
    }

    #[test]
    fn test_legacy_job_assignment_defaults_timeout() {
        let assignment: JobAssignment = serde_json::from_str(
            r#"{
                "job_id":"legacy-job",
                "name":"test",
                "pipeline_run_id":"run-legacy",
                "commands":["true"],
                "image":"rust:latest",
                "working_dir":null
            }"#,
        )
        .unwrap();

        assert_eq!(assignment.timeout_secs, 300);
    }

    #[test]
    fn test_job_assignment_without_working_dir() {
        let assignment = JobAssignment {
            job_id: "job-456".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-789".to_string(),
            commands: vec!["cargo test".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        assert!(assignment.working_dir.is_none());
    }

    #[test]
    fn test_runner_config_debug() {
        let config = RunnerConfig::default();
        let debug_str = format!("{:?}", config);
        assert!(debug_str.contains("runner"));
    }

    #[test]
    fn test_job_assignment_debug() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        let debug_str = format!("{:?}", assignment);
        assert!(debug_str.contains("job-123"));
    }

    #[test]
    fn test_runner_config_clone() {
        let config = RunnerConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.name, config.name);
        assert_eq!(cloned.capacity, config.capacity);
    }

    #[test]
    fn test_runner_config_partial_eq() {
        let config1 = RunnerConfig::default();
        let config2 = RunnerConfig::default();
        assert_eq!(config1.name, config2.name);
        assert_eq!(config1.capacity, config2.capacity);
    }

    #[test]
    fn test_job_assignment_serde_roundtrip() {
        let assignment = JobAssignment {
            job_id: "job-123".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-456".to_string(),
            commands: vec!["cargo build".to_string(), "cargo test".to_string()],
            image: "rust:latest".to_string(),
            working_dir: Some("/workspace".to_string()),
            timeout_secs: 300,
        };

        // Test JSON serialization
        let json = serde_json::to_string(&assignment).unwrap();
        assert!(json.contains("job-123"));
        assert!(json.contains("build"));

        // Test deserialization
        let deserialized: JobAssignment = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.job_id, assignment.job_id);
        assert_eq!(deserialized.name, assignment.name);
        assert_eq!(deserialized.commands, assignment.commands);
        assert_eq!(deserialized.working_dir, assignment.working_dir);
    }

    #[test]
    fn test_job_assignment_empty_commands() {
        let assignment = JobAssignment {
            job_id: "job-empty".to_string(),
            name: "noop".to_string(),
            pipeline_run_id: "run-001".to_string(),
            commands: vec![],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        assert!(assignment.commands.is_empty());
        assert!(assignment.working_dir.is_none());
    }

    #[test]
    fn test_job_assignment_multiple_commands() {
        let commands = vec![
            "cargo fetch".to_string(),
            "cargo build --release".to_string(),
            "cargo test".to_string(),
            "cargo clippy".to_string(),
        ];
        let assignment = JobAssignment {
            job_id: "job-multi".to_string(),
            name: "full-pipeline".to_string(),
            pipeline_run_id: "run-002".to_string(),
            commands,
            image: "rust:latest".to_string(),
            working_dir: Some("/project".to_string()),
            timeout_secs: 300,
        };
        assert_eq!(assignment.commands.len(), 4);
    }

    #[tokio::test]
    async fn test_runner_agent_not_registered() {
        // Agent without registration should have runner as None
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[tokio::test]
    async fn test_runner_register_sets_runner() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            allow_standalone: true,
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();

        let result = agent.register().await;
        assert!(result.is_ok());
        assert!(agent.runner.is_some());

        // Verify runner has correct properties
        let runner = agent.runner.as_ref().unwrap();
        assert_eq!(runner.name, "runner");
        assert_eq!(runner.capacity, 2);
    }

    #[test]
    fn test_runner_config_all_fields() {
        let config = RunnerConfig {
            scheduler_url: "http://example.com:8081".to_string(),
            name: "test-runner".to_string(),
            runner_type: "firecracker".to_string(),
            capacity: 8,
            heartbeat_interval_secs: 15,
            fetch_interval_secs: 3,
            scheduler_token: None,
            register_attempts: 2,
            register_backoff_secs: 1,
            allow_standalone: false,
        };

        assert_eq!(config.scheduler_url, "http://example.com:8081");
        assert_eq!(config.name, "test-runner");
        assert_eq!(config.runner_type, "firecracker");
        assert_eq!(config.capacity, 8);
        assert_eq!(config.heartbeat_interval_secs, 15);
        assert_eq!(config.fetch_interval_secs, 3);
    }

    #[test]
    fn test_runner_config_default_is_docker() {
        let config = RunnerConfig::default();
        assert_eq!(config.runner_type, "docker");
    }

    #[test]
    fn test_runner_config_default_heartbeat() {
        let config = RunnerConfig::default();
        // Default heartbeat is 30 seconds
        assert_eq!(config.heartbeat_interval_secs, 30);
        // Default fetch interval is 5 seconds
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[test]
    fn test_runner_agent_debug() {
        // We can't easily create a running agent for debug test
        // but we can verify the type implements Debug
        let config = RunnerConfig::default();
        assert!(format!("{:?}", config).contains("RunnerConfig"));
    }

    #[test]
    fn test_job_assignment_equality() {
        let assignment1 = JobAssignment {
            job_id: "job-1".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo 1".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        let assignment2 = JobAssignment {
            job_id: "job-1".to_string(),
            name: "build".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo 1".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        // JobAssignment should implement PartialEq if we add it
        // For now just verify individual field equality
        assert_eq!(assignment1.job_id, assignment2.job_id);
        assert_eq!(assignment1.name, assignment2.name);
    }

    #[test]
    fn test_job_assignment_serialize_with_minimal_fields() {
        let assignment = JobAssignment {
            job_id: "minimal-job".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-min".to_string(),
            commands: vec!["true".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };

        let json = serde_json::to_string(&assignment).unwrap();
        assert!(json.contains("minimal-job"));
        assert!(json.contains("test"));
        assert!(json.contains("minimal-job"));
    }

    #[tokio::test]
    async fn test_runner_stop_when_not_running() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        // Stop without running should not panic
        agent.stop(false).await;
    }

    #[tokio::test]
    async fn test_runner_stop_after_registration() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            allow_standalone: true,
            ..Default::default()
        };
        let mut agent = RunnerAgent::new(config).await.unwrap();
        agent.register().await.unwrap();
        // Stop after registration should not panic
        agent.stop(false).await;
    }

    #[test]
    fn test_runner_config_all_default_values() {
        let config = RunnerConfig::default();
        // Verify all default values
        assert_eq!(config.scheduler_url, "http://localhost:42781");
        assert_eq!(config.name, "runner");
        assert_eq!(config.runner_type, "docker");
        assert_eq!(config.capacity, 2);
        assert_eq!(config.heartbeat_interval_secs, 30);
        assert_eq!(config.fetch_interval_secs, 5);
    }

    #[test]
    fn test_runner_config_with_zero_capacity() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:42781".to_string(),
            name: "zero-cap".to_string(),
            runner_type: "docker".to_string(),
            capacity: 0,
            heartbeat_interval_secs: 30,
            fetch_interval_secs: 5,
            scheduler_token: None,
            register_attempts: 2,
            register_backoff_secs: 1,
            allow_standalone: false,
        };
        assert_eq!(config.capacity, 0);
    }

    #[test]
    fn test_job_assignment_with_many_commands() {
        let commands: Vec<String> = (0..100).map(|i| format!("echo step{}", i)).collect();
        let assignment = JobAssignment {
            job_id: "job-many".to_string(),
            name: "many-steps".to_string(),
            pipeline_run_id: "run-many".to_string(),
            commands,
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        assert_eq!(assignment.commands.len(), 100);
    }

    #[test]
    fn test_job_assignment_clone() {
        let assignment = JobAssignment {
            job_id: "clone-test".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo clone".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        let cloned = assignment.clone();
        assert_eq!(cloned.job_id, assignment.job_id);
        assert_eq!(cloned.commands, assignment.commands);
    }

    #[test]
    fn test_job_assignment_with_unicode_in_name() {
        let assignment = JobAssignment {
            job_id: "job-unicode".to_string(),
            name: "测试任务".to_string(),
            pipeline_run_id: "run-unicode".to_string(),
            commands: vec!["echo 测试".to_string()],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        assert_eq!(assignment.name, "测试任务");
    }

    #[test]
    fn test_job_assignment_with_special_chars_in_commands() {
        let assignment = JobAssignment {
            job_id: "special-cmd".to_string(),
            name: "special".to_string(),
            pipeline_run_id: "run-special".to_string(),
            commands: vec![
                "echo $HOME".to_string(),
                "echo \"quoted\"".to_string(),
                "echo 'single'".to_string(),
            ],
            image: "rust:latest".to_string(),
            working_dir: None,
            timeout_secs: 300,
        };
        assert_eq!(assignment.commands.len(), 3);
    }

    #[test]
    fn test_runner_config_with_special_url() {
        let config = RunnerConfig {
            scheduler_url: "http://user:pass@host:9090/path".to_string(),
            name: "special-url-runner".to_string(),
            runner_type: "docker".to_string(),
            capacity: 4,
            heartbeat_interval_secs: 45,
            fetch_interval_secs: 10,
            scheduler_token: None,
            register_attempts: 2,
            register_backoff_secs: 1,
            allow_standalone: false,
        };
        assert!(config.scheduler_url.contains("user:pass"));
    }

    #[test]
    fn test_job_assignment_deserialize() {
        let json = r#"{
            "job_id": "deserialized-job",
            "name": "deserialized",
            "pipeline_run_id": "run-123",
            "commands": ["cargo build", "cargo test"],
            "working_dir": "/workspace"
        }"#;
        let assignment: JobAssignment = serde_json::from_str(json).unwrap();
        assert_eq!(assignment.job_id, "deserialized-job");
        assert_eq!(assignment.commands.len(), 2);
    }

    #[tokio::test]
    async fn test_runner_agent_with_custom_config() {
        let config = RunnerConfig {
            scheduler_url: "http://custom-scheduler:8081".to_string(),
            name: "custom-runner".to_string(),
            runner_type: "kubernetes".to_string(),
            capacity: 10,
            heartbeat_interval_secs: 60,
            fetch_interval_secs: 15,
            scheduler_token: None,
            register_attempts: 2,
            register_backoff_secs: 1,
            allow_standalone: false,
        };
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(agent.runner.is_none());
    }

    #[test]
    fn test_runner_config_clone_is_independent() {
        let config1 = RunnerConfig::default();
        let mut config2 = config1.clone();
        config2.name = "modified".to_string();
        assert_ne!(config1.name, config2.name);
    }

    #[test]
    fn test_job_assignment_with_empty_working_dir() {
        let assignment = JobAssignment {
            job_id: "empty-wd".to_string(),
            name: "test".to_string(),
            pipeline_run_id: "run-1".to_string(),
            commands: vec!["echo test".to_string()],
            image: "rust:latest".to_string(),
            working_dir: Some("".to_string()),
            timeout_secs: 300,
        };
        assert!(assignment.working_dir.is_some());
    }

    #[tokio::test]
    async fn test_runner_is_running() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();
        assert!(!agent.is_running().await);
    }

    #[tokio::test]
    async fn test_runner_run_and_stop() {
        let config = RunnerConfig {
            scheduler_url: "http://localhost:99999".to_string(),
            allow_standalone: true,
            ..Default::default()
        };

        // Create and register agent
        let mut agent = RunnerAgent::new(config.clone()).await.unwrap();
        agent.register().await.unwrap();

        // Clone for use in spawn
        let agent_clone = agent.clone();

        // Start run in background
        let run_handle = tokio::spawn(async move { agent_clone.run().await });

        // Wait for start
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Check is_running
        assert!(agent.is_running().await);

        // Stop it
        agent.stop(false).await;

        // Give it time to shutdown
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // Verify run completed
        let result = run_handle.await.unwrap();
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_runner_run_requires_registration() {
        let config = RunnerConfig::default();
        let agent = RunnerAgent::new(config).await.unwrap();

        // Agent is not registered, run should fail
        let result = agent.run().await;
        assert!(result.is_err());
    }
}
