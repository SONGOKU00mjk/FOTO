#!/bin/sh
# =============================================================================
# foto installer — Wayland-native app launcher + image browser
#
#   One-line install (copy-paste into your terminal):
#
#     curl -fsSL https://raw.githubusercontent.com/SONGOKU00mjk/foto/main/install.sh | sh
#
#   What it does, in order:
#     1. identifies your distro and its package manager
#     2. downloads the PREBUILT binary for your architecture from the
#        latest GitHub release and verifies its SHA256 checksum — falling
#        back to a source build only when no artifact matches
#     3. source-build path: installs EVERYTHING foto needs to build (cc,
#        git, pkg-config, libxkbcommon, rust via rustup if cargo missing),
#        then git-clones the latest release tag (or main) and builds
#     4. asks where to put the binary (Enter = the recommended system-wide
#        install to /usr/local/bin, or 2 for a user-only ~/.local/bin one)
#     5. writes a starter config with your wallpaper setter (swaybg or awww)
#        and terminal auto-detected
#
#   Options:
#     --system       system-wide install to /usr/local/bin (recommended),
#                    skips the question
#     --user         user-only install to ~/.local/bin, skips the question
#     --prefix DIR   where --system installs (default /usr/local/bin)
#     --no-rustup    source builds must use a system rust toolchain;
#                    never run the rustup installer
#     --config-only  just (re)write the default config, nothing else
#     --uninstall    remove the foto binary (config and cache are kept)
#     --help         show this help
# =============================================================================
set -eu

# ---------------------------------------------------------------------------
# The repository to clone and build (your GitHub username / repo name).
#
# Optional environment hooks (used by the test suite and CI):
#   FOTO_REPO=/path/to/local/copy  clone this instead of GitHub
#   FOTO_PREFIX=/some/dir          where --system installs (default
#                                  /usr/local/bin)
#   FOTO_NONINTERACTIVE=1          never ask questions, take the defaults
# ---------------------------------------------------------------------------
REPO="${FOTO_REPO:-SONGOKU00mjk/foto}"

# SCOPE picks where the binary goes: 'system' (recommended) or 'user'.
SUDO=''
SCOPE=''

# Install destination for the system-wide scope (--prefix / FOTO_PREFIX).
PREFIX="${FOTO_PREFIX:-/usr/local/bin}"

# --no-rustup: never run the rustup installer; require a system toolchain.
NO_RUSTUP=0

# ---------- output helpers (colors only on a real terminal) ----------
if [ -t 1 ]; then
  C_B='\033[1;36m'; C_G='\033[1;32m'; C_Y='\033[1;33m'; C_R='\033[1;31m'; C_0='\033[0m'
else
  C_B=''; C_G=''; C_Y=''; C_R=''; C_0=''
fi
step() { printf "${C_B}==>${C_0} %s\n" "$*"; }
ok()   { printf "${C_G} ok:${C_0} %s\n" "$*"; }
warn() { printf "${C_Y}warning:${C_0} %s\n" "$*" >&2; }
die()  { printf "${C_R}error:${C_0} %s\n" "$*" >&2; exit 1; }

usage() {
  cat <<'EOF'
foto installer — Wayland-native app launcher + image browser

  one line, copy-paste into your terminal:

    curl -fsSL https://raw.githubusercontent.com/SONGOKU00mjk/foto/main/install.sh | sh

  identifies your distro, installs everything foto needs to build and run
  (compiler, git, pkg-config, libxkbcommon, rust), clones the repository,
  builds it, installs it, then writes a starter config (wallpaper setter
  swaybg/awww and terminal auto-detected).

options:
  --system       install system-wide to /usr/local/bin (recommended),
                 skips the question
  --user         install just for you to ~/.local/bin, skips the question
  --prefix DIR   where --system installs (default /usr/local/bin)
  --no-rustup    source builds must use a system rust toolchain; never
                 run the rustup installer
  --config-only  just (re)write the default config, nothing else
  --uninstall    remove the foto binary (your config and cache are kept;
                 it prints the exact rm -rf lines if you want a full wipe)
  --help         show this help

without any flag the installer simply asks; pressing Enter picks the
recommended system-wide install. CI / no-terminal runs get that same
system-wide default automatically.
EOF
}

