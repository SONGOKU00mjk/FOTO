//! KDL configuration: `~/.config/foto/config.kdl`
//!
//! foto reads a flat KDL document. Unknown keys are errors (catches typos),
//! and a missing file simply means "all defaults".

use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

/// RGBA color, straight (non-premultiplied) alpha.
pub type Rgba = [u8; 4];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    /// plain case-insensitive substring
    Substring,
    /// fzf-style subsequence with bonuses (word boundaries, consecutive chars)
    Scored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// one file per row (default)
    List,
    /// thumbnails side by side (--files mode only)
    Grid,
}

/// Where the layer-shell overlay sits on screen (`layer #true` only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Position {
    /// centered on the output (default — the launcher opens in the middle)
    Center,
    /// pinned to the top edge, horizontally centered (dmenu style)
    Top,
    /// pinned to the bottom edge, horizontally centered
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colors {
    pub background: Rgba,
    pub foreground: Rgba,
    pub selected: Rgba,
    pub border: Rgba,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            background: [0x1d, 0x20, 0x21, 0xf2], // gruvbox dark, slightly translucent
            foreground: [0xeb, 0xdb, 0xb2, 0xff],
            selected: [0x50, 0x49, 0x45, 0xff],
            border: [0x83, 0xa5, 0x98, 0xff],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub font: String,
    pub font_size: f32,
    pub icon_size: u32,
    pub icon_theme: String,
    pub terminal: String,
    pub match_mode: MatchMode,
    pub colors: Colors,
    pub width: u32,
    pub height: u32,
    /// command run on Enter in --files mode; the file path is appended,
    /// shell-quoted. Default sets the wallpaper with swaybg; alternatives:
    /// "awww img" (formerly swww), "xdg-open", "imv".
    pub open_with: String,
    /// default directory for `--files` when no argument is given (~/ expanded)
    pub files_dir: Option<String>,
    /// how --files rows are drawn
    pub layout: Layout,
    /// thumbnail cell size in grid mode
    pub grid_size: u32,
    /// --files: also descend into subdirectories (depth-limited walk).
    /// Off by default; `--recursive` enables it for one run.
    pub recursive: bool,
    /// true (default): open as a layer-shell overlay that floats above
    /// everything, exactly like fuzzel — works on niri, Hyprland, Sway, mango
    /// and every other layer-shell compositor. false: open a normal
    /// (tileable) xdg window.
    pub layer: bool,
    /// where the overlay appears on screen: middle (default), top or bottom.
    /// Ignored when `layer` is false — a plain xdg window is placed by the
    /// compositor (its window rules, not foto, decide its spot).
    pub position: Position,
    /// gap in pixels from the top/bottom edge when position is top/bottom
    /// (e.g. 40 to sit below a waybar). Ignored for center.
    pub margin_y: i32,
    /// corner rounding of the panel in pixels (0 = square). The corners are
    /// anti-aliased and the area outside the rounding is fully transparent,
    /// so the desktop shows through behind them.
    pub border_radius: u32,
    /// remember what you launch most (in ~/.cache/foto/history) and rank
    /// those entries first. `#false` turns recording and ranking off.
    pub history: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            font: "monospace".into(),
            font_size: 15.0,
            icon_size: 24,
            icon_theme: "hicolor".into(),
            terminal: "foot -e".into(),
            match_mode: MatchMode::Scored,
            colors: Colors::default(),
            width: 640,
            height: 440,
            open_with: "pkill swaybg; swaybg -m fill -i".into(),
            files_dir: None,
            layout: Layout::List,
            grid_size: 96,
            recursive: false,
            layer: true,
            position: Position::Center,
            margin_y: 0,
            border_radius: 12,
            history: true,
        }
    }
}

