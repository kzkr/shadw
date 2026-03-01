use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tracing::{debug, warn};

use super::ConversationEntry;
use crate::error::Result;
use crate::util::truncate;

pub struct ConversationWatcher {
    project_dir: PathBuf,
    cursors: HashMap<PathBuf, u64>,
    buffer: Vec<ConversationEntry>,
}

impl ConversationWatcher {
    pub fn new(project_dir: PathBuf, cursors: HashMap<PathBuf, u64>) -> Self {
        Self {
            project_dir,
            cursors,
            buffer: Vec::new(),
        }
    }

    /// Scan all JSONL files and advance cursors to current end-of-file.
    /// Called on startup so we only capture new entries going forward.
    pub fn scan(&mut self) -> Result<()> {
        if !self.project_dir.exists() {
            return Ok(());
        }
        for entry in fs::read_dir(&self.project_dir)?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let std::collections::hash_map::Entry::Vacant(e) = self.cursors.entry(path) {
                    // First time seeing this file — start from end
                    if let Ok(meta) = fs::metadata(e.key()) {
                        e.insert(meta.len());
                    }
                }
            }
        }
        Ok(())
    }

    /// Read new lines from a specific file that changed.
    pub fn read_file(&mut self, path: &Path) -> Result<()> {
        if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
            return Ok(());
        }
        self.read_new_lines(path)
    }

    fn read_new_lines(&mut self, path: &Path) -> Result<()> {
        let cursor = self.cursors.get(path).copied().unwrap_or(0);

        let file = match File::open(path) {
            Ok(f) => f,
            Err(_) => return Ok(()),
        };
        let file_len = file.metadata()?.len();

        if file_len <= cursor {
            return Ok(());
        }

        let mut reader = BufReader::new(file);
        reader.seek(SeekFrom::Start(cursor))?;

        let mut new_cursor = cursor;
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
                break;
            }
            new_cursor += bytes_read as u64;

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(value) => {
                    if let Some(entry) = parse_entry(&value) {
                        debug!(
                            "conversation entry: {} [{}] {}",
                            entry.entry_type,
                            entry.role.as_deref().unwrap_or("-"),
                            &entry.content_preview[..entry.content_preview.len().min(80)]
                        );
                        self.buffer.push(entry);
                    }
                }
                Err(e) => {
                    warn!("failed to parse JSONL line: {e}");
                }
            }
        }

        self.cursors.insert(path.to_path_buf(), new_cursor);
        Ok(())
    }

    /// Drain ALL buffered entries.
    pub fn drain_all(&mut self) -> Vec<ConversationEntry> {
        std::mem::take(&mut self.buffer)
    }

    pub fn buffer_len(&self) -> usize {
        self.buffer.len()
    }

    pub fn cursors(&self) -> &HashMap<PathBuf, u64> {
        &self.cursors
    }

}

fn parse_entry(value: &serde_json::Value) -> Option<ConversationEntry> {
    let entry_type = value.get("type")?.as_str()?;

    // Only keep user and assistant messages
    match entry_type {
        "user" | "assistant" => {}
        _ => return None,
    }

    let timestamp = value.get("timestamp")?.as_str()?.to_string();
    let session_id = value
        .get("sessionId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let git_branch = value
        .get("gitBranch")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let message = value.get("message")?;
    let role = message
        .get("role")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let content_preview = extract_content_preview(message);

    Some(ConversationEntry {
        entry_type: entry_type.to_string(),
        timestamp,
        session_id,
        git_branch,
        role,
        content_preview,
    })
}

fn extract_content_preview(message: &serde_json::Value) -> String {
    if let Some(content) = message.get("content") {
        if let Some(text) = content.as_str() {
            return truncate(text, 500);
        }
        if let Some(arr) = content.as_array() {
            let mut parts = Vec::new();
            for block in arr {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");
                match block_type {
                    "text" => {
                        if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                            parts.push(truncate(text, 400));
                        }
                    }
                    "tool_use" => {
                        let name = block.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        // Extract file path from tool input so the correlation filter can match
                        let file_hint = block
                            .get("input")
                            .and_then(|input| {
                                input
                                    .get("file_path")
                                    .or_else(|| input.get("path"))
                                    .or_else(|| input.get("command"))
                                    .and_then(|v| v.as_str())
                            })
                            .unwrap_or("");
                        if file_hint.is_empty() {
                            parts.push(format!("[tool: {name}]"));
                        } else {
                            parts.push(format!("[tool: {name} {file_hint}]"));
                        }
                    }
                    "tool_result" => {
                        // Dropped: zero signal value, wastes budget
                    }
                    _ => {}
                }
            }
            return parts.join(" ");
        }
    }
    String::new()
}

