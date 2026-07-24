//! Tiered fuzzy matching for completion filter + rank. Tiers, best-first:
//! 0 exact · 1 prefix · 2 word-boundary subsequence (`ui` → `user_id`) ·
//! 3 contiguous substring (`ui` → `guid`) · 4 gap subsequence (`usrid` → `user_id`).
//! `None` = the partial is not even a subsequence — the candidate is filtered out.
//! Case-insensitive throughout (ASCII — SQL identifiers and keywords).

/// Match `partial` against `candidate`; empty partial matches everything at tier 0
/// (context ordering then decides the list order).
pub(crate) fn match_tier(candidate: &str, partial: &str) -> Option<u8> {
    if partial.is_empty() {
        return Some(0);
    }
    let c = candidate.to_ascii_lowercase();
    let p = partial.to_ascii_lowercase();
    if c == p {
        return Some(0);
    }
    if c.starts_with(&p) {
        return Some(1);
    }
    if word_boundary_match(&c, &p) {
        return Some(2);
    }
    if c.contains(&p) {
        return Some(3);
    }
    if is_subsequence(&c, &p) {
        return Some(4);
    }
    None
}

/// Hump matching over `_`-separated words: the partial's first char must sit at a
/// word start; each further char continues the current run or jumps to a later word
/// start (`ui` → `u`ser_`i`d, `ordid` → `ord`er_`id`). Small backtracking search —
/// candidate/partial lengths are identifier-sized.
fn word_boundary_match(candidate: &str, partial: &str) -> bool {
    let c: Vec<char> = candidate.chars().collect();
    let starts: Vec<bool> = c
        .iter()
        .enumerate()
        .map(|(i, ch)| *ch != '_' && (i == 0 || c[i - 1] == '_'))
        .collect();
    let p: Vec<char> = partial.chars().collect();

    fn go(c: &[char], starts: &[bool], p: &[char], from: usize, pi: usize, run: bool) -> bool {
        if pi == p.len() {
            return true;
        }
        for i in from..c.len() {
            let allowed = (run && i == from) || starts[i];
            if allowed && c[i] == p[pi] && go(c, starts, p, i + 1, pi + 1, true) {
                return true;
            }
        }
        false
    }
    go(&c, &starts, &p, 0, 0, false)
}

/// `partial`'s chars appear in `candidate` in order (gaps allowed).
fn is_subsequence(candidate: &str, partial: &str) -> bool {
    let mut chars = candidate.chars();
    partial.chars().all(|p| chars.any(|c| c == p))
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
