use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tracing::{info, warn};

use super::CommitInfo;

pub struct GitWatcher {
    repo_root: PathBuf,
    known_refs: HashMap<String, String>,
}

impl GitWatcher {
    pub fn new(repo_root: PathBuf) -> crate::error::Result<Self> {
        let mut watcher = Self {
            repo_root,
            known_refs: HashMap::new(),
        };
        watcher.scan_refs()?;
        Ok(watcher)
    }

    fn scan_refs(&mut self) -> crate::error::Result<()> {
        let refs_dir = self.repo_root.join(".git/refs/heads");
        if refs_dir.exists() {
            self.scan_refs_dir(&refs_dir, "")?;
        }
        Ok(())
    }

    fn scan_refs_dir(&mut self, dir: &Path, prefix: &str) -> crate::error::Result<()> {
        for entry in fs::read_dir(dir)?.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let branch = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };

            if path.is_dir() {
                self.scan_refs_dir(&path, &branch)?;
            } else if let Ok(hash) = fs::read_to_string(&path) {
                self.known_refs.insert(branch, hash.trim().to_string());
            }
        }
        Ok(())
    }

    /// Check if a ref file change represents a new commit.
    pub fn check_ref_change(&mut self, path: &Path) -> Option<CommitInfo> {
        let refs_heads = self.repo_root.join(".git/refs/heads");
        let branch = path.strip_prefix(&refs_heads).ok()?;
        let branch_name = branch.to_string_lossy().to_string();

        let new_hash = fs::read_to_string(path).ok()?.trim().to_string();

        // Skip if hash hasn't changed (debounce duplicate events)
        if let Some(old_hash) = self.known_refs.get(&branch_name) {
            if *old_hash == new_hash {
                return None;
            }
        }

        info!("new commit on {branch_name}: {}", &new_hash[..new_hash.len().min(8)]);
        self.known_refs.insert(branch_name.clone(), new_hash.clone());

        get_commit_info(&self.repo_root, &new_hash, &branch_name)
    }

    pub fn refs_heads_dir(&self) -> PathBuf {
        self.repo_root.join(".git/refs/heads")
    }
}

fn get_commit_info(repo_root: &Path, hash: &str, branch: &str) -> Option<CommitInfo> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "log",
            "-1",
            "--format=%H%n%s%n%an%n%aI",
            hash,
        ])
        .output()
        .ok()?;

    if !output.status.success() {
        warn!("git log failed for {hash}");
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.trim().lines().collect();
    if lines.len() < 4 {
        return None;
    }

    let diff_output = Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "-r",
            hash,
        ])
        .output()
        .ok()?;

    let changed_files: Vec<String> = if diff_output.status.success() {
        String::from_utf8_lossy(&diff_output.stdout)
            .trim()
            .lines()
            .filter(|l| !l.is_empty())
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    Some(CommitInfo {
        hash: lines[0].to_string(),
        message: lines[1].to_string(),
        author: lines[2].to_string(),
        timestamp: lines[3].to_string(),
        branch: branch.to_string(),
        changed_files,
    })
}
