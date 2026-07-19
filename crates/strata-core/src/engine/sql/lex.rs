//! Tokenise SQL using **DataFusion's own `sqlparser`** (via the `datafusion::sql`
//! re-export, so dialect + version match the engine). We map its tokens into a small
//! [`Tok`] model with **byte-offset spans** — insulating the rest of `crate::sql`
//! from sqlparser's exact types and giving squiggle ranges + token-under-caret.
//!
//! sqlparser reports positions as 1-based line/column (character columns). We convert
//! to byte offsets against a line-start index; for ASCII SQL (the common case) columns
//! are bytes, so this is exact — non-ASCII columns are approximate (acceptable v1).

use std::ops::Range;

use datafusion::sql::sqlparser::dialect::{Dialect, GenericDialect};
use datafusion::sql::sqlparser::keywords::Keyword;
use datafusion::sql::sqlparser::tokenizer::{Token, Tokenizer};

/// Whether `ch` continues a SQL identifier/word, per DataFusion's parser dialect
/// (`GenericDialect` — its default; the tokeniser below uses the same one). Used for
/// completion's word-boundary + dismiss logic so it matches the parser's notion of a
/// word rather than a hardcoded character set.
pub fn is_word_char(ch: char) -> bool {
    GenericDialect {}.is_identifier_part(ch)
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

/// Tokenise `sql`, dropping whitespace/comments. On a tokenizer error returns the
/// tokens gathered so far plus the error (so completion/validation degrade instead of
/// bailing on mid-edit text).
pub fn lex(sql: &str) -> (Vec<Tok>, Option<LexError>) {
    let starts = line_starts(sql);
    let dialect = GenericDialect {};
    let mut tokenizer = Tokenizer::new(&dialect, sql);
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
            // sqlparser's TokenizerError carries a message + a location.
            let at = offset(&starts, e.location);
            (
                Vec::new(),
                Some(LexError {
                    message: e.message,
                    span: at..at.saturating_add(1),
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

fn offset(starts: &[usize], loc: datafusion::sql::sqlparser::tokenizer::Location) -> usize {
    let line = (loc.line.max(1) - 1) as usize;
    let base = starts.get(line).copied().unwrap_or(0);
    base + (loc.column.max(1) - 1) as usize
}
