//! **A remote relation's columns** as a freya-query capability (DB-07) — the one introspection a
//! data source does not do at connect time.
//!
//! ## Why this is a query at all
//!
//! Everything else the data-sources tree draws under a database is free: `Sources::listing` reads
//! the connect-time enumeration held beside the pool, so schemas and relation names cost nothing.
//! A relation's **columns** are the exception — DB-02 builds a relation's `TableProvider` lazily
//! precisely so that connecting to a database with a thousand tables is one round trip rather than
//! a thousand — so the first sight of a relation's columns is a real remote call, and the surfaces
//! that want them need a loading state.
//!
//! ## Why the key is a *list*
//!
//! The tree's walk is a plain synchronous function of its inputs, and it needs the columns of every
//! relation it is currently drawing open. A subscription per row cannot serve it (a walk cannot
//! await, and a virtualized row's scope is a slot), so the **pane** holds one subscription whose
//! key is the set of relations it has open, and hands the answer to the walk like any other input.
//! The inspector holds its own, on the one relation it is looking at, so it is reactive without
//! reaching into a pane it does not belong to.
//!
//! That is two entries over one relation, and it costs one extra *call* and no extra work:
//! `Sources::describe_remote` answers from the provider the data source caches per relation, so
//! every read after the first is local.
//!
//! ## Why the catalog generation is in the key
//!
//! A relation's columns are what the server had when we asked. Nothing on our side observes a
//! server-side `ALTER TABLE`, so the bound on that staleness is the same one the rest of the tree
//! carries: a ↻ re-connects, which builds new providers and moves the engine's catalog generation.
//! Keying on that number is what makes the refresh reach these columns too — without it a settled
//! entry would outlive the data source that answered it.

use freya::prelude::{use_side_effect, use_state};
use freya::query::{use_query, Captured, Query, QueryCapability, QueryStateData, UseQuery};
use std::collections::BTreeMap;
use std::time::Duration;
use strata_engine::sql::qualified;
use strata_engine::{CatalogGen, EngineError, RemoteRelation};
use strata_model::RemoteRef;

use crate::apps::project::contexts::EngineCtx;

/// What one relation's introspection came back with: the engine's own answer about it, or why it
/// could not be read.
///
/// The engine's [`RemoteRelation`] rather than a bare column list, because the same read is what
/// tells a surface whether the server calls it a table or a view — the label every profile gesture
/// is worded by, and a fact only this read and the tree's listing know (DB-02 made
/// `Relation::is_view` the one place the server's `relkind` is read, so the two agree by
/// construction).
///
/// A `Result` per relation rather than one for the whole read, because the relations in a key are
/// independent: one that the data source no longer lists must not blank the columns of the three
/// beside it.
pub type RemoteSchemas = BTreeMap<RemoteRef, Result<RemoteRelation, String>>;

/// Which relations to describe, and what the answer describes.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ColumnsSpec {
    /// Sorted, so the same set is the same key however it was assembled.
    pub relations: Vec<RemoteRef>,
    /// The catalog generation these columns are true as of — see the module docs. `None` while a
    /// registration pass is in flight, which is its own key and its own (never dispatched) entry.
    pub generation: Option<CatalogGen>,
}

/// The introspection capability. The engine handle rides as [`Captured`] — invisible to cache
/// identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct RemoteColumns(pub Captured<EngineCtx>);

impl QueryCapability for RemoteColumns {
    type Ok = RemoteSchemas;
    type Err = EngineError;
    type Keys = ColumnsSpec;

    /// One `describe_remote` per relation, in order.
    ///
    /// The three answers it keeps apart stay apart here. `Ok(None)` is the data source not listing
    /// this relation — which after a re-connect means it is **gone from what the server last told
    /// us**, so it is worded as the reconciliation it is rather than left as an absence a surface
    /// would have to invent a sentence for. An `Err` is the server refusing an introspection of a
    /// relation it does list, which is a fault about the data source and already carries the
    /// engine's own reading of it.
    async fn run(&self, spec: &ColumnsSpec) -> Result<RemoteSchemas, EngineError> {
        let mut out = RemoteSchemas::new();
        for relation in &spec.relations {
            let name = qualified([
                relation.source.as_str(),
                relation.schema.as_str(),
                relation.relation.as_str(),
            ]);
            let answer = match self.0.sources().describe_remote(name).await {
                Ok(Some(found)) => Ok(found),
                Ok(None) => Err(gone(relation)),
                Err(why) => Err(why.to_string()),
            };
            out.insert(relation.clone(), answer);
        }
        Ok(out)
    }
}

/// A relation the data source does not list. "Not in the data source" means "not in what it last
/// told us" — the enumeration is the connect-time one — so the sentence names the refresh, exactly
/// as the tree's missing-schema row does.
fn gone(relation: &RemoteRef) -> String {
    format!(
        "'{}' is not in this data source. Refresh the catalog if it has since been created.",
        relation.label()
    )
}

