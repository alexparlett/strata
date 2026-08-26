//! Data sources the engine can connect to, and the registry it looks one up in.
//!
//! [`DataSource`] is the seam: implement it, register it with
//! [`EngineBuilder::with_source`](crate::EngineBuilder::with_source), and connections naming its
//! [`SourceKind::NAME`] connect, enumerate, resolve and query like the shipped ones. Its
//! vocabulary is generic — every method is something any source can answer — so a document store
//! implements it as readily as a SQL server. What is SQL-shaped is [`sql`](super::sql), an
//! assembly a SQL-speaking source composes in one call and no source is required to.
//!
//! **Connecting yields the mode.** A source is something you connect to that answers with either
//! an object store or a catalog of relations ([`Sourced`]), and the mode-specific vocabulary rides
//! that sum's arms rather than the trait: a bucket is never asked to enumerate, because the method
//! is not there, and a connected [`SourceCatalog`] holds its own handle to the source.
//!
//! The shipped sources are ordinary registrants: each rides a cargo feature that gates its module
//! and its dependency tree, and registers through the same public call an embedder makes.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::TableProvider;
use datafusion::sql::TableReference;
use object_store::ObjectStore;

use crate::ddl::RemoteTarget;
use crate::fold_ident;
use crate::secrets::SecretProvider;
use strata_model::ConnectionDef;

/// Names a source for the registry, and for the surfaces that offer it.
///
/// A companion trait rather than methods on [`DataSource`], because an associated const is not
/// dyn-compatible: the consts are read once, where the concrete type is still in hand, so a source
/// cannot answer differently from the key it was filed under.
pub trait SourceKind {
    /// What connection defs call this source.
    ///
    /// A short lowercase word. It is also the URL scheme a connection's identity is composed
    /// from, and the prefix of the keystore family each of its secrets is filed under.
    const NAME: &'static str;
    /// What a person calls it — `PostgreSQL`, `MySQL`.
    const LABEL: &'static str;
    /// The short word a catalog row wears — `PG`.
    const BADGE: &'static str;
    /// What connecting to it yields, which a form has to know before anything connects.
    const MODE: SourceMode;
}

/// What connecting to a source yields.
///
/// Declared on [`SourceKind::MODE`] because the editor must draw a form before it can connect;
/// the conformance suite asserts a source's [`Sourced`] arm matches what it declared, so the
/// const cannot lie.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceMode {
    /// An object store: files, addressed by path, described by a table def.
    Store,
    /// A catalog: relations the source names itself.
    Catalog,
}

/// What connecting produced.
#[derive(Debug)]
pub enum Sourced {
    /// An object store, registered under `scheme://<address>`, which table defs then read paths
    /// through.
    Store {
        store: Arc<dyn ObjectStore>,
        scheme: &'static str,
    },
    /// A live catalog handle, holding whatever it reaches its source through.
    Catalog(Arc<dyn SourceCatalog>),
}

/// One setting a source declares.
///
/// The editor renders these rows rather than knowing any source's fields, and per-key validation
/// derives from the [`Field`]; what a value *means* is judged by
/// [`connect`](DataSource::connect), which is the real gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConnectionKey {
    /// The key it is stored under in [`SourceDef::config`](strata_model::SourceDef), or — for a
    /// [`Field::Secret`] — the key it is filed under in the keystore.
    pub key: &'static str,
    /// The row's label, in the editor's register.
    pub label: &'static str,
    pub field: Field,
    pub required: bool,
    /// What the value is when the connection says nothing.
    pub default: Option<&'static str>,
}

/// What kind of value a [`ConnectionKey`] takes, and therefore what the editor draws for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Text,
    /// A credential. The **value never reaches the def**: it goes to this machine's keystore
    /// under `SecretRef::derived("{kind}-{key}", url)`, or arrives through the source's own
    /// environment convention, and the def records only that it is set.
    Secret,
    /// One of a fixed set of words, in the source's own spelling.
    Choice(&'static [&'static str]),
    /// A path to a file the user owns — a certificate, a key file. The path, never the contents.
    Path,
    Flag,
}

