//! Runner-local crash-safe completion outbox.
//!
//! Persists exact `job_id` + serde_json completion payload to durable storage.
//! Uses atomic file replacement and is recoverable after a torn write.
//!
//! # Capacity
//! Fixed at 64 entries; oldest entry is evicted (FIFO) when at capacity.

use std::collections::VecDeque;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

/// Maximum number of entries the outbox can hold.
pub const OUTBOX_CAPACITY: usize = 64;

/// Default directory name within `$XDG_STATE_HOME` or `$HOME`.
const DEFAULT_OUTBOX_DIR: &str = ".local/share/gitforce-runner/outbox";

/// Sentinel prefix for the temp file used during atomic replacement.
const TEMP_PREFIX: &str = ".outbox.tmp.";

/// Extension for persisted outbox entry files.
const ENTRY_EXT: &str = "entry";

/// Sentinel suffix appended to indicate an entry is fully written.
#[allow(dead_code)]
const DONE_SUFFIX: &str = ".done";

/// A single outbox entry mapping a job_id to its completion payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboxEntry {
    /// The exact job ID.
    pub job_id: String,
    /// The serialized completion payload.
    pub payload: serde_json::Value,
}

/// The on-disk representation of an [`OutboxEntry`].
#[derive(serde::Serialize, serde::Deserialize)]
struct DiskEntry {
    job_id: String,
    payload: serde_json::Value,
}

/// Crash-safe completion outbox.
///
/// # Storage format
/// Each entry is stored as a single file `<dir>/<job_id>.entry`.  Writes use
/// a temp file (`<dir>/.outbox.tmp.<job_id>.entry`) that is atomically renamed
/// to the final name once fully written.  A `.done` marker file is written
/// after rename to detect torn writes on restart.
///
/// # Capacity
/// Fixed at [`OUTBOX_CAPACITY`] (64).  When at capacity, the oldest entry
/// (by file inode mtime) is evicted before a new one is enqueued.
#[derive(Debug, Clone)]
pub struct CompletionOutbox {
    /// Directory where entries are stored.
    dir: PathBuf,
    /// In-memory FIFO queue ordered by enqueue time (oldest first).
    queue: VecDeque<String>,
}

impl CompletionOutbox {
    /// Open (or create) an outbox at `path`.
    ///
    /// If `path` is empty, resolves a safe per-user default outside `/tmp`:
    /// `$XDG_STATE_HOME/completion-outbox` falling back to
    /// `$HOME/.local/share/gitforce-runner/outbox`.
    pub fn open(path: &str) -> io::Result<Self> {
        let dir = Self::resolve_dir(path)?;
        fs::create_dir_all(&dir)?;

        let mut outbox = Self {
            dir,
            queue: VecDeque::with_capacity(OUTBOX_CAPACITY),
        };
        outbox.reload()?;
        Ok(outbox)
    }