# ---------- which distro is this? ----------
# Reads /etc/os-release (or a path override — used by the test suite) and
# prints one of: apt pacman dnf zypper apk xbps gentoo nixos unknown
# SC2120: the in-file caller passes no argument on purpose — the optional
# path parameter exists for scripts/test_install.sh only.
# shellcheck disable=SC2120
detect_pkg() {
  _osr="${1:-/etc/os-release}"
  _id=''; _like=''
  if [ -f "$_osr" ]; then
    # shellcheck disable=SC1090
    . "$_osr" 2>/dev/null || true
    _id="${ID:-}"; _like="${ID_LIKE:-}"
  fi
  case "$_id" in
    debian|ubuntu|linuxmint|pop|pop-os|zorin|kali|parrot|raspbian|devuan|trisquel|mx)
      printf 'apt\n' ;;
    arch|manjaro|endeavouros|cachyos|garuda|artix|arcolinux|bliss)
      printf 'pacman\n' ;;
    fedora|rhel|centos|rocky|almalinux|nobara|bazzite|ultramarine|silverblue|asahi)
      printf 'dnf\n' ;;
    opensuse-leap|opensuse-tumbleweed|opensuse|sled|sles)
      printf 'zypper\n' ;;
    alpine) printf 'apk\n' ;;
    void)   printf 'xbps\n' ;;
    gentoo) printf 'gentoo\n' ;;
    nixos)  printf 'nixos\n' ;;
    *)
      case "$_like" in
        *debian*)                   printf 'apt\n' ;;
        *arch*)                     printf 'pacman\n' ;;
        *fedora*|*rhel*|*centos*)   printf 'dnf\n' ;;
        *suse*|*sles*)              printf 'zypper\n' ;;
        *)                          printf 'unknown\n' ;;
      esac
      ;;
  esac
}

# ---------- sudo detection (empty SUDO = we are root already) ----------
resolve_sudo() {
  SUDO=''
  [ "$(id -u)" -eq 0 ] && return 0
  if command -v sudo >/dev/null 2>&1; then SUDO='sudo'
  else warn "no sudo found on this machine"
  fi
}

# ---------- where to install? asked, or set by --system / --user ----------
# The answer is read from /dev/tty, so the question also works when the
# script is piped straight from curl:  curl -fsSL .../install.sh | sh
ask_scope() {
  SCOPE='system'   # the recommended default — chosen by just pressing Enter
  if [ -n "${FOTO_NONINTERACTIVE:-}" ]; then
    step "FOTO_NONINTERACTIVE set — taking the recommended system-wide install"
    step "(re-run with --user for a user-only install)"
    return 0
  fi
  if ! ( : < /dev/tty ) 2>/dev/null; then
    step "No interactive terminal — taking the recommended system-wide install"
    step "(re-run with --user for a user-only install)"
    return 0
  fi
  _tries=0
  while :; do
    cat <<'EOF'

  Where should foto be installed?
    1) System-wide  ->  /usr/local/bin   (recommended, needs sudo)
    2) Just for you ->  ~/.local/bin     (no sudo, must be in your PATH)
EOF
    printf '  Choose [1]: '
    _ans=''
    IFS= read -r _ans < /dev/tty || _ans=''
    case "$_ans" in
      ''|1|s|S|sys|system|System|SYSTEM) SCOPE='system'; break ;;
      2|u|U|me|user|User|USER)           SCOPE='user';   break ;;
      *)
        _tries=$((_tries + 1))
        if [ "$_tries" -ge 5 ]; then
          warn "five unclear answers — going with the recommended system-wide install"
          break
        fi
        printf '  Please answer 1 or 2 — or just press Enter for the recommended 1.\n'
        ;;
    esac
  done
  printf '\n'
}

choose_scope() {
  [ -n "$SCOPE" ] || ask_scope
  case "$SCOPE" in
    system) ok "install scope: system-wide (/usr/local/bin) — recommended" ;;
    user)   ok "install scope: just for you ($HOME/.local/bin)" ;;
    *)      die "internal error: unknown install scope '$SCOPE'" ;;
  esac
}