/// One relation a source is asked to act on.
#[derive(Clone, Debug, PartialEq)]
pub struct Located {
    /// The catalog the connection registered under, which is what a refusal about this relation
    /// names.
    pub connection: String,
    /// The connection's identity — its kind and address, as `ConnectionDef` derives it.
    ///
    /// Distinct from [`connection`](Self::connection), which a user can rename: this is what says
    /// two providers read through the same source.
    pub identity: String,
    /// The relation as the source spells it, `schema.relation`.
    pub relation: TableReference,
}

/// One relation a source holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Relation {
    /// The name as the source spells it, which is what a query has to say.
    pub name: String,
    /// Whether the source calls this a view rather than a table.
    ///
    /// One bit rather than the source's own vocabulary for relation kinds, because all that turns
    /// on it is what `SHOW TABLES` and the catalog tree print.
    pub view: bool,
}

/// What one source holds: its namespaces, each with its relations.
///
/// One shape whatever the source, and keyed case-insensitively the way SQL resolves names. A
/// `PostgreSQL` schema, a `MySQL` database and a document store's collections all arrive here.
#[derive(Debug, Default)]
pub struct Listing {
    schemas: BTreeMap<String, SchemaListing>,
}

#[derive(Debug, Default)]
pub(super) struct SchemaListing {
    /// The namespace as the source spells it.
    pub(super) name: String,
    pub(super) relations: BTreeMap<String, Relation>,
}

impl Listing {
    /// Folds `(namespace, relation)` pairs into a listing.
    ///
    /// Two namespaces or two relations whose names differ only in case cannot both be addressed,
    /// so the first wins and the other is logged. That is a property of the keying this type
    /// imposes rather than of any source, which is why every source gets it from here.
    pub fn of(relations: impl IntoIterator<Item = (String, Relation)>) -> Self {
        let mut schemas: BTreeMap<String, SchemaListing> = BTreeMap::new();
        for (schema, relation) in relations {
            let entry = schemas
                .entry(fold_ident(&schema))
                .or_insert_with(|| SchemaListing {
                    name: schema.clone(),
                    relations: BTreeMap::new(),
                });
            if entry.name != schema {
                tracing::warn!(
                    "source: schema '{schema}' is hidden by '{}', which folds to the same SQL \
                     name; its relations are not listed",
                    entry.name
                );
                continue;
            }
            let folded = fold_ident(&relation.name);
            let name = relation.name.clone();
            if let Some(held) = entry.relations.insert(folded.clone(), relation) {
                tracing::warn!(
                    "source: relation '{schema}.{name}' is hidden by '{}', which folds to the \
                     same SQL name",
                    held.name
                );
                entry.relations.insert(folded, held);
            }
        }
        Self { schemas }
    }

    pub(super) fn schemas(&self) -> &BTreeMap<String, SchemaListing> {
        &self.schemas
    }
}

/// What a source does with one of the engine's function names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Support {
    /// The source has a faithful spelling for it, giving the same answer computed there.
    Mapped,
    /// The source has none.
    ///
    /// `why` is a clause naming what to reach for instead, and is empty where there is nothing to
    /// name. [`unsupported_function`] is what words it into a refusal.
    Unmapped { why: String },
}

