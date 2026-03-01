use crate::daemon::registry::{self, DaemonStatus, RegistryTarget};
use crate::error::{Result, ShadwError};

pub fn exec(target: &str) -> Result<()> {
    let parsed = registry::parse_target(target);

    if matches!(parsed, RegistryTarget::All) {
        return Err(ShadwError::Other(
            "cannot remove all projects at once — too destructive. Remove them individually."
                .to_string(),
        ));
    }

    // Check if entry exists
    let reg = registry::load()?;
    let entry = registry::find(&reg, &parsed).ok_or_else(|| {
        ShadwError::ProjectNotFound(target.to_string())
    })?;

    // Stop daemon if running
    if entry.path.exists() {
        if let DaemonStatus::Running(_) = registry::entry_status(&entry) {
            println!("Stopping daemon for {} ...", registry::shorten_home(&entry.path));
            let _ = super::stop::stop_for_path(&entry.path);
        }
    }

    // Remove from registry
    registry::unregister(&parsed)?;

    println!(
        "Removed {} from registry. Git notes are preserved.",
        registry::shorten_home(&entry.path)
    );

    Ok(())
}
