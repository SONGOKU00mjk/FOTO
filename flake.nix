{
  description = "foto — minimal Wayland-native app launcher and image browser (no GTK, no Qt)";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs =
    { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        # nix build / nix run — the released launcher
        packages.default = pkgs.rustPlatform.buildRustPackage {
          pname = "foto";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).package.version;
          src = pkgs.lib.cleanSource self;

          # native libraries foto links against; the Wayland protocol itself
          # is spoken by foto (no libwayland runtime dependency for the
          # protocol, but libwayland-client is linked via wayland-client)
          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = [
            pkgs.libxkbcommon
            pkgs.wayland
          ];

          cargoLock.lockFile = ./Cargo.lock;

          meta = with pkgs.lib; {
            description = "Minimal Wayland-native app launcher with image preview";
            homepage = "https://github.com/SONGOKU00mjk/foto";
            license = licenses.gpl3Plus;
            mainProgram = "foto";
            platforms = platforms.linux;
          };
        };

        # nix develop — full dev environment (compiler + tools + native libs)
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = [
            pkgs.cargo
            pkgs.rustc
            pkgs.rustfmt
            pkgs.clippy
            pkgs.rust-analyzer
            pkgs.pkg-config
          ];
          buildInputs = [
            pkgs.libxkbcommon
            pkgs.wayland
          ];
          # rust-analyzer needs the standard library sources
          RUST_SRC_PATH = pkgs.rustPlatform.rustLibSrc;
        };
      }
    );
}
