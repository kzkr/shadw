use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ShadwError {
    #[error("not a git repository. Run `git init` first.")]
    NotGitRepo,

    #[error("already initialized: {0}/.shadw/ exists")]
    AlreadyInitialized(PathBuf),

    #[error("not initialized: run `shadw init` first")]
    NotInitialized,

    #[error("Shadw is already running (PID {0})")]
    DaemonAlreadyRunning(u32),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, ShadwError>;
