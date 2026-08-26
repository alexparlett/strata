//! Where a statement's target is — the axis every managing arm opens on.
//!
//! One resolution in front of the arms, so the question "whose catalog is this name in" is asked
//! once and answered once. The workspace catalog has exactly one schema, so a qualified name is a
//! longer spelling of the same place, a relation inside a database connection's catalog, or
//! nowhere at all — and registration takes a bare name, so an unrecognised qualifier would
//! otherwise be dropped and the object created somewhere else.
//!
//! [`Locality`] is the axis [`crate::policy`] grants over, which is what lets the fine-phase check
//! be *derived* from the resolved target rather than restated by each arm
//! (`StmtCtx::require_target`).

use datafusion::prelude::SessionContext;
use datafusion::sql::sqlparser::ast::ObjectName;
use datafusion::sql::TableReference;

use crate::policy::Locality;
use crate::providers::in_workspace;
use crate::sql::qualified;
use crate::{fold_ident, CATALOG, SCHEMA};

/// What a name a statement manages resolves to.
///
/// Total: every name is one of these three, so an arm matches wildcard-free and a case it has no
/// answer for is a compile error rather than a silent fallthrough.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// The workspace catalog's one schema, under the bare name registration takes.
    Workspace { name: String },
    /// A relation inside a live database connection's catalog.
    Remote(Remote),
    /// A qualifier that resolves to no catalog at all — [`elsewhere`]'s wording.
    Nowhere { qualifier: String },
}

impl Target {
    /// Where this target lives, or `None` for a name that resolves nowhere.
    ///
    /// The link to [`crate::policy`]: a grant is held per locality, so this is what the fine
    /// phase derives its question from. [`Nowhere`](Target::Nowhere) has no locality because
    /// there is nothing there to hold a grant over.
    pub fn locality(&self) -> Option<Locality> {
        match self {
            Target::Workspace { .. } => Some(Locality::Local),
            Target::Remote(_) => Some(Locality::Remote),
            Target::Nowhere { .. } => None,
        }
    }

    /// The bare name registration takes, for an arm that acts on the workspace and nothing else
    /// — the other two answers being its refusals, worded once here rather than per arm.
    ///
    /// `what` is the plural noun that arm creates (`"Tables"`, `"Views"`), which is the only part
    /// of either sentence an arm supplies.
    ///
    /// # Errors
    ///
    /// The name is a relation inside a database connection, or a qualifier that resolves to no
    /// catalog at all.
    pub fn workspace(self, what: &str) -> Result<String, String> {
        match self {
            Target::Workspace { name } => Ok(name),
            Target::Remote(at) => Err(in_database(&at.address(), &at.connection)),
            Target::Nowhere { .. } => Err(elsewhere(what)),
        }
    }
}

/// One relation inside a database connection.
///
/// `connection` is the catalog name in the spelling it was registered under — the registered
/// spelling rather than the folded key, because that is what the connection is called everywhere
/// else the user meets it.
///
/// `reference` is the **recorded** form: the resolved [`TableReference`] itself, which is what a
/// plan's `TableScan` holds and therefore what `PlanDeps::remote` records. Held rather than a
/// rendered string, because *a rendered spelling is never a lookup key* — matching
/// `pg.public."Orders"`'s quoted address against the plan's plain-dotted recording found nothing,
/// and a `DROP` then reported a destructive action as consequence-free.
#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub connection: String,
    pub reference: TableReference,
}

impl Remote {
    /// The namespace inside the connection, as the statement named it.
    ///
    /// [`resolve_target`] mints a `Remote` only from a three-part reference, so the fallback is
    /// unreachable through it; it is the workspace's one schema rather than a panic because a
    /// hand-built `Remote` is a fixture's, and a fixture is not worth a crash.
    pub fn schema(&self) -> &str {
        self.reference.schema().unwrap_or(SCHEMA)
    }

    /// The relation, as the statement named it.
    pub fn table(&self) -> &str {
        self.reference.table()
    }

    /// The address a message prints — [`qualified`], because these three parts are a source's
    /// spelling and quoting them whole would name one relation with dots in it.
    ///
    /// **Output only.** Never a lookup key: see [`recorded`](Self::recorded).
    pub fn address(&self) -> String {
        qualified([self.connection.as_str(), self.schema(), self.table()])
    }

    /// The reference as everything that *resolves* one holds it — what a match against a
    /// recorded name compares.
    pub fn recorded(&self) -> &TableReference {
        &self.reference
    }