# system-wide needs root or sudo; without either we degrade to a user-only
# install instead of failing after the long build
apply_scope_sudo() {
  resolve_sudo
  if [ "$SCOPE" = "system" ] && [ "$(id -u)" -ne 0 ] && [ -z "$SUDO" ]; then
    warn "system-wide needs sudo, but none was found — falling back to $HOME/.local/bin"
    SCOPE='user'
  fi
}

# ---------- install system packages with the distro package manager ----------
install_pkgs() { # install_pkgs pkg...
  [ $# -gt 0 ] || return 0
  step "Installing packages: $*"
  case "$PKGMGR" in
    apt)    $SUDO env DEBIAN_FRONTEND=noninteractive apt-get update -y
            $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y "$@" ;;
    pacman) $SUDO pacman -Sy --needed --noconfirm "$@" ;;
    dnf)    $SUDO dnf install -y "$@" ;;
    zypper) $SUDO zypper --non-interactive install "$@" ;;
    apk)    $SUDO apk add "$@" ;;
    xbps)   $SUDO xbps-install -Sy "$@" ;;
    *)      warn "unknown package manager — please install by hand: $*" ;;
  esac
}

# are the build dependencies already there? (makes re-runs instant)
deps_present() {
  command -v cc        >/dev/null 2>&1 &&
  command -v git       >/dev/null 2>&1 &&
  command -v cargo     >/dev/null 2>&1 &&
  command -v pkg-config >/dev/null 2>&1 &&
  pkg-config --exists xkbcommon
}

install_build_deps() {
  case "$PKGMGR" in
    apt)    install_pkgs build-essential pkg-config libxkbcommon-dev git curl ;;
    pacman) install_pkgs base-devel pkgconf libxkbcommon git curl rust ;;
    dnf)    install_pkgs gcc gcc-c++ make pkgconf-pkg-config libxkbcommon-devel git curl rust cargo ;;
    zypper) install_pkgs gcc gcc-c++ make pkgconf-pkg-config libxkbcommon-devel git curl rust cargo ;;
    apk)    install_pkgs build-base pkgconf libxkbcommon-dev git curl rust cargo ;;
    xbps)   install_pkgs base-devel pkg-config libxkbcommon-devel git curl rust ;;
    gentoo) warn "gentoo: emerge dev-build/rust dev-vcs/git pkgconf x11-libs/libxkbcommon yourself,"
            warn "then re-run this script (it skips packages that are already present)."
            command -v cargo >/dev/null 2>&1 || die "install rust, then re-run" ;;
    nixos)  warn "nixos: nix-shell -p rustc cargo pkg-config libxkbcommon git, then re-run" ;;
    *)      warn "unknown distro — make sure you have: cc make pkg-config libxkbcommon-dev git curl" ;;
  esac
}

# ---------- rust toolchain (rustup first on apt: distro rustc is often old) ----------
ensure_rust() {
  if command -v cargo >/dev/null 2>&1; then
    ok "cargo found: $(cargo --version)"
    return 0
  fi
  if [ "$NO_RUSTUP" -eq 1 ]; then
    die "no system cargo and --no-rustup forbids rustup — install rust via your distro, then re-run"
  fi
  step "Installing Rust via rustup (official toolchain)"
  if curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal; then
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
    export PATH="$HOME/.cargo/bin:$PATH"
  fi
  if ! command -v cargo >/dev/null 2>&1 && [ "$PKGMGR" = apt ]; then
    warn "rustup failed — falling back to the distro toolchain"
    install_pkgs rustc cargo
  fi
  command -v cargo >/dev/null 2>&1 || die "no cargo yet — install https://rustup.rs and re-run"
  ok "cargo ready: $(cargo --version)"
}

# ---------- download + build + install ----------
make_tmpdir() { # sets WORK, registers cleanup
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
}

# Map uname -m to the release-artifact target triple; empty = unsupported.
detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64)  printf 'x86_64-unknown-linux-gnu\n' ;;
    aarch64|arm64) printf 'aarch64-unknown-linux-gnu\n' ;;
    *)             printf '\n' ;;
  esac
}

