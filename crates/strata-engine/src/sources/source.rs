//! Data sources the engine can connect to, and the registry it looks one up in.
//!
//! [`DataSource`] is the seam: implement it, register it with
//! [`EngineBuilder::with_source`](crate::EngineBuilder::with_source), and data sources naming its
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

use crate::fold_ident;
use crate::secrets::SecretProvider;
use crate::statements::Remote;
use strata_model::SourceDef;

/// Names a source for the registry, and for the surfaces that offer it.
///
/// A companion trait rather than methods on [`DataSource`], because an associated const is not
/// dyn-compatible: the consts are read once, where the concrete type is still in hand, so a source
/// cannot answer differently from the key it was filed under.
pub trait SourceKind {
    /// What data source defs call this source.
    ///
    /// A short lowercase word. It is also the URL scheme a data source's identity is composed
    /// from, and the prefix of the keystore family each of its secrets is filed under.
    const NAME: &'static str;
    /// What a person calls it — `PostgreSQL`, `MySQL`.
    const LABEL: &'static str;
    /// The short word a catalog row wears — `PG`.
    const BADGE: &'static str;
    /// What connecting to it yields, which a form has to know before anything connects.
    const MODE: SourceMode;
    /// The settings whose values must not repeat across this kind's sources.
    ///
    /// **Uniqueness is the kind's own rule**, because whether two sources describing the same
    /// place is a mistake depends entirely on what the place is. Two object stores on one bucket
    /// are: they register one URL between them, so tables under either would resolve through
    /// whichever went in last. Two servers at one address are **ordinary** — four behind an SSM
    /// tunnel differ in credentials and in nothing else, which is why the default is empty and
    /// only the stores declare `&["address"]`.
    ///
    /// The **name** is never in here: it is the identity, and that it is unique is not a kind's
    /// business ([`check_catalog_name`](strata_model::check_catalog_name)).
    const UNIQUE: &'static [&'static str] = &[];
    /// The URL scheme an object store registers under, for a [`SourceMode::Store`] kind.
    ///
    /// A const rather than a field of [`Sourced::Store`] for [`MODE`](Self::MODE)'s own reason:
    /// a table's remote paths are composed onto `scheme://address` at **registration**, which
    /// happens for defs whose source may never have connected. `None` for a catalog kind, which
    /// registers no store and whose relations are addressed by name.
    const SCHEME: Option<&'static str> = None;
    /// Whether Strata can be asked to **change** what this source holds — whether
    /// [`SourceCatalog::writer`] and friends are implemented at all.
    ///
    /// Declared rather than inferred from [`MODE`](Self::MODE), which was only ever a proxy: a
    /// catalog you can read and not write is an ordinary source, and offering it a read-only
    /// toggle is offering a control that can never do anything. Defaults to `false`, so a source
    /// that says nothing is one Strata never writes to; the conformance body checks the claim
    /// against what the handle actually implements, in both directions.
    const WRITABLE: bool = false;
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
    /// An object store, registered under `scheme://<address>` — the scheme being the kind's own
    /// [`SourceKind::SCHEME`], so a connected store and a table's composed paths cannot disagree
    /// about it.
    Store {
        /// The store, registered under `scheme://<address>`.
        store: Arc<dyn ObjectStore>,
    },
    /// A live catalog handle, holding whatever it reaches its source through.
    Catalog(Arc<dyn SourceCatalog>),
}

