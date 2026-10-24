# foto

A minimal Wayland-only app launcher with image preview, written in Rust.
fuzzel-inspired philosophy: one binary, no GTK, no Qt — just the Wayland
protocol, a software rasterizer and a font shaper.

**Compositor-agnostic**: foto runs on *any* Wayland compositor that
speaks the `wlr-layer-shell` protocol — niri, Hyprland, Sway, river,
mango, labwc, Wayfire, KWin and the rest of the wlroots family. GNOME
is the only holdout (it has no layer-shell): there, `foto --no-layer`
gives you a normal window, and the TUI fallback works everywhere.

```
┌─ foto ────────────────────────────────┐
│ [images] > wall▏                      │
│ ─────────────────────────────────────  │
│   wall-blue.png                       │
│ › wall-red.png                        │
│   Firefox      Web Browser            │
│   LibreOffice  Office                 │
└───────────────────────────────────────┘
```

## Quick install

One line — paste it in your terminal and you're done. The script
identifies your distro, installs **everything foto needs to build and
run** (compiler, git, pkg-config, libxkbcommon — the only native library
foto links against; the Wayland protocol is spoken by foto itself in
pure Rust — plus Rust via rustup if you have no cargo yet), then **clones
this repository**, builds it, installs the binary, and writes a starter
`~/.config/foto/config.kdl` with your **wallpaper setter (swaybg or
awww)** and **terminal** auto-detected:

```sh
curl -fsSL https://raw.githubusercontent.com/SONGOKU00mjk/foto/main/install.sh | sh
```

The installer **asks where to put the binary**:

```
  Where should foto be installed?
    1) System-wide  ->  /usr/local/bin   (recommended, needs sudo)
    2) Just for you ->  ~/.local/bin     (no sudo, must be in your PATH)
  Choose [1]:
```

Just press **Enter** for the recommended **system-wide** install; type `2`
for a user-only one. This works even when piped straight from curl. If you
prefer to skip the question: `--system` (system-wide, recommended) or
`--user` (`~/.local/bin`). Also available: `--config-only` (just write the
config, nothing else). Supports Debian/Ubuntu, Arch, Fedora, openSUSE,
Alpine and Void out of the box — anything else, it tells you what to
install by hand and still tries the build. Re-runs are cheap:
already-present dependencies are skipped.

### What the installer auto-detects into your config

The generated `~/.config/foto/config.kdl` is not a generic template —
the installer detects your system and pre-fills:

- the **wallpaper setter** for `open-with`: `swaybg`, `awww`/`swww`, or
  `hyprpaper` (wrapped so a single Enter preloads and sets the image)
- the **terminal** for `Terminal=true` apps: `foot`, `kitty`,
  `alacritty`, or `ghostty`
- a **Nerd Font** for `font` (JetBrainsMono, CaskaydiaCove, Hack,
  FiraCode, MesloLGS, Symbols — falls back to `monospace` if none)
- your **icon theme** for `icon-theme`: Papirus, Tela, Nordzy, Fluent,
  Numix, Faenza, Adwaita, breeze, elementary — falls back to `hicolor`

## Other ways to install

**Arch Linux / AUR** — the repo ships ready-to-build recipes in
`packaging/aur/` (`foto` for releases, `foto-git` for the latest main).
After they are published on aur.archlinux.org, Arch users simply run:

```sh
yay -S foto          # or: paru -S foto
yay -S foto-git      # git version, tracks main
```

Until then, the same recipe builds by hand:

```sh
cp packaging/aur/PKGBUILD .
makepkg -si         # builds and installs with pacman
```

**NixOS / Nix** — a flake is included, so no publishing step is needed —
it installs straight from this GitHub repo:

```sh
nix profile install github:SONGOKU00mjk/foto     # install into a profile
nix run github:SONGOKU00mjk/foto                 # try it without installing
```

or add it to your own `flake.nix` inputs:

```nix
inputs.foto.url = "github:SONGOKU00mjk/foto";
```

