use std::fs;
use std::path::{Path, PathBuf};

use tracing::{debug, info, warn};

use super::{AgentWatcher, ConversationEntry};
use crate::error::Result;
use crate::util::truncate;

pub struct CursorWatcher {
    /// Platform-specific base for Cursor workspace storage.
    workspace_storage_dir: PathBuf,
    /// The discovered workspace hash directory for this project.
    vscdb_dir: PathBuf,
    /// Full path to the workspace state.vscdb (used to read composer IDs).
    workspace_vscdb_path: PathBuf,
    /// Full path to the global state.vscdb (where conversations live).
    global_vscdb_path: PathBuf,
    /// Buffered conversation entries.
    buffer: Vec<ConversationEntry>,
    /// Whether workspace discovery has succeeded.
    discovered: bool,
    /// The repo root this watcher is associated with.
    repo_root: PathBuf,
    /// Composer IDs discovered from the workspace composerData.
    composer_ids: Vec<String>,
    /// Number of bubbles already seen (so we only buffer new ones).
    seen_bubble_count: usize,
}

impl CursorWatcher {
    pub fn new(repo_root: PathBuf, state_dir: &Path) -> Self {
        let base = workspace_storage_base();
        let global_vscdb = global_storage_vscdb();
        let saved_count = load_cursor_state(state_dir);

        Self {
            workspace_storage_dir: base,
            vscdb_dir: PathBuf::new(),
            workspace_vscdb_path: PathBuf::new(),
            global_vscdb_path: global_vscdb,
            buffer: Vec::new(),
            discovered: false,
            repo_root,
            composer_ids: Vec::new(),
            seen_bubble_count: saved_count.unwrap_or(0),
        }
    }

    /// Whether a Cursor workspace exists for this project (for status checks).
    pub fn has_workspace(&self) -> bool {
        discover_workspace(&self.workspace_storage_dir, &self.repo_root).is_some()
    }

    /// Attempt to discover which workspace hash directory belongs to this repo.
    fn discover(&mut self) -> bool {
        if self.discovered {
            return true;
        }

        if !self.workspace_storage_dir.exists() {
            debug!(
                "Cursor workspace storage dir not found: {}",
                self.workspace_storage_dir.display()
            );
            return false;
        }

        match discover_workspace(&self.workspace_storage_dir, &self.repo_root) {
            Some(dir) => {
                self.workspace_vscdb_path = dir.join("state.vscdb");
                self.discovered = true;
                debug!("discovered Cursor workspace: {}", dir.display());
                self.vscdb_dir = dir;
                true
            }
            None => {
                debug!(
                    "no Cursor workspace found for {}",
                    self.repo_root.display()
                );
                false
            }
        }
    }

    /// Read composer IDs from the workspace DB (only needs to happen once,
    /// or when a new composer is created).
    fn ensure_composer_ids(&mut self) {
        if !self.composer_ids.is_empty() {
            return;
        }

        if !self.workspace_vscdb_path.exists() {
            return;
        }

        let data = match read_item_table_value(
            &self.workspace_vscdb_path,
            "composer.composerData",
        ) {
            Ok(Some(d)) => d,
            _ => return,
        };

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) {
            if let Some(composers) = value.get("allComposers").and_then(|v| v.as_array()) {
                self.composer_ids = composers
                    .iter()
                    .filter_map(|c| c.get("composerId").and_then(|v| v.as_str()))
                    .map(|s| s.to_string())
                    .collect();
                debug!("Cursor: found {} composer(s)", self.composer_ids.len());
            }
        }
    }

    /// Read conversations from the global DB and buffer new entries.
    fn read_conversations(&mut self) -> Result<()> {
        self.ensure_composer_ids();

        if self.composer_ids.is_empty() {
            return Ok(());
        }
        if !self.global_vscdb_path.exists() {
            return Ok(());
        }

        // Read all bubbles from the global DB for our composers
        let all_bubbles = match read_all_bubbles(&self.global_vscdb_path, &self.composer_ids) {
            Ok(b) => b,
            Err(e) => {
                warn!("failed to read Cursor bubbles: {e}");
                return Ok(());
            }
        };

        let total = all_bubbles.len();

        if total <= self.seen_bubble_count {
            return Ok(());
        }

        let new_bubbles = &all_bubbles[self.seen_bubble_count..];
        debug!(
            "Cursor: {} new bubble(s) (total {}, seen {})",
            new_bubbles.len(),
            total,
            self.seen_bubble_count
        );

        for bubble in new_bubbles {
            if let Some(entry) = bubble_to_entry(bubble) {
                debug!(
                    "cursor entry: {} [{}] {}",
                    entry.entry_type,
                    entry.role.as_deref().unwrap_or("-"),
                    &entry.content_preview[..entry.content_preview.len().min(80)]
                );
                self.buffer.push(entry);
            }
        }

        self.seen_bubble_count = total;
        Ok(())
    }
}

