//! Databases: turning a [`ConnectionDef`] with a [`PgStore`] into a live connection pool and
//! registering it on the session as a **catalog** (DB workstream, `docs/CONNECTIONS_SPEC.md`).
//!
//! The sibling of [`store`](super::store), deliberately not a path through it: an object store is
//! registered per bucket and answers about *files*, a database is registered as a catalog and
//! answers about *relations*. What the two share is the connection def, the `Reg` row it settles
//! onto, the pass's first phase, and the all-or-nothing contract.
//!
//! **The whole database comes through, and nothing is declared per table** — *discovery gets
//! catalogs, declaration gets defs*. A bucket cannot say what its tables are, so somebody must
//! declare globs and a format, and that declaration can fail; a database answers for itself. A def
//! per remote table would restate configuration the server owns, go stale silently, and mint
//! failure states for things whose only real failure is the connection's. Pinning one remote
//! relation into the workspace is a **view**, which needs no new machinery.
//!
//! **Ours rather than the provider crate's `DatabaseCatalogProvider`**, for three reasons read out
//! of its source: it snapshots the listing at construction (so a ↻ could not refresh it), builds
//! plain `SqlTable`s with the default unparser dialect, and skips the federation wrapper — so the
//! generic path would forfeit exactly the pushdown this workstream exists for.
//!
//! **A relation's provider is built one level below the crate's factory** ([`federate`]), so the
//! JSON accessor family can be rewritten into Postgres's own operators on its way out ([`json`]).
//! The dialect, the federation wrapper and the per-relation cache are unchanged by that move, and
//! [`DbSchemaProvider`] is still the one place a provider is constructed.
//!
//! **This module holds a password for the length of one login, and never stores one.** The def says
//! only that one is expected; the value is read per pool connection from the engine's
//! [`SecretProvider`] — the OS keystore in the app — under a
//! [reference derived](strata_core::secret::SecretRef::derived) from the connection's own identity.
//! [`connect`] takes the provider as an argument, so passwordless authentication is `None` rather
//! than a mode this module has to know about.

use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex, RwLock};

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, SchemaProvider, TableProvider};
use datafusion::common::{exec_err, DataFusionError, Result as DfResult};
use datafusion::logical_expr::{LogicalPlan, TableType};
use datafusion::prelude::*;
use datafusion::sql::TableReference;
use datafusion_table_providers_common::sql::db_connection_pool::PasswordProvider;
use datafusion_table_providers_common::util::secrets::to_secret_map;
use datafusion_table_providers_common::UnsupportedTypeAction;
use datafusion_table_providers_postgres::pool::{self, PostgresConnectionPool};
use secrecy::SecretString;
use tokio::task::spawn_blocking;

use strata_model::{
    check_catalog_name, parse_pg_address, ColumnInfo, ConnectionDef, PgStore, Provider,
};

use super::catalog::readable;
use super::connect::{self, Registration};
use super::fold_ident;
use super::providers::deregister_catalog;
use super::secrets::SecretProvider;
use strata_core::secret::SecretRef;

mod federate;
mod json;
mod write;

/// The federation rule list, wrapped so a write node is never federated whole (DB-12) — read by
/// `build_context`, because federation is installed on every engine whether or not one ever
/// connects to a database.
pub(crate) use federate::optimizer_rules;
/// An identifier as a statement **the server** parses may say it — see [`write::server_ident`].
pub(crate) use write::server_ident;
pub use write::RemoteTarget;

/// The keystore family every database password is filed under — the `kind` half of
/// [`SecretRef::derived`]. One string, here, because the editor's put and this module's read
/// have to land on the same slot.
pub const PG_PASSWORD: &str = "pg-password";

/// The keystore slot `conn` owns, if it owns one — **the one place the derivation is written**,
/// so the editor's put, the pool's read and a Forget's delete cannot address different entries.
///
/// `None` for every provider that keeps no secret. Nothing is stored on the def: the reference is
/// recomputed from the connection's URL each time one is needed ([`SecretRef::derived`]), which is
/// what keeps a machine-local id out of the committed `project.json` — and what makes a Forget
/// able to clean up from the def it is about to remove.
pub fn password_ref(conn: &ConnectionDef) -> Option<SecretRef> {
    match conn.provider {
        Provider::Postgres(_) => Some(SecretRef::derived(PG_PASSWORD, &conn.url())),
        _ => None,
    }
}

/// One relation the server told us about at connect: its own spelling, and what kind it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    /// The name as Postgres spells it — what a query has to say, and what the tree shows.
    pub name: String,
    /// `r` `p` `v` `m` `f` — an ordinary table, a partitioned one, a view, a materialized view,
    /// a foreign table. Kept as the server's own letter rather than mapped to a Strata word,
    /// because the only thing that reads it is [`table_type`](DbSchemaProvider::table_type),
    /// and a second vocabulary here would be one more thing to keep true.
    pub relkind: String,
}

impl Relation {
    /// Whether the server calls this a view — a view or a materialized view. The one place the
    /// server's letters are read, so the data-sources tree's Tables / Views split and
    /// DataFusion's own answer cannot disagree about a materialized view.
    pub fn is_view(&self) -> bool {
        matches!(self.relkind.as_str(), "v" | "m")
    }