/// A source's answer for the function names it has an opinion about.
///
/// A name absent from the map is left exactly as it is, so a source only lists the handful its
/// own vocabulary really differs on.
#[derive(Debug, Default)]
pub struct FunctionMap(BTreeMap<&'static str, Support>);

impl FunctionMap {
    /// Returns the map `entries` describe.
    pub fn of(entries: impl IntoIterator<Item = (&'static str, Support)>) -> Self {
        Self(entries.into_iter().collect())
    }

    /// Returns what this source does with `function`, or `None` where it has no opinion.
    pub fn support(&self, function: &str) -> Option<&Support> {
        self.0.get(function)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// What [`SourceCatalog::function_map`] answers by default.
static NO_FUNCTIONS: LazyLock<FunctionMap> = LazyLock::new(FunctionMap::default);

/// The way out every refusal about what a source cannot compute offers.
///
/// One string, because it is the same way out in every case: the data has to be on this side
/// before a function only this side has can be applied to it.
pub const MATERIALIZE: &str = "To use it as it is, copy the rows into the project first with \
                               CREATE TABLE ... AS SELECT ..., and query that table.";

/// Why a source cannot do something its implementation does not provide.
///
/// Every default body below reaches this rather than restating it.
pub fn unsupported(kind: &str, what: &str) -> String {
    format!("A '{kind}' source cannot {what}.")
}

/// Why `function` cannot be sent to `connection`.
///
/// The frame is the engine's for every source, and `why` is the source's own
/// ([`Support::Unmapped`]): a source with an alternative to name says so, and one without says
/// nothing rather than an apology.
pub fn unsupported_function(function: &str, connection: &str, why: &str) -> String {
    let why = match why.is_empty() {
        true => String::new(),
        false => format!(" {why}"),
    };
    format!("'{function}' cannot run on the connection '{connection}'.{why} {MATERIALIZE}")
}

/// One data source the engine can connect to.
///
/// Everything here is answerable before anything is connected — which is what lets the editor draw
/// a form, judge an address and offer the source at all. What a *connected* source can do rides
/// [`Sourced`].
#[async_trait]
pub trait DataSource: Send + Sync + fmt::Debug + 'static {
    /// Opens a live handle to what `def` describes, and **probes it**.
    ///
    /// Read a secret the def expects from `secrets`, through a
    /// [`SecretRequest`](crate::secrets::SecretRequest) naming this source's own family and its
    /// own environment convention — per use, never stored, never held past the login it is for.
    /// The def itself carries no secret value, only the expectation of one.
    ///
    /// # Errors
    ///
    /// If the source cannot be reached or the description is not usable. A connection settles onto
    /// one row and the error **is** that row's sentence, so word it as the thing to fix: this is
    /// all-or-nothing, not a handle that fails at the first query.
    async fn connect(
        &self,
        def: &ConnectionDef,
        secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, String>;

    /// Judges an address by this source's own naming rule.
    ///
    /// Reached by the editor before a connect is attempted, so a mistyped address is refused at
    /// the field rather than by a connection failure. The default refuses only an empty one, which
    /// is the single thing no source can dial.
    ///
    /// # Errors
    ///
    /// If the address is not one this source could dial, in words that name what is wrong with it.
    /// The sentence is shown under the field while the user is still typing, so it must say what
    /// the address should look like rather than what went wrong.
    fn check_address(&self, address: &str) -> Result<(), String> {
        match address.trim().is_empty() {
            true => Err("This connection has no address.".into()),
            false => Ok(()),
        }
    }

    /// The settings this source takes, which the editor renders as rows.
    ///
    /// The values live in [`SourceDef::config`](strata_model::SourceDef), except a
    /// [`Field::Secret`], whose value goes to the keystore. Empty for a source configured by its
    /// address alone.
    fn config_keys(&self) -> &'static [ConnectionKey] {
        &[]
    }
}

/// A connected source that answers with a catalog of relations.
///
/// Three methods have no default, because no catalog can be read without them; the rest default
/// to the honest answer for a source that does not do that thing.
///
/// # Plan-cache identity
///
/// Two connections must never share it. Whatever [`table_provider`](Self::table_provider) hands
/// back has to be distinguishable per connection by whatever the query engine fuses subplans on,
/// or two sources answer each other's queries. Composing
/// [`sql::federated`](super::sql::federated) discharges this; a source writing its own provider
/// carries it itself.
#[async_trait]
pub trait SourceCatalog: Send + Sync + fmt::Debug + 'static {
    /// The kind that opened this handle — [`SourceKind::NAME`], which is what the refusals below
    /// name.
    fn kind(&self) -> &'static str;

    /// Returns what the source holds, in its own spelling.
    ///
    /// Called again whenever a statement changes what the source holds, and by a catalog refresh.
    /// A relation left out is absent from the catalog tree, from completion, and from what a bare
    /// name resolves to.
    ///
    /// # Errors
    ///
    /// If the source could not be asked what it holds. Connecting is all-or-nothing, so on a
    /// connect or a catalog refresh this settles the connection's own row and nothing under it is
    /// registered. The re-read after a statement is the one exception: the previous listing
    /// stands and the failure is logged, because that statement has already succeeded.
    async fn enumerate(&self) -> Result<Listing, String>;

    /// Returns a read provider for one relation, over the source's own read path.
    ///
    /// A SQL-speaking source's whole body is one [`sql::federated`](super::sql::federated) call; a
    /// source with its own query language brings its own provider and compiles what it can push
    /// down into its own shape. The caller builds these lazily and caches one per relation, so a
    /// round trip here is expected.
    async fn table_provider(
        self: Arc<Self>,
        at: &Located,
    ) -> Result<Arc<dyn TableProvider>, String>;

    /// Returns what this source does with the function names it differs on, empty by default.
    fn function_map(&self) -> &FunctionMap {
        &NO_FUNCTIONS
    }

    /// Returns `name` as a statement the source parses may say it.
    ///
    /// The default is SQL's own rule, double-quoted unconditionally with embedded quotes doubled;
    /// override it for a source that spells identifiers another way. Quoted always rather than
    /// only where it is needed, because the reserved words are the source's and no local table
    /// knows them.
    ///
    /// This is the rule for identifiers the engine *composes*. What a user typed travels to the
    /// source exactly as typed, for the source to judge.
    fn server_ident(&self, name: &str) -> String {
        format!("\"{}\"", name.replace('"', "\"\""))
    }

    /// Rewords a failure the source reported, or returns `None` to keep the source's own answer.
    ///
    /// Recognize it by **code** — a `SQLSTATE`, an errno — never by prose: a wording is the
    /// source's to change, and matching words fires on messages where they merely co-occur.
    fn remote_refusal(&self, _raw: &str, _connection: &str) -> Option<String> {
        None
    }

    /// Returns a provider that writes into `at`, wrapping the read provider `read`.
    ///
    /// Built per statement rather than served from the schema provider: a plan that sees a writer
    /// where it expected the read provider forfeits pushdown on every read.
    ///
    /// # Errors
    ///
    /// The default refuses, naming the kind — the answer for a source the engine can read and not
    /// change.
    fn writer(
        &self,
        _read: Arc<dyn TableProvider>,
        _at: &RemoteTarget,
        _schema: SchemaRef,
    ) -> Result<Arc<dyn TableProvider>, String> {
        Err(unsupported(self.kind(), "be written to"))
    }

    /// Creates the relation `at` from `schema`, returning `false` where the source already held it
    /// and nothing was made.
    ///
    /// A `true` is a promise the caller acts on: it is what lets a failed fill take the relation
    /// back off the source without ever removing something it did not create. Answer the existence
    /// question inside the same transaction that creates, or it can go stale between the two.
    ///
    /// # Errors
    ///
    /// The default refuses, naming the kind.
    async fn create_relation(
        &self,
        _at: &RemoteTarget,
        _schema: SchemaRef,
    ) -> Result<bool, String> {
        Err(unsupported(self.kind(), "have relations created in it"))
    }

    /// Takes a relation [`create_relation`](Self::create_relation) reported making back off the
    /// source, on an error and on a cancel.
    ///
    /// # Errors
    ///
    /// If the source refused. The caller logs it rather than reporting it: the error a user is
    /// owed is the fill's, and a cleanup that also failed would replace it with a sentence about
    /// cleanup.
    async fn drop_relation(&self, _at: &RemoteTarget) -> Result<(), String> {
        Err(unsupported(self.kind(), "have relations dropped from it"))
    }

    /// Runs one statement on the source, in the source's own language, and returns the rows it
    /// moved.
    ///
    /// One statement, never a batch: what reaches here is text a user wrote, so a second statement
    /// smuggled past the parser has to be refused by the source rather than run.
    ///
    /// # Errors
    ///
    /// The default refuses, naming the kind.
    async fn execute_text(&self, _text: &str) -> Result<u64, String> {
        Err(unsupported(self.kind(), "run a statement of its own"))
    }
}

/// One registered source, as a surface that offers it sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceInfo {
    pub kind: &'static str,
    pub label: &'static str,
    pub badge: &'static str,
    pub mode: SourceMode,
    /// The settings the editor draws for it — [`DataSource::config_keys`].
    pub keys: &'static [ConnectionKey],
}

/// The sources an engine can serve a connection with, keyed by [`SourceKind::NAME`].
#[derive(Clone, Debug, Default)]
pub struct Sources(BTreeMap<&'static str, Registrant>);

/// One entry: what a surface offers it as, and what serves it.
#[derive(Clone, Debug)]
struct Registrant {
    info: SourceInfo,
    source: Arc<dyn DataSource>,
}

impl Sources {
    /// Adds `source` under its own name, displacing whatever was registered there.
    pub(crate) fn insert<S: DataSource + SourceKind>(&mut self, source: S) {
        self.0.insert(
            S::NAME,
            Registrant {
                info: SourceInfo {
                    kind: S::NAME,
                    label: S::LABEL,
                    badge: S::BADGE,
                    mode: S::MODE,
                    keys: source.config_keys(),
                },
                source: Arc::new(source),
            },
        );
    }

    /// The source serving `kind`, or the sentence a def whose kind nothing answers to settles as.
    pub(crate) fn get(&self, kind: &str) -> Result<Arc<dyn DataSource>, String> {
        self.0
            .get(kind)
            .map(|held| Arc::clone(&held.source))
            .ok_or_else(|| {
                format!(
                    "No source is registered for '{kind}'. Register one with \
                 EngineBuilder::with_source, or change this connection's kind."
                )
            })
    }

    /// Every registered source, in name order — the one read a picker, a badge and a form share.
    pub(crate) fn registrants(&self) -> Vec<SourceInfo> {
        self.0.values().map(|held| held.info.clone()).collect()
    }

    /// What `kind` makes of `address`, or the sentence saying nothing serves that kind.
    pub(crate) fn check_address(&self, kind: &str, address: &str) -> Result<(), String> {
        self.get(kind)?.check_address(address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two namespaces or two relations that fold to one SQL name cannot both be addressed, so the
    /// first wins — decided here, for every source at once.
    #[test]
    fn a_listing_keeps_the_first_of_two_names_that_fold_together() {
        let relation = |name: &str| Relation {
            name: name.to_string(),
            view: false,
        };
        let listing = Listing::of([
            ("public".to_string(), relation("orders")),
            ("public".to_string(), relation("Orders")),
            ("PUBLIC".to_string(), relation("shipments")),
        ]);
        let schemas = listing.schemas();
        assert_eq!(schemas.len(), 1, "one addressable namespace");
        let public = schemas.get("public").expect("the folded key");
        assert_eq!(public.name, "public", "in its own spelling");
        assert_eq!(
            public.relations.get("orders").map(|r| r.name.as_str()),
            Some("orders"),
            "the first spelling holds the folded name"
        );
        assert!(
            !public.relations.contains_key("shipments"),
            "a relation under a shadowed namespace is not listed"
        );
    }

    /// The refusals every default body reaches name the **kind**: a source that cannot be written
    /// to is not a fault to report, it is a source.
    #[test]
    fn the_defaults_refuse_by_naming_the_kind() {
        assert_eq!(
            unsupported("mongo", "be written to"),
            "A 'mongo' source cannot be written to."
        );
        let why = unsupported_function("json_length", "pg", "");
        assert!(why.starts_with("'json_length' cannot run on the connection 'pg'."));
        assert!(why.contains("CREATE TABLE"), "and the way out: {why}");
        assert!(
            unsupported_function("json_get", "pg", "Use '->>' instead.")
                .contains(". Use '->>' instead. To use it"),
            "a source's own clause sits between the two"
        );
    }

    /// An unregistered kind is a sentence naming the fix, because that sentence is the whole of
    /// what the failed connection row shows.
    #[test]
    fn an_unregistered_kind_names_the_fix() {
        let why = Sources::default()
            .get("mongo")
            .expect_err("nothing registered");
        assert!(
            why.contains("'mongo'") && why.contains("with_source"),
            "{why}"
        );
        assert_eq!(
            Sources::default().check_address("mongo", "somewhere"),
            Err(why)
        );
    }
}
