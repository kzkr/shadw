use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::error::{Result, ShadwError};

#[derive(Debug, Serialize, Deserialize)]
pub struct ShadwConfig {
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_agent")]
    pub agent: String,
    #[serde(default = "default_model")]
    pub model: String,
}

fn default_agent() -> String {
    "claude-code".to_string()
}

fn default_model() -> String {
    "gpt-oss".to_string()
}

impl Default for ShadwConfig {
    fn default() -> Self {
        Self {
            author: String::new(),
            agent: default_agent(),
            model: default_model(),
        }
    }
}

/// Runtime config passed to the extraction pipeline.
#[derive(Debug, Clone)]
pub struct ExtractionConfig {
    pub model: String,
    pub author: String,
}

impl ShadwConfig {
    pub fn to_toml(&self) -> std::result::Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    /// Build an `ExtractionConfig` from the flat config fields.
    pub fn extraction_config(&self) -> ExtractionConfig {
        ExtractionConfig {
            model: self.model.clone(),
            author: self.author.clone(),
        }
    }

    pub fn load(repo_root: &Path) -> Result<Self> {
        let path = shadw_dir(repo_root).join("config.toml");
        let contents = std::fs::read_to_string(&path)?;
        toml::from_str(&contents)
            .map_err(|e| ShadwError::Other(format!("invalid config.toml: {e}")))
    }
}

/// An AI agent that Shadw can watch.
pub struct AgentSpec {
    pub id: &'static str,
    pub name: &'static str,
}

static AGENTS: &[AgentSpec] = &[
    AgentSpec {
        id: "claude-code",
        name: "Claude Code",
    },
    AgentSpec {
        id: "cursor",
        name: "Cursor",
    },
];

pub fn list_agents() -> &'static [AgentSpec] {
    AGENTS
}

pub fn get_agent(id: &str) -> Option<&'static AgentSpec> {
    AGENTS.iter().find(|a| a.id == id)
}

/// Walk up from `start` looking for `.git/`. Returns the repo root or error.
pub fn find_git_root(start: &Path) -> Result<PathBuf> {
    let mut dir = start.to_path_buf();
    loop {
        if dir.join(".git").exists() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err(ShadwError::NotGitRepo);
        }
    }
}

/// Return the `.shadw/` directory path for a repo root.
pub fn shadw_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".shadw")
}

/// Return the state directory path (machine-local, gitignored).
pub fn state_dir(repo_root: &Path) -> PathBuf {
    shadw_dir(repo_root).join("state")
}

/// Return the PID file path.
pub fn pid_file(repo_root: &Path) -> PathBuf {
    state_dir(repo_root).join("daemon.pid")
}

/// Return the daemon log file path.
pub fn log_file(repo_root: &Path) -> PathBuf {
    state_dir(repo_root).join("daemon.log")
}

/// Compute the Claude Code project directory for a given repo root.
pub fn claude_code_project_dir(repo_root: &Path) -> PathBuf {
    let home = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/tmp"));

    let encoded = encode_project_path(repo_root);
    home.join(".claude/projects").join(encoded)
}

fn encode_project_path(path: &Path) -> String {
    path.to_string_lossy()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect()
}