    /// What DataFusion calls this relation — the answer `information_schema.tables` and
    /// `SHOW TABLES` print.
    fn table_type(&self) -> TableType {
        match self.is_view() {
            true => TableType::View,
            false => TableType::Base,
        }
    }
}

/// What one database connection put on the session, and what it takes to tear it down.
///
/// Held by URL on [`Databases`], not by catalog name, because [`Engine::disconnect`] is given a
/// URL and nothing else — the def is gone by then. It carries the catalog name so a rename (a
/// def whose URL is unchanged and whose catalog name moved) takes the *old* name back out.
struct Live {
    catalog: String,
    /// Held for its `Drop` — and, since DB-10, read: a write statement resolves its target
    /// through the catalog and then needs the pool the catalog reads through. Each pooled
    /// connection has a driver task spawned on the engine runtime, and that task ends when its
    /// client is dropped — so on a Forget, dropping this handle is what ends them, and on window
    /// close the engine's own `shutdown_background` does. Which is why the pool lives on the
    /// engine and not inside a task the engine's `Drop` is supposed to abort.
    pool: Arc<PostgresConnectionPool>,
    /// The connection's def, so a later connect can ask [`check_catalog_name`] which names are
    /// already taken **on the session** — a live fact this map owns, where the editor asks the
    /// same question of the project's stored defs. It is also what says whether the connection
    /// accepts writes ([`PgStore::read_only`]).
    def: ConnectionDef,
    /// The latest enumeration — the connect-time one until a statement that changed what the
    /// server holds re-runs it ([`relist`](Databases::relist)). Read by
    /// [`Engine::db_listing`](super::Engine::db_listing) rather than asking the server again.
    listing: Arc<Listing>,
    /// The schemas this connection **shows**, shared with the catalog provider — see [`Shown`].
    shown: Shown,
    /// The catalog this connection registered, held so a refresh can hand it a fresh enumeration
    /// without downcasting its way back out of the session's catalog list.
    provider: Arc<DbCatalogProvider>,
}

/// Which of a connection's schemas it shows, folded — **one live cell**, shared between the
/// connection and the catalog it registered.
///
/// Shared rather than copied onto each because the Schemas… picker edits the def without
/// reconnecting, so a copy taken at connect is stale the first time it is read. Its reader is
/// [`sql::qualify`](crate::sql), which scopes an **unqualified** name's search to what a
/// connection shows; a name written in full still resolves into any schema the role can see
/// ([`DbCatalogProvider::schema`] asks the listing, never this).
type Shown = Arc<RwLock<BTreeSet<String>>>;

/// The folded schema set a def asks for.
fn shown_of(pg: &PgStore) -> BTreeSet<String> {
    pg.schemas.iter().map(|schema| fold_ident(schema)).collect()
}

/// The schemas `catalog` shows, or `None` — the workspace's own catalog, or a test's stand-in —
/// which a caller reads as "no scoping to apply". The downcast is DataFusion's own pattern for a
/// custom provider, kept here so the resolver need not know this type.
pub(crate) fn shown_schemas(catalog: &dyn CatalogProvider) -> Option<BTreeSet<String>> {
    let db: &DbCatalogProvider = (catalog as &dyn Any).downcast_ref()?;
    Some(db.shown.read().unwrap().clone())
}

/// The live database connections this engine holds — the [`Connections`](super::Connections)
/// shape, for the same reasons.
///
/// A handle rather than a plain field because [`Engine::connect`] spawns its work onto the
/// engine runtime and that task must not hold the engine itself (the engine's `Drop` is what
/// aborts it). It holds pools, so it must not outlive the runtime they ride: the engine's own
/// field is the last strong reference, and the runtime is shut down after it in `Drop`.
#[derive(Clone, Default)]
pub(crate) struct Databases(Arc<Mutex<HashMap<String, Live>>>);

