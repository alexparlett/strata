//! **Data sources**: turning a [`ConnectionDef`] whose provider names a source into a live
//! connection and registering it on the session as a **catalog** (`docs/CONNECTIONS_SPEC.md`).
//!
//! The catalog half of this layer. [`store`] is the other, and neither is a path through the
//! other: an object store is registered per bucket and answers about *files*, a source is
//! registered as a catalog and answers about *relations*. What the two share is the connection
//! def, the `Reg` row it settles onto, the pass's first phase, and the all-or-nothing contract —
//! which is what puts them under one roof, and what EA-25 turns into one trait.
//!
//! **This module is the shell, and it knows nothing about any source.** What a source *is* lives
//! behind [`DataSource`](source::DataSource), keyed by the kind a def names; what is here is
//! everything that is true of all of them — connecting and taking back, the catalog providers, the
//! enumeration a refresh replaces, the scoping a picker edits, and the two write statements
//! DataFusion can plan against a relation somebody else owns. The shipped `PostgreSQL` source
//! reaches the engine through the same public registration an embedder's does.
//!
//! **The whole source comes through, and nothing is declared per table** — *discovery gets
//! catalogs, declaration gets defs*. A bucket cannot say what its tables are, so somebody must
//! declare globs and a format, and that declaration can fail; a database answers for itself. A def
//! per remote table would restate configuration the server owns, go stale silently, and mint
//! failure states for things whose only real failure is the connection's. Pinning one remote
//! relation into the workspace is a **view**, which needs no new machinery.
//!
//! **This module holds no secret and stores none.** The def says only which of a source's
//! secret-typed keys are set; the value is read per use by the source itself, from the engine's
//! [`SecretProvider`], under a slot derived from the connection's own identity
//! ([`secret_slot`]).

pub mod providers;
pub mod source;
pub mod sql;
pub(crate) mod store;

#[cfg(feature = "postgres")]
pub mod postgres;

#[cfg(test)]
pub(crate) mod fake;

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use datafusion::catalog::{CatalogProvider, TableProvider};
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::logical_expr::LogicalPlan;
use datafusion::prelude::*;

use strata_model::{check_catalog_name, ColumnInfo, ConnectionDef, SourceDef};

use self::providers::SourceCatalogProvider;
use self::source::{Listing, Relation, SourceCatalog, SourceInfo, Sourced, Sources};
use super::connect::{self, Registration};
use super::fold_ident;
use super::providers::deregister_catalog;
use super::secrets::{SecretProvider, SecretRequest};
use crate::policy::{Locality, TargetFacts};
use crate::statements::Remote;
use crate::CatalogGen;
use strata_core::secret::{migrate_derived, Secret, SecretRef};

/// The keystore slot one of `conn`'s secrets lives in — **the one place the derivation is
/// written**, so a save's put, a source's read and a Forget's delete cannot address different
/// entries.
///
/// One slot per secret-typed key a source declares, filed under `"{kind}-{key}"` over the
/// connection's **name**: a source with two credentials keeps them apart, no source's family
/// collides with another's, and renaming a connection moves its secrets with it because the same
/// funnel does both. Nothing is stored on the def — the reference is recomputed each time one is
/// needed, which is what keeps a machine-local id out of the committed `project.json`.
///
/// `None` for a provider that is not a source.
pub fn secret_slot(
    conn: &ConnectionDef,
    key: &str,
    env: &'static [&'static str],
) -> Option<SecretRequest> {
    let source = conn.provider.source()?;
    Some(SecretRequest {
        family: format!("{}-{key}", source.kind.trim()),
        connection: conn.named(),
        env,
    })
}

/// The keystore entry one of `conn`'s secrets is written to and deleted from — [`secret_slot`]'s
/// key, for the writes this module performs on a save.
pub(crate) fn secret_ref(conn: &ConnectionDef, key: &str) -> Option<SecretRef> {
    secret_slot(conn, key, &[]).map(|slot| slot.key())
}

/// Store `value` as one of `conn`'s secrets on this machine, or clear it when it is empty.
///
/// The **engine owns the write**, because where a secret lives is the kind's decision and the
/// derivation is one line above this one: a surface that composed a slot itself would be a second
/// copy of that rule, free to disagree with the read.
///
/// # Errors
///
/// If the keystore refused, in words suitable for display. A caller reports it and does not
/// save — never answers it by writing the secret somewhere else.
pub fn put_secret(conn: &ConnectionDef, key: &str, value: &str) -> Result<(), String> {
    let Some(slot) = secret_ref(conn, key) else {
        return Ok(());
    };
    match Secret::new(value) {
        Some(secret) => slot.put(&secret).map_err(|e| e.to_string()),
        None => slot.delete().map_err(|e| e.to_string()),
    }
}

/// Forget one of `conn`'s secrets on this machine, leaving the others alone.
///
/// What *remove from this machine* owes for a single declared key: a source with two credentials
/// keeps the one that was not abandoned, and the def's expectation is untouched, so a colleague's
/// own entry still answers.
///
/// # Errors
///
/// If the keystore refused.
pub fn forget_secret(conn: &ConnectionDef, key: &str) -> Result<(), String> {
    let Some(slot) = secret_ref(conn, key) else {
        return Ok(());
    };
    slot.delete().map_err(|e| e.to_string())
}

/// Forget every secret `conn` holds on this machine — the Forget gesture's keystore half, and
/// what a save owes when a connection stops being a source at all.
///
/// Silent about a slot with nothing in it, which is the ordinary case for a connection that never
/// had a secret stored on this machine.
///
/// # Errors
///
/// If the keystore refused.
pub fn forget_secrets(conn: &ConnectionDef) -> Result<(), String> {
    let Some(source) = conn.provider.source() else {
        return Ok(());
    };
    for key in &source.secrets {
        forget_secret(conn, key)?;
    }
    Ok(())
}

