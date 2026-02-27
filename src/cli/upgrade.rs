use std::process::Command;

use crate::error::{Result, ShadwError};

const INSTALL_URL: &str = "https://raw.githubusercontent.com/kzkr/shadw/main/install.sh";

pub fn exec() -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    println!("Current version: v{current}");
    println!("Fetching latest release...");

    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("curl -fsSL {INSTALL_URL} | sh"))
        .status()
        .map_err(|e| ShadwError::Other(format!("failed to run installer: {e}")))?;

    if !status.success() {
        return Err(ShadwError::Other("upgrade failed".to_string()));
    }

    Ok(())
}
