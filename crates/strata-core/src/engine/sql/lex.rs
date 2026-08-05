//! Tokenise SQL using **DataFusion's own `sqlparser`** (via the `datafusion::sql`
//! re-export, so dialect + version match the engine). We map its tokens into a small
//! [`Tok`] model with **byte-offset spans** — insulating the rest of `crate::sql`
//! from sqlparser's exact types and giving squiggle ranges + token-under-caret.
//!
//! The dialect is an **argument**, never a constant here: `datafusion.sql_parser.dialect`
//! is a key the user can set (W2), and the dialects disagree about tokenizing, not just
//! about grammar — under `postgresql` every operator character is a custom-operator part,
//! so `a>-1` lexes as one `>-` token, while `#`/`@` stop being identifier characters.
//! A hardcoded dialect here would put autocomplete and the squiggles on different rules
//! from the planner they are supposed to agree with (WJ-04).
//!
//! sqlparser reports positions as 1-based line/column (character columns). We convert
//! to byte offsets against a line-start index; for ASCII SQL (the common case) columns
//! are bytes, so this is exact — non-ASCII columns are approximate (acceptable v1).

use std::ops::Range;

use datafusion::sql::sqlparser::dialect::{dialect_from_str, Dialect, GenericDialect};
use datafusion::sql::sqlparser::keywords::Keyword;
use datafusion::sql::sqlparser::tokenizer::{Location, Token, Tokenizer};

/// Resolve the dialect `name` — the engine's own `datafusion.sql_parser.dialect` — through
/// sqlparser's own `dialect_from_str`, which is the same resolution DataFusion's planner
/// performs, so the two cannot drift.
///
/// A name sqlparser doesn't know falls back to `generic` rather than failing. That is not a
/// silent fallback: DataFusion's `ConfigOptions` parses this key into a typed `Dialect` and
/// **rejects** an unknown name at apply time, and [`policy_verdicts`](super::policy_verdicts)
/// refuses outright on one, so the fault is already being said out loud. Degrading here keeps
/// the editor lexing well enough to *show* the message rather than going blank alongside it.
pub(crate) fn dialect(name: &str) -> Box<dyn Dialect> {
    dialect_from_str(name).unwrap_or_else(|| Box::new(GenericDialect {}))
}

/// Whether `word` is **reserved in name positions** (terminates a table/column-alias
/// slot) per DataFusion's own parser tables — the authoritative "can this be an
/// identifier here", shared by the context scanner's name captures and completion's
/// identifier quoting. Everything else sqlparser merely *knows* as a keyword
/// (`name`, `status`, `type`, …) is a perfectly good identifier.
pub(crate) fn is_reserved_in_name_position(word: &str) -> bool {
    use datafusion::sql::sqlparser::keywords::{
        ALL_KEYWORDS, ALL_KEYWORDS_INDEX, RESERVED_FOR_COLUMN_ALIAS, RESERVED_FOR_TABLE_ALIAS,
    };
    match ALL_KEYWORDS.binary_search(&word.to_ascii_uppercase().as_str()) {
        Ok(i) => {
            let kw = ALL_KEYWORDS_INDEX[i];
            RESERVED_FOR_COLUMN_ALIAS.contains(&kw) || RESERVED_FOR_TABLE_ALIAS.contains(&kw)
        }
        Err(_) => false,
    }
}

