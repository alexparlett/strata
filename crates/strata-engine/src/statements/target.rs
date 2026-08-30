//! Where a statement's target is — the axis every managing arm opens on.
//!
//! One resolution in front of the arms, so the question "whose catalog is this name in" is asked
//! once and answered once. The workspace catalog has exactly one schema, so a qualified name is a
//! longer spelling of the same place, a relation inside a data source's catalog, or
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
use crate::providers::{in_workspace, is_store_catalog};
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
    /// A table in a **store** data source's catalog (EA-25) — one of the project's own rows,
    /// whose data is files in a bucket.
    Store(Stored),
    /// A relation inside a live data source's catalog.
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
            // A bucket table is **file-backed and the project's own**, so it holds the local
            // grant it held when it lived in the workspace catalog. The axis is where the work
            // happens, not which catalog resolves the name: nothing about it is a server's.
            Target::Workspace { .. } | Target::Store(_) => Some(Locality::Local),
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
    /// The name is a relation inside a source, or a qualifier that resolves to no
    /// catalog at all.
    pub fn workspace(self, what: &str) -> Result<String, String> {
        match self {
            Target::Workspace { name } => Ok(name),
            Target::Store(at) => Err(in_store(&at.address(), &at.source, what)),
            Target::Remote(at) => Err(in_database(&at.address(), &at.source)),
            Target::Nowhere { .. } => Err(elsewhere(what)),
        }
    }

    /// The project's own name for a **def-backed** table — the workspace's, or one in a store
    /// data source's catalog.
    ///
    /// The answer for an arm that manages a table the project holds a row for, wherever that
    /// row's provider was placed: `DROP TABLE regions` names the same def whether it is written
    /// bare or as `lake.public.regions`, because a store catalog is where a def is *registered*
    /// and never a second namespace to be in. The arm resolves the name through
    /// [`def_ref`](crate::providers::def_ref) rather than being handed a catalog, so it needs no
    /// idea which of the two placements it got.
    ///
    /// Separate from [`workspace`](Self::workspace) rather than folded into it: that one is for
    /// arms that act on the workspace and **nothing else**, and a store catalog is one of the
    /// things they refuse.
    ///
    /// # Errors
    ///
    /// The name is a relation inside a database data source, or a qualifier that resolves to no
    /// catalog at all.
    pub fn def(self, what: &str) -> Result<String, String> {
        match self {
            Target::Workspace { name } => Ok(name),
            Target::Store(at) => Ok(at.name),
            Target::Remote(at) => Err(in_database(&at.address(), &at.source)),
            Target::Nowhere { .. } => Err(elsewhere(what)),
        }
    }
}

/// One table in a **store** data source's catalog: the data source it reads through, and the
/// project's own name for it.
///
/// `name` is bare on purpose — it is what the def carries, what every registration takes and what
/// a drop removes. The catalog is *placement*: it decides where the provider was put and what a
/// Forget takes away, and it is never a second namespace, because table names are unique across
/// the whole project.
#[derive(Clone, Debug, PartialEq)]
pub struct Stored {
    /// The data source's name, which is the catalog its tables are registered in.
    pub source: String,
    /// The project's own name for the table.
    pub name: String,
}

impl Stored {
    /// The address a message prints — [`qualified`], for the reason [`Remote::address`] is.
    pub fn address(&self) -> String {
        qualified([self.source.as_str(), SCHEMA, self.name.as_str()])
    }
}

/// One relation inside a source.
///
/// `data source` is the catalog name in the spelling it was registered under — the registered
/// spelling rather than the folded key, because that is what the data source is called everywhere
/// else the user meets it.
///
/// `reference` is the **recorded** form: the resolved [`TableReference`] itself, which is what a
/// plan's `TableScan` holds and therefore what `PlanDeps::remote` records. Held rather than a
/// rendered string, because *a rendered spelling is never a lookup key* — matching
/// `pg.public."Orders"`'s quoted address against the plan's plain-dotted recording found nothing,
/// and a `DROP` then reported a destructive action as consequence-free.
#[derive(Clone, Debug, PartialEq)]
pub struct Remote {
    pub source: String,
    pub reference: TableReference,
}