/// One setting a source declares — **a row of the data source form, entire**.
///
/// The editor renders these rather than knowing any source's fields: what the row is called, what
/// it explains about itself, what it looks like, where its value lands, which section it sits in,
/// and whether it is offered at all. Rendering a row with a label it was given and anything else
/// it invents is the placement law broken where it is hardest to see, because the result compiles
/// and looks finished.
///
/// What a value may *be* is not here. Per-key validation is what the [`Field`] implies — a choice
/// is a picker, so an illegal word is unreachable; a required box is refused empty — and every
/// other rule is the source's own, asked by [`connect`](DataSource::connect), which is the real
/// gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SourceSetting {
    /// What this setting is called in the def, and what a [`When`] elsewhere names it by.
    pub key: &'static str,
    /// The row's label, in the editor's register.
    pub label: &'static str,
    /// What the editor draws for it.
    pub field: Field,
    /// The section this row sits under, `None` for one that sits above them all. A source's rows
    /// are drawn in the order it declares them, so a group's keys are declared together.
    pub group: Option<&'static str>,
    /// Whether the row is refused empty.
    pub required: bool,
    /// What the value is when the data source says nothing.
    pub default: Option<&'static str>,
    /// When this setting is offered at all — `None` for one that always is.
    pub when: Option<When>,
    /// What the row explains about itself — one sentence, no full stop, in the editor's hint
    /// register. `None` for a setting whose label is the whole story.
    pub hint: Option<&'static str>,
    /// Ghost text in the empty box: an example of the value, never a value that could be saved by
    /// accident. Meaningless for [`Field::Choice`] and [`Field::Flag`], which have no empty state.
    pub placeholder: Option<&'static str>,
}

/// A setting that only means something once another has a particular answer.
///
/// A root certificate is read by the verifying SSL modes and by no other, so offering the box
/// beside `disable` is offering a control that cannot do anything. Declared rather than left to
/// the editor, because which values of which key make a setting relevant is the source's own
/// knowledge — the same reason the values themselves are.
///
/// A setting this hides **keeps its value** (moving the deciding key back brings the box back
/// with what was in it) and is **not required of anyone**: a question that is not asked cannot be
/// unanswered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct When {
    /// Another key of the same source. One that names no declared key would hide its row forever,
    /// which the conformance body refuses.
    pub key: &'static str,
    /// The values of [`key`](Self::key) that offer the setting.
    pub values: &'static [&'static str],
}

/// What kind of value a [`SourceSetting`] takes, and therefore what the editor draws for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    /// Free text.
    Text,
    /// A credential. The **value never reaches the def**: it goes to this machine's keystore
    /// under `SecretRef::derived("{kind}-{key}", url)`, or arrives through the source's own
    /// environment convention, and the def records only that it is set.
    Secret,
    /// One of a fixed set of words, in the source's own spelling.
    Choice(&'static [&'static str]),
    /// A path to a file the user owns — a certificate, a key file. The path, never the contents.
    Path,
    /// A checkbox.
    Flag,
}

/// One relation a source is asked to act on.
#[derive(Clone, Debug, PartialEq)]
pub struct Located {
    /// The catalog the data source registered under, which is what a refusal about this relation
    /// names.
    pub source: String,
    /// The data source's identity — its kind and address, as `SourceDef` derives it.
    ///
    /// Distinct from [`data source`](Self::source), which a user can rename: this is what says
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
    Unmapped {
        /// A clause naming what to reach for instead, empty where there is nothing to name.
        why: String,
    },
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

    /// Whether the source has an opinion about any function name.
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

/// Why `function` cannot be sent to `data source`.
///
/// The frame is the engine's for every source, and `why` is the source's own
/// ([`Support::Unmapped`]): a source with an alternative to name says so, and one without says
/// nothing rather than an apology.
pub fn unsupported_function(function: &str, source: &str, why: &str) -> String {
    let why = match why.is_empty() {
        true => String::new(),
        false => format!(" {why}"),
    };
    format!("'{function}' cannot run on the data source '{source}'.{why} {MATERIALIZE}")
}

/// **Why a connect failed**: the sentence the data source's row shows, and — where the source
/// recognised one — the same failure in a form a surface can act on.
///
/// A refusal is a sentence first, so [`String`] converts into one and a source with nothing to add
/// writes `Err("...".into())` exactly as it did before. The facet is the exception, and it earns
/// its place by being what a surface *points with*: a sentence says what is wrong and cannot say
/// which control it is about, and recovering that by matching the prose makes a working fix depend
/// on wording nobody promised to keep.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectRefusal {
    /// Why, in the source's own words — the data source row's whole sentence.
    pub reason: String,
    /// The same failure as a fact, where the source could tell.
    pub fault: Option<ConnectFault>,
}