    /// The address as the **source** knows it, `schema.relation` — what a report about a
    /// statement the server ran names, the catalog being Strata's word for the connection and
    /// already in that report's other half ("on 'pg'").
    pub fn server_address(&self) -> String {
        qualified([self.schema(), self.table()])
    }

    /// How the **source** is addressed: `schema.table`, never the catalog, which is Strata's own
    /// prefix for the connection and means nothing to the server. A full reference would render
    /// `"pg"."public"."orders"` into a statement the server then refuses.
    pub fn relation(&self) -> TableReference {
        TableReference::partial(self.schema().to_string(), self.table().to_string())
    }
}

/// What `name` addresses.
///
/// **The one choke point in front of every arm.** Every intercepted statement that manages a
/// target comes through here, so one sentence covers them all and no arm grows its own copy of
/// the check. The catalog list is asked rather than a list of connections, because it is what
/// *resolves* the name: a catalog is registered exactly while its connection is live, which is
/// the window in which the user can address it.
///
/// A pure function of the session, deliberately: the editor asks it of a statement it is only
/// judging (`arms::remote::dispatched`), where no dispatch state exists. The
/// connection's own gates — is it writable, may this caller reach it — are asked of the resolved
/// answer, not folded into it.
pub fn resolve_target(ctx: &SessionContext, name: &TableReference) -> Target {
    if in_workspace(name) {
        return Target::Workspace {
            name: name.table().to_string(),
        };
    }
    let TableReference::Full { catalog, .. } = name else {
        return Target::Nowhere {
            qualifier: name.to_string(),
        };
    };
    match source_catalog(ctx, catalog) {
        Some(connection) => Target::Remote(Remote {
            connection,
            reference: name.clone(),
        }),
        None => Target::Nowhere {
            qualifier: name.to_string(),
        },
    }
}

/// [`resolve_target`] off a **parsed** name, for the arms that must answer before anything plans:
/// `CREATE TABLE pg.public.t (payload jsonb)` names a type DataFusion has no Arrow mapping for,
/// so planning it to find its target would refuse the statement first.
///
/// A name of more than three parts addresses nothing a [`TableReference`] can hold, and is
/// [`Nowhere`](Target::Nowhere) under its own spelling.
pub fn resolve_named(ctx: &SessionContext, name: &ObjectName) -> Target {
    match name.0.len() <= 3 {
        true => resolve_target(ctx, &TableReference::parse_str(&name.to_string())),
        false => Target::Nowhere {
            qualifier: name.to_string(),
        },
    }
}

/// The database connection's catalog `catalog` names, in the spelling it was registered under —
/// `None` for the workspace catalog, and for a qualifier that resolves to nothing.
///
/// **Folded on both sides, the workspace's own name included.** The catalog list resolves by
/// [`fold_ident`], so a quoted `"STRATA"` names the workspace catalog — and compared raw it
/// would slip past the guard below and then *match* the workspace's own entry in the search,
/// telling the user their project's catalog is a connection. No real connection can produce that
/// (`check_catalog` refuses `strata` case-insensitively), so the sentence would name a connection
/// that cannot exist.
fn source_catalog(ctx: &SessionContext, catalog: &str) -> Option<String> {
    let folded = fold_ident(catalog);
    if folded == CATALOG {
        return None;
    }
    ctx.catalog_names()
        .into_iter()
        .find(|registered| fold_ident(registered) == folded)
}

/// The wording for a statement that will **not** touch a name inside a database connection's
/// catalog — registering a table externally, which declares files and a format for a relation the
/// server already describes itself, and the view and drop statements the server owns instead.
pub fn in_database(name: &str, catalog: &str) -> String {
    format!(
        "'{name}' is in the database connection '{catalog}', which describes its own relations. \
         Tables cannot be registered inside one"
    )
}

/// The wording for a write into a connection that has not been opted in — **minted once**, beside
/// [`in_database`], because both arms that can reach it must say the same thing.
///
/// It names the setting rather than the rule: a connection is read-only by default, so the
/// user is one toggle away and the sentence is only useful if it says which.
pub fn read_only(at: &Remote) -> String {
    format!(
        "The database connection '{}' is read-only, so '{}' cannot be written. Turn off 'Read \
         only' in the connection's settings",
        at.connection,
        at.address()
    )
}

/// The wording for a name that points outside Strata's single schema — held apart from
/// [`resolve_target`] because a caller that parses the name itself has to be able to refuse the
/// forms a `TableReference` cannot even represent, in the same words (`views::definition`).
pub fn elsewhere(what: &str) -> String {
    format!("Strata has one schema, '{SCHEMA}'. {what} cannot be created elsewhere")
}

