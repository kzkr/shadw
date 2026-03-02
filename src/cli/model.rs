use crate::config::{self, ShadwConfig};
use crate::error::{Result, ShadwError};
use crate::models;

/// `shadw model [model]` — list available models or select one.
pub fn exec(model_name: Option<&str>) -> Result<()> {
    match model_name {
        None => list(),
        Some(name) => select(name),
    }
}

fn list() -> Result<()> {
    println!("Available models:\n");
    for spec in models::list_models() {
        let path = models::download::model_path(spec);
        let status = if path.exists() {
            "\x1b[32minstalled\x1b[0m".to_string()
        } else {
            format!("\x1b[2m{}\x1b[0m", models::registry::human_size(spec.size_bytes))
        };
        println!(
            "  \x1b[1m{}\x1b[0m  {} · ~{} · {} [{}]",
            spec.name, spec.params, models::registry::human_size(spec.size_bytes), spec.license, status
        );
        println!("  \x1b[2m{}\x1b[0m", spec.tagline);
    }

    // Show current selection if in a shadw project
    let cwd = std::env::current_dir()?;
    if let Ok(repo_root) = config::find_git_root(&cwd) {
        if let Ok(cfg) = ShadwConfig::load(&repo_root) {
            println!("\nCurrent: {}", cfg.model);
        }
    }

    println!("\nUsage: shadw model <model>");
    Ok(())
}

fn select(name: &str) -> Result<()> {
    let spec = models::get_model(name).ok_or_else(|| {
        ShadwError::Other(format!(
            "unknown model '{name}'. Run `shadw model` to see available models."
        ))
    })?;

    // Download if needed
    models::ensure_model(spec).map_err(ShadwError::Other)?;

    // Update config if in a shadw project
    let cwd = std::env::current_dir()?;
    if let Ok(repo_root) = config::find_git_root(&cwd) {
        let shadw = config::shadw_dir(&repo_root);
        if shadw.exists() {
            let mut cfg = ShadwConfig::load(&repo_root).unwrap_or_default();
            cfg.model = name.to_string();
            let toml_str = cfg
                .to_toml()
                .map_err(|e| ShadwError::Other(format!("failed to serialize config: {e}")))?;
            std::fs::write(shadw.join("config.toml"), toml_str)?;
            println!("Config updated: model = {name}");
        }
    }

    Ok(())
}
