//! Engine construction. See [`EngineBuilder`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use datafusion::execution::memory_pool::MemoryPool;
#[cfg(test)]
use datafusion::prelude::SessionContext;
use tokio::runtime::Builder as RuntimeBuilder;

use crate::formats::{FileFormatKind, FormatProvider, Formats};
use crate::functions::Functions;
use crate::generation::GenClock;
use crate::policy::{Capability, CapabilityPolicyProvider, PolicyProvider};
use crate::secrets::{KeystoreSecrets, SecretProvider};
use crate::snapshots::{LocalIpcSnapshotStore, SnapshotStore};
use crate::sources::source::{DataSource, Registrants, SourceKind};
use crate::sources::Live;
use crate::tables::{InternalTableStore, LocalIpcTableStore};
use crate::udf_package::UdfPackage;
use crate::{
    build_context, runtime_subset, Dependencies, Engine, InternalTables, Ledger, SessionScope,
    SourceDefs,
};

/// The engine-id allocator — see [`Engine::id`].
static ENGINE_SEQ: AtomicU64 = AtomicU64::new(0);

/// Configure and build an [`Engine`].
///
/// Every setting has a default, so `Engine::builder().build()` is a complete engine.
///
/// Beyond the builder, embedding the engine takes few calls:
/// [`Catalog::sync`](crate::Catalog::sync) to load a project's catalog,
/// [`Workspace::run`](crate::Workspace::run) and [`Workspace::explain`](crate::Workspace::explain) to execute a statement, [`SnapshotReads::page`](crate::SnapshotReads::page),
/// [`SnapshotReads::export_to`](crate::SnapshotReads::export_to) and [`SnapshotReads::live`](crate::SnapshotReads::live) to read a result, and
/// [`Lang::policy_verdicts`](crate::Lang::policy_verdicts) to check what a caller may run.
///
/// # Example
///
/// ```no_run
/// use strata_engine::{secrets::MemSecrets, Engine};
///
/// let engine = Engine::builder()
///     .with_data_dir("/data/lake")
///     .with_secrets(MemSecrets::new())
///     .build();
/// ```
pub struct EngineBuilder {
    config: BTreeMap<String, String>,
    data_dir: Option<PathBuf>,
    secrets: Arc<dyn SecretProvider>,
    udfs: Vec<Arc<dyn UdfPackage>>,
    memory_pool: Option<Arc<dyn MemoryPool>>,
    policy: Arc<dyn PolicyProvider>,
    sources: Registrants,
    formats: Formats,
    snapshots: Option<Arc<dyn SnapshotStore>>,
    tables: Option<Arc<dyn InternalTableStore>>,
}

/// The shipped sources and formats are registered here, through the same public calls an embedder
/// makes. Each source rides its own cargo feature, so an engine built with none of them has no
/// source at all — which is what makes the registry the only path in, on both axes.
impl Default for EngineBuilder {
    fn default() -> Self {
        let builder = Self {
            config: BTreeMap::new(),
            data_dir: None,
            secrets: Arc::new(KeystoreSecrets),
            udfs: vec![Arc::new(crate::udfs::StrataFunctions)],
            memory_pool: None,
            policy: Arc::new(CapabilityPolicyProvider::new(Capability::full())),
            sources: Registrants::default(),
            formats: Formats::shipped(),
            snapshots: None,
            tables: None,
        };
        let builder = builder
            .with_source(crate::sources::store::s3::S3)
            .with_source(crate::sources::store::gcs::Gcs)
            .with_source(crate::sources::store::http::Http);
        #[cfg(feature = "postgres")]
        let builder = builder.with_source(crate::sources::postgres::Pg);
        builder
    }
}

impl EngineBuilder {
    /// Creates an `EngineBuilder` with default settings.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the project directory, defaults to unset
    ///
    /// Tables the engine owns are stored under it, and `COPY` may not write into it. An engine
    /// with no project directory cannot create a table. [`Engine::set_data_dir`] sets the same
    /// value on a built engine.
    pub fn with_data_dir(mut self, root: impl AsRef<Path>) -> Self {
        self.data_dir = Some(root.as_ref().to_path_buf());
        self
    }

    /// Sets the `datafusion.*` configuration overrides
    ///
    /// `datafusion.runtime.*` keys are read only here. [`Engine::set_config`] changes the others
    /// on a built engine; changing a runtime key requires a new engine.
    pub fn with_config(mut self, overrides: BTreeMap<String, String>) -> Self {
        self.config = overrides;
        self
    }