/// Subscribe to the columns of `relations`.
///
/// **One place, because the whole [`Query`] is the cache key.** `stale_time(MAX)` because the key
/// already carries everything that can make the answer untrue: a different relation set is a
/// different key, and a re-connect is a different generation. `clean_time` is left at its
/// default, so entries for generations and sets nobody is watching any more are swept — a re-read
/// of a swept entry
/// is one hop onto the engine runtime and no network, since the provider it reads is cached for
/// the life of the data source.
///
/// The list is **canonicalized here** rather than by each caller, because it is part of the key:
/// the tree hands over what its walk drew open and the inspector one relation, and two orderings of
/// one set would be two entries over one introspection.
///
/// An empty list is not dispatched: it is the ordinary state of a tree with no database open, and
/// of an inspector standing on a workspace column.
pub fn use_remote_columns(
    engine: &EngineCtx,
    mut relations: Vec<RemoteRef>,
    generation: Option<CatalogGen>,
) -> UseQuery<RemoteColumns> {
    relations.sort();
    relations.dedup();
    let enabled = !relations.is_empty() && generation.is_some();
    use_query(
        Query::new(
            ColumnsSpec {
                relations,
                generation,
            },
            RemoteColumns(engine.captured()),
        )
        .enable(enabled)
        .stale_time(Duration::MAX),
    )
}

/// Subscribe to `relations`' columns and hand back **everything settled so far**, not the current
/// entry's value.
///
/// **The accumulation is the point, and it is why no surface reads the entry directly.** The key
/// carries the relation set *and* the catalog generation, and freya-query starts a changed key at
/// `Pending` with no carried value — so reading the entry would blank every already-described
/// relation whenever any *other* relation was opened, or whenever any unrelated catalog pass moved
/// the generation. Merging each settled answer into a map the caller keeps is the rule the inspector's
/// STATISTICS zone already holds: **never show less than a moment ago.** A relation the server has
/// since dropped is corrected by the `Err` its next answer merges over the old one, and the map
/// only grows, bounded by the relations looked at in this window's life.
///
/// One hook rather than the merge written at each call site, because the tree and the inspector
/// would otherwise be two copies of a rule that is only worth anything if both obey it.
pub fn use_remote_schemas(
    engine: &EngineCtx,
    relations: Vec<RemoteRef>,
    generation: Option<CatalogGen>,
) -> RemoteSchemas {
    let columns = use_remote_columns(engine, relations, generation);
    let described = use_state(RemoteSchemas::new);
    use_side_effect(move || {
        let fresh = match &*columns.read().state() {
            QueryStateData::Settled { res: Ok(found), .. } => found.clone(),
            _ => return,
        };
        let mut described = described;
        let landed = fresh
            .iter()
            .any(|(relation, answer)| described.peek().get(relation) != Some(answer));
        if landed {
            described.write().extend(fresh);
        }
    });
    described.read().clone()
}

#[cfg(test)]
mod tests {
    use strata_engine::Engine;

    use super::*;

    fn relation(name: &str) -> RemoteRef {
        RemoteRef {
            source: "pg".into(),
            schema: "public".into(),
            relation: name.into(),
        }
    }

    /// **The generation is what bounds the staleness.** The same relations at a moved generation
    /// are a new entry, so a ↻ — which re-connects, rebuilds every provider and moves the engine's
    /// number — re-reads these columns instead of serving what the data source answered before it.
    ///
    /// Both come from an engine because nothing else can mint one — opacity is what stops a
    /// window claiming a catalog moved when it did not.
    #[test]
    fn a_re_connect_is_a_different_key() {
        let engine = Engine::builder().build();
        let relations = vec![relation("orders")];
        let asked_at = engine.catalog().generation();
        let before = ColumnsSpec {
            relations: relations.clone(),
            generation: Some(asked_at),
        };
        assert_eq!(
            before,
            ColumnsSpec {
                relations: relations.clone(),
                generation: Some(asked_at)
            },
            "the same question is the same key — a remount reads the cache"
        );

        engine.catalog().deregister("a name this engine never held");

        assert_ne!(
            before,
            ColumnsSpec {
                relations,
                generation: Some(engine.catalog().generation())
            }
        );
    }

    /// A relation the data source does not list is named as the reconciliation it is, and only that
    /// relation is affected — the others in the same read keep their columns.
    #[test]
    fn a_relation_the_source_no_longer_lists_names_the_refresh() {
        let why = gone(&relation("orders"));
        assert!(
            why.contains("'pg.public.orders' is not in this data source"),
            "{why}"
        );
        assert!(why.contains("Refresh the catalog"), "{why}");
    }
}
