# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Shadw

CLI tool that captures the *why* behind code changes. It runs a background daemon that observes AI-developer conversations (Claude Code, Cursor), correlates them with git commits, extracts structured decision records using a local LLM, and attaches them as git notes. A GitHub Action then surfaces these decisions as PR comments.

Key properties: free, local-first, private by default. Conversations never leave the machine — only extracted decision summaries are shared via git notes.

## Build & Test

```bash
cargo build --release        # release binary at target/release/shadw
cargo test                   # 20 integration tests in tests/cli_test.rs
cargo test <test_name>       # run a single test, e.g. cargo test init_in_git_repo
RUST_LOG=shadw=debug cargo run -- start --foreground  # run daemon with debug logging
```

CUDA support: `cargo build --release --features cuda`

Pin `time` crate to 0.3.36 if you hit Rust 1.85 stable compatibility issues.

## Architecture

The system has three runtime modes that share the same binary:

**CLI commands** (`src/cli/`) — user-facing entry points. `main.rs` dispatches via clap subcommands: `init`, `ls`, `start`, `stop`, `restart`, `rm`, `status`, `model`, `agent`, `retry`, `upgrade`.

**Background daemon** (`src/daemon/server.rs`) — started by `shadw start`, runs a tokio event loop watching two filesystem paths via `notify`:
1. Agent conversation dir (via `AgentWatcher` trait) — new conversation entries are buffered by the agent-specific watcher (`watcher/conversation.rs` for Claude Code, `watcher/cursor.rs` for Cursor)
2. `.git/refs/heads/` — ref changes detected by `GitWatcher`, which triggers `capture_context()`

On commit detection: drain conversation buffer → save context JSON → spawn async extraction task.

**Extraction pipeline** (`src/extraction/mod.rs`) — runs a local GGUF model via `llama-cpp-2`:
1. Filter: `has_relevant_conversation()` checks file-name correlation between conversation and commit
2. Build prompt with commit metadata + trimmed conversation (5000 char budget)
3. Local inference with structured JSON output (prefix-forced)
4. Filter: `is_real_decision()` catches filler responses
5. Write git note → push notes if branch is on remote → delete context file

**Model management** (`src/models/`) — downloads GGUF files from HuggingFace, symlinks to `~/.shadw/models/`. `engine.rs` handles batched prompt processing and sampling via llama-cpp-2 bindings.

## Data Flow

```
Agent sessions → AgentWatcher (buffer) → capture_context() → context JSON
     +
git refs/heads change → GitWatcher → CommitInfo ──────────────────────┘
                                                                      ↓
                                                        extract_and_save()
                                                              ↓
                                                   local LLM (GGUF)
                                                              ↓
                                                    DecisionRecord
                                                              ↓
                                                  git notes --ref shadw
                                                              ↓
                                                  git push (if remote)
```

## Key Design Decisions

- One decision per commit — the model merges related work into a single title+summary
- Decision records focus on the developer's intent, not the AI's implementation details
- Empty conversation buffer → early return, no context file, no extraction (server.rs)
- Defense-in-depth: file correlation filter, filler-title filter, empty-decisions-no-note — all in extraction/mod.rs
- Context files are ephemeral (gitignored, deleted after successful note write)
- Git notes pushed via pre-push hook (installed by `shadw init`) and by the daemon after extraction
- PR comments use `<!-- shadw-pr-decisions -->` HTML marker for upsert (create or update)

## File Layout (non-obvious)

- `.shadw/` is gitignored entirely — decision records live in git notes, not files
- `src/templates/shadw.yml` — GitHub Action workflow template, installed by `shadw init`
- `src/util.rs` — shared utilities (truncate)
- Config lives at `.shadw/config.toml`, cursor state at `.shadw/state/cursor.json`
- Models stored globally at `~/.shadw/models/` (symlinked from hf-hub cache)
- Global daemon registry at `~/.shadw/daemons.toml` — maps project IDs → paths, enables `shadw ls` from anywhere