    /// Validate a `job_id` before filesystem access.
    ///
    /// Rejects: empty strings, path separators (`/`, `\`), `..`, and unsafe
    /// filename characters (`< > : " | ? * % @ ! ` { } [ ] # $ ^ & *`).
    fn validate_job_id(job_id: &str) -> io::Result<()> {
        if job_id.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "job_id cannot be empty",
            ));
        }
        let forbidden = ['/', '\\', '\0'];
        if job_id.contains(forbidden) || job_id.contains("..") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("job_id contains disallowed characters or '..': {}", job_id),
            ));
        }
        let unsafe_chars = "< > : \" | ? * % @ ! ` { } [ ] # $ ^ &";
        for c in unsafe_chars.chars() {
            if job_id.contains(c) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("job_id contains unsafe character '{}': {}", c, job_id),
                ));
            }
        }
        Ok(())
    }

    /// Resolve the storage directory, expanding env vars.
    fn resolve_dir(path: &str) -> io::Result<PathBuf> {
        if !path.is_empty() {
            let expanded = expand_envs(path);
            return Ok(PathBuf::from(expanded));
        }

        // Safe per-user default: $XDG_STATE_HOME or $HOME, never /tmp.
        let expanded = expand_envs(
            env::var("XDG_STATE_HOME")
                .ok()
                .filter(|s| !s.is_empty())
                .map(|s| format!("{}/gitforce-runner/outbox", s))
                .unwrap_or_else(|| {
                    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
                    format!(
                        "{}/{}/outbox",
                        home,
                        DEFAULT_OUTBOX_DIR.trim_start_matches("/")
                    )
                })
                .as_str(),
        );
        Ok(PathBuf::from(expanded))
    }

    /// Enqueue a completion entry.
    ///
    /// If the outbox is at capacity, the oldest entry is evicted first.
    /// Returns an error if the entry already exists.
    pub fn enqueue(&mut self, job_id: &str, payload: serde_json::Value) -> io::Result<()> {
        Self::validate_job_id(job_id)?;

        if self.queue.iter().any(|id| id == job_id) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("job {} already in outbox", job_id),
            ));
        }

        // Evict oldest if at capacity.
        if self.queue.len() >= OUTBOX_CAPACITY {
            if let Some(evicted) = self.queue.pop_front() {
                let evicted_path = self.entry_path(&evicted);
                let _ = fs::remove_file(&evicted_path);
                let _ = fs::remove_file(self.done_path(&evicted));
            }
        }

        let disk = DiskEntry {
            job_id: job_id.to_string(),
            payload,
        };

        // Write atomically: temp file → rename → .done marker.
        let tmp_path = self.tmp_path(job_id);
        {
            let file = File::create(&tmp_path)?;
            let mut writer = BufWriter::new(file);
            serde_json::to_writer(&mut writer, &disk)?;
            writer.flush()?;
            // Sync to disk before rename.
            writer.into_inner()?.sync_all()?;
        }
        let final_path = self.entry_path(job_id);
        fs::rename(&tmp_path, &final_path).map_err(|e| {
            // Clean up temp file on failure.
            let _ = fs::remove_file(&tmp_path);
            e
        })?;

        // Write .done marker to detect torn writes on restart.
        File::create(self.done_path(job_id))?.sync_all()?;

        self.queue.push_back(job_id.to_string());
        Ok(())
    }

    /// List all entries currently in the outbox (oldest first).
    pub fn list(&self) -> Vec<OutboxEntry> {
        let mut entries = Vec::with_capacity(self.queue.len());
        for job_id in &self.queue {
            if let Ok(file) = File::open(self.entry_path(job_id)) {
                if let Ok(entry) = serde_json::from_reader::<_, DiskEntry>(file) {
                    entries.push(OutboxEntry {
                        job_id: entry.job_id,
                        payload: entry.payload,
                    });
                }
            }
        }
        entries
    }

    /// Remove a specific entry by `job_id`.
    ///
    /// Returns `Ok` if the entry existed and was removed, or `Ok(false)` if
    /// the entry was not found.
    pub fn remove(&mut self, job_id: &str) -> io::Result<bool> {
        Self::validate_job_id(job_id)?;

        let pos = self.queue.iter().position(|id| id == job_id);
        let idx = match pos {
            Some(idx) => idx,
            None => return Ok(false),
        };

        self.queue.remove(idx);
        let _ = fs::remove_file(self.entry_path(job_id));
        let _ = fs::remove_file(self.done_path(job_id));
        Ok(true)
    }

    /// Reload the outbox from disk, re-reading all entry filenames.
    ///
    /// This is used after a process restart to rebuild the in-memory queue.
    pub fn reload(&mut self) -> io::Result<()> {
        self.queue.clear();

        let read_dir = match fs::read_dir(&self.dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };

        let mut entries: Vec<String> = Vec::new();
        for dir_entry in read_dir.flatten() {
            let path = dir_entry.path();
            // Only consider final entry files, skip temp/done files.
            if path.extension().and_then(|s| s.to_str()) == Some(ENTRY_EXT)
                && !path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.starts_with(TEMP_PREFIX.trim_end_matches('.')))
                    .unwrap_or(false)
            {
                // Verify the .done marker exists so we skip torn writes.
                let job_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();
                if self.done_path(&job_id).exists() {
                    entries.push(job_id);
                } else {
                    // Torn write: remove the orphaned entry file.
                    let _ = fs::remove_file(&path);
                }
            }
        }

        // Sort by mtime (oldest first) for deterministic FIFO ordering.
        entries.sort_by(|a, b| {
            let ma = fs::metadata(self.entry_path(a))
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let mb = fs::metadata(self.entry_path(b))
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            ma.cmp(&mb)
        });

        for job_id in entries {
            self.queue.push_back(job_id);
        }
        Ok(())
    }

    /// Returns the current number of entries in the outbox.
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Returns `true` if the outbox contains no entries.
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Returns the configured capacity.
    pub fn capacity(&self) -> usize {
        OUTBOX_CAPACITY
    }

    fn entry_path(&self, job_id: &str) -> PathBuf {
        self.dir.join(format!("{}.{}", job_id, ENTRY_EXT))
    }

    fn tmp_path(&self, job_id: &str) -> PathBuf {
        self.dir
            .join(format!("{}{}.{}", TEMP_PREFIX, job_id, ENTRY_EXT))
    }

    fn done_path(&self, job_id: &str) -> PathBuf {
        self.entry_path(job_id).with_extension(format!(
            "{}.{}",
            ENTRY_EXT.trim_start_matches('.'),
            "done"
        ))
    }
}

