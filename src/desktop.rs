//! Scanning freedesktop.org `.desktop` application entries.
//!
//! Apps live in `$XDG_DATA_HOME/applications` and `$XDG_DATA_DIRS/applications`
//! (defaults: `~/.local/share`, `/usr/local/share`, `/usr/share`). Directories
//! earlier in the list win, and nested subdirectories are supported (their
//! slash-separated path becomes the app id with dashes).

use anyhow::Result;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DesktopEntry {
    /// Display name (`Name=`), e.g. "Firefox"
    pub name: String,
    /// Optional `GenericName=`, e.g. "Web Browser"
    pub generic_name: String,
    /// Optional `Comment=` — a longer description
    pub comment: String,
    /// Icon name from `Icon=` (an icon-theme name or an absolute path).
    /// Only drawn by the GUI; kept for `--list` in core builds.
    #[cfg_attr(not(feature = "gui"), allow(dead_code))]
    pub icon: String,
    /// Raw `Exec=` string, field codes still included
    pub exec: String,
    /// `Terminal=true` means the app must be wrapped in a terminal
    pub terminal: bool,
    /// `Keywords=` entries, so "browser" also finds "Firefox"
    pub keywords: Vec<String>,
    /// Path of the .desktop file itself (shown by --list debugging, kept for future use)
    #[allow(dead_code)]
    pub path: PathBuf,
    /// `[Desktop Action …]` sub-entries, in the order the main section's
    /// `Actions=` key lists them (e.g. Firefox's "New Window"). Empty when
    /// the app has none — Shift+Enter simply does nothing then.
    pub actions: Vec<DesktopAction>,
}

/// One `[Desktop Action id]` sub-entry of a `.desktop` file — a secondary
/// launch target like "New Private Window", reachable through the actions
/// submenu (Shift+Enter on the parent app).
#[derive(Debug, Clone)]
pub struct DesktopAction {
    /// display name (`Name=`), e.g. "New Private Window"
    pub name: String,
    /// raw `Exec=` string, field codes still included
    pub exec: String,
    /// optional `Icon=`; the parent's icon is used when empty
    pub icon: String,
}

/// XDG data directories: $XDG_DATA_DIRS (or the spec default) with
/// $XDG_DATA_HOME (or ~/.local/share) prepended.
pub fn data_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(home) = dirs::data_dir() {
        v.push(home);
    }
    let sys = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    v.extend(sys.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    v
}

/// All launchable apps, sorted by lowercase name.
pub fn scan() -> Result<Vec<DesktopEntry>> {
    let mut seen: HashMap<String, DesktopEntry> = HashMap::new();

    for dir in data_dirs() {
        collect_dir(&dir.join("applications"), String::new(), &mut seen)?;
    }

    let mut out: Vec<DesktopEntry> = seen.into_values().collect();
    out.sort_by_key(|a| a.name.to_lowercase());
    Ok(out)
}

fn collect_dir(dir: &Path, prefix: String, seen: &mut HashMap<String, DesktopEntry>) -> Result<()> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Ok(()); // missing directory is normal, not an error
    };
    for entry in read.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(String::from) else {
            continue;
        };
        if path.is_dir() {
            let sub_prefix = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}-{name}")
            };
            collect_dir(&path, sub_prefix, seen)?;
        } else if name.ends_with(".desktop") {
            let id = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}-{name}")
            };
            // first directory in the XDG search order wins; unparseable files are skipped
            if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(id) {
                if let Some(entry) = parse(&path) {
                    e.insert(entry);
                }
            }
        }
    }
    Ok(())
}

/// Parse one .desktop file; returns `None` for anything not launchable
/// (Link files, Hidden=true, NoDisplay=true, missing Name/Exec, TryExec misses).
fn parse(path: &Path) -> Option<DesktopEntry> {
    let text = std::fs::read_to_string(path).ok()?;
    parse_str(&text, path)
}

