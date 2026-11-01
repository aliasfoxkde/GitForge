//! Result reuse for incremental CI (#340).
//!
//! A test/job result may be reused only when *all* inputs that can change
//! its outcome are fingerprint-identical to the run that produced the
//! stored result: the source files the job depends on, the job's own
//! definition (command, env, image), and the dependency closure's inputs.
//! Anything the fingerprint cannot prove is treated as a miss — the graph
//! scopes work, it never gates a green verdict on stale evidence.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// What a job depends on, for fingerprinting purposes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobInputs {
    /// Source files (workspace-relative) whose content affects the outcome.
    pub files: Vec<String>,
    /// The job's executed command line.
    pub command: String,
    /// Environment variables the job runs with (sorted map for stability).
    pub env: BTreeMap<String, String>,
    /// Container image the job runs in.
    pub image: String,
}

impl JobInputs {
    /// Stable fingerprint over every input. BTreeMap/Vec keep the encoding
    /// deterministic for identical input sets.
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        let mut files = self.files.clone();
        files.sort();
        for f in &files {
            hasher.update(b"file\0");
            hasher.update(f.as_bytes());
            hasher.update(b"\0");
        }
        hasher.update(b"command\0");
        hasher.update(self.command.as_bytes());
        hasher.update(b"\0");
        for (k, v) in &self.env {
            hasher.update(b"env\0");
            hasher.update(k.as_bytes());
            hasher.update(b"=\0");
            hasher.update(v.as_bytes());
            hasher.update(b"\0");
        }
        hasher.update(b"image\0");
        hasher.update(self.image.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Fingerprint that also mixes in the *content* of each file, so an
    /// edited file with the same path invalidates the result.
    ///
    /// Files that cannot be read are hashed as unreadable rather than
    /// skipped: a missing input must invalidate, never silently pass.
    pub fn content_fingerprint(&self, workspace_root: &Path) -> String {
        let mut hasher = Sha256::new();
        hasher.update(self.fingerprint().as_bytes());
        let mut files = self.files.clone();
        files.sort();
        for f in files {
            hasher.update(b"content\0");
            hasher.update(f.as_bytes());
            hasher.update(b"\0");
            match std::fs::read(workspace_root.join(&f)) {
                Ok(bytes) => {
                    hasher.update(&bytes);
                }
                Err(_) => {
                    hasher.update(b"<unreadable>");
                }
            }
            hasher.update(b"\0");
        }
        hex::encode(hasher.finalize())
    }
}

/// A stored result from a previous run, keyed by job name.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReusableResult {
    pub job_name: String,
    pub fingerprint: String,
    pub commit_sha: String,
    /// Unix millis, produced by the caller's clock — never read here.
    pub recorded_at_ms: i64,
    pub passed: bool,
}

/// Decision on whether a stored result can be reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReuseDecision {
    /// Stored fingerprint matches and the prior run passed — skip.
    Reuse,
    /// Stored fingerprint matches but the prior run failed — re-run.
    PriorFailure,
    /// No stored result for this job.
    NoResult,
    /// Fingerprints differ — inputs changed since the stored run.
    InputsChanged,
    /// The stored result is older than the configured max age (Unix ms).
    Expired,
}

/// Pure decision core: all environment (clock, store contents) is passed in
/// as data so tests never touch the filesystem or the wall clock.
pub fn decide_reuse(
    _job: &str,
    inputs: &JobInputs,
    workspace_root: &Path,
    stored: Option<&ReusableResult>,
    now_ms: i64,
    max_age_ms: i64,
) -> ReuseDecision {
    let Some(stored) = stored else {
        return ReuseDecision::NoResult;
    };
    if now_ms - stored.recorded_at_ms > max_age_ms {
        return ReuseDecision::Expired;
    }
    if stored.fingerprint != inputs.content_fingerprint(workspace_root) {
        return ReuseDecision::InputsChanged;
    }
    if stored.passed {
        ReuseDecision::Reuse
    } else {
        ReuseDecision::PriorFailure
    }
}

/// JSONL-backed result store, one record per line, append-only.
pub struct ResultStore {
    path: PathBuf,
}

