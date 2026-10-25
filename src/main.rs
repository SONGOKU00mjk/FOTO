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
mod history;
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

use state::App;
use std::path::PathBuf;

// The CLI is parsed by hand: foto takes a handful of long flags and no
// positional arguments, so pulling in a full argument-parsing crate would
// be the largest single chunk of the binary for zero added capability.
// The behavior matches what users expect (see `tests` at the bottom):
//   * --help/-h and --version/-V print to stdout and exit 0
//   * unknown options print to stderr and exit 2
//   * --files takes an optional value (--files, --files DIR, --files=DIR)

/// Parsed command line. Mirrors the flags in [`Cli::HELP`].
#[derive(Debug)]
struct Cli {
    files: Option<String>,
    recursive: bool,
    dmenu: bool,
    no_layer: bool,
    tui: bool,
    list: bool,
    print_config: bool,
}

/// What went wrong (or right) while parsing: Help/Version are "errors"
/// only so `main` can print and exit without running anything.
#[derive(Debug)]
enum ParseOutcome {
    Parsed(Cli),
    Help,
    Version,
    /// Unknown or malformed option; the string is the offending argument.
    Usage(String),
}

impl Cli {
    const HELP: &str = concat!(
        "foto ",
        env!("CARGO_PKG_VERSION"),
        "\n",
        "A minimal Wayland-native app launcher with image preview\n\n",
        "Usage: foto [OPTIONS]\n\n",
        "Options:\n",
        "      --files [<DIR>]  Browse image files instead of apps; optional \
         directory (default ~/Pictures)\n",
        "      --recursive      With --files: also scan subdirectories \
         (depth-limited; overrides the `recursive #false` config default \
         for this run)\n",
        "      --dmenu          dmenu mode: read menu items from stdin, print \
         the chosen line to stdout (e.g. cliphist list | foto --dmenu | \
         cliphist decode | wl-copy)\n",
        "      --no-layer       Open a normal tileable window even if the \
         compositor supports layer-shell (the floating fuzzel-style overlay \
         is the default)\n",
        "      --tui            Force the terminal UI even if Wayland is available\n",
        "      --list           Print detected entries and exit (debugging helper)\n",
        "      --print-config   Print an annotated default config and exit\n",
        "  -h, --help           Print help\n",
        "  -V, --version        Print version\n",
    );

    fn parse_from(args: &[String]) -> ParseOutcome {
        use ParseOutcome::*;
        let mut cli = Cli {
            files: None,
            recursive: false,
            dmenu: false,
            no_layer: false,
            tui: false,
            list: false,
            print_config: false,
        };
        let mut rest = args.iter().peekable();
        while let Some(arg) = rest.next() {
            match arg.as_str() {
                "-h" | "--help" => return Help,
                "-V" | "--version" => return Version,
                "--files" => {
                    // optional value: consume the next argument only if it
                    // does not look like another flag (matches clap's
                    // num_args = 0..=1 behavior); the `matches!` form ends
                    // the peek borrow before we consume below
                    if matches!(rest.peek(), Some(v) if !v.starts_with('-')) {
                        cli.files = Some(rest.next().unwrap().clone());
                    } else {
                        cli.files = Some(String::new());
                    }
                }
                a if a.starts_with("--files=") => {
                    cli.files = Some(a["--files=".len()..].to_string());
                }
                "--recursive" => cli.recursive = true,
                "--dmenu" => cli.dmenu = true,
                "--no-layer" => cli.no_layer = true,
                "--tui" => cli.tui = true,
                "--list" => cli.list = true,
                "--print-config" => cli.print_config = true,
                other => return Usage(other.to_string()),
            }
        }
        Parsed(cli)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cli = match Cli::parse_from(&args) {
        ParseOutcome::Parsed(cli) => cli,
        ParseOutcome::Help => {
            print!("{}", Cli::HELP);
            return;
        }
        ParseOutcome::Version => {
            println!("foto {}", env!("CARGO_PKG_VERSION"));
            return;
        }
        ParseOutcome::Usage(arg) => {
            eprintln!("foto: unrecognized option '{arg}'");
            eprintln!("try 'foto --help' for the option list");
            std::process::exit(2);
        }
    };
    if let Err(err) = run(cli) {
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

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn parse(args: &[&str]) -> Cli {
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        match Cli::parse_from(&owned) {
            ParseOutcome::Parsed(cli) => cli,
            other => panic!("expected a parse, got {other:?}"),
        }
    }

    #[test]
    fn bare_invocation_has_all_defaults() {
        let cli = parse(&[]);
        assert!(cli.files.is_none());
        assert!(!cli.recursive && !cli.dmenu && !cli.no_layer);
        assert!(!cli.tui && !cli.list && !cli.print_config);
    }

    #[test]
    fn files_bare_yields_empty_dir() {
        // `foto --files` alone means "default directory" (~/Pictures)
        assert_eq!(parse(&["--files"]).files, Some(String::new()));
    }

    #[test]
    fn files_takes_dir_as_next_arg() {
        assert_eq!(
            parse(&["--files", "/tmp/pics"]).files,
            Some("/tmp/pics".into())
        );
        // --files followed by another flag must NOT swallow the flag
        let cli = parse(&["--files", "--recursive"]);
        assert_eq!(cli.files, Some(String::new()));
        assert!(cli.recursive);
    }

    #[test]
    fn files_equals_form() {
        assert_eq!(
            parse(&["--files=/tmp/pics"]).files,
            Some("/tmp/pics".into())
        );
        // `--files=` explicitly means "no directory given"
        assert_eq!(parse(&["--files="]).files, Some(String::new()));
    }

    #[test]
    fn every_flag_parses_and_stacks() {
        let cli = parse(&["--dmenu", "--no-layer", "--tui", "--list", "--print-config"]);
        assert!(cli.dmenu && cli.no_layer && cli.tui && cli.list && cli.print_config);
    }

    #[test]
    fn short_help_and_version() {
        let owned: Vec<String> = vec!["-h".into()];
        assert!(matches!(Cli::parse_from(&owned), ParseOutcome::Help));
        let owned: Vec<String> = vec!["-V".into()];
        assert!(matches!(Cli::parse_from(&owned), ParseOutcome::Version));
        let owned: Vec<String> = vec!["--version".into()];
        assert!(matches!(Cli::parse_from(&owned), ParseOutcome::Version));
    }

    #[test]
    fn unknown_option_is_a_usage_error() {
        let owned: Vec<String> = vec!["--config".into()];
        match Cli::parse_from(&owned) {
            ParseOutcome::Usage(arg) => assert_eq!(arg, "--config"),
            other => panic!("expected a usage error, got {other:?}"),
        }
        // a bare word (no dash) is not a positional — foto takes none
        let owned: Vec<String> = vec!["random.png".into()];
        assert!(matches!(Cli::parse_from(&owned), ParseOutcome::Usage(_)));
    }

    #[test]
    fn help_text_lists_every_flag() {
        for flag in [
            "--files",
            "--recursive",
            "--dmenu",
            "--no-layer",
            "--tui",
            "--list",
            "--print-config",
            "--help",
            "--version",
        ] {
            assert!(Cli::HELP.contains(flag), "help is missing {flag}");
        }
    }
}