impl Databases {
    /// The defs of every *other* live database connection — what [`check_catalog_name`] folds a
    /// candidate against.
    fn peers(&self, url: &str) -> Vec<ConnectionDef> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(held, _)| held.as_str() != url)
            .map(|(_, live)| live.def.clone())
            .collect()
    }

    /// The catalog name and the enumeration a connection registered, or `None` if it is not
    /// live — what [`Engine::db_listing`](super::Engine::db_listing) reads.
    fn listing(&self, url: &str) -> Option<(String, Arc<Listing>)> {
        let held = self.0.lock().unwrap();
        let live = held.get(url)?;
        Some((live.catalog.clone(), Arc::clone(&live.listing)))
    }

    /// Forget `url`, handing back the catalog name it had registered.
    fn take(&self, url: &str) -> Option<String> {
        self.0.lock().unwrap().remove(url).map(|live| live.catalog)
    }

    /// What a write statement needs of the connection registered as `catalog`: its identity, its
    /// pool, its catalog provider, and whether it accepts writes at all.
    ///
    /// Keyed by the **catalog name** rather than the URL, because that is what a statement wrote
    /// and what the session's catalog list resolved; folded on both sides, since a catalog name is
    /// an unquoted identifier ([`StrataCatalogList`](crate::providers::StrataCatalogList)).
    fn at(&self, catalog: &str) -> Option<Connected> {
        let folded = fold_ident(catalog);
        let held = self.0.lock().unwrap();
        let (url, live) = held
            .iter()
            .find(|(_, live)| fold_ident(&live.catalog) == folded)?;
        Some(Connected {
            url: url.clone(),
            pool: Arc::clone(&live.pool),
            provider: Arc::clone(&live.provider),
            writable: match &live.def.provider {
                Provider::Postgres(pg) => !pg.read_only,
                _ => false,
            },
        })
    }

    /// Record a fresh enumeration for `url` — the half of a refresh the *map* owns, where
    /// [`DbCatalogProvider::adopt`] is the half the catalog owns. Both, because
    /// [`Engine::db_listing`](super::Engine::db_listing) reads this one and a query resolves
    /// through the other.
    fn relist(&self, url: &str, listing: Arc<Listing>) {
        if let Some(live) = self.0.lock().unwrap().get_mut(url) {
            live.listing = listing;
        }
    }

    /// Point this connection's [`Shown`] and its held def at what the stored def now says — the
    /// Schemas… picker's engine half, and the only writer besides [`connect`].
    ///
    /// Both, so the map holds one answer rather than two that can disagree. A no-op for a
    /// connection that is not live: the next connect reads the def anyway.
    pub(crate) fn show(&self, url: &str, pg: &PgStore) {
        let mut held = self.0.lock().unwrap();
        let Some(live) = held.get_mut(url) else {
            return;
        };
        *live.shown.write().unwrap() = shown_of(pg);
        if let Provider::Postgres(held) = &mut live.def.provider {
            held.schemas = pg.schemas.clone();
        }
    }

    /// Deregister every live database and drop its pool — the engine's `Drop`, and the only
    /// caller. See the comment there: this has to happen while the engine runtime is still up,
    /// because a `bb8` connection's own drop can spawn onto it.
    pub(crate) fn shutdown(&self, ctx: &SessionContext) {
        for (_, live) in self.0.lock().unwrap().drain() {
            deregister_catalog(ctx, &live.catalog);
        }
    }

    /// Record `live` under `url`, handing back whatever it displaced — which is the only thing
    /// that still knows the catalog name a renamed connection went in under. See [`connect`].
    fn put(&self, url: String, live: Live) -> Option<Live> {
        self.0.lock().unwrap().insert(url, live)
    }
}

/// One live database connection, as a write statement reaches it — see [`Databases::at`].
struct Connected {
    /// [`ConnectionDef::url`], which is what [`Databases`] is keyed by and therefore what a
    /// refresh has to name to put a new listing back.
    url: String,
    pool: Arc<PostgresConnectionPool>,
    provider: Arc<DbCatalogProvider>,
    writable: bool,
}

/// The connect-time shape of one database: its schemas in the server's own spelling, each with
/// its relations, keyed by [`fold_ident`] so resolution is case-insensitive the way SQL is.
#[derive(Debug, Default)]
pub struct Listing {
    schemas: BTreeMap<String, SchemaListing>,
}

#[derive(Debug, Default)]
struct SchemaListing {
    /// The schema as Postgres spells it.
    name: String,
    relations: BTreeMap<String, Relation>,
}

/// One schema as a surface sees it: what it is called, what is in it, and whether the
/// connection is set to show it.
///
/// **Scoped and tagged here**, so no consumer re-derives visibility from
/// [`PgStore::schemas`]: the tree, the schema picker and completion all read one answer.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaListingView {
    pub name: String,
    /// Filled only for [`SchemaVisibility::Live`]. There is nothing to list for an
    /// [`EnabledButMissing`](SchemaVisibility::EnabledButMissing) schema, and nothing that reads
    /// this *may* list a [`NotEnabled`](SchemaVisibility::NotEnabled) one — the tree drops those
    /// before it draws, the picker reads only the name and the tag, and completion offers what
    /// the connection shows. A schema's relation list is the **server's** to size, so cloning one
    /// that every consumer discards is the one avoidable cost in this read.
    pub relations: Vec<Relation>,
    pub visibility: SchemaVisibility,
}

/// One relation inside a database connection's catalog, as a surface outside the engine sees
/// it — [`Engine::describe_remote`](super::Engine::describe_remote)'s answer.
///
/// Deliberately not a [`TableMeta`](super::TableMeta): that is what a *registration* learned
/// about a def, and a remote relation has no def, no sources and no free row count. What it has
/// is an address, a connection it belongs to, and the schema the connection reports — which is
/// the whole of what a describe can honestly say about it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteRelation {
    /// The catalog the connection registered, in that connection's own spelling.
    pub connection: String,
    /// The relation's address inside the database, `schema.table`.
    pub relation: String,
    /// Whether the server calls it a view — a view or a materialized view
    /// ([`Relation::table_type`]), which the listing already knows and answers for free.
    pub view: bool,
    pub columns: Vec<ColumnInfo>,
}

