use std::fs;
use std::path::Path;

use nix::sys::signal::{self, Signal};
use nix::unistd::Pid;

use crate::config;
use crate::error::{Result, ShadwError};

/// Read the PID from the PID file, returning None if it doesn't exist.
pub fn read_pid(repo_root: &Path) -> Result<Option<u32>> {
    let pid_path = config::pid_file(repo_root);
    if !pid_path.exists() {
        return Ok(None);
    }
    let contents = fs::read_to_string(&pid_path)?;
    let pid: u32 = contents
        .trim()
        .parse()
        .map_err(|e| ShadwError::Other(format!("corrupt PID file: {e}")))?;
    Ok(Some(pid))
}

/// Write the PID to the PID file.
pub fn write_pid(repo_root: &Path, pid: u32) -> Result<()> {
    let pid_path = config::pid_file(repo_root);
    fs::write(&pid_path, pid.to_string())?;
    Ok(())
}

/// Remove the PID file.
pub fn remove_pid(repo_root: &Path) -> Result<()> {
    let pid_path = config::pid_file(repo_root);
    if pid_path.exists() {
        fs::remove_file(&pid_path)?;
    }
    Ok(())
}

/// Check if a process with the given PID is alive.
pub fn is_alive(pid: u32) -> bool {
    // signal 0 doesn't send a signal, just checks if process exists
    signal::kill(Pid::from_raw(pid as i32), None).is_ok()
}

/// Send SIGTERM to a process.
pub fn send_sigterm(pid: u32) -> Result<()> {
    signal::kill(Pid::from_raw(pid as i32), Signal::SIGTERM)
        .map_err(|e| ShadwError::Other(format!("failed to send SIGTERM: {e}")))?;
    Ok(())
}

/// Send SIGKILL to a process.
pub fn send_sigkill(pid: u32) -> Result<()> {
    signal::kill(Pid::from_raw(pid as i32), Signal::SIGKILL)
        .map_err(|e| ShadwError::Other(format!("failed to send SIGKILL: {e}")))?;
    Ok(())
}

/// Check for stale PID file and clean it up. Returns Ok(Some(pid)) if a
/// daemon is actually running, Ok(None) if no daemon (stale cleaned up).
pub fn check_running(repo_root: &Path) -> Result<Option<u32>> {
    match read_pid(repo_root)? {
        Some(pid) => {
            if is_alive(pid) {
                Ok(Some(pid))
            } else {
                // Stale PID file — process is dead
                remove_pid(repo_root)?;
                Ok(None)
            }
        }
        None => Ok(None),
    }
}
