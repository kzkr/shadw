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

/// Minimum conversation budget floor (chars) for very small context windows.
const MIN_CONVERSATION_BUDGET: usize = 2000;

/// Compute the conversation character budget from the model's context window size.
fn conversation_budget_from_n_ctx(n_ctx: u32) -> usize {
    // 4096 = generation token reserve (matches engine.rs)
    // 1000 = overhead estimate (system prompt ~700 + commit metadata ~200 + template ~100)
    // * 2 = conservative chars-per-token ratio
    let raw = (n_ctx as usize).saturating_sub(4096).saturating_sub(1000) * 2;
    raw.max(MIN_CONVERSATION_BUDGET)
}

async fn extract(
    context: &CapturedContext,
    config: &ExtractionConfig,
) -> Result<DecisionRecord, String> {
    // Skip the LLM if there's no conversation that actually relates to this commit.
    // For Claude Code, the watcher buffer may contain entries from unrelated sessions
    // (e.g., the developer is chatting about project A while committing in project B).
    // We check whether any conversation entry mentions at least one changed file.
    //
    // For workspace-scoped agents like Cursor, conversations are already filtered by
    // project, and users describe intent at a high level without mentioning exact
    // filenames — so the file correlation filter would incorrectly reject valid entries.
    let skip_file_filter = context.agent == "cursor";
    if !skip_file_filter
        && !has_relevant_conversation(&context.conversation, &context.commit.changed_files)
    {
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

    // Look up model spec to derive context budget
    let spec = crate::models::get_model(&config.model).ok_or_else(|| {
        format!(
            "unknown model '{}'. Run `shadw model` to see available models.",
            config.model
        )
    })?;
    let budget = conversation_budget_from_n_ctx(spec.n_ctx);

    let author = if config.author.is_empty() {
        "The developer".to_string()
    } else {
        config.author.clone()
    };
    let system = SYSTEM_PROMPT.replace("{author}", &author);
    let prompt = build_prompt(context, budget);
    let response = call_llm(&prompt, &system, config, spec.n_ctx).await?;
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

fn build_prompt(context: &CapturedContext, conversation_budget: usize) -> String {
    let mut prompt = String::with_capacity(conversation_budget + 2000);

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

    // Conversation — selected with head/middle/tail strategy
    prompt.push_str("\n## Conversation\n");

    if context.conversation.is_empty() {
        prompt.push_str("(no conversation captured for this commit)\n");
    } else {
        let selected = select_messages(
            &context.conversation,
            &context.commit.changed_files,
            conversation_budget,
        );
        for line in &selected {
            prompt.push_str(line);
        }
    }

    prompt
}

/// Per-message truncation cap (independent of model size — about per-entry signal density).
const MAX_PER_MSG: usize = 800;

/// Format a conversation entry as a prompt line.
/// Maps "assistant" → "AI" to prevent the model from echoing the word "assistant" in output.
fn format_entry(entry: &crate::watcher::ConversationEntry) -> String {
    let raw_role = entry.role.as_deref().unwrap_or(&entry.entry_type);
    let role = if raw_role == "assistant" { "AI" } else { raw_role };
    let preview = truncate(&entry.content_preview, MAX_PER_MSG);
    format!("[{role}]: {preview}\n")
}

/// Select conversation messages using a head/middle/tail strategy.
///
/// - Short conversations (<=6 messages): include everything
/// - Long conversations: reserve head (first 3) and tail (last 3), fill middle
///   with highest-priority messages that fit the budget
fn select_messages(
    conversation: &[crate::watcher::ConversationEntry],
    changed_files: &[String],
    budget: usize,
) -> Vec<String> {
    // Filter out entries with empty content (e.g. former tool_result-only messages)
    let eligible: Vec<(usize, &crate::watcher::ConversationEntry)> = conversation
        .iter()
        .enumerate()
        .filter(|(_, e)| !e.content_preview.trim().is_empty())
        .collect();

    if eligible.is_empty() {
        return vec!["(no conversation captured for this commit)\n".to_string()];
    }

    // Short conversations: include everything that fits
    if eligible.len() <= 6 {
        let mut result = Vec::new();
        let mut total = 0;
        for (_, entry) in &eligible {
            let line = format_entry(entry);
            if total + line.len() > budget {
                break;
            }
            total += line.len();
            result.push(line);
        }
        return result;
    }

    // Long conversations: head/middle/tail strategy
    let head_count = 3;
    let tail_count = 3;
    let head: Vec<(usize, &crate::watcher::ConversationEntry)> =
        eligible[..head_count].to_vec();
    let tail: Vec<(usize, &crate::watcher::ConversationEntry)> =
        eligible[eligible.len() - tail_count..].to_vec();
    let middle_pool: Vec<(usize, &crate::watcher::ConversationEntry)> =
        eligible[head_count..eligible.len() - tail_count].to_vec();

    // Reserve budget for head and tail first
    let mut head_lines: Vec<(usize, String)> = Vec::new();
    let mut tail_lines: Vec<(usize, String)> = Vec::new();
    let mut reserved = 0;

    for (idx, entry) in &head {
        let line = format_entry(entry);
        reserved += line.len();
        head_lines.push((*idx, line));
    }
    for (idx, entry) in &tail {
        let line = format_entry(entry);
        reserved += line.len();
        tail_lines.push((*idx, line));
    }

    // If head+tail already exceed budget, just return what fits
    if reserved > budget {
        let mut result = Vec::new();
        let mut total = 0;
        for (_, line) in head_lines.iter().chain(tail_lines.iter()) {
            if total + line.len() > budget {
                break;
            }
            total += line.len();
            result.push(line.clone());
        }
        return result;
    }

    // Score and select middle messages
    let file_basenames: Vec<String> = changed_files
        .iter()
        .filter_map(|f| std::path::Path::new(f).file_name())
        .map(|n| n.to_string_lossy().to_lowercase())
        .collect();

    let mut scored: Vec<(usize, String, i32)> = Vec::new();
    for (idx, entry) in &middle_pool {
        let line = format_entry(entry);
        let mut score: i32 = 0;
        // User messages contain reasoning/intent
        if entry.role.as_deref() == Some("user") || entry.entry_type == "user" {
            score += 2;
        }
        // Messages referencing changed files are directly relevant
        let content_lower = entry.content_preview.to_lowercase();
        if file_basenames
            .iter()
            .any(|name| content_lower.contains(name.as_str()))
        {
            score += 1;
        }
        scored.push((*idx, line, score));
    }

    // Sort by score descending, then by original index ascending (prefer earlier messages)
    scored.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));

    // Greedily fill remaining budget (use continue, not break, to try smaller messages)
    let mut middle_remaining = budget - reserved;
    let mut middle_selected: Vec<(usize, String)> = Vec::new();
    for (idx, line, _score) in scored {
        if line.len() <= middle_remaining {
            middle_remaining -= line.len();
            middle_selected.push((idx, line));
        }
    }

    // Sort middle by original index for chronological output
    middle_selected.sort_by_key(|(idx, _)| *idx);

    // Merge all selections in chronological order, inserting gap markers
    let mut all: Vec<(usize, String)> = Vec::new();
    all.extend(head_lines);
    all.extend(middle_selected);
    all.extend(tail_lines);
    all.sort_by_key(|(idx, _)| *idx);

    // Deduplicate (head/tail may overlap with middle in edge cases)
    all.dedup_by_key(|(idx, _)| *idx);

    // Build final output with gap markers between non-consecutive messages
    let mut result = Vec::new();
    let mut prev_idx: Option<usize> = None;
    for (idx, line) in &all {
        if let Some(prev) = prev_idx {
            if *idx > prev + 1 {
                result.push("[...]\n".to_string());
            }
        }
        result.push(line.clone());
        prev_idx = Some(*idx);
    }

    result
}