impl AgentWatcher for CursorWatcher {
    fn scan(&mut self) -> Result<()> {
        if !self.discover() {
            return Ok(());
        }

        self.ensure_composer_ids();
        info!(
            "Cursor: {} composer(s) after scan",
            self.composer_ids.len()
        );

        // Count existing bubbles so we only capture new ones going forward
        if !self.composer_ids.is_empty() && self.global_vscdb_path.exists() {
            match read_all_bubbles(&self.global_vscdb_path, &self.composer_ids) {
                Ok(bubbles) => {
                    self.seen_bubble_count = bubbles.len();
                    info!(
                        "Cursor: scanned {} existing bubble(s), will capture new ones",
                        self.seen_bubble_count
                    );
                }
                Err(e) => {
                    warn!("failed to scan Cursor bubbles: {e}");
                }
            }
        }

        Ok(())
    }

    fn on_file_changed(&mut self, _path: &Path) -> Result<()> {
        if !self.discovered {
            self.discover();
        }
        self.read_conversations()
    }

    fn refresh(&mut self) -> Result<()> {
        self.read_conversations()
    }

    fn drain_all(&mut self) -> Vec<ConversationEntry> {
        std::mem::take(&mut self.buffer)
    }

    fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    fn watch_dir(&self) -> &Path {
        if self.discovered {
            &self.vscdb_dir
        } else {
            &self.workspace_storage_dir
        }
    }

    fn handles_path(&self, path: &Path) -> bool {
        // Only react to writes in the global storage directory, where conversations
        // actually live. The workspace state.vscdb changes frequently for unrelated
        // editor state — watching it would trigger unnecessary DB reads.
        path.parent() == self.global_vscdb_path.parent()
            && path
                .file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("state.vscdb"))
                .unwrap_or(false)
    }

    fn save_state(&self, state_dir: &Path) {
        let state = serde_json::json!({
            "seen_bubble_count": self.seen_bubble_count,
        });
        if let Ok(json) = serde_json::to_string_pretty(&state) {
            let _ = fs::write(state_dir.join("cursor_state.json"), json);
        }
    }

    fn extra_watch_dir(&self) -> Option<&Path> {
        // Watch the global storage dir so we catch bubble writes
        self.global_vscdb_path.parent()
    }
}

// --- Platform paths ---

fn workspace_storage_base() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        directories::UserDirs::new()
            .map(|d| {
                d.home_dir()
                    .join("Library/Application Support/Cursor/User/workspaceStorage")
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }

    #[cfg(target_os = "linux")]
    {
        directories::UserDirs::new()
            .map(|d| {
                d.home_dir()
                    .join(".config/Cursor/User/workspaceStorage")
            })
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        PathBuf::from("/tmp")
    }
}

fn global_storage_vscdb() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        directories::UserDirs::new()
            .map(|d| {
                d.home_dir()
                    .join("Library/Application Support/Cursor/User/globalStorage/state.vscdb")
            })
            .unwrap_or_else(|| PathBuf::from("/tmp/state.vscdb"))
    }

    #[cfg(target_os = "linux")]
    {
        directories::UserDirs::new()
            .map(|d| {
                d.home_dir()
                    .join(".config/Cursor/User/globalStorage/state.vscdb")
            })
            .unwrap_or_else(|| PathBuf::from("/tmp/state.vscdb"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        PathBuf::from("/tmp/state.vscdb")
    }
}

// --- Workspace discovery ---

/// Scan workspace storage directories to find the one matching our repo root.
fn discover_workspace(base: &Path, repo_root: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(base).ok()?;

    for entry in entries.flatten() {
        let dir = entry.path();
        if !dir.is_dir() {
            continue;
        }
        let ws_json = dir.join("workspace.json");
        if !ws_json.exists() {
            continue;
        }

        if let Ok(contents) = fs::read_to_string(&ws_json) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&contents) {
                if let Some(folder) = value.get("folder").and_then(|v| v.as_str()) {
                    let folder_path = folder.strip_prefix("file://").unwrap_or(folder);

                    if Path::new(folder_path) == repo_root {
                        return Some(dir);
                    }
                }
            }
        }
    }

    None
}

