//! The `PostgreSQL` data source, registered like any other.
//!
//! Everything `PostgreSQL`-specific in the sources layer is here: the settings it declares, the pool
//! (which is the probe), the enumeration query, the federated read path, the JSON operator family,
//! the two write statements DataFusion can plan, and the statements only the server can run. The
//! module and its dependency tree ride the `postgres` cargo feature, so an engine built without it
//! has no `PostgreSQL` in its tree at all, and a def naming this kind settles as a failed row saying
//! so.
//!
//! **This module holds a password for the length of one login, and never stores one.** The def
//! says only that one is set; the value is read per pool connection from the engine's
//! `SecretProvider` ([`SecretPassword`]), from this machine's keystore or from `PGPASSWORD`.
//! Passwordless authentication is `None` rather than a mode anything has to know about.

mod json;
pub mod settings;
mod write;

use std::collections::HashMap;
use std::error::Error;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::TableProvider;
use datafusion::sql::unparser::dialect::PostgreSqlDialect;
use datafusion_table_providers_common::sql::db_connection_pool::PasswordProvider;
use datafusion_table_providers_common::sql::sql_provider_datafusion::SqlTable;
use datafusion_table_providers_common::util::secrets::to_secret_map;
use datafusion_table_providers_common::UnsupportedTypeAction;
use datafusion_table_providers_postgres::pool::{self, PostgresConnectionPool};
use datafusion_table_providers_postgres::write::PostgresTableWriter;
use datafusion_table_providers_postgres::DynPostgresConnectionPool;
use secrecy::SecretString;
use tokio::task::spawn_blocking;

use strata_model::ConnectionDef;

use self::settings::{PgSettings, PASSWORD, PASSWORD_ENV};
use crate::catalog::readable;
use crate::ddl::RemoteTarget;
use crate::secrets::{SecretProvider, SecretRequest};
use crate::sources::secret_slot;
use crate::sources::source::{
    ConnectionKey, DataSource, FunctionMap, Listing, Located, Relation, SourceCatalog, SourceKind,
    SourceMode, Sourced,
};
use crate::sources::sql::{federated, SQLExecutor, SqlSpec};

/// The `PostgreSQL` data source.
///
/// Stateless: everything about one connection lives on the [`PgCatalog`] a connect hands back, so
/// one registered value serves every `PostgreSQL` connection a project holds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pg;

impl SourceKind for Pg {
    const NAME: &'static str = "postgres";
    const LABEL: &'static str = "PostgreSQL";
    const BADGE: &'static str = "PG";
    const MODE: SourceMode = SourceMode::Catalog;
}

#[async_trait]
impl DataSource for Pg {
    /// **Building the pool is the probe**, and for free: `PostgresConnectionPool::new` resolves
    /// the host, opens a TCP connection, authenticates, builds the pool and runs `SELECT 1`,
    /// failing on any of them. There is nothing left to ask — a server either let us in or did
    /// not.
    async fn connect(
        &self,
        def: &ConnectionDef,
        secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, String> {
        let source = def
            .provider
            .source()
            .ok_or_else(|| format!("'{}' is not a PostgreSQL connection.", def.identity()))?;
        let settings = PgSettings::read(&source.config)?;
        let passwords = match source.secrets.contains(PASSWORD) {
            false => None,
            true => {
                let request = secret_slot(def, PASSWORD, PASSWORD_ENV)
                    .ok_or_else(|| format!("'{}' is not a data source.", def.identity()))?;
                Some(Arc::new(SecretPassword { request, secrets }) as Arc<dyn PasswordProvider>)
            }
        };
        let pool = build_pool(def, &settings, passwords).await?;
        Ok(Sourced::Catalog(Arc::new(PgCatalog {
            pool: Arc::new(pool),
        })))
    }

    fn check_address(&self, address: &str) -> Result<(), String> {
        settings::parse_address(address.trim()).map(|_| ())
    }

    fn config_keys(&self) -> &'static [ConnectionKey] {
        settings::KEYS
    }
}

/// One live `PostgreSQL` connection: the pool, and everything a catalog is asked about it.
#[derive(Debug)]
pub struct PgCatalog {
    pool: Arc<PostgresConnectionPool>,
}

