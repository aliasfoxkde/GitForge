//! Process subreaper implementation
//!
//! A subreaper process is responsible for reaping orphaned child processes.
//! This prevents zombie processes from accumulating when child processes
//! terminate but are not explicitly waited on.

use std::io::Result;
use libc::prctl;
use libc::PR_SET_CHILD_SUBREAPER;

/// Set this process as a subreaper.
///
/// A subreaper is a process that inherits orphaned child processes and is
/// responsible for reaping them. Without this, orphaned children can become
/// zombies that are never reaped.
///
/// # Example
/// ```
/// gitforce_process::become_subreaper().expect("failed to become subreaper");
/// ```
pub fn become_subreaper() -> Result<()> {
    unsafe {
        if prctl(PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    tracing::info!("process configured as subreaper");
    Ok(())
}

/// Check if this process is configured as a subreaper
#[cfg(target_os = "linux")]
pub fn is_subreaper() -> bool {
    let mut reaper: i32 = 0;
    unsafe {
        libc::prctl(libc::PR_GET_CHILD_SUBREAPER, &mut reaper as *mut i32, 0, 0, 0);
    }
    reaper != 0
}

#[cfg(not(target_os = "linux"))]
pub fn is_subreaper() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_become_subreaper() {
        become_subreaper().expect("failed to become subreaper");
        // On Linux, verify we're actually a subreaper
        #[cfg(target_os = "linux")]
        assert!(is_subreaper());
    }
}