# Latest release tag via git (no API rate limits); empty when none.
latest_tag() {
  _url="https://github.com/$REPO.git"
  git ls-remote --tags "$_url" 'v*' 2>/dev/null \
    | sed 's|.*refs/tags/||' \
    | grep -v '\^{}$' \
    | sort -V \
    | tail -n 1
}

# Try the prebuilt release artifact: download, verify SHA256, unpack to
# $WORK/foto. Returns 0 and sets BINARY on success; 1 to fall back to a
# source build (no release yet, unsupported arch, network trouble, or a
# checksum mismatch — never installs an unverified binary).
try_prebuilt() {
  # only for the real GitHub repo; a FOTO_REPO override means local testing
  [ "$REPO" = "SONGOKU00mjk/foto" ] || return 1
  TARGET="$(detect_arch)"
  [ -n "$TARGET" ] || { step "unsupported arch $(uname -m) — building from source"; return 1; }

  BASE="https://github.com/$REPO/releases/latest/download"
  TAR="foto-$TARGET.tar.gz"
  step "Trying prebuilt binary: $TAR"
  curl -fsSL --max-time 120 -o "$WORK/$TAR" "$BASE/$TAR" || {
    warn "no prebuilt artifact (first release not tagged yet?) — building from source"
    return 1
  }
  # checksum: fetch the release SHA256SUMS and verify our tarball line
  if curl -fsSL --max-time 60 -o "$WORK/SHA256SUMS" "$BASE/SHA256SUMS" \
     && (cd "$WORK" && grep " $TAR\$" SHA256SUMS | sha256sum -c - >/dev/null 2>&1); then
    ok "SHA256 checksum verified"
  else
    warn "checksum missing or mismatched — NOT installing an unverified binary"
    return 1
  fi
  (cd "$WORK" && tar xzf "$TAR") || return 1
  # the tarball's top dir carries the version — locate the binary generically
  BINARY="$(find "$WORK" -type f -name foto | head -n 1)"
  [ -n "$BINARY" ] && [ -f "$BINARY" ] || return 1
  return 0
}

clone_repo() { # sets WORK and SRC — git clone, works with any default branch
  make_tmpdir
  command -v git >/dev/null 2>&1 || die "git not found — install git and re-run"
  case "$REPO" in
    https://*|http://*|git@*|*://*|/*|./*) _url="$REPO" ;;
    *) _url="https://github.com/$REPO.git" ;;
  esac
  # release installs pin the latest tag instead of a moving branch;
  # set -- builds the clone argument list in portable POSIX sh
  set --
  if [ "$REPO" = "SONGOKU00mjk/foto" ] && _tag="$(latest_tag)" && [ -n "$_tag" ]; then
    step "Latest release tag: $_tag"
    set -- "$@" --branch "$_tag"
  fi
  step "Cloning $_url with git"
  git clone --depth=1 "$@" "$_url" "$WORK/foto" \
    || die "git clone failed — check the repo exists and is public: $REPO"
  SRC="$WORK/foto"
  [ -f "$SRC/Cargo.toml" ] || die "cloned, but Cargo.toml is missing — is the code pushed to GitHub?"
}

build() {
  step "Building foto — first run compiles ~200 crates, expect a few minutes"
  ( cd "$SRC" && cargo build --release ) \
    || die "build failed (is your rustc too old? try rustup: curl https://sh.rustup.rs | sh)"
  ok "built $SRC/target/release/foto"
}

