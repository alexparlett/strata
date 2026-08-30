//! The data sources this engine can serve, and what the live ones hold.

use std::sync::Arc;

use datafusion::common::TableReference;
use datafusion::logical_expr::TableType;
use strata_arrow::column_info;
use strata_model::SourceDef;

use crate::catalog;
use crate::sources::source::SourceInfo;
use crate::sources::store::s3;
use crate::sources::{self, RemoteRelation, SchemaVisibility, SourceDetail, SourcesSnapshot};
use crate::sql::{DatabaseSym, RelationSym, SchemaSym};
use crate::{fold_ident, Dependents, Engine, EngineError, CATALOG};

/// This engine's data sources, from [`Engine::sources`].
///
/// The registrants it can connect with, the data sources it has been told about, and what a live
/// one enumerated. Every read answers from the connect-time enumeration rather than asking the
/// server, which is what makes them free; re-running the registration pass is the refresh.
#[derive(Clone, Copy)]
pub struct Sources<'a> {
    pub(super) engine: &'a Engine,
}

impl Sources<'_> {
    /// Register what one [`SourceDef`] describes: an **object store**, so tables can be
    /// registered over its bucket (W7), or a **database catalog**, so its relations resolve as
    /// `pg.public.orders`.
    ///
    /// **Before any table that reads it.** DataFusion resolves no remote scheme on its own:
    /// without this, a source path under `s3://acme-lake` fails its registration with "No
    /// suitable object store found" no matter how well-formed the def is. That ordering is
    /// [`Catalog::sync`](crate::Catalog::sync)'s, so every replay of a project gets
    /// it — and a data source needs exactly the same phase for a different reason, since
    /// a view over `pg.public.orders` cannot be created before the catalog exists.
    ///
    /// **The provider decides the arm, and there is one spawn either way**, so the two cannot
    /// drift apart on which runtime they ride: a pool may spawn a driver task per data source, and
    /// those have to land on the engine's own runtime or the engine's `Drop` does not end them.
    ///
    /// `Err` means nothing was registered, and carries what to fix — a missing region, a profile
    /// the credential chain does not answer for, a server that refused the user, a password this
    /// machine does not have, a kind nothing is registered for. See [`sources::connect`], which is
    /// the **one** path now: a bucket and a server are both a registrant answering `connect`.
    ///
    /// Moves the [`generation`](crate::Catalog::generation) on either arm: a refused connect
    /// takes back whatever this data source last registered, so a three-part name that resolved
    /// no longer does.
    pub async fn connect(self, conn: SourceDef) -> Result<(), EngineError> {
        let engine = self.engine;
        let ctx = engine.ctx.clone();
        let name = conn.named();
        engine.source_defs.note(&conn);
        let live = engine.live.clone();
        let registrants = engine.registrants.clone();
        let secrets = Arc::clone(&engine.secrets);
        let settled = engine
            .rt()
            .spawn(async move { sources::connect(&ctx, &registrants, &live, &conn, secrets).await })
            .await
            .map_err(|e| EngineError::task("connect", e))?;
        if engine.source_defs.resolve(&name).is_none() {
            self.disconnect(&name);
        }
        engine.generation.bump();
        settled.map_err(EngineError::from)
    }

    /// Forget what the data source called `name` registered — the Forget gesture's engine half.
    ///
    /// Synchronous, like [`Catalog::deregister`](crate::Catalog::deregister) and for the same
    /// reason: DataFusion just drops the entry from its registry, so there is no work to spawn
    /// and no answer to await. Dropping the handle is synchronous too — a pool's driver tasks
    /// end with it, on the runtime they were spawned on.
    ///
    /// **Both a store and a catalog are taken back**, because a name is all this is given — which
    /// is why the def an object store registered under is kept beside the name until now. Neither
    /// is a fault when it does nothing: a data source that never worked registered nothing, and a
    /// catalog kind put no store on the session at all. See [`sources::disconnect`].
    pub fn disconnect(self, name: &str) {
        let engine = self.engine;
        let def = engine.source_defs.def(name);
        sources::disconnect(
            &engine.ctx,
            &engine.registrants,
            &engine.live,
            def.as_ref(),
            name,
        );
        engine.source_defs.forget(name);
        engine.generation.bump();
    }

    /// Every data source this engine holds, read as of one moment — see [`SourcesSnapshot`].
    ///
    /// Answers from the connect-time enumeration rather than from any source, so it costs no I/O
    /// and every surface that reads it describes the same instant. Re-running the registration
    /// pass is the refresh.
    ///
    /// A data source this engine was told about is listed whether or not it could be reached: one
    /// whose credentials this machine cannot resolve today is still a data source, and
    /// [`SourceListing::live`](crate::sources::SourceListing::live) says so.
    pub fn listing(self) -> SourcesSnapshot {
        let engine = self.engine;
        sources::snapshot(
            &engine.ctx,
            &engine.registrants,
            &engine.live,
            &engine.source_defs.all(),
            engine.generation.current(),
        )
    }

    /// Sets which schemas the data source called `name` shows, without reconnecting.
    ///
    /// An unqualified name is searched for in the schemas a data source shows, so the session has
    /// to be told as the choice is made. Silent for a name this engine holds nothing for, and for
    /// one that registered an object store, which has no namespaces.
    ///
    /// Takes the set rather than a def, so the caller's own copy and this one cannot disagree
    /// about a data source's scoping; [`listing`](Self::listing) answers the new set on the next
    /// read.
    ///
    /// Moves the [`generation`](crate::Catalog::generation) whether or not this engine held the
    /// data source: over-invalidating once is cheaper than leaving a caller answering about a
    /// scoping that has moved.
    pub fn show_schemas(self, name: &str, schemas: &[String]) {
        let engine = self.engine;
        if let Some(mut def) = engine.source_defs.def(name) {
            def.schemas = schemas.to_vec();
            engine.source_defs.note(&def);
            engine.live.show(&def);
        }
        engine.generation.bump();
    }

    /// The qualified names completion may offer — one [`DatabaseSym`] per data source that
    /// registers a catalog, with its schemas and their relations.
    ///
    /// Every catalog, live or not: the name comes from the def, so a data source that has never
    /// answered still offers the name a query would have to write. Only a
    /// [`Live`](SchemaVisibility::Live) schema is offered under it, a schema the source does not
    /// have being a name that cannot resolve.
    ///
    /// Costs no I/O, like the [`listing`](Self::listing) it reads.
    pub fn database_syms(self) -> Vec<DatabaseSym> {
        self.listing()
            .sources
            .into_iter()
            .filter_map(|source| match source.detail {
                SourceDetail::Catalog { catalog, schemas } if !catalog.is_empty() => {
                    Some((catalog, schemas))
                }
                _ => None,
            })
            .map(|(name, schemas)| DatabaseSym {
                name,
                schemas: schemas
                    .into_iter()
                    .filter(|s| s.visibility == SchemaVisibility::Live)
                    .map(|s| SchemaSym {
                        name: s.name,
                        relations: s
                            .relations
                            .into_iter()
                            .map(|r| RelationSym {
                                view: r.view,
                                name: r.name,
                            })
                            .collect(),
                    })
                    .collect(),
            })
            .collect()
    }

    /// What forgetting the data source called `name` would leave invalid.
    ///
    /// Which half of the answer is empty follows from the kind of data source, so the caller does
    /// not say. Nothing reads an object store by name, so what it holds up is the table defs that
    /// name it and then everything reading one of those; no def can name a source, its relations
    /// being discovered rather than declared, so what it holds up is the views whose plans scan
    /// through its catalog.
    ///
    /// *Invalid*, not stopped: a dependent view captured its sources by `Arc` and goes on
    /// answering until the next reload.
    ///
    /// Bounded by what the last registration established (see
    /// [`Dependencies`](crate::Dependencies)): a def no pass has reached is not counted, and
    /// neither is a view the engine could not create, which has no recorded plan to have read
    /// anything with.
    ///
    /// Costs no I/O.
    pub fn dependents(self, name: &str) -> Dependents {
        let engine = self.engine;
        match engine.catalog_of(name) {
            Some(catalog) => Dependents {
                tables: Vec::new(),
                views: engine.dependencies.reading(&catalog),
            },
            None => {
                let tables = engine.dependencies.over(name);
                let views = engine.dependencies.above(&tables);
                Dependents { tables, views }
            }
        }
    }

    /// Every data source this engine can connect to — what a picker offers, what a catalog row
    /// badges, and what a data source form draws its rows from.
    ///
    /// One read for all three, off the registry itself, so a source an embedder registered is
    /// offered on the same terms as a shipped one and nothing keeps a second list of them.
    /// Synchronous and free.
    pub fn registrants(self) -> Vec<SourceInfo> {
        self.engine.registrants.registrants()
    }

    /// What the source registered as `kind` makes of `address`.
    ///
    /// The kind's own naming rule, reached without the caller knowing whose it is — so the editor
    /// refuses a mistyped address at the field rather than by a failed connect, and the rule has
    /// exactly one copy.
    ///
    /// # Errors
    ///
    /// The address's own refusal, or the sentence saying nothing is registered for `kind`.
    pub fn check_address(self, kind: &str, address: &str) -> Result<(), EngineError> {
        self.engine
            .registrants
            .check_address(kind, address)
            .map_err(EngineError::from)
    }

    /// Whether `candidate` repeats a setting its kind says two of its sources may not share.
    ///
    /// The kind's own rule ([`SourceKind::UNIQUE`](crate::SourceKind::UNIQUE)), asked of the
    /// registry so the editor refuses at the field exactly what a connect would refuse — and, for
    /// a kind that declares none, refuses nothing: two servers at one address differ in
    /// credentials and in nothing else, which is ordinary.
    ///
    /// `existing` is what to fold it against, `candidate` excluded by the caller.
    ///
    /// # Errors
    ///
    /// Naming the source already holding those values, and which settings they are.
    pub fn check_unique(
        self,
        candidate: &SourceDef,
        existing: &[SourceDef],
    ) -> Result<(), EngineError> {
        self.engine
            .registrants
            .check_unique(candidate, existing)
            .map_err(EngineError::from)
    }

    /// One relation inside a data source's catalog, from what the session already
    /// holds.
    ///
    /// The columns come from the provider the catalog caches per relation, so this costs one remote
    /// introspection the first time and nothing afterwards. There is **no def and no `Reg` row**
    /// behind it, by design: a database answers for itself.
    ///
    /// **The three answers are kept apart, which is why this is not an `Option`.** `Ok(None)` is an
    /// *expected absence* and the caller's own not-found stands; `Err` is a relation the data source
    /// **does** list whose introspection failed, which is a fault about the server rather than a
    /// fact about the name — reporting it absent would tell an agent a relation does not exist when
    /// it does.
    ///
    /// Existence is asked of `table_exist`, which reads the connect-time listing and costs nothing,
    /// where `table` is the round trip. So the common miss never dials out.
    pub async fn describe_remote(
        self,
        name: String,
    ) -> Result<Option<RemoteRelation>, EngineError> {
        let engine = self.engine;
        let reference = TableReference::parse_str(&name);
        let TableReference::Full { catalog, .. } = &reference else {
            return Ok(None);
        };
        let folded = fold_ident(catalog);
        if folded == CATALOG {
            return Ok(None);
        }
        let Some(source) = engine
            .ctx
            .catalog_names()
            .into_iter()
            .find(|registered| fold_ident(registered) == folded)
        else {
            return Ok(None);
        };
        let Some(schema_name) = reference.schema() else {
            return Ok(None);
        };
        let Some(schema) = engine
            .ctx
            .catalog(&source)
            .and_then(|catalog| catalog.schema(schema_name))
        else {
            return Ok(None);
        };
        if !schema.table_exist(reference.table()) {
            return Ok(None);
        }
        let relation = format!("{schema_name}.{}", reference.table());
        let table = reference.table().to_string();
        // **The kind is the schema provider's, never the built provider's.** A relation's
        // `TableProvider` here is the crate's federated `SqlTable`, whose `table_type` is
        // hardcoded `Base` — so asking *it* reports every remote view as a table, and this answer
        // and the tree's (which reads `relkind`) would disagree about the same relation.
        // `DbSchemaProvider::table_type` is the relkind-aware one, and it costs nothing.
        let kind = engine
            .rt()
            .spawn({
                let schema = Arc::clone(&schema);
                let table = table.clone();
                async move { schema.table_type(&table).await }
            })
            .await
            .map_err(|e| EngineError::Failed(format!("Reading '{name}' failed: {e}")))?
            .map_err(|e| EngineError::Failed(catalog::readable(&e.to_string())))?;
        let provider = engine
            .rt()
            .spawn(async move { schema.table(&table).await })
            .await
            .map_err(|e| EngineError::Failed(format!("Reading '{name}' failed: {e}")))?
            .map_err(|e| EngineError::Failed(catalog::readable(&e.to_string())))?;
        Ok(provider.map(|provider| RemoteRelation {
            source,
            relation,
            view: kind == Some(TableType::View),
            columns: provider
                .schema()
                .fields()
                .iter()
                .map(|field| column_info(field))
                .collect(),
        }))
    }

    /// The AWS profile names this machine's own configuration defines — what the data source
    /// editor's **Named profile** picker offers (W7 · 03). See `store::s3::aws_profiles`; no
    /// profile's *contents* are read.
    ///
    /// On the engine rather than beside the surface that asks for it, for the two reasons every
    /// other method here is: `aws-config` is [`store`]'s dependency and stays there, and this
    /// reads files — so it belongs on the runtime that keeps a read off the thread drawing every
    /// window, not in a component that would have to invent one.
    pub async fn aws_profiles(self) -> Vec<String> {
        self.engine
            .rt()
            .spawn(s3::aws_profiles())
            .await
            .unwrap_or_default()
    }
}