impl Remote {
    /// The namespace inside the data source, as the statement named it.
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
        qualified([self.source.as_str(), self.schema(), self.table()])
    }

    /// The reference as everything that *resolves* one holds it — what a match against a
    /// recorded name compares.
    pub fn recorded(&self) -> &TableReference {
        &self.reference
    }

    /// The address as the **source** knows it, `schema.relation` — what a report about a
    /// statement the server ran names, the catalog being Strata's word for the data source and
    /// already in that report's other half ("on 'pg'").
    pub fn server_address(&self) -> String {
        qualified([self.schema(), self.table()])
    }

    /// How the **source** is addressed: `schema.table`, never the catalog, which is Strata's own
    /// prefix for the data source and means nothing to the server. A full reference would render
    /// `"pg"."public"."orders"` into a statement the server then refuses.
    pub fn relation(&self) -> TableReference {
        TableReference::partial(self.schema().to_string(), self.table().to_string())
    }
}

/// What `name` addresses.
///
/// **The one choke point in front of every arm.** Every intercepted statement that manages a
/// target comes through here, so one sentence covers them all and no arm grows its own copy of
/// the check. The catalog list is asked rather than a list of data sources, because it is what
/// *resolves* the name: a catalog is registered exactly while its data source is live, which is
/// the window in which the user can address it.
///
/// A pure function of the session, deliberately: the editor asks it of a statement it is only
/// judging (`arms::remote::dispatched`), where no dispatch state exists. The
/// data source's own gates — is it writable, may this caller reach it — are asked of the resolved
/// answer, not folded into it.
pub fn resolve_target(ctx: &SessionContext, name: &TableReference) -> Target {
    if in_workspace(name) {
        return Target::Workspace {
            name: name.table().to_string(),
        };
    }
    // A store data source's catalog is **ours**, so it answers here and never falls through to
    // the database search below: it has one schema like the workspace's, and a name written
    // against any other resolves to nothing in it — which is `Nowhere`, exactly as
    // `strata.other.t` already is, rather than a relation on some server.
    if let TableReference::Full {
        catalog, schema, ..
    } = name
    {
        if is_store_catalog(ctx, catalog) {
            return match schema.as_ref() == SCHEMA {
                true => Target::Store(Stored {
                    source: catalog.to_string(),
                    name: name.table().to_string(),
                }),
                false => Target::Nowhere {
                    qualifier: name.to_string(),
                },
            };
        }
    }
    let TableReference::Full { catalog, .. } = name else {
        return Target::Nowhere {
            qualifier: name.to_string(),
        };
    };
    match source_catalog(ctx, catalog) {
        Some(source) => Target::Remote(Remote {
            source,
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

/// The data source's catalog `catalog` names, in the spelling it was registered under —
/// `None` for the workspace catalog, and for a qualifier that resolves to nothing.
///
/// **Folded on both sides, the workspace's own name included.** The catalog list resolves by
/// [`fold_ident`], so a quoted `"STRATA"` names the workspace catalog — and compared raw it
/// would slip past the guard below and then *match* the workspace's own entry in the search,
/// telling the user their project's catalog is a source. No real data source can produce that
/// (`check_catalog` refuses `strata` case-insensitively), so the sentence would name a data source
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

/// The wording for a statement that will **not** create or drop a name inside a **store** data
/// source's catalog.
///
/// Its own sentence rather than [`in_database`]'s, because the reason is a different one and so
/// is the fix: a store catalog holds the tables this project reads through a bucket, and what
/// decides which bucket a table reads through is its own LOCATION, not the catalog somebody
/// wrote in front of the name. Saying "which describes its own relations" of a bucket would be
/// simply untrue.
pub fn in_store(name: &str, source: &str, what: &str) -> String {
    format!(
        "'{name}' is in the data source '{source}', which holds the tables this project reads \
         through it. {what} are created in the project, so write the name without a catalog"
    )
}

/// The wording for a statement that will **not** touch a name inside a data source's
/// catalog — registering a table externally, which declares files and a format for a relation the
/// server already describes itself, and the view and drop statements the server owns instead.
pub fn in_database(name: &str, catalog: &str) -> String {
    format!(
        "'{name}' is in the data source '{catalog}', which describes its own relations. \
         Tables cannot be registered inside one"
    )
}

/// The wording for a write into a data source that has not been opted in — **minted once**, beside
/// [`in_database`], because both arms that can reach it must say the same thing.
///
/// It names the setting rather than the rule: a data source is read-only by default, so the
/// user is one toggle away and the sentence is only useful if it says which.
pub fn read_only(at: &Remote) -> String {
    format!(
        "The data source '{}' is read-only, so '{}' cannot be written. Turn off 'Read \
         only' in the source's settings",
        at.source,
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

    /// A session holding the workspace catalog and one data source called `pg`.
    fn session() -> SessionContext {
        let ctx = crate::builder::test_context(&std::collections::BTreeMap::new());
        fake_source(&ctx, "pg", &["orders"]);
        ctx
    }

    fn resolved(ctx: &SessionContext, name: &str) -> Target {
        resolve_target(ctx, &TableReference::parse_str(name))
    }

    /// **A store data source's catalog is its own answer** (EA-25 item 6), and the difference
    /// between the two helpers is the point of it.
    ///
    /// A bucket table is file-backed and the project's own, so an arm that *manages a def*
    /// reaches it by its bare name ([`Target::def`]) and every gate answers as it did before the
    /// tables moved. An arm that acts on the workspace and nothing else
    /// ([`Target::workspace`]) refuses it — in the store catalog's own words, not the
    /// database one's, because a bucket does not describe its own relations.
    ///
    /// Folding this into [`Target::Workspace`] was tried and is what this test exists to stop:
    /// the bare name it handed back resolves only in the workspace, so `DROP TABLE` on a bucket
    /// table answered "does not exist" about a table that was right there.
    #[test]
    fn a_bucket_table_is_a_target_of_its_own() {
        let ctx = session();
        ctx.register_catalog(
            "lake",
            std::sync::Arc::new(crate::providers::StoreCatalogProvider::new("lake".into())),
        );

        let target = resolved(&ctx, "lake.public.regions");
        assert_eq!(
            target,
            Target::Store(Stored {
                source: "lake".into(),
                name: "regions".into(),
            })
        );
        assert_eq!(
            target.locality(),
            Some(Locality::Local),
            "file-backed: it holds the grant it held in the workspace"
        );

        assert_eq!(
            resolved(&ctx, "lake.public.regions").def(WHAT_TABLES),
            Ok("regions".to_string()),
            "an arm managing the def gets the project's own name"
        );

        let refused = resolved(&ctx, "lake.public.regions")
            .workspace(WHAT_VIEWS)
            .expect_err("a view is not created inside a bucket's catalog");
        assert!(
            refused.contains("'lake'") && refused.contains("without a catalog"),
            "the refusal names the data source and the fix: {refused}"
        );
        assert!(
            !refused.contains("describes its own relations"),
            "and it is not the database wording, which would be untrue of a bucket: {refused}"
        );

        assert!(
            matches!(resolved(&ctx, "lake.other.regions"), Target::Nowhere { .. }),
            "the store catalog has one schema, like the workspace"
        );
    }

    const WHAT_TABLES: &str = "Tables";
    const WHAT_VIEWS: &str = "Views";

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

    /// A relation inside a data source resolves to it under the data source's **registered**
    /// spelling, whatever case the statement wrote — that being what every other surface calls it.
    #[test]
    fn a_sources_relation_resolves_under_its_registered_spelling() {
        let ctx = session();
        let Target::Remote(at) = resolved(&ctx, "PG.public.orders") else {
            panic!("'PG.public.orders' did not resolve to the data source");
        };
        assert_eq!(at.source, "pg");
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
                panic!("'{relation}' did not resolve to the data source");
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
