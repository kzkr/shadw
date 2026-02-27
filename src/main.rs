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
    /// Start watching for changes
    Start {
        /// Run in foreground (for development/debugging)
        #[arg(long)]
        foreground: bool,
    },
    /// Stop watching for changes
    Stop,
    /// Restart the daemon
    Restart,
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
}

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => cli::init::exec(),
        Commands::Start { foreground } => cli::start::exec(foreground),
        Commands::Stop => cli::stop::exec(),
        Commands::Restart => cli::restart::exec(),
        Commands::Use { model } => cli::use_model::exec(model.as_deref()),
        Commands::Status => cli::status::exec(),
        Commands::Retry { hash } => cli::retry::exec(&hash),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