// --- SQLite reading ---

/// Read a value from the ItemTable in a workspace state.vscdb.
fn read_item_table_value(
    vscdb_path: &Path,
    key: &str,
) -> std::result::Result<Option<String>, String> {
    let conn = open_readonly(vscdb_path)?;

    let mut stmt = conn
        .prepare("SELECT value FROM ItemTable WHERE key = ?1")
        .map_err(|e| format!("failed to prepare query: {e}"))?;

    stmt.query_row([key], |row| row.get(0))
        .map(Some)
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            _ => Err(format!("query failed: {e}")),
        })
}

/// A raw bubble from the global DB.
struct RawBubble {
    /// 1 = user, 2 = assistant
    bubble_type: i64,
    text: String,
    thinking_text: Option<String>,
}

/// Read all bubbles for the given composer IDs from the global cursorDiskKV.
///
/// For each composer, reads `composerData:<id>` to get the bubble order
/// from `fullConversationHeadersOnly`, then fetches each bubble.
fn read_all_bubbles(
    global_vscdb: &Path,
    composer_ids: &[String],
) -> std::result::Result<Vec<RawBubble>, String> {
    let conn = open_readonly(global_vscdb)?;
    let mut all_bubbles = Vec::new();

    for composer_id in composer_ids {
        // Get the full composerData to learn bubble order
        let key = format!("composerData:{composer_id}");
        let data: Option<String> = conn
            .prepare("SELECT value FROM cursorDiskKV WHERE key = ?1")
            .map_err(|e| format!("prepare: {e}"))?
            .query_row([&key], |row| row.get(0))
            .ok();

        let bubble_ids = match data {
            Some(ref json_str) => extract_bubble_ids(json_str),
            None => continue,
        };

        // Fetch each bubble
        let mut stmt = conn
            .prepare("SELECT value FROM cursorDiskKV WHERE key = ?1")
            .map_err(|e| format!("prepare bubble: {e}"))?;

        for bubble_id in &bubble_ids {
            let bkey = format!("bubbleId:{composer_id}:{bubble_id}");
            let blob: Option<String> = stmt.query_row([&bkey], |row| row.get(0)).ok();

            if let Some(ref json_str) = blob {
                if let Some(bubble) = parse_bubble(json_str) {
                    all_bubbles.push(bubble);
                }
            }
        }
    }

    Ok(all_bubbles)
}

fn open_readonly(path: &Path) -> std::result::Result<rusqlite::Connection, String> {
    rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("failed to open {}: {e}", path.display()))
}

/// Extract ordered bubble IDs from a composerData JSON blob.
fn extract_bubble_ids(json_str: &str) -> Vec<String> {
    let value: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };

    value
        .get("fullConversationHeadersOnly")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item.get("bubbleId").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Parse a single bubble JSON into a RawBubble.
