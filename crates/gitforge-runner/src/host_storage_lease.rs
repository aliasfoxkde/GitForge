#![allow(dead_code)]
//! Cross-process serialization for storage-heavy host jobs.
//!
//! The lease file is provisioned by systemd-tmpfiles at
//! `/run/lock/codex-host-storage.lock`, owned by `root:www-data` and mode
//! `0660`. Both the harness manager and the GitForge runner run as members of
//! `www-data`. Missing or inaccessible lease files fail closed; do not create
//! the file from an unprivileged process because its owner and group would be
//! wrong for the other launcher.
#[allow(dead_code)]
#[allow(clippy::unused_unit)]
use std::fs::{File, OpenOptions};
use std::future::Future;
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};

#[allow(dead_code)]
const DEFAULT_LEASE_PATH: &str = "/run/lock/codex-host-storage.lock";
#[allow(dead_code)]
const LEASE_PATH_ENV: &str = "HOST_STORAGE_LEASE_PATH";
#[allow(dead_code)]
const IO_PRESSURE_PATH: &str = "/proc/pressure/io";
#[allow(dead_code)]
const CPU_PRESSURE_PATH: &str = "/proc/pressure/cpu";
#[allow(dead_code)]
const PROC_ROOT: &str = "/proc";
#[allow(dead_code)]
const IO_LIMIT_ENV: &str = "GITFORGE_IO_FULL_AVG10_LIMIT_PCT";
#[allow(dead_code)]
const CPU_LIMIT_ENV: &str = "GITFORGE_CPU_SOME_AVG10_LIMIT_PCT";
#[allow(dead_code)]
const DEFAULT_IO_LIMIT_PCT: f64 = 25.0;
#[allow(dead_code)]
const DEFAULT_CPU_LIMIT_PCT: f64 = 80.0;
#[allow(dead_code)]
const BUILD_PROCESS_NAMES: &[&str] = &[
    "cargo",
    "cargo-clippy",
    "rustc",
    "clippy-driver",
    "rust-lld",
    "ld.lld",
    "trunk",
    "wasm-bindgen",
    "wasm-opt",
    "xtask",
];

/// Reserve local storage capacity before asking the scheduler to claim a job.
/// `Ok(None)` means the reservation was busy; callers must defer the scheduler
/// assignment before retrying later. On success, the returned file keeps the
/// lock alive for the caller's job lifetime.
pub(crate) async fn reserve_before_claim<T, Reserve, Claim, ClaimFuture>(
    required: bool,
    reserve: Reserve,
    claim: Claim,
) -> io::Result<Option<(Option<T>, Option<File>)>>
where
    Reserve: FnOnce() -> io::Result<Option<File>>,
    Claim: FnOnce() -> ClaimFuture,
    ClaimFuture: Future<Output = Option<T>>,
{
    let lease = if required {
        let Some(lease) = reserve()? else {
            return Ok(None);
        };
        Some(lease)
    } else {
        None
    };

    // Keep the outer Option for local admission (None means busy), and keep
    // the claim's Option inside it (None means the scheduler did not accept
    // this assignment). Conflating these states would defer stale or failed
    // claims as if the local host lease were busy.
    Ok(Some((claim().await, lease)))
}

/// Acquire local storage/build admission without waiting. `None` means the
/// lease is busy, host pressure is over its limit, or the process probe found
/// an active build; errors mean the lease contract is unavailable.
pub(crate) fn try_acquire() -> io::Result<Option<File>> {
    let path = std::env::var_os(LEASE_PATH_ENV)
        .map_or_else(|| PathBuf::from(DEFAULT_LEASE_PATH), PathBuf::from);
    try_acquire_at_with(&path, local_host_admission_block_reason)
}