impl Config {
    /// Where foto looks for its config: `$XDG_CONFIG_HOME/foto/config.kdl`.
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("foto").join("config.kdl"))
    }

    /// Load the user config, or fall back to defaults if the file is absent.
    pub fn load() -> Result<Self> {
        match Self::config_path() {
            Some(path) if path.exists() => Self::from_file(&path),
            _ => Ok(Self::default()),
        }
    }

    pub fn from_file(path: &std::path::Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading config {}", path.display()))?;
        let mut cfg = Self::default();
        cfg.apply_kdl(&text)
            .with_context(|| format!("parsing config {}", path.display()))?;
        Ok(cfg)
    }

    fn apply_kdl(&mut self, text: &str) -> Result<()> {
        use kdl::KdlDocument;
        let doc: KdlDocument = text.parse()?;

        for node in doc.nodes() {
            let name = node.name().value();
            match name {
                "font" => self.font = str_arg(node)?.to_string(),
                "font-size" => self.font_size = f32_arg(node)?,
                "icon-size" => self.icon_size = f32_arg(node)? as u32,
                "icon-theme" => self.icon_theme = str_arg(node)?.to_string(),
                "terminal" => self.terminal = str_arg(node)?.to_string(),
                "open-with" => self.open_with = str_arg(node)?.to_string(),
                "files-dir" => {
                    let s = str_arg(node)?;
                    self.files_dir = if s.is_empty() {
                        None
                    } else {
                        Some(s.to_string())
                    };
                }
                "layout" => {
                    self.layout = match str_arg(node)? {
                        "list" => Layout::List,
                        "grid" => Layout::Grid,
                        other => {
                            return Err(anyhow!(
                                "layout must be \"list\" or \"grid\", got {other:?}"
                            ))
                        }
                    };
                }
                "grid-size" => self.grid_size = f32_arg(node)?.max(32.0) as u32,
                "recursive" => self.recursive = bool_arg(node)?,
                "layer" => self.layer = bool_arg(node)?,
                "position" => {
                    self.position = match str_arg(node)? {
                        "center" | "middle" => Position::Center,
                        "top" => Position::Top,
                        "bottom" => Position::Bottom,
                        other => {
                            return Err(anyhow!(
                                "position must be \"center\", \"top\" or \"bottom\", got {other:?}"
                            ))
                        }
                    };
                }
                "margin-y" => self.margin_y = f32_arg(node)? as i32,
                "border-radius" => self.border_radius = f32_arg(node)?.max(0.0) as u32,
                "history" => self.history = bool_arg(node)?,
                "width" => self.width = f32_arg(node)? as u32,
                "height" => self.height = f32_arg(node)? as u32,
                "match-mode" => {
                    self.match_mode = match str_arg(node)? {
                        "substring" => MatchMode::Substring,
                        "scored" => MatchMode::Scored,
                        other => {
                            return Err(anyhow!(
                                "match-mode must be \"substring\" or \"scored\", got {other:?}"
                            ))
                        }
                    };
                }
                "colors" => {
                    let children = node
                        .children()
                        .ok_or_else(|| anyhow!("`colors` needs a {{ }} block with children"))?;
                    for c in children.nodes() {
                        let value = parse_color(str_arg(c)?)?;
                        match c.name().value() {
                            "background" => self.colors.background = value,
                            "foreground" => self.colors.foreground = value,
                            "selected" => self.colors.selected = value,
                            "border" => self.colors.border = value,
                            other => return Err(anyhow!("unknown color key {other:?}")),
                        }
                    }
                }
                "theme" => {
                    let name = str_arg(node)?;
                    self.colors = theme_preset(name).ok_or_else(|| {
                        anyhow!(
                            "unknown theme {name:?} — try: gruvbox, tokyo-night, catppuccin, \
                             nord, dracula or everforest (a later colors {{ }} block still wins)"
                        )
                    })?;
                }
                other => return Err(anyhow!("unknown config key {other:?}")),
            }
        }
        Ok(())
    }

    /// An annotated default config, printed by `foto --print-config`.
    pub fn example_kdl() -> &'static str {
        EXAMPLE
    }
}