install_binary() { # install_binary <path-to-binary>
  _bin="$1"
  [ -f "$_bin" ] || die "binary not found: $_bin"
  chmod +x "$_bin"
  if [ "$SCOPE" = "user" ]; then
    BIN_DIR="$HOME/.local/bin"
    mkdir -p "$BIN_DIR"
    cp "$_bin" "$BIN_DIR/foto"
    case ":$PATH:" in
      *":$HOME/.local/bin:"*) : ;;
      *)
        warn "$HOME/.local/bin is not in your PATH — add it, e.g.:"
        warn "  echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.profile   # then log out & in"
        ;;
    esac
  else
    BIN_DIR="$PREFIX"   # --prefix DIR / FOTO_PREFIX (default /usr/local/bin)
    $SUDO mkdir -p "$BIN_DIR"
    $SUDO cp "$_bin" "$BIN_DIR/foto"
  fi
  ok "installed to $BIN_DIR/foto"
  # what did we just put there? (best effort — never fails the install)
  if _v="$("$BIN_DIR/foto" --version 2>/dev/null)"; then
    ok "$_v"
  fi
  # and how big is it? foto ships size-tuned (fat LTO, opt-level "s",
  # panic=abort, stripped) so the number is worth printing
  if _size="$(du -h "$BIN_DIR/foto" 2>/dev/null | cut -f1)" && [ -n "$_size" ]; then
    ok "size on disk: $_size (stripped, fat-LTO, panic=abort)"
  fi
}

# ---------- auto-detect wallpaper setter, terminal, font, icons ----------
detect_wallpaper() {
  if command -v swaybg >/dev/null 2>&1; then
    WP_CMD='pkill swaybg; swaybg -m fill -i'
    WP_NAME='swaybg'
  elif command -v awww >/dev/null 2>&1; then
    WP_CMD='awww img'
    WP_NAME='awww (start its daemon once per session: awww-daemon &)'
  elif command -v swww >/dev/null 2>&1; then
    WP_CMD='swww img'
    WP_NAME='swww (older name of awww — start swww-daemon &)'
  elif command -v hyprctl >/dev/null 2>&1 && command -v hyprpaper >/dev/null 2>&1; then
    # hyprpaper needs preload + wallpaper; foto appends the path to the
    # command line, so a nested sh -c hands it to the wrapper as "$1".
    WP_CMD="sh -c 'hyprctl hyprpaper preload \\\"\$1\\\" ; hyprctl hyprpaper wallpaper \\\",\$1\\\"' _"
    WP_NAME='hyprpaper (via hyprctl)'
  else
    WP_CMD='pkill swaybg; swaybg -m fill -i'
    WP_NAME='swaybg — NOT INSTALLED YET: install swaybg or awww'
    warn "no wallpaper setter found — installing one is recommended (swaybg or awww)"
  fi
}

# first Nerd Font fc-list can find (preferred families first); else monospace
detect_font() {
  FONT_NAME='monospace'
  FONT_NOTE='monospace — no Nerd Font found (install one, e.g. ttf-jetbrainsmono-nerd, for icon glyphs)'
  if command -v fc-list >/dev/null 2>&1; then
    _f=''
    for _p in 'JetBrainsMono Nerd' 'CaskaydiaCove Nerd' 'Hack Nerd' 'FiraCode Nerd' 'MesloLGS Nerd' 'Symbols Nerd'; do
      _f="$(fc-list : family 2>/dev/null | grep -i -m1 -- "$_p" | cut -d',' -f1)"
      if [ -n "$_f" ]; then break; fi
    done
    if [ -z "$_f" ]; then
      _f="$(fc-list : family 2>/dev/null | grep -i -m1 'nerd font' | cut -d',' -f1)"
    fi
    if [ -n "$_f" ]; then
      FONT_NAME="$_f"
      FONT_NOTE='Nerd Font detected'
    fi
  fi
  return 0
}

# first well-known icon theme present on disk; else the hicolor fallback
detect_icons() {
  ICON_NAME='hicolor'
  ICON_NOTE='hicolor (fallback — install an icon theme like Papirus for the best look)'
  for _d in "$HOME/.local/share/icons" "$HOME/.icons" /usr/share/icons; do
    if [ -d "$_d" ]; then
      for _t in Papirus Tela Nordzy Fluent Numix Faenza Adwaita breeze elementary; do
        if [ -f "$_d/$_t/index.theme" ]; then
          ICON_NAME="$_t"
          ICON_NOTE="$_t (detected)"
          return 0
        fi
      done
    fi
  done
  return 0
}

detect_terminal() {
  TERM_CMD='foot -e'
  TERM_NAME='foot (default)'
  for t in foot kitty alacritty ghostty; do
    if command -v "$t" >/dev/null 2>&1; then
      TERM_CMD="$t -e"
      TERM_NAME="$t"
      break
    fi
  done
}

