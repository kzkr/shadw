use std::fs;

use crate::config::{self, ShadwConfig};
use crate::error::{Result, ShadwError};
use crate::extraction;
use crate::watcher::CapturedContext;

/// Re-run extraction for a commit whose context was already captured.
pub fn exec(hash: &str) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let repo_root = config::find_git_root(&cwd)?;
    let shadw = config::shadw_dir(&repo_root);

    if !shadw.exists() {
        return Err(ShadwError::NotInitialized);
    }

    let contexts_dir = shadw.join("contexts");

    // Find the context file by hash prefix
    let context_path = find_context_file(&contexts_dir, hash)?;
    let raw = fs::read_to_string(&context_path)
        .map_err(|e| ShadwError::Other(format!("failed to read context: {e}")))?;
    let context: CapturedContext = serde_json::from_str(&raw)
        .map_err(|e| ShadwError::Other(format!("failed to parse context: {e}")))?;

    let hash_short = &context.commit.hash[..context.commit.hash.len().min(8)];
    println!("Re-extracting decisions for {hash_short}...");
    println!(
        "  commit: {} ({} conversation entries)",
        context.commit.message,
        context.conversation.len()
    );

    let shadw_config = ShadwConfig::load(&repo_root).unwrap_or_default();
    let config = shadw_config.extraction_config();

    // Minimal tracing so extraction info!/warn! are visible
    tracing_subscriber::fmt()
        .with_target(false)
        .with_level(false)
        .without_time()
        .init();

    // Run extraction synchronously (same path as the daemon)
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| ShadwError::Other(format!("failed to create runtime: {e}")))?;

    rt.block_on(extraction::extract_and_save(
        context,
        contexts_dir,
        repo_root,
        config,
    ));

    Ok(())
}

fn find_context_file(
    contexts_dir: &std::path::Path,
    hash_prefix: &str,
) -> Result<std::path::PathBuf> {
    if !contexts_dir.exists() {
        return Err(ShadwError::Other("no contexts directory found".into()));
    }

    // Try direct path first: contexts/<first2chars>/<full_hash>.json
    for shard in fs::read_dir(contexts_dir)?.flatten() {
        if !shard.path().is_dir() {
            continue;
        }
        for entry in fs::read_dir(shard.path())?.flatten() {
            let path = entry.path();
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if stem.starts_with(hash_prefix) {
                    return Ok(path);
                }
            }
        }
    }

    Err(ShadwError::Other(format!(
        "no saved context found for hash prefix '{hash_prefix}'. \
         Context is only available for commits the daemon has already seen."
    )))
}