/// Whether a schema is one the connection shows, and whether the server has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaVisibility {
    /// Enabled on the def, and the server has it.
    Live,
    /// Enabled on the def, and the server does not have it (or the role cannot see it) — a
    /// schema that was dropped or renamed, which the def cannot know about on its own.
    EnabledButMissing,
    /// The server has it and the def does not show it. Still queryable: registration exposes
    /// every schema, and this scopes display only ([`PgStore::schemas`]).
    NotEnabled,
}

/// Build the connection pool `conn` describes, enumerate the database, and register it as a
/// catalog on `ctx` — the database arm of `Engine::connect`.
///
/// **Building the pool is the probe**, all-or-nothing exactly as [`store::connect`] is, and for
/// free: `PostgresConnectionPool::new` resolves the host, opens a TCP connection, authenticates,
/// builds the pool and runs `SELECT 1`, failing on any of them. There is no separate
/// `reachable` step here because there is nothing left to ask — where an object store's
/// description can be well-formed and wrong in a way only the bucket knows, a database either
/// let us in or did not.
///
/// `passwords` is the password seam, and it is an argument rather than something read here:
/// [`Engine::connect`](super::Engine::connect) hands in [`SecretPassword`], and the
/// integration test hands in the crate's `StaticPasswordProvider`, so both drive this same entry
/// point and no test process opens a keystore. `None` is passwordless authentication (`trust`,
/// `peer`, certificate).
///
/// **Re-connecting replaces.** A URL this map already holds has its catalog deregistered first,
/// so a def whose *catalog name* moved while its URL did not — the editor's rename — is handled
/// here by construction rather than by the editor remembering to say so.
pub(crate) async fn connect(
    ctx: &SessionContext,
    dbs: &Databases,
    conn: &ConnectionDef,
    pg: &PgStore,
    passwords: Option<Arc<dyn PasswordProvider>>,
) -> Result<(), String> {
    let url = conn.url();
    let registration = match prepare(dbs, conn, pg, passwords).await {
        Ok((prepared, live)) => {
            let displaced = dbs.put(url.clone(), live);
            Ok((
                Registration::Catalog(prepared.name.clone(), prepared.provider),
                displaced.filter(|old| old.catalog != prepared.name),
            ))
        }
        Err(why) => Err(why),
    };
    let (registration, displaced) = match registration {
        Ok((registration, displaced)) => (Ok(registration), displaced),
        Err(why) => (Err(why), None),
    };
    let settled = connect::settle(ctx, registration, || take_back(ctx, dbs, &url));
    if let Some(old) = displaced {
        deregister_catalog(ctx, &old.catalog);
    }
    settled
}

/// Remove whatever `url` last registered, under the name it registered it under. Silent when
/// there is nothing: a first connect, or a def that has never worked.
fn take_back(ctx: &SessionContext, dbs: &Databases, url: &str) {
    if let Some(previous) = dbs.take(url) {
        deregister_catalog(ctx, &previous);
    }
}

/// The catalog a connect built, and its name — kept together so [`connect`]'s settle has one
/// value to hand over.
struct Prepared {
    name: String,
    provider: Arc<dyn CatalogProvider>,
}

/// Everything a database connection can be judged on: its address, its catalog name against the
/// session's other databases, the login, and the enumeration.
///
/// Split from [`connect`] the way `store::prepare` is, so the registration is one line with one
/// meaning — but note that unlike the object-store arm every step here does reach the server,
/// because a database's description cannot be checked any other way.
async fn prepare(
    dbs: &Databases,
    conn: &ConnectionDef,
    pg: &PgStore,
    passwords: Option<Arc<dyn PasswordProvider>>,
) -> Result<(Prepared, Live), String> {
    let url = conn.url();
    conn.provider.check_address(&conn.address)?;
    check_catalog_name(&dbs.peers(&url), conn)?;

    let pool = build_pool(conn, pg, passwords).await?;
    let listing = Arc::new(enumerate(&pool).await?);
    let catalog = pg.catalog.trim().to_string();
    let shown: Shown = Arc::new(RwLock::new(shown_of(pg)));
    let provider = Arc::new(DbCatalogProvider::new(
        catalog.clone(),
        Arc::clone(&pool),
        Arc::clone(&listing),
        Arc::clone(&shown),
    ));
    Ok((
        Prepared {
            name: catalog.clone(),
            provider: Arc::clone(&provider) as Arc<dyn CatalogProvider>,
        },
        Live {
            catalog,
            pool,
            def: conn.clone(),
            listing,
            shown,
            provider,
        },
    ))
}

/// Forget the catalog a connection registered — the Forget gesture's engine half, and the half
/// an edit that moves a connection's URL also needs.
///
/// Addressed by [`ConnectionDef::url`] like [`store::disconnect`], and silent about doing
/// nothing for the same reason: a URL this engine holds no database for is the ordinary case
/// (every object-store connection, and every database that never connected).
pub(crate) fn disconnect(ctx: &SessionContext, dbs: &Databases, url: &str) {
    take_back(ctx, dbs, url);
}

