//! Shared launcher state, used identically by the TUI and the Wayland GUI.
//!
//! Both frontends:
//!   1. call [`App::on_key`] with their platform keys translated into [`Key`],
//!   2. draw `app.matches` however they like,
//!   3. act on the returned [`Flow`] (Continue / Launch / Exit).

use crate::config::{Config, Layout};
use crate::desktop::{self, DesktopEntry};
use crate::files::{self, ImageFile};
use crate::icons::{Icon, IconFinder};
use crate::matcher::{self, Scored};
// The icon_finder/engine fields and CellIcon are driven only by the GUI
// frontend; core (tui-only) builds never touch them.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
use crate::thumbs::ThumbEngine;
#[cfg(feature = "gui")]
use crate::thumbs::ThumbState;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::sync::Arc;

/// What one row of the launcher represents.
pub enum EntryKind {
    App(DesktopEntry),
    Image(ImageFile),
    /// a raw line from stdin in `--dmenu` mode; the title IS the line
    Line,
}

pub struct Entry {
    pub kind: EntryKind,
    pub title: String,
    pub subtitle: String,
    /// lowercased haystack the matcher runs against (name + generic + keywords)
    pub search: String,
}

/// One action to perform on Enter.
#[derive(Debug)]
pub enum Action {
    /// run a command line through `sh -c` (apps, and --files via `open-with`)
    Shell(String),
    /// print a line to stdout (dmenu mode: `… | foto --dmenu | cliphist decode`)
    Print(String),
}

#[derive(Debug)]
pub enum Flow {
    Continue,
    Launch(Action),
    Exit,
}

/// Frontend-neutral key — TUI and GUI each translate their events into this.
/// `Delete` has no effect today (no cursor editing yet) but both frontends map it
/// so adding cursor editing later touches only `state::on_key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum Key {
    Char(char),
    Backspace,
    Delete,
    Up,
    Down,
    Left,
    Right,
    Top,
    Bottom,
    WordDelete,
    ClearLine,
    Enter,
    Escape,
    /// toggle between the apps list and the images list (both available)
    Tab,
    Other,
}

#[cfg_attr(not(feature = "gui"), allow(dead_code))]
pub struct App {
    pub cfg: Config,
    pub entries: Vec<Entry>,
    /// the entries of the *other* mode (apps <-> images); Tab swaps them in
    pub alt_entries: Option<Vec<Entry>>,
    /// matched entry indices, best first
    pub matches: Vec<Scored>,
    pub query: String,
    /// byte offset of the caret inside `query` (always kept on a char
    /// boundary). Editing keys (Left/Right/Home/End/Delete/Backspace) act
    /// around it; with an empty query those keys keep their old
    /// list-navigation meaning.
    pub cursor: usize,
    /// index into `matches`
    pub selected: usize,
    pub icon_finder: IconFinder,
    /// true when the images list is active (drives grid layout + nav)
    pub files_mode: bool,
    /// true in `--dmenu` mode: Enter prints the selected line, Tab is off
    pub dmenu: bool,
    /// columns of the --files grid; the GUI updates it every frame so that
    /// Up/Down can jump a whole row. TUI leaves it at 1 (list behavior).
    pub grid_cols: usize,
    /// background thumbnail loader (GUI only; None in the TUI)
    pub engine: Option<Arc<ThumbEngine>>,
}

/// What the renderer should draw in the icon slot of a row/cell.
#[cfg_attr(not(feature = "gui"), allow(dead_code))]
#[derive(Clone)]
pub enum CellIcon {
    Ready(Option<Icon>),
    /// image thumbnails may still be decoding on a worker thread
    Pending,
}

impl App {
    // ---------- constructors ----------

    pub fn new_apps(cfg: Config) -> Result<Self> {
        let apps = desktop::scan()?;
        anyhow::ensure!(!apps.is_empty(), "no applications found in XDG data dirs");

        let entries = entries_from_apps(apps);
        // Tab target: the images list, if a files dir exists and has images
        let alt_entries = files_dir_entries(&cfg);

        let icon_finder = IconFinder::new(&cfg.icon_theme, cfg.icon_size);
        let mut app = Self {
            cfg,
            entries,
            alt_entries,
            matches: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            icon_finder,
            files_mode: false,
            dmenu: false,
            grid_cols: 1,
            engine: None,
        };
        app.refresh();
        Ok(app)
    }