/// Move every secret `was` holds to the slots `now` derives — what a **rename** owes, because the
/// slot is derived from the name.
///
/// Beside the derivation on purpose: that a moved name moves the entry is a fact about how the
/// slot is composed, and nothing that composes no slot should have to know it. A no-op where the
/// name did not move.
///
/// # Errors
///
/// If the keystore refused.
pub fn migrate_secrets(was: &ConnectionDef, now: &ConnectionDef) -> Result<(), String> {
    if was.named() == now.named() {
        return Ok(());
    }
    let Some(source) = now.provider.source() else {
        return Ok(());
    };
    for key in &source.secrets {
        if let (Some(from), Some(to)) = (secret_ref(was, key), secret_ref(now, key)) {
            migrate_derived(&from, &to).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

/// What one connection put on the session, and what it takes to tear it down.
///
/// Held by **name** on [`Live`], which is what a Forget is given and what a table def points at.
/// The catalog it registered under is recorded rather than re-derived, so a teardown deregisters
/// what this row actually put on the session — the two agree today, a source's catalog being its
/// connection's name, and a teardown that re-derived one would be trusting that to stay true.
struct LiveSource {
    catalog: String,
    /// The connected source. Held for its `Drop`, and read: a write statement resolves its target
    /// through the catalog and then needs the handle the catalog reads through. A pooled
    /// connection may have a driver task spawned on the engine runtime, and that task ends when
    /// its client is dropped — so on a Forget, dropping this handle is what ends them, and on
    /// window close the engine's own `shutdown_background` does. Which is why the handle lives on
    /// the engine and not inside a task the engine's `Drop` is supposed to abort.
    source: Arc<dyn SourceCatalog>,
    /// The connection's def, so a later connect can ask [`check_catalog_name`] which names are
    /// already taken **on the session** — a live fact this map owns, where the editor asks the
    /// same question of the project's stored defs. It is also what says whether the connection
    /// accepts writes ([`SourceDef::read_only`](strata_model::SourceDef::read_only)).
    def: ConnectionDef,
    /// The latest enumeration — the connect-time one until a statement that changed what the
    /// source holds re-runs it ([`relist`](Live::relist)). Read by
    /// [`Sources::listing`](super::Sources::listing) rather than asking the source again.
    listing: Arc<Listing>,
    /// The namespaces this connection **shows**, shared with the catalog provider — see [`Shown`].
    shown: Shown,
    /// The catalog this connection registered, held so a refresh can hand it a fresh enumeration
    /// without downcasting its way back out of the session's catalog list.
    provider: Arc<SourceCatalogProvider>,
}

/// Which of a connection's namespaces it shows, folded — **one live cell**, shared between the
/// connection and the catalog it registered.
///
/// Shared rather than copied onto each because the Schemas… picker edits the def without
/// reconnecting, so a copy taken at connect is stale the first time it is read. Its reader is
/// [`sql::qualify`](crate::sql), which scopes an **unqualified** name's search to what a
/// connection shows; a name written in full still resolves into any namespace the connection has
/// ([`SourceCatalogProvider::schema`](providers::SourceCatalogProvider) asks the listing, never
/// this).
pub(crate) type Shown = Arc<RwLock<BTreeSet<String>>>;

/// The folded namespace set a def asks for.
fn shown_of(source: &SourceDef) -> BTreeSet<String> {
    source
        .schemas
        .iter()
        .map(|schema| fold_ident(schema))
        .collect()
}

/// The live source connections this engine holds — the [`Connections`](super::Connections)
/// shape, for the same reasons.
///
/// A handle rather than a plain field because [`Sources::connect`](super::Sources::connect) spawns its work onto the
/// engine runtime and that task must not hold the engine itself (the engine's `Drop` is what
/// aborts it). It holds pools, so it must not outlive the runtime they ride: the engine's own
/// field is the last strong reference, and the runtime is shut down after it in `Drop`.
#[derive(Clone, Default)]
pub struct Live(Arc<Mutex<HashMap<String, LiveSource>>>);

impl Live {
    /// The defs of every *other* live source connection — what [`check_catalog_name`] folds a
    /// candidate against.
    fn peers(&self, name: &str) -> Vec<ConnectionDef> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(held, _)| held.as_str() != name)
            .map(|(_, live)| live.def.clone())
            .collect()
    }

    /// The enumeration a connection registered, or `None` if it is not live.
    ///
    /// [`snapshot`]'s **one** read of this map per connection: it answers whether the connection
    /// is live and what it enumerated together, because a teardown landing between two reads
    /// leaves a row claiming to be live over nothing — a state this map never held.
    fn listing(&self, name: &str) -> Option<Arc<Listing>> {
        let held = self.0.lock().unwrap();
        Some(Arc::clone(&held.get(name)?.listing))
    }

    /// Forget the connection called `name`, handing back the catalog name it had registered.
    fn take(&self, name: &str) -> Option<String> {
        self.0.lock().unwrap().remove(name).map(|live| live.catalog)
    }

    /// What a statement needs of the connection registered as `catalog`: its identity, its
    /// connected source, its catalog provider, and whether it accepts writes at all.
    ///
    /// Keyed by the **catalog name** rather than the connection's, because that is what a
    /// statement wrote and what the session's catalog list resolved; folded on both sides, since
    /// a catalog name is
    /// an unquoted identifier ([`StrataCatalogList`](crate::providers::StrataCatalogList)).
    fn at(&self, catalog: &str) -> Option<Connected> {
        let folded = fold_ident(catalog);
        let held = self.0.lock().unwrap();
        let (name, live) = held
            .iter()
            .find(|(_, live)| fold_ident(&live.catalog) == folded)?;
        Some(Connected {
            name: name.clone(),
            kind: live
                .def
                .provider
                .source()
                .map(|s| s.kind.clone())
                .unwrap_or_default(),
            source: Arc::clone(&live.source),
            provider: Arc::clone(&live.provider),
            writable: live.def.provider.source().is_some_and(|s| !s.read_only),
        })
    }

    /// Record a fresh enumeration for the connection called `name` — the half of a refresh the
    /// *map* owns, where
    /// [`SourceCatalogProvider::adopt`](providers::SourceCatalogProvider) is the half the catalog
    /// owns. Both, because [`Sources::listing`](super::Sources::listing) reads this one and a
    /// query resolves through the other.
    fn relist(&self, name: &str, listing: Arc<Listing>) {
        if let Some(live) = self.0.lock().unwrap().get_mut(name) {
            live.listing = listing;
        }
    }

    /// Point this connection's [`Shown`] and its held def at what the stored def now says — the
    /// Schemas… picker's engine half, and the only writer besides [`connect`].
    ///
    /// Both, so the map holds one answer rather than two that can disagree. A no-op for a
    /// connection that is not live: the next connect reads the def anyway.
    pub(crate) fn show(&self, conn: &ConnectionDef) {
        let Some(source) = conn.provider.source() else {
            return;
        };
        let mut held = self.0.lock().unwrap();
        let Some(live) = held.get_mut(&conn.named()) else {
            return;
        };
        *live.shown.write().unwrap() = shown_of(source);
        live.def = conn.clone();
    }

    /// Deregister every live source and drop its handle — the engine's `Drop`, and the only
    /// caller. See the comment there: this has to happen while the engine runtime is still up,
    /// because a pooled connection's own drop can spawn onto it.
    pub(crate) fn shutdown(&self, ctx: &SessionContext) {
        for (_, live) in self.0.lock().unwrap().drain() {
            deregister_catalog(ctx, &live.catalog);
        }
    }

    /// Record `live` under `name`, replacing whatever that name held.
    ///
    /// A **re-connect** displaces its own previous row, and the catalog it registered is that same
    /// name, so `settle` re-registering under it is the whole of the replacement. A **rename** is
    /// not a displacement here — the new name is a new key — and it cannot be: two connections may
    /// share an identity and differ only by name ([`check_catalog_name`] lets them), so nothing
    /// the engine can see tells a renamed connection from a second one to the same server.
    /// Retiring the old name is therefore the renaming gesture's own call to
    /// [`Sources::disconnect`](super::Sources::disconnect), which is what the connection editor's
    /// Save does.
    fn put(&self, name: String, live: LiveSource) {
        self.0.lock().unwrap().insert(name, live);
    }
}

/// One live source connection, as a statement reaches it — see [`Live::at`].
struct Connected {
    /// The connection's own name, which is what [`LiveSource`] is keyed by and therefore what a
    /// refresh has to name to put a new listing back — and what a
    /// [`RemoteSel::Connection`](crate::RemoteSel::Connection) selects on, the name being the
    /// handle.
    name: String,
    /// Which registered kind serves it — what a
    /// [`RemoteSel::Kind`](crate::RemoteSel::Kind) selects on.
    kind: String,
    source: Arc<dyn SourceCatalog>,
    provider: Arc<SourceCatalogProvider>,
    writable: bool,
}

/// One namespace as a surface sees it: what it is called, what is in it, and whether the
/// connection is set to show it.
///
/// **Scoped and tagged here**, so no consumer re-derives visibility from
/// [`SourceDef::schemas`]: the tree, the schema picker and completion all read one answer.
#[derive(Clone, Debug, PartialEq)]
pub struct SchemaListingView {
    pub name: String,
    /// Filled only for [`SchemaVisibility::Live`]. There is nothing to list for an
    /// [`EnabledButMissing`](SchemaVisibility::EnabledButMissing) schema, and nothing that reads
    /// this *may* list a [`NotEnabled`](SchemaVisibility::NotEnabled) one — the tree drops those
    /// before it draws, the picker reads only the name and the tag, and completion offers what
    /// the connection shows. A schema's relation list is the **source's** to size, so cloning one
    /// that every consumer discards is the one avoidable cost in this read.
    pub relations: Vec<Relation>,
    pub visibility: SchemaVisibility,
}

/// One relation inside a source's catalog, as a surface outside the engine sees it —
/// [`Sources::describe_remote`](super::Sources::describe_remote)'s answer.
///
/// Deliberately not a [`TableMeta`](super::TableMeta): that is what a *registration* learned
/// about a def, and a remote relation has no def, no sources and no free row count. What it has
/// is an address, a connection it belongs to, and the schema the connection reports — which is
/// the whole of what a describe can honestly say about it.
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteRelation {
    /// The catalog the connection registered, in that connection's own spelling.
    pub connection: String,
    /// The relation's address inside the source, `schema.table`.
    pub relation: String,
    /// Whether the source calls it a view ([`Relation::view`]), which the listing already knows
    /// and answers for free.
    pub view: bool,
    pub columns: Vec<ColumnInfo>,
}

/// Every connection an engine holds, read as of one moment.
///
/// Answering each question separately would let two of them describe different instants; this is
/// taken under one read and stamped with the [`generation`](Self::generation) it was taken at.
#[derive(Clone, Debug, PartialEq)]
pub struct SourcesSnapshot {
    /// The catalog generation this was read at.
    ///
    /// Key a derived answer on it and re-derive when [`Catalog::generation`](crate::Catalog::generation)
    /// stops matching: connecting, disconnecting and changing which schemas a connection shows all
    /// move it.
    pub generation: CatalogGen,
    /// Every connection, in name order.
    ///
    /// The workspace catalog is not among them — it is the engine's own, addressed as
    /// [`WORKSPACE_CATALOG`](strata_model::WORKSPACE_CATALOG), and nothing connects to it.
    pub sources: Vec<SourceListing>,
    /// Every source this engine can serve a connection with — what
    /// [`Sources::registrants`](super::Sources::registrants) answers.
    ///
    /// Carried so that [`badge`](Self::badge) can answer for a kind this engine has not been asked
    /// to connect yet.
    pub registrants: Vec<SourceInfo>,
}

/// One connection, and what it registered.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceListing {
    /// What the connection is called, which is how every call addresses it.
    pub name: String,
    /// Whether the session holds its registration now: its store resolves paths, or its catalog
    /// resolves names.
    ///
    /// A connection this engine was told about but could not reach is listed with `false` rather
    /// than left out.
    pub live: bool,
    pub detail: SourceDetail,
}