impl ConnectRefusal {
    /// A refusal the source recognised as a **rejected credential**, naming the declared key
    /// whose value the server would not take.
    ///
    /// `key` is one of this source's own [`SourceSetting::key`]s, which is why it is `&'static
    /// str`: a fault naming something the kind never declared points at no control, and the
    /// declaration is what a surface joins it against.
    pub fn rejected(reason: impl Into<String>, key: &'static str) -> Self {
        ConnectRefusal {
            reason: reason.into(),
            fault: Some(ConnectFault::Rejected { key }),
        }
    }
}

impl From<String> for ConnectRefusal {
    fn from(reason: String) -> Self {
        ConnectRefusal {
            reason,
            fault: None,
        }
    }
}

impl From<&str> for ConnectRefusal {
    fn from(reason: &str) -> Self {
        ConnectRefusal::from(reason.to_string())
    }
}

impl fmt::Display for ConnectRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.reason)
    }
}

/// **What a connect refusal is about**, for a surface to key on rather than read.
///
/// One arm per fact a surface has a control to point at: a failure nothing can be pointed at is a
/// sentence and belongs in [`ConnectRefusal::reason`] alone. Minting one is the *source's* job,
/// off whatever its server said in codes — never off prose, here or anywhere else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectFault {
    /// **The server refused a sign-in the credential `key` names was part of.**
    ///
    /// Not the same as absent: a secret the def expects and this machine does not have never
    /// reaches a server at all, and fails with the source's own no-secret sentence instead. How
    /// precisely it is *this* value that was refused is the server's to say and the source's to
    /// know — `28P01` means the password, MySQL's 1045 does not separate a wrong password from an
    /// unknown user — so a surface pointing with this should word what it draws accordingly.
    Rejected {
        /// The declared [`SourceSetting::key`] whose value was refused.
        key: &'static str,
    },
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
    /// If the source cannot be reached or the description is not usable. A data source settles onto
    /// one row and the error **is** that row's sentence, so word it as the thing to fix: this is
    /// all-or-nothing, not a handle that fails at the first query. A `String` converts into the
    /// refusal, so a source with nothing but that sentence writes one; add a
    /// [`ConnectFault`] where the server said, in codes, something a surface can point at.
    async fn connect(
        &self,
        def: &SourceDef,
        secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, ConnectRefusal>;

    /// Judges an address by this source's own naming rule.
    ///
    /// Reached by the editor before a connect is attempted, so a mistyped address is refused at
    /// the field rather than by a data source failure. The default refuses only an empty one, which
    /// is the single thing no source can dial.
    ///
    /// # Errors
    ///
    /// If the address is not one this source could dial, in words that name what is wrong with it.
    /// The sentence is shown under the field while the user is still typing, so it must say what
    /// the address should look like rather than what went wrong.
    fn check_address(&self, address: &str) -> Result<(), String> {
        match address.trim().is_empty() {
            true => Err("This data source has no address.".into()),
            false => Ok(()),
        }
    }

    /// The settings this source takes, which the editor renders as rows.
    ///
    /// The values live in [`SourceDef::config`](strata_model::SourceDef), except a
    /// [`Field::Secret`], whose value goes to the keystore. Empty for a source configured by its
    /// address alone.
    fn settings(&self) -> &'static [SourceSetting] {
        &[]
    }
}

/// An identifier the engine composes into a statement bound for a **source**, spelled by that
/// source's own rule ([`SourceCatalog::server_ident`]).
///
/// The fourth of the crate's four identifier renderers, and the only one whose rule is not SQL's
/// own or DataFusion's — a `PostgreSQL` server reserves words sqlparser does not, and a MySQL one
/// quotes with backticks. It is a type for the same reason the other three are
/// ([`sql::name`](crate::sql::name)): the four are silently wrong when confused, and a signature
/// naming this one cannot be handed a workspace name by mistake. It lives here rather than beside
/// them because `sources` and `sql` are peers inside this crate and neither may import the other
/// (`boundaries.rs`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerIdent(String);

