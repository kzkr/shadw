use std::path::Path;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::ExtractionConfig;
use crate::util::truncate;
use crate::watcher::CapturedContext;

/// A structured decision record extracted from a commit context.
#[derive(Debug, Serialize, Deserialize)]
pub struct DecisionRecord {
    pub commit_hash: String,
    pub commit_message: String,
    pub branch: String,
    pub timestamp: String,
    pub files_changed: Vec<String>,
    pub decisions: Vec<Decision>,
    pub extracted_at: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    /// Short headline: the decision, not the commit message
    pub title: String,
    /// One paragraph telling the full story: what the developer wanted,
    /// what options were explored, what was chosen and why.
    pub summary: String,
}

/// Check whether any conversation entry actually relates to the commit's changed files.
/// Returns false if the conversation is empty or entirely about unrelated files.
fn has_relevant_conversation(
    conversation: &[crate::watcher::ConversationEntry],
    changed_files: &[String],
) -> bool {
    if conversation.is_empty() {
        return false;
    }
    if changed_files.is_empty() {
        // No file info to correlate — assume relevant
        return true;
    }

    // Extract basenames of changed files for matching
    let names: Vec<String> = changed_files
        .iter()
        .filter_map(|f| std::path::Path::new(f).file_name())
        .map(|n| n.to_string_lossy().to_lowercase())
        .collect();

    conversation.iter().any(|entry| {
        let content = entry.content_preview.to_lowercase();
        names.iter().any(|name| content.contains(name.as_str()))
    })
}

/// Filter out non-decisions the model returns instead of an empty array.
fn is_real_decision(d: &Decision) -> bool {
    let title = d.title.to_lowercase();
    let dominated_by_filler = title.contains("no decision")
        || title.contains("no meaningful decision")
        || title.contains("not a decision")
        || title.contains("no design choice")
        || title.contains("no choice")
        || title.contains("nothing to document")
        || d.title.is_empty();
    !dominated_by_filler
}

/// Response shape we ask the LLM to produce.
#[derive(Debug, Deserialize)]
struct LlmResponse {
    #[serde(default)]
    decisions: Vec<Decision>,
}

/// Run extraction on a captured context, write a git note, push notes
/// if the branch is on remote, and clean up the context file on success.
/// Spawned as a background tokio task — errors are logged, not propagated.
pub async fn extract_and_save(
    context: CapturedContext,
    contexts_dir: std::path::PathBuf,
    repo_root: std::path::PathBuf,
    config: ExtractionConfig,
) {
    let hash_short = &context.commit.hash[..context.commit.hash.len().min(8)];
    info!("extracting decisions for {hash_short}...");

    match extract(&context, &config).await {
        Ok(record) => {
            let count = record.decisions.len();
            info!("extracted {} decision(s) for {hash_short}", count);
            for d in &record.decisions {
                info!("  → {}", d.title);
            }

            if count == 0 {
                info!("no decisions to record for {hash_short}, skipping note");
                cleanup_context(&contexts_dir, &context.commit.hash);
                return;
            }

            // Write git note
            match serde_json::to_string(&record) {
                Ok(json) => {
                    if let Err(e) = write_git_note(&repo_root, &record.commit_hash, &json) {
                        warn!("failed to write git note: {e}");
                    } else {
                        info!("git note attached to {hash_short}");
                        push_notes_if_remote(&repo_root, &context.commit.branch);
                        // Context served its purpose — clean up
                        cleanup_context(&contexts_dir, &context.commit.hash);
                    }
                }
                Err(e) => warn!("failed to serialize record for note: {e}"),
            }
        }
        Err(e) => {
            warn!("extraction failed for {hash_short}: {e}");
        }
    }
}

async fn extract(
    context: &CapturedContext,
    config: &ExtractionConfig,
) -> Result<DecisionRecord, String> {
    // Skip the LLM if there's no conversation that actually relates to this commit.
    // The watcher buffer may contain entries from unrelated Claude Code sessions
    // (e.g., the developer is chatting about project A while committing in project B).
    // We check whether any conversation entry mentions at least one changed file.
    if !has_relevant_conversation(&context.conversation, &context.commit.changed_files) {
        info!("no relevant conversation for this commit, skipping extraction");
        return Ok(DecisionRecord {
            commit_hash: context.commit.hash.clone(),
            commit_message: context.commit.message.clone(),
            branch: context.commit.branch.clone(),
            timestamp: context.commit.timestamp.clone(),
            files_changed: context.commit.changed_files.clone(),
            decisions: vec![],
            extracted_at: chrono::Utc::now().to_rfc3339(),
            model: config.model.clone(),
        });
    }

    let author = if config.author.is_empty() {
        "The developer".to_string()
    } else {
        config.author.clone()
    };
    let system = SYSTEM_PROMPT.replace("{author}", &author);
    let prompt = build_prompt(context);
    let response = call_llm(&prompt, &system, config).await?;
    let parsed: LlmResponse = parse_json_response(&response)?;

    Ok(DecisionRecord {
        commit_hash: context.commit.hash.clone(),
        commit_message: context.commit.message.clone(),
        branch: context.commit.branch.clone(),
        timestamp: context.commit.timestamp.clone(),
        files_changed: context.commit.changed_files.clone(),
        decisions: parsed.decisions.into_iter().filter(is_real_decision).collect(),
        extracted_at: chrono::Utc::now().to_rfc3339(),
        model: config.model.clone(),
    })
}

