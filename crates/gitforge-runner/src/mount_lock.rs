//! Serializing the host directories a job mounts.
//!
//! Two pipeline runs that mount the same host directory compile into one
//! build tree. A rebuild in one run replaces the binaries another run is
//! about to execute, and that run dies with ENOENT on the missing binary
//! (GF-11, measured 2026-11-05 on the DIL repository, where two coverage
//! jobs lost their test binaries to a concurrent rebuild).
//!
//! The runner takes an exclusive advisory lock on the host directory
//! itself and holds it for the job's lifetime. Locking the directory
//! rather than a lock file beside it keeps one source of truth: the runner
//! locks exactly the path it hands to the sandbox, there is no file for the
//! job's container to delete (the container mounts the directory, not a
//! sibling of it), and no second path convention to keep in step with the
//! mount.
//!
//! The lock is advisory and lives on the host. Only the runner creates
//! containers, so every writer of a shared host directory passes through
//! it.
//!
//! A mount the runner cannot lock is not mounted: the job runs without it
//! and rebuilds cold, rather than sharing a directory with a job that is
//! writing to it. The sandbox already degrades a mount it cannot create
//! the same way, so the runner's behaviour matches the mount contract.

use std::fmt;
use std::fs::{self, File};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

use gitforge_common::{VolumeSpec, VolumeType};
use tokio::time::Instant;

/// How long to wait between attempts to take a busy directory.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Why a shared host directory could not be locked.
#[derive(Debug)]
pub enum MountLockError {
    /// The directory could not be created, opened, or locked.
    Unavailable { path: PathBuf, error: io::Error },
    /// Another job holds the directory and did not release it in time.
    Busy { path: PathBuf, waited: Duration },
}

impl fmt::Display for MountLockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable { path, error } => {
                write!(formatter, "{} is not lockable: {error}", path.display())
            }
            Self::Busy { path, waited } => write!(
                formatter,
                "{} stayed busy for {}s",
                path.display(),
                waited.as_secs()
            ),
        }
    }
}

impl std::error::Error for MountLockError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unavailable { error, .. } => Some(error),
            Self::Busy { .. } => None,
        }
    }
}

/// An exclusive advisory lock on a host directory.
///
/// The lock is released when the value is dropped.
#[derive(Debug)]
pub struct MountLock {
    directory: File,
    path: PathBuf,
}