    /// Sets the source of secrets, defaults to [`KeystoreSecrets`]
    pub fn with_secrets(mut self, secrets: impl SecretProvider + 'static) -> Self {
        self.secrets = Arc::new(secrets);
        self
    }

    /// Adds a package of SQL functions
    ///
    /// May be called more than once. Packages are registered in the order added, after the
    /// built-ins.
    pub fn with_udfs(mut self, package: impl UdfPackage + 'static) -> Self {
        self.udfs.push(Arc::new(package));
        self
    }

    /// Sets who may perform what, defaults to `CapabilityPolicyProvider::new(Capability::full())`
    ///
    /// The default refuses nothing, so restriction is something an embedder says rather than
    /// something it has to switch off: pass `CapabilityPolicyProvider::new(Capability::read_only())`
    /// for an engine whose statements may only read, or your own [`PolicyProvider`] to decide
    /// against a policy service. A caller's own capability narrows this one and never widens it,
    /// so this is a ceiling.
    ///
    /// Asked by [`Workspace::run`](crate::Workspace::run), [`Lang::analyze`](crate::Lang::analyze) and [`Lang::policy_verdicts`](crate::Lang::policy_verdicts) — the
    /// entries that classify a statement. [`Workspace::query`](crate::Workspace::query) and [`Workspace::explain`](crate::Workspace::explain) are handed a
    /// statement to read and are limited to reading by the read path's own `SQLOptions`; they do
    /// not consult this.
    pub fn with_policy(mut self, policy: impl PolicyProvider) -> Self {
        self.policy = Arc::new(policy);
        self
    }

    /// Adds a data source this engine can connect to
    ///
    /// May be called more than once, and a source registered under a name another already holds
    /// replaces it — which is how an embedder substitutes their own for a shipped one. A
    /// data source def reaches its source by [`SourceKind::NAME`], so what is registered here is
    /// what a def's kind may say; a kind nothing answers to settles as a failed row naming the
    /// fix rather than a fault.
    pub fn with_source<S: DataSource + SourceKind>(mut self, source: S) -> Self {
        self.sources.insert(source);
        self
    }

    /// Adds a file format this engine can read
    ///
    /// May be called more than once. A table def reaches its reader by
    /// [`FileFormatKind::NAME`], which is also the word `STORED AS` takes and the key the
    /// `STORED AS` offer is built from; a format nothing answers to settles as a failed row
    /// naming the fix rather than a fault. A format declaring
    /// [`writer`](FormatProvider::writer) is registered on the session under that same name, so
    /// `COPY … STORED AS <name>` writes through it.
    ///
    /// # Panics
    ///
    /// If that name is already registered, including by one of the shipped formats. Unlike a
    /// source, a format is not replaceable: the session's writer map is what DataFusion
    /// resolves `COPY … STORED AS` against, so registering over `parquet` / `csv` / `json` /
    /// `arrow` would change what every other `COPY` in the session writes.
    pub fn with_format<F: FormatProvider + FileFormatKind>(mut self, format: F) -> Self {
        self.formats.insert(format);
        self
    }

    /// Sets where this engine's results live, defaults to [`LocalIpcSnapshotStore`]
    ///
    /// The default spools each result to an Arrow IPC file in a directory it claims under the
    /// machine's temp root, which is what keeps RAM to one page however large a result is;
    /// [`MemSnapshotStore`](crate::snapshots::MemSnapshotStore) holds them in RAM instead, and an
    /// embedder that wants them somewhere else implements [`SnapshotStore`]. The store is built
    /// here rather than by [`build`](Self::build) so that a builder that is never built claims
    /// nothing.
    pub fn with_snapshot_store(mut self, store: impl SnapshotStore) -> Self {
        self.snapshots = Some(Arc::new(store));
        self
    }

    /// Sets where this engine's internal tables live, defaults to a [`LocalIpcTableStore`]
    /// following the project folder
    ///
    /// The default spools each table into `.strata/tables/<slug>/` under whatever
    /// [`with_data_dir`](Self::with_data_dir) or [`Engine::set_data_dir`](crate::Engine::set_data_dir)
    /// said, which is what keeps a table's def portable and its data with the project;
    /// [`MemTableStore`](crate::tables::MemTableStore) holds tables in RAM instead — tests and
    /// ephemeral workspaces only, because the defs outlive the process while the data does not,
    /// so a restart re-registers against vanished data — and an
    /// embedder that wants Strata-owned tables somewhere else implements
    /// [`InternalTableStore`].
    pub fn with_table_store(mut self, store: impl InternalTableStore) -> Self {
        self.tables = Some(Arc::new(store));
        self
    }

