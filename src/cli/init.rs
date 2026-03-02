use std::fs;
use std::io::{self, IsTerminal, Write as _};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use crate::config::{self, ShadwConfig};
use crate::error::{Result, ShadwError};
use crate::models;
use crate::config::{list_agents, get_agent};

pub fn exec() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_root = config::find_git_root(&cwd)?;
    let shadw = config::shadw_dir(&repo_root);

    if shadw.exists() {
        return Err(ShadwError::AlreadyInitialized(repo_root));
    }

    let interactive = io::stdin().is_terminal();

    // --- Gather all user input before writing anything to disk ---
    // If the user Ctrl+C's during a prompt, nothing has been created yet.

    let mut config = ShadwConfig::default();
    let git_author = detect_github_user(&repo_root);

    // Author
    config.author = if interactive {
        pick_author(&git_author)?
    } else {
        git_author
    };

    // Agent selection
    config.agent = if interactive {
        pick_agent()?
    } else {
        list_agents()[0].id.to_string()
    };

    // Model selection
    let model_name = if interactive {
        pick_model()?
    } else {
        models::list_models()[0].name.to_string()
    };
    config.model = model_name.clone();

    // --- All input collected — write to disk ---

    fs::create_dir_all(shadw.join("contexts"))?;
    fs::create_dir_all(shadw.join("state"))?;

    let toml_str = config
        .to_toml()
        .map_err(|e| ShadwError::Other(format!("failed to serialize config: {e}")))?;
    fs::write(shadw.join("config.toml"), toml_str)?;
    fs::write(shadw.join("state/cursor.json"), "{}")?;

    append_gitignore(&repo_root)?;
    install_pre_push_hook(&repo_root)?;
    install_github_action(&repo_root)?;

    println!("Initialized Shadw in {}/.shadw/", repo_root.display());

    // Register in global daemon registry (non-fatal)
    if let Err(e) = crate::daemon::registry::register(&repo_root) {
        eprintln!("warning: could not register in global registry: {e}");
    }

    if interactive {
        // Download the model
        let spec = models::get_model(&model_name).ok_or_else(|| {
            ShadwError::Other(format!("unknown model '{model_name}'"))
        })?;
        models::ensure_model(spec).map_err(ShadwError::Other)?;

        // Start watching
        println!();
        super::start::exec(false, None)?;
        println!("\nRun `shadw --help` for available commands.");
    }

    Ok(())
}

/// Ask for the author's GitHub handle.
fn pick_author(default: &str) -> Result<String> {
    if default.is_empty() {
        print!("GitHub handle \x1b[2m(@username)\x1b[0m: ");
    } else {
        print!("GitHub handle \x1b[2m({})\x1b[0m: ", default);
    }
    io::stdout().flush().map_err(ShadwError::Io)?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(ShadwError::Io)?;
    let input = input.trim();

    if input.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(input.to_string())
    }
}

/// Present the agent list and ask the user to pick one.
fn pick_agent() -> Result<String> {
    let all = list_agents();

    println!("Select an AI agent to watch:\n");
    for (i, spec) in all.iter().enumerate() {
        println!("  [{}] {}", i + 1, spec.name);
    }
    println!();

    let default = &all[0];
    print!("Agent \x1b[2m({})\x1b[0m: ", default.name);
    io::stdout().flush().map_err(ShadwError::Io)?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(ShadwError::Io)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(default.id.to_string());
    }

    // Accept number or id
    if let Ok(n) = input.parse::<usize>() {
        if n >= 1 && n <= all.len() {
            return Ok(all[n - 1].id.to_string());
        }
    }

    if let Some(spec) = get_agent(input) {
        return Ok(spec.id.to_string());
    }

    Err(ShadwError::Other(format!(
        "unknown agent '{input}'."
    )))
}

