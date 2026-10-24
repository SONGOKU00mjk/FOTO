//! Launch history: the launcher remembers what you open most.
//!
//! Every successful activation bumps a per-entry counter, persisted in
//! `$XDG_CACHE_HOME/foto/history`. On the next launch the counts feed a
//! score bonus in [`crate::state::App::refresh`], so the apps you actually
//! use day-to-day float to the top of the list — the same "frecency" idea
//! fuzzel popularized.
//!
//! File format (one entry per line, TAB-separated):
//!
//! ```text
//! <count>\t<key>
//! ```
//!
//! The key is the entry's stable identifier: the sanitized `Exec=` for
//! apps, the absolute path for images, the raw line for `--dmenu` items.
//! Keys may themselves contain TABs (cliphist lines do); parsing splits on
//! the FIRST TAB only, so that is safe.
//!
//! Everything is best-effort: a missing, unreadable or malformed file is
//! simply an empty history, and write failures are ignored — a usage
//! counter must never break launching. `history #false` in the config (or
//! an empty key) disables the whole mechanism.

use std::collections::HashMap;
use std::path::PathBuf;

/// Entries kept in the file. Past this, the least-used keys are dropped so
/// a long session cannot grow the file without bound.
const MAX_ENTRIES: usize = 512;
/// Per-entry count cap: saturation keeps a runaway favorite from dominating
/// the score bonus arithmetic.
const MAX_COUNT: u32 = 9_999;

#[derive(Debug, Clone)]
pub struct History {
    counts: HashMap<String, u32>,
    /// Where to persist; `None` = ephemeral (tests, or `history #false`).
    path: Option<PathBuf>,
}

impl Default for History {
    fn default() -> Self {
        Self::new()
    }
}

impl History {
    /// Empty, non-persisted history.
    pub fn new() -> Self {
        Self {
            counts: HashMap::new(),
            path: None,
        }
    }

    /// Load the user history (honoring the `history` config switch).
    /// A missing file just means "no history yet".
    pub fn load(enabled: bool) -> Self {
        if !enabled {
            return Self::new();
        }
        match cache_path() {
            Some(path) => Self::load_from(&path),
            None => Self::new(),
        }
    }

    /// Load from an explicit file (tests point this at a temp path).
    pub fn load_from(path: &std::path::Path) -> Self {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        Self {
            counts: parse(&text),
            path: Some(path.to_path_buf()),
        }
    }

    pub fn get(&self, key: &str) -> u32 {
        self.counts.get(key).copied().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// Test-only: seed a count directly (no file, no bump).
    #[cfg(test)]
    pub fn seed(&mut self, key: &str, count: u32) {
        self.counts.insert(key.to_string(), count);
    }

    /// Test-only: point the history at a file so bump() records.
    #[cfg(test)]
    pub fn persist_to(&mut self, path: &std::path::Path) {
        self.path = Some(path.to_path_buf());
    }

    /// Count one more launch of `key` and persist the table. Failures are
    /// silently ignored — history is a nicety, never a requirement.
    pub fn bump(&mut self, key: &str) {
        if key.is_empty() || self.path.is_none() {
            return;
        }
        let count = self.counts.entry(key.to_string()).or_insert(0);
        *count = count.saturating_add(1).min(MAX_COUNT);
        if self.counts.len() > MAX_ENTRIES {
            self.prune();
        }
        self.save();
    }

    /// Drop the least-used entries until the table fits MAX_ENTRIES.
    fn prune(&mut self) {
        while self.counts.len() > MAX_ENTRIES {
            let weakest = self
                .counts
                .iter()
                .min_by_key(|(_, &c)| c)
                .map(|(k, _)| k.clone());
            match weakest {
                Some(key) => {
                    self.counts.remove(&key);
                }
                None => break,
            }
        }
    }

    /// Rewrite the whole file, most-used first. The table is tiny (a few
    /// hundred short lines), so a full rewrite beats any incremental log.
    fn save(&self) {
        let Some(path) = &self.path else { return };
        let mut lines: Vec<(&String, &u32)> = self.counts.iter().collect();
        lines.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
        let text = lines
            .iter()
            .map(|(key, count)| format!("{count}\t{key}"))
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write(path, text + "\n");
    }
}

/// `$XDG_CACHE_HOME/foto/history`.
pub fn cache_path() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("foto").join("history"))
}