#[cfg(test)]
mod tests {
    use datafusion::prelude::SessionContext;

    use crate::providers::fake_source;

    use super::*;

    /// A session holding the workspace catalog and one database connection called `pg`.
    fn session() -> SessionContext {
        let ctx = crate::builder::test_context(&std::collections::BTreeMap::new());
        fake_source(&ctx, "pg", &["orders"]);
        ctx
    }

    fn resolved(ctx: &SessionContext, name: &str) -> Target {
        resolve_target(ctx, &TableReference::parse_str(name))
    }

    /// **The three spellings of the workspace's one schema are one answer**, and the qualified
    /// one is folded on the catalog and exact on the schema — the way the things that resolve
    /// them compare.
    #[test]
    fn every_spelling_of_the_workspace_resolves_to_the_workspace() {
        let ctx = session();
        for name in [
            "orders",
            "public.orders",
            "strata.public.orders",
            "\"STRATA\".public.orders",
        ] {
            assert_eq!(
                resolved(&ctx, name),
                Target::Workspace {
                    name: "orders".into()
                },
                "'{name}'"
            );
        }
    }

    /// A relation inside a connection resolves to it under the connection's **registered**
    /// spelling, whatever case the statement wrote — that being what every other surface calls it.
    #[test]
    fn a_connections_relation_resolves_under_its_registered_spelling() {
        let ctx = session();
        let Target::Remote(at) = resolved(&ctx, "PG.public.orders") else {
            panic!("'PG.public.orders' did not resolve to the connection");
        };
        assert_eq!(at.connection, "pg");
        assert_eq!(at.recorded().to_string(), "pg.public.orders");
    }

    /// A qualifier naming no catalog, and a schema the workspace catalog cannot have, are both
    /// nowhere — one answer, because there is nothing at either address.
    #[test]
    fn a_qualifier_that_resolves_to_nothing_is_nowhere() {
        let ctx = session();
        for name in [
            "nosuch.public.orders",
            "elsewhere.orders",
            "strata.other.orders",
        ] {
            assert!(
                matches!(resolved(&ctx, name), Target::Nowhere { .. }),
                "'{name}'"
            );
        }
    }

    /// **The three answers a workspace-only arm gets**, each with the sentence it is refused by —
    /// `bare_name`'s whole contract, in one wildcard-free match.
    #[test]
    fn the_workspace_only_answer_carries_all_three_wordings() {
        let ctx = session();
        assert_eq!(
            resolved(&ctx, "orders").workspace("Tables"),
            Ok("orders".to_string())
        );
        assert_eq!(
            resolved(&ctx, "pg.public.orders").workspace("Tables"),
            Err(in_database("pg.public.orders", "pg"))
        );
        assert_eq!(
            resolved(&ctx, "nosuch.public.orders").workspace("Views"),
            Err(elsewhere("Views"))
        );
    }

    /// The link to [`crate::policy`]: a target's locality is what the fine phase derives its
    /// question from, and a name that resolves nowhere has none.
    #[test]
    fn locality_is_the_axis_grants_are_held_over() {
        let ctx = session();
        assert_eq!(resolved(&ctx, "orders").locality(), Some(Locality::Local));
        assert_eq!(
            resolved(&ctx, "pg.public.orders").locality(),
            Some(Locality::Remote)
        );
        assert_eq!(resolved(&ctx, "nosuch.public.orders").locality(), None);
    }

    /// **A rendered spelling is never a lookup key.** The address a message prints quotes the
    /// parts that need it; the recorded reference is what a plan holds and therefore the only
    /// thing a match against `PlanDeps::remote` can compare — for a quoted identifier and for a
    /// reserved word alike.
    #[test]
    fn the_recorded_form_is_what_a_dependent_lookup_matches() {
        let ctx = session();
        fake_source(&ctx, "quoted", &["Orders", "order"]);
        for (relation, printed) in [("Orders", "\"Orders\""), ("order", "\"order\"")] {
            let Target::Remote(at) = resolved(&ctx, &format!("quoted.public.\"{relation}\""))
            else {
                panic!("'{relation}' did not resolve to the connection");
            };
            assert_eq!(
                at.recorded().to_string(),
                format!("quoted.public.{relation}")
            );
            assert_eq!(at.address(), format!("quoted.public.{printed}"));
            assert_eq!(at.server_address(), format!("public.{printed}"));
        }
    }
}