/// Present the model list and ask the user to pick one.
fn pick_model() -> Result<String> {
    let all = models::list_models();

    println!("Select a model:\n");
    for (i, spec) in all.iter().enumerate() {
        let path = models::download::model_path(spec);
        let status = if path.exists() {
            "\x1b[32minstalled\x1b[0m"
        } else {
            &format!("\x1b[2m{}\x1b[0m", models::registry::human_size(spec.size_bytes))
        };
        println!("  [{}] \x1b[1m{}\x1b[0m  {} · {} [{}]", i + 1, spec.name, spec.params, spec.license, status);
        println!("      \x1b[2m{}\x1b[0m", spec.tagline);
    }
    println!();

    // Default to the first model
    let default = &all[0];
    print!("Model \x1b[2m({})\x1b[0m: ", default.name);
    io::stdout().flush().map_err(ShadwError::Io)?;

    let mut input = String::new();
    io::stdin().read_line(&mut input).map_err(ShadwError::Io)?;
    let input = input.trim();

    if input.is_empty() {
        return Ok(default.name.to_string());
    }

    // Accept number or name
    if let Ok(n) = input.parse::<usize>() {
        if n >= 1 && n <= all.len() {
            return Ok(all[n - 1].name.to_string());
        }
    }

    if let Some(spec) = models::get_model(input) {
        return Ok(spec.name.to_string());
    }

    Err(ShadwError::Other(format!(
        "unknown model '{input}'. Run `shadw model` to see available models."
    )))
}

fn append_gitignore(repo_root: &Path) -> Result<()> {
    let gitignore_path = repo_root.join(".gitignore");
    let entries = [".shadw/"];

    let mut contents = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path)?
    } else {
        String::new()
    };

    let mut changed = false;
    for entry in entries {
        if !contents.lines().any(|line| line.trim() == entry) {
            if !contents.is_empty() && !contents.ends_with('\n') {
                contents.push('\n');
            }
            contents.push_str(entry);
            contents.push('\n');
            changed = true;
        }
    }

    if changed {
        fs::write(&gitignore_path, contents)?;
    }

    Ok(())
}

fn install_pre_push_hook(repo_root: &Path) -> Result<()> {
    let hooks_dir = repo_root.join(".git/hooks");
    fs::create_dir_all(&hooks_dir)?;

    let hook_path = hooks_dir.join("pre-push");

    if hook_path.exists() {
        let existing = fs::read_to_string(&hook_path)?;
        if existing.contains("refs/notes/shadw") {
            // Already installed
            return Ok(());
        }
        // Append to existing hook
        let mut contents = existing;
        if !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(HOOK_SNIPPET);
        fs::write(&hook_path, contents)?;
    } else {
        fs::write(&hook_path, format!("#!/bin/sh\n{HOOK_SNIPPET}"))?;
    }

    // Make executable
    let perms = fs::Permissions::from_mode(0o755);
    fs::set_permissions(&hook_path, perms)?;

    Ok(())
}

const HOOK_SNIPPET: &str = r#"
# Shadw: push decision notes alongside code (only if notes exist)
if git rev-parse --verify refs/notes/shadw >/dev/null 2>&1; then
  git push origin refs/notes/shadw --no-verify >/dev/null 2>&1 || true
fi
"#;

/// Detect a GitHub handle from git config.
/// Tries `github.user` → `user.name` → `user.email` (local part).
fn detect_github_user(repo_root: &Path) -> String {
    for key in ["github.user", "user.name", "user.email"] {
        if let Ok(output) = std::process::Command::new("git")
            .args(["-C", &repo_root.to_string_lossy(), "config", "--get", key])
            .output()
        {
            if output.status.success() {
                let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !val.is_empty() {
                    // For email, extract the local part before @
                    let name = if key == "user.email" {
                        val.split('@').next().unwrap_or(&val).to_string()
                    } else {
                        val
                    };
                    return format!("@{name}");
                }
            }
        }
    }

    String::new()
}

fn install_github_action(repo_root: &Path) -> Result<()> {
    let workflows_dir = repo_root.join(".github/workflows");
    let workflow_path = workflows_dir.join("shadw.yml");

    if workflow_path.exists() {
        return Ok(());
    }

    fs::create_dir_all(&workflows_dir)?;
    fs::write(&workflow_path, include_str!("../templates/shadw.yml"))?;

    Ok(())
}