#[async_trait]
impl SourceCatalog for PgCatalog {
    fn kind(&self) -> &'static str {
        Pg::NAME
    }

    async fn enumerate(&self) -> Result<Listing, String> {
        let conn = self
            .pool
            .connect_direct()
            .await
            .map_err(|e| format!("Cannot read the database's schemas: {e}"))?;
        let rows = conn
            .conn
            .query(RELATIONS_QUERY, &[])
            .await
            .map_err(|e| format!("Cannot read the database's schemas: {e}"))?;
        Ok(Listing::of(rows.iter().map(|row| {
            let schema: String = row.get(0);
            let name: String = row.get(1);
            let relkind: String = row.get(2);
            (
                schema,
                Relation {
                    name,
                    view: is_view(&relkind),
                },
            )
        })))
    }

    /// One call into the SQL assembly, over the crate's `SqlTable` in `PostgreSQL`'s unparser
    /// dialect.
    async fn table_provider(
        self: Arc<Self>,
        at: &Located,
    ) -> Result<Arc<dyn TableProvider>, String> {
        let pool: Arc<DynPostgresConnectionPool> =
            Arc::clone(&self.pool) as Arc<DynPostgresConnectionPool>;
        let dialect = Arc::new(PostgreSqlDialect {});
        let table = Arc::new(
            SqlTable::new(Pg::NAME, &pool, at.relation.clone())
                .await
                .map_err(|e| e.to_string())?
                .with_dialect(dialect.clone()),
        );
        let connection = at.connection.clone();
        Ok(federated(
            self,
            SqlSpec {
                dialect,
                executor: Arc::clone(&table) as Arc<dyn SQLExecutor>,
                provider: table,
                analyzer: Some(Arc::new(move || {
                    let connection = connection.clone();
                    Box::new(move |statement| json::push_down(statement, &connection))
                })),
            },
            at,
        ))
    }

    /// The JSON accessor family, read off the one table that says which members have a faithful
    /// spelling on a server — as *support* here, and as *spelling* by the rewrite.
    fn function_map(&self) -> &FunctionMap {
        &JSON_SUPPORT
    }

    /// `undefined_function` (`SQLSTATE` 42883), which covers a missing function *and* a missing
    /// operator — what a federated statement gets back for carrying a name only DataFusion knows.
    fn remote_refusal(&self, raw: &str, connection: &str) -> Option<String> {
        json::lacks_the_name(raw).then(|| json::remote_refusal(raw, connection))
    }

    fn writer(
        &self,
        read: Arc<dyn TableProvider>,
        at: &RemoteTarget,
        schema: SchemaRef,
    ) -> Result<Arc<dyn TableProvider>, String> {
        Ok(PostgresTableWriter::create(
            read,
            write::target(&self.pool, at, schema),
            None,
        ))
    }

    async fn create_relation(&self, at: &RemoteTarget, schema: SchemaRef) -> Result<bool, String> {
        write::create(self, at, schema).await
    }

    async fn drop_relation(&self, at: &RemoteTarget) -> Result<(), String> {
        write::discard(self, at).await
    }

    /// Over the extended query protocol rather than `batch_execute`: it is the only one that
    /// answers with an affected-row count, and it carries exactly one statement, so a second one
    /// smuggled past the parser is refused by the driver rather than run.
    async fn execute_text(&self, text: &str) -> Result<u64, String> {
        let conn = self
            .pool
            .connect_direct()
            .await
            .map_err(|e| format!("Cannot reach the server: {e}"))?;
        conn.conn
            .execute(text, &[])
            .await
            .map_err(|e| readable(&server_error(&e)))
    }
}

/// [`json`]'s own table as the engine's generic lens over it — built once, because the table is
/// constant and the map is read per refusal.
static JSON_SUPPORT: LazyLock<FunctionMap> = LazyLock::new(json::support);

/// Whether the server calls this relation kind a view — a view or a materialized view.
///
/// The one place the server's letters are read (`r` `p` `v` `m` `f`: an ordinary table, a
/// partitioned one, a view, a materialized view, a foreign table), so the data-sources tree's
/// Tables / Views split and DataFusion's own answer cannot disagree about a materialized view.
fn is_view(relkind: &str) -> bool {
    matches!(relkind, "v" | "m")
}

/// What the server said, rather than the driver's own `db error` placeholder: `tokio_postgres`
/// renders the useful sentence — the `SQLSTATE`, the position, the hint — on the wrapped `DbError`.
fn server_error(e: &impl Error) -> String {
    match e.source() {
        Some(cause) => cause.to_string(),
        None => e.to_string(),
    }
}

/// **One round trip for the whole catalog shape.** `pg_class` joined to `pg_namespace`, filtered
/// to the relation kinds a query can read and to what this role may actually use.
///
/// `pg_class`, not the crate's `pg_tables`, and that is one of the three reasons the listing is
/// ours: `pg_tables` is tables only, so remote **views**, materialized views, partitioned tables
/// and foreign tables would be missing from the tree while remaining perfectly queryable — a tree
/// that lies about what is there.
///
/// The system schemas are left out. `pg_catalog` and `information_schema` are visible to every
/// role and would add hundreds of relations to every listing; `DataGrip` hides them for the same
/// reason, and nothing stops a query naming one (registration is not what resolves a schema, and
/// a query for a system table simply finds no schema rather than misbehaving).
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

