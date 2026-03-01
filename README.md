<p align="center">
  <img height="100" src="https://shadw.dev/github-icon.png" alt="Shadw — Every code change has a story. Shadw captures it."/>
</p>

<h1 align="center">Shadw</h1>

<p align="center">
  <a href="https://github.com/kzkr/shadw/releases"><img src="https://img.shields.io/github/v/release/kzkr/shadw?style=flat-square&color=blue" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
</p>

A git diff shows what changed. Shadw shows why.

When you work with AI coding agents, the reasoning, alternatives explored, and tradeoffs debated vanish the moment the session ends. Shadw runs locally, watches your conversations and commits, and surfaces a **Decision Trail** on your pull requests.

> ⚠️ Under active development. CLI flags and the decision record format may change between releases.

## What you get

On every PR, a collapsible decision trail appears as a comment:

<details>
<summary><strong><code>f4a21c7</code></strong> Switched from REST polling to WebSockets for real-time dashboard updates</summary>
<br>

@sarah flagged that the dashboard was hammering the API with 2-second polls across 50+ widgets. We considered SSE but needed bidirectional communication for filters. WebSockets cut server load by 80% and gave us sub-100ms updates. The tradeoff is reconnection logic on flaky networks — solved with exponential backoff and a visible connection status indicator.

</details>

<details>
<summary><strong><code>a83ef02</code></strong> Kept Stripe webhooks idempotent instead of adding a job queue for payments</summary>
<br>

@james wanted to guarantee no double charges during peak traffic. A job queue (Bull, SQS) would add infrastructure and latency. Instead, we store a unique idempotency key per webhook event and check it before processing. Simpler to debug, no new service to monitor, and Stripe already retries on failure. If we ever need async processing, we can layer it on later without changing the core flow.

</details>

<details>
<summary><strong><code>d19b4e6</code></strong> Chose row-level security in Postgres over application-level tenant isolation</summary>
<br>

@priya needed multi-tenant data isolation that couldn't be bypassed by a missed WHERE clause. We debated separate schemas per tenant, but that breaks connection pooling and makes migrations painful at scale. RLS policies on a tenant_id column enforce isolation at the database level — every query is automatically scoped. One less thing the application layer can get wrong.

</details>

Decisions live in [git notes](https://git-scm.com/docs/git-notes) — portable, versioned, zero lock-in.

## Install

```bash
curl -fsSL https://shadw.dev/install.sh | bash
cd your-project && shadw init
```

That's it. Shadw is now watching. `shadw init` downloads the model, installs hooks, and starts the daemon.

Requirements: macOS (Apple Silicon & Intel) or Linux (x86_64).

<details>
<summary>Build from source</summary>

```bash
cargo build --release
```

Requires Rust 1.85+ and cmake.

</details>

## How it works

**1. You code with AI** — a background daemon watches your agent's session files.

**2. You commit** — a local LLM reads the conversation and extracts key decisions.

**3. Notes attach** — decision records are written as git notes on `refs/notes/shadw`.

**4. PRs get context** — a GitHub Action posts a structured decision trail as a comment.

## Why Shadw

🔒 **Private by design** — conversations never leave your machine. Only curated summaries are shared.

💸 **Zero cost** — open-weight model, no API keys, no subscriptions, no vendor lock-in.

⚡ **Zero friction** — no extra steps, no new habits. You keep coding exactly like before.

🪶 **Lightweight** — single Rust binary. No Docker, no database, no server.

👀 **Better reviews** — reviewers see intent, not just diffs. Less back-and-forth, faster approvals.

## Commands

```
shadw init             Initialize in the current git repo
shadw ls               List all projects and daemon status
shadw start [target]   Start daemon(s) — ID, path, or "all"
shadw stop [target]    Stop daemon(s) — ID, path, or "all"
shadw restart [target] Restart daemon(s)
shadw rm <target>      Unregister a project
shadw status           Show daemon, model, and agent status
shadw use [model]      List or select the extraction model
shadw retry <hash>     Re-extract decisions for a commit
shadw upgrade          Upgrade to the latest release
```

## Models

| Model               | Params  | Size    | License    |
| ------------------- | ------- | ------- | ---------- |
| `qwen3-4b`          | 4B      | ~2.5 GB | Apache 2.0 |
| `gpt-oss` (default) | 20B MoE | ~12 GB  | Apache 2.0 |
| `qwen3-32b`         | 32B     | ~20 GB  | Apache 2.0 |

Larger models produce richer, more nuanced decision summaries. Use `qwen3-4b` when disk space or RAM is limited; use `qwen3-32b` for the best extraction quality.

All inference runs locally via [llama.cpp](https://github.com/ggerganov/llama.cpp). Switch models with `shadw use <model>`.

## Agents

| Agent                                 | Status    |
| ------------------------------------- | --------- |
| [Claude Code](https://claude.ai/code) | Supported |
| Cursor                                | Planned   |
| Windsurf                              | Planned   |
| Copilot                               | Planned   |

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE)
