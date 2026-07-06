//! Parse a raw engine/DataFusion error string into a structured [`QueryError`]
//! for the results-pane error view (S6).
//!
//! DataFusion surfaces failures as flat strings, e.g.
//! `"Error during planning: table 'foo' not found"` or a sqlparser message like
//! `"SQL error: ParserError(\"Expected: …, found: FROM at Line: 2, Column: 8\")"`.
//! We split off a friendly error class, pull out a `line:col` location when one
//! is present, build a one-line code frame with a caret under the offending
//! column, and attach a short hint for a few common cases. Everything is
//! best-effort: an unrecognised string still yields a usable message.

/// A parsed query error, shaped for the results-pane error banner + code frame.
#[derive(Clone, Debug, PartialEq)]
pub struct QueryError {
    /// Short error class shown in the banner, e.g. "Planning Error".
    pub etype: String,
    /// Optional `line L:C` location shown dimmed next to the type.
    pub loc: Option<String>,
    /// The human-readable message body.
    pub message: String,
    /// Optional source-line code frame with a caret under the offending column.
    pub frame: Option<CodeFrame>,
    /// Optional one-line hint under the message.
    pub hint: Option<String>,
}

/// A single-line excerpt of the query with a caret marking the error column.
#[derive(Clone, Debug, PartialEq)]
pub struct CodeFrame {
    pub line_no: usize,
    pub line_text: String,
    /// Spaces before the caret (`column - 1`).
    pub caret_pad: String,
    /// The caret itself (`^`, widened to the offending token when known).
    pub caret: String,
}

impl QueryError {
    /// Parse `raw` (the engine's error string) against `sql` (the query text the
    /// error came from, used to build the code frame).
    pub fn parse(raw: &str, sql: &str) -> Self {
        let raw = raw.trim();
        let (etype, message) = split_type(raw);

        let (loc, frame) = match extract_line_col(&message) {
            Some((line, col)) => {
                let token_len = extract_found_token(&message)
                    .map(|t| t.chars().count())
                    .unwrap_or(1)
                    .clamp(1, 40);
                let frame = sql.lines().nth(line.saturating_sub(1)).map(|lt| CodeFrame {
                    line_no: line,
                    line_text: lt.to_string(),
                    caret_pad: " ".repeat(col.saturating_sub(1)),
                    caret: "^".repeat(token_len),
                });
                (Some(format!("line {line}:{col}")), frame)
            }
            None => (None, None),
        };

        let clean = strip_loc_suffix(&message);
        let hint = hint_for(&etype, &clean);
        Self {
            etype,
            loc,
            message: clean,
            frame,
            hint,
        }
    }
}

/// Known DataFusion error prefixes → friendly display names.
const KNOWN: &[(&str, &str)] = &[
    ("Error during planning", "Planning Error"),
    ("Schema error", "Schema Error"),
    ("SQL error", "SQL Error"),
    ("Arrow error", "Arrow Error"),
    ("Parquet error", "Parquet Error"),
    ("Execution error", "Execution Error"),
    ("Optimizer rule", "Optimizer Error"),
    ("This feature is not implemented", "Not Implemented"),
    ("Invalid or Unsupported Configuration", "Config Error"),
    ("Object Store error", "Storage Error"),
    ("External error", "External Error"),
    ("Internal error", "Internal Error"),
    ("IO error", "IO Error"),
];

/// Split `raw` into (friendly type, message) using the known prefixes, falling
/// back to a generic "Query Error" for anything unrecognised.
fn split_type(raw: &str) -> (String, String) {
    for (pfx, nice) in KNOWN {
        if let Some(rest) = raw.strip_prefix(*pfx) {
            let rest = rest.trim_start_matches([':', ' ']).trim();
            // sqlparser wraps its message in `ParserError("…")`; unwrap it.
            let rest = unwrap_parser_error(rest);
            let msg = if rest.is_empty() {
                raw.to_string()
            } else {
                rest
            };
            return ((*nice).to_string(), msg);
        }
    }
    ("Query Error".to_string(), unwrap_parser_error(raw))
}

