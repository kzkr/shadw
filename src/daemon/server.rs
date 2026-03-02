use std::path::{Path, PathBuf};

use notify::{Config, RecursiveMode, Watcher};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{self, ExtractionConfig, ShadwConfig};
use crate::daemon::process;
use crate::error::{Result, ShadwError};
use crate::extraction;
use crate::watcher::git::GitWatcher;
use crate::watcher::{AgentWatcher, CapturedContext};

pub async fn run(repo_root: &Path) -> Result<()> {
    info!("daemon started, watching {}", repo_root.display());

    // Load config
    let shadw_config = ShadwConfig::load(repo_root).unwrap_or_else(|e| {
        warn!("failed to load config, using defaults: {e}");
        ShadwConfig::default()
    });
    let extraction_config = shadw_config.extraction_config();
    info!("extraction: model={}", extraction_config.model);

    let mut sigterm = signal(SignalKind::terminate())
        .map_err(|e| ShadwError::Other(format!("failed to register SIGTERM handler: {e}")))?;
    let mut sigint = signal(SignalKind::interrupt())
        .map_err(|e| ShadwError::Other(format!("failed to register SIGINT handler: {e}")))?;

    // Create agent-specific conversation watcher
    let state_dir = config::state_dir(repo_root);
    let agent_name = &shadw_config.agent;
    let mut conv_watcher = crate::watcher::create_watcher(agent_name, repo_root, &state_dir)?;
    conv_watcher.scan()?;
    let watch_dir = conv_watcher.watch_dir().to_path_buf();
    info!("agent: {} — watching {}", agent_name, watch_dir.display());

    // Git watcher
    let mut git_watcher = GitWatcher::new(repo_root.to_path_buf())?;
    let refs_dir = git_watcher.refs_heads_dir();
    info!("git refs: {}", refs_dir.display());

    // Filesystem event channel
    let (tx, mut rx) = mpsc::channel::<notify::Event>(256);
    let mut fs_watcher = notify::RecommendedWatcher::new(
        move |res: std::result::Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.blocking_send(event);
            }
        },
        Config::default(),
    )
    .map_err(|e| ShadwError::Other(format!("failed to create fs watcher: {e}")))?;

    let mut watching_agent_dir = false;
    if watch_dir.exists() {
        fs_watcher
            .watch(&watch_dir, RecursiveMode::NonRecursive)
            .map_err(|e| ShadwError::Other(format!("failed to watch agent dir: {e}")))?;
        watching_agent_dir = true;
    } else {
        warn!(
            "agent dir not found: {}",
            watch_dir.display()
        );
        warn!("will poll until it appears");
    }

    // Watch extra dir if the agent needs it (e.g. Cursor's global storage)
    if let Some(extra_dir) = conv_watcher.extra_watch_dir() {
        if extra_dir.exists() {
            fs_watcher
                .watch(extra_dir, RecursiveMode::NonRecursive)
                .map_err(|e| {
                    ShadwError::Other(format!("failed to watch extra agent dir: {e}"))
                })?;
            info!("also watching: {}", extra_dir.display());
        }
    }

    if refs_dir.exists() {
        fs_watcher
            .watch(&refs_dir, RecursiveMode::Recursive)
            .map_err(|e| ShadwError::Other(format!("failed to watch git refs: {e}")))?;
    }

    let paths = DaemonPaths {
        refs_dir,
        contexts_dir: config::shadw_dir(repo_root).join("contexts"),
        state_dir,
        repo_root: repo_root.to_path_buf(),
        agent: agent_name.to_string(),
    };
    info!("daemon ready");

    let mut poll_interval = tokio::time::interval(std::time::Duration::from_secs(5));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                handle_event(
                    &event,
                    conv_watcher.as_mut(),
                    &mut git_watcher,
                    &paths,
                    &extraction_config,
                );
            }
            _ = poll_interval.tick(), if !watching_agent_dir => {
                if watch_dir.exists() {
                    match fs_watcher.watch(&watch_dir, RecursiveMode::NonRecursive) {
                        Ok(()) => {
                            info!("agent dir appeared, now watching: {}", watch_dir.display());
                            conv_watcher.scan().ok();
                            watching_agent_dir = true;
                        }
                        Err(e) => {
                            warn!("failed to watch agent dir: {e}");
                        }
                    }
                }
            }
            _ = sigterm.recv() => {
                info!("received SIGTERM, shutting down");
                break;
            }
            _ = sigint.recv() => {
                info!("received SIGINT, shutting down");
                break;
            }
        }
    }

    conv_watcher.save_state(&paths.state_dir);
    let _ = process::remove_pid(repo_root);

    info!("daemon stopped");
    Ok(())
}

