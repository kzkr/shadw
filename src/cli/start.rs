use std::env;
use std::fs;
use std::path::Path;
use std::process::Command;

use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config;
use crate::daemon::{process, registry, server};
use crate::error::{Result, ShadwError};

/// Set by the parent process when spawning the background daemon child.
/// The child skips the "already running" check since the parent already did it.
const DAEMON_ENV: &str = "SHADW_DAEMON";

pub fn exec(foreground: bool, target: Option<&str>) -> Result<()> {
    match target {
        None => {
            // No target: same as before (current project), but auto-register + mark_started
            let cwd = env::current_dir()?;
            let repo_root = config::find_git_root(&cwd)?;
            let shadw = config::shadw_dir(&repo_root);
            if !shadw.exists() {
                return Err(ShadwError::NotInitialized);
            }

            let is_child = env::var(DAEMON_ENV).is_ok();
            if !is_child {
                if let Some(pid) = process::check_running(&repo_root)? {
                    return Err(ShadwError::DaemonAlreadyRunning(pid));
                }
            }

            // Auto-register in global registry
            let id = registry::register(&repo_root).ok();

            if foreground {
                run_foreground(&repo_root)?;
            } else {
                run_background(&repo_root)?;
            }

            // Mark started in registry
            if let Some(id) = id {
                let _ = registry::mark_started(id);
            }

            Ok(())
        }
        Some(arg) => {
            if foreground {
                return Err(ShadwError::Other(
                    "foreground mode is only valid for the current project (no target)".to_string(),
                ));
            }

            let parsed = registry::parse_target(arg);
            let reg = registry::load()?;
            let entries = registry::resolve_targets(&reg, &parsed);

            if entries.is_empty() {
                return Err(ShadwError::ProjectNotFound(arg.to_string()));
            }

            let mut started = 0;
            for entry in &entries {
                if !entry.path.exists() {
                    return Err(ShadwError::ProjectDirMissing(entry.path.clone()));
                }
                let shadw = config::shadw_dir(&entry.path);
                if !shadw.exists() {
                    eprintln!(
                        "warning: {} — not initialized, skipping",
                        registry::shorten_home(&entry.path)
                    );
                    continue;
                }
                // Skip if already running
                if let Some(pid) = process::check_running(&entry.path)? {
                    if entries.len() > 1 {
                        eprintln!(
                            "warning: {} — already running (PID {pid}), skipping",
                            registry::shorten_home(&entry.path)
                        );
                        continue;
                    } else {
                        return Err(ShadwError::DaemonAlreadyRunning(pid));
                    }
                }
                let label = registry::shorten_home(&entry.path);
                match start_for_path(&entry.path) {
                    Ok(()) => {
                        let _ = registry::mark_started(entry.id);
                        started += 1;
                    }
                    Err(e) => eprintln!("warning: {label} — {e}"),
                }
            }

            if entries.len() > 1 {
                println!("Started {started}/{} daemons.", entries.len());
            }

            Ok(())
        }
    }
}

/// Start the daemon for a specific project path (background mode).
fn start_for_path(repo_root: &Path) -> Result<()> {
    let exe = env::current_exe()?;
    let log_path = config::log_file(repo_root);

    fs::create_dir_all(config::state_dir(repo_root))?;

    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let log_err = log_file.try_clone()?;

    let child = Command::new(exe)
        .arg("start")
        .arg("--foreground")
        .env(DAEMON_ENV, "1")
        .current_dir(repo_root)
        .stdout(log_file)
        .stderr(log_err)
        .stdin(std::process::Stdio::null())
        .spawn()?;

    let pid = child.id();
    process::write_pid(repo_root, pid)?;
    println!("Shadw is now watching {}.", registry::shorten_home(repo_root));

    Ok(())
}

fn run_foreground(repo_root: &Path) -> Result<()> {
    // Log to stderr in foreground mode
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("shadw=info".parse().unwrap()),
        )
        .with_writer(std::io::stderr)
        .init();

    let pid = std::process::id();
    process::write_pid(repo_root, pid)?;
    info!("running in foreground (PID {pid})");
    println!("Shadw is watching this repo (PID {pid})");

    let repo_root = repo_root.to_path_buf();
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| ShadwError::Other(format!("failed to create tokio runtime: {e}")))?;
    let result = rt.block_on(server::run(&repo_root));

    // Ensure PID file is cleaned up
    process::remove_pid(&repo_root)?;
    result
}

fn run_background(repo_root: &Path) -> Result<()> {
    let exe = env::current_exe()?;
    let log_path = config::log_file(repo_root);

    // Ensure state dir exists
    fs::create_dir_all(config::state_dir(repo_root))?;

    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let log_err = log_file.try_clone()?;

    let child = Command::new(exe)
        .arg("start")
        .arg("--foreground")
        .env(DAEMON_ENV, "1")
        .current_dir(repo_root)
        .stdout(log_file)
        .stderr(log_err)
        .stdin(std::process::Stdio::null())
        .spawn()?;

    let pid = child.id();
    process::write_pid(repo_root, pid)?;
    println!("Shadw is now watching this repo.");

    Ok(())
}
