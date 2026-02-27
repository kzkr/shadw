# Contributing to Shadw

Thanks for your interest in contributing to Shadw.

## License Agreement

Shadw is licensed under [MIT](LICENSE). By submitting a pull request, you agree that your contributions will be licensed under the same terms.

## Getting Started

```bash
# Build
cargo build

# Run tests
cargo test

# Lint (must pass with zero warnings)
cargo clippy

# Run a single test
cargo test <test_name>

# Debug the daemon locally
RUST_LOG=shadw=debug cargo run -- start --foreground
```

## Pull Requests

1. **Open an issue first** for anything non-trivial — let's align on approach before you write code.
2. **Keep PRs focused** — one logical change per PR.
3. **All checks must pass** — `cargo clippy` with zero warnings, `cargo test` with all tests green.
4. **Include tests** for new CLI commands or behavior changes. Tests live in `tests/cli_test.rs`.

## Code Conventions

- Handle errors with `ShadwError` / `Result<T>` — avoid `unwrap()` outside of infallible cases.
- Use `tracing` (`info!`, `warn!`, `debug!`) for daemon logging, `println!`/`eprintln!` for CLI output.
- Shared utilities go in `src/util.rs`.
- Keep the daemon lightweight — it runs in the background for the entire dev session.

## What We're Looking For

- New agent sources (beyond Claude Code)
- New model support
- Linux/Windows compatibility improvements
- Bug fixes and edge case handling

## Reporting Issues

Open an issue with:
- What you expected vs what happened
- Your OS and Rust version (`rustc --version`)
- Daemon logs if relevant (`cat .shadw/state/daemon.log`)