    /// Sets the memory pool DataFusion allocates from
    ///
    /// Takes precedence over `datafusion.runtime.memory_limit`, which otherwise builds one.
    pub fn with_memory_pool(mut self, pool: impl MemoryPool + 'static) -> Self {
        self.memory_pool = Some(Arc::new(pool));
        self
    }

    /// Returns an [`Engine`] that uses this configuration.
    ///
    /// The engine owns a Tokio runtime and a snapshot directory, both released when the last
    /// handle to it is dropped.
    pub fn build(self) -> Arc<Engine> {
        let engine_id = ENGINE_SEQ.fetch_add(1, Ordering::Relaxed);
        let rt = RuntimeBuilder::new_multi_thread()
            .worker_threads(2)
            .thread_name(format!("df-engine-{engine_id}"))
            .enable_all()
            .build()
            .expect("tokio runtime");
        let ctx = build_context(&self.config, &self.udfs, &self.formats, self.memory_pool);
        let functions = Functions::new(&ctx);
        let snapshots = self
            .snapshots
            .unwrap_or_else(|| Arc::new(LocalIpcSnapshotStore::new()));
        let data_root = Arc::new(Mutex::new(self.data_dir));
        let tables = self
            .tables
            .unwrap_or_else(|| Arc::new(LocalIpcTableStore::following(Arc::clone(&data_root))));
        Arc::new_cyclic(|self_ref| Engine {
            engine_id,
            self_ref: self_ref.clone(),
            rt: Some(rt),
            ctx,
            built_runtime: runtime_subset(&self.config),
            overrides: Mutex::new(self.config),
            snap_seq: AtomicU64::new(1),
            dispatch_seq: AtomicU64::new(1),
            snapshots,
            lifecycle: Mutex::default(),
            inflight_flag: Arc::new(AtomicBool::new(false)),
            functions,
            data_root,
            tables,
            internal: InternalTables::default(),
            dependencies: Dependencies::default(),
            ledger: Ledger::default(),
            source_defs: SourceDefs::default(),
            generation: GenClock::default(),
            live: Live::default(),
            registrants: self.sources,
            formats: self.formats,
            session: SessionScope::default(),
            secrets: self.secrets,
            policy: self.policy,
        })
    }
}

