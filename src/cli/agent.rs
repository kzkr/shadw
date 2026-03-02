use crate::config::{self, ShadwConfig};
use crate::daemon::process;
use crate::error::{Result, ShadwError};

/// `shadw agent [name]` — list available agents or select one.
pub fn exec(name: Option<&str>) -> Result<()> {
    match name {
        None => list(),
        Some(n) => select(n),
    }
}

fn list() -> Result<()> {
    println!("Available agents:\n");
    for spec in config::list_agents() {
        println!("  \x1b[1m{}\x1b[0m  {}", spec.id, spec.name);
    }

    // Show current selection if in a shadw project
    let cwd = std::env::current_dir()?;
    if let Ok(repo_root) = config::find_git_root(&cwd) {
        if let Ok(cfg) = ShadwConfig::load(&repo_root) {
            println!("\nCurrent: {}", cfg.agent);
        }
    }

    println!("\nUsage: shadw agent <name>");
    Ok(())
}

fn select(name: &str) -> Result<()> {
    let _spec = config::get_agent(name).ok_or_else(|| {
        ShadwError::Other(format!(
            "unknown agent '{name}'. Run `shadw agent` to see available agents."
        ))
    })?;

    // Update config if in a shadw project
    let cwd = std::env::current_dir()?;
    let repo_root = config::find_git_root(&cwd)?;
    let shadw = config::shadw_dir(&repo_root);

    if !shadw.exists() {
        return Err(ShadwError::NotInitialized);
    }

    let mut cfg = ShadwConfig::load(&repo_root).unwrap_or_default();
    cfg.agent = name.to_string();
    let toml_str = cfg
        .to_toml()
        .map_err(|e| ShadwError::Other(format!("failed to serialize config: {e}")))?;
    std::fs::write(shadw.join("config.toml"), toml_str)?;
    println!("Config updated: agent = {name}");

    // Auto-restart daemon if running
    if let Ok(Some(_pid)) = process::check_running(&repo_root) {
        println!("Restarting daemon...");
        super::stop::exec(None)?;
        super::start::exec(false, None)?;
    }

    Ok(())
}