/// Expand a leading `~/` to the user's home directory.
pub fn expand_tilde(s: &str) -> PathBuf {
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

// ---------- small KDL helpers ----------

/// First positional (nameless) argument of a node: `font "monospace"` -> "monospace".
fn first_positional(node: &kdl::KdlNode) -> Result<&kdl::KdlValue> {
    node.entries()
        .iter()
        .find(|e| e.name().is_none())
        .map(|e| e.value())
        .ok_or_else(|| anyhow!("missing value for `{}`", node.name().value()))
}

fn str_arg(node: &kdl::KdlNode) -> Result<&str> {
    first_positional(node)?
        .as_string()
        .ok_or_else(|| anyhow!("`{}` expects a string", node.name().value()))
}

fn f32_arg(node: &kdl::KdlNode) -> Result<f32> {
    let v = first_positional(node)?;
    v.as_float()
        .or_else(|| v.as_integer().map(|i| i as f64))
        .map(|f| f as f32)
        .ok_or_else(|| anyhow!("`{}` expects a number", node.name().value()))
}

fn bool_arg(node: &kdl::KdlNode) -> Result<bool> {
    first_positional(node)?
        .as_bool()
        .ok_or_else(|| anyhow!("`{}` expects true or false", node.name().value()))
}

/// Parse `#RRGGBB` or `#RRGGBBAA` into straight-alpha RGBA.
pub fn parse_color(s: &str) -> Result<Rgba> {
    let hex = s.trim().trim_start_matches('#');
    let bad = || anyhow!("bad color {s:?} — use #RRGGBB or #RRGGBBAA");
    let byte =
        |i: usize| -> Result<u8> { u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad()) };
    match hex.len() {
        6 => Ok([byte(0)?, byte(2)?, byte(4)?, 0xff]),
        8 => Ok([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => Err(bad()),
    }
}

/// One-word color schemes for the `theme` config key. Everything is the
/// stock palette of the named theme, with a slightly translucent background
/// (the `e6` alpha — 90%) so the desktop glows through, matching the gruvbox
/// default. A `colors { … }` block AFTER the theme line still overrides
/// individual colors; one BEFORE it is replaced.
fn theme_preset(name: &str) -> Option<Colors> {
    let (bg, fg, sel, border) = match name {
        // the built-in default, spelled out so `theme "gruvbox"` is explicit
        "gruvbox" => ("#1d2021f2", "#ebdbb2", "#504945", "#83a598"),
        "tokyo-night" => ("#1a1b26e6", "#c0caf5", "#33467c", "#7aa2f7"),
        "catppuccin" => ("#1e1e2ee6", "#cdd6f4", "#45475a", "#89b4fa"),
        "nord" => ("#2e3440e6", "#d8dee9", "#434c5e", "#88c0d0"),
        "dracula" => ("#282a36e6", "#f8f8f2", "#44475a", "#bd93f9"),
        "everforest" => ("#2d353be6", "#d3c6aa", "#414b50", "#a7c080"),
        _ => return None,
    };
    // preset colors are compile-time constants — parsing cannot fail
    Some(Colors {
        background: parse_color(bg).expect("preset background parses"),
        foreground: parse_color(fg).expect("preset foreground parses"),
        selected: parse_color(sel).expect("preset selected parses"),
        border: parse_color(border).expect("preset border parses"),
    })
}

const EXAMPLE: &str = r##"// foto configuration — copy to ~/.config/foto/config.kdl
// Every value below is the default. Delete what you don't want to change.

font        "monospace"     // try "ComicShannsMono Nerd" — foto matches Nerd Font
                            // family names even if fc-list spells them differently
font-size   15.0
icon-size   24              // square side in pixels, for app icons and image thumbnails
icon-theme  "hicolor"       // icon theme name for app icons
terminal    "foot -e"       // used to launch apps with Terminal=true (e.g. "kitty -e", "alacritty -e")
open-with   "pkill swaybg; swaybg -m fill -i"
                            // --files mode: command run on Enter, path appended.
                            // swaybg replaces the wallpaper safely (the old one is
                            // killed first). Alternatives: "awww img" (formerly
                            // swww), "xdg-open", "imv"
files-dir   ""              // default folder for --files with no argument (~/ expands)
                            // e.g. "~/Pictures/wallpapers" — once set, Tab inside
                            // foto toggles between apps and images
layout      "list"          // --files mode: "list" (rows) or "grid" (side by side)
grid-size   96              // --files grid: thumbnail cell size in pixels
recursive   #false          // --files: also scan subdirectories (depth-limited
                            // to 8, dot-directories skipped); --recursive
                            // enables it for a single run
layer       #true           // KDL booleans take a '#'. #true = floating overlay
                            // like fuzzel (layer-shell, niri + mango); #false =
                            // normal tileable window
position    "center"        // where the overlay sits: "center" (middle of the
                            // screen — "middle" works too), "top" (dmenu style)
                            // or "bottom". Only used when layer is #true
margin-y    0               // gap from the top/bottom edge for position top/
                            // bottom (e.g. 40 to sit below a waybar)
border-radius 12           // corner rounding in pixels (anti-aliased, the
                            // area outside is transparent); 0 = square
match-mode  "scored"        // "scored" (fzf-like ranking) or "substring"
theme       "gruvbox"       // one-word color scheme: gruvbox (the default),
                            // tokyo-night, catppuccin, nord, dracula or
                            // everforest. A colors block AFTER this line
                            // still overrides individual colors
history     #true           // remember what you launch most (kept in
                            // ~/.cache/foto/history) and rank those entries
                            // first; #false turns recording + ranking off
width       640             // requested window size (compositors may override)
height      440

colors {
    background  "#1d2021f2"  // "RRGGBB" or "RRGGBBAA" (last 2 digits = transparency)
    foreground  "#ebdbb2"
    selected    "#504945"
    border      "#83a598"
}
"##;

// helpers used only by tests (avoid touching the filesystem)
#[cfg(test)]
impl Config {
    fn from_file_contents(text: &str) -> Self {
        Self::try_from_contents(text).unwrap_or_else(|e| panic!("config should parse: {e:#?}"))
    }

    fn try_from_contents(text: &str) -> Result<Self> {
        let mut cfg = Self::default();
        cfg.apply_kdl(text)?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty() {
        let cfg = Config::from_file_contents("");
        assert_eq!(cfg.font, "monospace");
        assert_eq!(cfg.match_mode, MatchMode::Scored);
    }

    #[test]
    fn parses_example() {
        let cfg = Config::from_file_contents(Config::example_kdl());
        assert_eq!(cfg.font_size, 15.0);
        assert_eq!(cfg.icon_size, 24);
        assert_eq!(cfg.terminal, "foot -e");
        assert_eq!(cfg.colors.border, [0x83, 0xa5, 0x98, 0xff]);
        assert_eq!(cfg.colors.background[3], 0xf2); // alpha survived
    }

    #[test]
    fn overrides() {
        let cfg = Config::from_file_contents(
            r##"
            font "JetBrains Mono"
            font-size 12
            match-mode "substring"
            colors {
                background "#000000"
            }
        "##,
        );
        assert_eq!(cfg.font, "JetBrains Mono");
        assert_eq!(cfg.font_size, 12.0);
        assert_eq!(cfg.match_mode, MatchMode::Substring);
        assert_eq!(cfg.colors.background, [0, 0, 0, 0xff]);
        assert_eq!(cfg.colors.foreground, Colors::default().foreground);
    }

    #[test]
    fn open_with_override() {
        let cfg = Config::from_file_contents("open-with \"swww img\"");
        assert_eq!(cfg.open_with, "swww img");
        // swaybg is the default wallpaper setter (user's main one)
        assert_eq!(
            Config::default().open_with,
            "pkill swaybg; swaybg -m fill -i"
        );
        // the shipped example must also parse
        let ex = Config::from_file_contents(Config::example_kdl());
        assert_eq!(ex.open_with, "pkill swaybg; swaybg -m fill -i");
    }

    #[test]
    fn layer_key() {
        assert!(Config::default().layer);
        let off = Config::from_file_contents("layer #false");
        assert!(!off.layer);
        assert!(Config::try_from_contents("layer yes").is_err());
    }

    #[test]
    fn position_and_margin() {
        // default: middle of the screen
        assert_eq!(Config::default().position, Position::Center);
        assert_eq!(Config::default().margin_y, 0);
        // "middle" is an alias for "center"
        assert_eq!(
            Config::from_file_contents("position \"middle\"").position,
            Position::Center
        );
        assert_eq!(
            Config::from_file_contents("position \"top\"").position,
            Position::Top
        );
        assert_eq!(
            Config::from_file_contents("position \"bottom\"").position,
            Position::Bottom
        );
        let m = Config::from_file_contents("position \"top\"\nmargin-y 40");
        assert_eq!(m.position, Position::Top);
        assert_eq!(m.margin_y, 40);
        assert!(Config::try_from_contents("position \"left\"").is_err());
        // the shipped example keeps matching the defaults
        let ex = Config::from_file_contents(Config::example_kdl());
        assert_eq!(ex.position, Position::Center);
    }

    #[test]
    fn border_radius_key() {
        assert_eq!(Config::default().border_radius, 12);
        assert_eq!(
            Config::from_file_contents("border-radius 24").border_radius,
            24
        );
        assert_eq!(
            Config::from_file_contents("border-radius 0").border_radius,
            0
        );
        // negatives clamp to 0 instead of erroring
        assert_eq!(
            Config::from_file_contents("border-radius -5").border_radius,
            0
        );
        let ex = Config::from_file_contents(Config::example_kdl());
        assert_eq!(ex.border_radius, 12);
    }

    #[test]
    fn files_layout_grid_keys() {
        let cfg = Config::from_file_contents(
            r##"
            files-dir "~/Pictures/wallpapers"
            layout "grid"
            grid-size 128
        "##,
        );
        assert_eq!(cfg.files_dir.as_deref(), Some("~/Pictures/wallpapers"));
        assert_eq!(cfg.layout, Layout::Grid);
        assert_eq!(cfg.grid_size, 128);
        // tilde expansion
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~/x/y.png"), home.join("x/y.png"));
        assert_eq!(expand_tilde("/abs/path"), PathBuf::from("/abs/path"));
        // grid-size floor
        let tiny = Config::from_file_contents("grid-size 5");
        assert_eq!(tiny.grid_size, 32);
    }

    #[test]
    fn unknown_key_is_error() {
        assert!(Config::try_from_contents("nope 1").is_err());
        assert!(Config::try_from_contents("match-mode \"bogus\"").is_err());
        assert!(Config::try_from_contents("colors { nope #ffffff }").is_err());
    }

    #[test]
    fn color_formats() {
        assert_eq!(parse_color("#ff8000").unwrap(), [0xff, 0x80, 0x00, 0xff]);
        assert_eq!(parse_color("ff8000cc").unwrap(), [0xff, 0x80, 0x00, 0xcc]);
        assert!(parse_color("#ff8").is_err());
    }

    #[test]
    fn theme_presets() {
        // the default IS gruvbox: naming it changes nothing
        let def = Config::default().colors;
        assert_eq!(Config::from_file_contents("theme \"gruvbox\"").colors, def);
        // a preset replaces all four colors...
        let tn = Config::from_file_contents("theme \"tokyo-night\"").colors;
        assert_eq!(tn.background, [0x1a, 0x1b, 0x26, 0xe6]);
        assert_eq!(tn.border, [0x7a, 0xa2, 0xf7, 0xff]);
        assert_ne!(tn, def);
        // ...but a later colors block still wins
        let mix =
            Config::from_file_contents("theme \"tokyo-night\"\ncolors { border \"#ffffff\" }");
        assert_eq!(mix.colors.border, [0xff, 0xff, 0xff, 0xff]);
        assert_eq!(mix.colors.background, tn.background);
        // unknown names are an error listing the alternatives
        assert!(Config::try_from_contents("theme \"solarized\"").is_err());
        assert!(Config::try_from_contents("theme \"tokyo night\"").is_err());
        // every preset parses and differs from the others in the border
        let mut borders: Vec<_> = [
            "gruvbox",
            "tokyo-night",
            "catppuccin",
            "nord",
            "dracula",
            "everforest",
        ]
        .iter()
        .map(|t| {
            Config::from_file_contents(&format!("theme \"{t}\""))
                .colors
                .border
        })
        .collect();
        borders.dedup();
        assert_eq!(borders.len(), 6, "every preset has its own border color");
    }

    #[test]
    fn malformed_kdl_is_error() {
        // unterminated string breaks the KDL parser itself
        assert!(Config::try_from_contents("font \"unterminated").is_err());
        // pure garbage is a parse error, not a panic
        assert!(Config::try_from_contents("}}} not kdl {{{").is_err());
        // a key without its value is our error (missing value)
        assert!(Config::try_from_contents("font").is_err());
        // wrong value type for a known key
        assert!(Config::try_from_contents("font-size \"big\"").is_err());
    }

    #[test]
    fn missing_config_file_yields_defaults() {
        // point XDG_CONFIG_HOME at an empty temp dir: no foto/config.kdl
        // exists there, so Config::load() must return the defaults instead
        // of erroring.
        let dir = std::env::temp_dir().join(format!("foto-noconfig-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = std::env::var("XDG_CONFIG_HOME").ok();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let cfg = Config::load();
        // restore the environment before asserting, so a failed assert
        // cannot poison unrelated tests running later in the same process
        match old {
            Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
            None => std::env::remove_var("XDG_CONFIG_HOME"),
        }
        let cfg = cfg.expect("a missing config file must yield defaults");
        assert_eq!(cfg.font, "monospace");
        assert_eq!(cfg.border_radius, 12);
        assert_eq!(cfg.position, Position::Center);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn history_key() {
        // on by default — existing configs without the key keep ranking
        assert!(Config::default().history);
        assert!(Config::from_file_contents(Config::example_kdl()).history);
        assert!(!Config::from_file_contents("history #false").history);
        assert!(Config::from_file_contents("history #true").history);
        assert!(Config::try_from_contents("history yes").is_err());
    }

    #[test]
    fn recursive_key() {
        // default off — existing configs without the key stay valid
        assert!(!Config::default().recursive);
        assert!(
            !Config::from_file_contents(Config::example_kdl()).recursive,
            "the shipped example keeps the default"
        );
        assert!(Config::from_file_contents("recursive #true").recursive);
        assert!(!Config::from_file_contents("recursive #false").recursive);
        assert!(Config::try_from_contents("recursive yes").is_err());
    }
}