/// Whether the caret sits inside a string literal or comment — including regions left
/// **unterminated** at end-of-input (an open `'…` fails the whole tokenize, so the
/// token stream can't answer this; comments are dropped by [`lex`] entirely). One
/// linear scan: `'…'` strings with `''` escapes, `--` line comments, `/* … */` block
/// comments (treated as non-nesting — the outermost `*/` ends the region, which only
/// under-suppresses). `"quoted idents"` are skipped as
/// opaque regions (they may contain `--` etc.) but do **not** count as inside —
/// completion may legitimately fire there.
pub fn caret_in_string_or_comment(sql: &str, caret: usize) -> bool {
    let b = sql.as_bytes();
    let mut i = 0usize;
    while i < b.len() && i < caret {
        match b[i] {
            b'\'' => {
                let start = i;
                i += 1;
                let mut end = None; // byte after the closing quote
                while i < b.len() {
                    if b[i] == b'\'' {
                        if b.get(i + 1) == Some(&b'\'') {
                            i += 2; // '' escape
                        } else {
                            i += 1;
                            end = Some(i);
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
                match end {
                    Some(e) if caret < e => return caret > start,
                    Some(_) => {}
                    None => return caret > start, // unterminated → inside to EOF
                }
            }
            b'-' if b.get(i + 1) == Some(&b'-') => {
                let start = i;
                match sql[i..].find('\n') {
                    Some(n) => {
                        let end = i + n + 1; // first byte of the next line
                        if caret > start && caret < end {
                            return true;
                        }
                        i = end;
                    }
                    None => return caret > start, // comment runs to EOF
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                let start = i;
                let end = sql[i + 2..].find("*/").map(|n| i + 2 + n + 2);
                match end {
                    Some(e) if caret < e => return caret > start,
                    Some(e) => i = e,
                    None => return caret > start, // unterminated → inside to EOF
                }
            }
            b'"' => {
                // Opaque quoted-identifier region; skip (not a suppression zone).
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    i += 1;
                }
                i += 1; // past the closing quote (or EOF)
            }
            _ => i += 1,
        }
    }
    false
}

/// Whether the caret extends a numeric literal mid-shape — `1.|`, the dot absorbed
/// into the number token — a position where nothing can complete. Token-authoritative
/// (an ident's dot lexes as a separate `.` and is a qualifier); lives here beside the
/// string/comment scanner because it is the same kind of fact: "the caret is inside
/// a literal, stay quiet".
pub(crate) fn caret_extends_numeric_literal(toks: &[Tok], caret: usize) -> bool {
    toks.iter()
        .any(|t| t.kind == TokKind::Num && t.span.end == caret && t.text.ends_with('.'))
}

/// One lexical token with a byte-offset span into the source.
#[derive(Clone, Debug, PartialEq)]
pub struct Tok {
    pub kind: TokKind,
    /// Source text of the token (as written — case preserved).
    pub text: String,
    pub span: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokKind {
    Keyword,
    /// Bare identifier (unquoted, non-keyword word).
    Ident,
    /// `"quoted identifier"`.
    QuotedIdent,
    /// `'string literal'`.
    Str,
    Num,
    /// `,` `(` `)` `.` `;` and other single-char punctuation.
    Punct,
    /// An operator (`=`, `<`, `||`, `::`, …).
    Op,
    Other,
}

impl Tok {
    pub fn is(&self, kind: TokKind) -> bool {
        self.kind == kind
    }
    /// Case-insensitive text compare (for keyword / clause matching).
    pub fn eq_ci(&self, s: &str) -> bool {
        self.text.eq_ignore_ascii_case(s)
    }
}

/// A tokenisation failure (unterminated string, stray char) with its byte span.
pub struct LexError {
    pub message: String,
    pub span: Range<usize>,
}

// ---- statement split -------------------------------------------------------

/// Byte ranges partitioning `0..sql_len` on top-level `;` tokens — the one splitter
/// shared by validation (all statements) and the context analyzer (statement under
/// the caret). Token-level, so `;` inside strings never splits. Untrimmed: segments
/// may be empty or comment-only; callers filter to taste.
pub(crate) fn split_statements(toks: &[Tok], sql_len: usize) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for t in toks {
        if t.kind == TokKind::Punct && t.text == ";" {
            ranges.push(start..t.span.start);
            start = t.span.end;
        }
    }
    ranges.push(start..sql_len);
    ranges
}

/// The segment of [`split_statements`] containing `caret` (a caret on the `;` itself
/// belongs to the statement it terminates; one just after it starts the next).
pub(crate) fn statement_at(toks: &[Tok], sql_len: usize, caret: usize) -> Range<usize> {
    split_statements(toks, sql_len)
        .into_iter()
        .find(|r| caret <= r.end)
        .unwrap_or(0..sql_len) // unreachable: the final segment ends at sql_len
}

/// Byte offset of 1-based (`line`, `column`) within `slice` (clamped to its end).
/// Columns count characters; for ASCII SQL that's exact, non-ASCII is approximate —
/// the same convention as the token mapping above.
pub(crate) fn rel_offset(slice: &str, line: u64, column: u64) -> usize {
    let line = (line.max(1) - 1) as usize;
    let column = (column.max(1) - 1) as usize;
    let base = slice
        .split_inclusive('\n')
        .take(line)
        .map(|l| l.len())
        .sum::<usize>();
    (base + column).min(slice.len())
}

/// Tokenise `sql` under the engine's configured `dialect` (see [`dialect`]), dropping
/// whitespace/comments. On a tokenizer error returns the tokens gathered so far plus the
/// error (so completion/validation degrade instead of bailing on mid-edit text).
pub fn lex(sql: &str, dialect: &str) -> (Vec<Tok>, Option<LexError>) {
    let starts = line_starts(sql);
    let dialect = self::dialect(dialect);
    let mut tokenizer = Tokenizer::new(dialect.as_ref(), sql);
    match tokenizer.tokenize_with_location() {
        Ok(tokens) => (
            tokens
                .into_iter()
                .filter_map(|t| {
                    convert(
                        &starts,
                        sql,
                        t.token,
                        offset(&starts, t.span.start),
                        offset(&starts, t.span.end),
                    )
                })
                .collect(),
            None,
        ),
        Err(e) => {
            // sqlparser's TokenizerError carries a message + a location — which
            // can sit one past the end (unterminated string/comment at EOF), so
            // clamp the one-byte span inside the buffer.
            let end = offset(&starts, e.location).saturating_add(1).min(sql.len());
            (
                Vec::new(),
                Some(LexError {
                    message: e.message,
                    span: end.saturating_sub(1)..end,
                }),
            )
        }
    }
}

fn convert(_starts: &[usize], _sql: &str, token: Token, start: usize, end: usize) -> Option<Tok> {
    let span = start..end.max(start);
    let (kind, text) = match token {
        Token::Whitespace(_) => return None,
        Token::Word(w) => {
            let kind = if w.quote_style == Some('"') {
                TokKind::QuotedIdent
            } else if w.keyword != Keyword::NoKeyword {
                TokKind::Keyword
            } else {
                TokKind::Ident
            };
            (kind, w.value)
        }
        Token::Number(n, _) => (TokKind::Num, n),
        Token::SingleQuotedString(s)
        | Token::NationalStringLiteral(s)
        | Token::EscapedStringLiteral(s) => (TokKind::Str, s),
        Token::DoubleQuotedString(s) => (TokKind::QuotedIdent, s),
        Token::Comma => (TokKind::Punct, ",".into()),
        Token::LParen => (TokKind::Punct, "(".into()),
        Token::RParen => (TokKind::Punct, ")".into()),
        Token::Period => (TokKind::Punct, ".".into()),
        Token::SemiColon => (TokKind::Punct, ";".into()),
        Token::Colon => (TokKind::Punct, ":".into()),
        Token::DoubleColon => (TokKind::Op, "::".into()),
        Token::EOF => return None,
        // Everything else (operators, brackets, etc.) — keep its source rendering.
        other => (TokKind::Op, other.to_string()),
    };
    Some(Tok { kind, text, span })
}

fn line_starts(sql: &str) -> Vec<usize> {
    let mut v = vec![0usize];
    for (i, b) in sql.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

fn offset(starts: &[usize], loc: Location) -> usize {
    let line = (loc.line.max(1) - 1) as usize;
    let base = starts.get(line).copied().unwrap_or(0);
    base + (loc.column.max(1) - 1) as usize
}

#[cfg(test)]
mod tests {
    use super::caret_in_string_or_comment as guard;
    use super::lex;

    /// **The lexer follows the engine's dialect; it never picks one.** The dialects disagree
    /// at the *token* level, not only about grammar, so a hardcoded dialect here would put
    /// autocomplete and the squiggles on different rules from the planner (WJ-04). Two
    /// measured divergences pin it: `postgresql` treats every operator character as a
    /// custom-operator part, so `a>-1` lexes as one `>-` token, and it drops `#` from the
    /// identifier characters `generic` allows.
    #[test]
    fn the_dialect_comes_from_the_engine() {
        let text = |sql: &str, dialect: &str| {
            let (toks, err) = lex(sql, dialect);
            assert!(err.is_none(), "{sql} under {dialect}");
            toks.into_iter().map(|t| t.text).collect::<Vec<_>>()
        };

        assert_eq!(text("a>-1", "generic"), ["a", ">", "-", "1"]);
        assert_eq!(text("a>-1", "postgresql"), ["a", ">-", "1"]);
        assert_eq!(text("a#b", "generic"), ["a#b"]);
        assert_eq!(text("a#b", "postgresql"), ["a", "#", "b"]);
    }

    /// A dialect name sqlparser doesn't know lexes as `generic` rather than going blank —
    /// the planner is already refusing that config out loud, and an editor that stopped
    /// tokenising would hide the message instead of showing it.
    #[test]
    fn an_unknown_dialect_lexes_as_generic() {
        let (toks, err) = lex("SELECT a#b", "not-a-dialect");
        assert!(err.is_none());
        assert_eq!(
            toks.iter().map(|t| t.text.as_str()).collect::<Vec<_>>(),
            ["SELECT", "a#b"]
        );
    }

    /// Caret at the `|` marker.
    fn at(sql_with_caret: &str) -> bool {
        let caret = sql_with_caret.find('|').expect("caret marker");
        let sql = sql_with_caret.replace('|', "");
        guard(&sql, caret)
    }

    #[test]
    fn plain_code_is_not_suppressed() {
        assert!(!at("SELECT |a FROM t"));
        assert!(!at("|SELECT 'x'"));
    }

    #[test]
    fn inside_a_closed_string() {
        assert!(at("SELECT 'ab|c' FROM t"));
        assert!(at("SELECT 'abc|' FROM t")); // before the closing quote
        assert!(!at("SELECT 'abc'| FROM t")); // after the closing quote
        assert!(!at("SELECT |'abc' FROM t")); // before the opening quote
    }

    #[test]
    fn doubled_quote_escape_stays_inside() {
        assert!(at("SELECT 'ab''c|d' FROM t"));
        assert!(!at("SELECT 'ab''cd'| FROM t"));
    }

    #[test]
    fn unterminated_string_runs_to_eof() {
        assert!(at("SELECT 'ab|"));
        assert!(at("SELECT 'ab|c"));
    }

    #[test]
    fn line_comment() {
        assert!(at("SELECT a -- no|te"));
        assert!(at("SELECT a -- note|"));
        assert!(at("SELECT a --|"));
        assert!(!at("SELECT a |-- note"));
        // The next line is code again.
        assert!(!at("SELECT a -- note\n|FROM t"));
        assert!(!at("SELECT a -- note\nFROM |t"));
    }

    #[test]
    fn block_comment() {
        assert!(at("SELECT /* no|te */ a"));
        assert!(!at("SELECT /* note */ |a"));
        assert!(at("SELECT /* unterminated |"));
    }

    #[test]
    fn quoted_idents_are_opaque_but_not_suppressing() {
        // A `--` inside a quoted identifier is not a comment.
        assert!(!at("SELECT \"a -- b\", |c FROM t"));
        // And the caret inside a quoted identifier is a legit completion position.
        assert!(!at("SELECT \"my col|umn\" FROM t"));
    }
}
