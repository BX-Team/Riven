{
  description = "Riven Launcher — Minecraft launcher and modpack toolkit";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    systems.url = "github:nix-systems/default-linux";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      systems,
      rust-overlay,
    }:
    let
      inherit (nixpkgs) lib;
      eachSystem = f: lib.foldl' lib.recursiveUpdate { } (map f (import systems));
    in
    eachSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ (import rust-overlay) ];
        };

        rustToolchain = (pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml).override {
          extensions = [
            "rust-src"
            "rust-analyzer"
            "clippy"
            "rustfmt"
          ];
        };

        # gpui dlopens these at runtime (GUI build only).
        runtimeLibs = with pkgs; [
          wayland
          libxkbcommon
          libx11
          libxcb
          libxcursor
          libxi
          libxrandr
          vulkan-loader
          libGL
          fontconfig
          freetype
        ];
      in
      {
        devShells.${system}.default = pkgs.mkShell {
          buildInputs = runtimeLibs;
          nativeBuildInputs = with pkgs; [
            rustToolchain
            pkg-config
            git
          ];

          shellHook = ''
            export LD_LIBRARY_PATH="${lib.makeLibraryPath runtimeLibs}:$LD_LIBRARY_PATH"
            echo "Riven dev shell ready."
            echo "  cargo run                                         # riven (GUI + CLI)"
            echo "  cargo run --no-default-features --features cli    # riven-cli"
          '';
        };

        formatter.${system} = pkgs.nixfmt;
      }
    );
}