/// The pool itself, with every failure turned into a sentence naming what to fix.
async fn build_pool(
    conn: &ConnectionDef,
    settings: &PgSettings,
    passwords: Option<Arc<dyn PasswordProvider>>,
) -> Result<PostgresConnectionPool, String> {
    let address = settings::parse_address(conn.address.trim())?;
    let mut params = HashMap::from([
        ("host".to_string(), address.host.to_string()),
        ("port".to_string(), address.port.to_string()),
        ("db".to_string(), address.database.to_string()),
        ("user".to_string(), settings.user.clone()),
        ("sslmode".to_string(), settings.sslmode.clone()),
        ("application_name".to_string(), "Strata".to_string()),
    ]);
    let cert = settings.sslrootcert.trim();
    if settings.verifies() && !cert.is_empty() {
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
        .map(|pool| pool.with_unsupported_type_action(UnsupportedTypeAction::String))
        .map_err(|e| refused(conn, settings, e))
}

/// Why a login did not happen, in the terms of the thing to fix.
///
/// The crate's own prose is good and is kept wherever it already names the fault; the two arms
/// rewritten here are the ones it cannot word as well as we can, because it does not know there
/// is a connection editor behind them. Nothing in any of it is a password: the crate builds its
/// connection string without one on purpose, and our own provider's failure is
/// [`SecretPassword`]'s sentence.
fn refused(conn: &ConnectionDef, settings: &PgSettings, e: pool::Error) -> String {
    match e {
        pool::Error::InvalidHostOrPortError { host, port, .. } => format!(
            "Cannot reach a PostgreSQL server at '{host}:{port}'. Check the address, and that \
             the server is running."
        ),
        pool::Error::InvalidUsernameOrPassword { .. } => format!(
            "The server refused the user '{}'. Check the user and its password.",
            settings.user
        ),
        pool::Error::PasswordProviderError { source } => source.to_string(),
        other => format!("Cannot connect to '{}': {other}", conn.identity()),
    }
}

/// One connection's password: read **per new pool connection**, never cached, never held past the
/// login it is for.
///
/// The slot is derived from the connection's identity rather than stored on the def, so the
/// committed `project.json` carries no machine-local id. Which means the ordinary answer on a
/// colleague's machine is *there is no entry*, and that is a sentence rather than a fault — one
/// naming both the keystore and `PGPASSWORD`.
///
/// The read is blocking — a keystore call can wait on a platform lock or on the user — so it goes
/// through `spawn_blocking` rather than stalling a runtime worker while the pool opens a
/// connection.
struct SecretPassword {
    request: SecretRequest,
    secrets: Arc<dyn SecretProvider>,
}

#[async_trait]
impl PasswordProvider for SecretPassword {
    async fn get_password(&self) -> Result<SecretString, Box<dyn Error + Send + Sync>> {
        let request = self.request.clone();
        let secrets = Arc::clone(&self.secrets);
        let read = spawn_blocking(move || secrets.secret(&request))
            .await
            .map_err(|e| {
                format!(
                    "Reading the password for '{}' failed: {e}",
                    self.request.connection
                )
            })?;
        match read? {
            Some(secret) => Ok(SecretString::from(secret.expose().to_string())),
            None => Err(format!(
                "No password is stored on this machine for '{}'. {}",
                self.request.connection,
                self.request.fixes()
            )
            .into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::source::Support;

    /// The relation kinds a query can read, split the way the tree splits them — the server's own
    /// letters, read in one place.
    #[test]
    fn the_server_letters_that_mean_view_are_the_two_view_kinds() {
        assert!(is_view("v") && is_view("m"));
        for table in ["r", "p", "f"] {
            assert!(!is_view(table), "'{table}' is a table");
        }
    }

    /// The support map and the rewrite read **one** table: a member the rewrite can spell is
    /// `Mapped` here, and one it cannot is `Unmapped` with whatever the family has to add.
    #[test]
    fn the_function_map_is_the_familys_own_answer() {
        let map = &*JSON_SUPPORT;
        assert_eq!(map.support("json_as_text"), Some(&Support::Mapped));
        assert_eq!(
            map.support("json_get"),
            Some(&Support::Unmapped {
                why: json::ARROW_INSTEAD.to_string()
            })
        );
        assert_eq!(
            map.support("json_length"),
            Some(&Support::Unmapped { why: String::new() })
        );
        assert_eq!(map.support("upper"), None, "not this table's business");
    }

    /// The address rule is this source's own, reached through the trait rather than by anything
    /// outside knowing what a `PostgreSQL` address looks like.
    #[test]
    fn the_address_rule_is_reached_through_the_trait() {
        assert_eq!(Pg.check_address("db.internal:5432/analytics"), Ok(()));
        let why = Pg
            .check_address("db.internal/analytics")
            .expect_err("no port");
        assert!(why.contains("needs a port"), "{why}");
    }
}