/// Strip a `ParserError("…")` / `SchemaError(…)` wrapper down to its inner text.
fn unwrap_parser_error(s: &str) -> String {
    let s = s.trim();
    for tag in ["ParserError(", "TokenizerError(", "RecursionLimitExceeded("] {
        if let Some(inner) = s.strip_prefix(tag).and_then(|r| r.strip_suffix(')')) {
            return inner.trim().trim_matches('"').to_string();
        }
    }
    s.to_string()
}

/// Pull a `Line: L, Column: C` location out of a message (case-insensitive).
fn extract_line_col(s: &str) -> Option<(usize, usize)> {
    let lower = s.to_ascii_lowercase();
    let line = num_after(&lower, "line:")?;
    let col = num_after(&lower, "column:")?;
    Some((line, col))
}

/// The first run of digits after `key` in `lower`, parsed as usize.
fn num_after(lower: &str, key: &str) -> Option<usize> {
    let i = lower.find(key)? + key.len();
    let digits: String = lower[i..]
        .chars()
        .skip_while(|c| c.is_whitespace())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// The token reported after `found:` in a sqlparser error (used to size the
/// caret). Reads up to `, ` or ` at Line` or end-of-string.
fn extract_found_token(s: &str) -> Option<String> {
    let lower = s.to_ascii_lowercase();
    let i = lower.find("found:")? + "found:".len();
    let rest = s[i..].trim_start();
    let rest_lower = rest.to_ascii_lowercase();
    let end = rest_lower
        .find(" at line")
        .or_else(|| rest.find(','))
        .unwrap_or(rest.len());
    let tok = rest[..end].trim().trim_matches(['"', '\'']);
    if tok.is_empty() {
        None
    } else {
        Some(tok.to_string())
    }
}

/// Drop a trailing ` at Line: L, Column: C` clause so the message reads cleanly
/// (the location is shown separately in the banner).
fn strip_loc_suffix(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    if let Some(i) = lower.rfind(" at line:") {
        return s[..i].trim_end().to_string();
    }
    s.trim().to_string()
}

/// A short, human hint for a handful of common failure shapes.
fn hint_for(etype: &str, msg: &str) -> Option<String> {
    let m = msg.to_ascii_lowercase();
    let missing = m.contains("not found") || m.contains("does not exist") || m.contains("no such");
    if m.contains("table") && missing {
        return Some(
            "This table isn't registered. Check the name against the catalog on the left, or add it as a source."
                .into(),
        );
    }
    if (m.contains("column") || m.contains("field")) && (missing || m.contains("no field")) {
        return Some(
            "Check the column name and the table's schema — identifiers are case-sensitive.".into(),
        );
    }
    if etype == "SQL Error" || m.contains("expected") {
        return Some(
            "There's a syntax error near the highlighted position — look for a missing keyword, comma, or quote."
                .into(),
        );
    }
    if etype == "Not Implemented" {
        return Some(
            "DataFusion doesn't support this yet. Try rewriting the query using a supported form."
                .into(),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planning_table_missing() {
        let e = QueryError::parse("Error during planning: table 'events' not found", "SELECT * FROM events");
        assert_eq!(e.etype, "Planning Error");
        assert!(e.message.contains("table 'events' not found"));
        assert!(e.loc.is_none());
        assert!(e.hint.is_some());
    }

    #[test]
    fn sql_parser_with_location_builds_frame() {
        let sql = "SELECT *\nFRM events";
        let raw = "SQL error: ParserError(\"Expected: an expression, found: events at Line: 2, Column: 5\")";
        let e = QueryError::parse(raw, sql);
        assert_eq!(e.etype, "SQL Error");
        assert_eq!(e.loc.as_deref(), Some("line 2:5"));
        let f = e.frame.expect("frame");
        assert_eq!(f.line_no, 2);
        assert_eq!(f.line_text, "FRM events");
        assert_eq!(f.caret_pad.len(), 4);
        assert_eq!(f.caret, "^".repeat("events".len()));
        assert!(!e.message.contains("at Line"));
    }

    #[test]
    fn unknown_prefix_is_generic() {
        let e = QueryError::parse("something weird happened", "SELECT 1");
        assert_eq!(e.etype, "Query Error");
        assert_eq!(e.message, "something weird happened");
    }
}