    pub fn new_files(cfg: Config, dir: Option<PathBuf>, recursive: bool) -> Result<Self> {
        let dir = dir
            .or_else(|| {
                cfg.files_dir
                    .as_ref()
                    .map(|s| crate::config::expand_tilde(s))
            })
            .or_else(files::default_dir)
            .ok_or_else(|| anyhow!("--files: no directory given and ~/Pictures does not exist"))?;

        // Stream the walk: each discovered directory's batch is appended as
        // soon as it is found instead of collecting one big Vec first, so
        // state builds incrementally while the tree is being walked.
        let mut entries: Vec<Entry> = Vec::new();
        files::scan_streaming(&dir, recursive, &mut |batch| {
            entries.extend(entries_from_images(batch.to_vec()));
        });
        anyhow::ensure!(
            !entries.is_empty(),
            "no image files found in {}",
            dir.display()
        );
        // Tab target: the apps list, if any apps are installed
        let alt_entries = match desktop::scan() {
            Ok(apps) if !apps.is_empty() => Some(entries_from_apps(apps)),
            _ => None,
        };

        let icon_finder = IconFinder::new(&cfg.icon_theme, cfg.icon_size);
        let mut app = Self {
            cfg,
            entries,
            alt_entries,
            matches: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            icon_finder,
            files_mode: true,
            dmenu: false,
            grid_cols: 1,
            engine: None,
        };
        app.refresh();
        Ok(app)
    }

    /// `--dmenu`: the menu items come from stdin; Enter prints the chosen
    /// line to stdout (dmenu protocol). Works with cliphist:
    /// `cliphist list | foto --dmenu | cliphist decode | wl-copy`
    pub fn new_dmenu(cfg: Config, lines: Vec<String>) -> Result<Self> {
        anyhow::ensure!(!lines.is_empty(), "--dmenu: no items on stdin");
        let entries = lines
            .into_iter()
            .map(|line| Entry {
                search: line.to_lowercase(),
                title: line,
                subtitle: String::new(),
                kind: EntryKind::Line,
            })
            .collect();
        let icon_finder = IconFinder::new(&cfg.icon_theme, cfg.icon_size);
        let mut app = Self {
            cfg,
            entries,
            alt_entries: None,
            matches: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            icon_finder,
            files_mode: false,
            dmenu: true,
            grid_cols: 1,
            engine: None,
        };
        app.refresh();
        Ok(app)
    }

    // ---------- state transitions ----------

    /// Recompute `matches` from the current query.
    pub fn refresh(&mut self) {
        let texts: Vec<String> = self.entries.iter().map(|e| e.search.clone()).collect();
        self.matches = matcher::filter(&texts, &self.query, self.cfg.match_mode);
        if self.selected >= self.matches.len() {
            self.selected = self.matches.len().saturating_sub(1);
        }
    }

    pub fn move_selection(&mut self, delta: i32) {
        let len = self.matches.len() as i32;
        if len == 0 {
            return;
        }
        let next = (self.selected as i32 + delta).clamp(0, len - 1);
        self.selected = next as usize;
    }

    /// Swap the apps list and the images list (Tab). Only possible when the
    /// other mode has entries (e.g. files-dir configured, apps installed).
    pub fn switch_mode(&mut self) {
        if self.dmenu {
            return;
        }
        let Some(mut alt) = self.alt_entries.take() else {
            return;
        };
        std::mem::swap(&mut self.entries, &mut alt);
        self.alt_entries = Some(alt);
        self.files_mode = !self.files_mode;
        self.grid_cols = 1; // re-computed by the grid renderer every frame
        self.query.clear();
        self.cursor = 0;
        self.selected = 0;
        self.refresh();
    }

    /// Split the query at the caret for rendering. The offset is clamped
    /// down to the nearest char boundary, so a stale cursor can never panic.
    pub fn caret_split(&self) -> (&str, &str) {
        let mut at = self.cursor.min(self.query.len());
        while at > 0 && !self.query.is_char_boundary(at) {
            at -= 1;
        }
        self.query.split_at(at)
    }