impl ServerIdent {
    /// SQL's own rule: double-quoted unconditionally, embedded quotes doubled — the default
    /// [`SourceCatalog::server_ident`] supplies.
    pub fn standard(name: &str) -> Self {
        ServerIdent(format!("\"{}\"", name.replace('"', "\"\"")))
    }

    /// A source stating its own spelling of `name` — the escape the other rules refuse, and the
    /// reason this is a type rather than a fifth free function.
    pub fn spelled(rendered: String) -> Self {
        ServerIdent(rendered)
    }

    /// The rendered identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ServerIdent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A connected source that answers with a catalog of relations.
///
/// Three methods have no default, because no catalog can be read without them; the rest default
/// to the honest answer for a source that does not do that thing.
///
/// # Plan-cache identity
///
/// Two data sources must never share it. Whatever [`table_provider`](Self::table_provider) hands
/// back has to be distinguishable per data source by whatever the query engine fuses subplans on,
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
    /// connect or a catalog refresh this settles the data source's own row and nothing under it is
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
    /// The default is SQL's own rule, [`ServerIdent::standard`] — double-quoted unconditionally
    /// with embedded quotes doubled; override it (through [`ServerIdent::spelled`]) for a source
    /// that spells identifiers another way. Quoted always rather than only where it is needed,
    /// because the reserved words are the source's and no local table knows them.
    ///
    /// This is the rule for identifiers the engine *composes*. What a user typed travels to the
    /// source exactly as typed, for the source to judge.
    fn server_ident(&self, name: &str) -> ServerIdent {
        ServerIdent::standard(name)
    }

