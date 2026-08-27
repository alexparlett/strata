//! The connections this engine can serve, and what the live ones hold.

use std::sync::Arc;

use datafusion::common::TableReference;
use datafusion::logical_expr::TableType;
use strata_arrow::column_info;
use strata_model::{ConnectionDef, Provider};

use crate::catalog;
use crate::sources::source::SourceInfo;
use crate::sources::{self, RemoteRelation, SchemaVisibility, SourceDetail, SourcesSnapshot};
use crate::sql::{DatabaseSym, RelationSym, SchemaSym};
use crate::{fold_ident, store, Dependents, Engine, CATALOG};

/// This engine's data sources, from [`Engine::sources`].
///
/// The registrants it can connect with, the connections it has been told about, and what a live
/// one enumerated. Every read answers from the connect-time enumeration rather than asking the
/// server, which is what makes them free; re-running the registration pass is the refresh.
#[derive(Clone, Copy)]
pub struct Sources<'a> {
    pub(super) engine: &'a Engine,
}

impl Sources<'_> {
    /// Register what one [`ConnectionDef`] describes: an **object store**, so tables can be
    /// registered over its bucket (W7), or a **database catalog**, so its relations resolve as
    /// `pg.public.orders`.
    ///
    /// **Before any table that reads it.** DataFusion resolves no remote scheme on its own:
    /// without this, a source path under `s3://acme-lake` fails its registration with "No
    /// suitable object store found" no matter how well-formed the def is. That ordering is
    /// [`Catalog::sync`](crate::Catalog::sync)'s, so every replay of a project gets
    /// it — and a database connection needs exactly the same phase for a different reason, since
    /// a view over `pg.public.orders` cannot be created before the catalog exists.
    ///
    /// **The provider decides the arm, and there is one spawn either way**, so the two cannot
    /// drift apart on which runtime they ride: a pool may spawn a driver task per connection, and
    /// those have to land on the engine's own runtime or the engine's `Drop` does not end them.
    ///
    /// `Err` means nothing was registered, and carries what to fix — a missing region, a profile
    /// the credential chain does not answer for, a server that refused the user, a password this
    /// machine does not have, a kind nothing is registered for. See `store::connect` and
    /// [`sources::connect`].
    ///
    /// Moves the [`generation`](crate::Catalog::generation) on either arm: a refused connect
    /// takes back whatever this connection last registered, so a three-part name that resolved
    /// no longer does.
    pub async fn connect(self, conn: ConnectionDef) -> Result<(), String> {
        let engine = self.engine;
        let ctx = engine.ctx.clone();
        let name = conn.named();
        engine.connections.note(&conn);
        let live = engine.live.clone();
        let registrants = engine.sources.clone();
        let secrets = Arc::clone(&engine.secrets);
        let settled = engine
            .rt()
            .spawn(async move {
                match conn.provider.source().is_some() {
                    true => sources::connect(&ctx, &registrants, &live, &conn, secrets).await,
                    false => store::connect(&ctx, &conn).await,
                }
            })
            .await
            .map_err(|e| format!("connect task failed: {e}"))?;
        if engine.connections.resolve(&name).is_none() {
            self.disconnect(&name);
        }
        engine.generation.bump();
        settled
    }

    /// Forget what the connection called `name` registered — the Forget gesture's engine half.
    ///
    /// Synchronous, like [`Catalog::deregister`](crate::Catalog::deregister) and for the same
    /// reason: DataFusion just drops the entry from its registry, so there is no work to spawn
    /// and no answer to await. Dropping the handle is synchronous too — a pool's driver tasks
    /// end with it, on the runtime they were spawned on.
    ///
    /// **Both arms are asked**, because a name is all this is given — the def is gone by the time
    /// a Forget reaches here, which is why the identity an object store was registered under is
    /// kept beside the name. Neither arm is a fault when it does nothing: see `store::disconnect`
    /// and [`sources::disconnect`].
    pub fn disconnect(self, name: &str) {
        let engine = self.engine;
        if let Some(identity) = engine.connections.identity(name) {
            store::disconnect(&engine.ctx, &identity);
        }
        engine.connections.forget(name);
        sources::disconnect(&engine.ctx, &engine.live, name);
        engine.generation.bump();
    }

    /// **Every connection this engine holds**, as one value — see [`SourcesSnapshot`].
    ///
    /// The one read the data-sources tree, the schema picker, completion and the agent's catalog
    /// answers share, so no two of them can be looking at different moments and no consumer
    /// re-derives schema visibility from a def. It reads the connect-time enumeration rather than
    /// asking any source, which is what makes it free to call: a ↻ re-runs the registration pass,
    /// and *that* is the refresh.
    ///
    /// Every connection is listed, live or not — membership, in the same sense
    /// [`Connections`](crate::Connections) is — because a connection whose credentials this
    /// machine cannot resolve today is still one the project has, and a surface that dropped it
    /// would have nothing to hang the failure on.
    ///
    /// Synchronous and not on the runtime, because there is no I/O in it.
    pub fn listing(self) -> SourcesSnapshot {
        let engine = self.engine;
        sources::snapshot(
            &engine.ctx,
            &engine.sources,
            &engine.live,
            &engine.connections.all(),
            engine.generation.current(),
        )
    }

    /// Tell the session which schemas the connection called `name` now **shows** — the Schemas…
    /// picker's engine half, which writes the session without reconnecting.
    ///
    /// An unqualified name searches what a connection shows, so the session has to learn the new
    /// set as the picker commits it. Silent about a name this engine holds nothing for, and about
    /// one that registers an object store — a bucket has no namespaces — which are the ordinary
    /// cases rather than faults.
    ///
    /// Addressed by **name and the set**, rather than by a def: the picker has just written the
    /// def the host holds, and handing that back would let two spellings of one connection's
    /// schemas exist for as long as it took to be told. The engine's own retained def is updated
    /// with it, so [`listing`](Self::listing) answers the new scoping on the next read.
    ///
    /// Moves the [`generation`](crate::Catalog::generation) **on the gesture**, not on whether
    /// this engine happened to hold the connection: the caller has changed what its project
    /// shows, and every surface that re-derives on the generation is cheaper to over-invalidate
    /// once than to leave answering about a scoping that has moved.
    pub fn show_schemas(self, name: &str, schemas: &[String]) {
        let engine = self.engine;
        if let Some(mut def) = engine.connections.def(name) {
            if let Provider::Source(source) = &mut def.provider {
                source.schemas = schemas.to_vec();
                engine.connections.note(&def);
                engine.live.show(&def);
            }
        }
        engine.generation.bump();
    }

    /// The **qualified names completion may offer** — one [`DatabaseSym`] per connection that
    /// registers a catalog, its schemas and relations off [`listing`](Self::listing), so the
    /// popup and the tree cannot disagree about which remote names exist.
    ///
    /// Derived here rather than on the snapshot because `sources` and `sql` are peers inside this
    /// crate and neither imports the other; the facade is where both are already in scope.
    ///
    /// **Every catalog, live or not**: the name comes from the def, so a connection that has
    /// never answered still offers the name a query has to say. Only a
    /// [`Live`](SchemaVisibility::Live) schema is offered under it — one the def enables and the
    /// source does not have is a name that cannot resolve, and the tree already says so on its own
    /// row; a schema the connection does not show arrives with no relations, so this walk clones
    /// what it offers rather than the whole database.
    ///
    /// Free and synchronous, like the listing it reads: it is what lets the completion snapshot
    /// carry remote names without the popup ever reaching the network.
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

    /// **What forgetting the connection called `name` would leave invalid** — the tables that
    /// read through it and the views behind those, in one answer.
    ///
    /// The Forget confirm's whole sentence, and the reason it is the engine's: the two halves are
    /// derived from what registration *established* — a table's own def named its connection, and
    /// a view's plan named what it scanned — and re-deriving them beside the surface meant two
    /// consumers reimplementing a dependency walk over data this engine produced.
    ///
    /// **Which half is empty is what kind of connection it is**, not an option the caller passes.
    /// An object store has no readers in the SQL namespace at all — nothing reads a bucket *by
    /// name* — so what it has is the table defs that name it, and then everything reading one of
    /// those. A source is the other way round: no def can name one, its relations being discovered
    /// rather than declared, so its only readers are the views whose plans scan through its
    /// catalog.
    ///
    /// **Left invalid, not stopped**: a dependent view captured its sources by `Arc` and goes on
    /// answering until the next reload. What this names is what the catalog rows will say.
    ///
    /// Bounded by what the **last pass established** (see [`Dependencies`](crate::Dependencies)):
    /// a def no pass has reached yet is not counted, and neither is a view the engine could not
    /// create — the second bound is `ViewMeta`'s own and predates this, and a view that failed
    /// against a connection has no recorded plan to have read it with.
    ///
    /// Synchronous and free — two maps this session holds.
    pub fn dependents(self, name: &str) -> Dependents {
        let engine = self.engine;
        match engine.connections.def(name).and_then(|def| def.catalog()) {
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
    /// badges, and what a connection form draws its rows from.
    ///
    /// One read for all three, off the registry itself, so a source an embedder registered is
    /// offered on the same terms as a shipped one and nothing keeps a second list of them.
    /// Synchronous and free.
    pub fn registrants(self) -> Vec<SourceInfo> {
        self.engine.sources.registrants()
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
    pub fn check_address(self, kind: &str, address: &str) -> Result<(), String> {
        self.engine.sources.check_address(kind, address)
    }

    /// One relation inside a database connection's catalog, from what the session already
    /// holds.
    ///
    /// The columns come from the provider the catalog caches per relation, so this costs one remote
    /// introspection the first time and nothing afterwards. There is **no def and no `Reg` row**
    /// behind it, by design: a database answers for itself.
    ///
    /// **The three answers are kept apart, which is why this is not an `Option`.** `Ok(None)` is an
    /// *expected absence* and the caller's own not-found stands; `Err` is a relation the connection
    /// **does** list whose introspection failed, which is a fault about the server rather than a
    /// fact about the name — reporting it absent would tell an agent a relation does not exist when
    /// it does.
    ///
    /// Existence is asked of `table_exist`, which reads the connect-time listing and costs nothing,
    /// where `table` is the round trip. So the common miss never dials out.
    pub async fn describe_remote(self, name: String) -> Result<Option<RemoteRelation>, String> {
        let engine = self.engine;
        let reference = TableReference::parse_str(&name);
        let TableReference::Full { catalog, .. } = &reference else {
            return Ok(None);
        };
        let folded = fold_ident(catalog);
        if folded == CATALOG {
            return Ok(None);
        }
        let Some(connection) = engine
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
            .catalog(&connection)
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
            .map_err(|e| format!("Reading '{name}' failed: {e}"))?
            .map_err(|e| catalog::readable(&e.to_string()))?;
        let provider = engine
            .rt()
            .spawn(async move { schema.table(&table).await })
            .await
            .map_err(|e| format!("Reading '{name}' failed: {e}"))?
            .map_err(|e| catalog::readable(&e.to_string()))?;
        Ok(provider.map(|provider| RemoteRelation {
            connection,
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

    /// The AWS profile names this machine's own configuration defines — what the connection
    /// editor's **Named profile** picker offers (W7 · 03). See `store::aws_profiles`; no
    /// profile's *contents* are read.
    ///
    /// On the engine rather than beside the surface that asks for it, for the two reasons every
    /// other method here is: `aws-config` is [`store`]'s dependency and stays there, and this
    /// reads files — so it belongs on the runtime that keeps a read off the thread drawing every
    /// window, not in a component that would have to invent one.
    pub async fn aws_profiles(self) -> Vec<String> {
        self.engine
            .rt()
            .spawn(store::aws_profiles())
            .await
            .unwrap_or_default()
    }
}
