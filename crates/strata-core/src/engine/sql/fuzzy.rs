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

    /// The invariant the subsequence pre-filter rests on: **every tier is a stronger
    /// condition than "is a subsequence"**, so rejecting a non-subsequence up front
    /// cannot drop a candidate any tier would have matched.
    ///
    /// This is the one claim in `match_tier` that is an argument rather than an
    /// observation, and it is invisible from the outside — a tier added later that can
    /// match a non-subsequence (an acronym or initialism matcher is the obvious
    /// candidate) would not fail a completion test, it would just silently stop being
    /// offered. So it is pinned here, per predicate, rather than through the ladder.
    #[test]
    fn every_tier_implies_a_subsequence_match() {
        let words = [
            "user_id",
            "USER_ID",
            "userid",
            "guid",
            "order_id",
            "_leading",
            "trailing_",
            "__dunder__",
            "a",
            "ab",
            "a_b",
            "created_at",
            "CreatedAt",
            "col_1",
            "COL_01",
            "précis",
            "über_id",
            "日本語",
            "sum",
            "count_star",
            "s3_bucket_name",
            "x9y",
            "_",
            "1",
        ];
        let mut partials: Vec<String> = words.iter().copied().map(String::from).collect();
        for w in words {
            let n = w.chars().count();
            for take in 1..=n.min(5) {
                partials.push(w.chars().take(take).collect());
                partials.push(w.chars().skip(1).take(take).collect());
            }
            partials.push(w.chars().step_by(2).collect());
            partials.push(w.to_ascii_uppercase());
        }

        let mut fired = [0usize; 4];
        for c in words {
            for p in &partials {
                if p.is_empty() {
                    continue;
                }
                let sub = is_subsequence(c, p);
                for (i, (tier, holds)) in [
                    ("exact", c.eq_ignore_ascii_case(p)),
                    ("prefix", starts_with_ci(c, p)),
                    ("word-boundary", word_boundary_match(c, p)),
                    ("contains", contains_ci(c, p)),
                ]
                .into_iter()
                .enumerate()
                {
                    if holds {
                        fired[i] += 1;
                    }
                    assert!(
                        !holds || sub,
                        "{tier} matched {c:?} against {p:?} but it is not a subsequence — \
                         the pre-filter in match_tier would drop it"
                    );
                }
            }
        }
        assert!(
            fired.iter().all(|&n| n > 0),
            "corpus never exercised some tier: {fired:?} (exact, prefix, word-boundary, contains)"
        );
    }

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
        assert_eq!(match_tier("user_id", "ui"), Some(2));
        assert_eq!(match_tier("guid", "ui"), Some(3));
    }

    #[test]
    fn hump_runs_continue_within_a_word() {
        assert_eq!(match_tier("order_id", "ordid"), Some(2));
    }

    #[test]
    fn gap_subsequence_matches_last() {
        assert_eq!(match_tier("user_id", "usrid"), Some(4));
    }

    #[test]
    fn non_subsequence_is_none() {
        assert_eq!(match_tier("amount", "xyz"), None);
        assert_eq!(match_tier("id", "idx"), None);
    }

    #[test]
    fn case_insensitive() {
        assert_eq!(match_tier("USER_ID", "ui"), Some(2));
        assert_eq!(match_tier("User_Id", "USRID"), Some(4));
    }
}
