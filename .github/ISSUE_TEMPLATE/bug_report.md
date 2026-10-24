---
name: Bug report
about: foto misbehaves — launcher, image browser, dmenu or TUI
title: ''
labels: bug
assignees: ''
---

Thank you for reporting a bug. Please fill in everything below — the
diagnostics are quick to run and they remove most of the guesswork.

## What happened?

A clear description of what you expected and what actually happened.

## How to reproduce

Steps (or a shell snippet) that reliably triggers the bug.

## Diagnostics

Please paste the output of these commands:

```sh
foto --list
```

```sh
foto --print-config
```

```sh
rustc --version
```

## Environment

- **Compositor and version** (e.g. `niri 25.02`, `Hyprland 0.48`, `Sway 1.10`,
  `mango`): 
- **Wayland or TUI?** Does the bug also reproduce with `foto --tui`?
  (yes / no / did not try — but please try, it tells us whether the bug is
  in the shared core or in the GUI frontend)
- **Linux distribution**: 
- **How was foto installed** (install.sh, AUR, from source): 

## Anything else?

Screenshots, config snippets (`~/.config/foto/config.kdl`), or relevant log
lines. If foto crashed, paste the full terminal output.