    /// Rewords a failure the source reported, or returns `None` to keep the source's own answer.
    ///
    /// Recognize it by **code** — a `SQLSTATE`, an errno — never by prose: a wording is the
    /// source's to change, and matching words fires on messages where they merely co-occur.
    fn remote_refusal(&self, _raw: &str, _source: &str) -> Option<String> {
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
        _at: &Remote,
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
    async fn create_relation(&self, _at: &Remote, _schema: SchemaRef) -> Result<bool, String> {
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
    async fn drop_relation(&self, _at: &Remote) -> Result<(), String> {
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
    /// The word the source is registered under.
    pub kind: &'static str,
    /// What it is called, as a picker prints it.
    pub label: &'static str,
    /// Its short badge, as the catalog tree prints it.
    pub badge: &'static str,
    /// Whether connecting it yields a store or a catalog.
    pub mode: SourceMode,
    /// The settings the editor draws for it — [`DataSource::settings`].
    pub settings: &'static [SourceSetting],
    /// Whether it can be written to — [`SourceKind::WRITABLE`], and therefore whether a
    /// data source to it is offered the read-only toggle.
    pub writable: bool,
    /// The settings two of its sources may not share — [`SourceKind::UNIQUE`].
    pub unique: &'static [&'static str],
    /// What its object stores register under — [`SourceKind::SCHEME`].
    pub scheme: Option<&'static str>,
}

/// The **kinds** an engine can serve a data source with, keyed by [`SourceKind::NAME`].
///
/// Named for what it holds — registrants, not sources. A *source* is one thing a project has;
/// this is the set of things one could be.
#[derive(Clone, Debug, Default)]
pub struct Registrants(BTreeMap<&'static str, Registrant>);

/// One entry: what a surface offers it as, and what serves it.
#[derive(Clone, Debug)]
struct Registrant {
    info: SourceInfo,
    source: Arc<dyn DataSource>,
}

impl Registrants {
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
                    settings: source.settings(),
                    writable: S::WRITABLE,
                    unique: S::UNIQUE,
                    scheme: S::SCHEME,
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
                 EngineBuilder::with_source, or change this source's kind."
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

    /// Whether `candidate` repeats a setting its kind says two of its sources may not share.
    ///
    /// `existing` is what to fold it against, **`candidate` excluded by the caller** — the
    /// project's stored defs for the editor, the live sources for a connect. The sentence is
    /// minted here from the declaration (the kind's `LABEL`, the setting's `label`), so no source
    /// writes prose and every kind refuses in the same words.
    ///
    /// # Errors
    ///
    /// Naming the source already holding those values, and which settings they are.
    pub(crate) fn check_unique(
        &self,
        candidate: &SourceDef,
        existing: &[SourceDef],
    ) -> Result<(), String> {
        let Some(held) = self.0.get(candidate.kind.trim()) else {
            return Ok(());
        };
        let info = &held.info;
        if info.unique.is_empty() {
            return Ok(());
        }
        let same = |other: &SourceDef| {
            other.kind.trim() == candidate.kind.trim()
                && info.unique.iter().all(|key| {
                    other
                        .setting(key)
                        .eq_ignore_ascii_case(candidate.setting(key))
                })
        };
        let Some(other) = existing.iter().find(|other| same(other)) else {
            return Ok(());
        };
        let labels: Vec<&str> = info
            .unique
            .iter()
            .map(|key| {
                info.settings
                    .iter()
                    .find(|declared| declared.key == *key)
                    .map_or(*key, |declared| declared.label)
            })
            .collect();
        Err(format!(
            "The source '{}' is already a {} with the same {}. Point this one somewhere else, or \
             edit that one.",
            other.named(),
            info.label,
            labels.join(" and ")
        ))
    }

    /// What connecting to `kind` yields, or `None` for a kind nothing is registered for.
    ///
    /// The registry answers, because the mode is declared on the kind — so "does this data source
    /// have a catalog" is a question with a registry behind it rather than a field on the def that
    /// could disagree with the source serving it.
    pub(crate) fn mode(&self, kind: &str) -> Option<SourceMode> {
        Some(self.0.get(kind.trim())?.info.mode)
    }

    /// What `kind` registers its object stores under, or `None` for a kind that registers none.
    ///
    /// The registry answers because the scheme is the *kind's*, and a table's remote paths are
    /// composed before anything connects — so this is the one lookup that turns a def into the
    /// `scheme://authority` its files hang off.
    pub(crate) fn scheme(&self, kind: &str) -> Option<&'static str> {
        self.0.get(kind.trim())?.info.scheme
    }

    /// The `scheme://authority` a source's remote paths hang off, or `None` for a source whose
    /// relations are addressed by name rather than read as files.
    ///
    /// An address that already carries a scheme **is** the prefix: an HTTP source is addressed by
    /// a whole origin, and only the person who typed it knows whether their server speaks `http`
    /// or `https`, so the kind's own scheme is what the *other* stores hang their paths off and
    /// nothing this one needs.
    pub(crate) fn prefix(&self, def: &SourceDef) -> Option<String> {
        let scheme = self.scheme(&def.kind)?;
        let address = def.setting("address");
        match address.contains("://") {
            true => Some(address.to_string()),
            false => Some(format!("{scheme}://{address}")),
        }
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
        assert!(why.starts_with("'json_length' cannot run on the data source 'pg'."));
        assert!(why.contains("CREATE TABLE"), "and the way out: {why}");
        assert!(
            unsupported_function("json_get", "pg", "Use '->>' instead.")
                .contains(". Use '->>' instead. To use it"),
            "a source's own clause sits between the two"
        );
    }

    /// An unregistered kind is a sentence naming the fix, because that sentence is the whole of
    /// what the failed data source row shows.
    #[test]
    fn an_unregistered_kind_names_the_fix() {
        let why = Registrants::default()
            .get("mongo")
            .expect_err("nothing registered");
        assert!(
            why.contains("'mongo'") && why.contains("with_source"),
            "{why}"
        );
        assert_eq!(
            Registrants::default().check_address("mongo", "somewhere"),
            Err(why)
        );
    }
}
