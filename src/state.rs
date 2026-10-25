//! Shared launcher state, used identically by the TUI and the Wayland GUI.
//!
//! Both frontends:
//!   1. call [`App::on_key`] with their platform keys translated into [`Key`],
//!   2. draw `app.matches` however they like,
//!   3. act on the returned [`Flow`] (Continue / Launch / Exit).

use crate::config::{Config, Layout};
use crate::desktop::{self, DesktopEntry};
use crate::files::{self, ImageFile};
use crate::history::History;
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

impl Entry {
    /// Stable identifier for launch-history ranking: the sanitized `Exec=`
    /// for apps, the absolute path for images, the raw line for dmenu items.
    /// Survives re-scans (unlike an entry's index), so counts stay attached
    /// to the same app/file across runs.
    pub fn kind_key(&self) -> String {
        match &self.kind {
            EntryKind::App(a) => desktop::sanitize_exec(&a.exec),
            EntryKind::Image(f) => f.path.to_string_lossy().to_string(),
            EntryKind::Line => self.title.clone(),
        }
    }
}

/// One action to perform on Enter.
#[derive(Debug, PartialEq)]
pub enum Action {
    /// run a command line through `sh -c` (apps, and --files via `open-with`)
    Shell(String),
    /// print a line to stdout (dmenu mode: `… | foto --dmenu | cliphist decode`)
    Print(String),
}

#[derive(Debug, PartialEq)]
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
    /// Shift+Enter on an app with `[Desktop Action]`s: open its actions
    /// submenu (Esc — or Backspace on an empty query — returns to the apps)
    Actions,
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
    /// launch counts, feeding the frecency ranking in `refresh`
    pub history: History,
    /// Row geometry the Wayland renderer published for its last frame
    /// (physical pixels); the pointer handlers map clicks back to entries.
    /// None in the TUI (there is no pointer) and before the first frame.
    pub hit: Option<HitLayout>,
    /// The parked app list while an actions submenu is open (Shift+Enter);
    /// `None` means the normal list is active.
    pub submenu: Option<SavedView>,
}

/// The app list parked while an actions submenu is open, restored verbatim
/// on close (the saved matches are still valid — nothing can have changed
/// them: Tab is disabled inside the submenu).
pub struct SavedView {
    entries: Vec<Entry>,
    matches: Vec<Scored>,
    query: String,
    cursor: usize,
    selected: usize,
}

/// Where the renderer drew the visible rows/cells of its last frame, in
/// physical pixels — the same space as the pixmap. Lets the pointer
/// handlers translate a cursor position into a match index without
/// duplicating the render math.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HitLayout {
    /// false = list rows, true = --files grid cells
    pub grid: bool,
    /// first visible match index (scroll offset)
    pub start: usize,
    /// total match count (upper bound for indices)
    pub total: usize,
    /// list: y where the rows begin; row height
    pub top: f32,
    pub row_h: f32,
    /// how many rows (list) or cell rows (grid) are visible
    pub rows: usize,
    /// panel width and left padding (grid: where cells start)
    pub width: f32,
    pub pad: f32,
    /// grid only: cell size, cell height incl. label, gap, columns
    pub cell: f32,
    pub cell_h: f32,
    pub gap: f32,
    pub cols: usize,
}