# ---------- write the starter config (never overwrites an existing one) ----------
write_config() {
  CFG_DIR="$HOME/.config/foto"
  CFG="$CFG_DIR/config.kdl"
  if [ -f "$CFG" ]; then
    ok "config already exists — keeping it ($CFG)"
    return 0
  fi
  mkdir -p "$CFG_DIR"
  detect_wallpaper
  detect_terminal
  detect_font
  detect_icons
  cat > "$CFG" <<EOF
// foto configuration — generated by install.sh on $(date +%Y-%m-%d)
// Every value below is the default, except the auto-detected lines:
//   font       -> $FONT_NAME   ($FONT_NOTE)
//   icon-theme -> $ICON_NAME   ($ICON_NOTE)
//   terminal   -> $TERM_NAME   (launches apps with Terminal=true)
//   open-with  -> $WP_NAME     (sets the wallpaper in --files mode)
// Delete what you don't want to change; unknown keys are errors on purpose.

font        "$FONT_NAME"    // try a Nerd Font family name, e.g. "ComicShannsMono Nerd"
font-size   15.0
icon-size   24              // square side in pixels, for app icons and thumbnails
icon-theme  "$ICON_NAME"    // icon theme for app icons
terminal    "$TERM_CMD"     // e.g. "kitty -e", "alacritty -e", "ghostty -e"
open-with   "$WP_CMD"
                            // --files mode: command run on Enter, path appended.
                            // swaybg kills the old instance first (it cannot
                            // replace itself). awww needs its daemon running.
files-dir   ""              // default folder for --files, e.g. "~/Pictures/wallpapers";
                            // once set, Tab inside foto toggles apps <-> images
layout      "list"          // "list" rows or "grid" thumbnails
grid-size   96              // --files grid: thumbnail cell size in pixels
layer       #true           // KDL booleans take a '#'. #true = floating overlay
                            // like fuzzel (works on niri, Hyprland, Sway, mango...);
                            // #false = normal tileable window
position    "center"        // "center" (middle of the screen), "top" or "bottom"
margin-y    0               // gap from top/bottom edge for top/bottom (e.g. 40)
border-radius 12            // corner rounding in pixels (anti-aliased); 0 = square
match-mode  "scored"        // "scored" (fzf-like) or "substring"
width       640             // window size (compositors may override)
height      440

colors {
    background  "#1d2021f2"  // "RRGGBB" or "RRGGBBAA" (last 2 digits = transparency)
    foreground  "#ebdbb2"
    selected    "#504945"
    border      "#83a598"
}
EOF
  ok "wrote $CFG (font: $FONT_NAME; icons: $ICON_NAME; wallpaper: $WP_NAME)"
}

# ---------- remove an installed foto (keeps your config + cache) ----------
uninstall() {
  step "Removing foto…"
  resolve_sudo
  _removed=''
  for _bin in "$HOME/.local/bin/foto" "$PREFIX/foto"; do
    [ -f "$_bin" ] || continue
    if rm -f "$_bin" 2>/dev/null; then
      ok "removed $_bin"
      _removed=1
    elif [ -n "$SUDO" ] && $SUDO rm -f "$_bin" 2>/dev/null; then
      ok "removed $_bin (with sudo)"
      _removed=1
    else
      warn "could not remove $_bin — no permission?"
    fi
  done
  [ -n "$_removed" ] || warn "no foto binary found in ~/.local/bin or $PREFIX"
  printf '\n'
  step "Kept (delete by hand for a full wipe):"
  printf '  config:  rm -rf %s\n' "$HOME/.config/foto"
  printf '  cache:   rm -rf %s   # launch history + thumbnails\n' "$HOME/.cache/foto"
  printf '\nReinstall any time with the one-liner from the README.\n\n'
}

