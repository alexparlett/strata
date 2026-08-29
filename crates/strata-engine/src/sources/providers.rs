//! One source, as DataFusion sees it: the catalog and schema providers every source is served
//! through, SQL-speaking or not.
//!
//! One concrete pair rather than a trait a source implements, because nothing in them is a
//! source's business: what they hold is the latest [`Listing`], and what they do with it is
//! resolution, enumeration and a cache. The source appears exactly once, in
//! [`SourceSchemaProvider::table`], which asks it for a provider it does not have yet.
//!
//! **Ours rather than the provider crate's `DatabaseCatalogProvider`**, for three reasons read out
//! of its source: it snapshots the listing at construction (so a ↻ could not refresh it), builds
//! plain `SqlTable`s with the default unparser dialect, and skips the federation wrapper — so the
//! generic path would forfeit exactly the pushdown the sources layer exists for.
//!
//! Read-only *of its own shape* — schemas and tables are not created or dropped through the
//! provider traits — and it says so rather than leaning on the trait's default refusal, because
//! the sentence a user gets should name the data source they are addressing rather than "catalog
//! provider". A write statement does not come through here: it resolves its target through this
//! catalog and then asks the source for a writer of its own.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, SchemaProvider, TableProvider};
use datafusion::common::{exec_err, DataFusionError, Result as DfResult};
use datafusion::logical_expr::TableType;
use datafusion::sql::TableReference;

use super::source::{Listing, Located, Relation, SourceCatalog};
use super::Shown;
use crate::fold_ident;

/// One source's catalog: the namespaces the latest enumeration found, each a
/// [`SourceSchemaProvider`].
pub struct SourceCatalogProvider {
    /// The name this data source registered under — Strata's own word for it.
    catalog: String,
    /// The data source's identity, carried down to every provider built under it as the fusion key.
    identity: String,
    source: Arc<dyn SourceCatalog>,
    /// Behind a lock because a statement that changes what the source holds re-enumerates and
    /// hands the result to [`adopt`](Self::adopt) — the alternative is a re-connect, which drops a
    /// live pool mid-session.
    schemas: RwLock<BTreeMap<String, Arc<SourceSchemaProvider>>>,
    /// What an unqualified name's search is scoped to — see [`Shown`]. Never consulted by
    /// [`schema`](CatalogProvider::schema) or by enumeration: a namespace switched off is still
    /// resolvable, still listed by `information_schema`, and still queryable in full.
    shown: Shown,
}

impl SourceCatalogProvider {
    pub(crate) fn new(
        catalog: String,
        identity: String,
        source: Arc<dyn SourceCatalog>,
        listing: &Listing,
        shown: Shown,
    ) -> Self {
        let provider = Self {
            catalog,
            identity,
            source,
            schemas: RwLock::new(BTreeMap::new()),
            shown,
        };
        provider.adopt(listing);
        provider
    }

    /// The namespaces this data source **shows**, for the resolver that scopes a bare name.
    pub(crate) fn shown(&self) -> BTreeSet<String> {
        self.shown.read().unwrap().clone()
    }

    /// Take on a fresh enumeration: a namespace the source has gained gets a provider, one it has
    /// lost loses its, and **one that survives keeps the provider it had** with its relation list
    /// replaced.
    ///
    /// Kept rather than rebuilt because a [`SourceSchemaProvider`] carries the built-provider
    /// cache, and rebuilding would make the next diagnostics pass re-introspect every remote
    /// relation the open buffers mention. What a relation the enumeration no longer lists loses is
    /// exactly its own cache entry.
    pub(super) fn adopt(&self, listing: &Listing) {
        let mut schemas = self.schemas.write().unwrap();
        schemas.retain(|folded, _| listing.schemas().contains_key(folded));
        for (folded, schema) in listing.schemas() {
            match schemas.get(folded) {
                Some(held) => held.relist(&schema.relations),
                None => {
                    schemas.insert(
                        folded.clone(),
                        Arc::new(SourceSchemaProvider {
                            catalog: self.catalog.clone(),
                            identity: self.identity.clone(),
                            schema: schema.name.clone(),
                            source: Arc::clone(&self.source),
                            relations: RwLock::new(schema.relations.clone()),
                            built: Mutex::new(BTreeMap::new()),
                        }),
                    );
                }
            }
        }
    }
}

impl fmt::Debug for SourceCatalogProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceCatalogProvider")
            .field("catalog", &self.catalog)
            .field(
                "schemas",
                &self.schemas.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl CatalogProvider for SourceCatalogProvider {
    fn schema_names(&self) -> Vec<String> {
        self.schemas
            .read()
            .unwrap()
            .values()
            .map(|schema| schema.schema.clone())
            .collect()
    }

    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        self.schemas
            .read()
            .unwrap()
            .get(&fold_ident(name))
            .map(|schema| Arc::clone(schema) as Arc<dyn SchemaProvider>)
    }

    fn register_schema(
        &self,
        _name: &str,
        _schema: Arc<dyn SchemaProvider>,
    ) -> DfResult<Option<Arc<dyn SchemaProvider>>> {
        exec_err!(
            "'{}' is a read-only view of a database. Schemas cannot be created in it",
            self.catalog
        )
    }

    fn deregister_schema(
        &self,
        _name: &str,
        _cascade: bool,
    ) -> DfResult<Option<Arc<dyn SchemaProvider>>> {
        exec_err!(
            "'{}' is a read-only view of a database. Schemas cannot be dropped from it",
            self.catalog
        )
    }
}