/// What a surface sees of one live database: the catalog it is addressed by, and its schemas
/// scoped against [`PgStore::schemas`] — see [`Engine::db_listing`](super::Engine::db_listing).
pub(crate) fn listing(
    dbs: &Databases,
    conn: &ConnectionDef,
    pg: &PgStore,
) -> Option<(String, Vec<SchemaListingView>)> {
    let (catalog, listing) = dbs.listing(&conn.url())?;
    let enabled: BTreeSet<String> = pg.schemas.iter().map(|s| fold_ident(s)).collect();
    let mut views: Vec<SchemaListingView> = listing
        .schemas
        .iter()
        .map(|(folded, schema)| match enabled.contains(folded) {
            true => SchemaListingView {
                name: schema.name.clone(),
                relations: schema.relations.values().cloned().collect(),
                visibility: SchemaVisibility::Live,
            },
            false => SchemaListingView {
                name: schema.name.clone(),
                relations: Vec::new(),
                visibility: SchemaVisibility::NotEnabled,
            },
        })
        .collect();
    views.extend(
        enabled
            .iter()
            .filter(|folded| !listing.schemas.contains_key(*folded))
            .map(|folded| SchemaListingView {
                name: pg
                    .schemas
                    .iter()
                    .find(|s| &fold_ident(s) == folded)
                    .cloned()
                    .unwrap_or_else(|| folded.clone()),
                relations: Vec::new(),
                visibility: SchemaVisibility::EnabledButMissing,
            }),
    );
    views.sort_by(|a, b| a.name.cmp(&b.name));
    Some((catalog, views))
}

/// Whether the connection registered as `catalog` accepts writes — the inverse of
/// [`PgStore::read_only`], and `false` for a catalog no live database registered.
///
/// The *answer*, never the refusal: what a user is told about a read-only connection is
/// [`ddl`](crate::ddl)'s, beside every other sentence about a remote target.
pub(crate) fn writable(dbs: &Databases, catalog: &str) -> bool {
    dbs.at(catalog).is_some_and(|live| live.writable)
}

/// Append `input`'s rows to the remote relation `at` — the `INSERT` arm's engine half (DB-10).
///
/// The read provider comes from the **catalog**, which is where it is already built and cached;
/// the writer wraps it, drives the sink once and is dropped. The schema the sink validates
/// against is that provider's — the server's own — because DataFusion planned `input` against it.
pub(crate) async fn insert_into(
    ctx: &SessionContext,
    dbs: &Databases,
    at: &RemoteTarget,
    input: &LogicalPlan,
) -> Result<u64, String> {
    let live = connected(dbs, &at.catalog)?;
    let provider = relation_provider(ctx, at).await?;
    let schema = provider.schema();
    write::append(ctx, &live.pool, at, provider, schema, input).await
}

/// Create the remote relation `at` from `input`'s schema and fill it — the CTAS arm's engine half.
/// `None` means the server already held the relation and nothing was created, which is the arm's
/// to word (`IF NOT EXISTS`, `OR REPLACE`, or a plain refusal).
///
/// **The rollback is the point of the ordering.** The table is created, the catalog re-enumerated
/// so the new relation resolves, and only then filled; a fill that fails takes the table back off
/// the server and re-enumerates again, so nothing is left holding a name the user thinks has data
/// in it. A **cancel** is the other way out and reaches no error path at all, which is what
/// [`write::Created`] is for — armed only while the awaits ahead of `settled` can be dropped.
///
/// The existence question is answered inside [`write::create`]'s own transaction rather than by a
/// round trip before it, so the answer cannot go stale in between: `CreateTableBuilder` hardcodes
/// `IF NOT EXISTS`, and a relation adopted that way would be dropped by a failed fill.
///
/// A refresh that itself fails is logged rather than reported: the statement did what it said, and
/// a ↻ re-runs the registration pass.
pub(crate) async fn create_table_as(
    ctx: &SessionContext,
    dbs: &Databases,
    at: &RemoteTarget,
    input: &LogicalPlan,
) -> Result<Option<u64>, String> {
    let live = connected(dbs, &at.catalog)?;
    let schema = Arc::clone(input.schema().inner());
    if !write::create(&live.pool, at, Arc::clone(&schema)).await? {
        return Ok(None);
    }
    let mut created = write::Created::open(Arc::clone(&live.pool), at.clone());
    relist(&live, dbs).await;

    let filled = match relation_provider(ctx, at).await {
        Ok(provider) => write::append(ctx, &live.pool, at, provider, schema, input).await,
        Err(why) => Err(why),
    };
    created.settled();
    if filled.is_err() {
        write::discard(&live.pool, at).await;
        relist(&live, dbs).await;
    }
    filled.map(Some)
}

/// Run one statement **on the server** and report the rows it moved, over the extended query
/// protocol rather than `batch_execute`: it is the only one that answers with an affected-row
/// count, and it carries exactly one statement, so a second one smuggled past the parser is
/// refused by the driver rather than run.
pub(crate) async fn execute(dbs: &Databases, catalog: &str, sql: &str) -> Result<u64, String> {
    let live = connected(dbs, catalog)?;
    let conn = live
        .pool
        .connect_direct()
        .await
        .map_err(|e| format!("Cannot reach '{catalog}': {e}"))?;
    conn.conn
        .execute(sql, &[])
        .await
        .map_err(|e| readable(&server_error(&e)))
}

