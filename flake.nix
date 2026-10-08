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
      rivenLib = import ./nix/lib.nix;
    in
    {
      lib = rivenLib;
    }
    // eachSystem (
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
        buildToolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;
        rustPlatform = pkgs.makeRustPlatform {
          cargo = buildToolchain;
          rustc = buildToolchain;
        };
        cargoToml = lib.importTOML ./Cargo.toml;

        rivenPackage =
          {
            pname,
            cli ? false,
          }:
          rustPlatform.buildRustPackage {
            inherit pname;
            inherit (cargoToml.workspace.package) version;
            src = lib.fileset.toSource {
              root = ./.;
              fileset = lib.fileset.unions [
                ./Cargo.toml
                ./Cargo.lock
                ./src
                ./crates
                ./build.rs
                ./assets
                ./locales
              ];
            };
            cargoLock.lockFile = ./Cargo.lock;
            buildNoDefaultFeatures = cli;
            buildFeatures = lib.optionals cli [ "cli" ];
            nativeBuildInputs = [
              pkgs.pkg-config
            ]
            ++ lib.optionals (!cli) [ pkgs.autoPatchelfHook ];
            buildInputs = lib.optionals (!cli) (runtimeLibs ++ [ pkgs.stdenv.cc.cc.lib ]);
            # gpui dlopens Vulkan, Wayland and X11 at runtime; autoPatchelf bakes them into the rpath.
            runtimeDependencies = lib.optionals (!cli) runtimeLibs;
            # The workspace tests run in CI; here they would only repeat it.
            doCheck = false;
            postInstall = lib.optionalString (!cli) ''
              install -Dm644 assets/brand/riven.svg $out/share/icons/hicolor/scalable/apps/riven.svg
              install -Dm644 assets/brand/riven-512.png $out/share/icons/hicolor/512x512/apps/riven.png
              install -Dm644 assets/riven.desktop $out/share/applications/riven.desktop
            '';
            # build.rs cannot ask git inside the sandbox; the About page shows this commit.
            RIVEN_REV = self.shortRev or self.dirtyShortRev or "unknown";
            meta = {
              description = cargoToml.package.description;
              homepage = cargoToml.workspace.package.homepage;
              license = lib.licenses.gpl3Plus;
              mainProgram = "riven";
            };
          };

        miniPack =
          args:
          rivenLib.mkModpack (
            {
              inherit pkgs;
              src = ./nix/tests/mini-pack;
            }
            // args
          );
        expect =
          name: drv: script:
          pkgs.runCommand "riven-check-${name}" { } ''
            cd ${drv}
            ${script}
            touch $out
          '';
      in
      {
        packages.${system} = rec {
          riven = rivenPackage { pname = "riven"; };
          riven-cli = rivenPackage {
            pname = "riven-cli";
            cli = true;
          };
          default = riven;
        };

        checks.${system} = {
          mini-pack-server = expect "mini-pack-server" (miniPack { side = "server"; }) ''
            test -f "mods/server tools.jar"
            test -f config/a.toml
            test -f server.properties
            test ! -e config/a.toml.bak
            test ! -e options.txt
            test ! -e shaderpacks/bsl.zip
          '';
          mini-pack-client =
            expect "mini-pack-client"
              (miniPack {
                side = "client";
                groups.shaders = true;
              })
              ''
                test -f shaderpacks/bsl.zip
                test -f options.txt
                test ! -e "mods/server tools.jar"
                test ! -e server.properties
              '';
          mini-pack-exclude =
            expect "mini-pack-exclude"
              (miniPack {
                side = "server";
                exclude = [ "server-tools" ];
              })
              ''
                test ! -e "mods/server tools.jar"
              '';
        };

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
