use crate::daemon::registry::{self, DaemonStatus};
use crate::error::Result;

pub fn exec() -> Result<()> {
    let reg = registry::load()?;

    if reg.daemons.is_empty() {
        println!("No projects registered. Run `shadw init` in a git repo to get started.");
        return Ok(());
    }

    // Header
    println!(
        "{:<4}  {:<40}  {:<10}  {}",
        "ID", "PROJECT", "STATUS", "UPTIME"
    );
    println!("{}", "-".repeat(70));

    let mut running = 0;

    for entry in &reg.daemons {
        let project = registry::shorten_home(&entry.path);
        let dir_exists = entry.path.exists();

        let (status_str, uptime_str) = if !dir_exists {
            ("\x1b[33m(missing)\x1b[0m".to_string(), String::from("-"))
        } else {
            match registry::entry_status(entry) {
                DaemonStatus::Running(_pid) => {
                    running += 1;
                    let uptime = entry
                        .started_at
                        .as_deref()
                        .map(registry::format_uptime)
                        .unwrap_or_else(|| String::from("-"));
                    ("\x1b[32mrunning\x1b[0m".to_string(), uptime)
                }
                DaemonStatus::Stopped => {
                    ("\x1b[31mstopped\x1b[0m".to_string(), String::from("-"))
                }
            }
        };

        println!("{:<4}  {:<40}  {:<21}  {}", entry.id, project, status_str, uptime_str);
    }

    println!();
    println!(
        "{} project{} registered, {} running",
        reg.daemons.len(),
        if reg.daemons.len() == 1 { "" } else { "s" },
        running
    );

    Ok(())
}