async fn call_llm(prompt: &str, system_prompt: &str, config: &ExtractionConfig, n_ctx: u32) -> Result<String, String> {
    use crate::models;

    let spec = models::get_model(&config.model).ok_or_else(|| {
        format!(
            "unknown model '{}'. Run `shadw model` to see available models.",
            config.model
        )
    })?;

    let model_path = models::ensure_model(spec)?;

    let system = system_prompt.to_string();
    let user = prompt.to_string();
    let prefix = r#"{"decisions":[{"#.to_string();

    tokio::task::spawn_blocking(move || {
        models::infer(&model_path, &system, &user, &prefix, 8192, n_ctx)
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

## Voice — READ THIS FIRST

There are exactly two actors in every summary:
- **{author}** — the developer. Use "{author}" for anything the developer initiated, decided, or asked for.
- **"we"** — the collaborative work between {author} and the AI tool.

NEVER reference the AI as a separate actor. The conversation log labels AI messages as [AI] — ignore that label. Do NOT write "the AI suggested", "the assistant recommended", "the tool proposed", or any similar phrasing. The word "assistant" must NEVER appear in the output. Instead, use passive voice ("it was suggested", "the option came up") or attribute to "we" ("we considered", "we settled on").

BAD: "The assistant suggested moving the animation into a config block."
GOOD: "During the rewrite, moving the animation into a Tailwind config block came up as the cleaner approach."
GOOD: "We moved the custom float animation into a Tailwind config block."

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

## Rules

- NEVER write "the assistant", "the AI", "the tool", "the model", "the bot", or "I". These words must not appear in the output. Use {author}, "we", or passive voice instead.
- Write naturally — vary how you start sentences. Do NOT always open with "{author} wanted..." or "We discussed...". Mix it up: lead with the problem, the context, the constraint, or the insight.
- The title must frame a choice with a visible alternative. If the conversation discussed options, name them.
- Be technically specific — mention actual library names, API patterns, file structures, config choices when they came up in conversation.
- DO NOT repeat or rephrase the commit message as the title.
- DO NOT say the same thing in both the title and the summary — they should complement each other.
- If the conversation is empty or contains no meaningful developer-AI discussion, return {"decisions": []}. A commit with ZERO conversation entries means the developer made the change without AI assistance — there is NO invisible story to tell. Do NOT invent reasoning or fabricate a narrative from the diff alone. The diff is already visible to reviewers; your job is ONLY to surface what happened in conversation.

Respond with ONLY valid JSON:
{"decisions": [{"title": "...", "summary": "..."}]}"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watcher::ConversationEntry;

    fn make_entry(role: &str, content: &str) -> ConversationEntry {
        ConversationEntry {
            entry_type: role.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            session_id: "test".to_string(),
            git_branch: "main".to_string(),
            role: Some(role.to_string()),
            content_preview: content.to_string(),
        }
    }

    #[test]
    fn empty_conversation() {
        let result = select_messages(&[], &["src/foo.rs".to_string()], 5000);
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("no conversation"));
    }

    #[test]
    fn short_conversation_all_included() {
        let entries = vec![
            make_entry("user", "add a login page"),
            make_entry("assistant", "sure, I'll create login.rs"),
            make_entry("user", "use JWT for auth"),
        ];
        let result = select_messages(&entries, &[], 50000);
        assert_eq!(result.len(), 3);
        // No gap markers for consecutive messages
        assert!(!result.iter().any(|l| l.contains("[...]")));
    }

    #[test]
    fn long_conversation_has_head_tail_and_gaps() {
        let mut entries = Vec::new();
        for i in 0..20 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            entries.push(make_entry(role, &format!("message number {i}")));
        }
        let result = select_messages(&entries, &[], 50000);
        // First message should be head[0]
        assert!(result[0].contains("message number 0"));
        // Last message should be tail[-1]
        assert!(result.last().unwrap().contains("message number 19"));
        // Should have at least one gap marker since not all middle messages may be consecutive
        // (with a large budget they may all fit, so check that gap markers exist between non-consecutive)
        let has_gap = result.iter().any(|l| l.contains("[...]"));
        // With large budget all should fit, so gaps appear only if indices are non-consecutive
        // In this case all 20 messages fit, so there should be no gaps
        assert!(!has_gap, "all messages fit, no gaps expected");
    }

    #[test]
    fn user_messages_prioritized_over_tool_use_in_middle() {
        let mut entries = Vec::new();
        // Head: 0,1,2
        entries.push(make_entry("user", "start of conversation"));
        entries.push(make_entry("assistant", "head assistant msg"));
        entries.push(make_entry("user", "head user msg 2"));
        // Middle: 3..16 (mix of user and assistant)
        for i in 3..17 {
            let role = if i % 2 == 0 { "user" } else { "assistant" };
            entries.push(make_entry(role, &format!("middle message {i} with some content")));
        }
        // Tail: 17,18,19
        entries.push(make_entry("assistant", "tail assistant msg"));
        entries.push(make_entry("user", "tail user msg"));
        entries.push(make_entry("assistant", "final resolution"));

        // Use a budget tight enough that not all middle messages fit
        let result = select_messages(&entries, &[], 2000);

        // Head should be present
        assert!(result.iter().any(|l| l.contains("start of conversation")));
        // Tail should be present
        assert!(result.iter().any(|l| l.contains("final resolution")));

        // Count how many middle user vs assistant messages made it
        let middle_user_count = result
            .iter()
            .filter(|l| l.contains("[user]") && l.contains("middle message"))
            .count();
        let middle_assistant_count = result
            .iter()
            .filter(|l| l.contains("[assistant]") && l.contains("middle message"))
            .count();

        // User messages should be prioritized (more of them included)
        assert!(
            middle_user_count >= middle_assistant_count,
            "user messages ({middle_user_count}) should be >= assistant ({middle_assistant_count})"
        );
    }

    #[test]
    fn empty_content_entries_skipped() {
        let entries = vec![
            make_entry("user", "real message"),
            make_entry("assistant", ""),
            make_entry("assistant", "   "),
            make_entry("user", "another real message"),
        ];
        let result = select_messages(&entries, &[], 50000);
        // Only 2 real messages should be included (empty ones filtered)
        let non_gap_lines: Vec<_> = result.iter().filter(|l| !l.contains("[...]")).collect();
        assert_eq!(non_gap_lines.len(), 2);
    }

    #[test]
    fn budget_respected_with_large_messages() {
        let big_msg = "x".repeat(500);
        let entries = vec![
            make_entry("user", &big_msg),
            make_entry("assistant", &big_msg),
            make_entry("user", &big_msg),
            make_entry("assistant", &big_msg),
        ];
        // Budget of 1200 chars: should fit at most ~2 messages (each ~510+ chars with role prefix)
        let result = select_messages(&entries, &[], 1200);
        let content_lines: Vec<_> = result.iter().filter(|l| !l.contains("[...]") && !l.contains("no conversation")).collect();
        assert!(content_lines.len() <= 3, "should not exceed budget, got {} lines", content_lines.len());
    }

    #[test]
    fn file_relevance_boost_in_middle() {
        let mut entries = Vec::new();
        // Head
        entries.push(make_entry("user", "start"));
        entries.push(make_entry("assistant", "ok"));
        entries.push(make_entry("user", "let's go"));
        // Middle — one mentions foo.rs, one doesn't
        entries.push(make_entry("assistant", "working on something unrelated"));
        entries.push(make_entry("assistant", "editing src/foo.rs now"));
        entries.push(make_entry("assistant", "another unrelated thing"));
        entries.push(make_entry("assistant", "yet another unrelated thing"));
        // Tail
        entries.push(make_entry("user", "looks good"));
        entries.push(make_entry("assistant", "done"));
        entries.push(make_entry("user", "thanks"));

        // Tight budget so only ~1 middle message fits
        let result = select_messages(
            &entries,
            &["src/foo.rs".to_string()],
            900,
        );
        // The file-relevant middle message should be selected
        let has_foo = result.iter().any(|l| l.contains("foo.rs"));
        assert!(has_foo, "file-relevant message should be prioritized");
    }

    #[test]
    fn budget_adapts_to_n_ctx() {
        // gpt-oss: 16384
        let budget = conversation_budget_from_n_ctx(16384);
        assert_eq!(budget, (16384 - 4096 - 1000) * 2); // 22576

        // Hypothetical small model: 4096
        let budget_small = conversation_budget_from_n_ctx(4096);
        assert_eq!(budget_small, MIN_CONVERSATION_BUDGET); // clamped to floor

        // Edge case: n_ctx smaller than reserves
        let budget_tiny = conversation_budget_from_n_ctx(2000);
        assert_eq!(budget_tiny, MIN_CONVERSATION_BUDGET);
    }
}
