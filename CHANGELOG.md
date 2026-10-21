# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] - 2026-10-22

### Added

- HiDPI / fractional-scale rendering (`wp_viewporter` +
  `wp_fractional_scale_v1`) — crisp text and icons on scaled outputs.
- Caret editing in the query: insert, delete, Home/End, arrow keys.
- Opt-in recursive `--files` scan (`--recursive`, depth-limited walk).
- Icon themes: `Inherits=` chains from `index.theme` are followed;
  a warning is printed once when an XPM is the only matching icon.
- Installer: prebuilt-binary installs from GitHub Releases with SHA256
  verification, plus `--branch`, `--prefix` and `--no-rustup` flags; the
  generated config now auto-detects a Nerd Font (`font`), the icon theme
  (`icon-theme`), the wallpaper setter (`open-with` — `awww`/`swww`,
  `hyprpaper`, `swaybg`) and the terminal; the Hyprland hint targets
  `hyprland.lua` (0.55+).
- Packaging: AUR `PKGBUILD`s (`foto`, `foto-git`) and a Nix flake.
- CI: fmt/clippy/test/build/shellcheck on every push; tag-triggered
  release workflow (x86_64 + aarch64 tarballs with `SHA256SUMS`).
- Tests: unit suites for every core module plus CLI integration tests.

### Changed

- Version bumped to **1.0.0** — the feature set is stable; configs and
  CLI flags from 0.1.0 keep working unchanged.
- `resvg`'s `usvg` re-export is used instead of a separate `usvg`
  dependency (smaller dependency tree).

## [0.1.0] - 2026-10-22

### Added

- Wayland-native floating launcher: layer-shell overlay
  (`zwlr_layer_shell_v1`) with an `xdg_toplevel` fallback, rendered with
  a software rasterizer (`tiny-skia`) — no GTK, no Qt.
- Fuzzy app filtering with two strategies: `scored` (fzf-style
  subsequence ranking) and `substring`.
- Desktop entry parsing (`desktop.rs`) with icon loading, Terminal
  wrapping and `%u`/`%f`/`%i` field-code handling.
- `--files` image browser with background thumbnail workers and a disk
  cache at `~/.cache/foto/thumbs`.
- `--dmenu` mode reading menu items from stdin
  (e.g. `cliphist list | foto --dmenu`).
- TUI fallback (`--tui`, automatic when Wayland is absent) built on
  ratatui, sharing the same `App::on_key` state machine as the GUI.
- KDL configuration at `~/.config/foto/config.kdl`; unknown keys are
  errors by design. Annotated default via `foto --print-config`.
- Diagnostics helpers: `foto --list`.
- One-line installer (`install.sh`): detects the distro, installs build
  dependencies, clones, builds, installs and writes a starter config
  with auto-detected wallpaper setter and terminal.

[1.0.0]: https://github.com/SONGOKU00mjk/foto/releases/tag/v1.0.0
[0.1.0]: https://github.com/SONGOKU00mjk/foto/releases/tag/v0.1.0
