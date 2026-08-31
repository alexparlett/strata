//! Writing a name **into** SQL — four rules, four types.
//!
//! There is no one way to render an identifier, and the four rules this crate needs differ in
//! ways that are silently wrong when confused: one folds, one preserves, one quotes
//! unconditionally, and one belongs to a server. They were four free functions and the only thing
//! stopping a caller reaching for the nearest was a doc comment; they are four newtypes now, so
//! the helper that composes a statement names the rule it needs in its signature and the wrong
//! one does not type-check.
//!
//! - [`WorkspaceName`] — *fold-preserving*: renders a name so DataFusion resolves it to
//!   [`fold_ident(name)`](crate::fold_ident), which is the identity a workspace def has actually
//!   been registered under. It lower-cases `DailySales` on purpose.
//! - [`SessionName`] — *case-preserving*: the name survives this session's parser exactly as
//!   spelled, which is what a relation whose spelling belongs to a **server** needs, and what a
//!   completion inserts so the row the user picked is the row the statement reaches.
//! - [`ResultColumn`] — *quoted unconditionally*: a result column's name is whatever the user's
//!   query produced, and no fold, keyword table or bare-word test applies to it.
//!
//! The fourth rule is a source's own —
//! [`ServerIdent`](crate::sources::source::ServerIdent), for an identifier the engine composes
//! into a statement bound for a server, spelled by
//! [`SourceCatalog::server_ident`](crate::sources::source::SourceCatalog::server_ident). It is
//! the same shape as these and lives beside the trait that mints it rather than here, because
//! `sources` and `sql` are peers inside this crate and neither may import the other
//! (`boundaries.rs` fails the build on it).
//!
//! Each type is a *rendered* fragment: minting one applies the rule, and [`Display`] is what a
//! statement says. What a message prints is the same string — a refusal that named a relation one
//! way while the statement reached it another would be the drift these types exist to end.

use std::fmt::{self, Display, Formatter};

use crate::fold_ident;
use crate::sql::context::{LITERAL_WORDS, OPERAND_EXPECTING};
use crate::sql::lex::is_reserved_in_name_position;

