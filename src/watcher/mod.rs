pub mod conversation;
pub mod git;

use serde::{Deserialize, Serialize};

/// A parsed conversation entry from a Claude Code JSONL file.
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
}