/// What connecting yielded, by mode.
#[derive(Clone, Debug, PartialEq)]
pub enum SourceDetail {
    /// An object store. It enumerates nothing: what it holds is described by the table defs read
    /// through it.
    Store,
    /// A catalog of relations the source names itself.
    Catalog {
        /// The catalog its relations are addressed by.
        ///
        /// Taken from the def, so a connection that has never answered still reports the name a
        /// query would have to write.
        catalog: String,
        /// Its namespaces, scoped and tagged against [`SourceDef::schemas`]. Empty while the
        /// connection is not live.
        schemas: Vec<SchemaListingView>,
    },
}

impl SourcesSnapshot {
    /// The catalogs live connections registered — what a three-part name resolves through now.
    ///
    /// Live, where [`Sources::database_syms`](super::Sources::database_syms) is not: use this to
    /// report what can be reached into, and that to offer the name a query would have to write
    /// whether or not the connection is up.
    pub fn catalog_names(&self) -> Vec<String> {
        self.sources
            .iter()
            .filter(|source| source.live)
            .filter_map(|source| match &source.detail {
                SourceDetail::Catalog { catalog, .. } => Some(catalog.clone()),
                SourceDetail::Store => None,
            })
            .collect()
    }

    /// One connection by name, or `None` for a name this engine has not been told about.
    pub fn source(&self, name: &str) -> Option<&SourceListing> {
        self.sources.iter().find(|source| source.name == name)
    }

