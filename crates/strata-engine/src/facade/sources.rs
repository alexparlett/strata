//! The connections this engine can serve, and what the live ones hold.

use std::sync::Arc;

use datafusion::common::TableReference;
use datafusion::logical_expr::TableType;
use strata_arrow::column_info;
use strata_model::ConnectionDef;

use crate::catalog;
use crate::sources::source::SourceInfo;
use crate::sources::{self, RemoteRelation, SchemaListingView, SchemaVisibility};
use crate::sql::{DatabaseSym, RelationSym, SchemaSym};
use crate::{fold_ident, store, Engine, CATALOG};

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
    /// [`register_pass`](crate::register::register_pass)'s, so every replay of a project gets
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
        engine.connections.note(&name, &conn.identity());
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

    /// What a live connection to a source registered: the catalog it is addressed by, and its
    /// schemas **scoped and tagged** against the def's own [`SourceDef::schemas`](strata_model::SourceDef::schemas) — `None` for a
    /// connection that holds no live catalog.
    ///
    /// The one read the data-sources tree, the schema picker and completion share, so no
    /// consumer re-derives visibility from the def. It reads the connect-time enumeration
    /// rather than asking the server, which is what makes it free to call: a ↻ re-runs the
    /// registration pass, and *that* is the refresh.
    ///
    /// Synchronous and not on the runtime, because there is no I/O in it.
    pub fn listing(self, conn: &ConnectionDef) -> Option<(String, Vec<SchemaListingView>)> {
        let source = conn.provider.source()?;
        sources::listing(&self.engine.live, conn, source)
    }

    /// Tell the session which schemas `conn` now **shows** — the Schemas… picker's engine half,
    /// which writes the def without reconnecting.
    ///
    /// An unqualified name searches what a connection shows, so the session has to
    /// learn the new set as the picker commits it. A no-op for a connection that is not live.
    ///
    /// Moves the [`generation`](crate::Catalog::generation): what a bare name resolves to has
    /// changed.
    pub fn show_schemas(self, conn: &ConnectionDef) {
        self.engine.live.show(conn);
        self.engine.generation.bump();
    }

    /// The **qualified names completion may offer** for `defs` — one [`DatabaseSym`] per database
    /// connection, its schemas and relations from [`listing`](Self::listing).
    ///
    /// Built here rather than in the editor because both halves are read the way the rest of the
    /// engine reads them: the catalog name off the def, so a connection that has never answered
    /// still offers the name a query has to say, and the schemas off the connect-time
    /// enumeration, already scoped. Only a `Live` schema is offered — one the def enables and the
    /// server does not have is a name that cannot resolve, and the tree already says so on its
    /// own row; a schema the connection does not show arrives here empty
    /// ([`SchemaListingView::relations`]), so this walk clones what it offers rather than the
    /// whole database.
    ///
    /// Free and synchronous, like the listing it reads: it is what lets the completion snapshot
    /// carry remote names without the popup ever reaching the network.
    pub fn database_syms<'a>(
        self,
        defs: impl IntoIterator<Item = &'a ConnectionDef>,
    ) -> Vec<DatabaseSym> {
        defs.into_iter()
            .filter_map(|def| {
                def.provider.source()?;
                let name = def.named();
                if name.is_empty() {
                    return None;
                }
                let schemas = self
                    .listing(def)
                    .map(|(_, schemas)| schemas)
                    .unwrap_or_default()
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
                    .collect();
                Some(DatabaseSym { name, schemas })
            })
            .collect()
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

    /// The catalogs data-source connections have registered, in the spelling they were registered
    /// under — the workspace's own excluded, since it is not one.
    ///
    /// Membership, not liveness, in the same sense `connections` is: a catalog is on the list
    /// exactly while its connection is live, which is also exactly while a three-part name can
    /// resolve through it. Synchronous and free — the list is a map this session holds.
    pub fn catalogs(self) -> Vec<String> {
        self.engine
            .ctx
            .catalog_names()
            .into_iter()
            .filter(|name| fold_ident(name) != CATALOG)
            .collect()
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