fn build_prompt(context: &CapturedContext) -> String {
    let mut prompt = String::with_capacity(8000);

    // Commit details
    prompt.push_str("## Git Commit\n");
    prompt.push_str(&format!("Hash: {}\n", &context.commit.hash[..context.commit.hash.len().min(12)]));
    prompt.push_str(&format!("Message: {}\n", context.commit.message));
    prompt.push_str(&format!("Branch: {}\n", context.commit.branch));
    if !context.commit.changed_files.is_empty() {
        prompt.push_str("Files changed:\n");
        for f in &context.commit.changed_files {
            prompt.push_str(&format!("  - {f}\n"));
        }
    }

    // Conversation — trimmed to fit context window
    prompt.push_str("\n## Conversation\n");

    if context.conversation.is_empty() {
        prompt.push_str("(no conversation captured for this commit)\n");
    } else {
        let mut messages: Vec<String> = Vec::new();
        let mut total_chars = 0;
        const MAX_CHARS: usize = 5000;
        const MAX_PER_MSG: usize = 600;

        for entry in context.conversation.iter().rev() {
            let role = entry.role.as_deref().unwrap_or(&entry.entry_type);
            let preview = truncate(&entry.content_preview, MAX_PER_MSG);
            let line = format!("[{role}]: {preview}\n");

            if total_chars + line.len() > MAX_CHARS {
                break;
            }
            total_chars += line.len();
            messages.push(line);
        }
        messages.reverse();

        for msg in &messages {
            prompt.push_str(msg);
        }
    }

    prompt
}

async fn call_llm(prompt: &str, system_prompt: &str, config: &ExtractionConfig) -> Result<String, String> {
    use crate::models;

    let spec = models::get_model(&config.model).ok_or_else(|| {
        format!(
            "unknown model '{}'. Run `shadw use` to see available models.",
            config.model
        )
    })?;

    let model_path = models::ensure_model(spec)?;

    let system = system_prompt.to_string();
    let user = prompt.to_string();
    let prefix = r#"{"decisions":[{"#.to_string();

    tokio::task::spawn_blocking(move || {
        models::infer(&model_path, &system, &user, &prefix, 8192)
    })
    .await
    .map_err(|e| format!("inference task panicked: {e}"))?
}

fn parse_json_response(response: &str) -> Result<LlmResponse, String> {
    if let Ok(parsed) = serde_json::from_str::<LlmResponse>(response) {
        return Ok(parsed);
    }

    if let Some(start) = response.find("```json") {
        if let Some(end) = response[start + 7..].find("```") {
            let json_str = &response[start + 7..start + 7 + end];
            if let Ok(parsed) = serde_json::from_str::<LlmResponse>(json_str.trim()) {
                return Ok(parsed);
            }
        }
    }

    if let Some(start) = response.find('{') {
        if let Some(end) = response.rfind('}') {
            let json_str = &response[start..=end];
            if let Ok(parsed) = serde_json::from_str::<LlmResponse>(json_str) {
                return Ok(parsed);
            }
        }
    }

    // Try to repair truncated JSON — model may have hit token limit.
    if let Some(start) = response.find("{\"decisions\"") {
        let json = &response[start..];

        // Try closing as-is with various bracket suffixes
        for suffix in ["\"}]}", "\"]}", "\"]}]}", "}]}", "]}", "}", ""] {
            let attempt = format!("{json}{suffix}");
            if let Ok(parsed) = serde_json::from_str::<LlmResponse>(&attempt) {
                warn!("repaired truncated JSON response");
                return Ok(parsed);
            }
        }

        // Try trimming back to the last quote, then closing
        if let Some(last_q) = json.rfind('"') {
            let trimmed = &json[..=last_q];
            for suffix in ["}]}", "]}", "}", ",\"summary\":\"\"}]}", ""] {
                let attempt = format!("{trimmed}{suffix}");
                if let Ok(parsed) = serde_json::from_str::<LlmResponse>(&attempt) {
                    warn!("repaired truncated JSON (trimmed to last complete string)");
                    return Ok(parsed);
                }
            }
        }
    }

    Err(format!(
        "could not parse LLM response as JSON: {}",
        truncate(response, 300)
    ))
}