    /// The short word `kind` is badged with — its [`BADGE`](source::SourceKind::BADGE).
    ///
    /// A kind nothing is registered for is badged with the kind itself, which is all that can
    /// honestly be said about a connection this build cannot serve.
    pub fn badge(&self, kind: &str) -> String {
        let kind = kind.trim();
        self.registrants
            .iter()
            .find(|info| info.kind == kind)
            .map(|info| info.badge.to_string())
            .unwrap_or_else(|| kind.to_string())
    }
}

/// Read every connection this engine has been told about into one [`SourcesSnapshot`].
///
/// Costs no I/O — every answer is already held — so re-reading it is how a caller refreshes a
/// derived value. Asking a source anything is the registration pass's job.
pub(crate) fn snapshot(
    ctx: &SessionContext,
    registrants: &Sources,
    live: &Live,
    defs: &[ConnectionDef],
    generation: CatalogGen,
) -> SourcesSnapshot {
    let sources = defs
        .iter()
        .map(|def| {
            let name = def.named();
            match def.provider.source() {
                Some(source) => {
                    let enumerated = live.listing(&name);
                    SourceListing {
                        live: enumerated.is_some(),
                        detail: SourceDetail::Catalog {
                            catalog: name.clone(),
                            schemas: enumerated
                                .map(|listing| scoped(&listing, source))
                                .unwrap_or_default(),
                        },
                        name,
                    }
                }
                None => SourceListing {
                    live: store::registered(ctx, &def.identity()),
                    detail: SourceDetail::Store,
                    name,
                },
            }
        })
        .collect();
    SourcesSnapshot {
        generation,
        sources,
        registrants: registrants.registrants(),
    }
}

/// Whether a namespace is one the connection shows, and whether the source has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchemaVisibility {
    /// Enabled on the def, and the source has it.
    Live,
    /// Enabled on the def, and the source does not have it (or the role cannot see it) — a
    /// schema that was dropped or renamed, which the def cannot know about on its own.
    EnabledButMissing,
    /// The source has it and the def does not show it. Still queryable: registration exposes
    /// every namespace, and this scopes display only ([`SourceDef::schemas`]).
    NotEnabled,
}

/// Connect what `conn` describes through the source its kind names, and register what connecting
/// yielded on `ctx` — the source arm of `Sources::connect`.
///
/// **Connecting is the probe**, all-or-nothing exactly as `store::connect` is: a source answers
/// `Ok` only for something it actually reached, so there is no separate `reachable` step here.
///
/// **A kind nothing is registered for is an ordinary failure**, not a parse error and not a
/// panic: the sentence names the fix and settles onto the connection's `Reg` row like any other.
///
/// **Re-connecting replaces, and renaming does not.** A name this map already holds is replaced
/// under that name, catalog included. A *rename* arrives as a new name and leaves the old one
/// registered, because nothing here can tell it from a second connection to the same server —
/// [`check_catalog_name`] lets two defs share an identity and differ only by name. Retiring the
/// old catalog is the renaming gesture's own [`Sources::disconnect`](super::Sources::disconnect),
/// which the connection editor's Save makes; it is **not** redundant with anything here, and
/// dropping it would leave the old catalog resolving for the life of the window.
pub(crate) async fn connect(
    ctx: &SessionContext,
    sources: &Sources,
    live: &Live,
    conn: &ConnectionDef,
    secrets: Arc<dyn SecretProvider>,
) -> Result<(), String> {
    let named = conn.named();
    let registration = match prepare(sources, live, conn, secrets).await {
        Ok(Prepared::Store(registration)) => Ok(registration),
        Ok(Prepared::Catalog {
            name,
            provider,
            row,
        }) => {
            live.put(named.clone(), row);
            Ok(Registration::Catalog(name, provider))
        }
        Err(why) => Err(why),
    };
    connect::settle(ctx, registration, || take_back(ctx, live, &named))
}

/// Remove whatever the connection called `name` last registered, under the catalog name it
/// registered it under. Silent when there is nothing: a first connect, or a def that has never
/// worked.
fn take_back(ctx: &SessionContext, live: &Live, name: &str) {
    if let Some(previous) = live.take(name) {
        deregister_catalog(ctx, &previous);
    }
}

/// What a connect built, by the mode it turned out to be.
enum Prepared {
    /// An object store, which the session registers and this module keeps nothing of: what it
    /// holds is answered by the table defs read through it.
    Store(Registration),
    /// A catalog, its name, and the row this module holds for the life of the connection.
    Catalog {
        name: String,
        provider: Arc<dyn CatalogProvider>,
        row: LiveSource,
    },
}