fn try_acquire_at(path: &Path) -> io::Result<Option<File>> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    // SAFETY: `file` is a live descriptor. Its ownership is retained in the
    // returned `File`, so the kernel lock remains held until that value drops.
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        return Ok(Some(file));
    }
    let error = io::Error::last_os_error();
    if error.kind() == io::ErrorKind::WouldBlock {
        Ok(None)
    } else {
        Err(error)
    }
}

fn try_acquire_at_with(
    path: &Path,
    admission_check: impl FnOnce() -> Option<&'static str>,
) -> io::Result<Option<File>> {
    let Some(lease) = try_acquire_at(path)? else {
        return Ok(None);
    };
    if let Some(reason) = admission_check() {
        tracing::debug!(
            reason,
            "runner local admission deferred storage-sensitive work"
        );
        // Dropping the file releases flock before the scheduler assignment is
        // deferred by the caller.
        drop(lease);
        Ok(None)
    } else {
        Ok(Some(lease))
    }
}

fn local_host_admission_block_reason() -> Option<&'static str> {
    let io_pressure = std::fs::read_to_string(IO_PRESSURE_PATH).ok();
    let cpu_pressure = std::fs::read_to_string(CPU_PRESSURE_PATH).ok();
    let active_build = active_build_processes(Path::new(PROC_ROOT));
    let io_limit = configured_limit(IO_LIMIT_ENV, DEFAULT_IO_LIMIT_PCT);
    let cpu_limit = configured_limit(CPU_LIMIT_ENV, DEFAULT_CPU_LIMIT_PCT);
    local_admission_block_reason(
        io_pressure.as_deref(),
        cpu_pressure.as_deref(),
        active_build,
        io_limit,
        cpu_limit,
    )
}

#[cfg(test)]
fn local_admission_allows(
    io_pressure: Option<&str>,
    cpu_pressure: Option<&str>,
    active_build: Option<bool>,
    io_limit: Option<f64>,
    cpu_limit: Option<f64>,
) -> bool {
    local_admission_block_reason(io_pressure, cpu_pressure, active_build, io_limit, cpu_limit)
        .is_none()
}

fn local_admission_block_reason(
    io_pressure: Option<&str>,
    cpu_pressure: Option<&str>,
    active_build: Option<bool>,
    io_limit: Option<f64>,
    cpu_limit: Option<f64>,
) -> Option<&'static str> {
    let Some(io) = io_pressure.and_then(|text| parse_avg10(text, "full")) else {
        return Some("io_pressure_unavailable");
    };
    let Some(cpu) = cpu_pressure.and_then(|text| parse_avg10(text, "some")) else {
        return Some("cpu_pressure_unavailable");
    };
    let Some(io_limit) = io_limit else {
        return Some("io_pressure_limit_invalid");
    };
    let Some(cpu_limit) = cpu_limit else {
        return Some("cpu_pressure_limit_invalid");
    };
    if io > io_limit {
        return Some("io_pressure_high");
    }
    if cpu > cpu_limit {
        return Some("cpu_pressure_high");
    }
    match active_build {
        Some(true) => Some("active_build_process"),
        Some(false) => None,
        None => Some("build_process_probe_unavailable"),
    }
}

fn parse_avg10(contents: &str, metric: &str) -> Option<f64> {
    let prefix = format!("{metric} ");
    let raw = contents
        .lines()
        .find(|line| line.starts_with(&prefix))?
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))?;
    let value = raw.parse::<f64>().ok()?;
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

fn configured_limit(name: &str, default: f64) -> Option<f64> {
    let value = match std::env::var(name) {
        Ok(raw) => raw.trim().parse::<f64>().ok()?,
        Err(std::env::VarError::NotPresent) => default,
        Err(std::env::VarError::NotUnicode(_)) => return None,
    };
    (value.is_finite() && (0.0..=100.0).contains(&value)).then_some(value)
}

