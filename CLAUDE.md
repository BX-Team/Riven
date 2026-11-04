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
| `riven-game`     | Game files: Mojang/loader metadata, downloads, Forge installers, Java, run. |
| `riven-launch`   | Instances, accounts, MS sign-in, Java, logs; launches through `riven-game`. |
| `riven-demo`     | The launcher's main screens on made-up data, built for the browser (bxteam.org). |

The root crate is a thin shell: `src/cli/` (clap subcommands, progress, `--json`), later `src/app/` (lifecycle) and `src/ui/` (gpui views).

- All logic lives in `crates/*` with **no gpui dependency**. CLI and GUI call the same functions and subscribe to the same progress events.
- `lib.rs::run()` is the entry point: no args and the `gui` feature → GUI, otherwise CLI.
- Features: `default = ["gui", "cli"]`; `--no-default-features --features cli` is the headless `riven-cli` build for servers and Docker. Anything gpui-related must stay behind `gui`.
- Windows: the release exe is GUI-subsystem; CLI runs call `AttachConsole(ATTACH_PARENT_PROCESS)` so output reaches the launching terminal.

### riven-demo

The site shows `crates/riven-demo` instead of screenshots: the sidebar, instances with mods (Vanilla Reimagined's real list), resource packs, shaders and worlds (icons from Modrinth's CDN), an Add dialog over a fixed catalog, row menus, the launch bar with a scripted launch and log, and Settings (General with live themes, Java & memory, Launch, Accounts, About). It depends on `gpui-kit` and `moonglass` only — never on another `riven-*` crate, since none of them builds for `wasm32` — so it **repeats** the layout of `src/gui` rather than sharing it: when a screen it shows changes in the app, change it here too. Screens it has no data for show a placeholder with a download button.

- `crates/riven-demo/build-web.sh` writes `dist/` (`index.html`, `riven_demo.js`, the wasm). It needs a **nightly** toolchain with the `wasm32-unknown-unknown` target — gpui's web backend pulls `wasm_thread`, which uses an unstable feature — and `wasm-bindgen-cli` at the exact version in `Cargo.lock`; `wasm-opt` (binaryen) is used when present.
- It runs on one thread (`WebPlatform::new(false)`), so the page needs no COOP/COEP headers and can sit in an iframe. gpui puts its canvas on `<body>`, so the demo is always its own page.
- The browser has no fonts gpui can read: Inter (OFL, `assets/fonts`) is bundled and set as the theme font. `std::time::Instant::now()` panics on `wasm32-unknown-unknown`; do not call it, nor moonglass APIs that do (theme fades: the demo switches palettes itself in `settings.rs`).
- `cargo run -p riven-demo` opens the same window natively, for working on it.
- The site serves it from `apps/web/public/demo/riven/` in the BX-Team/code repository: after a change, copy `dist/riven_demo.js` and `dist/riven_demo_bg.wasm` there. The site writes its own page around them, so `web/index.html` is only for trying the build locally.

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
- Every key must exist in ALL locale files: `locales/en-US.yml`, `ru-RU.yml`.

### Testing

Tests live next to the code in `#[cfg(test)]` modules. What earns one is a real trap: deterministic writer golden files, path-traversal cases, odd `mods.toml`/`fabric.mod.json`, version ranges, installer interruption/preserve. Not one test per function. API responses come from recorded fixtures — no network in tests.

## Bash Guidelines
- Don't pipe output through `head`/`tail`/`less` to truncate — use tool-native flags (`git log -n 10`, `cargo clippy --message-format=short`). Read the full output.
- Don't create scratch files (scripts, notes) unless asked.
- When given failures, just fix them — don't argue about who introduced them.
