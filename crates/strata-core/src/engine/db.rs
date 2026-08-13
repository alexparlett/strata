//! Databases: turning a [`ConnectionDef`] with a [`PgStore`] into a live connection pool and
//! registering it on the session as a **catalog** (DB workstream, `docs/CONNECTIONS_SPEC.md`).
//!
//! The sibling of [`store`](super::store), and deliberately not a path through it. An object
//! store is registered per bucket and answers about *files*; a database is registered as a
//! catalog and answers about *relations* — different registry, different key, different
//! failure. What the two share is the connection def, the `Reg` row it settles onto, the
//! registration pass's first phase, and the all-or-nothing contract
//! ([`connect::settle`](super::connect::settle)).
//!
//! **The whole database comes through, and nothing is declared per table.** Connect enumerates
//! every schema the role can see and every relation in them, in one round trip, and registers a
//! catalog whose providers are built lazily on first use. There are no per-table defs and no
//! manual adds: *discovery gets catalogs, declaration gets defs*. A bucket cannot say what its
//! tables are — somebody must declare globs, a format and its options, and that declaration can
//! fail, which is what the `Reg` rows exist to show — while a database answers for itself. A
//! def per remote table would restate configuration the server owns, go stale silently, cost an
//! introspection per def per pass, and mint failure states for things whose only real failure is
//! the connection's. Pinning one remote table into the workspace is a **view**
//! (`CREATE VIEW orders AS SELECT * FROM pg.public.orders`), which needs no new machinery at
//! all.
//!
//! **Ours rather than the provider crate's `DatabaseCatalogProvider`**, for three reasons all
//! read out of its source: it snapshots the schema and table list at construction (so a ↻ could
//! not refresh it), it builds plain `SqlTable`s with the default unparser dialect, and it skips
//! the federation wrapper — so the generic path would silently forfeit exactly the pushdown this
//! workstream exists for. Ours enumerates at connect, lists lazily, and builds every provider
//! through `PostgresTableFactory`, dialect and federation included.
//!
//! **This module holds a password for the length of one login, and never stores one.** The
//! def says only that a password is expected (`PgPassword::Keystore`); the value is read from
//! the OS keystore per pool connection, inside [`KeystorePassword`], under a
//! [reference derived](crate::secret::SecretRef::derived) from the connection's own identity. It
//! is the seam, not a fact about this module: [`connect`] takes the provider as an argument, so
//! passwordless authentication is `None` rather than a mode this module has to know about, and
//! `Engine::connect` is the only caller that builds the keystore-backed one.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::error::Error;
use std::fmt;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use datafusion::catalog::{CatalogProvider, SchemaProvider, TableProvider};
use datafusion::common::{exec_err, DataFusionError, Result as DfResult};
use datafusion::logical_expr::TableType;
use datafusion::prelude::*;
use datafusion::sql::TableReference;
use datafusion_table_providers_common::sql::db_connection_pool::PasswordProvider;
use datafusion_table_providers_common::util::secrets::to_secret_map;
use datafusion_table_providers_common::UnsupportedTypeAction;
use datafusion_table_providers_postgres::pool::{self, PostgresConnectionPool};
use datafusion_table_providers_postgres::PostgresTableFactory;
use secrecy::SecretString;
use tokio::task::spawn_blocking;

use strata_model::{check_catalog_name, parse_pg_address, ConnectionDef, PgStore};

use super::connect::{self, Registration};
use super::fold_ident;
use super::providers::deregister_catalog;
use crate::secret::SecretRef;

