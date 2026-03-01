mod cli;
mod config;
mod daemon;
mod error;
mod extraction;
mod models;
mod util;
mod watcher;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "shadw", version, about = "Capture the why behind code changes")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize Shadw in the current git repository
    Init,
    /// List all projects and daemon status
    Ls,
    /// Start watching for changes
    Start {
        /// Run in foreground (for development/debugging)
        #[arg(long)]
        foreground: bool,
        /// Target: project ID, path, or "all"
        target: Option<String>,
    },
    /// Stop watching for changes
    Stop {
        /// Target: project ID, path, or "all"
        target: Option<String>,
    },
    /// Restart the daemon
    Restart {
        /// Target: project ID, path, or "all"
        target: Option<String>,
    },
    /// Unregister a project from the global registry
    Rm {
        /// Target: project ID or path
        target: String,
    },
    /// Select or list available models
    Use {
        /// Model name (omit to list available models)
        model: Option<String>,
    },
    /// Show project status
    Status,
    /// Re-extract decisions for a previously failed commit
    Retry {
        /// Commit hash (or prefix)
        hash: String,
    },
    /// Upgrade to the latest release
    Upgrade,
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => cli::init::exec(),
        Commands::Ls => cli::ls::exec(),
        Commands::Start { foreground, target } => {
            cli::start::exec(foreground, target.as_deref())
        }
        Commands::Stop { target } => cli::stop::exec(target.as_deref()),
        Commands::Restart { target } => cli::restart::exec(target.as_deref()),
        Commands::Rm { target } => cli::rm::exec(&target),
        Commands::Use { model } => cli::use_model::exec(model.as_deref()),
        Commands::Status => cli::status::exec(),
        Commands::Retry { hash } => cli::retry::exec(&hash),
        Commands::Upgrade => cli::upgrade::exec(),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