/// Whether an identifier must be double-quoted to survive DataFusion's parser *and mean the
/// name*: anything that isn't a plain lowercase `[a-z_][a-z0-9_]*` word, or that collides with a
/// reserved keyword (`order`), **or** with the expression grammar's own vocabulary — a column
/// named `null` inserted bare selects the literal (silently wrong data), one named `case` breaks
/// the parse. The collision set is the union of every table the model already declares:
/// parser-reserved ∪ [`OPERAND_EXPECTING`] ∪ [`LITERAL_WORDS`]. Merely-known keywords outside
/// those — `name`, `status`, `plain` — stay unquoted.
fn needs_quoting(name: &str) -> bool {
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

/// `name` in double quotes, embedded quotes doubled — SQL's own escape, shared by the rules here
/// that quote. The fourth rule spells it again for itself, the peer boundary being what it is.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// A **workspace def's** name, rendered into a statement.
///
/// DataFusion lower-cases an unquoted identifier and takes a quoted one verbatim, so a view named
/// `DailySales` has been registering as `dailysales` all along; emitting `"DailySales"` would
/// re-key it and break every sibling def that says `FROM dailysales`. So a name that already
/// worked keeps its exact old identity — nothing sayable bare is quoted, and the fold runs here
/// rather than in the parser, which also makes the identity independent of
/// `datafusion.sql_parser.enable_ident_normalization`.
///
/// Quoting is therefore never a re-keying, only a capability gain, and it fires in two cases:
/// names that were genuinely broken (`Sales 2024`, `2024`, `sales-eu`) where nothing was ever
/// registered to preserve, and reserved words defensively — `Order` folds to `"order"` first, the
/// same identity the unquoted spelling had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceName(String);

impl WorkspaceName {
    /// The name a workspace def called `name` answers to, as a statement may say it.
    pub fn of(name: &str) -> Self {
        let id = fold_ident(name);
        let mut rest = id.chars();
        let bare = matches!(rest.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
            && rest.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            && !is_reserved_in_name_position(&id);
        WorkspaceName(if bare { id } else { quoted(&id) })
    }

    /// The rendered name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A name whose **spelling** this session must preserve: a relation a server names, a symbol the
/// completion popup offered, a config key the user picked.
///
/// Quoted only where `needs_quoting` says it must be, so a plain word stays plain and reads
/// like what the user typed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionName(String);

impl SessionName {
    /// `name`, spelling preserved.
    pub fn of(name: &str) -> Self {
        SessionName(match needs_quoting(name) {
            true => quoted(name),
            false => name.to_string(),
        })
    }

    /// A dotted name, each segment rendered by [`of`](Self::of) — `pg.public."Orders"`.
    ///
    /// Segment by segment, never over the joined string: `pg.public.orders` as one name is not a
    /// plain lowercase word, so quoting it whole would produce `"pg.public.orders"`, which
    /// DataFusion reads as a *bare* relation with dots in it.
    pub fn qualified<'a>(parts: impl IntoIterator<Item = &'a str>) -> Self {
        SessionName(
            parts
                .into_iter()
                .map(|part| SessionName::of(part).0)
                .collect::<Vec<_>>()
                .join("."),
        )
    }

    /// The rendered name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A **result column**'s name rendered into SQL: double-quoted verbatim, embedded quotes doubled.
///
/// Deliberately neither of the two above. [`WorkspaceName`] folds a bare word to lowercase, which
/// is right for a catalog name (that fold is its registered identity) and wrong for a result
/// column, whose name is exactly what the user's query produced; [`SessionName`] would leave a
/// plain word bare, which is right for a name the catalog holds and needlessly fragile for one a
/// projection invented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResultColumn(String);

impl ResultColumn {
    /// `name`, quoted verbatim.
    pub fn of(name: impl AsRef<str>) -> Self {
        ResultColumn(quoted(name.as_ref()))
    }

    /// The rendered name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! renders {
    ($($t:ty),*) => {$(
        impl Display for $t {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<$t> for String {
            fn from(name: $t) -> String {
                name.0
            }
        }
    )*};
}

renders!(WorkspaceName, SessionName, ResultColumn);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workspace_name_renders_the_identity_the_def_registered_under() {
        assert_eq!(WorkspaceName::of("orders").as_str(), "orders");
        assert_eq!(WorkspaceName::of("DailySales").as_str(), "dailysales");
        assert_eq!(WorkspaceName::of("ORDERS").as_str(), "orders");
    }

    #[test]
    fn a_workspace_name_quotes_what_was_never_sayable_bare() {
        assert_eq!(WorkspaceName::of("Sales 2024").as_str(), "\"Sales 2024\"");
        assert_eq!(WorkspaceName::of("2024").as_str(), "\"2024\"");
        assert_eq!(WorkspaceName::of("sales-eu").as_str(), "\"sales-eu\"");
        assert_eq!(WorkspaceName::of("Order").as_str(), "\"order\"");
    }

    #[test]
    fn a_session_name_keeps_the_case_the_folding_renderer_would_lose() {
        assert_eq!(SessionName::of("orders").as_str(), "orders");
        assert_eq!(SessionName::of("Orders").as_str(), "\"Orders\"");
        assert_eq!(SessionName::of("DailySales").as_str(), "\"DailySales\"");
    }

    #[test]
    fn a_session_name_quotes_a_reserved_or_grammar_word() {
        assert_eq!(SessionName::of("order").as_str(), "\"order\"");
        assert_eq!(SessionName::of("null").as_str(), "\"null\"");
        assert_eq!(SessionName::of("status").as_str(), "status");
    }

    #[test]
    fn an_embedded_quote_is_doubled_by_every_rule_that_quotes() {
        assert_eq!(SessionName::of("say \"hi\"").as_str(), "\"say \"\"hi\"\"\"");
        assert_eq!(
            ResultColumn::of("say \"hi\"").as_str(),
            "\"say \"\"hi\"\"\""
        );
    }

    #[test]
    fn a_qualified_name_is_quoted_per_segment() {
        assert_eq!(
            SessionName::qualified(["pg", "public", "Orders"]).as_str(),
            "pg.public.\"Orders\""
        );
        assert_eq!(
            SessionName::qualified(["pg", "sales eu", "order"]).as_str(),
            "pg.\"sales eu\".\"order\""
        );
    }

    #[test]
    fn a_result_column_is_quoted_whatever_it_spells() {
        assert_eq!(ResultColumn::of("total").as_str(), "\"total\"");
        assert_eq!(ResultColumn::of("Total Sales").as_str(), "\"Total Sales\"");
    }
}