impl HitLayout {
    /// Which match index sits at physical-pixel `(x, y)`, if any.
    pub fn index_at(&self, x: f32, y: f32) -> Option<usize> {
        if self.total == 0 || x < 0.0 || x >= self.width {
            return None;
        }
        let dy = y - self.top;
        if dy < 0.0 {
            return None;
        }
        if self.grid {
            if self.cols == 0 || self.cell_h <= 0.0 {
                return None;
            }
            let row_in_view = (dy / self.cell_h).floor() as usize;
            if row_in_view >= self.rows {
                return None;
            }
            let col = ((x - self.pad) / (self.cell + self.gap)).floor() as usize;
            if x < self.pad || col >= self.cols {
                return None;
            }
            let index = (self.start + row_in_view) * self.cols + col;
            (index < self.total).then_some(index)
        } else {
            if self.row_h <= 0.0 {
                return None;
            }
            let row_in_view = (dy / self.row_h).floor() as usize;
            if row_in_view >= self.rows {
                return None;
            }
            let index = self.start + row_in_view;
            (index < self.total).then_some(index)
        }
    }
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
        let history = History::load(cfg.history);
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
            history,
            hit: None,
            submenu: None,
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
        let history = History::load(cfg.history);
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
            history,
            hit: None,
            submenu: None,
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
        let history = History::load(cfg.history);
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
            history,
            hit: None,
            submenu: None,
        };
        app.refresh();
        Ok(app)
    }

    // ---------- state transitions ----------

    /// Recompute `matches` from the current query.
    pub fn refresh(&mut self) {
        let texts: Vec<String> = self.entries.iter().map(|e| e.search.clone()).collect();
        let mut matches = matcher::filter(&texts, &self.query, self.cfg.match_mode);

        // Frecency: launch counts tilt the ranking so day-to-day favorites
        // float to the top (an empty query becomes pure frequency order).
        // The bonus is capped, so relevance stays in charge — a hundred
        // launches never outweigh a query that clearly names another entry.
        if !self.history.is_empty() {
            for m in &mut matches {
                let count = i64::from(self.history.get(&self.entries[m.index].kind_key()));
                m.score += count.min(16) * 4;
            }
            // stable: equal final scores keep the matcher's own order
            // (alphabetical among equal matches, scan order with no query)
            matches.sort_by_key(|m| std::cmp::Reverse(m.score));
        }

        self.matches = matches;
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
        if self.dmenu || self.submenu.is_some() {
            return; // Tab keeps its normal meaning; actions come first
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

    // ---------- actions submenu (Shift+Enter) ----------

    /// Shift+Enter on an app with `[Desktop Action]`s: swap the app list
    /// for that app's actions (a real submenu — the app list is parked in
    /// [`SavedView`]). The submenu opens with an empty query so the actions
    /// show in their declared `Actions=` order; typing filters them.
    /// Esc — or Backspace once the query is empty — walks back out.
    /// A no-op for apps without actions (plain Enter still launches them),
    /// and in dmenu/images mode.
    pub fn open_actions(&mut self) {
        if self.submenu.is_some() || self.dmenu || self.files_mode {
            return;
        }
        let Some(m) = self.matches.get(self.selected) else {
            return;
        };
        let index = m.index;
        // Clone what the submenu needs out of the entry first: everything
        // below mutates self (park + replace the list), which cannot hold
        // a borrow into self.entries across the moves.
        let (app_name, app_icon, app_terminal, app_path, actions) = {
            let EntryKind::App(app) = &self.entries[index].kind else {
                return;
            };
            (
                app.name.clone(),
                app.icon.clone(),
                app.terminal,
                app.path.clone(),
                app.actions.clone(),
            )
        };
        if actions.is_empty() {
            return;
        }
        let entries: Vec<Entry> = actions
            .iter()
            .filter_map(|a| {
                let exec = desktop::sanitize_exec(&a.exec);
                if exec.is_empty() {
                    return None; // an action without a command is not launchable
                }
                Some(Entry {
                    title: a.name.clone(),
                    subtitle: exec.clone(),
                    // the parent's name is baked in so the preset query below
                    // (the app's name) matches every action
                    search: format!("{} {}", app_name.to_lowercase(), a.name.to_lowercase()),
                    kind: EntryKind::App(DesktopEntry {
                        name: a.name.clone(),
                        generic_name: String::new(),
                        comment: String::new(),
                        icon: if a.icon.is_empty() {
                            app_icon.clone()
                        } else {
                            a.icon.clone()
                        },
                        exec, // already sanitized; sanitize_exec is idempotent
                        terminal: app_terminal,
                        keywords: Vec::new(),
                        path: app_path.clone(),
                        actions: Vec::new(),
                    }),
                })
            })
            .collect();
        if entries.is_empty() {
            return;
        }
        self.submenu = Some(SavedView {
            entries: std::mem::take(&mut self.entries),
            matches: std::mem::take(&mut self.matches),
            query: std::mem::take(&mut self.query),
            cursor: self.cursor,
            selected: self.selected,
        });
        self.entries = entries;
        // empty query on purpose: the matcher passes declared order through
        // untouched (all scores equal), which is the submenu we want
        self.cursor = 0;
        self.selected = 0;
        self.refresh();
    }

    /// Leave the actions submenu; the parked app list comes back unchanged.
    fn close_actions(&mut self) {
        if let Some(saved) = self.submenu.take() {
            self.entries = saved.entries;
            self.matches = saved.matches;
            self.query = saved.query;
            self.cursor = saved.cursor;
            self.selected = saved.selected;
        }
    }

    /// Is an actions submenu open right now? (Renderers use it for hints.)
    pub fn in_actions(&self) -> bool {
        self.submenu.is_some()
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
        if self.submenu.is_some() {
            "actions"
        } else if self.dmenu {
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
        if self.dmenu || self.submenu.is_some() || self.alt_entries.is_none() {
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

    /// Scroll by `steps` wheel notches: one row in list layouts, one full
    /// row of thumbnails in grid mode. Positive steps move down.
    pub fn scroll(&mut self, steps: i32) {
        self.move_selection(steps * self.nav_step());
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
            Key::Escape => {
                // first Esc leaves an open actions submenu; the next one quits
                if self.submenu.is_some() {
                    self.close_actions();
                } else {
                    return Flow::Exit;
                }
            }
            Key::Enter => return self.activate(),
            Key::Actions => self.open_actions(),
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
                // a time); at the end of the query this is the old behavior.
                // With nothing left to delete inside the actions submenu,
                // Backspace walks back out to the apps list.
                let at = self.cursor.min(self.query.len());
                if let Some(c) = self.query[..at].chars().next_back() {
                    let start = at - c.len_utf8();
                    self.query.replace_range(start..at, "");
                    self.cursor = start;
                    self.refresh();
                } else if self.submenu.is_some() {
                    self.close_actions();
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

    /// Resolve the current selection into an action, recording the launch
    /// in the history. A selection that builds no action (nothing matched,
    /// or an app with an empty `Exec=`) keeps the UI open.
    fn activate(&mut self) -> Flow {
        let Some(m) = self.matches.get(self.selected) else {
            return Flow::Exit; // nothing selected — just close
        };
        let index = m.index;
        let Some(action) = self.action_for(index) else {
            return Flow::Continue; // e.g. an app entry with an empty Exec
        };
        // a real launch: count it so the next run ranks this entry higher
        let key = self.entries[index].kind_key();
        self.history.bump(&key);
        Flow::Launch(action)
    }

    /// The action behind entry `index`, or None when there is nothing to do.
    fn action_for(&self, index: usize) -> Option<Action> {
        match &self.entries[index].kind {
            EntryKind::App(app) => {
                let exec = desktop::sanitize_exec(&app.exec);
                if exec.is_empty() {
                    return None;
                }
                // terminal apps get wrapped in the configured terminal,
                // quoted safely; GUI apps run the command line as-is
                let cmd = if app.terminal {
                    format!("{} {}", self.cfg.terminal, shell_quote(&exec))
                } else {
                    exec
                };
                Some(Action::Shell(cmd))
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
                Some(Action::Shell(cmd))
            }
            // dmenu: hand the selected line back to the caller's pipe
            EntryKind::Line => Some(Action::Print(self.entries[index].title.clone())),
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
    use crate::desktop::DesktopAction;

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
            history: History::new(),
            hit: None,
            submenu: None,
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
            history: History::new(),
            hit: None,
            submenu: None,
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
            history: History::new(),
            hit: None,
            submenu: None,
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
            history: History::new(),
            hit: None,
            submenu: None,
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
            history: History::new(),
            hit: None,
            submenu: None,
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
                actions: Vec::new(),
            }),
        }
    }

    #[test]
    fn history_ranks_frequent_entries_first() {
        let mut app = bare_app(Config::default());
        for n in ["alpha", "beta", "gamma"] {
            app.entries.push(app_entry(n));
        }
        // pretend the user launches beta a lot
        app.history.seed("beta", 5);
        app.refresh();
        // empty query: pure frequency order — beta (5 launches) first
        assert_eq!(app.matches[0].index, 1, "beta must rank first");
        assert_eq!(app.entries[app.matches[0].index].title, "beta");

        // a query that clearly names another entry still wins: relevance
        // beats the (capped) frequency bonus
        app.query = "alph".into();
        app.refresh();
        assert_eq!(app.entries[app.matches[0].index].title, "alpha");
    }

    #[test]
    fn activate_records_launch_history() {
        let mut app = bare_app(Config::default());
        // give the history a file so bump() actually records
        let hist = std::env::temp_dir().join(format!("foto-state-hist-{}", std::process::id()));
        let _ = std::fs::remove_file(&hist);
        app.history.persist_to(&hist);
        app.entries.push(desktop_entry("Editor", "gedit", false));
        app.entries.push(img_entry("wall.png"));
        app.refresh();

        app.on_key(Key::Enter); // Editor
        assert_eq!(app.history.get("gedit"), 1);
        assert_eq!(app.history.get("/tmp/wall.png"), 0);

        // select the image and activate: its PATH is the key
        app.selected = 1;
        app.on_key(Key::Enter);
        assert_eq!(app.history.get("/tmp/wall.png"), 1);

        // failed activations (no matches) record nothing
        app.query = "zzz".into();
        app.refresh();
        app.on_key(Key::Enter); // exits with no matches
        assert_eq!(app.history.get("gedit"), 1, "counts unchanged");
        assert_eq!(app.history.get("/tmp/wall.png"), 1);
        let _ = std::fs::remove_file(&hist);
    }

    #[test]
    fn scroll_moves_selection() {
        let mut app = bare_app(Config::default());
        for n in ["a", "b", "c", "d", "e"] {
            app.entries.push(app_entry(n));
        }
        app.refresh();
        app.scroll(2);
        assert_eq!(app.selected, 2);
        app.scroll(-1);
        assert_eq!(app.selected, 1);
        // clamped at both ends
        app.scroll(-10);
        assert_eq!(app.selected, 0);
        app.scroll(100);
        assert_eq!(app.selected, 4);
        // grid mode: one notch = one row of thumbnails
        app.grid_cols = 3;
        app.files_mode = true;
        app.cfg.layout = Layout::Grid;
        app.selected = 1;
        app.scroll(1);
        assert_eq!(app.selected, 4);
    }

    #[test]
    fn hit_layout_list_rows() {
        let hit = HitLayout {
            grid: false,
            start: 2,
            total: 10,
            top: 100.0,
            row_h: 40.0,
            rows: 5,
            width: 600.0,
            pad: 12.0,
            cell: 0.0,
            cell_h: 0.0,
            gap: 0.0,
            cols: 1,
        };
        assert_eq!(hit.index_at(300.0, 100.0), Some(2), "first visible row");
        assert_eq!(hit.index_at(300.0, 139.9), Some(2), "same row near bottom");
        assert_eq!(hit.index_at(300.0, 140.0), Some(3), "second row");
        assert_eq!(hit.index_at(300.0, 299.9), Some(6), "last visible row");
        assert_eq!(hit.index_at(300.0, 300.0), None, "below the list");
        assert_eq!(hit.index_at(300.0, 50.0), None, "above the list");
        assert_eq!(hit.index_at(600.0, 120.0), None, "x is exclusive at width");
        assert_eq!(hit.index_at(-1.0, 120.0), None, "negative x");
    }

    #[test]
    fn hit_layout_grid_cells() {
        let hit = HitLayout {
            grid: true,
            start: 6, // scrolled one page (2 rows of 3)
            total: 24,
            top: 50.0,
            row_h: 0.0,
            rows: 2,
            width: 400.0,
            pad: 10.0,
            cell: 100.0,
            cell_h: 120.0,
            gap: 10.0,
            cols: 3,
        };
        assert_eq!(hit.index_at(10.0, 50.0), Some(18), "first cell of the view");
        assert_eq!(hit.index_at(130.0, 60.0), Some(19), "second column");
        assert_eq!(
            hit.index_at(115.0, 60.0),
            Some(18),
            "the gap is still column 0's row"
        );
        assert_eq!(hit.index_at(10.0, 170.0), Some(21), "second cell row");
        assert_eq!(hit.index_at(10.0, 50.0 + 240.0), None, "below the cells");
        assert_eq!(hit.index_at(10.0, 40.0), None, "above the cells");
        // the gap right of the last column is not a cell
        assert_eq!(hit.index_at(390.0, 60.0), None);
        // a smaller total clips the second row: indices 21.. stay invalid
        let mut clipped = hit;
        clipped.total = 21;
        assert_eq!(clipped.index_at(10.0, 170.0), None, "past the last entry");
    }

    #[test]
    fn hit_layout_empty_list_never_hits() {
        let hit = HitLayout {
            grid: false,
            start: 0,
            total: 0,
            top: 100.0,
            row_h: 40.0,
            rows: 5,
            width: 600.0,
            pad: 12.0,
            cell: 0.0,
            cell_h: 0.0,
            gap: 0.0,
            cols: 1,
        };
        assert_eq!(hit.index_at(300.0, 120.0), None);
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
            history: History::new(),
            hit: None,
            submenu: None,
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
            history: History::new(),
            hit: None,
            submenu: None,
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

    /// An app entry that carries two [Desktop Action]s, plus a plain app.
    fn actions_fixture() -> App {
        let mut app = bare_app(Config::default());
        app.entries.push(Entry {
            title: "Firefox".into(),
            subtitle: String::new(),
            search: "firefox".into(),
            kind: EntryKind::App(DesktopEntry {
                name: "Firefox".into(),
                generic_name: String::new(),
                comment: String::new(),
                icon: "firefox".into(),
                exec: "firefox %u".into(),
                terminal: false,
                keywords: Vec::new(),
                path: PathBuf::from("/tmp/firefox.desktop"),
                actions: vec![
                    DesktopAction {
                        name: "New Window".into(),
                        exec: "firefox --new-window %u".into(),
                        icon: String::new(),
                    },
                    DesktopAction {
                        name: "New Private Window".into(),
                        exec: "firefox --private-window".into(),
                        icon: "firefox-private".into(),
                    },
                ],
            }),
        });
        app.entries.push(app_entry("Editor"));
        app.refresh();
        app
    }

    #[test]
    fn shift_enter_opens_the_actions_submenu() {
        let mut app = actions_fixture();
        assert_eq!(app.matches[0].index, 0, "Firefox first (alphabetical)");
        app.on_key(Key::Actions);
        assert!(app.in_actions());
        assert_eq!(app.mode_label(), "actions");
        // the submenu opens with an empty query, preserving Actions= order
        assert_eq!(app.query, "");
        let titles: Vec<&str> = app
            .matches
            .iter()
            .map(|m| app.entries[m.index].title.as_str())
            .collect();
        assert_eq!(titles, ["New Window", "New Private Window"]);

        // Enter inside the submenu launches the ACTION's exec
        match app.on_key(Key::Enter) {
            Flow::Launch(Action::Shell(cmd)) => {
                assert_eq!(cmd, "firefox --new-window");
            }
            other => panic!("expected a shell launch, got {other:?}"),
        }
    }

    #[test]
    fn esc_and_backspace_leave_the_submenu_intact() {
        let mut app = actions_fixture();
        let before = app.matches.clone();
        app.on_key(Key::Actions);
        assert!(app.in_actions());

        // the submenu opens with an empty query, so the FIRST Backspace
        // walks straight back out
        app.on_key(Key::Backspace);
        assert!(!app.in_actions());
        assert_eq!(app.query, "");
        assert_eq!(app.selected, 0);

        // Esc too — and it does NOT quit foto on that first press
        app.on_key(Key::Actions);
        assert!(app.in_actions());
        assert_eq!(app.on_key(Key::Escape), Flow::Continue);
        assert!(!app.in_actions());
        // the parked app list is restored verbatim
        assert_eq!(app.matches, before);
        // ...and the SECOND Esc quits as always
        assert_eq!(app.on_key(Key::Escape), Flow::Exit);
    }

    #[test]
    fn actions_are_a_noop_without_actions_or_in_other_modes() {
        // a plain app: Shift+Enter does nothing, Enter still launches
        let mut app = bare_app(Config::default());
        app.entries.push(app_entry("Editor"));
        app.refresh();
        app.on_key(Key::Actions);
        assert!(!app.in_actions());
        assert!(matches!(app.on_key(Key::Enter), Flow::Launch(_)));

        // dmenu mode: never opens a submenu
        let mut menu = bare_app(Config::default());
        menu.dmenu = true;
        menu.entries.push(Entry {
            title: "line".into(),
            subtitle: String::new(),
            search: "line".into(),
            kind: EntryKind::Line,
        });
        menu.refresh();
        menu.on_key(Key::Actions);
        assert!(!menu.in_actions());
    }
}
