use crate::config;
use crate::daemon::process;
use crate::error::{Result, ShadwError};

pub fn exec() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_root = config::find_git_root(&cwd)?;
    let shadw = config::shadw_dir(&repo_root);

    if !shadw.exists() {
        return Err(ShadwError::NotInitialized);
    }

    // Watcher status
    match process::check_running(&repo_root)? {
        Some(_) => println!("Watcher: \x1b[32mrunning\x1b[0m"),
        None => println!("Watcher: \x1b[31mstopped\x1b[0m"),
    }

    // Model + install status on the same line
    let config = config::ShadwConfig::load(&repo_root).unwrap_or_default();
    let model_status = if let Some(spec) = crate::models::get_model(&config.model) {
        let path = crate::models::download::model_path(spec);
        if path.exists() {
            "\x1b[32minstalled\x1b[0m"
        } else {
            "\x1b[33mnot installed\x1b[0m"
        }
    } else {
        "\x1b[31munknown\x1b[0m"
    };
    println!("Model:   {} ({})", config.model, model_status);

    // Agent source
    let claude_dir = config::claude_code_project_dir(&repo_root);
    let source_status = if claude_dir.exists() {
        "\x1b[32mfound\x1b[0m"
    } else {
        "\x1b[31mnot found\x1b[0m"
    };
    println!("Agent:   {} ({})", config.agent, source_status);

    Ok(())
}