/// Everything a source connection can be judged on: which kind serves it, its address by that
/// kind's own rule, its catalog name against the session's other sources, and the connect itself.
///
/// Split from [`connect`] the way `store::prepare` is, so the registration is one line with one
/// meaning — but note that unlike the object-store arm the last steps here do reach the source,
/// because a source's description cannot be checked any other way.
async fn prepare(
    sources: &Sources,
    live: &Live,
    conn: &ConnectionDef,
    secrets: Arc<dyn SecretProvider>,
) -> Result<Prepared, String> {
    let named = conn.named();
    let def = conn
        .provider
        .source()
        .ok_or_else(|| format!("'{named}' is not a data source"))?;
    let source = sources.get(def.kind.trim())?;
    source.check_address(&conn.address)?;
    check_catalog_name(&live.peers(&named), conn)?;

    match source.connect(conn, secrets).await? {
        Sourced::Store { store, scheme } => {
            let at = ObjectStoreUrl::parse(format!("{scheme}://{}", conn.address.trim()))
                .map_err(|e| format!("Cannot register '{named}': {e}"))?;
            Ok(Prepared::Store(Registration::ObjectStore(at, store)))
        }
        Sourced::Catalog(handle) => {
            let listing = Arc::new(handle.enumerate().await?);
            let catalog = conn.named();
            let shown: Shown = Arc::new(RwLock::new(shown_of(def)));
            let provider = Arc::new(SourceCatalogProvider::new(
                catalog.clone(),
                conn.identity(),
                Arc::clone(&handle),
                &listing,
                Arc::clone(&shown),
            ));
            Ok(Prepared::Catalog {
                name: catalog.clone(),
                provider: Arc::clone(&provider) as Arc<dyn CatalogProvider>,
                row: LiveSource {
                    catalog,
                    source: handle,
                    def: conn.clone(),
                    listing,
                    shown,
                    provider,
                },
            })
        }
    }
}

/// Forget the catalog a connection registered — the Forget gesture's engine half, and the half
/// an edit that moves a connection's URL also needs.
///
/// Addressed by the connection's **name** like `store::disconnect` is by its identity, and silent
/// about doing nothing for the same reason: a name this engine holds no source for is the ordinary
/// case (every object-store connection, and every source that never connected).
pub(crate) fn disconnect(ctx: &SessionContext, sources: &Live, name: &str) {
    take_back(ctx, sources, name);
}

