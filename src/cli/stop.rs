use std::path::Path;
use std::thread;
use std::time::Duration;

use crate::config;
use crate::daemon::{process, registry};
use crate::error::{Result, ShadwError};

pub fn exec(target: Option<&str>) -> Result<()> {
    match target {
        None => {
            // No target: same as before (current project), but auto-register
            let cwd = std::env::current_dir()?;
            let repo_root = config::find_git_root(&cwd)?;
            let shadw = config::shadw_dir(&repo_root);
            if !shadw.exists() {
                return Err(ShadwError::NotInitialized);
            }

            // Auto-register in global registry
            let id = registry::register(&repo_root).ok();

            stop_for_path(&repo_root)?;

            // Mark stopped in registry
            if let Some(id) = id {
                let _ = registry::mark_stopped(id);
            }

            Ok(())
        }
        Some(arg) => {
            let parsed = registry::parse_target(arg);
            let reg = registry::load()?;
            let entries = registry::resolve_targets(&reg, &parsed);

            if entries.is_empty() {
                return Err(ShadwError::ProjectNotFound(arg.to_string()));
            }

            let mut stopped = 0;
            for entry in &entries {
                if !entry.path.exists() {
                    eprintln!(
                        "warning: {} — directory missing, skipping",
                        registry::shorten_home(&entry.path)
                    );
                    continue;
                }
                let label = registry::shorten_home(&entry.path);
                match stop_for_path(&entry.path) {
                    Ok(()) => {
                        let _ = registry::mark_stopped(entry.id);
                        stopped += 1;
                    }
                    Err(e) => eprintln!("warning: {label} — {e}"),
                }
            }

            if entries.len() > 1 {
                println!("Stopped {stopped}/{} daemons.", entries.len());
            }

            Ok(())
        }
    }
}

/// Stop the daemon for a specific project path.
pub fn stop_for_path(repo_root: &Path) -> Result<()> {
    let pid = match process::check_running(repo_root)? {
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
            process::remove_pid(repo_root)?;
            println!("Shadw stopped.");
            return Ok(());
        }
    }

    // Force kill
    process::send_sigkill(pid)?;
    thread::sleep(Duration::from_millis(100));
    process::remove_pid(repo_root)?;
    println!("Shadw stopped.");

    Ok(())
}
