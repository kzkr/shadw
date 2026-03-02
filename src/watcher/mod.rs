pub mod conversation;
pub mod cursor;
pub mod git;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::Result;

/// A parsed conversation entry from an AI agent's session files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversationEntry {
    pub entry_type: String,
    pub timestamp: String,
    pub session_id: String,
    pub git_branch: String,
    pub role: Option<String>,
    pub content_preview: String,
}

/// Information about a git commit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub hash: String,
    pub message: String,
    pub author: String,
    pub timestamp: String,
    pub branch: String,
    pub changed_files: Vec<String>,
}

/// A captured context: commit + the conversation that led to it.
#[derive(Debug, Serialize, Deserialize)]
pub struct CapturedContext {
    pub commit: CommitInfo,
    pub conversation: Vec<ConversationEntry>,
    pub captured_at: String,
    /// Which agent produced this conversation (e.g. "claude-code", "cursor").
    #[serde(default)]
    pub agent: String,
}

/// Trait for agent-specific conversation watchers.
pub trait AgentWatcher: Send {
    /// Scan existing files and advance past current state (startup).
    fn scan(&mut self) -> Result<()>;

    /// Handle a filesystem change event for the given path.
    fn on_file_changed(&mut self, path: &Path) -> Result<()>;

    /// Drain all buffered conversation entries.
    fn drain_all(&mut self) -> Vec<ConversationEntry>;

    /// Number of buffered entries.
    fn buffer_len(&self) -> usize;

    /// The directory this watcher monitors for filesystem events.
    fn watch_dir(&self) -> &Path;

    /// Whether this watcher handles events for the given path.
    fn handles_path(&self, path: &Path) -> bool;

    /// Persist watcher state to the given state directory.
    fn save_state(&self, state_dir: &Path);

    /// Re-read latest conversation state before draining on commit.
    /// Default is a no-op (suitable for watchers driven by reliable fs events).
    fn refresh(&mut self) -> Result<()> {
        Ok(())
    }

    /// Optional additional directory to watch (e.g. global storage for Cursor).
    /// Returns None by default.
    fn extra_watch_dir(&self) -> Option<&Path> {
        None
    }
}

/// Create the appropriate watcher for the configured agent.
pub fn create_watcher(
    agent: &str,
    repo_root: &Path,
    state_dir: &Path,
) -> Result<Box<dyn AgentWatcher>> {
    match agent {
        "claude-code" => {
            let claude_dir = crate::config::claude_code_project_dir(repo_root);
            let cursor_path = state_dir.join("cursor.json");
            let cursors = conversation::load_cursors(&cursor_path);
            Ok(Box::new(conversation::ConversationWatcher::new(
                claude_dir, cursors,
            )))
        }
        "cursor" => Ok(Box::new(cursor::CursorWatcher::new(
            repo_root.to_path_buf(),
            state_dir,
        ))),
        _ => Err(crate::error::ShadwError::Other(format!(
            "unknown agent '{agent}'"
        ))),
    }
}