/// One connection's namespaces, scoped and tagged against [`SourceDef::schemas`].
///
/// Takes the enumeration rather than the [`Live`] map that holds it, so a second read of that map
/// is not expressible here — see [`Live::listing`].
fn scoped(listing: &Listing, source: &SourceDef) -> Vec<SchemaListingView> {
    let enabled: BTreeSet<String> = shown_of(source);
    let mut views: Vec<SchemaListingView> = listing
        .schemas()
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
            .filter(|folded| !listing.schemas().contains_key(*folded))
            .map(|folded| SchemaListingView {
                name: source
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
    views
}

/// Whether the connection registered as `catalog` accepts writes — the inverse of
/// [`SourceDef::read_only`], and `false` for a catalog no live source registered.
///
/// The *answer*, never the refusal: what a user is told about a read-only connection is
/// [`statements::target`](crate::statements::target)'s, beside every other sentence about a
/// remote target.
pub(crate) fn writable(sources: &Live, catalog: &str) -> bool {
    sources.at(catalog).is_some_and(|live| live.writable)
}

/// What a policy decision may turn on about the connection registered as `catalog` — its backend
/// kind and the name it is held under.
///
/// Empty for a catalog no live source registered, which is the honest answer: a
/// [`RemoteScope::Only`](crate::RemoteScope::Only) names connections, and a catalog this map has
/// never heard of is not one of them. Fails closed by construction rather than by a rule anyone
/// has to remember.
pub(crate) fn connection_facts(sources: &Live, catalog: &str) -> TargetFacts {
    match sources.at(catalog) {
        Some(live) => TargetFacts::remote(live.kind, live.name),
        None => TargetFacts {
            locality: Locality::Remote,
            kind: None,
            connection: None,
        },
    }
}

/// `name` as a statement the source registered as `catalog` parses may say it — its own rule
/// ([`SourceCatalog::server_ident`](source::SourceCatalog::server_ident)).
///
/// The standard spelling for a catalog no source registered: this is reached only from inside a
/// statement already dispatched to a live one, and a fallback that quoted nothing would compose a
/// statement rather than refuse to.
pub(crate) fn server_ident(sources: &Live, catalog: &str, name: &str) -> String {
    match sources.at(catalog) {
        Some(live) => live.source.server_ident(name),
        None => format!("\"{}\"", name.replace('"', "\"\"")),
    }
}

/// Append `input`'s rows to the remote relation `at` — the `INSERT` arm's engine half.
///
/// The read provider comes from the **catalog**, which is where it is already built and cached;
/// the source's writer wraps it, drives the sink once and is dropped. The schema the sink
/// validates against is that provider's — the source's own — because DataFusion planned `input`
/// against it.
pub(crate) async fn insert_into(
    ctx: &SessionContext,
    sources: &Live,
    at: &Remote,
    input: &LogicalPlan,
) -> Result<u64, String> {
    let live = connected(sources, &at.connection)?;
    let provider = relation_provider(ctx, at).await?;
    let schema = provider.schema();
    let writer = live.source.writer(provider, at, schema)?;
    crate::sink::append_rows(ctx, writer, input).await
}

/// Create the remote relation `at` from `input`'s schema and fill it — the CTAS arm's engine half.
/// `None` means the source already held the relation and nothing was created, which is the arm's
/// to word (`IF NOT EXISTS`, `OR REPLACE`, or a plain refusal).
///
/// **The rollback is the point of the ordering.** The relation is created, the catalog
/// re-enumerated so it resolves, and only then filled; a fill that fails takes it back off the
/// source and re-enumerates again, so nothing is left holding a name the user thinks has data in
/// it. A **cancel** is the other way out and reaches no error path at all, which is what
/// [`Created`] is for — armed only while the awaits ahead of `settled` can be dropped.
///
/// Whether the relation was already there is the source's to answer inside its own create, rather
/// than by a round trip before it, so the answer cannot go stale in between.
///
/// A refresh that itself fails is logged rather than reported: the statement did what it said, and
/// a ↻ re-runs the registration pass.
pub(crate) async fn create_table_as(
    ctx: &SessionContext,
    live_map: &Live,
    at: &Remote,
    input: &LogicalPlan,
) -> Result<Option<u64>, String> {
    let live = connected(live_map, &at.connection)?;
    let schema = Arc::clone(input.schema().inner());
    if !live.source.create_relation(at, Arc::clone(&schema)).await? {
        return Ok(None);
    }
    let mut created = Created::open(&live, at.clone());
    relist(&live, live_map).await;

    let filled = match relation_provider(ctx, at).await {
        Ok(provider) => match live.source.writer(provider, at, schema) {
            Ok(writer) => crate::sink::append_rows(ctx, writer, input).await,
            Err(why) => Err(why),
        },
        Err(why) => Err(why),
    };
    created.settled();
    if filled.is_err() {
        discard(&live.source, at).await;
        relist(&live, live_map).await;
    }
    filled.map(Some)
}

/// The relation a CTAS created, removed on **every** way out that is not a settled fill — an
/// error, and a **cancel**.
///
/// The cancel is why this is a guard rather than an `if filled.is_err()`: a CTAS is registered as
/// the workspace's in-flight call, so `Workspace::cancel` and a re-press both abort the task, and an
/// aborted task's future is *dropped* at its next await — no error path runs. Without this, every
/// cancelled remote CTAS would leave an empty relation on the source under the name the user
/// chose, and the retry would then refuse it as already existing. `statements::arms::tables::Staging` is the
/// same guard for the local half, for the same reason.
///
/// The removal is **async**, so it is spawned rather than performed: the future is being dropped
/// on the engine runtime, which is where `Handle::current` resolves. Best effort, exactly as the
/// local guard's `remove_dir_all` is — a runtime already shutting down may never run it.
struct Created {
    source: Arc<dyn SourceCatalog>,
    at: Remote,
    armed: bool,
}

impl Created {
    fn open(live: &Connected, at: Remote) -> Self {
        Self {
            source: Arc::clone(&live.source),
            at,
            armed: true,
        }
    }

    /// The awaits that could be cancelled are behind us, so the caller's own paths own the
    /// relation from here — including its deterministic rollback.
    fn settled(&mut self) {
        self.armed = false;
    }
}

impl Drop for Created {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let (source, at) = (Arc::clone(&self.source), self.at.clone());
        match tokio::runtime::Handle::try_current() {
            Ok(rt) => {
                rt.spawn(async move { discard(&source, &at).await });
            }
            Err(e) => tracing::warn!(
                "could not remove '{}' after its CREATE TABLE AS was cancelled ({e}); it is empty",
                self.at.address()
            ),
        }
    }
}

/// Run one statement **on the source** and report the rows it moved — the source's own
/// [`execute_text`](source::SourceCatalog::execute_text), so a source that runs no statement of
/// its own refuses in the trait's own words.
pub(crate) async fn execute_text(sources: &Live, catalog: &str, text: &str) -> Result<u64, String> {
    let live = connected(sources, catalog)?;
    live.source.execute_text(text).await
}

/// Take `at` back off the source, logging a cleanup that itself failed rather than reporting it —
/// the error a user is owed is the fill's, and a sentence about cleanup would replace it.
async fn discard(source: &Arc<dyn SourceCatalog>, at: &Remote) {
    if let Err(why) = source.drop_relation(at).await {
        tracing::warn!(
            "could not remove '{}' after its CREATE TABLE AS did not settle ({why}); it is empty",
            at.address()
        );
    }
}

/// [`relist`] reached by catalog name, for the arms that resolve a target and nothing else — a
/// relation a drop removed loses its cached provider here, which is what keeps a stale one from
/// answering scans for something the source no longer has.
pub(crate) async fn relist_at(sources: &Live, catalog: &str) {
    let Ok(live) = connected(sources, catalog) else {
        return;
    };
    relist(&live, sources).await;
}

/// Re-enumerate `live` and hand the result to both halves that hold one — the map that
/// [`Sources::listing`](super::Sources::listing) reads, and the catalog a query resolves
/// through.
async fn relist(live: &Connected, sources: &Live) {
    match live.source.enumerate().await {
        Ok(listing) => {
            let listing = Arc::new(listing);
            live.provider.adopt(&listing);
            sources.relist(&live.name, listing);
        }
        Err(why) => tracing::warn!(
            "could not re-read the source's schemas after a statement changed them ({why}); \
             refresh the catalog to see it"
        ),
    }
}

/// The live connection registered as `catalog`, or a sentence saying it is not one.
///
/// Unreachable in the app — a write arm gets here only after the session's catalog list resolved
/// the name — and stated anyway, because the headless host and the tests can register a catalog
/// this map has never heard of.
fn connected(sources: &Live, catalog: &str) -> Result<Connected, String> {
    sources
        .at(catalog)
        .ok_or_else(|| format!("'{catalog}' is not a connected database"))
}

/// The read provider for one remote relation, resolved the way a query resolves it.
async fn relation_provider(
    ctx: &SessionContext,
    at: &Remote,
) -> Result<Arc<dyn TableProvider>, String> {
    ctx.table_provider(at.recorded().clone())
        .await
        .map_err(|e| e.to_string())
}

/// **What one snapshot says**, driven through a real engine — membership, liveness, badges and
/// the two name reads, asked once and answered together.
#[cfg(test)]
mod snapshot_tests {
    use std::collections::BTreeMap;

    use strata_model::{Provider, S3Auth, S3Store};

    use super::fake::{fake_def, TestDoc};
    use super::source::SourceKind;
    use super::*;
    use crate::Engine;

    /// A bucket connection whose credentials nothing here can resolve — it registers no store,
    /// which is exactly the state `live` has to be able to say.
    fn bucket(name: &str) -> ConnectionDef {
        ConnectionDef {
            address: "acme-lake".into(),
            name: name.into(),
            provider: Provider::S3(S3Store {
                region: String::new(),
                auth: S3Auth::Ambient,
                ..Default::default()
            }),
            client_config: BTreeMap::new(),
        }
    }

    /// The snapshot is **membership**: a connection the engine was told about is listed whether
    /// or not it went in, in name order, each carrying what it registers and its kind's own
    /// badge.
    #[tokio::test]
    async fn every_connection_told_about_is_listed_live_or_not() {
        let engine = Engine::builder()
            .with_source(TestDoc::holding("fixture", &["orders"]))
            .build();
        let _ = engine
            .sources()
            .connect(fake_def::<TestDoc>("sales", "fixture"))
            .await;
        let _ = engine
            .sources()
            .connect(fake_def::<TestDoc>("void", "nowhere"))
            .await;
        let _ = engine.sources().connect(bucket("lake")).await;

        let snapshot = engine.sources().listing();
        let names: Vec<&str> = snapshot.sources.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["lake", "sales", "void"], "in name order");

        let sales = &snapshot.sources[1];
        assert!(sales.live, "it connected");
        assert_eq!(
            snapshot.badge(TestDoc::NAME),
            TestDoc::BADGE,
            "its kind's own word, asked of the kind"
        );
        let SourceDetail::Catalog { catalog, schemas } = &sales.detail else {
            panic!("a source registers a catalog");
        };
        assert_eq!(catalog, "sales", "addressed by its own name");
        assert_eq!(schemas.len(), 1, "the one schema it enumerated");
        assert_eq!(schemas[0].visibility, SchemaVisibility::Live);

        let void = &snapshot.sources[2];
        assert!(!void.live, "it was refused");
        let SourceDetail::Catalog { catalog, schemas } = &void.detail else {
            panic!("a refused source is still a source");
        };
        assert_eq!(catalog, "void", "and still says what a query would write");
        assert!(schemas.is_empty(), "with nothing enumerated behind it");

        assert_eq!(snapshot.sources[0].detail, SourceDetail::Store);
        assert!(!snapshot.sources[0].live, "no credentials, no store");
    }

    /// **The two name reads are not the same question.** Completion offers the catalog of a
    /// connection that has never answered, because that is the name a query has to write;
    /// the listing of databases an agent is handed is what can be reached into now.
    #[tokio::test]
    async fn completion_offers_what_a_query_writes_and_the_agent_is_told_what_answers() {
        let engine = Engine::builder()
            .with_source(TestDoc::holding("fixture", &["orders"]))
            .build();
        let _ = engine
            .sources()
            .connect(fake_def::<TestDoc>("sales", "fixture"))
            .await;
        let _ = engine
            .sources()
            .connect(fake_def::<TestDoc>("void", "nowhere"))
            .await;
        let _ = engine.sources().connect(bucket("lake")).await;

        let syms = engine.sources().database_syms();
        let offered: Vec<&str> = syms.iter().map(|sym| sym.name.as_str()).collect();
        assert_eq!(offered, ["sales", "void"], "both catalogs, no bucket");
        assert_eq!(
            engine.sources().listing().catalog_names(),
            ["sales".to_string()],
            "only the one that answered"
        );

        let sales = syms
            .iter()
            .find(|sym| sym.name == "sales")
            .expect("the live catalog");
        assert_eq!(sales.schemas.len(), 1);
        assert_eq!(sales.schemas[0].relations.len(), 1, "its one relation");
    }

    /// The picker's Apply writes the session **and** the def this engine retained, so the next
    /// read scopes by what was just chosen rather than by what the def said at connect.
    #[tokio::test]
    async fn showing_a_different_set_of_schemas_moves_the_next_listing() {
        let engine = Engine::builder()
            .with_source(TestDoc::holding("fixture", &["orders"]))
            .build();
        engine
            .sources()
            .connect(fake_def::<TestDoc>("sales", "fixture"))
            .await
            .expect("connect");

        engine.sources().show_schemas("sales", &[]);

        let snapshot = engine.sources().listing();
        let SourceDetail::Catalog { schemas, .. } = &snapshot.sources[0].detail else {
            panic!("a source registers a catalog");
        };
        assert_eq!(
            schemas.iter().map(|s| s.visibility).collect::<Vec<_>>(),
            [SchemaVisibility::NotEnabled],
            "the schema is still there, and no longer shown"
        );
        assert!(
            engine.sources().database_syms()[0].schemas.is_empty(),
            "so completion stops offering its names"
        );
    }
}

