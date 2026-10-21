//! foto — a minimal Wayland app launcher with image preview.
//!
//! Frontends (chosen in `run` below):
//!   * Wayland GUI (default) — raw Wayland + tiny-skia, no toolkits
//!   * TUI (ratatui)         — automatic fallback when Wayland is missing, or `--tui`

mod config;
mod desktop;
mod files;
#[cfg(feature = "gui")]
mod gui;
mod icons;
mod matcher;
mod state;
mod thumbs;
#[cfg(feature = "tui")]
mod tui;

// A core-only build (no frontends) is a deliberate compile error: foto is
// an interactive tool and without gui/tui there is nothing to run.
#[cfg(not(any(feature = "gui", feature = "tui")))]
compile_error!(
    "foto needs at least one frontend. Build with --features gui, tui (both are on by default); a core-only build has no interactive frontend."
);

use clap::Parser;
use state::App;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "foto",
    version,
    about = "A minimal Wayland-native app launcher with image preview"
)]
struct Cli {
    /// Browse image files instead of apps; optional directory (default ~/Pictures)
    #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = "")]
    files: Option<String>,

    /// With --files: also scan subdirectories (depth-limited; overrides
    /// the `recursive #false` config default for this run)
    #[arg(long)]
    recursive: bool,

    /// dmenu mode: read menu items from stdin, print the chosen line to stdout
    /// (e.g. cliphist list | foto --dmenu | cliphist decode | wl-copy)
    #[arg(long)]
    dmenu: bool,

    /// Open a normal tileable window even if the compositor supports
    /// layer-shell (the floating fuzzel-style overlay is the default)
    #[arg(long)]
    no_layer: bool,

    /// Force the terminal UI even if Wayland is available
    #[arg(long)]
    tui: bool,

    /// Print detected entries and exit (debugging helper)
    #[arg(long)]
    list: bool,

    /// Print an annotated default config and exit
    #[arg(long)]
    print_config: bool,
}

fn main() {
    if let Err(err) = run(Cli::parse()) {
        eprintln!("foto: {err:#}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> anyhow::Result<()> {
    if cli.print_config {
        print!("{}", config::Config::example_kdl());
        return Ok(());
    }

    let cfg = config::Config::load()?;

    let mut app = if cli.dmenu {
        let lines = read_dmenu_lines()?;
        App::new_dmenu(cfg, lines)?
    } else {
        match cli.files {
            Some(dir) => {
                let dir = if dir.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(dir))
                };
                let recursive = cli.recursive || cfg.recursive;
                App::new_files(cfg, dir, recursive)?
            }
            None => App::new_apps(cfg)?,
        }
    };

    if cli.list {
        for m in &app.matches {
            let entry = &app.entries[m.index];
            let subtitle = if entry.subtitle.is_empty() {
                String::new()
            } else {
                format!(" — {}", entry.subtitle)
            };
            println!("{}{subtitle}", entry.title);
        }
        return Ok(());
    }

    if cli.tui {
        return run_tui(&mut app);
    }

    // Default: try the Wayland GUI; if there is no session, use the TUI.
    #[cfg(feature = "gui")]
    {
        match wayland_client::Connection::connect_to_env() {
            Ok(conn) => return gui::run(conn, app, cli.no_layer),
            Err(e) => {
                eprintln!("foto: no Wayland session ({e}); falling back to the terminal UI")
            }
        }
    }

    run_tui(&mut app)
}

// ---------- frontend shims (buildable with either feature set) ----------

#[cfg(feature = "tui")]
fn run_tui(app: &mut App) -> anyhow::Result<()> {
    crate::tui::run(app)
}

#[cfg(not(feature = "tui"))]
fn run_tui(_: &mut App) -> anyhow::Result<()> {
    anyhow::bail!(
        "this build has no TUI (built without the tui feature); rebuild with --features tui"
    )
}

/// dmenu input: all of stdin, one menu item per line. Guard against being
/// run without a pipe (reading a TTY would block forever).
fn read_dmenu_lines() -> anyhow::Result<Vec<String>> {
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        anyhow::bail!("--dmenu reads items from stdin; try: cliphist list | foto --dmenu");
    }
    let mut buf = String::new();
    std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)?;
    Ok(buf.lines().map(|l| l.to_string()).collect())
}