struct DaemonPaths {
    refs_dir: PathBuf,
    contexts_dir: PathBuf,
    state_dir: PathBuf,
    repo_root: PathBuf,
    agent: String,
}

fn handle_event(
    event: &notify::Event,
    conv_watcher: &mut dyn AgentWatcher,
    git_watcher: &mut GitWatcher,
    paths: &DaemonPaths,
    extraction_config: &ExtractionConfig,
) {
    for path in &event.paths {
        debug!("fs event: {:?} {}", event.kind, path.display());
        if path.starts_with(&paths.refs_dir) {
            if let Some(commit) = git_watcher.check_ref_change(path) {
                capture_context(commit, conv_watcher, paths, extraction_config);
            }
        } else if conv_watcher.handles_path(path) {
            if let Err(e) = conv_watcher.on_file_changed(path) {
                warn!("failed to read agent file: {e}");
            } else {
                debug!("buffered entries: {}", conv_watcher.buffer_len());
            }
        }
    }
}

fn capture_context(
    commit: crate::watcher::CommitInfo,
    conv_watcher: &mut dyn AgentWatcher,
    paths: &DaemonPaths,
    extraction_config: &ExtractionConfig,
) {
    // Re-read latest state before draining (important for SQLite-based watchers
    // where WAL writes may not reliably trigger filesystem events).
    if let Err(e) = conv_watcher.refresh() {
        warn!("failed to refresh conversation state: {e}");
    }

    let entries = conv_watcher.drain_all();

    // No conversation since last commit — nothing to extract
    if entries.is_empty() {
        info!("no conversation entries for {}, skipping", &commit.hash[..8.min(commit.hash.len())]);
        conv_watcher.save_state(&paths.state_dir);
        return;
    }

    let entry_count = entries.len();

    let context = CapturedContext {
        captured_at: chrono::Utc::now().to_rfc3339(),
        commit,
        conversation: entries,
        agent: paths.agent.clone(),
    };

    // Save raw context (gitignored)
    let hash = &context.commit.hash;
    let prefix = &hash[..2.min(hash.len())];
    let dir = paths.contexts_dir.join(prefix);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        warn!("failed to create context dir: {e}");
        return;
    }

    let path = dir.join(format!("{hash}.json"));
    match serde_json::to_string_pretty(&context) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, &json) {
                warn!("failed to write context: {e}");
            } else {
                info!(
                    "context captured: {} ({} conversation entries, {} files changed)",
                    &hash[..8],
                    entry_count,
                    context.commit.changed_files.len()
                );
                info!("  commit: {}", context.commit.message);
            }
        }
        Err(e) => {
            warn!("failed to serialize context: {e}");
            return;
        }
    }

    conv_watcher.save_state(&paths.state_dir);

    // Spawn async extraction → write git note → push notes → cleanup context
    let contexts_dir = paths.contexts_dir.clone();
    let repo_root = paths.repo_root.clone();
    let config = extraction_config.clone();
    tokio::spawn(extraction::extract_and_save(
        context,
        contexts_dir,
        repo_root,
        config,
    ));
}
