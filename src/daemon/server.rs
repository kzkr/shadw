use std::collections::HashMap;
use std::path::{Path, PathBuf};

use notify::{Config, RecursiveMode, Watcher};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use crate::config::{self, ExtractionConfig, ShadwConfig};
use crate::daemon::process;
use crate::error::{Result, ShadwError};
use crate::extraction;
use crate::watcher::conversation::ConversationWatcher;
use crate::watcher::git::GitWatcher;
use crate::watcher::CapturedContext;

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

    // Conversation watcher
    let claude_dir = config::claude_code_project_dir(repo_root);
    let cursor_path = config::state_dir(repo_root).join("cursor.json");
    let cursors = load_cursors(&cursor_path);
    let mut conv_watcher = ConversationWatcher::new(claude_dir.clone(), cursors);
    conv_watcher.scan()?;
    info!("conversations: {}", claude_dir.display());

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

    let mut watching_claude_dir = false;
    if claude_dir.exists() {
        fs_watcher
            .watch(&claude_dir, RecursiveMode::NonRecursive)
            .map_err(|e| ShadwError::Other(format!("failed to watch conversation dir: {e}")))?;
        watching_claude_dir = true;
    } else {
        warn!(
            "Claude Code project dir not found: {}",
            claude_dir.display()
        );
        warn!("will poll until it appears");
    }

    if refs_dir.exists() {
        fs_watcher
            .watch(&refs_dir, RecursiveMode::Recursive)
            .map_err(|e| ShadwError::Other(format!("failed to watch git refs: {e}")))?;
    }

    let paths = DaemonPaths {
        refs_dir,
        contexts_dir: config::shadw_dir(repo_root).join("contexts"),
        cursor_path,
        repo_root: repo_root.to_path_buf(),
    };
    info!("daemon ready");

    let mut poll_interval = tokio::time::interval(std::time::Duration::from_secs(5));
    poll_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            Some(event) = rx.recv() => {
                handle_event(
                    &event,
                    &mut conv_watcher,
                    &mut git_watcher,
                    &paths,
                    &extraction_config,
                );
            }
            _ = poll_interval.tick(), if !watching_claude_dir => {
                if claude_dir.exists() {
                    match fs_watcher.watch(&claude_dir, RecursiveMode::NonRecursive) {
                        Ok(()) => {
                            info!("Claude Code project dir appeared, now watching: {}", claude_dir.display());
                            conv_watcher.scan().ok();
                            watching_claude_dir = true;
                        }
                        Err(e) => {
                            warn!("failed to watch conversation dir: {e}");
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

    save_cursors(&paths.cursor_path, conv_watcher.cursors());
    let _ = process::remove_pid(repo_root);

    info!("daemon stopped");
    Ok(())
}

struct DaemonPaths {
    refs_dir: PathBuf,
    contexts_dir: PathBuf,
    cursor_path: PathBuf,
    repo_root: PathBuf,
}

fn handle_event(
    event: &notify::Event,
    conv_watcher: &mut ConversationWatcher,
    git_watcher: &mut GitWatcher,
    paths: &DaemonPaths,
    extraction_config: &ExtractionConfig,
) {
    for path in &event.paths {
        if path.starts_with(&paths.refs_dir) {
            if let Some(commit) = git_watcher.check_ref_change(path) {
                capture_context(commit, conv_watcher, paths, extraction_config);
            }
        } else if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
            if let Err(e) = conv_watcher.read_file(path) {
                warn!("failed to read conversation file: {e}");
            } else {
                debug!("buffered entries: {}", conv_watcher.buffer_len());
            }
        }
    }
}

fn capture_context(
    commit: crate::watcher::CommitInfo,
    conv_watcher: &mut ConversationWatcher,
    paths: &DaemonPaths,
    extraction_config: &ExtractionConfig,
) {
    let entries = conv_watcher.drain_all();

    // No conversation since last commit — nothing to extract
    if entries.is_empty() {
        info!("no conversation entries for {}, skipping", &commit.hash[..8.min(commit.hash.len())]);
        save_cursors(&paths.cursor_path, conv_watcher.cursors());
        return;
    }

    let entry_count = entries.len();

    let context = CapturedContext {
        captured_at: chrono::Utc::now().to_rfc3339(),
        commit,
        conversation: entries,
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

    save_cursors(&paths.cursor_path, conv_watcher.cursors());

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

fn load_cursors(path: &Path) -> HashMap<PathBuf, u64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_cursors(path: &Path, cursors: &HashMap<PathBuf, u64>) {
    if let Ok(json) = serde_json::to_string_pretty(cursors) {
        let _ = std::fs::write(path, json);
    }
}
