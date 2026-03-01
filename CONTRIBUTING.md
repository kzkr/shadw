# Contributing

Want to help build Shadw? Here's everything you need to get going.

## 🛠️ Setup

```bash
cargo build                                        # build
cargo test                                         # run all tests
cargo clippy                                       # lint (zero warnings required)
cargo test <test_name>                             # run a single test
RUST_LOG=shadw=debug cargo run -- start --foreground   # debug the daemon
```

## 🔀 Pull requests

1. **Open an issue first** for anything non-trivial — let's align before you write code.
2. **One logical change per PR** — keep it focused.
3. **`cargo clippy` clean, `cargo test` green** — no exceptions.
4. **Add tests** for new commands or behavior changes. Tests live in `tests/cli_test.rs`.

## 📐 Code conventions

- Errors: `ShadwError` / `Result<T>` — no `unwrap()` outside infallible cases.
- Logging: `tracing` (`info!`, `warn!`, `debug!`) in the daemon, `println!`/`eprintln!` in CLI output.
- Utilities: shared helpers go in `src/util.rs`.
- Keep the daemon light — it runs in the background for entire dev sessions.

## 🔍 What we're looking for

- 🤖 New agent sources (Cursor, Windsurf, Copilot)
- 🧠 New extraction models
- 🐧 Linux/Windows compatibility
- 🐛 Bug fixes and edge cases

## 🐛 Reporting issues

Open an issue with:
- What you expected vs what happened
- Your OS and Rust version (`rustc --version`)
- Daemon logs if relevant (`cat .shadw/state/daemon.log`)

## 📄 License

By submitting a PR, you agree your contributions are licensed under [MIT](LICENSE).