impl MountLock {
    /// Take the lock, waiting up to `wait` for a holder to release it.
    pub async fn acquire(path: &Path, wait: Duration) -> Result<Self, MountLockError> {
        if let Err(error) = fs::create_dir_all(path) {
            return Err(MountLockError::Unavailable {
                path: path.to_path_buf(),
                error,
            });
        }
        let directory = File::open(path).map_err(|error| MountLockError::Unavailable {
            path: path.to_path_buf(),
            error,
        })?;
        let deadline = Instant::now() + wait;
        loop {
            match try_lock(&directory) {
                Ok(true) => {
                    return Ok(Self {
                        directory,
                        path: path.to_path_buf(),
                    })
                }
                Ok(false) => {}
                Err(error) => {
                    return Err(MountLockError::Unavailable {
                        path: path.to_path_buf(),
                        error,
                    })
                }
            }
            if Instant::now() >= deadline {
                return Err(MountLockError::Busy {
                    path: path.to_path_buf(),
                    waited: wait,
                });
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The host directory this lock guards.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for MountLock {
    fn drop(&mut self) {
        // Closing the descriptor releases the lock too; unlocking states
        // the intent and releases it a little earlier.
        unsafe {
            libc::flock(self.directory.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

/// Try to take the lock without waiting.
///
/// A second open of the same directory is a different open file
/// description, so the lock conflicts with a holder in the same process
/// too - which is what makes the runner's own jobs exclude each other.
fn try_lock(directory: &File) -> io::Result<bool> {
    // SAFETY: the descriptor is open and owned for the duration of the call.
    let result = unsafe { libc::flock(directory.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
        return Ok(false);
    }
    Err(error)
}

/// The host directories a job shares with other jobs.
///
/// Only writable bind mounts of a host path are shared: a read-only mount
/// cannot be written to, a tmpfs lives inside the container, and a named
/// volume is managed by the container daemon rather than by a host path
/// the runner can lock.
pub fn shared_sources(image: &str, volumes: &[VolumeSpec], cache_root: &str) -> Vec<PathBuf> {
    let mut sources: Vec<PathBuf> = volumes
        .iter()
        .filter(|volume| !volume.read_only && volume.resolved_type() == VolumeType::Bind)
        .filter_map(|volume| volume.expanded_source(image, cache_root))
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .collect();
    sources.sort();
    sources.dedup();
    sources
}

/// Lock every shared host directory the job mounts.
///
/// Returns the locks to hold for the job's lifetime and the volumes the
/// job's container may mount.
///
/// A directory that stays busy for the whole wait is a conflict the runner
/// cannot resolve: the job is refused rather than run against a directory
/// another job is writing to. A directory the runner cannot open at all is
/// dropped from the job's mounts, so the job rebuilds cold instead of
/// sharing a directory it cannot protect.
pub async fn lock_job_mounts(
    image: &str,
    volumes: &[VolumeSpec],
    cache_root: &str,
    wait: Duration,
) -> Result<(Vec<MountLock>, Vec<VolumeSpec>), MountLockError> {
    let deadline = Instant::now() + wait;
    let mut locks = Vec::new();
    let mut refused: Vec<PathBuf> = Vec::new();
    for source in shared_sources(image, volumes, cache_root) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match MountLock::acquire(&source, remaining).await {
            Ok(lock) => locks.push(lock),
            Err(error @ MountLockError::Unavailable { .. }) => {
                tracing::warn!(
                    path = %source.display(),
                    %error,
                    "shared mount is not lockable; the job runs without it"
                );
                refused.push(source);
            }
            Err(error @ MountLockError::Busy { .. }) => return Err(error),
        }
    }
    let volumes = volumes
        .iter()
        .filter(|volume| {
            volume
                .expanded_source(image, cache_root)
                .is_none_or(|source| !refused.contains(&PathBuf::from(source)))
        })
        .cloned()
        .collect();
    Ok((locks, volumes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn bind(source: &str) -> VolumeSpec {
        VolumeSpec {
            volume_type: Some(VolumeType::Bind),
            source: Some(source.to_string()),
            destination: "/cargo-target".to_string(),
            read_only: false,
            size: None,
        }
    }

    #[tokio::test]
    async fn a_second_holder_waits_for_the_first() {
        let root = tempfile::tempdir().expect("a scratch directory");
        let shared = root.path().join("shared");
        let first = MountLock::acquire(&shared, Duration::from_secs(1))
            .await
            .expect("the first holder takes the lock");
        assert_eq!(first.path(), shared);

        let second = MountLock::acquire(&shared, Duration::from_millis(200)).await;
        assert!(
            matches!(second, Err(MountLockError::Busy { .. })),
            "a second holder must be refused, got {second:?}"
        );
    }

    #[tokio::test]
    async fn the_lock_is_released_when_the_holder_drops_it() {
        let root = tempfile::tempdir().expect("a scratch directory");
        let shared = root.path().join("shared");
        let first = MountLock::acquire(&shared, Duration::from_secs(1))
            .await
            .expect("the first holder takes the lock");
        drop(first);
        let second = MountLock::acquire(&shared, Duration::from_secs(1))
            .await
            .expect("the lock is free once the holder drops it");
        assert_eq!(second.path(), shared);
    }

    #[tokio::test]
    async fn different_directories_do_not_block_each_other() {
        let root = tempfile::tempdir().expect("a scratch directory");
        let one = root.path().join("one");
        let two = root.path().join("two");
        let held = MountLock::acquire(&one, Duration::from_secs(1))
            .await
            .expect("the first holder takes the lock");
        let other = MountLock::acquire(&two, Duration::from_millis(200)).await;
        assert!(other.is_ok(), "a different directory must not be refused");
        drop(held);
    }

    #[test]
    fn only_writable_bind_mounts_are_shared() {
        let root = tempfile::tempdir().expect("a scratch directory");
        let writable = root.path().join("writable");
        let read_only = root.path().join("read-only");
        let mut read_only_spec = bind(read_only.to_str().expect("a utf-8 path"));
        read_only_spec.read_only = true;
        let tmpfs = VolumeSpec {
            volume_type: Some(VolumeType::Tmpfs),
            source: None,
            destination: "/scratch".to_string(),
            read_only: false,
            size: None,
        };
        let relative = bind("relative/path");

        let sources = shared_sources(
            "backend-ci-rust:1",
            &[
                bind(writable.to_str().expect("a utf-8 path")),
                read_only_spec,
                tmpfs,
                relative,
            ],
            "/var/cache/gitforge",
        );
        assert_eq!(sources, vec![writable]);
    }

    #[tokio::test]
    async fn a_busy_mount_refuses_the_job() {
        let root = tempfile::tempdir().expect("a scratch directory");
        let shared = root.path().join("shared");
        let held = MountLock::acquire(&shared, Duration::from_secs(1))
            .await
            .expect("the first holder takes the lock");
        let outcome = lock_job_mounts(
            "backend-ci-rust:1",
            &[bind(shared.to_str().expect("a utf-8 path"))],
            "/var/cache/gitforge",
            Duration::from_millis(200),
        )
        .await;
        assert!(
            matches!(outcome, Err(MountLockError::Busy { .. })),
            "a busy mount must refuse the job, got {outcome:?}"
        );
        drop(held);
    }

    #[tokio::test]
    async fn a_mount_that_cannot_be_locked_is_not_mounted() {
        let root = tempfile::tempdir().expect("a scratch directory");
        // A regular file cannot be a parent directory, so the mount source
        // can neither be created nor opened.
        let blocker = root.path().join("blocker");
        fs::write(&blocker, b"not a directory").expect("a scratch file");
        let unusable = blocker.join("cache");
        let volume = bind(unusable.to_str().expect("a utf-8 path"));

        let (locks, volumes) = lock_job_mounts(
            "backend-ci-rust:1",
            &[volume],
            "/var/cache/gitforge",
            Duration::from_millis(200),
        )
        .await
        .expect("an unusable mount does not fail the job");
        assert!(locks.is_empty(), "an unusable mount takes no lock");
        assert!(
            volumes.is_empty(),
            "an unusable mount must not be handed to the sandbox"
        );
    }
}
