use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::daemon::process;
use crate::error::{Result, ShadwError};

/// Global daemon registry stored at ~/.shadw/daemons.toml
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DaemonRegistry {
    #[serde(default)]
    pub daemons: Vec<DaemonEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonEntry {
    pub id: u32,
    pub path: PathBuf,
    pub started_at: Option<String>,
}

/// Parsed from CLI argument — identifies one or all registry entries.
pub enum RegistryTarget {
    Id(u32),
    Path(PathBuf),
    All,
}

/// Computed daemon status (never stored).
pub enum DaemonStatus {
    Running(u32), // PID
    Stopped,
}

/// Return the global registry path: ~/.shadw/daemons.toml
pub fn registry_path() -> PathBuf {
    let home = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".shadw/daemons.toml")
}

/// Load the registry from disk. Returns an empty registry if file is missing.
pub fn load() -> Result<DaemonRegistry> {
    let path = registry_path();
    if !path.exists() {
        return Ok(DaemonRegistry::default());
    }
    let contents = fs::read_to_string(&path)?;
    toml::from_str(&contents)
        .map_err(|e| ShadwError::Other(format!("corrupt daemons.toml: {e}")))
}

/// Save the registry to disk atomically (write tmp + rename).
pub fn save(registry: &DaemonRegistry) -> Result<()> {
    let path = registry_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let contents = toml::to_string_pretty(registry)
        .map_err(|e| ShadwError::Other(format!("failed to serialize registry: {e}")))?;
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, contents)?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Register a project path. Idempotent — returns existing ID if already registered.
pub fn register(project_path: &Path) -> Result<u32> {
    let mut reg = load()?;
    let canonical = project_path
        .canonicalize()
        .unwrap_or_else(|_| project_path.to_path_buf());

    // Check if already registered
    for entry in &reg.daemons {
        let existing = entry.path.canonicalize().unwrap_or_else(|_| entry.path.clone());
        if existing == canonical {
            return Ok(entry.id);
        }
    }

    // Assign next ID
    let next_id = reg.daemons.iter().map(|e| e.id).max().unwrap_or(0) + 1;
    reg.daemons.push(DaemonEntry {
        id: next_id,
        path: canonical,
        started_at: None,
    });
    save(&reg)?;
    Ok(next_id)
}

/// Remove an entry from the registry by target. Returns the removed entry if found.
pub fn unregister(target: &RegistryTarget) -> Result<Option<DaemonEntry>> {
    let mut reg = load()?;
    let idx = match target {
        RegistryTarget::Id(id) => reg.daemons.iter().position(|e| e.id == *id),
        RegistryTarget::Path(p) => {
            let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
            reg.daemons.iter().position(|e| {
                let existing = e.path.canonicalize().unwrap_or_else(|_| e.path.clone());
                existing == canonical
            })
        }
        RegistryTarget::All => return Err(ShadwError::Other(
            "cannot unregister all projects at once — too destructive. Remove them individually.".to_string(),
        )),
    };
    match idx {
        Some(i) => {
            let entry = reg.daemons.remove(i);
            save(&reg)?;
            Ok(Some(entry))
        }
        None => Ok(None),
    }
}

/// Find an entry in the registry by target.
pub fn find(registry: &DaemonRegistry, target: &RegistryTarget) -> Option<DaemonEntry> {
    match target {
        RegistryTarget::Id(id) => registry.daemons.iter().find(|e| e.id == *id).cloned(),
        RegistryTarget::Path(p) => {
            let canonical = p.canonicalize().unwrap_or_else(|_| p.clone());
            registry.daemons.iter().find(|e| {
                let existing = e.path.canonicalize().unwrap_or_else(|_| e.path.clone());
                existing == canonical
            }).cloned()
        }
        RegistryTarget::All => None, // use resolve_targets for All
    }
}

/// Resolve a target to a list of entries. For All, returns all entries.
pub fn resolve_targets(registry: &DaemonRegistry, target: &RegistryTarget) -> Vec<DaemonEntry> {
    match target {
        RegistryTarget::All => registry.daemons.clone(),
        _ => find(registry, target).into_iter().collect(),
    }
}

/// Check the actual daemon status for an entry by reading its PID file.
pub fn entry_status(entry: &DaemonEntry) -> DaemonStatus {
    match process::check_running(&entry.path) {
        Ok(Some(pid)) => DaemonStatus::Running(pid),
        _ => DaemonStatus::Stopped,
    }
}

/// Set started_at timestamp for an entry.
pub fn mark_started(id: u32) -> Result<()> {
    let mut reg = load()?;
    if let Some(entry) = reg.daemons.iter_mut().find(|e| e.id == id) {
        entry.started_at = Some(Utc::now().to_rfc3339());
    }
    save(&reg)
}

/// Clear started_at for an entry.
pub fn mark_stopped(id: u32) -> Result<()> {
    let mut reg = load()?;
    if let Some(entry) = reg.daemons.iter_mut().find(|e| e.id == id) {
        entry.started_at = None;
    }
    save(&reg)
}

/// Parse a CLI argument into a RegistryTarget.
pub fn parse_target(s: &str) -> RegistryTarget {
    if s == "all" {
        return RegistryTarget::All;
    }
    if let Ok(id) = s.parse::<u32>() {
        return RegistryTarget::Id(id);
    }
    RegistryTarget::Path(PathBuf::from(s))
}

/// Shorten a path by replacing the home directory with ~.
pub fn shorten_home(path: &Path) -> String {
    if let Some(dirs) = directories::UserDirs::new() {
        let home = dirs.home_dir();
        if let Ok(stripped) = path.strip_prefix(home) {
            return format!("~/{}", stripped.display());
        }
    }
    path.display().to_string()
}

/// Format uptime from an RFC 3339 started_at timestamp.
pub fn format_uptime(started_at: &str) -> String {
    let Ok(start) = chrono::DateTime::parse_from_rfc3339(started_at) else {
        return String::from("?");
    };
    let elapsed = Utc::now().signed_duration_since(start);
    let total_secs = elapsed.num_seconds();
    if total_secs < 0 {
        return String::from("0s");
    }
    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;
    if days > 0 {
        format!("{days}d {hours}h")
    } else if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m")
    } else {
        format!("{total_secs}s")
    }
}
