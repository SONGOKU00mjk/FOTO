# Contributing to foto

Thank you for wanting to improve foto! This document gets you from a
fresh clone to a passing pull request.

foto is a Wayland-native application launcher and image browser: one
binary, no GTK, no Qt, no webview — just the Wayland protocol, a
software rasterizer (`tiny-skia`) and a font shaper (`cosmic-text`).

## Prerequisites

You need a Rust toolchain and the native libraries foto links against:

- **Rust** — install via [rustup](https://rustup.rs)
  (`curl https://sh.rustup.rs | sh`), or use your distribution's rustc
  if it is recent enough
- **A C compiler** and `git`
- **pkg-config**, **libxkbcommon** and **libwayland** (development
  packages) — foto speaks the Wayland protocol itself, but it links
  `libxkbcommon` for keymap handling and `libwayland-client` for the
  connection

Debian/Ubuntu: `sudo apt install build-essential pkg-config libxkbcommon-dev libwayland-dev git curl`
Arch: `sudo pacman -S base-devel pkgconf libxkbcommon wayland git rust`
Fedora: `sudo dnf install gcc make pkgconf-pkg-config libxkbcommon-devel wayland-devel git rust cargo`

The repository's `install.sh` does all of the above for you and builds
foto; using it to bootstrap a development machine is fine.

## Building and running the tests

```sh
cargo build                      # debug build
cargo test                       # unit + integration tests
cargo clippy --all-targets -- -D warnings
cargo fmt --all -- --check
```

Both frontends are cargo features (`gui`, `tui`, default = both). Check
your change compiles for the feature set it touches:

```sh
cargo check --no-default-features --features tui
cargo check --no-default-features --features gui
```

`--no-default-features` (neither feature) is a deliberate compile error.


CI runs exactly these four commands plus `cargo build --release`, so
please run them before pushing. Every command passes without a
compositor: the tests are pure logic plus CLI-surface checks and never
open a Wayland connection.

## Manual testing

### TUI (no Wayland needed)

The terminal UI is the fastest feedback loop and works over SSH:

```sh
cargo run -- --tui
```

If Wayland is not available at all, foto falls back to the TUI
automatically — you do not even need the flag inside a plain SSH
session.

Try at minimum: typing to filter, arrow navigation, Enter to activate,
Tab (apps <-> files switch), and Esc to exit.

### GUI (needs a Wayland session)

Run the debug build directly:

```sh
cargo run                         # layer-shell overlay on niri/Hyprland/Sway/mango
cargo run -- --no-layer           # force a normal tileable window
cargo run -- --files              # image browser
```

Check that the overlay floats above other windows, grabs the keyboard
immediately and renders with rounded corners.

### CLI helpers

```sh
cargo run -- --list               # dump the detected desktop entries
cargo run -- --print-config       # print the annotated default config
echo hi | cargo run -- --dmenu    # dmenu mode over stdin
```

## Testing the installer

`install.sh` carries a test hook: when the environment variable
`FOTO_INSTALLER_TEST` is set, sourcing the script gives you all of its
functions without running the installer itself. That is how the
installer's unit tests exercise `write_config`, `detect_pkg`, and
friends:

```sh
FOTO_INSTALLER_TEST=1 sh -c '. install.sh && detect_pkg'
FOTO_INSTALLER_TEST=1 sh -c '. install.sh && write_config'   # writes a config into $HOME/.config/foto
FOTO_INSTALLER_TEST=1 HOME=/tmp/fakehome sh -c '. install.sh && write_config'
```

`write_config` never overwrites an existing config, so repeated runs
are safe. A full end-to-end run of the real installer can be pointed at
a local checkout instead of GitHub with `FOTO_REPO=/path/to/copy sh
install.sh --user`.

### Containerized installer smoke test

`containers/smoke-test.sh` runs the installer inside Debian, Arch,
Fedora, Alpine and Void containers and asserts the binary installs and
`--help` works. It needs docker (or podman):

```sh
sh containers/smoke-test.sh              # all five distros
sh containers/smoke-test.sh debian arch  # just these
```

Each `containers/Dockerfile.<distro>` installs the distro's own rust
toolchain and native libraries, then runs
`install.sh --user --no-rustup` against the mounted repository — so the
test exercises the real dependency chain per distro family. The
prebuilt-download path additionally needs a published release tag; drop
`--no-rustup` and the `FOTO_REPO` override to exercise it end to end.

## Commit message convention

Use [Conventional Commits](https://www.conventionalcommits.org/) style,
one concern per commit:

```
feat: add --recursive scan for --files
fix: clamp selection at list boundaries
docs: expand TUI keybind table
style: apply cargo fmt
refactor: split config.rs into submodules
test: cover matcher tie-breaking
ci: cache clippy build artifacts
chore: bump cosmic-text to 0.14
```

The type determines the changelog entry later (`feat`/`fix`/`perf` are
user-visible; `docs`/`style`/`ci`/`chore` usually are not).

## Project rules

1. **No GUI toolkits.** No GTK, no Qt, no Electron, no webview. The
   rendering stack is `tiny-skia` + `cosmic-text` + `swash`, the UI is
   Wayland layer-shell (with an xdg_toplevel fallback) or ratatui.
2. **No new heavyweight dependencies.** Before adding a crate, state why
   `std` or an existing dependency cannot do the job, and get approval
   in the issue or PR. Prefer optional dependencies behind feature
   flags.
3. **Keep the layered architecture.** UI-agnostic logic lives in the
   core modules (`state.rs`, `matcher.rs`, `config.rs`, `desktop.rs`,
   `files.rs`, `icons.rs`, `thumbs.rs`); `tui.rs` and `gui/` are thin
   translators that feed events into the shared `App::on_key` state
   machine and render its `Flow` decisions.
4. **Do not rename config keys or CLI flags.** Backward compatibility
   matters; unknown keys in the KDL config are errors by design, so
   renames break working setups.
5. **All gates must pass.** `cargo fmt --check`, `cargo clippy -D
   warnings`, `cargo test` — CI enforces them on every push.

## Reporting bugs

Open an issue with the template's diagnostics (`foto --list`,
`foto --print-config`, `rustc --version`, compositor name and version,
and whether `--tui` reproduces the bug). Without those five data points
a Wayland bug is usually untestable.

## Licensing

By contributing you agree that your contributions are licensed under
the project's GPL-3.0-or-later license.