fn parse_bubble(json_str: &str) -> Option<RawBubble> {
    let value: serde_json::Value = serde_json::from_str(json_str).ok()?;

    let bubble_type = value.get("type").and_then(|v| v.as_i64())?;

    let text = value
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let thinking_text = value
        .get("thinking")
        .and_then(|v| v.get("text"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    Some(RawBubble {
        bubble_type,
        text,
        thinking_text,
    })
}

/// Convert a raw bubble into a ConversationEntry.
fn bubble_to_entry(bubble: &RawBubble) -> Option<ConversationEntry> {
    let (entry_type, role) = match bubble.bubble_type {
        1 => ("user", "user"),
        2 => ("assistant", "assistant"),
        _ => return None,
    };

    // For assistant messages, prefer `text`; fall back to `thinking.text`
    let content = if !bubble.text.is_empty() {
        &bubble.text
    } else if let Some(ref thinking) = bubble.thinking_text {
        thinking
    } else {
        return None;
    };

    let now = chrono::Utc::now().to_rfc3339();

    Some(ConversationEntry {
        entry_type: entry_type.to_string(),
        timestamp: now,
        session_id: String::new(),
        git_branch: String::new(),
        role: Some(role.to_string()),
        content_preview: truncate(content, 500),
    })
}

fn load_cursor_state(state_dir: &Path) -> Option<usize> {
    let path = state_dir.join("cursor_state.json");
    let contents = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&contents).ok()?;
    value
        .get("seen_bubble_count")
        .and_then(|v| v.as_u64())
        .map(|n| n as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_bubble_ids_from_composer_data() {
        let json = r#"{
            "_v": 13,
            "composerId": "abc",
            "fullConversationHeadersOnly": [
                {"bubbleId": "b1", "type": 1},
                {"bubbleId": "b2", "type": 2},
                {"bubbleId": "b3", "type": 2}
            ]
        }"#;

        let ids = extract_bubble_ids(json);
        assert_eq!(ids, vec!["b1", "b2", "b3"]);
    }

    #[test]
    fn extract_bubble_ids_empty() {
        assert!(extract_bubble_ids("{}").is_empty());
        assert!(extract_bubble_ids("invalid").is_empty());
    }

    #[test]
    fn parse_user_bubble() {
        let json = r#"{"type": 1, "text": "Add a login form", "bubbleId": "b1"}"#;
        let bubble = parse_bubble(json).unwrap();
        assert_eq!(bubble.bubble_type, 1);
        assert_eq!(bubble.text, "Add a login form");
    }

    #[test]
    fn parse_assistant_bubble_with_text() {
        let json = r#"{"type": 2, "text": "I'll create that for you.", "bubbleId": "b2"}"#;
        let bubble = parse_bubble(json).unwrap();
        assert_eq!(bubble.bubble_type, 2);
        assert_eq!(bubble.text, "I'll create that for you.");
    }

    #[test]
    fn parse_assistant_bubble_with_thinking_only() {
        let json = r#"{"type": 2, "text": "", "thinking": {"text": "Planning the approach..."}, "bubbleId": "b3"}"#;
        let bubble = parse_bubble(json).unwrap();
        assert_eq!(bubble.bubble_type, 2);
        assert!(bubble.text.is_empty());
        assert_eq!(
            bubble.thinking_text.as_deref(),
            Some("Planning the approach...")
        );
    }

    #[test]
    fn bubble_to_entry_user() {
        let bubble = RawBubble {
            bubble_type: 1,
            text: "Fix the bug".to_string(),
            thinking_text: None,
        };
        let entry = bubble_to_entry(&bubble).unwrap();
        assert_eq!(entry.entry_type, "user");
        assert_eq!(entry.content_preview, "Fix the bug");
    }

    #[test]
    fn bubble_to_entry_assistant_prefers_text() {
        let bubble = RawBubble {
            bubble_type: 2,
            text: "Done!".to_string(),
            thinking_text: Some("thinking...".to_string()),
        };
        let entry = bubble_to_entry(&bubble).unwrap();
        assert_eq!(entry.entry_type, "assistant");
        assert_eq!(entry.content_preview, "Done!");
    }

    #[test]
    fn bubble_to_entry_assistant_falls_back_to_thinking() {
        let bubble = RawBubble {
            bubble_type: 2,
            text: String::new(),
            thinking_text: Some("Planning the component...".to_string()),
        };
        let entry = bubble_to_entry(&bubble).unwrap();
        assert_eq!(entry.content_preview, "Planning the component...");
    }

    #[test]
    fn bubble_to_entry_skips_empty() {
        let bubble = RawBubble {
            bubble_type: 2,
            text: String::new(),
            thinking_text: None,
        };
        assert!(bubble_to_entry(&bubble).is_none());
    }

    #[test]
    fn bubble_to_entry_skips_unknown_type() {
        let bubble = RawBubble {
            bubble_type: 99,
            text: "something".to_string(),
            thinking_text: None,
        };
        assert!(bubble_to_entry(&bubble).is_none());
    }

    #[test]
    fn discover_workspace_with_matching_folder() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();
        let repo_root = Path::new("/tmp/my-project");

        let ws_dir = base.join("abc123hash");
        fs::create_dir(&ws_dir).unwrap();
        fs::write(
            ws_dir.join("workspace.json"),
            r#"{"folder": "file:///tmp/my-project"}"#,
        )
        .unwrap();

        let result = discover_workspace(base, repo_root);
        assert_eq!(result, Some(ws_dir));
    }

    #[test]
    fn discover_workspace_no_match() {
        let tmp = tempfile::TempDir::new().unwrap();
        let base = tmp.path();
        let repo_root = Path::new("/tmp/my-project");

        let ws_dir = base.join("abc123hash");
        fs::create_dir(&ws_dir).unwrap();
        fs::write(
            ws_dir.join("workspace.json"),
            r#"{"folder": "file:///tmp/other-project"}"#,
        )
        .unwrap();

        let result = discover_workspace(base, repo_root);
        assert!(result.is_none());
    }
}