/// The keystore family every database password is filed under — the `kind` half of
/// [`SecretRef::derived`]. One string, here, because the editor's put and this module's read
/// have to land on the same slot.
pub const PG_PASSWORD: &str = "pg-password";

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
    /// What DataFusion calls this relation — the answer `information_schema.tables` and
    /// `SHOW TABLES` print.
    fn table_type(&self) -> TableType {
        match self.relkind.as_str() {
            "v" | "m" => TableType::View,
            _ => TableType::Base,
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
    /// Held for its `Drop`, never read. Each pooled connection has a driver task spawned on the
    /// engine runtime, and that task ends when its client is dropped — so on a Forget, dropping
    /// this handle is what ends them, and on window close the engine's own
    /// `shutdown_background` does. Which is why the pool lives on the engine and not inside a
    /// task the engine's `Drop` is supposed to abort.
    _pool: Arc<PostgresConnectionPool>,
    /// The connection's def, so a later connect can ask [`check_catalog_name`] which names are
    /// already taken **on the session** — a live fact this map owns, where the editor asks the
    /// same question of the project's stored defs.
    def: ConnectionDef,
    /// The connect-time enumeration, shared with the catalog provider so
    /// [`Engine::db_listing`](super::Engine::db_listing) reads what was registered rather than
    /// asking the server again.
    listing: Arc<Listing>,
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
    /// Empty for [`SchemaVisibility::EnabledButMissing`] — there is nothing to list.
    pub relations: Vec<Relation>,
    pub visibility: SchemaVisibility,
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
/// [`Engine::connect`](super::Engine::connect) hands in [`KeystorePassword`], and the
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
            // **Register first, then take back what this URL held before**, which is the
            // opposite order to the obvious one and the reason is a window: `take_back`
            // deregisters, so deregistering first leaves the catalog name unbound for as long as
            // this thread takes to re-register it, and a plan resolving `pg.public.orders` on
            // another runtime thread in that gap fails with "failed to resolve catalog" for a
            // connection that was live before the refresh and live after it. The object-store
            // arm has no such gap — `register_object_store` replaces under one key — so this is
            // the arm that has to arrange it.
            //
            // Replacing the map entry hands back what it displaced, which is what makes the
            // rename case fall out: a def whose catalog *name* moved while its URL stood still
            // leaves an old name registered that today's def cannot name, and the displaced
            // `Live` is the only thing that still knows it.
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
    // The def's own rules, from the def's own module — the same call the connection editor
    // makes, so a name refused at the field is refused here in the same words.
    conn.provider.check_address(&conn.address)?;
    // Folded against what is **registered**, not against what is stored: a connection that
    // failed to connect reserves no catalog name, and the editor is where a stored clash is
    // warned about before anything is dialled.
    check_catalog_name(&dbs.peers(&url), conn)?;

    let pool = build_pool(conn, pg, passwords).await?;
    let listing = Arc::new(enumerate(&pool).await?);
    let factory = Arc::new(PostgresTableFactory::new(Arc::clone(&pool)));
    // The user's own spelling, not a folded one: this is what a message names the connection
    // by and what a surface prints. Case-insensitive resolution is the *registry's* job —
    // `StrataCatalogList` folds on the way in and on every lookup, exactly as
    // `StrataSchemaProvider` does for table names.
    let catalog = pg.catalog.trim().to_string();
    let provider = Arc::new(DbCatalogProvider::new(
        catalog.clone(),
        factory,
        Arc::clone(&listing),
    ));
    Ok((
        Prepared {
            name: catalog.clone(),
            provider,
        },
        Live {
            catalog,
            _pool: pool,
            def: conn.clone(),
            listing,
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
    // A `BTreeSet` rather than a `Vec`, so membership is a lookup rather than a scan per server
    // schema, and so a def that names one schema twice cannot produce two rows for it.
    let enabled: BTreeSet<String> = pg.schemas.iter().map(|s| fold_ident(s)).collect();
    let mut views: Vec<SchemaListingView> = listing
        .schemas
        .iter()
        .map(|(folded, schema)| SchemaListingView {
            name: schema.name.clone(),
            relations: schema.relations.values().cloned().collect(),
            visibility: match enabled.contains(folded) {
                true => SchemaVisibility::Live,
                false => SchemaVisibility::NotEnabled,
            },
        })
        .collect();
    // A schema the def shows and the server does not have is its own answer rather than a
    // silent absence: it is what a dropped or renamed schema looks like, and the picker has to
    // be able to show the entry the user is about to un-tick.
    //
    // Driven off the **folded set**, not off `pg.schemas` beside a parallel array of its folds:
    // `fold_ident` answers a name it cannot read as one identifier verbatim, so a schema needing
    // quotes (`Sales Ops`) folds differently from the def's own spelling of it and the two lists
    // stop lining up — which emitted the same schema twice, once `NotEnabled` and once
    // `EnabledButMissing`.
    views.extend(
        enabled
            .iter()
            .filter(|folded| !listing.schemas.contains_key(*folded))
            .map(|folded| SchemaListingView {
                // The def's own spelling of the missing schema, which is the only one anyone
                // has: the server never answered for it.
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

// --- the pool ----------------------------------------------------------------------------

/// The pool itself, with every failure turned into a sentence naming what to fix.
async fn build_pool(
    conn: &ConnectionDef,
    pg: &PgStore,
    passwords: Option<Arc<dyn PasswordProvider>>,
) -> Result<Arc<PostgresConnectionPool>, String> {
    // **The def's own rules, from the def's own module, and its own parse.** All four values
    // below are interpolated into a libpq connection string with no quoting, so each is refused
    // by name here rather than mangled by the driver's parser (`parse_pg_address` and
    // `check_user` share that rule). Splitting the address a second time in this crate would put
    // one grammar in two places — the IPv6 bracket rule is exactly what lands in one copy and not
    // the other — and its refusals would be unreachable prose, since `prepare` has already run
    // `check_address` before this is called.
    let address = parse_pg_address(conn.address.trim())?;
    pg.check_user()?;
    let user = pg.user.trim();
    // The crate's own parameter names. **No `pass` key, ever** — with a provider installed it
    // is ignored, and without one the password would be in a connection string this process
    // formats; the whole point of the provider seam is that neither happens.
    let mut params = HashMap::from([
        ("host".to_string(), address.host.to_string()),
        ("port".to_string(), address.port.to_string()),
        ("db".to_string(), address.database.to_string()),
        ("user".to_string(), user.to_string()),
        ("sslmode".to_string(), pg.sslmode.as_str().to_string()),
        // What the server's own `pg_stat_activity` shows, so a DBA looking at a busy database
        // can see which client is asking.
        ("application_name".to_string(), "Strata".to_string()),
    ]);
    // Only for the modes that read it: the crate refuses a path that does not exist, and a
    // stale one left on the def by switching modes must not fail a connection it has nothing
    // to do with.
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
        // **`jsonb` and every other exotic type arrive as text rather than as a refusal.** The
        // crate's default (`Error`) makes a table with one such column entirely unreadable;
        // `String` hands it over as `Utf8` JSON text, which the app's own Postgres-style
        // accessors (`json_get`, `->`, `->>`) already read. Representation honesty rather than
        // silent corruption: the value is intact, only the type is wider.
        .map(|pool| Arc::new(pool.with_unsupported_type_action(UnsupportedTypeAction::String)))
        .map_err(|e| refused(conn, pg, e))
}

/// Why a login did not happen, in the terms of the thing to fix.
///
/// The crate's own prose is good and is kept wherever it already names the fault; the two arms
/// rewritten here are the ones it cannot word as well as we can, because it does not know there
/// is a connection editor behind them. Nothing in any of it is a password: the crate builds its
/// connection string without one on purpose, and our own provider's failure is
/// [`KeystorePassword`]'s sentence.
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
        // Ours already, and already a sentence about this machine — see [`KeystorePassword`].
        pool::Error::PasswordProviderError { source } => source.to_string(),
        other => format!("Cannot connect to '{}': {other}", conn.url()),
    }
}

/// The keystore behind one connection's password: read **per new pool connection**, never
/// cached, never held past the login it is for.
///
/// The reference is [derived](SecretRef::derived) from the connection's URL rather than stored
/// on the def, so the committed `project.json` carries no machine-local id — see
/// [`PgPassword`](strata_model::PgPassword). Which means the ordinary answer on a colleague's
/// machine is *there is no entry*, and that is a sentence rather than a fault: they enter the
/// password once, into their own keystore, and nothing in git changes.
///
/// The read is blocking (a keystore call is a platform call that can wait on a lock or a user
/// prompt), so it goes through `spawn_blocking` rather than stalling a runtime worker while bb8
/// opens a connection.
pub(crate) struct KeystorePassword {
    key: SecretRef,
    /// For the message alone. The URL rather than the host, because that is what the row and
    /// the editor both name a connection by.
    url: String,
}

impl KeystorePassword {
    /// The provider for `url`'s password, addressing the slot that URL derives.
    pub(crate) fn new(url: String) -> Self {
        Self {
            key: SecretRef::derived(PG_PASSWORD, &url),
            url,
        }
    }
}

#[async_trait]
impl PasswordProvider for KeystorePassword {
    async fn get_password(&self) -> Result<SecretString, Box<dyn Error + Send + Sync>> {
        let key = self.key.clone();
        let read = spawn_blocking(move || key.get())
            .await
            .map_err(|e| format!("Reading the password for '{}' failed: {e}", self.url))?;
        match read.map_err(|e| format!("{e}"))? {
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

// --- enumeration -------------------------------------------------------------------------

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

    // **A name that folds onto one already here is skipped, not merged.** The namespace is the
    // *server's*, and Postgres identifiers are case-sensitive when quoted, so `Foo` and `foo`
    // really are two schemas — while SQL folds an unquoted `pg.foo.t` to one of them and this
    // map can only hold one. Merging is the actively wrong answer: it would keep the first
    // schema's name and file the second's relations under it, so a scan would unparse to
    // `FROM "Foo"."t"` and read a relation the query did not name. First wins (the query is
    // ordered, so that is stable), and the loser is logged rather than silently absent.
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
            // Put the first one back: same rule, one level down.
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

// --- the providers -----------------------------------------------------------------------

/// One database, as DataFusion sees it: the schemas the connect-time enumeration found, each a
/// [`DbSchemaProvider`].
///
/// Read-only, and it says so rather than leaning on the trait's default refusal — the sentence
/// a user gets should name the connection they are addressing, not "catalog provider".
struct DbCatalogProvider {
    catalog: String,
    schemas: BTreeMap<String, Arc<DbSchemaProvider>>,
}

impl DbCatalogProvider {
    fn new(catalog: String, factory: Arc<PostgresTableFactory>, listing: Arc<Listing>) -> Self {
        let schemas = listing
            .schemas
            .iter()
            .map(|(folded, schema)| {
                (
                    folded.clone(),
                    Arc::new(DbSchemaProvider {
                        catalog: catalog.clone(),
                        schema: schema.name.clone(),
                        factory: Arc::clone(&factory),
                        relations: schema.relations.clone(),
                        built: Mutex::new(BTreeMap::new()),
                    }),
                )
            })
            .collect();
        Self { catalog, schemas }
    }
}

impl fmt::Debug for DbCatalogProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbCatalogProvider")
            .field("catalog", &self.catalog)
            .field("schemas", &self.schemas.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl CatalogProvider for DbCatalogProvider {
    fn schema_names(&self) -> Vec<String> {
        self.schemas
            .values()
            .map(|schema| schema.schema.clone())
            .collect()
    }

    fn schema(&self, name: &str) -> Option<Arc<dyn SchemaProvider>> {
        self.schemas
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
    factory: Arc<PostgresTableFactory>,
    relations: BTreeMap<String, Relation>,
    built: Mutex<BTreeMap<String, Arc<dyn TableProvider>>>,
}

impl fmt::Debug for DbSchemaProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DbSchemaProvider")
            .field("catalog", &self.catalog)
            .field("schema", &self.schema)
            .field("relations", &self.relations.len())
            .finish()
    }
}

#[async_trait]
impl SchemaProvider for DbSchemaProvider {
    fn table_names(&self) -> Vec<String> {
        self.relations
            .values()
            .map(|relation| relation.name.clone())
            .collect()
    }

    async fn table(&self, name: &str) -> DfResult<Option<Arc<dyn TableProvider>>> {
        let Some(relation) = self.relations.get(&fold_ident(name)) else {
            return Ok(None);
        };
        if let Some(built) = self.built.lock().unwrap().get(&fold_ident(name)) {
            return Ok(Some(Arc::clone(built)));
        }
        // Built outside the lock: this is a network round trip, and holding a `std` mutex
        // across it would serialize every other relation in the schema behind it. Two callers
        // racing build the same provider twice and the second insert wins, which costs one
        // extra introspection and is correct either way.
        let provider = self
            .factory
            .table_provider(TableReference::partial(
                self.schema.clone(),
                relation.name.clone(),
            ))
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
        Ok(self
            .relations
            .get(&fold_ident(name))
            .map(Relation::table_type))
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
        self.relations.contains_key(&fold_ident(name))
    }
}