/// Remove the context file after a successful note write.
fn cleanup_context(contexts_dir: &Path, commit_hash: &str) {
    let prefix = &commit_hash[..2.min(commit_hash.len())];
    let path = contexts_dir.join(prefix).join(format!("{commit_hash}.json"));
    if path.exists() {
        if let Err(e) = std::fs::remove_file(&path) {
            warn!("failed to remove context file: {e}");
        } else {
            info!("context cleaned up for {}", &commit_hash[..8.min(commit_hash.len())]);
        }
    }
}

// ---------------------------------------------------------------------------
// Git notes
// ---------------------------------------------------------------------------

/// Attach a decision record as a git note on the commit.
fn write_git_note(repo_root: &Path, commit_hash: &str, json: &str) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "notes",
            "--ref",
            "shadw",
            "add",
            "-f", // overwrite if note already exists
            "-m",
            json,
            commit_hash,
        ])
        .output()
        .map_err(|e| format!("failed to run git notes: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git notes failed: {stderr}"));
    }

    Ok(())
}

/// If the branch has been pushed to origin, push the notes ref too.
fn push_notes_if_remote(repo_root: &Path, branch: &str) {
    // Check if this branch exists on the remote
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "ls-remote",
            "--heads",
            "origin",
            branch,
        ])
        .output();

    let has_remote = output
        .as_ref()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false);

    if !has_remote {
        info!("branch '{branch}' not on remote, skipping notes push");
        return;
    }

    info!("pushing notes to origin...");
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_root.to_string_lossy(),
            "push",
            "origin",
            "refs/notes/shadw",
            "--no-verify",
        ])
        .output();

    match output {
        Ok(o) if o.status.success() => {
            info!("notes pushed to origin");
        }
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            warn!("notes push failed (will retry on next push): {stderr}");
        }
        Err(e) => {
            warn!("failed to push notes: {e}");
        }
    }
}

const SYSTEM_PROMPT: &str = r#"You surface the invisible reasoning behind code changes — the thinking, exploring, and deciding that happened in a developer-AI conversation BEFORE the code was written.

You are NOT a code reviewer. You do NOT describe what the code does. A reviewer can read the diff. Your job is to tell the story they CAN'T see: what {author} was trying to solve, what alternatives came up in conversation, and why this path won.

IMPORTANT: The developer is ALWAYS called {author}. Never use any other name, even if a different name appears in the conversation or commit data.

## Output format

ONE decision per commit with two fields:

- title: The CHOICE made, framed as "X over Y" or "X instead of Y" whenever the conversation reveals alternatives. Start with "Chose", "Went with", "Opted for", "Switched to".
  BAD: "Add Rentals page" (describes the code, not the thinking)
  BAD: "Opted for a reusable component" (over what? too vague)
  GOOD: "Chose a shared PropertyCard over duplicating markup per page"
  GOOD: "Went with manual meta tags over a Vue-SEO plugin for zero runtime overhead"
- summary: A natural, technically rich narrative structured in 2-3 short paragraphs, separated by \n\n. Each paragraph has a role:
  1. **Context** — What {author} was trying to solve: the goal, the constraint, or what triggered the change. Set the scene.
  2. **Debate** — The specific alternatives, technologies, or approaches that came up in conversation. Why this path won — the actual reasoning (performance, simplicity, ecosystem fit, etc.).
  3. **Detail** (optional) — Any notable technical specifics a reviewer would find interesting: APIs chosen, patterns applied, config choices, tradeoffs accepted.

Aim for 5-8 sentences across the paragraphs. Each paragraph should be 2-3 sentences. Do NOT pad with filler — if a commit is simple, 2 shorter paragraphs is fine. If {author} gave a direct instruction with no real discussion, a single paragraph of 1-2 sentences is enough.

Write like you're a senior engineer explaining a decision to a teammate over coffee — natural, specific, technically grounded. Vary your sentence structure. Do NOT use the same opening pattern for every summary.

Use {author} for what the developer initiated. Use "we" for the collaborative work.

## Rules

- ALWAYS refer to the developer as {author} — never use any other name, "the developer", "the assistant", "the AI", "the team", or "I". The word "assistant" must NEVER appear in the output.
- Write naturally — vary how you start sentences. Do NOT always open with "{author} wanted..." or "We discussed...". Mix it up: lead with the problem, the context, the constraint, or the insight.
- The title must frame a choice with a visible alternative. If the conversation discussed options, name them.
- Be technically specific — mention actual library names, API patterns, file structures, config choices when they came up in conversation.
- DO NOT repeat or rephrase the commit message as the title
- DO NOT say the same thing in both the title and the summary — they should complement each other
- If the conversation is empty or contains no meaningful developer-AI discussion, return {"decisions": []}. A commit with ZERO conversation entries means the developer made the change without AI assistance — there is NO invisible story to tell. Do NOT invent reasoning or fabricate a narrative from the diff alone. The diff is already visible to reviewers; your job is ONLY to surface what happened in conversation.

Respond with ONLY valid JSON:
{"decisions": [{"title": "...", "summary": "..."}]}"#;