pub(crate) fn parse_str(text: &str, path: &Path) -> Option<DesktopEntry> {
    let mut section = String::new();
    let mut name = None;
    let mut generic_name = String::new();
    let mut comment = String::new();
    let mut icon = String::new();
    let mut exec = None;
    let mut terminal = false;
    let mut keywords = Vec::new();
    let mut try_exec = None;
    // `Actions=` ids in declared order (the display order of the submenu)
    let mut actions_order: Vec<String> = Vec::new();
    // raw Name/Exec/Icon collected from `[Desktop Action id]` sections
    let mut raw_actions: HashMap<String, DesktopAction> = HashMap::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            // [Desktop Entry] vs [Desktop Action xyz] — we want both: the
            // former IS the app, the latter feeds its actions submenu
            section = line.trim_matches(|c| c == '[' || c == ']').to_string();
            continue;
        }
        if section != "Desktop Entry" {
            // inside [Desktop Action id]: remember Name/Exec/Icon per id
            if let Some(id) = section.strip_prefix("Desktop Action ") {
                if let Some((key, value)) = line.split_once('=') {
                    let key = key.trim();
                    if key.contains('[') {
                        continue; // localized keys like Name[de]
                    }
                    let value = value.trim();
                    let slot = raw_actions.entry(id.to_string()).or_insert(DesktopAction {
                        name: String::new(),
                        exec: String::new(),
                        icon: String::new(),
                    });
                    match key {
                        "Name" => slot.name = value.to_string(),
                        "Exec" => slot.exec = value.to_string(),
                        "Icon" => slot.icon = value.to_string(),
                        _ => {}
                    }
                }
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.contains('[') {
            continue; // localized keys like Name[de] — skip
        }
        let value = value.trim();
        match key {
            "Type" if value != "Application" => return None,
            "Hidden" if value == "true" => return None,
            "NoDisplay" if value == "true" => return None,
            "Name" => name = Some(value.to_string()),
            "GenericName" => generic_name = value.to_string(),
            "Comment" => comment = value.to_string(),
            "Icon" => icon = value.to_string(),
            "Exec" => exec = Some(value.to_string()),
            "Terminal" => terminal = value == "true",
            "Keywords" => {
                keywords = value
                    .split(';')
                    .map(|k| k.trim().to_string())
                    .filter(|k| !k.is_empty())
                    .collect();
            }
            "TryExec" => try_exec = Some(value.to_string()),
            "Actions" => {
                actions_order = value
                    .split(';')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            _ => {}
        }
    }

    if let Some(te) = try_exec {
        if !te.is_empty() && !exists_in_path(&te) {
            return None; // the app itself is not installed
        }
    }

    // Actions only count when the main section lists them (spec order =
    // display order); ids without a section (or without Name+Exec) are skipped.
    let actions = actions_order
        .into_iter()
        .filter_map(|id| raw_actions.remove(&id))
        .filter(|a| !a.name.is_empty() && !a.exec.is_empty())
        .collect();

    Some(DesktopEntry {
        name: name?,
        generic_name,
        comment,
        icon,
        exec: exec?,
        terminal,
        keywords,
        path: path.to_path_buf(),
        actions,
    })
}

fn exists_in_path(bin: &str) -> bool {
    if bin.contains('/') {
        return Path::new(bin).exists();
    }
    std::env::var("PATH")
        .map(|paths| {
            paths
                .split(':')
                .any(|dir| !dir.is_empty() && Path::new(dir).join(bin).is_file())
        })
        .unwrap_or(false)
}

/// Strip freedesktop Exec field codes so the string can run under `sh -c`.
///
/// `%f %F %u %U` (file args), `%d %D %n %N %i %c %k %v %m` (legacy metadata) are
/// removed — we have no file to pass when launching from a menu. `%%` is an
/// escaped percent and stays as a single `%`.
pub fn sanitize_exec(exec: &str) -> String {
    let mut out = String::with_capacity(exec.len());
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('%') => out.push('%'),
            // consume the code silently (codes with an argument, like %f file, can't happen here)
            Some(code) if "fFuUdDnNiNcCkKvVmM".contains(code) => {}
            Some(other) => {
                out.push('%');
                out.push(other);
            }
            None => {}
        }
    }
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[Desktop Entry]
Type=Application
Name=Firefox Web Browser
GenericName=Web Browser
Comment=Browse the World Wide Web
Exec=firefox %u
Icon=firefox
Terminal=false
Keywords=web;browser;internet;
TryExec=

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window %u
"#;

    #[test]
    fn parses_main_section_only() {
        let e = parse_str(SAMPLE, Path::new("/tmp/firefox.desktop")).unwrap();
        assert_eq!(e.name, "Firefox Web Browser");
        assert_eq!(e.generic_name, "Web Browser");
        assert_eq!(e.icon, "firefox");
        assert_eq!(e.exec, "firefox %u");
        assert!(e.keywords.contains(&"browser".to_string()));
        assert!(!e.terminal);
    }

    #[test]
    fn skips_hidden_and_links() {
        let hidden = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nHidden=true\n";
        assert!(parse_str(hidden, Path::new("x.desktop")).is_none());

        let link = "[Desktop Entry]\nType=Link\nName=X\nURL=http://x\n";
        assert!(parse_str(link, Path::new("x.desktop")).is_none());
    }

    #[test]
    fn skips_nodisplay() {
        let nodisplay = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=true\n";
        assert!(parse_str(nodisplay, Path::new("x.desktop")).is_none());
        // explicit false stays visible
        let shown = "[Desktop Entry]\nType=Application\nName=X\nExec=x\nNoDisplay=false\n";
        assert!(parse_str(shown, Path::new("x.desktop")).is_some());
    }

    #[test]
    fn try_exec_missing_binary_skips_entry() {
        let t = "[Desktop Entry]\nType=Application\nName=Ghost\n\
                 Exec=ghost\nTryExec=foto-test-no-such-binary-xyz\n";
        assert!(parse_str(t, Path::new("g.desktop")).is_none());
    }

    #[test]
    fn try_exec_present_binary_keeps_entry() {
        // `sh` exists everywhere the test suite runs
        let t = "[Desktop Entry]\nType=Application\nName=Sh App\n\
                 Exec=sh-app\nTryExec=sh\n";
        let e = parse_str(t, Path::new("s.desktop")).expect("TryExec=sh must resolve");
        assert_eq!(e.name, "Sh App");
    }

    #[test]
    fn terminal_flag() {
        let t = "[Desktop Entry]\nType=Application\nName=HTop\nExec=htop\nTerminal=true\n";
        let e = parse_str(t, Path::new("h.desktop")).unwrap();
        assert!(e.terminal);
    }

    #[test]
    fn sanitizes_field_codes() {
        assert_eq!(sanitize_exec("firefox %u"), "firefox");
        assert_eq!(sanitize_exec("nvim %F"), "nvim");
        assert_eq!(sanitize_exec("100% done"), "100% done"); // bare % kept
        assert_eq!(sanitize_exec("a %% b %u"), "a % b");
        assert_eq!(sanitize_exec("code --new-window %F"), "code --new-window");
    }

    #[test]
    fn sanitizes_all_legacy_field_codes() {
        // %u (url), %f (file), %i (icon) and %c (class) must all be stripped,
        // in any position. The spaces around a stripped code stay — the
        // result runs under `sh -c`, which collapses runs of whitespace, so
        // the double space is harmless.
        assert_eq!(sanitize_exec("app %i --opt %f %u"), "app  --opt");
        // only the END is trimmed; a leading code leaves a leading space
        // (documenting current behavior — the UI never shows it)
        assert_eq!(sanitize_exec("%c app %k"), " app");
        assert_eq!(sanitize_exec("app --flag=%U %d %n %v %m"), "app --flag=");
        // a lone trailing code leaves no trailing whitespace
        assert_eq!(sanitize_exec("gedit %F"), "gedit");
    }

    #[test]
    fn parses_desktop_actions_in_declared_order() {
        let t = r#"
[Desktop Entry]
Type=Application
Name=Firefox
Exec=firefox %u
Actions=new-window;private;

[Desktop Action private]
Name=New Private Window
Exec=firefox --private-window %u
Icon=firefox-private

[Desktop Action new-window]
Name=New Window
Exec=firefox --new-window %u
"#;
        let e = parse_str(t, Path::new("f.desktop")).unwrap();
        assert_eq!(e.actions.len(), 2, "both listed actions survive");
        assert_eq!(e.actions[0].name, "New Window", "Actions= order wins");
        assert_eq!(e.actions[0].exec, "firefox --new-window %u");
        assert_eq!(e.actions[1].name, "New Private Window");
        assert_eq!(e.actions[1].icon, "firefox-private");
    }

    #[test]
    fn unlisted_or_empty_actions_are_ignored() {
        // action sections exist but Actions= is absent -> none are exposed
        let no_list = "[Desktop Entry]\nType=Application\nName=X\nExec=x\n\n\
                       [Desktop Action a]\nName=A\nExec=a\n";
        assert!(parse_str(no_list, Path::new("x.desktop"))
            .unwrap()
            .actions
            .is_empty());
        // an id listed but with no section (or no Exec) is skipped
        let ghost = "[Desktop Entry]\nType=Application\nName=X\nExec=x\n\
                     Actions=ghost;real;\n\n\
                     [Desktop Action ghost]\nName=Ghost\n\n\
                     [Desktop Action real]\nName=Real\nExec=real\n";
        let e = parse_str(ghost, Path::new("x.desktop")).unwrap();
        assert_eq!(e.actions.len(), 1);
        assert_eq!(e.actions[0].name, "Real");
    }
}