/// What the server said, rather than the driver's own `db error` placeholder: `tokio_postgres`
/// renders the useful sentence — the SQLSTATE, the position, the hint — on the wrapped `DbError`.
fn server_error(e: &impl Error) -> String {
    match e.source() {
        Some(cause) => cause.to_string(),
        None => e.to_string(),
    }
}

/// [`relist`] reached by catalog name, for the arms that resolve a target and nothing else — a
/// relation a drop removed loses its cached provider here, which is what keeps a stale one from
/// answering scans for something the server no longer has.
pub(crate) async fn relist_at(dbs: &Databases, catalog: &str) {
    let Ok(live) = connected(dbs, catalog) else {
        return;
    };
    relist(&live, dbs).await;
}

/// Re-enumerate `live` and hand the result to both halves that hold one — the map that
/// [`Engine::db_listing`](super::Engine::db_listing) reads, and the catalog a query resolves
/// through.
async fn relist(live: &Connected, dbs: &Databases) {
    match enumerate(&live.pool).await {
        Ok(listing) => {
            let listing = Arc::new(listing);
            live.provider.adopt(&listing);
            dbs.relist(&live.url, listing);
        }
        Err(why) => tracing::warn!(
            "could not re-read the database's schemas after a statement changed them ({why}); \
             refresh the catalog to see it"
        ),
    }
}

/// The live connection registered as `catalog`, or a sentence saying it is not one.
///
/// Unreachable in the app — a write arm gets here only after the session's catalog list resolved
/// the name — and stated anyway, because the headless host and the tests can register a catalog
/// this map has never heard of.
fn connected(dbs: &Databases, catalog: &str) -> Result<Connected, String> {
    dbs.at(catalog)
        .ok_or_else(|| format!("'{catalog}' is not a connected database"))
}

/// The read provider for one remote relation, resolved the way a query resolves it.
async fn relation_provider(
    ctx: &SessionContext,
    at: &RemoteTarget,
) -> Result<Arc<dyn TableProvider>, String> {
    ctx.table_provider(TableReference::full(
        at.catalog.clone(),
        at.schema.clone(),
        at.table.clone(),
    ))
    .await
    .map_err(|e| e.to_string())
}

/// The pool itself, with every failure turned into a sentence naming what to fix.
async fn build_pool(
    conn: &ConnectionDef,
    pg: &PgStore,
    passwords: Option<Arc<dyn PasswordProvider>>,
) -> Result<Arc<PostgresConnectionPool>, String> {
    let address = parse_pg_address(conn.address.trim())?;
    pg.check_user()?;
    let user = pg.user.trim();
    let mut params = HashMap::from([
        ("host".to_string(), address.host.to_string()),
        ("port".to_string(), address.port.to_string()),
        ("db".to_string(), address.database.to_string()),
        ("user".to_string(), user.to_string()),
        ("sslmode".to_string(), pg.sslmode.as_str().to_string()),
        ("application_name".to_string(), "Strata".to_string()),
    ]);
    let cert = pg.sslrootcert.trim();
    if pg.sslmode.verifies() && !cert.is_empty() {
        params.insert("sslrootcert".to_string(), cert.to_string());
    }

    let params = to_secret_map(params);
    let built = match passwords {
        Some(provider) => {
            PostgresConnectionPool::new_with_password_provider(params, provider).await
        }
        None => PostgresConnectionPool::new(params).await,
    };
    built
        .map(|pool| Arc::new(pool.with_unsupported_type_action(UnsupportedTypeAction::String)))
        .map_err(|e| refused(conn, pg, e))
}

/// Why a login did not happen, in the terms of the thing to fix.
///
/// The crate's own prose is good and is kept wherever it already names the fault; the two arms
/// rewritten here are the ones it cannot word as well as we can, because it does not know there
/// is a connection editor behind them. Nothing in any of it is a password: the crate builds its
/// connection string without one on purpose, and our own provider's failure is
/// [`SecretPassword`]'s sentence.
fn refused(conn: &ConnectionDef, pg: &PgStore, e: pool::Error) -> String {
    match e {
        pool::Error::InvalidHostOrPortError { host, port, .. } => format!(
            "Cannot reach a PostgreSQL server at '{host}:{port}'. Check the address, and that \
             the server is running."
        ),
        pool::Error::InvalidUsernameOrPassword { .. } => format!(
            "The server refused the user '{}'. Check the user and its password.",
            pg.user.trim()
        ),
        pool::Error::PasswordProviderError { source } => source.to_string(),
        other => format!("Cannot connect to '{}': {other}", conn.url()),
    }
}

