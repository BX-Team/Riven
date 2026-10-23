<div align="center">

<img src="assets/brand/riven-128.png" width="128" height="128" alt="Riven Logo" />

# Riven

A native Minecraft launcher and modpack toolkit in pure Rust — install, update and build packs from one app or one CLI.

[![Chat on Discord](https://cdn.jsdelivr.net/npm/@intergrav/devins-badges@3/assets/cozy/social/discord-plural_vector.svg)](https://discord.gg/qNyybSSPm5)
[![github](https://cdn.jsdelivr.net/npm/@intergrav/devins-badges@3/assets/cozy/available/github_vector.svg)](https://github.com/BX-Team/Riven)

</div>

## 🖼️ Showcase

<details>
<summary>Screenshots</summary>

![Instances and mods](.github/showcase/showcase-instance.png)

![Developer mode](.github/showcase/showcase-developer.png)

![Config editor](.github/showcase/showcase-editor.png)

![Releases](.github/showcase/showcase-releases.png)

</details>

## ✨ Features

- **Launcher** — instances, Microsoft and offline accounts, automatic Java, Vanilla / Fabric / Quilt / NeoForge / Forge, logs and crash reports.
- **Modpacks from a link** — Modrinth modpacks, `.mrpack`, Prism exports and Riven packs; a pack updates itself before every launch.
- **Own pack format** — one `riven.json` with pinned versions and hashes, edited through the app or the CLI, never by hand.
- **Sources** — Modrinth, GitHub Releases, direct URLs and local files, with dependency resolution and conflict checks.
- **Fast, safe updates** — release channels, a tiny pointer checked with ETag, content-addressed downloads, ed25519 signatures.
- **Developer mode** — content table, dependency tree, config editor, releases, git and a test instance in one window.
- **Servers and Nix** — headless `riven install --side server`, and a Nix library that builds a pack without a fixed-output hash.

## 📦 Installation

[![Download for Windows](https://raw.githubusercontent.com/NONPLAYT/badges/refs/heads/master/download-for-windows.svg)](https://github.com/BX-Team/Riven/releases/latest/download/Riven-Setup-x86_64.exe)
[![Portable for Windows](https://raw.githubusercontent.com/NONPLAYT/badges/refs/heads/master/portable-for-windows.svg)](https://github.com/BX-Team/Riven/releases/latest/download/riven-x86_64-windows.zip)
[![Download for macOS](https://raw.githubusercontent.com/NONPLAYT/badges/refs/heads/master/download-for-macos.svg)](https://github.com/BX-Team/Riven/releases/latest/download/Riven-macos.dmg)
[![AppImage for Linux](https://raw.githubusercontent.com/NONPLAYT/badges/refs/heads/master/appimage-for-linux.svg)](https://github.com/BX-Team/Riven/releases/latest/download/Riven-x86_64.AppImage)
[![Package for Debian and Ubuntu](https://raw.githubusercontent.com/NONPLAYT/badges/refs/heads/master/package-for-debian.svg)](https://github.com/BX-Team/Riven/releases/latest/download/riven-amd64.deb)
[![Package for Fedora](https://raw.githubusercontent.com/NONPLAYT/badges/refs/heads/master/package-for-fedora.svg)](https://github.com/BX-Team/Riven/releases/latest/download/riven-x86_64.rpm)
[![Package for Arch Linux](https://raw.githubusercontent.com/NONPLAYT/badges/refs/heads/master/package-for-arch.svg)](https://github.com/BX-Team/Riven/releases/latest/download/riven-x86_64.pkg.tar.zst)
[![Run with Nix](https://raw.githubusercontent.com/NONPLAYT/badges/refs/heads/master/run-with-nix.svg)](#nix)

**ARM64 Linux:** [AppImage](https://github.com/BX-Team/Riven/releases/latest/download/Riven-aarch64.AppImage) · [.deb](https://github.com/BX-Team/Riven/releases/latest/download/riven-arm64.deb) · [.rpm](https://github.com/BX-Team/Riven/releases/latest/download/riven-aarch64.rpm). Every file of every version is on the [Releases page](https://github.com/BX-Team/Riven/releases).

The macOS app is not notarized: right-click → Open the first time.

For servers, `riven-cli-<arch>-<os>` archives hold the CLI without the GUI and its libraries:

```bash
riven install --side server --dir /srv/mc --headless gh:owner/pack
```

The launcher updates itself from GitHub Releases (Settings → About), the CLI with `riven self-update`. Copies from a package manager or Nix are updated there instead.

## ⌨️ CLI

`riven` with no arguments opens the launcher; a subcommand runs in the terminal.

```bash
riven init --mc 1.21.1 --loader neoforge   # new riven.json
riven add sodium                           # with its dependencies
riven update --dry-run                     # preview newer versions
riven build --channel stable               # signed release in dist/
riven deploy                               # dist/ → GitHub Pages
```

Every command is described in the [documentation](https://bxteam.org/docs/riven).

<a id="nix"></a>

## ❄️ Nix

Run the launcher without installing:

```bash
nix run --accept-flake-config github:BX-Team/Riven/v1.2.1
```

Releases are built for `x86_64-linux` and `aarch64-linux` and pushed to Cachix, so a tagged version downloads instead of compiling. `--accept-flake-config` lets Nix use the cache the flake names — if you are a trusted Nix user (`trusted-users`); otherwise Nix ignores it and compiles. Without a tag Nix takes `master`, which may not be in Cachix yet and then compiles from source.

To use the cache in any case, add it to your system:

```nix
nix = {
  settings = {
    substituters = [
      "https://bx-team.cachix.org"
    ];
    trusted-public-keys = [
      "bx-team.cachix.org-1:tnGNc1rsS8QOav+VGxXCZzf/Y0/SGchOwVCCBA/eG6E="
    ];
  };
};
```

The flake exposes `packages.<system>.riven` and `riven-cli`, plus `lib.mkModpack`, which turns a pack repository into a server's mods and configs — every file is its own `fetchurl`, no `packHash`:

```nix
modpack = riven.lib.mkModpack {
  inherit pkgs;
  src = ./.;
  side = "server";
  groups.velocity = true;
};
```

## 🔨 Build from source

Riven is a single pure-Rust [gpui](https://github.com/zed-industries/zed) application. It needs the toolchain pinned in `rust-toolchain.toml`.

```bash
git clone https://github.com/BX-Team/Riven.git
cd Riven
cargo run                                        # launcher + CLI
cargo run --no-default-features --features cli   # CLI only
cargo build --release                            # target/release/riven
```

On **Linux** the launcher also needs the gpui system libraries (Wayland, X11, xkbcommon, Vulkan, fontconfig), or just `nix develop`.

## 🤝 Contributing

Ideas, bug reports and pull requests are welcome in [Issues](https://github.com/BX-Team/Riven/issues) or on [Discord](https://discord.gg/qNyybSSPm5).

## ⚖️ License

This project is licensed under the GPL-3.0-or-later License — see the [LICENSE](LICENSE) file for details.

## 💛 Credits

- [zed-industries/zed](https://github.com/zed-industries/zed): Home of the [gpui](https://www.gpui.rs/) GPU-accelerated UI framework Riven is built on.
- [longbridge/gpui-kit](https://github.com/longbridge/gpui-kit): The gpui component library under the Riven design.
- [Modrinth](https://modrinth.com): The content platform Riven searches and installs from.
