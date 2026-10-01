# Riven

**Riven Launcher** — a Minecraft launcher and modpack toolkit (an alternative to packwiz) in pure Rust with [gpui](https://github.com/zed-industries/zed). One binary `riven`: no arguments → GUI, a subcommand → CLI.

`RIVEN_PLAN.md` is the development plan: formats, CLI surface, milestones. Follow it; ask before deviating.

## Architecture

Cargo workspace; the root package is `riven` (lib `riven_lib`, binary `riven`).

| Crate            | Responsibility                                                              |
| ---------------- | --------------------------------------------------------------------------- |
| `riven-format`   | Types, serde, JSON Schema, migrations, deterministic writer.                |
| `riven-sources`  | Modrinth, GitHub, URL, local behind one `Source` trait.                     |
| `riven-resolve`  | Dependency resolver, jar metadata parsing, conflict checks.                 |
| `riven-build`    | Releases: blobs, manifest, channels, signatures, exports/imports.           |
| `riven-sync`     | Installer: diff, content store, state, preserve, manual files.              |
| `riven-launch`   | Wrapper over `lighty-launcher`: instances, accounts, Java, launch, logs.    |

The root crate is a thin shell: `src/cli/` (clap subcommands, progress, `--json`), later `src/app/` (lifecycle) and `src/ui/` (gpui views).

- All logic lives in `crates/*` with **no gpui dependency**. CLI and GUI call the same functions and subscribe to the same progress events.
- `lib.rs::run()` is the entry point: no args and the `gui` feature → GUI, otherwise CLI.
- Features: `default = ["gui", "cli"]`; `--no-default-features --features cli` is the headless `riven-cli` build for servers and Docker. Anything gpui-related must stay behind `gui`.
- Windows: the release exe is GUI-subsystem; CLI runs call `AttachConsole(ATTACH_PARENT_PROCESS)` so output reaches the launching terminal.

## Commands

```bash
cargo run                                          # riven (GUI + CLI)
cargo run --no-default-features --features cli     # riven-cli
cargo fmt
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
RIVEN_LOG=debug cargo run -- <subcommand>          # verbose logs
```

`nix develop` gives the toolchain from `rust-toolchain.toml` plus the gpui system libraries.

Before every commit, the same checks CI runs must pass: `cargo fmt --all --check`, clippy for **both** feature sets (default and `--no-default-features --features cli`), and `cargo test --workspace --locked`. CI runs on Linux, Windows **and** macOS, so a `cfg`-gated branch that only compiles on one of them fails there and not locally.

## Code Guidelines

### Comments
- NO file-header comment blocks (`//!` module banners) and NO "heading"/divider comments like `// --- helpers ---`. Group code with functions, not comment art.
- Avoid inline `//` comments. Add one only when the code is genuinely non-obvious (a real footgun) — a wire-format quirk, a platform edge. Then keep it to a line or two.
- Doc comments (`///`) on public items are fine, but keep them short — a single line describing intent.
- Don't narrate the obvious. If a comment restates the next line, delete it.

### Style
- rustfmt is the source of truth — never hand-format against it.
- JSON files Riven writes (`riven.json`, manifests, state) go through the deterministic writer in `riven-format` — never `serde_json::to_string_pretty` directly.
- Paths inside packs are relative POSIX paths; validate them (no absolute, `..`, `\`, drive letters, escaping symlinks) before touching the filesystem.
- Never log account tokens.

### i18n
- All user-facing GUI strings go through `rust_i18n::t!` with a key.
- Every key must exist in ALL locale files: `locales/en-US.yml`, `ru-RU.yml`, `zh-CN.yml`.

### Testing

Tests live next to the code in `#[cfg(test)]` modules. What earns one is a real trap: deterministic writer golden files, path-traversal cases, odd `mods.toml`/`fabric.mod.json`, version ranges, installer interruption/preserve. Not one test per function. API responses come from recorded fixtures — no network in tests.

## Bash Guidelines
- Don't pipe output through `head`/`tail`/`less` to truncate — use tool-native flags (`git log -n 10`, `cargo clippy --message-format=short`). Read the full output.
- Don't create scratch files (scripts, notes) unless asked.
- When given failures, just fix them — don't argue about who introduced them.
