# Shadw

**Capture the _why_ behind code changes.**

> **Note:** Shadw is under active development. APIs, CLI flags, and the decision record format may change between releases.

When developers work with AI coding assistants, the most valuable part — the reasoning, the alternatives considered, the tradeoffs debated — disappears the moment the conversation ends. The diff shows _what_ changed. Shadw captures _why_.

## How it works

Shadw runs a lightweight daemon that watches two things:

1. **Your AI conversations** — reads Claude Code's JSONL session files as they're written
2. **Your git commits** — detects new commits via ref changes

When a commit lands, Shadw correlates it with the preceding conversation, runs a local LLM to extract a structured decision record, and attaches it as a [git note](https://git-scm.com/docs/git-notes). On push, a GitHub Action renders these into a **Decision Trail** on your pull request:

<details>
<summary><strong><code>a1b2c3d4</code></strong> Chose a shared PropertyCard over duplicating markup per page</summary>
<br>

@kzkr needed a way to display property listings across both the search results and favorites pages. The conversation explored three options: a shared component, page-specific templates, and a slot-based layout wrapper.

The shared PropertyCard won because both pages render identical data — only the grid layout and empty states differ. Duplicating the markup would mean syncing changes across two files every time the card design evolves. The component accepts a `variant` prop for the minor visual differences between contexts.

</details>

## Key properties

- **Free** — uses a local open-weight model, no API keys or subscriptions
- **Local-first** — extraction runs entirely on your machine via [llama.cpp](https://github.com/ggerganov/llama.cpp)
- **Private by default** — your conversations never leave your machine; only curated decision summaries are shared via git notes
- **Fast** — single Rust binary, no runtime, no Docker, no background services beyond the daemon itself
- **Zero configuration** — one command to set up, then it works silently in the background
- **Fits your workflow** — uses native git features (notes, hooks), no new tools to learn

## Install

```bash
curl -fsSL https://shadw.dev/install.sh | bash
```

Prebuilt binaries are available for macOS (Apple Silicon & Intel) and Linux (x86_64).

<details>
<summary>Build from source</summary>

```bash
cargo build --release
# Binary at target/release/shadw
```

Requires Rust 1.85+ and cmake.

</details>

## Quick start

```bash
# Initialize in your project (downloads the model on first run)
cd your-project
shadw init

# That's it. Shadw is now watching.
```

`shadw init` will:

- Create a `.shadw/` directory (gitignored)
- Download the extraction model (~12 GB, one-time)
- Install a `pre-push` hook to sync decision notes
- Add a GitHub Action workflow for PR comments
- Start the daemon

## Commands

| Command              | Description                              |
| -------------------- | ---------------------------------------- |
| `shadw init`         | Initialize Shadw in the current git repo |
| `shadw start`        | Start the background daemon              |
| `shadw stop`         | Stop the daemon                          |
| `shadw restart`      | Restart the daemon                       |
| `shadw status`       | Show daemon, model, and agent status     |
| `shadw use [model]`  | List or select the extraction model      |
| `shadw retry <hash>` | Re-extract decisions for a commit        |
| `shadw upgrade`      | Upgrade to the latest release            |

## Requirements

- macOS (Apple Silicon or Intel) or Linux (x86_64)
- Git
- ~12 GB disk space for the default model

## Supported agents

| Agent | Source |
|-------|--------|
| [Claude Code](https://claude.ai/code) | JSONL session files |

More agents coming soon (Cursor, Windsurf, Copilot).

## Extraction models

| Model | Params | Size | License |
|-------|--------|------|---------|
| `gpt-oss` (default) | 20B MoE | ~12 GB | Apache 2.0 |

Models run locally via llama.cpp. More models coming soon. Use `shadw use` to list or switch models.

## How decisions reach your PR

```
Developer + AI conversation
        ↓
  Shadw daemon (local)
        ↓
  Local LLM extraction
        ↓
  git notes --ref shadw
        ↓
  git push (via pre-push hook)
        ↓
  GitHub Action reads notes
        ↓
  PR comment: Decision Trail
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

[MIT](LICENSE)
