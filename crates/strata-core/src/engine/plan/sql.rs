//! EXPLAIN text helpers: detect an `EXPLAIN` statement, rewrite SQL under
//! `EXPLAIN [ANALYZE]`, and split an operator's one-line display into name/detail.
//! Pure string work — the only plan code that touches SQL text at all.

/// Does this SQL start with `EXPLAIN` (ignoring leading comments / parens)?
pub fn is_explain(sql: &str) -> bool {
    let mut bare = String::new();
    for line in sql.lines() {
        let l = match line.find("--") {
            Some(i) => &line[..i],
            None => line,
        };
        bare.push_str(l);
        bare.push(' ');
    }
    bare.trim_start_matches(['(', ' ', '\t', '\n', '\r'])
        .to_ascii_lowercase()
        .starts_with("explain")
}

/// Rewrite `sql` to run under `EXPLAIN` / `EXPLAIN ANALYZE` (E4): strip any existing
/// leading `EXPLAIN [ANALYZE] [VERBOSE]` keyword sequence, then prepend the requested
/// prefix. The editor's Explain-plan / Explain-analyze buttons rewrite the buffer with
/// this so the mode is explicit + user-editable, then run.
pub fn as_explain(sql: &str, analyze: bool) -> String {
    // Strip a leading keyword (word-boundary, case-insensitive), returning the rest.
    fn strip<'a>(s: &'a str, kw: &str) -> Option<&'a str> {
        s.get(..kw.len())
         .filter(|h| h.eq_ignore_ascii_case(kw))
         .filter(|_| {
             s[kw.len()..]
                 .chars()
                 .next()
                 .map_or(true, |c| c.is_whitespace())
         })
         .map(|_| s[kw.len()..].trim_start())
    }
    let mut body = sql.trim_start();
    if let Some(rest) = strip(body, "explain") {
        body = rest;
        if let Some(rest) = strip(body, "analyze") {
            body = rest;
        }
        if let Some(rest) = strip(body, "verbose") {
            body = rest;
        }
    }
    let prefix = if analyze {
        "EXPLAIN ANALYZE"
    } else {
        "EXPLAIN"
    };
    if body.is_empty() {
        prefix.to_string()
    } else {
        format!("{prefix}\n{body}")
    }
}

/// Split a `"<Name>: <detail>"` operator line (from DataFusion's `display()` /
/// `one_line()`) into (name, detail). No colon → all name.
pub fn split_name_detail(line: &str) -> (String, String) {
    match line.find(':') {
        Some(i) => (
            line[..i].trim().to_string(),
            line[i + 1..].trim().to_string(),
        ),
        None => (line.trim().to_string(), String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_explain() {
        assert!(is_explain("EXPLAIN SELECT 1"));
        assert!(is_explain("  explain analyze select * from t"));
        assert!(is_explain("-- plan\nEXPLAIN SELECT 1"));
        assert!(!is_explain("SELECT * FROM explain_table"));
    }

    #[test]
    fn as_explain_strips_and_reapplies() {
        assert_eq!(as_explain("SELECT 1", false), "EXPLAIN\nSELECT 1");
        assert_eq!(as_explain("SELECT 1", true), "EXPLAIN ANALYZE\nSELECT 1");
        assert_eq!(
            as_explain("EXPLAIN SELECT 1", true),
            "EXPLAIN ANALYZE\nSELECT 1"
        );
        assert_eq!(
            as_explain("explain analyze select 1", false),
            "EXPLAIN\nselect 1"
        );
        assert_eq!(
            as_explain("  EXPLAIN VERBOSE select 1", false),
            "EXPLAIN\nselect 1"
        );
        // Don't strip an identifier that merely starts with "explain".
        assert_eq!(
            as_explain("SELECT * FROM explain_t", false),
            "EXPLAIN\nSELECT * FROM explain_t"
        );
    }

    #[test]
    fn splits_name_and_detail() {
        let (n, d) = split_name_detail("ParquetExec: file_groups={…}, projection=[a, b]");
        assert_eq!(n, "ParquetExec");
        assert_eq!(d, "file_groups={…}, projection=[a, b]");
        let (n, d) = split_name_detail("CoalesceBatchesExec");
        assert_eq!(n, "CoalesceBatchesExec");
        assert_eq!(d, "");
    }
}