/// One connection's password: read **per new pool connection**, never cached, never held past the
/// login it is for.
///
/// The reference is [derived](SecretRef::derived) from the connection's URL rather than stored
/// on the def, so the committed `project.json` carries no machine-local id — see
/// [`PgPassword`](strata_model::PgPassword). Which means the ordinary answer on a colleague's
/// machine is *there is no entry*, and that is a sentence rather than a fault: they enter the
/// password once, into their own keystore, and nothing in git changes.
///
/// The read is blocking — a keystore call can wait on a platform lock or on the user — so it goes
/// through `spawn_blocking` rather than stalling a runtime worker while bb8 opens a connection.
pub(crate) struct SecretPassword {
    key: SecretRef,
    secrets: Arc<dyn SecretProvider>,
    /// For the message alone. The URL rather than the host, because that is what the row and
    /// the editor both name a connection by.
    url: String,
}

impl SecretPassword {
    /// The provider for `url`'s password, addressing the slot that URL derives.
    pub(crate) fn new(url: String, secrets: Arc<dyn SecretProvider>) -> Self {
        Self {
            key: SecretRef::derived(PG_PASSWORD, &url),
            secrets,
            url,
        }
    }
}

#[async_trait]
impl PasswordProvider for SecretPassword {
    async fn get_password(&self) -> Result<SecretString, Box<dyn Error + Send + Sync>> {
        let key = self.key.clone();
        let secrets = Arc::clone(&self.secrets);
        let read = spawn_blocking(move || secrets.secret(&key))
            .await
            .map_err(|e| format!("Reading the password for '{}' failed: {e}", self.url))?;
        match read? {
            Some(secret) => Ok(SecretString::from(secret.expose().to_string())),
            None => Err(format!(
                "No password is stored on this machine for '{}'. Open the connection and enter \
                 it.",
                self.url
            )
            .into()),
        }
    }
}

/// **One round trip for the whole catalog shape.** `pg_class` joined to `pg_namespace`, filtered
/// to the relation kinds a query can read and to what this role may actually use.
///
/// `pg_class`, not the crate's `pg_tables`, and that is the third reason the listing is ours:
/// `pg_tables` is tables only, so remote **views**, materialized views, partitioned tables and
/// foreign tables would be missing from the tree while remaining perfectly queryable — a tree
/// that lies about what is there.
///
/// The system schemas are left out. `pg_catalog` and `information_schema` are visible to every
/// role and would add hundreds of relations to every listing; DataGrip hides them for the same
/// reason, and nothing stops a query naming one (registration is not what resolves a schema —
/// see [`DbCatalogProvider::schema`], which answers for any schema this listing knows, and note
/// that a query for a system table simply finds no schema rather than misbehaving).
///
/// The privilege filters are what make "every schema the role can see" true rather than
/// aspirational: a relation the role cannot `SELECT` would be listed, offered in completion,
/// and then fail.
const RELATIONS_QUERY: &str = "\
SELECT n.nspname, c.relname, c.relkind::text \
FROM pg_catalog.pg_class c \
JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
WHERE c.relkind IN ('r', 'p', 'v', 'm', 'f') \
  AND n.nspname NOT IN ('pg_catalog', 'information_schema') \
  AND n.nspname NOT LIKE 'pg\\_toast%' \
  AND n.nspname NOT LIKE 'pg\\_temp%' \
  AND pg_catalog.has_schema_privilege(n.oid, 'USAGE') \
  AND pg_catalog.has_table_privilege(c.oid, 'SELECT') \
ORDER BY 1, 2";

/// Run [`RELATIONS_QUERY`] and fold it into the shape the catalog provider serves.
async fn enumerate(pool: &Arc<PostgresConnectionPool>) -> Result<Listing, String> {
    let conn = pool
        .connect_direct()
        .await
        .map_err(|e| format!("Cannot read the database's schemas: {e}"))?;
    let rows = conn
        .conn
        .query(RELATIONS_QUERY, &[])
        .await
        .map_err(|e| format!("Cannot read the database's schemas: {e}"))?;

    let mut schemas: BTreeMap<String, SchemaListing> = BTreeMap::new();
    for row in &rows {
        let schema: String = row.get(0);
        let name: String = row.get(1);
        let relkind: String = row.get(2);
        let entry = schemas
            .entry(fold_ident(&schema))
            .or_insert_with(|| SchemaListing {
                name: schema.clone(),
                relations: BTreeMap::new(),
            });
        if entry.name != schema {
            tracing::warn!(
                "database: schema '{schema}' is hidden by '{}', which folds to the same SQL \
                 name; its relations are not listed",
                entry.name
            );
            continue;
        }
        if let Some(held) = entry.relations.insert(
            fold_ident(&name),
            Relation {
                name: name.clone(),
                relkind,
            },
        ) {
            tracing::warn!(
                "database: relation '{schema}.{name}' is hidden by '{}', which folds to the \
                 same SQL name",
                held.name
            );
            entry.relations.insert(fold_ident(&name), held);
        }
    }
    Ok(Listing { schemas })
}

