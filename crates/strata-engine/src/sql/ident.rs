//! Writing a name **into** a statement so DataFusion resolves it to exactly that name.
//!
//! This is the completion insert's own rule, lifted out of [`complete`](super::complete) so more
//! than one surface can apply it: the popup inserts through it, and the data-sources tree's
//! gestures compose their `FROM` through it (DB-06).
//!
//! **There are two identifier renderers in this crate and they are not interchangeable.**
//! [`quote_ident`](crate::quote_ident) is *fold-preserving*: it renders a name so that
//! DataFusion resolves it to [`fold_ident(name)`](crate::fold_ident), which is the
//! identity a workspace def has actually been registered under — so it lower-cases `DailySales`
//! on purpose, and a name a def already answered to keeps answering to it. [`quote_verbatim`] is
//! *case-preserving*: the name survives the parser exactly as spelled, which is what a relation
//! whose spelling belongs to a **server** needs. Pick by whose identity the name is; a third
//! rule (`export::quote_col`, which quotes unconditionally) exists for a third reason and is
//! neither of these.

use crate::sql::context::{LITERAL_WORDS, OPERAND_EXPECTING};
use crate::sql::lex::is_reserved_in_name_position;

/// Whether an identifier must be double-quoted to survive DataFusion's parser *and mean the
/// name*: anything that isn't a plain lowercase `[a-z_][a-z0-9_]*` word, or that collides with a
/// reserved keyword (`order`), **or** with the expression grammar's own vocabulary — a column
/// named `null` inserted bare selects the literal (silently wrong data), one named `case` breaks
/// the parse. The collision set is the union of every table the model already declares:
/// parser-reserved ∪ [`OPERAND_EXPECTING`] ∪ [`LITERAL_WORDS`]. Merely-known keywords outside
/// those — `name`, `status`, `plain` — stay unquoted.
pub fn needs_quoting(name: &str) -> bool {
    let plain = {
        let mut chars = name.chars();
        matches!(chars.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    !plain
        || is_reserved_in_name_position(name)
        || OPERAND_EXPECTING
            .iter()
            .any(|w| w.eq_ignore_ascii_case(name))
        || LITERAL_WORDS.iter().any(|w| w.eq_ignore_ascii_case(name))
}

/// `name` as a statement may say it, quoted only where [`needs_quoting`] says it must be (any
/// embedded `"` doubled). The name is preserved exactly — see the module docs for the other
/// renderer and when it is the right one.
pub fn quote_verbatim(name: &str) -> String {
    match needs_quoting(name) {
        true => format!("\"{}\"", name.replace('"', "\"\"")),
        false => name.to_string(),
    }
}

/// A dotted name, each segment rendered by [`quote_verbatim`] — `pg.public."Orders"`.
///
/// Segment by segment, never over the joined string: `pg.public.orders` as one name is not a
/// plain lowercase word, so quoting it whole would produce `"pg.public.orders"`, which DataFusion
/// reads as a *bare* relation with dots in it.
pub fn qualified<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts
        .into_iter()
        .map(quote_verbatim)
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_plain_lowercase_word_is_written_bare() {
        assert_eq!(quote_verbatim("orders"), "orders");
        assert_eq!(quote_verbatim("order_items_2024"), "order_items_2024");
    }

    #[test]
    fn case_survives_where_the_folding_renderer_would_lose_it() {
        assert_eq!(quote_verbatim("Orders"), "\"Orders\"");
        assert_eq!(quote_verbatim("DailySales"), "\"DailySales\"");
    }

    #[test]
    fn a_reserved_or_grammar_word_is_quoted() {
        assert_eq!(quote_verbatim("order"), "\"order\"");
        assert_eq!(quote_verbatim("null"), "\"null\"");
        assert_eq!(quote_verbatim("status"), "status");
    }

    #[test]
    fn an_embedded_quote_is_doubled() {
        assert_eq!(quote_verbatim("say \"hi\""), "\"say \"\"hi\"\"\"");
    }

    #[test]
    fn a_qualified_name_is_quoted_per_segment() {
        assert_eq!(
            qualified(["pg", "public", "Orders"]),
            "pg.public.\"Orders\""
        );
        assert_eq!(
            qualified(["pg", "sales eu", "order"]),
            "pg.\"sales eu\".\"order\""
        );
    }
}