/// **What a Forget would leave invalid**, driven through a real pass — both derivations, and the
/// reconciliation that keeps them true.
#[cfg(test)]
mod dependents_tests {
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};
    use std::{env, fs, process};

    use strata_model::{Provider, S3Auth, S3Store, SourceFormat, ViewDef};

    use super::fake::{fake_def, TestDoc};
    use super::*;
    use crate::register::CatalogSpec;
    use crate::{Dependents, Engine, TableSpec};

    /// A scratch folder holding one two-column CSV, so every table below registers for real.
    fn scratch(tag: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("strata_dependents_{}_{tag}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("t.csv"), "id,name\n1,a\n2,b\n").unwrap();
        dir
    }

    /// A table over `root/t.csv` that **names** `connection`.
    ///
    /// Built directly rather than through `table_spec`, which is what composes a remote path onto
    /// the connection's store: the subject here is the field the spec carries, and a bucket is
    /// not something a unit test can read.
    fn table(root: &Path, name: &str, connection: Option<&str>) -> TableSpec {
        TableSpec {
            name: name.into(),
            paths: vec![root.join("t.csv").display().to_string()],
            format: SourceFormat::from_name("csv"),
            partitions: Vec::new(),
            connection: connection.map(str::to_string),
            internal: false,
        }
    }

    fn view(name: &str, sql: &str) -> ViewDef {
        ViewDef {
            name: name.into(),
            sql: sql.into(),
        }
    }

    /// A bucket connection nothing can reach — membership is what this test needs, and a def the
    /// engine was told about is a member whatever the connect answered.
    fn bucket(name: &str) -> ConnectionDef {
        ConnectionDef {
            address: "acme-lake".into(),
            name: name.into(),
            provider: Provider::S3(S3Store {
                region: String::new(),
                auth: S3Auth::Ambient,
                ..Default::default()
            }),
            client_config: BTreeMap::new(),
        }
    }

    /// **An object store's dependents are its tables and the views over them** — including a view
    /// that reaches the bucket only through another view, because DataFusion inlines what a view
    /// reads and the recorded scans are the base tables of both.
    #[tokio::test]
    async fn a_bucket_names_its_tables_and_the_views_behind_them() {
        let root = scratch("bucket");
        let engine = Engine::builder().build();
        engine
            .catalog()
            .sync(
                CatalogSpec {
                    connections: vec![bucket("lake"), bucket("spare")],
                    tables: vec![
                        table(&root, "orders", Some("lake")),
                        table(&root, "users", None),
                    ],
                    views: vec![
                        view("orders_daily", "SELECT * FROM orders"),
                        view("orders_weekly", "SELECT * FROM orders_daily"),
                        view("everyone", "SELECT * FROM users"),
                    ],
                },
                |_| {},
            )
            .await;

        assert_eq!(
            engine.sources().dependents("lake"),
            Dependents {
                tables: vec!["orders".into()],
                views: vec!["orders_daily".into(), "orders_weekly".into()],
            },
            "the table that names it, and both views over that table"
        );
        assert_eq!(
            engine.sources().dependents("spare"),
            Dependents::default(),
            "a connection nothing reads through holds nothing up"
        );
    }

    /// **A name is matched the way SQL matches one**, because that is also what decided whether
    /// the table registered over that store at all.
    #[tokio::test]
    async fn a_connection_is_matched_folded() {
        let root = scratch("folded");
        let engine = Engine::builder().build();
        engine
            .catalog()
            .sync(
                CatalogSpec {
                    connections: vec![bucket("Lake")],
                    tables: vec![table(&root, "orders", Some("Lake"))],
                    views: Vec::new(),
                },
                |_| {},
            )
            .await;

        assert_eq!(engine.sources().dependents("lake").tables, ["orders"]);
    }

    /// **A source's dependents are the views scanning its catalog, and it never has tables** —
    /// no def can name a connection whose relations are discovered rather than declared, so a def
    /// that names one anyway (an edited `project.json`; the Configure picker offers only object
    /// stores) is still not a table over it. Which half is empty is the *kind*, and this is what
    /// pins that the arm is chosen by the connection rather than by what happens to be recorded.
    ///
    /// The two views that must *not* count are the point. `just_local` reads nothing remote. And
    /// `homonym` reads a **workspace table called `sales`**, which is the same word as the
    /// catalog and is matched by the other of `ViewMeta`'s two lists — the split those lists exist
    /// for, and the reason a bare name resolves to the workspace first.
    #[tokio::test]
    async fn a_source_names_the_views_that_scan_through_it() {
        let root = scratch("source");
        let engine = Engine::builder()
            .with_source(TestDoc::holding("fixture", &["orders"]))
            .build();
        engine
            .catalog()
            .sync(
                CatalogSpec {
                    connections: vec![fake_def::<TestDoc>("sales", "fixture")],
                    tables: vec![
                        table(&root, "sales", None),
                        table(&root, "local", Some("sales")),
                    ],
                    views: vec![
                        view("remote_orders", "SELECT * FROM SALES.public.orders"),
                        view("homonym", "SELECT * FROM sales"),
                        view("just_local", "SELECT * FROM local"),
                    ],
                },
                |_| {},
            )
            .await;

        assert_eq!(
            engine.sources().dependents("sales"),
            Dependents {
                tables: Vec::new(),
                views: vec!["remote_orders".into()],
            },
            "the view that reads across it — the catalog part folds, a workspace table sharing \
             its name is not a reader, and a def naming a source is not a table over it"
        );
    }

    /// **The reconciliation is what keeps it true.** A table whose registration *failed* is still
    /// a table that reads through the connection — no deregistration will ever report it, so only
    /// a later pass that stops naming it can take its entry out.
    #[tokio::test]
    async fn a_failed_table_still_counts_and_a_pass_that_drops_it_takes_it_out() {
        let root = scratch("failed");
        let engine = Engine::builder().build();
        let mut broken = table(&root, "orders", Some("lake"));
        broken.paths = vec![root.join("nothing-here.csv").display().to_string()];

        engine
            .catalog()
            .sync(
                CatalogSpec {
                    connections: vec![bucket("lake")],
                    tables: vec![broken],
                    views: Vec::new(),
                },
                |_| {},
            )
            .await;
        assert_eq!(
            engine.sources().dependents("lake").tables,
            ["orders"],
            "a table over the connection is a table over it whether or not it registered"
        );

        engine
            .catalog()
            .sync(
                CatalogSpec {
                    connections: vec![bucket("lake")],
                    tables: Vec::new(),
                    views: Vec::new(),
                },
                |_| {},
            )
            .await;
        assert_eq!(
            engine.sources().dependents("lake"),
            Dependents::default(),
            "and the def is gone, so nothing is left holding the connection up"
        );
    }

    /// A **drop** takes its entry out through the funnel it already goes through, so the confirm
    /// after it does not name what the drop just removed.
    #[tokio::test]
    async fn dropping_a_view_stops_it_being_named() {
        let root = scratch("dropped");
        let engine = Engine::builder().build();
        engine
            .catalog()
            .sync(
                CatalogSpec {
                    connections: vec![bucket("lake")],
                    tables: vec![table(&root, "orders", Some("lake"))],
                    views: vec![view("orders_daily", "SELECT * FROM orders")],
                },
                |_| {},
            )
            .await;
        assert_eq!(engine.sources().dependents("lake").views, ["orders_daily"]);

        engine
            .catalog()
            .drop_view("orders_daily".into())
            .await
            .expect("drop view");

        assert!(
            engine.sources().dependents("lake").views.is_empty(),
            "the view is gone, and so is what it read"
        );
    }
}