/// One database, as DataFusion sees it: the schemas the latest enumeration found, each a
/// [`DbSchemaProvider`].
///
/// Read-only *of its own shape* — schemas and tables are not created or dropped through the
/// provider traits — and it says so rather than leaning on the trait's default refusal, because
/// the sentence a user gets should name the connection they are addressing rather than "catalog
/// provider". A write statement does not come through here: it resolves its target through this
/// catalog and then builds a writer of its own ([`write`]).
struct DbCatalogProvider {
    catalog: String,
    /// Behind a lock because a statement that changes what the server holds re-enumerates and
    /// hands the result to [`adopt`](Self::adopt) — the alternative is a re-connect, which drops
    /// a live pool mid-session.
    schemas: RwLock<BTreeMap<String, Arc<DbSchemaProvider>>>,
    pool: Arc<PostgresConnectionPool>,
    /// What an unqualified name's search is scoped to — see [`Shown`]. Never consulted by
    /// [`schema`](CatalogProvider::schema) or by enumeration: a schema switched off is still
    /// resolvable, still listed by `information_schema`, and still queryable in full.
    shown: Shown,
}

impl DbCatalogProvider {
    fn new(
        catalog: String,
        pool: Arc<PostgresConnectionPool>,
        listing: Arc<Listing>,
        shown: Shown,
    ) -> Self {
        let provider = Self {
            catalog,
            schemas: RwLock::new(BTreeMap::new()),
            pool,
            shown,
        };
        provider.adopt(&listing);
        provider
    }

    /// Take on a fresh enumeration: a schema the server has gained gets a provider, one it has
    /// lost loses its, and **a schema that survives keeps the provider it had** with its relation
    /// list replaced.
    ///
    /// Kept rather than rebuilt because a `DbSchemaProvider` carries the built-provider cache, and
    /// rebuilding would make the next diagnostics pass re-introspect every remote relation the
    /// open buffers mention. What a relation the enumeration no longer lists loses is exactly its
    /// own cache entry.
    fn adopt(&self, listing: &Listing) {
        let mut schemas = self.schemas.write().unwrap();
        schemas.retain(|folded, _| listing.schemas.contains_key(folded));
        for (folded, schema) in &listing.schemas {
            match schemas.get(folded) {
                Some(held) => held.relist(&schema.relations),
                None => {
                    schemas.insert(
                        folded.clone(),
                        Arc::new(DbSchemaProvider {
                            catalog: self.catalog.clone(),
                            schema: schema.name.clone(),
                            pool: Arc::clone(&self.pool),
                            relations: RwLock::new(schema.relations.clone()),
                            built: Mutex::new(BTreeMap::new()),
                        }),
                    );
                }
            }
        }
    }
}

impl fmt::Debug for DbCatalogProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbCatalogProvider")
            .field("catalog", &self.catalog)
            .field(
                "schemas",
                &self.schemas.read().unwrap().keys().collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl CatalogProvider for DbCatalogProvider {
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

/// One remote schema: the relations the enumeration found, and a lazily built, cached
/// `TableProvider` per relation.
///
/// **Building a provider costs a remote introspection**, so it happens on first *use* and is
/// then kept for the life of the connection. Diagnostics validate a buffer on every catalog
/// epoch, so without the cache a query mentioning a remote table would introspect it per
/// keystroke. A ↻ re-runs the registration pass, which re-connects — that, and nothing else, is
/// the refresh.
struct DbSchemaProvider {
    catalog: String,
    schema: String,
    pool: Arc<PostgresConnectionPool>,
    relations: RwLock<BTreeMap<String, Relation>>,
    built: Mutex<BTreeMap<String, Arc<dyn TableProvider>>>,
}

impl DbSchemaProvider {
    /// Adopt a fresh relation list, dropping the cached provider of anything no longer in it —
    /// see [`DbCatalogProvider::adopt`]. A relation that survives keeps its provider, which is
    /// the whole reason the cache is kept across a refresh at all.
    fn relist(&self, relations: &BTreeMap<String, Relation>) {
        *self.relations.write().unwrap() = relations.clone();
        self.built
            .lock()
            .unwrap()
            .retain(|folded, _| relations.contains_key(folded));
    }

    /// The relation `name` names, in the server's own spelling.
    fn relation(&self, name: &str) -> Option<Relation> {
        self.relations
            .read()
            .unwrap()
            .get(&fold_ident(name))
            .cloned()
    }
}

impl fmt::Debug for DbSchemaProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbSchemaProvider")
            .field("catalog", &self.catalog)
            .field("schema", &self.schema)
            .field("relations", &self.relations.read().unwrap().len())
            .finish()
    }
}

#[async_trait]
impl SchemaProvider for DbSchemaProvider {
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
        let provider = federate::table_provider(
            &self.pool,
            &self.catalog,
            TableReference::partial(self.schema.clone(), relation.name.clone()),
        )
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
    /// provider, and therefore run a remote introspection, per remote relation.
    ///
    /// With it, `information_schema.tables` and `SHOW TABLES` cost **zero** remote calls.
    /// `information_schema.columns` still builds providers, because a column list is genuinely
    /// the schema; that is bounded by the cache above (once per relation per connection) and
    /// accepted.
    async fn table_type(&self, name: &str) -> DfResult<Option<TableType>> {
        Ok(self.relation(name).map(|relation| relation.table_type()))
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
