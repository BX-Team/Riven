# Riven

Riven Launcher: a Minecraft launcher and modpack toolkit in pure Rust on [gpui](https://github.com/zed-industries/zed) and [moonglass](https://github.com/BX-Team/moonglass). One binary, `riven`: no arguments opens the GUI, a subcommand runs the CLI.

## Architecture

A Cargo workspace; the root package `riven` (lib `riven_lib`) is a thin shell: `src/cli/` (clap subcommands, progress, `--json`) and `src/gui/` (gpui views).

| Crate            | Responsibility                                                              |
| ---------------- | --------------------------------------------------------------------------- |
| `riven-format`   | Types, serde, JSON Schema, migrations, deterministic writer.                |
| `riven-sources`  | Modrinth, GitHub, URL and local files behind one `Source` trait.            |
| `riven-resolve`  | Dependency resolver, jar metadata parsing, conflict checks.                 |
| `riven-build`    | Pack authoring and releases: manifest, channels, signatures, imports, exports. |
| `riven-sync`     | Installer: diff, content store, state, preserved files.                     |
| `riven-game`     | Game files: Mojang and loader metadata, downloads, Forge installers, run.   |
| `riven-launch`   | Instances, accounts, Microsoft sign-in, Java, logs, self-update.            |
| `riven-demo`     | The launcher's screens on made-up data, built for the browser.              |

- All logic lives in `crates/*` with no gpui dependency; the CLI and the GUI call the same functions and subscribe to the same progress events.
- Features: `default = ["gui", "cli"]`; `--no-default-features --features cli` is the headless build. Anything gpui-related stays behind `gui`.
- `riven-demo` depends on no other `riven-*` crate and repeats the layout of `src/gui`: when a screen it shows changes in the app, change it there too.

## Commands

```bash
cargo run                                          # GUI + CLI
cargo run --no-default-features --features cli     # headless CLI
RIVEN_LOG=debug cargo run -- <subcommand>          # verbose logs
cargo fmt
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo clippy --workspace --all-targets --locked --no-default-features --features cli -- -D warnings
cargo test --workspace --locked
scripts/licenses.sh                                # after dependency updates: assets/licenses.json
```

`nix develop` gives the toolchain and the gpui system libraries. CI runs `cargo fmt --all --check`, both clippy runs and the tests on Linux, Windows and macOS, so a `cfg`-gated branch must compile on all three.

## Working on Tasks

- If the message is a question, investigate and answer only. Edit files only when asked to fix or implement.
- Stay within the scope of the request. Mention related problems you notice instead of fixing them.
- Fix the cause, not the symptom. No special-case constants or hardcoded lists to make one case work.
- When a change can't be verified without running the app, say so.
- Trust the user's reports about what they observed in the running app.
- Don't edit `CLAUDE.md` or `AGENTS.md` unless asked. Decisions, plans and progress don't belong in them.

## Code Guidelines

### Comments

- NO file-header banners (`//!`) and NO divider comments (`// --- helpers ---`).
- Avoid inline comments. Add one only where the code is genuinely non-obvious: a footgun, a wire-format quirk, a reason a thing is done backwards. One line, two at most.
- Why a thing is done this way, or the trap it avoids, is that one short comment at the code, not a paragraph and not an entry here.
- Doc comments (`///`) on public items are fine; keep them to a sentence.

### Style

- rustfmt is the source of truth — never hand-format against it.
- JSON files Riven writes (`riven.json`, manifests, state) go through the deterministic writer in `riven-format`, never `serde_json::to_string_pretty`.
- Paths inside packs are relative POSIX paths; validate them (no absolute, `..`, `\`, drive letters, escaping symlinks) before touching the filesystem.
- Never log account tokens.

### User-facing strings

GUI strings go through `rust_i18n::t!` with a key, and every key exists in both `locales/en-US.yml` and `locales/ru-RU.yml`. CLI output is English.

### Testing

Tests live next to the code in `#[cfg(test)]` modules; API responses come from recorded fixtures, never the network. One test per real trap, not one per function.

## Bash Guidelines

- Don't pipe output through `head`/`tail`/`less` to truncate — use tool-native flags (`git log -n 10`, `cargo clippy --message-format=short`). Read the full output.
- Don't create scratch files (scripts, notes) unless asked.
- When given failures, just fix them — don't argue about who introduced them.

## Subprojects

- [`crates/riven-demo/CLAUDE.md`](crates/riven-demo/CLAUDE.md) — the browser demo on bxteam.org
