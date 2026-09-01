//! The `__snap_` namespace — the name a result snapshot is registered under, and the predicate
//! every fence over it asks.

use datafusion::sql::TableReference;
use strata_model::SnapshotId;

use crate::catalog_providers::in_workspace;

/// The prefix every result snapshot is registered under. Named here, next to the
/// only thing that mints one, because two other rules key off it: the statement
/// router refuses an intercepted statement that names a table with this prefix
/// (`statements::classify_stmt`), and the schema provider hides such tables from every
/// enumeration (`engine::providers`) — the naming rule and the hiding rule must not
/// be able to drift apart.
const SNAPSHOT_PREFIX: &str = "__snap_";

/// The table name `snapshot` is registered under.
pub fn snapshot_name(snapshot: SnapshotId) -> String {
    format!("{SNAPSHOT_PREFIX}{snapshot}")
}

/// Whether `name` is in the snapshot namespace — the one predicate the refusal and the
/// hiding both ask, so neither can answer differently from [`snapshot_name`].
///
/// Case-folded, because the one namespace is case-insensitive and `__SNAP_2` is the same
/// table — compared in place rather than through `to_ascii_lowercase`, because the router
/// runs this per identifier per statement on every keystroke and the whole answer is seven
/// bytes wide.
pub fn is_snapshot_name(name: &str) -> bool {
    name.get(..SNAPSHOT_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(SNAPSHOT_PREFIX))
}

/// Whether the *reference* `name` addresses the snapshot namespace — the prefix, **scoped to
/// the workspace catalog**, which is the only place a Run mints into.
///
/// The scoping is the DB workstream's correction and it lives here, beside the naming rule, for
/// the reason [`is_snapshot_name`] does: the refusal (`sql::validate`) and the hiding
/// (`engine::providers`) ask one question, so neither can decide on its own where the namespace
/// ends. Since a session holds a catalog per source, the prefix alone stopped being
/// the whole question: a server may perfectly well have a relation called `__snap_3`, where the
/// name is an ordinary one that reserves nothing, hides nothing and collides with nothing —
/// reading it is fine, and refusing it would fence off a table Strata does not own.
pub(crate) fn is_snapshot_ref(name: &TableReference) -> bool {
    in_workspace(name) && is_snapshot_name(name.table())
}