The flake reads the version directly from `Cargo.toml` and builds the
default features (gui + tui).

## Requirements

- Linux with a Wayland compositor (niri, Hyprland, Sway, mango, ...)
- **Feature flags**: `gui` (Wayland frontend: wayland-client, sctk,
  tiny-skia, cosmic-text, resvg) and `tui` (ratatui, crossterm) — both on
  by default. `cargo build --no-default-features --features tui` gives a
  terminal-only binary; `--no-default-features` alone is a compile error
  by design (a launcher with no frontend has nothing to run).
- **MSRV 1.95** — the minimum Rust toolchain when building from source
  (verified in CI; the current dependency set — `kdl 6.7` — is what sets
  the floor). Prebuilt release binaries have no Rust requirement at all.

## Features

- **Floating overlay like fuzzel** — foto opens as a **layer-shell surface**
  (`zwlr_layer_shell_v1`): it floats above every window, grabs the keyboard
  instantly and never gets tiled — on niri, Hyprland, Sway, mango and
  every other layer-shell compositor. No window rules needed. `foto --no-layer` (or `layer #false`)
  falls back to a normal tileable window.
- **Centered on screen** — by default the overlay sits in the **middle** of
  the output; `position "top"` / `"bottom"` pin it to an edge instead
  (dmenu/taskbar style), with `margin-y` for the gap.
- **Rounded corners** — the panel is drawn with anti-aliased rounded corners
  (`border-radius 12` by default; the area outside the rounding is fully
  transparent so the desktop shows through). `border-radius 0` = square.
- **App launcher** — scans `.desktop` files from the XDG data directories,
  resolves icons from your icon theme (PNG and SVG), launches apps
  (wrapping `Terminal=true` apps in your terminal).
- **Fast startup** — icon themes are indexed (one directory walk per theme,
  every lookup a hash hit) instead of re-walking the tree per icon, which
  keeps the first frame quick even on huge themes like Papirus (~60,000
  files) — most noticeably on the first launch after a fresh boot.