fn active_build_processes(proc_root: &Path) -> Option<bool> {
    for entry in std::fs::read_dir(proc_root).ok()? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => return None,
        };
        if entry.file_name().to_str()?.parse::<u32>().is_err() {
            continue;
        }
        let process_dir = entry.path();
        let comm = match std::fs::read_to_string(process_dir.join("comm")) {
            Ok(comm) => comm,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        if !BUILD_PROCESS_NAMES.contains(&comm.trim()) {
            continue;
        }
        let status = match std::fs::read_to_string(process_dir.join("status")) {
            Ok(status) => status,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return None,
        };
        let state = status.lines().find_map(|line| {
            line.strip_prefix("State:")
                .and_then(|value| value.split_whitespace().next())
        })?;
        if state != "Z" {
            return Some(true);
        }
    }
    Some(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn busy_host_lease_does_not_claim_scheduler_assignment() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host-storage.lock");
        File::create(&path).unwrap();
        let held = try_acquire_at(&path).unwrap().expect("lease holder");
        let claim_called = Arc::new(AtomicBool::new(false));
        let claim_called_in_future = claim_called.clone();

        let result = reserve_before_claim(
            true,
            || try_acquire_at(&path),
            || async move {
                claim_called_in_future.store(true, Ordering::SeqCst);
                Some("claimed")
            },
        )
        .await
        .unwrap();

        assert!(result.is_none());
        assert!(!claim_called.load(Ordering::SeqCst));
        drop(held);
    }

    #[tokio::test]
    async fn successful_host_lease_is_held_with_claim_result() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host-storage.lock");
        File::create(&path).unwrap();

        let result =
            reserve_before_claim(true, || try_acquire_at(&path), || async { Some("claimed") })
                .await
                .unwrap()
                .expect("claim result");

        assert_eq!(result.0, Some("claimed"));
        assert!(try_acquire_at(&path).unwrap().is_none());
        drop(result);
        assert!(try_acquire_at(&path).unwrap().is_some());
    }

    #[tokio::test]
    async fn failed_scheduler_claim_is_distinct_from_busy_host_lease() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host-storage.lock");
        File::create(&path).unwrap();

        let result = reserve_before_claim(
            true,
            || try_acquire_at(&path),
            || async { None::<&'static str> },
        )
        .await
        .unwrap()
        .expect("the local lease was acquired even though the claim failed");

        assert_eq!(result.0, None);
        assert!(result.1.is_some());
        assert!(try_acquire_at(&path).unwrap().is_none());
        drop(result);
        assert!(try_acquire_at(&path).unwrap().is_some());
    }

    #[tokio::test]
    async fn missing_host_lease_fails_closed_without_claiming() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("missing.lock");
        let claim_called = Arc::new(AtomicBool::new(false));
        let claim_called_in_future = claim_called.clone();

        let result = reserve_before_claim(
            true,
            || try_acquire_at(&path),
            || async move {
                claim_called_in_future.store(true, Ordering::SeqCst);
                Some("claimed")
            },
        )
        .await;

        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::NotFound);
        assert!(!claim_called.load(Ordering::SeqCst));
    }

    #[test]
    fn lease_is_exclusive_and_released_when_file_closes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host-storage.lock");
        File::create(&path).unwrap();

        let first = try_acquire_at(&path).unwrap().expect("first lease");
        assert!(try_acquire_at(&path).unwrap().is_none());
        drop(first);
        assert!(try_acquire_at(&path).unwrap().is_some());
    }

    #[test]
    fn missing_lease_file_fails_closed() {
        let directory = tempfile::tempdir().unwrap();
        let error = try_acquire_at(&directory.path().join("missing.lock")).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn pressure_gate_requires_low_io_cpu_and_no_active_build() {
        let io = "some avg10=12.0 avg60=10 avg300=8 total=10\nfull avg10=12.0 avg60=10 avg300=8 total=10\n";
        let cpu =
            "some avg10=30.0 avg60=10 avg300=8 total=10\nfull avg10=0 avg60=0 avg300=0 total=0\n";
        assert_eq!(
            local_admission_block_reason(Some(io), Some(cpu), Some(false), Some(25.0), Some(80.0)),
            None
        );
        assert!(!local_admission_allows(
            Some(io),
            Some(cpu),
            Some(true),
            Some(25.0),
            Some(80.0)
        ));
        assert!(!local_admission_allows(
            Some(io),
            Some(cpu),
            None,
            Some(25.0),
            Some(80.0)
        ));
        let high_io = io.replace("full avg10=12.0", "full avg10=25.1");
        assert_eq!(
            local_admission_block_reason(
                Some(&high_io),
                Some(cpu),
                Some(false),
                Some(25.0),
                Some(80.0)
            ),
            Some("io_pressure_high")
        );
        let high_cpu = cpu.replace("some avg10=30.0", "some avg10=80.1");
        assert_eq!(
            local_admission_block_reason(
                Some(io),
                Some(&high_cpu),
                Some(false),
                Some(25.0),
                Some(80.0)
            ),
            Some("cpu_pressure_high")
        );
    }

    #[test]
    fn missing_or_malformed_pressure_fails_closed() {
        assert_eq!(
            local_admission_block_reason(None, None, Some(false), Some(25.0), Some(80.0)),
            Some("io_pressure_unavailable")
        );
        assert_eq!(
            local_admission_block_reason(
                Some("full avg10=NaN"),
                Some("some avg10=0"),
                Some(false),
                Some(25.0),
                Some(80.0)
            ),
            Some("io_pressure_unavailable")
        );
        assert_eq!(
            local_admission_block_reason(
                Some("full avg10=0"),
                Some("some avg10=0"),
                Some(false),
                None,
                Some(80.0)
            ),
            Some("io_pressure_limit_invalid")
        );
        assert_eq!(
            local_admission_block_reason(
                Some("full avg10=0"),
                Some("some avg10=0"),
                Some(false),
                Some(25.0),
                None
            ),
            Some("cpu_pressure_limit_invalid")
        );
    }

    #[test]
    fn pressure_denial_releases_the_host_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host-storage.lock");
        File::create(&path).unwrap();
        assert!(try_acquire_at_with(&path, || Some("io_pressure_high"))
            .unwrap()
            .is_none());
        assert!(try_acquire_at(&path).unwrap().is_some());
    }

    #[tokio::test]
    async fn pressure_denial_does_not_claim_scheduler_assignment() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("host-storage.lock");
        File::create(&path).unwrap();
        let claim_called = Arc::new(AtomicBool::new(false));
        let claim_called_in_future = claim_called.clone();

        let result = reserve_before_claim(
            true,
            || try_acquire_at_with(&path, || Some("io_pressure_high")),
            || async move {
                claim_called_in_future.store(true, Ordering::SeqCst);
                Some("claimed")
            },
        )
        .await
        .unwrap();

        assert!(result.is_none());
        assert!(!claim_called.load(Ordering::SeqCst));
        assert!(try_acquire_at(&path).unwrap().is_some());
    }

    #[test]
    fn process_probe_blocks_live_builds_and_ignores_zombies() {
        let directory = tempfile::tempdir().unwrap();
        let live = directory.path().join("123");
        let zombie = directory.path().join("456");
        let other = directory.path().join("not-a-pid");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(&zombie).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(live.join("comm"), "cargo\n").unwrap();
        std::fs::write(live.join("status"), "Name:\tcargo\nState:\tR (running)\n").unwrap();
        std::fs::write(zombie.join("comm"), "rustc\n").unwrap();
        std::fs::write(zombie.join("status"), "Name:\trustc\nState:\tZ (zombie)\n").unwrap();
        assert_eq!(active_build_processes(directory.path()), Some(true));
        std::fs::remove_dir_all(&live).unwrap();
        assert_eq!(active_build_processes(directory.path()), Some(false));
    }
}