/// Parse `count\tkey` lines; malformed lines are skipped, counts are
/// clamped. Keys keep everything after the first TAB verbatim.
fn parse(text: &str) -> HashMap<String, u32> {
    text.lines()
        .filter_map(|line| {
            let (count, key) = line.split_once('\t')?;
            let count: u32 = count.trim().parse().ok()?;
            if key.is_empty() {
                None
            } else {
                Some((key.to_string(), count.min(MAX_COUNT)))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("foto-history-{tag}-{}", std::process::id()))
    }

    #[test]
    fn parse_roundtrip() {
        let mut h = History::load_from(&temp_path("roundtrip"));
        // load_from on a missing file starts empty and records the path,
        // so bump() below actually persists
        assert!(h.is_empty());
        h.bump("firefox");
        h.bump("firefox");
        h.bump("foot");
        assert_eq!(h.get("firefox"), 2);
        assert_eq!(h.get("foot"), 1);
        assert_eq!(h.get("never-launched"), 0);

        // reload from disk: counts survive
        let h2 = History::load_from(&temp_path("roundtrip"));
        assert_eq!(h2.get("firefox"), 2);
        assert_eq!(h2.get("foot"), 1);
        let _ = std::fs::remove_file(temp_path("roundtrip"));
    }

    #[test]
    fn keys_with_tabs_survive() {
        // cliphist lines contain TABs: "1\timage/png\t223KB"
        let line = "1\timage/png\t223KB";
        let mut h = History::load_from(&temp_path("tabs"));
        h.bump(line);
        assert_eq!(h.get(line), 1);
        let h2 = History::load_from(&temp_path("tabs"));
        assert_eq!(h2.get(line), 1, "the full line must round-trip");
        let _ = std::fs::remove_file(temp_path("tabs"));
    }

    #[test]
    fn malformed_lines_are_skipped() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("foto-history-malformed-{}", std::process::id()));
        std::fs::write(
            &path,
            "garbage line without tab\n\
             not-a-number\tfirefox\n\
             3\tok\n\
             \t\n\
             5\t\n\
             2\talso-ok\n",
        )
        .unwrap();
        let h = History::load_from(&path);
        assert_eq!(h.get("ok"), 3);
        assert_eq!(h.get("also-ok"), 2);
        assert_eq!(h.get("firefox"), 0, "unparsable count ignored");
        assert_eq!(h.counts.len(), 2);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn disabled_history_never_touches_disk() {
        let mut h = History::load(false);
        h.bump("firefox");
        assert_eq!(h.get("firefox"), 0, "no path -> no recording");
        assert!(History::load(false).is_empty());
    }

    #[test]
    fn empty_key_is_ignored() {
        let mut h = History::load_from(&temp_path("emptykey"));
        h.bump("");
        assert!(h.is_empty());
        let _ = std::fs::remove_file(temp_path("emptykey"));
    }

    #[test]
    fn prune_keeps_the_most_used() {
        let mut h = History::new();
        h.path = Some(temp_path("prune")); // force persistence on
        for i in 0..(MAX_ENTRIES + 10) {
            h.counts.insert(format!("app{i}"), 1);
        }
        h.counts.insert("favorite".into(), 100);
        h.bump("favorite"); // triggers prune
        assert!(h.counts.len() <= MAX_ENTRIES);
        assert!(h.counts.contains_key("favorite"), "the favorite survives");
        let _ = std::fs::remove_file(temp_path("prune"));
    }

    #[test]
    fn counts_saturate() {
        let mut h = History::new();
        h.path = Some(temp_path("sat"));
        for _ in 0..(MAX_COUNT + 50) {
            h.bump("many");
        }
        assert_eq!(h.get("many"), MAX_COUNT);
        let _ = std::fs::remove_file(temp_path("sat"));
    }
}