- **Image browser** — `foto --files [DIR]` (default `~/Pictures` or the
  config's `files-dir`) lists images with live thumbnails, optionally as a
  side-by-side **grid** (`layout "grid"`); Enter sets the wallpaper with
  **swaybg** by default (`open-with` can run anything: `awww img` — the
  project formerly known as swww — `hyprpaper` (the installer writes a
  working wrapper for it), `xdg-open`, `imv`…). Thumbnails decode
  on **background threads** (the window opens instantly, images pop in)
  and are cached on disk (`~/.cache/foto`), so the next launch is instant.
- **One launcher for both** — once `files-dir` is set, press **Tab** inside
  foto to switch between apps and images at any time.
- **dmenu mode** — `foto --dmenu` reads menu items from stdin and prints the
  chosen line to stdout: plug it into **cliphist** exactly like fuzzel
  (recipe below).
- **Two fuzzy matchers** — `scored` (fzf-style ranking) and `substring`,
  switchable in the config.
- **Launch history** — foto remembers what you open most (kept in
  `~/.cache/foto/history`) and floats your day-to-day favorites to the top
  of the list, exactly like fuzzel's frequency ranking. An empty query
  lists apps in usage order; typing still wins when it clearly names
  something else. `history #false` turns it off.
- **Mouse support** — point at a row (or a grid thumbnail) to select it,
  left-click to launch it, and scroll the wheel to move through the list.
  Hovering and clicking do the same thing as ↑/↓ and Enter.
- **TUI fallback** — no Wayland session? foto automatically runs a full
  terminal UI (ratatui) instead. `foto --tui` forces it.

## Build & install

You need the usual Rust toolchain and `libxkbcommon` development headers
(every Wayland tool needs this; on Debian/Ubuntu `libxkbcommon-dev`,
Arch `libxkbcommon`, Fedora `libxkbcommon-devel`).

```sh
cargo build --release
cp target/release/foto ~/.local/bin/     # or /usr/local/bin
```

Optional but recommended — make the compositor render SVG icons correctly:
nothing extra is needed; SVG support is built in via resvg.

## Usage

```
foto                     # app launcher (Wayland GUI, or TUI if no Wayland)
foto --files             # browse images in ~/Pictures
foto --files ~/wallpapers
foto --dmenu             # dmenu: read items from stdin, print the choice
foto --no-layer          # normal tileable window instead of the overlay
foto --tui               # force the terminal UI
foto --files --recursive # also scan subdirectories (depth-limited walk)
foto --list              # print detected entries and exit (debugging)
foto --print-config      # print an annotated default config
```

Keys (identical in GUI and TUI):

| Key                | Action                          |
|--------------------|---------------------------------|
| typing             | fuzzy-filter the list (inserted at the caret) |
| ↑ / ↓ or Ctrl-J/K  | move selection (whole row in grid) |
| ← / →              | **caret** left/right while the query is non-empty; with an empty query: step one item / one cell |
| Tab                | switch apps ⇄ images (when files-dir is set) |
| Home / End or Ctrl-A / Ctrl-E | **caret** to start/end while typing; with an empty query: jump to top / bottom |
| Enter              | run app / set wallpaper / choose dmenu item |
| Esc or Ctrl-C      | quit                            |
| Backspace          | delete character before the caret |
| Delete             | delete character at the caret   |
| Ctrl-W             | delete word before the caret    |
| Ctrl-U             | clear query                     |

Why the split: Up/Down never collide with text editing, so they always
drive the list. Left/Right and Home/End do collide — a caret you cannot
move is useless — so they edit the query whenever there is one to edit,
and fall back to list navigation when it is empty (nothing to edit).
Ctrl-J/K are the always-available selection keys if you prefer to keep
your hands on the home row.

## Configuration — `~/.config/foto/config.kdl`

```kdl
font        "monospace"     // e.g. "ComicShannsMono Nerd" — Nerd Font name
                            // matching is forgiving, see the Fonts section
font-size   15.0
icon-size   24
icon-theme  "hicolor"
terminal    "foot -e"        // e.g. "kitty -e", "alacritty -e", "ghostty -e"
open-with   "pkill swaybg; swaybg -m fill -i"
                             // --files mode: command run on Enter (see below)
files-dir   ""               // default folder for --files (e.g. "~/Pictures/wallpapers")
layout      "list"           // --files mode: "list" rows or "grid" thumbnails
grid-size   96               // --files grid: thumbnail cell size in pixels
layer       #true            // KDL booleans take a '#': #true = layer-shell
                             // overlay like fuzzel, #false = normal window
position    "center"         // "center" (middle of the screen — "middle" also
                             // works), "top" or "bottom"; needs layer #true
margin-y    0                // gap from the top/bottom edge for top/bottom
                             // positions (e.g. 40 to clear a waybar)
border-radius 12            // corner rounding in pixels; 0 = square corners
match-mode  "scored"         // or "substring"
history     #true            // remember launches + rank favorites first
width       640
height      440

colors {
    background  "#1d2021f2"  // "RRGGBB" or "RRGGBBAA" (last 2 digits = alpha)
    foreground  "#ebdbb2"
    selected    "#504945"
    border      "#83a598"
}
```

A ready-made copy lives in `config.example.kdl` and `foto --print-config`
prints the same. Unknown keys are treated as errors on purpose — typos
should not silently do nothing. Missing file = all defaults.

### Ready-made themes

Every color takes `#RRGGBB` or `#RRGGBBAA` (the last two digits are
transparency — `f2` in the default background is a nearly opaque panel).
Paste a whole `colors` block into your config to restyle the panel:

**Tokyo Night** (deep blue, moonlit purple border):

```kdl
colors {
    background  "#1a1b26e6"
    foreground  "#c0caf5"
    selected    "#33467c"
    border      "#7aa2f7"
}
```

**Catppuccin Mocha** (soft dark, lavender accent):

```kdl
colors {
    background  "#1e1e2ee6"
    foreground  "#cdd6f4"
    selected    "#45475a"
    border      "#b4befe"
}
```

**Nord** (cold, calm, frost-blue border):

```kdl
colors {
    background  "#2e3440e6"
    foreground  "#d8dee9"
    selected    "#434c5e"
    border      "#88c0d0"
}
```

### Launch history

Every successful launch bumps a counter in `~/.cache/foto/history` — apps
are tracked by their `Exec=`, images by path, dmenu lines by text. On the
next run the counts feed a capped score bonus, so:

- with an empty query, the list is ordered by how much you use things;
- with a query, relevance still wins — the bonus only breaks near-ties,
  so searching for a rarely-used app never buries it under a favorite.

Delete the file to reset all counters, or set `history #false` to turn
recording and ranking off entirely.

### Turn `--files` into a wallpaper picker

In `--files` mode Enter runs `pkill swaybg; swaybg -m fill -i <file>` by
default — your main wallpaper setter, replace the old wallpaper safely.
The `open-with` key swaps that command (paths are shell-quoted
automatically):

```kdl
open-with  "pkill swaybg; swaybg -m fill -i"     // default (swaybg)
// open-with "awww img"                           // awww (formerly swww)
// open-with "xdg-open"                           // open in a viewer
// open-with "feh --bg-fill"                      // X11 / feh
```

Note the `pkill` before swaybg: swaybg doesn't replace a running
instance, so kill the old one first or processes pile up. The whole
string runs through `sh -c`, so shell syntax is fine — the image path
is appended shell-quoted at the end. For awww, start its daemon once
per session (`awww-daemon &` in your autostart; older docs call it
`swww-daemon`).

### Choose apps or images after opening — Tab

Set `files-dir` in the config and foto starts as the **app launcher**
but knows about your images too: press **Tab** to flip to the images
list (grid or list, your choice), press **Tab** again to go back to
apps. The prompt badge shows the active mode (`[apps]`, `[images]`),
the footer shows what Tab switches to, and the query resets on switch.
Of course `foto --files` still starts directly in images mode.

### Clipboard history with cliphist — dmenu mode

fuzzel-style pipelines work through `foto --dmenu`: foto reads one menu
item per line from stdin, you fuzzy-filter and pick, and foto prints
the chosen line to stdout. The classic **cliphist** recipe:

```sh
cliphist list | foto --dmenu | cliphist decode | wl-copy
```

Bind it to a key:

**niri** (`~/.config/niri/config.kdl`):

```kdl
binds {
    Mod+V { spawn "sh" "-c" "cliphist list | foto --dmenu | cliphist decode | wl-copy"; }
}
```

**Hyprland 0.55+** (`~/.config/hypr/hyprland.lua`):

```lua
hl.bind("SUPER + V", hl.dsp.exec_cmd("sh -c 'cliphist list | foto --dmenu | cliphist decode | wl-copy'"))
```

**Sway** (`~/.config/sway/config`):

```conf
bindsym $mod+v exec sh -c 'cliphist list | foto --dmenu | cliphist decode | wl-copy'
```

**mango** (`~/.config/mango/config.conf` or your binds file):

```conf
bind = SUPER,V,spawn,sh -c 'cliphist list | foto --dmenu | cliphist decode | wl-copy'
```

(`sh -c` wrapper is needed so the pipes survive. In mango, remember to
install and start `cliphist` and, if you use it, `wl-clip-persist` to
keep the clipboard alive between apps.)

Esc closes foto without printing anything, exactly like dmenu cancelling.
The same mode also works in the TUI fallback (`foto --tui --dmenu`).

### Wallpapers with awww (formerly swww) transitions

After pointing `open-with` at awww, picking a wallpaper looks like this:
`foto --files ~/Pictures/wallpapers`, arrows to choose, Enter — done.
awww even gives you fancy transitions:

```kdl
open-with  "awww img --transition-type grow --transition-step 30"
```

### A comfortable wallpaper browser

Skip the argument and the folder comes from the config; with the grid
layout it feels like a wallpaper gallery (large window recommended):

```kdl
files-dir  "~/Pictures/wallpapers"
layout     "grid"
grid-size  120
width      800
height     600
```

Then just bind `foto --files` to a key — or don't: with `files-dir` set,
plain `foto` plus Tab gets you there. In grid mode ↑/↓ jump a whole row
of thumbnails (←/→ step one cell), filenames truncate to fit under each
cell, and the TUI fallback stays a normal list (terminals can't show
images). Thumbnails render on worker threads, so even a folder of huge
wallpapers opens the window immediately — cells light up as they decode,
and every image is cached in `~/.cache/foto/thumbs` for instant reuse.

### Fonts

The GUI renders with the family named by `font`. Use the exact family
name `fc-list` shows — Nerd Fonts are usually called e.g. `ComicShanns
Mono Nerd Font`. foto is forgiving: it also tries appending `Nerd Font
Mono`, `Nerd Font` and `Mono` to what you typed, and falls back to the
default monospace if nothing matches. The TUI's font comes from your
terminal emulator, not this config.

## Bind it to a key

The overlay floats by itself on any layer-shell compositor — niri,
Hyprland, Sway, river, mango, labwc, Wayfire… — so no window rules are
needed anywhere. Just spawn `foto`; pick your compositor below. Note
the syntax differences: **Hyprland 0.55+ uses a Lua config** (`hl.bind`);
Sway and river use spaces (also with `exec`); niri and mango are shown
as-is.

> **If your keybinds stopped working after a Hyprland update:** Hyprland
> 0.55+ reads `~/.config/hypr/hyprland.lua` first and only falls back to
> the old `hyprland.conf` when no `.lua` file exists. If you have both,
> the `.conf` is silently ignored — move your binds into the `.lua`.

**niri** (`~/.config/niri/config.kdl`):

```kdl
binds {
    Mod+D { spawn "foto"; }
    Mod+F { spawn "foto" "--files"; }
}
```

**Hyprland 0.55+** (`~/.config/hypr/hyprland.lua`):

```lua
hl.bind("SUPER + D", hl.dsp.exec_cmd("foto"))
hl.bind("SUPER + F", hl.dsp.exec_cmd("foto --files"))
```

**Hyprland ≤ 0.54** (`~/.config/hypr/hyprland.conf`):

```conf
bind = SUPER,D,exec,foto
bind = SUPER,F,exec,foto --files
```

**Sway / river / most wlroots compositors** (`~/.config/sway/config`):

```conf
bindsym $mod+d exec foto
bindsym $mod+f exec foto --files
```

**mango** (`~/.config/mango/config.conf` or your keybind config):

```conf
bind = SUPER,d,spawn,foto
bind = SUPER,f,spawn,foto --files
```

(Adjust to your setup. If you ever run `foto --no-layer` or set
`layer #false`, you get a normal tileable window with
`app_id = "foto"` — add a float rule for it only in that case.)

## How it works — a tour for readers

The codebase is deliberately small and layered. Everything below the
frontends is UI-agnostic, so the TUI and the Wayland GUI are thin
translators over the same state machine.

```
main.rs        CLI (clap) → build Config + App → pick a frontend
│
├─ core (no UI)
│  ├─ config.rs   KDL file → Config. One flat block + colors block.
│  ├─ desktop.rs  scan $XDG_DATA_DIRS/applications/**/*.desktop,
│  │              parse [Desktop Entry], skip Hidden/NoDisplay/Link,
│  │              honor TryExec; sanitize Exec field codes (%u etc.)
│  ├─ icons.rs    freedesktop icon-theme lookup:
│  │              closest-size PNG → any SVG → hicolor → pixmaps,
│  │              decoded + scaled + premultiplied, cached by name
│  ├─ files.rs    --files: scan a dir for PNG/JPG/WebP/GIF/BMP + the
│  │              on-disk thumbnail cache (~/.cache/foto/thumbs)
│  ├─ thumbs.rs   background thumbnail engine: worker threads decode
│  │              off the render thread, ping the event loop when done
│  └─ matcher.rs  substring & scored (fzf-like) ranking
│
├─ state.rs      App { entries, alt_entries, query, matches, selected }
│                on_key(Key) → Flow::{Continue, Launch, Exit}
│                both frontends call exactly this; Tab swaps entries
│
└─ frontends
   ├─ tui.rs          ratatui; used when Wayland is missing or --tui
   └─ gui/
      ├─ mod.rs       Wayland session: globals → layer-shell overlay
      │               (zwlr_layer_shell_v1, xdg_toplevel fallback),
      │               wl_seat keyboard (+ key repeat via calloop),
      │               event loop, shm buffer presentation
      └─ render.rs    tiny-skia pixmap: background/border/prompt/rows,
                      cosmic-text shaping → swash glyph blending,
                      icons & thumbnails drawn from the icon cache
```

The Wayland flow, briefly:

1. `Connection::connect_to_env()` connects to the compositor.
2. We bind `wl_compositor`, `wl_shm`, `wl_seat` and — if available —
   `zwlr_layer_shell_v1`. The layer surface sits on the **overlay layer**
   and takes **exclusive keyboard** focus, which is exactly what fuzzel
   does; this is why it floats above everything without any compositor
   rules. With `position "center"` (the default) the surface gets **no
   anchor at all**, and the protocol says unanchored surfaces are placed
   **in the middle of the output**; `top`/`bottom` anchor that edge plus
   `margin-y`. Without layer shell we fall back to a classic
   `xdg_toplevel` window.
3. On each redraw we render a `tiny_skia::Pixmap`, copy it into a
   shared-memory buffer (`wl_shm`, ARGB8888) and commit the surface.
4. Keyboard input arrives as xkb keysyms (with hold-to-repeat); we map them
   to a neutral `Key` enum and feed `App::on_key`, then redraw if needed.
5. In `--files` mode, visible cells ask the thumbnail engine for a bitmap;
   misses are queued to worker threads and the UI repaints as results land
   in through a calloop channel.

## Rendering & HiDPI

foto binds `wp_viewporter` + `wp_fractional_scale_v1` when the compositor
advertises them (niri, Hyprland, Sway, KDE, ...): buffers are allocated at
`ceil(logical size × preferred scale)` physical pixels and squashed back to
the logical size with `wp_viewport.set_destination` before commit, so **text
and geometry render sharp on fractional-scale outputs** (1.25x, 1.5x, 2x,
...). When the scale changes (or the surface enters another output), the
surface re-renders at the new scale automatically.

If either protocol global is missing, foto falls back cleanly to scale 1 —
correct size, slightly soft text, exactly like the pre-HiDPI builds. One
deliberate trade-off: raster icons and `--files` thumbnails are still
fetched at their *logical* size (keeping the disk cache small) and upscaled
with bilinear filtering at draw time, so they can be a touch soft on 2x
outputs while everything vector-drawn stays crisp.

## Current limitations (roadmap ideas)

- `--files` scans one directory by default; pass `--recursive` (or
  `recursive #true`) to descend into subdirectories — the walk is
  depth-limited to 8 and skips dot-directories.
- XPM icons are not decoded — the `image` crate has no XPM support and a
  custom decoder for the legacy C-array format only serves ancient GTK
  apps; foto warns once per process and moves on.
- Icon themes are followed through their full `Inherits=` chain, but a
  theme installed under several data dirs uses the first `index.theme`
  found (the spec's behavior for well-behaved systems).

## License

foto is **free software** under the **GNU General Public License v3 or
later** (`GPL-3.0-or-later`). The full text ships in the `LICENSE`
file, and `Cargo.toml` carries the matching SPDX identifier.

In short: you may run, study, change and redistribute foto, but any
redistribution — modified or not — must remain free software under the
same license. That's the deal the GPL makes: improvements flow back to
everyone.
