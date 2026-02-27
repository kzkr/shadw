use std::thread;
use std::time::Duration;

use crate::config;
use crate::daemon::process;
use crate::error::{Result, ShadwError};

pub fn exec() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_root = config::find_git_root(&cwd)?;
    let shadw = config::shadw_dir(&repo_root);

    if !shadw.exists() {
        return Err(ShadwError::NotInitialized);
    }

    let pid = match process::check_running(&repo_root)? {
        Some(pid) => pid,
        None => {
            println!("Shadw is not running.");
            return Ok(());
        }
    };

    // Send SIGTERM
    process::send_sigterm(pid)?;

    // Wait up to 3 seconds for graceful shutdown
    for _ in 0..30 {
        thread::sleep(Duration::from_millis(100));
        if !process::is_alive(pid) {
            process::remove_pid(&repo_root)?;
            println!("Shadw stopped.");
            return Ok(());
        }
    }

    // Force kill
    process::send_sigkill(pid)?;
    thread::sleep(Duration::from_millis(100));
    process::remove_pid(&repo_root)?;
    println!("Shadw stopped.");

    Ok(())
}