// ─── Environment variable expansion helper ────────────────────────────────────

/// Expand `~` and `$VAR` / `${VAR}` in a path string (basic implementation).
fn expand_envs(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '~' {
            if chars.peek() == Some(&'/') || chars.peek().is_none() {
                // Expand ~ to $HOME (leave as-is if $HOME is not set).
                if let Ok(home) = env::var("HOME") {
                    result.push_str(&home);
                } else {
                    result.push('~');
                }
            } else {
                result.push('~');
            }
        } else if ch == '$' {
            let mut var_name = String::new();
            let has_braces = chars.peek() == Some(&'{');
            if has_braces {
                chars.next();
            }
            while let Some(&c) = chars.peek() {
                if c.is_alphanumeric() || c == '_' {
                    var_name.push(c);
                    chars.next();
                } else if has_braces && c == '}' {
                    chars.next();
                    break;
                } else {
                    break;
                }
            }
            if var_name.is_empty() {
                result.push('$');
            } else if let Ok(val) = env::var(&var_name) {
                result.push_str(&val);
            }
            // If var not set, leave empty (var was empty/unset).
        } else {
            result.push(ch);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Atomic counter for unique test directory names.
    static TEST_COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Unique test directory using process ID + atomic counter.
    /// Cleaned up when the returned `TempDir` is dropped.
    struct TempDir(PathBuf);
    impl TempDir {
        fn new() -> Self {
            let pid = std::process::id();
            let ctr = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
            let base = PathBuf::from(
                env::var("XDG_STATE_HOME")
                    .or_else(|_| env::var("HOME").map(|home| format!("{home}/.local/state")))
                    .expect("HOME or XDG_STATE_HOME must be set for runner tests"),
            )
            .join("gitforce-runner-tests");
            let dir = base.join(format!("test-{:}-{:}", pid, ctr));
            fs::create_dir_all(&dir).expect("failed to create test dir");
            TempDir(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn payload(success: bool, exit_code: i32) -> serde_json::Value {
        serde_json::json!({ "success": success, "exit_code": exit_code })
    }

    // ── open / resolve_dir ─────────────────────────────────────────────────

    #[test]
    fn test_open_creates_dir() {
        let td = TempDir::new();
        let outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        assert!(td.path().exists());
        assert!(outbox.is_empty());
        assert_eq!(outbox.capacity(), OUTBOX_CAPACITY);
    }

    #[test]
    fn test_open_empty_string_uses_default() {
        // Should not panic; uses safe default or skips if env vars absent.
        let outbox = CompletionOutbox::open("").unwrap();
        assert!(outbox.is_empty());
    }

    #[test]
    fn test_open_with_expanded_path() {
        let td = TempDir::new();
        let outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        assert_eq!(outbox.len(), 0);
    }

    // ── enqueue ────────────────────────────────────────────────────────────

    #[test]
    fn test_enqueue_single_entry() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("job-001", payload(true, 0)).unwrap();

        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox.list()[0].job_id, "job-001");
    }

    #[test]
    fn test_enqueue_persists_to_disk() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("job-002", payload(false, 1)).unwrap();

        drop(outbox);

        // Re-open and verify.
        let outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        assert_eq!(outbox.len(), 1);
        let entries = outbox.list();
        assert_eq!(entries[0].job_id, "job-002");
        assert_eq!(entries[0].payload["exit_code"], 1);
    }

    #[test]
    fn test_enqueue_idempotent_is_rejected() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("job-003", payload(true, 0)).unwrap();
        let err = outbox.enqueue("job-003", payload(true, 0)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn test_enqueue_multiple_distinct_entries() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        for i in 0..5 {
            outbox
                .enqueue(&format!("job-{:03}", i), payload(true, 0))
                .unwrap();
        }
        assert_eq!(outbox.len(), 5);
        assert_eq!(outbox.list()[0].job_id, "job-000");
        assert_eq!(outbox.list()[4].job_id, "job-004");
    }

    // ── capacity / eviction ───────────────────────────────────────────────

    #[test]
    fn test_enqueue_evicts_oldest_when_at_capacity() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();

        // Fill to capacity.
        for i in 0..OUTBOX_CAPACITY {
            outbox
                .enqueue(&format!("job-{:03}", i), payload(true, i as i32))
                .unwrap();
        }
        assert_eq!(outbox.len(), OUTBOX_CAPACITY);
        assert!(outbox.list()[0].job_id == "job-000");

        // One more should evict job-000.
        outbox.enqueue("job-overflow", payload(true, 99)).unwrap();
        assert_eq!(outbox.len(), OUTBOX_CAPACITY);
        assert!(!outbox.list().iter().any(|e| e.job_id == "job-000"));
        assert!(outbox.list().iter().any(|e| e.job_id == "job-001"));
    }

    #[test]
    fn test_capacity_constant() {
        assert_eq!(OUTBOX_CAPACITY, 64);
    }

    // ── remove ─────────────────────────────────────────────────────────────

    #[test]
    fn test_remove_existing_entry() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("job-r1", payload(true, 0)).unwrap();
        outbox.enqueue("job-r2", payload(true, 0)).unwrap();

        let removed = outbox.remove("job-r1").unwrap();
        assert!(removed);
        assert_eq!(outbox.len(), 1);
        assert!(outbox.list()[0].job_id == "job-r2");
    }

    #[test]
    fn test_remove_nonexistent_entry() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        let removed = outbox.remove("nonexistent").unwrap();
        assert!(!removed);
    }

    #[test]
    fn test_remove_then_enqueue_same_id() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("job-rt1", payload(true, 0)).unwrap();
        outbox.remove("job-rt1").unwrap();

        outbox.enqueue("job-rt1", payload(false, 1)).unwrap();
        assert_eq!(outbox.len(), 1);
        assert_eq!(outbox.list()[0].payload["exit_code"], 1);
    }

    // ── list ───────────────────────────────────────────────────────────────

    #[test]
    fn test_list_returns_all_entries() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("a", payload(true, 10)).unwrap();
        outbox.enqueue("b", payload(false, 20)).unwrap();

        let entries = outbox.list();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].job_id, "a");
        assert_eq!(entries[1].job_id, "b");
        assert_eq!(entries[1].payload["exit_code"], 20);
    }

    #[test]
    fn test_list_empty() {
        let td = TempDir::new();
        let outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        assert!(outbox.list().is_empty());
    }

    // ── reload ─────────────────────────────────────────────────────────────

    #[test]
    fn test_reload_preserves_queue_after_reopen() {
        let td = TempDir::new();
        let path = td.path().to_str().unwrap().to_string();

        let mut outbox = CompletionOutbox::open(&path).unwrap();
        for i in 0..3 {
            outbox
                .enqueue(&format!("reload-{:02}", i), payload(true, i as i32))
                .unwrap();
        }
        drop(outbox);

        let mut reloaded = CompletionOutbox::open(&path).unwrap();
        reloaded.reload().unwrap();
        assert_eq!(reloaded.len(), 3);
        assert_eq!(reloaded.list()[0].job_id, "reload-00");
    }

    #[test]
    fn test_reload_skips_torn_entry_without_done_marker() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("torn-job", payload(true, 0)).unwrap();
        let done_path = outbox.done_path("torn-job");
        drop(outbox);

        // Remove the .done marker to simulate a torn write.
        fs::remove_file(&done_path).unwrap();

        let mut reloaded = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        reloaded.reload().unwrap();
        assert!(reloaded.list().is_empty());
    }

    #[test]
    fn test_reload_idempotent() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("idem", payload(true, 0)).unwrap();
        outbox.reload().unwrap();
        outbox.reload().unwrap();
        assert_eq!(outbox.len(), 1);
    }

    // ── len / is_empty ────────────────────────────────────────────────────

    #[test]
    fn test_len_and_is_empty() {
        let td = TempDir::new();
        let outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        assert!(outbox.is_empty());
        assert_eq!(outbox.len(), 0);

        drop(outbox);
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("len-test", payload(true, 0)).unwrap();
        assert!(!outbox.is_empty());
        assert_eq!(outbox.len(), 1);
    }

    // ── entry path helpers ─────────────────────────────────────────────────

    #[test]
    fn test_entry_path_format() {
        let td = TempDir::new();
        let outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        let p = outbox.entry_path("abc-123");
        assert!(p.to_str().unwrap().ends_with("abc-123.entry"));
    }

    #[test]
    fn test_done_path_format() {
        let td = TempDir::new();
        let outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        let p = outbox.done_path("abc-123");
        assert!(p.to_str().unwrap().ends_with("abc-123.entry.done"));
    }

    // ── job_id validation ─────────────────────────────────────────────────

    #[test]
    fn test_validate_job_id_rejects_empty() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        let err = outbox.enqueue("", payload(true, 0)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_validate_job_id_rejects_path_separator() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        let err = outbox.enqueue("job/001", payload(true, 0)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_validate_job_id_rejects_backslash() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        let err = outbox.enqueue("job\\001", payload(true, 0)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_validate_job_id_rejects_parent_traversal() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        let err = outbox.enqueue("job..sneaky", payload(true, 0)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn test_validate_job_id_rejects_unsafe_chars() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        for c in [
            '<', '>', ':', '"', '|', '?', '*', '%', '@', '!', '`', '{', '}', '[', ']', '#', '$',
            '^', '&',
        ] {
            let job_id = format!("job{}001", c);
            let err = outbox.enqueue(&job_id, payload(true, 0)).unwrap_err();
            assert_eq!(
                err.kind(),
                io::ErrorKind::InvalidInput,
                "char {:?} should be rejected",
                c
            );
        }
    }

    #[test]
    fn test_validate_job_id_rejects_on_remove() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("valid-job", payload(true, 0)).unwrap();
        let err = outbox.remove("../etc/passwd").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    // ── expand_envs ────────────────────────────────────────────────────────

    #[test]
    fn test_expand_envs_dollar_var() {
        env::set_var("TEST_OUTBOX_VAR", "my-value");
        let expanded = expand_envs("/tmp/$TEST_OUTBOX_VAR/path");
        assert_eq!(expanded, "/tmp/my-value/path");
        env::remove_var("TEST_OUTBOX_VAR");
    }

    #[test]
    fn test_expand_envs_braced_var() {
        env::set_var("TEST_OUTBOX_BRC", "bracketed");
        let expanded = expand_envs("/tmp/${TEST_OUTBOX_BRC}/extra");
        assert_eq!(expanded, "/tmp/bracketed/extra");
        env::remove_var("TEST_OUTBOX_BRC");
    }

    #[test]
    fn test_expand_envs_tilde() {
        let expanded = expand_envs("~/outbox");
        let home = env::var("HOME").unwrap();
        assert_eq!(expanded, format!("{home}/outbox"));
    }

    #[test]
    fn test_expand_envs_noop() {
        let s = "/already/absolute/path";
        assert_eq!(expand_envs(s), s);
    }

    #[test]
    fn test_expand_envs_unknown_var_empty() {
        // Unknown vars expand to empty string.
        let expanded = expand_envs("/tmp/$UNKNOWN_VAR_XXXX/test");
        assert_eq!(expanded, "/tmp//test"); // $UNKNOWN... is empty
    }

    // ── atomic replacement ─────────────────────────────────────────────────

    #[test]
    fn test_entry_file_exists_after_enqueue() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("atomic-test", payload(true, 0)).unwrap();

        let entry_file = td.path().join("atomic-test.entry");
        assert!(entry_file.exists());

        let done_file = td.path().join("atomic-test.entry.done");
        assert!(done_file.exists());
    }

    #[test]
    fn test_temp_file_not_present_after_enqueue() {
        let td = TempDir::new();
        let mut outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        outbox.enqueue("no-temp", payload(true, 0)).unwrap();

        let temp_file = td.path().join(".outbox.tmp.no-temp.entry");
        assert!(!temp_file.exists());
    }

    // ── Debug ──────────────────────────────────────────────────────────────

    #[test]
    fn test_outbox_debug() {
        let td = TempDir::new();
        let outbox = CompletionOutbox::open(td.path().to_str().unwrap()).unwrap();
        let debug_str = format!("{:?}", outbox);
        assert!(debug_str.contains("CompletionOutbox"));
    }

    #[test]
    fn test_outbox_entry_debug() {
        let entry = OutboxEntry {
            job_id: "j-1".to_string(),
            payload: payload(true, 0),
        };
        let debug_str = format!("{:?}", entry);
        assert!(debug_str.contains("j-1"));
    }
}