    /// Current mode name for the prompt badge / TUI title.
    pub fn mode_label(&self) -> &'static str {
        if self.dmenu {
            "menu"
        } else if self.files_mode {
            "images"
        } else {
            "apps"
        }
    }

    /// What Tab switches to, if anything.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))] // rendered in footers
    pub fn alt_label(&self) -> Option<&'static str> {
        if self.dmenu || self.alt_entries.is_none() {
            None
        } else if self.files_mode {
            Some("apps")
        } else {
            Some("images")
        }
    }

    /// How many entries one Up/Down press skips: 1 in list layouts, one full
    /// row of thumbnails in --files grid mode.
    fn nav_step(&self) -> i32 {
        if self.files_mode && self.cfg.layout == Layout::Grid {
            self.grid_cols.max(1) as i32
        } else {
            1
        }
    }

    /// Feed one key event; returns what the frontend should do next.
    ///
    /// Arrow-key conflict resolution: **Up/Down always move the selection**
    /// (Ctrl-J/K are hard-wired alternates in both frontends). **Left/Right
    /// and Home/End move the caret while the query is non-empty** — there is
    /// nothing to edit otherwise — and keep their old meaning (grid-cell
    /// steps / first-and-last entry) when the query is empty. In --files
    /// grid mode with text typed, row navigation stays on Up/Down.
    pub fn on_key(&mut self, key: Key) -> Flow {
        match key {
            Key::Escape => return Flow::Exit,
            Key::Enter => return self.activate(),
            Key::Up => self.move_selection(-self.nav_step()),
            Key::Down => self.move_selection(self.nav_step()),
            // Left/Right: caret movement while typing, list/grid navigation
            // otherwise (see the doc comment above)
            Key::Left => {
                if self.query.is_empty() {
                    self.move_selection(-1);
                } else if let Some(c) = self.query[..self.cursor.min(self.query.len())]
                    .chars()
                    .next_back()
                {
                    self.cursor -= c.len_utf8();
                }
            }
            Key::Right => {
                if self.query.is_empty() {
                    self.move_selection(1);
                } else if let Some(c) = self.query[self.cursor.min(self.query.len())..]
                    .chars()
                    .next()
                {
                    self.cursor += c.len_utf8();
                }
            }
            Key::Tab => self.switch_mode(),
            // Home/End: caret to the ends while typing, first/last entry
            // otherwise
            Key::Top => {
                if self.query.is_empty() {
                    self.selected = 0;
                } else {
                    self.cursor = 0;
                }
            }
            Key::Bottom => {
                if self.query.is_empty() {
                    self.selected = self.matches.len().saturating_sub(1);
                } else {
                    self.cursor = self.query.len();
                }
            }
            Key::Backspace => {
                // delete the char BEFORE the caret (UTF-8 safe, one char at
                // a time); at the end of the query this is the old behavior
                let at = self.cursor.min(self.query.len());
                if let Some(c) = self.query[..at].chars().next_back() {
                    let start = at - c.len_utf8();
                    self.query.replace_range(start..at, "");
                    self.cursor = start;
                    self.refresh();
                }
            }
            Key::Delete => {
                // delete the char AT the caret; no-op at the end
                let at = self.cursor.min(self.query.len());
                if let Some(c) = self.query[at..].chars().next() {
                    let end = at + c.len_utf8();
                    self.query.replace_range(at..end, "");
                    self.refresh();
                }
            }
            Key::Other => {}
            Key::Char(c) => {
                let at = self.cursor.min(self.query.len());
                self.query.insert(at, c);
                self.cursor = at + c.len_utf8();
                self.refresh();
            }
            Key::WordDelete => {
                // readline C-w: kill the word before the caret
                let at = self.cursor.min(self.query.len());
                let trimmed = self.query[..at].trim_end();
                let cut = trimmed
                    .rfind(char::is_whitespace)
                    .map(|i| i + 1)
                    .unwrap_or(0);
                self.query.replace_range(cut..at, "");
                self.cursor = cut;
                self.refresh();
            }
            Key::ClearLine => {
                self.query.clear();
                self.cursor = 0;
                self.refresh();
            }
        }
        Flow::Continue
    }

    fn activate(&mut self) -> Flow {
        let Some(m) = self.matches.get(self.selected) else {
            return Flow::Exit; // nothing selected — just close
        };
        match &self.entries[m.index].kind {
            EntryKind::App(app) => {
                let exec = desktop::sanitize_exec(&app.exec);
                let cmd = if app.terminal && !exec.is_empty() {
                    // terminal command + the app's command line, quoted safely
                    format!("{} {}", self.cfg.terminal, shell_quote(&exec))
                } else {
                    exec
                };
                if cmd.is_empty() {
                    return Flow::Continue;
                }
                Flow::Launch(Action::Shell(cmd))
            }
            EntryKind::Image(f) => {
                // user-configurable open command: sets the wallpaper with
                // swaybg by default. Shell-quoting keeps paths with spaces
                // safe under `sh -c`.
                let cmd = format!(
                    "{} {}",
                    self.cfg.open_with,
                    shell_quote(&f.path.to_string_lossy())
                );
                Flow::Launch(Action::Shell(cmd))
            }
            // dmenu: hand the selected line back to the caller's pipe
            EntryKind::Line => {
                let line = self.entries[m.index].title.clone();
                Flow::Launch(Action::Print(line))
            }
        }
    }

    // ---------- icons & thumbnails (GUI only) ----------

    /// What to draw in the icon slot of entry `index`.
    /// Apps resolve from the icon theme; images come from the background
    /// thumbnail engine (or synchronously if there is none, i.e. the TUI).
    #[cfg(feature = "gui")]
    pub fn cell_icon(&mut self, index: usize) -> CellIcon {
        match &self.entries[index].kind {
            EntryKind::App(app) => CellIcon::Ready(self.icon_finder.find(&app.icon)),
            EntryKind::Image(f) => {
                let size = self.thumb_size();
                match &self.engine {
                    Some(engine) => match engine.lookup(&f.path, size) {
                        ThumbState::Ready(icon) => CellIcon::Ready(icon),
                        ThumbState::Pending => CellIcon::Pending,
                    },
                    None => CellIcon::Ready(files::load_thumbnail(&f.path, size)),
                }
            }
            EntryKind::Line => CellIcon::Ready(None),
        }
    }

    /// Target thumbnail edge: grid cells want the big size, list rows the small one.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    fn thumb_size(&self) -> u32 {
        if self.files_mode && self.cfg.layout == Layout::Grid {
            self.cfg.grid_size
        } else {
            self.cfg.icon_size
        }
    }
}

