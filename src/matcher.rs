//! Fuzzy matching, two strategies behind one function.
//!
//! * `Substring` — literal case-insensitive match, earlier occurrences rank higher.
//! * `Scored`    — fzf-style subsequence match: word boundaries and consecutive
//!   characters earn bonuses, gaps are penalized.
//!
//! Both return a list of `(entry index, score)` pairs sorted best-first.

use crate::config::MatchMode;

pub struct Scored {
    pub index: usize,
    pub score: i64,
}

/// Filter + rank `search_texts` against `query`.
pub fn filter(search_texts: &[String], query: &str, mode: MatchMode) -> Vec<Scored> {
    if query.is_empty() {
        // No query: list everything in order, all scores equal.
        return (0..search_texts.len())
            .map(|index| Scored { index, score: 0 })
            .collect();
    }

    let needle: Vec<char> = query.to_lowercase().chars().collect();
    let mut out = Vec::new();

    for (index, text) in search_texts.iter().enumerate() {
        let haystack: Vec<char> = text.to_lowercase().chars().collect();
        let score = match mode {
            MatchMode::Substring => substring_score(&haystack, &needle),
            MatchMode::Scored => subsequence_score(&haystack, &needle),
        };
        if let Some(score) = score {
            out.push(Scored { index, score });
        }
    }

    out.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| search_texts[a.index].cmp(&search_texts[b.index]))
    });
    out
}

/// Does the needle appear as a contiguous block? Earlier + word-start matches win.
fn substring_score(haystack: &[char], needle: &[char]) -> Option<i64> {
    if needle.len() > haystack.len() {
        return None;
    }
    for start in 0..=(haystack.len() - needle.len()) {
        if haystack[start..start + needle.len()] == needle[..] {
            let boundary = start == 0 || !haystack[start - 1].is_alphanumeric();
            return Some(64 - start as i64 + i64::from(boundary) * 32);
        }
    }
    None
}

/// Needle chars must appear in order (subsequence). Best single greedy pass.
fn subsequence_score(haystack: &[char], needle: &[char]) -> Option<i64> {
    let mut score: i64 = 0;
    let mut hi = 0usize;
    let mut last: Option<usize> = None;

    for &qc in needle {
        let mut found = None;
        while hi < haystack.len() {
            if haystack[hi] == qc {
                found = Some(hi);
                hi += 1;
                break;
            }
            hi += 1;
        }
        let pos = found?;

        let mut pts: i64 = 8; // base for any match
        if pos == 0 || !haystack[pos - 1].is_alphanumeric() {
            pts += 12; // start of a word (after space, -, _, ., / …)
        }
        match last {
            Some(prev) if pos == prev + 1 => pts += 8, // consecutive run
            Some(prev) => pts -= ((pos - prev - 1).min(8)) as i64, // gap penalty
            None => {}
        }
        score += pts;
        last = Some(pos);
    }
    Some(score)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn scored_matches_subsequences() {
        let t = texts(&["Firefox"]);
        assert!(filter(&t, "ffx", MatchMode::Scored).len() == 1);
        assert!(filter(&t, "zzz", MatchMode::Scored).is_empty());
    }

    #[test]
    fn scored_prefers_consecutive_and_boundaries() {
        // "ffmpeg" has f-f-m consecutive from the start; "Xfce File Manager" only
        // scattered matches — ffmpeg must rank first.
        let t = texts(&["Xfce File Manager", "ffmpeg"]);
        let out = filter(&t, "ffm", MatchMode::Scored);
        assert_eq!(out[0].index, 1);
    }

    #[test]
    fn substring_is_literal() {
        let t = texts(&["firefox"]);
        // "ffx" is a subsequence of firefox but NOT a contiguous substring
        assert!(filter(&t, "ffx", MatchMode::Substring).is_empty());
        assert!(filter(&t, "ffx", MatchMode::Scored).len() == 1);
        assert!(filter(&t, "fire", MatchMode::Substring).len() == 1);
    }

    #[test]
    fn substring_prefers_prefix() {
        let t = texts(&["xterm", "uxterm"]);
        let out = filter(&t, "xterm", MatchMode::Substring);
        assert_eq!(out[0].index, 0);
    }

    #[test]
    fn empty_query_lists_all() {
        let t = texts(&["a", "b", "c"]);
        assert_eq!(filter(&t, "", MatchMode::Scored).len(), 3);
    }

    #[test]
    fn case_insensitive() {
        let t = texts(&["VLC Media Player"]);
        assert_eq!(filter(&t, "vlc", MatchMode::Scored).len(), 1);
        assert_eq!(filter(&t, "VLC", MatchMode::Substring).len(), 1);
    }

    #[test]
    fn no_match_returns_empty_in_both_modes() {
        let t = texts(&["Firefox", "Foot", "Font Manager"]);
        assert!(filter(&t, "zzz", MatchMode::Scored).is_empty());
        assert!(filter(&t, "zzz", MatchMode::Substring).is_empty());
        // a longer needle than any haystack can never match either
        assert!(filter(&t, "firefox-web-browser", MatchMode::Substring).is_empty());
    }

    #[test]
    fn unicode_input_matches() {
        // accented Latin folds case correctly through to_lowercase
        let t = texts(&["Café Noir", "Plain Text Editor"]);
        let out = filter(&t, "café", MatchMode::Scored);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].index, 0);

        // CJK needs no case folding but must pass through the char pipeline
        let t = texts(&["文件管理器", "Archive Manager"]);
        assert_eq!(filter(&t, "文件", MatchMode::Substring).len(), 1);
        assert_eq!(filter(&t, "文件", MatchMode::Scored).len(), 1);

        // Greek, mixed case
        let t = texts(&["Ωμέγα"]);
        assert_eq!(filter(&t, "ωμέ", MatchMode::Scored).len(), 1);
    }

    #[test]
    fn equal_scores_tie_break_alphabetically() {
        // "ab" matches both at the same position with the same boundary bonus,
        // so the sort must fall back to the search text — "ab" before "ab2".
        let t = texts(&["ab2", "ab"]);
        let out = filter(&t, "ab", MatchMode::Substring);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].index, 1, "'ab' must rank before 'ab2' on a tie");
        assert_eq!(out[1].index, 0);
    }

    #[test]
    fn substring_and_scored_can_disagree_on_order() {
        // Verified against the exact scoring algorithm: "aa bb cc abc" piles
        // up word-boundary bonuses (+12 each) and consecutive runs (+8), so
        // scored mode ranks it FIRST even though its literal substring sits
        // late in the string — substring mode prefers the plain prefix.
        let t = texts(&["abc", "aa bb cc abc"]);

        let sub = filter(&t, "abc", MatchMode::Substring);
        assert_eq!(sub[0].index, 0, "substring mode: prefix match wins");

        let scored = filter(&t, "abc", MatchMode::Scored);
        assert_eq!(scored[0].index, 1, "scored mode: boundary-heavy entry wins");
    }
}