/// One namespace inside a source: the relations the enumeration found, and a lazily built, cached
/// `TableProvider` per relation.
///
/// **Building a provider can cost a round trip**, so it happens on first *use* and is then kept
/// for the life of the data source. Diagnostics validate a buffer on every catalog epoch, so
/// without the cache a query mentioning a remote table would introspect it per keystroke. A ↻
/// re-runs the registration pass, which re-connects — that, and nothing else, is the refresh.
pub struct SourceSchemaProvider {
    catalog: String,
    identity: String,
    schema: String,
    source: Arc<dyn SourceCatalog>,
    relations: RwLock<BTreeMap<String, Relation>>,
    built: Mutex<BTreeMap<String, Arc<dyn TableProvider>>>,
}

impl SourceSchemaProvider {
    /// Adopt a fresh relation list, dropping the cached provider of anything no longer in it —
    /// see [`SourceCatalogProvider::adopt`]. A relation that survives keeps its provider, which is
    /// the whole reason the cache is kept across a refresh at all.
    fn relist(&self, relations: &BTreeMap<String, Relation>) {
        *self.relations.write().unwrap() = relations.clone();
        self.built
            .lock()
            .unwrap()
            .retain(|folded, _| relations.contains_key(folded));
    }

    /// The relation `name` names, in the source's own spelling.
    fn relation(&self, name: &str) -> Option<Relation> {
        self.relations
            .read()
            .unwrap()
            .get(&fold_ident(name))
            .cloned()
    }
}

impl fmt::Debug for SourceSchemaProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SourceSchemaProvider")
            .field("catalog", &self.catalog)
            .field("schema", &self.schema)
            .field("relations", &self.relations.read().unwrap().len())
            .finish()
    }
}

#[async_trait]
impl SchemaProvider for SourceSchemaProvider {
    fn table_names(&self) -> Vec<String> {
        self.relations
            .read()
            .unwrap()
            .values()
            .map(|relation| relation.name.clone())
            .collect()
    }

    async fn table(&self, name: &str) -> DfResult<Option<Arc<dyn TableProvider>>> {
        let Some(relation) = self.relation(name) else {
            return Ok(None);
        };
        if let Some(built) = self.built.lock().unwrap().get(&fold_ident(name)) {
            return Ok(Some(Arc::clone(built)));
        }
        let at = Located {
            source: self.catalog.clone(),
            identity: self.identity.clone(),
            relation: TableReference::partial(self.schema.clone(), relation.name.clone()),
        };
        let provider = Arc::clone(&self.source)
            .table_provider(&at)
            .await
            .map_err(|e| {
                DataFusionError::Execution(format!(
                    "Cannot read '{}.{}.{}': {e}",
                    self.catalog, self.schema, relation.name
                ))
            })?;
        self.built
            .lock()
            .unwrap()
            .insert(fold_ident(name), Arc::clone(&provider));
        Ok(Some(provider))
    }

    /// **Overridden, and it is what keeps `SHOW TABLES` free.** The trait's default is
    /// `self.table(name).await.map(…table_type())`, and `information_schema.tables` calls it
    /// for every relation in every catalog — so without this, one `SHOW TABLES` would build a
    /// provider, and therefore a round trip, per remote relation.
    ///
    /// With it, `information_schema.tables` and `SHOW TABLES` cost **zero** remote calls.
    /// `information_schema.columns` still builds providers, because a column list is genuinely
    /// the schema; that is bounded by the cache above (once per relation per data source) and
    /// accepted.
    async fn table_type(&self, name: &str) -> DfResult<Option<TableType>> {
        Ok(self.relation(name).map(|relation| match relation.view {
            true => TableType::View,
            false => TableType::Base,
        }))
    }

    fn register_table(
        &self,
        _name: String,
        _table: Arc<dyn TableProvider>,
    ) -> DfResult<Option<Arc<dyn TableProvider>>> {
        exec_err!(
            "'{}' is a read-only view of a database. Tables cannot be created in it",
            self.catalog
        )
    }

    fn deregister_table(&self, _name: &str) -> DfResult<Option<Arc<dyn TableProvider>>> {
        exec_err!(
            "'{}' is a read-only view of a database. Tables cannot be dropped from it",
            self.catalog
        )
    }

    fn table_exist(&self, name: &str) -> bool {
        self.relations
            .read()
            .unwrap()
            .contains_key(&fold_ident(name))
    }
}