// ---------- launching ----------

/// Quote a string so `sh -c` sees it as one word.
pub fn shell_quote(s: &str) -> String {
    if s.is_empty() {
        return "''".to_string();
    }
    format!("'{}'", s.replace('\'', r#"'\''"#))
}

/// Spawn the action detached; failures are reported but never crash the UI.
/// `Action::Print` is handled by the frontends themselves (it must reach
/// foto's own stdout before exiting, and the TUI has to leave the alternate
/// screen first).
pub fn run_action(action: &Action) -> Result<()> {
    use std::process::{Command, Stdio};
    match action {
        Action::Shell(cmd) => {
            Command::new("sh")
                .arg("-c")
                .arg(cmd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| anyhow!("spawn `{cmd}`: {e}"))?;
        }
        Action::Print(_) => {}
    }
    Ok(())
}

// ---------- entry building helpers ----------

fn entries_from_apps(apps: Vec<DesktopEntry>) -> Vec<Entry> {
    apps.into_iter()
        .map(|a| {
            let mut search = a.name.to_lowercase();
            if !a.generic_name.is_empty() {
                search.push(' ');
                search.push_str(&a.generic_name.to_lowercase());
            }
            if !a.comment.is_empty() {
                search.push(' ');
                search.push_str(&a.comment.to_lowercase());
            }
            for kw in &a.keywords {
                search.push(' ');
                search.push_str(&kw.to_lowercase());
            }
            let subtitle = a.generic_name.clone();
            Entry {
                title: a.name.clone(),
                subtitle,
                search,
                kind: EntryKind::App(a),
            }
        })
        .collect()
}

fn entries_from_images(images: Vec<ImageFile>) -> Vec<Entry> {
    images
        .into_iter()
        .map(|f| {
            let search = format!(
                "{} {}",
                f.name(),
                f.path.display().to_string().to_lowercase()
            );
            Entry {
                title: f.name().to_string(),
                subtitle: String::new(),
                search,
                kind: EntryKind::Image(f),
            }
        })
        .collect()
}

/// The images list for Tab switching: from the config's files-dir, or ~/Pictures.
fn files_dir_entries(cfg: &Config) -> Option<Vec<Entry>> {
    let dir = cfg
        .files_dir
        .as_ref()
        .map(|s| crate::config::expand_tilde(s))
        .or_else(files::default_dir)?;
    let images = files::scan(&dir);
    if images.is_empty() {
        None
    } else {
        Some(entries_from_images(images))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("firefox"), "'firefox'");
        assert_eq!(shell_quote("it's"), r#"'it'\''s'"#);
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn image_activation_uses_open_with() {
        let cfg = Config {
            open_with: "swww img".into(),
            ..Config::default()
        };
        let path = std::env::temp_dir().join("foto-test-wall with spaces.png");
        let mut app = App {
            cfg,
            entries: vec![Entry {
                title: "wall.png".into(),
                subtitle: String::new(),
                search: "wall.png".into(),
                kind: EntryKind::Image(ImageFile { path: path.clone() }),
            }],
            alt_entries: None,
            matches: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            icon_finder: IconFinder::new("hicolor", 16),
            files_mode: true,
            dmenu: false,
            grid_cols: 1,
            engine: None,
        };
        app.refresh();
        let flow = app.on_key(Key::Enter);
        match flow {
            Flow::Launch(Action::Shell(cmd)) => {
                assert_eq!(cmd, format!("swww img '{}'", path.display()))
            }
            other => panic!("expected Launch(Shell), got {other:?}"),
        }
    }

    #[test]
    fn grid_navigation_moves_by_row() {
        let cfg = Config {
            layout: Layout::Grid,
            ..Config::default()
        };
        let mut app = App {
            cfg,
            entries: Vec::new(),
            alt_entries: None,
            matches: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            icon_finder: IconFinder::new("hicolor", 16),
            files_mode: true,
            dmenu: false,
            grid_cols: 3,
            engine: None,
        };
        for i in 0..7 {
            app.entries.push(Entry {
                title: format!("f{i}.png"),
                subtitle: String::new(),
                search: format!("f{i}.png"),
                kind: EntryKind::Image(ImageFile {
                    path: PathBuf::from(format!("/tmp/f{i}.png")),
                }),
            });
        }
        app.refresh(); // empty query -> all 7 matched in order
        assert_eq!(app.matches.len(), 7);

        app.on_key(Key::Down); // one row = 3 columns -> index 3
        assert_eq!(app.selected, 3);
        app.on_key(Key::Down); // index 6
        assert_eq!(app.selected, 6);
        app.on_key(Key::Down); // clamped at the last cell
        assert_eq!(app.selected, 6);
        app.on_key(Key::Up); // back to index 3
        assert_eq!(app.selected, 3);
        // list mode sanity: with grid_cols=1 nav step is 1
        app.grid_cols = 1;
        app.on_key(Key::Up);
        assert_eq!(app.selected, 2);
    }

    #[test]
    fn left_right_step_one_cell() {
        let cfg = Config {
            layout: Layout::Grid,
            ..Config::default()
        };
        let mut app = App {
            cfg,
            entries: Vec::new(),
            alt_entries: None,
            matches: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            icon_finder: IconFinder::new("hicolor", 16),
            files_mode: true,
            dmenu: false,
            grid_cols: 4,
            engine: None,
        };
        for i in 0..7 {
            app.entries.push(Entry {
                title: format!("f{i}.png"),
                subtitle: String::new(),
                search: format!("f{i}.png"),
                kind: EntryKind::Image(ImageFile {
                    path: PathBuf::from(format!("/tmp/f{i}.png")),
                }),
            });
        }
        app.refresh();
        app.on_key(Key::Right);
        assert_eq!(app.selected, 1);
        app.on_key(Key::Left);
        assert_eq!(app.selected, 0);
        app.on_key(Key::Left); // clamped at 0
        assert_eq!(app.selected, 0);
        // Down moves a whole row (4), Left from there is one cell back
        app.on_key(Key::Down);
        assert_eq!(app.selected, 4);
        app.on_key(Key::Left);
        assert_eq!(app.selected, 3);
    }

    #[test]
    fn word_delete() {
        // Hand-built App: no filesystem access needed for query editing tests.
        let mut app = App {
            cfg: Config::default(),
            entries: Vec::new(),
            alt_entries: None,
            matches: Vec::new(),
            query: String::from("foo bar "),
            cursor: 8, // caret after "foo bar "
            selected: 0,
            icon_finder: IconFinder::new("hicolor", 16),
            files_mode: false,
            dmenu: false,
            grid_cols: 1,
            engine: None,
        };
        app.refresh();
        app.on_key(Key::WordDelete);
        assert_eq!(app.query, "foo ");
        app.on_key(Key::ClearLine);
        assert_eq!(app.query, "");
        app.on_key(Key::Char('a'));
        app.on_key(Key::Char('b'));
        app.on_key(Key::Backspace);
        assert_eq!(app.query, "a");
    }

    fn img_entry(name: &str) -> Entry {
        Entry {
            title: name.into(),
            subtitle: String::new(),
            search: name.to_lowercase(),
            kind: EntryKind::Image(ImageFile {
                path: PathBuf::from(format!("/tmp/{name}")),
            }),
        }
    }

    fn app_entry(name: &str) -> Entry {
        Entry {
            title: name.into(),
            subtitle: String::new(),
            search: name.to_lowercase(),
            kind: EntryKind::Line, // cheap stand-in; kind is irrelevant here
        }
    }

    /// Minimal App with no entries and no filesystem access; tests push
    /// their own entries. Keep the fields in sync with `App` itself.
    fn bare_app(cfg: Config) -> App {
        App {
            cfg,
            entries: Vec::new(),
            alt_entries: None,
            matches: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            icon_finder: IconFinder::new("hicolor", 16),
            files_mode: false,
            dmenu: false,
            grid_cols: 1,
            engine: None,
        }
    }

    fn desktop_entry(name: &str, exec: &str, terminal: bool) -> Entry {
        Entry {
            title: name.into(),
            subtitle: String::new(),
            search: name.to_lowercase(),
            kind: EntryKind::App(DesktopEntry {
                name: name.into(),
                generic_name: String::new(),
                comment: String::new(),
                icon: String::new(),
                exec: exec.into(),
                terminal,
                keywords: Vec::new(),
                path: PathBuf::from(format!("/tmp/{name}.desktop")),
            }),
        }
    }

    #[test]
    fn terminal_app_launches_in_configured_terminal() {
        let mut app = bare_app(Config::default()); // terminal: "foot -e"
        app.entries
            .push(desktop_entry("HTop", "htop", /* terminal */ true));
        app.entries
            .push(desktop_entry("Editor", "gedit", /* terminal */ false));
        app.refresh();

        app.on_key(Key::Char('h')); // only HTop matches
        match app.on_key(Key::Enter) {
            Flow::Launch(Action::Shell(cmd)) => {
                assert_eq!(cmd, "foot -e 'htop'", "Terminal=true must be wrapped")
            }
            other => panic!("expected Launch(Shell), got {other:?}"),
        }

        // the GUI app stays un-wrapped
        let mut app = bare_app(Config::default());
        app.entries.push(desktop_entry("Editor", "gedit", false));
        app.refresh();
        match app.on_key(Key::Enter) {
            Flow::Launch(Action::Shell(cmd)) => assert_eq!(cmd, "gedit"),
            other => panic!("expected Launch(Shell), got {other:?}"),
        }
    }

    #[test]
    fn selection_clamps_at_both_boundaries() {
        let mut app = bare_app(Config::default());
        for n in ["alpha", "beta", "gamma"] {
            app.entries.push(app_entry(n));
        }
        app.refresh();
        assert_eq!(app.matches.len(), 3);

        // walk past the bottom edge — must clamp at 2
        app.on_key(Key::Down);
        app.on_key(Key::Down);
        app.on_key(Key::Down);
        assert_eq!(app.selected, 2);

        // walk past the top edge — must clamp at 0
        app.on_key(Key::Up);
        app.on_key(Key::Up);
        app.on_key(Key::Up);
        assert_eq!(app.selected, 0);

        // Home/End helpers
        app.on_key(Key::Bottom);
        assert_eq!(app.selected, 2);
        app.on_key(Key::Top);
        assert_eq!(app.selected, 0);

        // narrowing the query clamps a too-large selection instead of
        // keeping an out-of-bounds index
        app.on_key(Key::Bottom);
        app.query = "al".into(); // only "alpha" remains
        app.refresh();
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn query_editing_is_unicode_safe() {
        let mut app = bare_app(Config::default());
        // 'é' is 2 bytes in UTF-8 but ONE char: Backspace must pop it whole
        app.on_key(Key::Char('é'));
        app.on_key(Key::Char('x'));
        assert_eq!(app.query, "éx");
        app.on_key(Key::Backspace);
        assert_eq!(app.query, "é", "backspace removes one char, not one byte");
        app.on_key(Key::Backspace);
        assert_eq!(app.query, "");
        // backspacing an empty query is a no-op, not a panic
        app.on_key(Key::Backspace);
        assert_eq!(app.query, "");
    }

    #[test]
    fn flow_contract_for_special_keys() {
        let mut app = bare_app(Config::default());
        app.entries.push(app_entry("alpha"));

        // Escape always exits
        assert!(matches!(app.on_key(Key::Escape), Flow::Exit));
        // Enter with nothing matched exits quietly — nothing to launch
        assert!(matches!(app.on_key(Key::Enter), Flow::Exit));
        // Delete is accepted but currently a no-op (no cursor editing yet)
        assert!(matches!(app.on_key(Key::Delete), Flow::Continue));
        assert!(matches!(app.on_key(Key::Other), Flow::Continue));
        // typing still works and refreshes matches
        assert!(matches!(app.on_key(Key::Char('a')), Flow::Continue));
        assert_eq!(app.query, "a");
        assert_eq!(app.matches.len(), 1);
    }

    #[test]
    fn tab_swaps_apps_and_images() {
        let mut app = App {
            cfg: Config::default(),
            entries: vec![app_entry("Firefox"), app_entry("Foot")],
            alt_entries: Some(vec![img_entry("a.png"), img_entry("b.png")]),
            matches: Vec::new(),
            query: String::from("fir"), // switching clears the query
            cursor: 3,
            selected: 1,
            icon_finder: IconFinder::new("hicolor", 16),
            files_mode: false,
            dmenu: false,
            grid_cols: 1,
            engine: None,
        };
        app.refresh();
        assert_eq!(app.mode_label(), "apps");
        assert_eq!(app.alt_label(), Some("images"));

        app.on_key(Key::Tab);
        assert!(app.files_mode);
        assert_eq!(app.mode_label(), "images");
        assert_eq!(app.alt_label(), Some("apps"));
        assert_eq!(app.entries.len(), 2);
        assert_eq!(app.entries[0].title, "a.png");
        assert_eq!(app.alt_entries.as_ref().unwrap()[0].title, "Firefox");
        assert_eq!(app.query, ""); // query reset on switch
        assert_eq!(app.selected, 0);

        app.on_key(Key::Tab);
        assert!(!app.files_mode);
        assert_eq!(app.entries[0].title, "Firefox");
    }

    #[test]
    fn dmenu_enter_prints_line() {
        let mut app = App {
            cfg: Config::default(),
            entries: vec![
                Entry {
                    title: "1\timage/png\t223KB".into(),
                    subtitle: String::new(),
                    search: "1\timage/png\t223kb".into(),
                    kind: EntryKind::Line,
                },
                Entry {
                    title: "2\ttext/plain\t17B\thello".into(),
                    subtitle: String::new(),
                    search: "2\ttext/plain\t17b\thello".into(),
                    kind: EntryKind::Line,
                },
            ],
            alt_entries: None,
            matches: Vec::new(),
            query: String::new(),
            cursor: 0,
            selected: 0,
            icon_finder: IconFinder::new("hicolor", 16),
            files_mode: false,
            dmenu: true,
            grid_cols: 1,
            engine: None,
        };
        app.refresh();
        assert_eq!(app.mode_label(), "menu");
        assert_eq!(app.alt_label(), None);
        // Tab does nothing in dmenu mode
        app.on_key(Key::Tab);
        assert!(!app.files_mode);
        // filter + Enter prints the exact original line
        app.on_key(Key::Char('h'));
        app.on_key(Key::Char('e'));
        app.on_key(Key::Char('l'));
        assert_eq!(app.matches[0].index, 1);
        let flow = app.on_key(Key::Enter);
        match flow {
            Flow::Launch(Action::Print(line)) => {
                assert_eq!(line, "2\ttext/plain\t17B\thello")
            }
            other => panic!("expected Launch(Print), got {other:?}"),
        }
    }

    #[test]
    fn cursor_editing_insert_move_delete() {
        let mut app = bare_app(Config::default());
        for c in "abc".chars() {
            app.on_key(Key::Char(c));
        }
        assert_eq!(app.query, "abc");
        assert_eq!(app.cursor, 3);

        // Home (Ctrl-A) jumps to the start; typing inserts before 'a'
        app.on_key(Key::Top);
        assert_eq!(app.cursor, 0);
        app.on_key(Key::Char('X'));
        assert_eq!(app.query, "Xabc");
        assert_eq!(app.cursor, 1);

        // caret movement clamps at both ends
        app.on_key(Key::Left);
        assert_eq!(app.cursor, 0);
        app.on_key(Key::Left);
        assert_eq!(app.cursor, 0);
        for _ in 0..5 {
            app.on_key(Key::Right);
        }
        assert_eq!(app.cursor, 4, "clamped at the end of the query");
        app.on_key(Key::Bottom);
        assert_eq!(app.cursor, 4);

        // Delete removes the char AT the caret; at the end it is a no-op
        app.on_key(Key::Delete);
        assert_eq!(app.query, "Xabc");
        app.on_key(Key::Left);
        app.on_key(Key::Delete); // kills 'c'
        assert_eq!(app.query, "Xab");
        assert_eq!(app.cursor, 3);
    }

    #[test]
    fn backspace_deletes_before_caret() {
        let mut app = bare_app(Config::default());
        for c in "hello".chars() {
            app.on_key(Key::Char(c));
        }
        app.on_key(Key::Top); // caret at 0
        app.on_key(Key::Right); // caret before 'e'
        app.on_key(Key::Backspace); // kills 'h'
        assert_eq!(app.query, "ello");
        assert_eq!(app.cursor, 0);
        // backspacing at the start is a no-op
        app.on_key(Key::Backspace);
        assert_eq!(app.query, "ello");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn word_delete_kills_word_before_caret() {
        let mut app = bare_app(Config::default());
        for c in "foo bar baz".chars() {
            app.on_key(Key::Char(c));
        }
        assert_eq!(app.cursor, 11);
        app.on_key(Key::WordDelete); // kills "baz" (caret at the end)
        assert_eq!(app.query, "foo bar ");
        assert_eq!(app.cursor, 8);
        app.on_key(Key::WordDelete); // kills "bar "
        assert_eq!(app.query, "foo ");
        assert_eq!(app.cursor, 4);
        // Ctrl-E to the end again, Ctrl-A home, ClearLine resets everything
        app.on_key(Key::Bottom);
        app.on_key(Key::Top);
        assert_eq!(app.cursor, 0);
        app.on_key(Key::ClearLine);
        assert_eq!(app.query, "");
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn caret_is_unicode_safe() {
        let mut app = bare_app(Config::default());
        app.on_key(Key::Char('é')); // 2 bytes
        app.on_key(Key::Char('x'));
        assert_eq!(app.cursor, 3);
        // Left walks whole chars, never into the middle of 'é'
        app.on_key(Key::Left); // before 'x'
        assert_eq!(app.cursor, 2);
        app.on_key(Key::Left); // before 'é'
        assert_eq!(app.cursor, 0);
        app.on_key(Key::Char('y'));
        assert_eq!(app.query, "yéx");
        assert_eq!(app.cursor, 1);
        // deleting before the caret removes the full 2-byte char
        app.on_key(Key::Backspace);
        assert_eq!(app.query, "éx");
        // caret_split is boundary-safe even if a stale offset appears
        app.cursor = 1; // mid-'é' byte
        let (before, after) = app.caret_split();
        assert_eq!((before, after), ("", "éx"));
    }

    #[test]
    fn empty_query_keeps_arrow_navigation() {
        let mut app = bare_app(Config::default());
        for n in ["alpha", "beta", "gamma"] {
            app.entries.push(app_entry(n));
        }
        app.refresh();
        // with an empty query, Left/Right still step the selection and
        // Home/End still jump to the first/last entry
        app.on_key(Key::Right);
        assert_eq!(app.selected, 1);
        app.on_key(Key::Left);
        assert_eq!(app.selected, 0);
        app.on_key(Key::Bottom);
        assert_eq!(app.selected, 2);
        app.on_key(Key::Top);
        assert_eq!(app.selected, 0);
        // and typing switches those keys over to caret duty
        app.on_key(Key::Char('b'));
        app.on_key(Key::Top);
        assert_eq!(app.cursor, 0);
        assert_eq!(app.selected, 0);
    }
}