/// The `SessionContext` a default builder produces, for a test that needs a session and not a
/// whole engine. Through the builder, so what those tests run on cannot drift from what an engine
/// runs on.
#[cfg(test)]
pub(crate) fn test_context(overrides: &BTreeMap<String, String>) -> SessionContext {
    let builder = EngineBuilder::new().with_config(overrides.clone());
    build_context(
        &builder.config,
        &builder.udfs,
        &builder.formats,
        builder.memory_pool,
    )
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::fmt;

    use crate::secrets::SecretRequest;
    use datafusion::error::Result as DFResult;
    use datafusion::execution::memory_pool::{MemoryLimit, MemoryReservation, UnboundedMemoryPool};
    use strata_core::secret::Secret;

    use super::*;
    use crate::secrets::MemSecrets;
    use crate::udf_package::tests::OnePackage;

    /// A pool that delegates every decision, so the only thing it carries is its identity — which
    /// is what a test asking "did *this* pool reach the runtime" needs.
    #[derive(Debug, Default)]
    struct NamedPool(UnboundedMemoryPool);

    impl fmt::Display for NamedPool {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(f, "NamedPool")
        }
    }

    impl MemoryPool for NamedPool {
        fn name(&self) -> &str {
            "NamedPool"
        }

        fn grow(&self, reservation: &MemoryReservation, additional: usize) {
            self.0.grow(reservation, additional);
        }

        fn shrink(&self, reservation: &MemoryReservation, shrink: usize) {
            self.0.shrink(reservation, shrink);
        }

        fn try_grow(&self, reservation: &MemoryReservation, additional: usize) -> DFResult<()> {
            self.0.try_grow(reservation, additional)
        }

        fn reserved(&self) -> usize {
            self.0.reserved()
        }
    }

    /// Whether `engine` is allocating from a [`NamedPool`].
    fn pool_is_ours(engine: &Engine) -> bool {
        let pool = engine.ctx.runtime_env().memory_pool.clone();
        (pool.as_ref() as &dyn Any).is::<NamedPool>()
    }

    fn ask() -> SecretRequest {
        SecretRequest {
            family: "postgres-password".into(),
            source: "orders".into(),
            slot: strata_model::SecretRef::derived("postgres-password", "orders"),
            env: &[],
        }
    }

    /// A package reaches the engine the builder built. What a package may *contain*, and the rules
    /// registration applies, are [`crate::udf_package`]'s tests.
    #[test]
    fn a_package_given_to_the_builder_reaches_the_engine() {
        let engine = Engine::builder()
            .with_udfs(OnePackage("embedder_answer"))
            .build();
        assert!(engine.lang().functions().contains("embedder_answer"));
    }

    /// A default engine reads the four shipped formats, in the order they are offered — which is
    /// what every surface that names a format is built from.
    #[test]
    fn the_shipped_formats_are_what_a_default_engine_reads() {
        let engine = Engine::builder().build();
        let words: Vec<&str> = engine.formats().iter().map(|f| f.name).collect();
        assert_eq!(words, ["parquet", "csv", "json", "arrow"]);
        assert!(
            engine.formats().iter().all(|f| f.copy_to),
            "every shipped format is one DataFusion writes"
        );
    }

    /// The defaults build the engine the app has always built.
    ///
    /// The secrets default is deliberately not asserted here: the only way to tell
    /// [`KeystoreSecrets`] from another provider is to ask it, and what a keystore answers depends
    /// on whether some other test in this binary installed a process-global store. It is pinned
    /// where it is set, in [`EngineBuilder::default`].
    #[test]
    fn the_default_build_is_todays_engine() {
        let engine = Engine::builder().build();
        assert!(!engine.restart_owed());
        assert_eq!(engine.overrides(), BTreeMap::new());
        assert!(engine.lang().functions().contains("struct_get"));
        assert_eq!(*engine.data_root.lock().unwrap(), None);
        assert!(
            !pool_is_ours(&engine),
            "no pool was given, so DataFusion's own is in place"
        );
    }

    /// The provider the builder was given is the one the engine reads through — the slug
    /// a source resolves a data source's password with.
    #[test]
    fn secrets_given_to_the_builder_are_the_ones_the_engine_reads() {
        let engine = Engine::builder()
            .with_secrets(MemSecrets::new().with(ask().key(), Secret::new("hunter2").unwrap()))
            .build();
        assert_eq!(
            engine
                .secrets
                .secret(&ask())
                .unwrap()
                .map(|s| s.expose().to_string()),
            Some("hunter2".to_string())
        );
    }

    /// A pool reaches the `RuntimeEnv` DataFusion allocates from.
    #[test]
    fn a_memory_pool_given_to_the_builder_is_the_one_datafusion_allocates_from() {
        let engine = Engine::builder()
            .with_memory_pool(NamedPool::default())
            .build();
        assert!(pool_is_ours(&engine));
    }

    /// A pool wins over `memory_limit`, which otherwise builds one of its own. Both halves are
    /// asserted, because the rule is only meaningful if the limit does build a pool when it is
    /// the only thing said.
    #[test]
    fn a_memory_pool_takes_precedence_over_the_memory_limit() {
        let limit = BTreeMap::from([(
            "datafusion.runtime.memory_limit".to_string(),
            "64M".to_string(),
        )]);
        let with_both = Engine::builder()
            .with_config(limit.clone())
            .with_memory_pool(NamedPool::default())
            .build();
        assert!(pool_is_ours(&with_both), "the given pool is in place");

        let limit_only = Engine::builder().with_config(limit).build();
        assert!(
            !pool_is_ours(&limit_only),
            "and the limit builds a pool of its own when nothing else is said"
        );
        assert!(matches!(
            limit_only.ctx.runtime_env().memory_pool.memory_limit(),
            MemoryLimit::Finite(bytes) if bytes == 64 * 1024 * 1024
        ));
    }

    #[test]
    fn a_data_dir_given_to_the_builder_is_the_one_set_data_dir_would_have_set() {
        let root = std::env::temp_dir().join("strata-builder-data-dir");
        let built = Engine::builder().with_data_dir(&root).build();
        let set = Engine::builder().build();
        set.set_data_dir(&root);
        assert_eq!(
            *built.data_root.lock().unwrap(),
            *set.data_root.lock().unwrap()
        );
    }

    /// An override reaches the session it was given for.
    #[test]
    fn config_given_to_the_builder_reaches_the_session() {
        let engine = Engine::builder()
            .with_config(BTreeMap::from([(
                "datafusion.execution.batch_size".to_string(),
                "512".to_string(),
            )]))
            .build();
        assert_eq!(
            engine.ctx.state().config().options().execution.batch_size,
            512
        );
    }
}
