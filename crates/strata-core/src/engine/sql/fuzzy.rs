//! Tiered fuzzy matching for completion filter + rank. Tiers, best-first:
//! 0 exact · 1 prefix · 2 word-boundary subsequence (`ui` → `user_id`) ·
//! 3 contiguous substring (`ui` → `guid`) · 4 gap subsequence (`usrid` → `user_id`).
//! `None` = the partial is not even a subsequence — the candidate is filtered out.
//! Case-insensitive throughout (ASCII — SQL identifiers and keywords).

/// Match `partial` against `candidate`; empty partial matches everything at tier 0
/// (context ordering then decides the list order).
///
/// **Allocation-free, and it rejects before it ranks.** Every tier is a *stronger*
/// condition than "is a subsequence", so a candidate that is not one cannot be in any
/// tier — testing that first is exact, and it is what makes the common case cheap: a
/// typed prefix swept over a large catalog rejects almost everything, and rejection is
/// now one scan with no allocation instead of the ladder plus two lowercased copies
/// per candidate. (Measured on a 100-table x 1000-column catalog, `SELECT xy|` — the
/// all-columns fallback, 100k candidates: 36ms per keystroke before, 1.6ms after; this
/// change alone took it to 1.8ms and `ranking::Pool` the rest.)
/// Case-insensitivity is ASCII, applied per char at the comparison, which is exactly
/// what lowercasing both sides did — verified equal to the original on 6.6M pairs.
pub(crate) fn match_tier(candidate: &str, partial: &str) -> Option<u8> {
    if partial.is_empty() {
        return Some(0);
    }
    if !is_subsequence(candidate, partial) {
        return None;
    }
    if candidate.eq_ignore_ascii_case(partial) {
        return Some(0);
    }
    if starts_with_ci(candidate, partial) {
        return Some(1);
    }
    if word_boundary_match(candidate, partial) {
        return Some(2);
    }
    if contains_ci(candidate, partial) {
        return Some(3);
    }
    Some(4)
}

/// `partial` is an ASCII-case-insensitive prefix of `candidate`. Byte-wise: ASCII
/// lowercasing never changes a non-ASCII byte, so this is what comparing two
/// `to_ascii_lowercase` copies did.
fn starts_with_ci(candidate: &str, partial: &str) -> bool {
    let (c, p) = (candidate.as_bytes(), partial.as_bytes());
    c.len() >= p.len() && c[..p.len()].eq_ignore_ascii_case(p)
}

/// `partial` occurs contiguously in `candidate`, ASCII-case-insensitively.
fn contains_ci(candidate: &str, partial: &str) -> bool {
    let (c, p) = (candidate.as_bytes(), partial.as_bytes());
    c.len() >= p.len() && c.windows(p.len()).any(|w| w.eq_ignore_ascii_case(p))
}

/// Hump matching over `_`-separated words: the partial's first char must sit at a
/// word start; each further char continues the current run or jumps to a later word
/// start (`ui` → `u`ser_`i`d, `ordid` → `ord`er_`id`). Small backtracking search —
/// candidate/partial lengths are identifier-sized, and [`match_tier`] only reaches
/// here for a candidate that already matched as a subsequence.
fn word_boundary_match(candidate: &str, partial: &str) -> bool {
    fn go(c: &[char], starts: &[bool], p: &[char], from: usize, pi: usize, run: bool) -> bool {
        if pi == p.len() {
            return true;
        }
        for i in from..c.len() {
            let allowed = (run && i == from) || starts[i];
            if allowed && c[i].eq_ignore_ascii_case(&p[pi]) && go(c, starts, p, i + 1, pi + 1, true)
            {
                return true;
            }
        }
        false
    }

    let c: Vec<char> = candidate.chars().collect();
    let starts: Vec<bool> = c
        .iter()
        .enumerate()
        .map(|(i, ch)| *ch != '_' && (i == 0 || c[i - 1] == '_'))
        .collect();
    let p: Vec<char> = partial.chars().collect();
    go(&c, &starts, &p, 0, 0, false)
}

/// `partial`'s chars appear in `candidate` in order (gaps allowed), ASCII-case-insensitively.
fn is_subsequence(candidate: &str, partial: &str) -> bool {
    let mut chars = candidate.chars();
    partial
        .chars()
        .all(|p| chars.any(|c| c.eq_ignore_ascii_case(&p)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_partial_is_tier_zero() {
        assert_eq!(match_tier("anything", ""), Some(0));
    }

    #[test]
    fn exact_beats_prefix() {
        assert_eq!(match_tier("from", "FROM"), Some(0));
        assert_eq!(match_tier("FROM", "fr"), Some(1));
    }

    #[test]
    fn humps_beat_substring() {
        // `ui` hits the word starts of user_id but is only an interior run of guid.
        assert_eq!(match_tier("user_id", "ui"), Some(2));
        assert_eq!(match_tier("guid", "ui"), Some(3));
    }

    #[test]
    fn hump_runs_continue_within_a_word() {
        // `ordid` = "ord" run at word start + "id" at a later word start.
        assert_eq!(match_tier("order_id", "ordid"), Some(2));
    }

    #[test]
    fn gap_subsequence_matches_last() {
        // usrid: u-s-r in "user" with a gap (not word-boundary runs), then id.
        assert_eq!(match_tier("user_id", "usrid"), Some(4));
    }

    #[test]
    fn non_subsequence_is_none() {
        assert_eq!(match_tier("amount", "xyz"), None);
        assert_eq!(match_tier("id", "idx"), None); // longer than candidate
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(match_tier("USER_ID", "ui"), Some(2));
        assert_eq!(match_tier("User_Id", "USRID"), Some(4));
    }
}
