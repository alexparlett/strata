//! How a statement reaches a relation that is **not** the workspace's.
//!
//! Every intercepted kind is one of three, keyed wildcard-free on [`StmtKind`] so a kind the
//! engine gains has to answer here before it compiles — and the answer decides whether a target
//! inside a database connection is something that kind can act on at all.

use crate::statements::StmtKind;

/// What a kind does with a target inside a database connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mechanism {
    /// DataFusion plans it and the arm drives the source's own sink — the input is an ordinary
    /// query, so a local scan, a federated remote one and a cross-source join all reach the
    /// server already working, and what is added is where the rows land.
    PlanIntoSink,
    /// Only the server can run it, so the statement the user typed is dispatched as text with
    /// the catalog qualifier cut out. Its types, its functions and its clauses are the server's
    /// vocabulary: a column-list `CREATE TABLE` names `serial` and `jsonb`, and a plan of one
    /// would refuse it before anything looked.
    ServerText,
    /// The kind acts on the workspace or on nothing at all. A remote target is refused by name
    /// ([`in_database`](super::target::in_database)) or is not a shape the kind can even carry.
    Refused,
}

/// How `kind` reaches a remote target.
///
/// Wildcard-free, so a new [`StmtKind`] is a compile error here rather than a statement that
/// silently inherits somebody else's mechanism — and inheriting [`Refused`](Mechanism::Refused)
/// by default would be the quiet half of that: a kind the server could run, refused for the life
/// of the release with nobody told.
pub fn mechanism(kind: StmtKind) -> Mechanism {
    match kind {
        StmtKind::Insert | StmtKind::Ctas => Mechanism::PlanIntoSink,
        StmtKind::CreateTable
        | StmtKind::CreateView
        | StmtKind::DropTable
        | StmtKind::DropView
        | StmtKind::Update
        | StmtKind::Delete => Mechanism::ServerText,
        StmtKind::CreateExternalTable
        | StmtKind::Copy
        | StmtKind::Set
        | StmtKind::Reset
        | StmtKind::Prepare
        | StmtKind::Deallocate
        | StmtKind::CreateFunction
        | StmtKind::DropFunction => Mechanism::Refused,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **A `CREATE TABLE` with a query is a different kind**, which is what lets the mechanism be
    /// keyed on the kind alone: a column list goes to the server as text because its types are
    /// the server's, and a CTAS plans into the sink because its input is an ordinary query.
    #[test]
    fn the_two_creates_take_different_mechanisms() {
        assert_eq!(mechanism(StmtKind::CreateTable), Mechanism::ServerText);
        assert_eq!(mechanism(StmtKind::Ctas), Mechanism::PlanIntoSink);
    }

    /// The statements that name no relation, and the one that names a remote relation only to
    /// refuse it, are the same answer for two reasons — stated together so a reader does not
    /// take `Refused` for "unimplemented".
    #[test]
    fn a_kind_with_no_remote_form_is_refused() {
        for kind in [
            StmtKind::CreateExternalTable,
            StmtKind::Copy,
            StmtKind::Set,
            StmtKind::Reset,
            StmtKind::Prepare,
            StmtKind::Deallocate,
            StmtKind::CreateFunction,
            StmtKind::DropFunction,
        ] {
            assert_eq!(mechanism(kind), Mechanism::Refused, "{kind:?}");
        }
    }
}