# ---------- print the keybind snippet for the running compositor ----------
compositor_hint() {
  printf '\n'
  if [ -n "${NIRI_SOCKET-}" ]; then
    step "Looks like niri — add this to ~/.config/niri/config.kdl:"
    printf 'binds {\n    Mod+D { spawn "foto"; }\n    Mod+F { spawn "foto" "--files"; }\n}\n'
  elif [ -n "${HYPRLAND_INSTANCE_SIGNATURE-}" ]; then
    step "Looks like Hyprland 0.55+ — add this to ~/.config/hypr/hyprland.lua:"
    printf 'hl.bind("SUPER + D", hl.dsp.exec_cmd("foto"))\nhl.bind("SUPER + F", hl.dsp.exec_cmd("foto --files"))\n'
    step "(Hyprland <= 0.54 uses hyprland.conf instead: bind = SUPER,D,exec,foto)"
  elif [ -n "${SWAYSOCK-}" ]; then
    step "Looks like Sway — add this to ~/.config/sway/config:"
    printf 'bindsym $mod+d exec foto\nbindsym $mod+f exec foto --files\n'
  elif printf '%s' "${XDG_CURRENT_DESKTOP-}" | grep -qi mango; then
    step "Looks like mango — add this to your keybind config:"
    printf 'bind = SUPER,d,spawn,foto\nbind = SUPER,f,spawn,foto --files\n'
  else
    step "Bind foto to a key — snippets for niri / Hyprland / Sway / mango are in the README"
  fi
  printf '\nAll done! Run:  foto      (or: foto --files to browse images)\n\n'
}

# ---------- main ----------
OPT_CONFIG_ONLY=0
OPT_UNINSTALL=0

parse_flags() {
  while [ $# -gt 0 ]; do
    case "$1" in
      --system)
        if [ "$SCOPE" = "user" ]; then
          die "--user and --system are mutually exclusive — pick one"
        fi
        SCOPE='system' ;;
      --user)
        if [ "$SCOPE" = "system" ]; then
          die "--user and --system are mutually exclusive — pick one"
        fi
        SCOPE='user' ;;
      --prefix)
        [ $# -ge 2 ] || die "--prefix needs a directory argument"
        PREFIX="$2"
        shift ;;
      --no-rustup)     NO_RUSTUP=1 ;;
      --config-only)   OPT_CONFIG_ONLY=1 ;;
      --uninstall)     OPT_UNINSTALL=1 ;;
      -h|--help)       usage; exit 0 ;;
      *)               die "unknown option: $1 (see --help)" ;;
    esac
    shift
  done
}

main() {
  parse_flags "$@"

  if [ "$OPT_UNINSTALL" -eq 1 ]; then
    if [ "$OPT_CONFIG_ONLY" -eq 1 ]; then
      die "--uninstall and --config-only are mutually exclusive"
    fi
    uninstall
    return 0
  fi

  if [ "$OPT_CONFIG_ONLY" -eq 0 ]; then
    [ "$(uname -s)" = "Linux" ] || die "foto needs Linux with a Wayland compositor"
    PKGMGR="$(detect_pkg)"
    step "Detected distro package manager: $PKGMGR"
    choose_scope
  fi

  if [ "$OPT_CONFIG_ONLY" -eq 1 ]; then
    write_config
    compositor_hint
    return 0
  fi

  # system-wide needs root or sudo; without either we degrade to a
  # user-only install instead of failing after the long build
  apply_scope_sudo

  # prebuilt first: no compiler, no clone, no build — seconds instead of
  # minutes. Falls back to a source build whenever anything is off.
  make_tmpdir
  if try_prebuilt; then
    install_binary "$BINARY"
    write_config
    compositor_hint
    return 0
  fi

  if deps_present; then
    ok "build dependencies already present — skipping package install"
  else
    if [ "$(id -u)" -ne 0 ] && [ -z "$SUDO" ]; then
      die "no sudo — cannot install packages; install cc, git, pkg-config, libxkbcommon-dev and rust by hand, then re-run"
    fi
    install_build_deps
  fi
  ensure_rust
  clone_repo
  build
  install_binary "$SRC/target/release/foto"

  write_config
  compositor_hint
}

# FOTO_INSTALLER_TEST=1 lets the test suite source this file without running
if [ -z "${FOTO_INSTALLER_TEST:-}" ]; then
  main "$@"
fi
