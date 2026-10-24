//! Filesystem fixtures.
//!
//! All fixtures own their storage and delete it on drop, so a test that
//! panics mid-scenario still cleans up. Temp roots are created under the
//! system temp dir (`std::env::temp_dir()`), which honors `TMPDIR` — tests
//! never hardcode `/tmp` (the 8G tmpfs cap rule).

use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A private directory that is deleted when the last handle drops.
///
/// Clone it to share one root across helper calls; the dir dies with the
/// last clone, and [`TempDir::path`] is the only way in.
#[derive(Debug, Clone)]
pub struct TempDir {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    path: PathBuf,
}

impl TempDir {
    /// Create a unique directory. Panics if the OS refuses — a fixture
    /// that cannot get a temp dir means the test environment is broken
    /// (full disk, hostile TMPDIR) and the test cannot run meaningfully.
    pub fn new() -> Self {
        let dir = tempfile::Builder::new()
            .tempdir()
            .unwrap_or_else(|e| panic!("fixture: cannot create temp dir: {e}"));
        Self {
            inner: Arc::new(Inner {
                // Persist the path past tempfile::TempDir's own Drop (which
                // would delete immediately) and take over deletion in our
                // Drop so the dir survives clones of the handle.
                path: dir.keep(),
            }),
        }
    }

    /// The fixture root inside the test filesystem.
    pub fn path(&self) -> &Path {
        &self.inner.path
    }

    /// Write `contents` to `rel` under the root, creating parent dirs.
    pub fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let target = self.inner.path.join(rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .unwrap_or_else(|e| panic!("fixture: mkdir {}: {e}", parent.display()));
        }
        std::fs::write(&target, contents)
            .unwrap_or_else(|e| panic!("fixture: write {}: {e}", target.display()));
        target
    }

    /// Read back a file written earlier.
    pub fn read(&self, rel: &str) -> String {
        let target = self.inner.path.join(rel);
        std::fs::read_to_string(&target)
            .unwrap_or_else(|e| panic!("fixture: read {}: {e}", target.display()))
    }
}

impl Default for TempDir {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Best-effort cleanup; a leaked temp dir on an error path is the
        // system temp cleaner's problem, not a test failure.
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_and_read_roundtrip() {
        let dir = TempDir::new();
        dir.write("a/b.txt", "hello");
        assert_eq!(dir.read("a/b.txt"), "hello");
    }

    #[test]
    fn clones_share_one_root() {
        let dir = TempDir::new();
        let clone = dir.clone();
        let path = dir.path().to_owned();
        clone.write("x", "1");
        assert_eq!(dir.read("x"), "1");
        assert!(path.is_dir());
        drop(clone);
        assert!(path.is_dir(), "root must outlive non-final handles");
        drop(dir);
        assert!(!path.exists(), "root dies with the last handle");
    }
}
