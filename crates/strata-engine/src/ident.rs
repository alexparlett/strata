//! The **engine identity** of a catalog name.
//!
//! One function, below every layer that needs it: `sql` renders names against it
//! ([`WorkspaceName`](crate::sql::WorkspaceName)), `sources` keys its listings by it, and the
//! arms compare names with it. It is not in `sql::name` beside the rendering rules because
//! `sources` and `sql` are peers inside this crate and neither may import the other
//! (`boundaries.rs` fails the build on it).

use datafusion::sql::TableReference;

/// The **engine identity** of a catalog name: the string DataFusion ends up keying the
/// object under, once [`WorkspaceName`](crate::sql::WorkspaceName) has rendered `name` into a statement.
///
/// It is not a re-derivation of DataFusion's rules — it *asks* `TableReference::parse_str`,
/// the very function `ctx.register_table(&str)` and `ctx.table(&str)` resolve a plain
/// `&str` through. So a view created via [`WorkspaceName`](crate::sql::WorkspaceName) and a table registered from the
/// same def name land on the same identity by construction: a single bare word folds to
/// ASCII-lowercase (`MyView` → `myview`, `Order` → `order`), and anything the parser can't
/// read as one identifier — a space, a hyphen, a leading digit, a stray quote — is the
/// name verbatim.
///
/// A dotted name parses as *qualified*, which we deliberately don't honour: the engine owns
/// one schema and a catalog name is an opaque label, so `a.b` is the literal name `a.b`.
/// (Nothing regresses: `register_table("a.b")` resolves to schema `a`, which doesn't exist,
/// so such a table never registered either.)
/// `pub` because the empty-table panel asks the same question of its column rows: two
/// rows collide exactly when the create arm's own fold says they do, and a form approximating
/// that with a case-insensitive compare would refuse pairs the engine accepts.
pub fn fold_ident(name: &str) -> String {
    match TableReference::parse_str(name) {
        TableReference::Bare { table } => table.to_string(),
        _ => name.to_string(),
    }
}