impl ResultStore {
    pub fn at_path(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load_all(&self) -> anyhow::Result<Vec<ReusableResult>> {
        let mut results = Vec::new();
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return Ok(results);
        };
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let r: ReusableResult = serde_json::from_str(line)
                .map_err(|e| anyhow::anyhow!("corrupt result record: {e}"))?;
            results.push(r);
        }
        Ok(results)
    }

    /// Latest result for a job name (last record wins).
    pub fn latest(&self, job: &str) -> anyhow::Result<Option<ReusableResult>> {
        let all = self.load_all()?;
        Ok(all.into_iter().rev().find(|r| r.job_name == job))
    }

    pub fn record(&self, result: &ReusableResult) -> anyhow::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| anyhow::anyhow!("create result store dir: {e}"))?;
        }
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| anyhow::anyhow!("open result store: {e}"))?;
        let line =
            serde_json::to_string(result).map_err(|e| anyhow::anyhow!("serialize result: {e}"))?;
        writeln!(f, "{line}").map_err(|e| anyhow::anyhow!("write result: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir for test")
    }

    fn write_file(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        let _ = p.parent().map(std::fs::create_dir_all);
        let mut f = std::fs::File::create(p).expect("create test file");
        f.write_all(content.as_bytes()).expect("write test file");
    }

    #[test]
    fn fingerprint_is_stable_and_order_insensitive() {
        let mut a = JobInputs {
            files: vec!["src/a.rs".into(), "src/b.rs".into()],
            command: "cargo test".into(),
            env: BTreeMap::from([("K".into(), "V".into())]),
            image: "ci:1".into(),
        };
        let mut b = a.clone();
        b.files.reverse();
        assert_eq!(a.fingerprint(), b.fingerprint());
        a.env.insert("K2".into(), "V2".into());
        assert_ne!(a.fingerprint(), b.fingerprint());
    }

    #[test]
    fn content_fingerprint_changes_when_file_content_changes() {
        let dir = tempdir();
        write_file(dir.path(), "src/lib.rs", "fn a() {}");
        let inputs = JobInputs {
            files: vec!["src/lib.rs".into()],
            command: "test".into(),
            env: BTreeMap::new(),
            image: "img".into(),
        };
        let before = inputs.content_fingerprint(dir.path());
        write_file(dir.path(), "src/lib.rs", "fn b() {}");
        let after = inputs.content_fingerprint(dir.path());
        assert_ne!(before, after);
    }

    #[test]
    fn unreadable_file_invalidates_deterministically() {
        let dir = tempdir();
        let inputs = JobInputs {
            files: vec!["missing.rs".into()],
            command: "t".into(),
            env: BTreeMap::new(),
            image: "i".into(),
        };
        let f1 = inputs.content_fingerprint(dir.path());
        let f2 = inputs.content_fingerprint(dir.path());
        assert_eq!(f1, f2);
        let mut with_file = inputs.clone();
        write_file(dir.path(), "missing.rs", "x");
        with_file.files = vec!["missing.rs".into()];
        assert_ne!(f1, with_file.content_fingerprint(dir.path()));
    }

    #[test]
    fn no_result_means_rerun() {
        let dir = tempdir();
        let inputs = JobInputs::default();
        assert_eq!(
            decide_reuse("j", &inputs, dir.path(), None, 1_000, 10_000),
            ReuseDecision::NoResult
        );
    }

    #[test]
    fn matching_passed_result_is_reused() {
        let dir = tempdir();
        write_file(dir.path(), "src/lib.rs", "x");
        let inputs = JobInputs {
            files: vec!["src/lib.rs".into()],
            command: "t".into(),
            env: BTreeMap::new(),
            image: "i".into(),
        };
        let stored = ReusableResult {
            job_name: "j".into(),
            fingerprint: inputs.content_fingerprint(dir.path()),
            commit_sha: "abc".into(),
            recorded_at_ms: 500,
            passed: true,
        };
        assert_eq!(
            decide_reuse("j", &inputs, dir.path(), Some(&stored), 1_000, 10_000),
            ReuseDecision::Reuse
        );
    }

    #[test]
    fn changed_inputs_force_rerun() {
        let dir = tempdir();
        write_file(dir.path(), "src/lib.rs", "x");
        let inputs = JobInputs {
            files: vec!["src/lib.rs".into()],
            command: "t".into(),
            env: BTreeMap::new(),
            image: "i".into(),
        };
        let stored = ReusableResult {
            job_name: "j".into(),
            fingerprint: inputs.content_fingerprint(dir.path()),
            commit_sha: "abc".into(),
            recorded_at_ms: 500,
            passed: true,
        };
        write_file(dir.path(), "src/lib.rs", "y");
        assert_eq!(
            decide_reuse("j", &inputs, dir.path(), Some(&stored), 1_000, 10_000),
            ReuseDecision::InputsChanged
        );
    }

    #[test]
    fn expired_result_is_not_reused_even_if_matching() {
        let dir = tempdir();
        let inputs = JobInputs::default();
        let stored = ReusableResult {
            job_name: "j".into(),
            fingerprint: inputs.content_fingerprint(dir.path()),
            commit_sha: "abc".into(),
            recorded_at_ms: 0,
            passed: true,
        };
        assert_eq!(
            decide_reuse("j", &inputs, dir.path(), Some(&stored), 11_000, 10_000),
            ReuseDecision::Expired
        );
    }

    #[test]
    fn failed_prior_result_reruns() {
        let dir = tempdir();
        let inputs = JobInputs::default();
        let stored = ReusableResult {
            job_name: "j".into(),
            fingerprint: inputs.content_fingerprint(dir.path()),
            commit_sha: "abc".into(),
            recorded_at_ms: 500,
            passed: false,
        };
        assert_eq!(
            decide_reuse("j", &inputs, dir.path(), Some(&stored), 1_000, 10_000),
            ReuseDecision::PriorFailure
        );
    }

    #[test]
    fn store_roundtrips_and_latest_wins() {
        let dir = tempdir();
        let store = ResultStore::at_path(dir.path().join("results.jsonl"));
        let mk = |passed: bool, sha: &str| ReusableResult {
            job_name: "j".into(),
            fingerprint: "f".into(),
            commit_sha: sha.into(),
            recorded_at_ms: 1,
            passed,
        };
        store.record(&mk(true, "one")).expect("record one");
        store.record(&mk(false, "two")).expect("record two");
        let latest = store.latest("j").expect("load latest").expect("present");
        assert_eq!(latest.commit_sha, "two");
        assert!(!latest.passed);
        assert_eq!(store.load_all().expect("load all").len(), 2);
    }

    #[test]
    fn empty_store_loads_clean() {
        let dir = tempdir();
        let store = ResultStore::at_path(dir.path().join("absent").join("r.jsonl"));
        assert!(store.load_all().expect("empty").is_empty());
        assert!(store.latest("x").expect("none").is_none());
    }
}
