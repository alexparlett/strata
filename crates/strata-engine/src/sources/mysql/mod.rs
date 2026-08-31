//! The `MySQL` source, registered like any other.
//!
//! Everything `MySQL`-specific in the sources layer is here: the settings it declares, the pool
//! (which is the probe), the enumeration query, the federated read path, the JSON accessor family,
//! the two write statements DataFusion can plan and the statements only the server can run. The
//! module and its dependency tree ride the `mysql` cargo feature, so an engine built
//! without it has no `MySQL` in its tree at all, and a def naming this kind settles as a failed row
//! saying so.
//!
//! **Nothing outside this directory knows `MySQL` exists.** That is the claim the second
//! registrant was built to test: the address rule, the form, the enumeration, the identifier
//! spelling and the JSON vocabulary are all answers to [`DataSource`] and [`SourceCatalog`]
//! questions, and the seams a `PostgreSQL` source found sufficient were sufficient here without
//! widening.
//!
//! **A server, and its databases as schemas.** A `MySQL` database is a namespace inside the
//! server rather than a separate connection, so one data source is one *server* and every database
//! the account can read is a schema of it — `source.database.table`, the `DataGrip` model. The
//! address is therefore `host:port` and a database segment in it is refused
//! ([`settings::parse_address`]).
//!
//! **The write half is [`write`]'s**, and the one question it settles differently from the
//! `PostgreSQL` arm — how a CTAS knows it made the relation, without transactional DDL — is
//! written out there.
//!
//! **This module holds a password for the length of one login, and never stores one.** The def
//! says only that one is set; the value is read per connect from the engine's `SecretProvider`,
//! from this machine's keystore or from `MYSQL_PWD`. Unlike the `PostgreSQL` pool, this driver
//! takes the password as a connection parameter rather than asking a provider per connection, so
//! the value is read once here and lives inside the pool's own `SecretString` for the pool's
//! life. Passwordless authentication is `None` rather than a mode anything has to know about.

mod dialect;
mod executor;
mod json;
pub mod settings;
mod write;

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use datafusion::arrow::datatypes::SchemaRef;
use datafusion::catalog::TableProvider;
use datafusion_table_providers_common::sql::sql_provider_datafusion::SqlTable;
use datafusion_table_providers_common::util::secrets::to_secret_map;
use datafusion_table_providers_mysql::pool::{self, MySQLConnectionPool};
use datafusion_table_providers_mysql::DynMySQLConnectionPool;
use mysql_async::prelude::Queryable;
use secrecy::SecretString;
use tokio::task::spawn_blocking;

use strata_model::SourceDef;

use self::dialect::MyDialect;
use self::executor::NamedProjection;
use self::settings::{MyAddress, MySettings, PASSWORD, PASSWORD_ENV};
use crate::secrets::{SecretProvider, SecretRequest};
use crate::sources::source::{
    ConnectRefusal, DataSource, FunctionMap, Listing, Located, Relation, ServerIdent,
    SourceCatalog, SourceKind, SourceMode, SourceSetting, Sourced,
};
use crate::sources::sql::{federated, SQLExecutor, SqlSpec};
use crate::sources::{no_secret, secret_slot};
use crate::statements::Remote;

/// The `MySQL` source.
///
/// Stateless: everything about one data source lives on the [`MyCatalog`] a connect hands back, so
/// one registered value serves every `MySQL` data source a project holds.
#[derive(Clone, Copy, Debug, Default)]
pub struct My;

impl SourceKind for My {
    const NAME: &'static str = "mysql";
    const LABEL: &'static str = "MySQL";
    const BADGE: &'static str = "MY";
    const MODE: SourceMode = SourceMode::Catalog;
    const WRITABLE: bool = true;
}

#[async_trait]
impl DataSource for My {
    /// **Building the pool is the probe**: `MySQLConnectionPool::new` resolves the host, opens a
    /// TCP connection, authenticates, builds the pool and runs `SELECT 1`, failing on any of them.
    /// There is nothing left to ask — a server either let us in or did not.
    async fn connect(
        &self,
        def: &SourceDef,
        secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, ConnectRefusal> {
        let settings = MySettings::read(&def.config)?;
        let address = settings::parse_address(def.setting("address"))?;
        let password = match secret_slot(def, PASSWORD, PASSWORD_ENV) {
            Some(request) => Some(password(request, secrets).await?),
            None => None,
        };
        let pool = build_pool(def, &address, &settings, password).await?;
        Ok(Sourced::Catalog(Arc::new(MyCatalog {
            pool: Arc::new(pool),
        })))
    }

    fn check_address(&self, address: &str) -> Result<(), String> {
        settings::parse_address(address).map(|_| ())
    }

    fn settings(&self) -> &'static [SourceSetting] {
        settings::SETTINGS
    }
}

/// One live `MySQL` data source: the pool, and everything a catalog is asked about it.
#[derive(Debug)]
pub struct MyCatalog {
    pool: Arc<MySQLConnectionPool>,
}

#[async_trait]
impl SourceCatalog for MyCatalog {
    fn kind(&self) -> &'static str {
        My::NAME
    }

    async fn enumerate(&self) -> Result<Listing, String> {
        let conn = self
            .pool
            .connect_direct()
            .await
            .map_err(|e| format!("Cannot read the server's databases: {e}"))?;
        let rows: Vec<(String, String, String)> = conn
            .conn
            .lock()
            .await
            .query(RELATIONS_QUERY)
            .await
            .map_err(|e| format!("Cannot read the server's databases: {e}"))?;
        Ok(Listing::of(rows.into_iter().map(|(schema, name, kind)| {
            (
                schema,
                Relation {
                    name,
                    view: is_view(&kind),
                },
            )
        })))
    }

    /// One call into the SQL assembly, over the crate's `SqlTable` in this data source's own
    /// unparser dialect.
    ///
    /// The **same** dialect value on both, which is what puts the JSON rewrite on the federated
    /// statement and on the fallback provider's own scan alike: `SqlTable` is what a scan the
    /// federation rule does not take reads through.
    ///
    /// The generic `SqlTable` rather than the crate's own `MySQLTable`, which hardcodes
    /// `MySqlDialect` and so would send `json_as_text(…)` to a server that has no such function.
    async fn table_provider(
        self: Arc<Self>,
        at: &Located,
    ) -> Result<Arc<dyn TableProvider>, String> {
        let pool: Arc<DynMySQLConnectionPool> =
            Arc::clone(&self.pool) as Arc<DynMySQLConnectionPool>;
        let dialect = Arc::new(MyDialect::new(at.source.clone()));
        let table = Arc::new(
            SqlTable::new(My::NAME, &pool, at.relation.clone())
                .await
                .map_err(|e| e.to_string())?
                .with_dialect(dialect.clone()),
        );
        let executor = Arc::new(NamedProjection {
            inner: Arc::clone(&table) as Arc<dyn SQLExecutor>,
        });
        Ok(federated(
            self,
            SqlSpec {
                dialect,
                executor,
                provider: table,
            },
            at,
        ))
    }

    /// The JSON accessor family, read off the one table that says which members have a faithful
    /// spelling on a server — as *support* here, and as *spelling* by the rewrite.
    fn function_map(&self) -> &FunctionMap {
        &JSON_SUPPORT
    }

    fn server_ident(&self, name: &str) -> ServerIdent {
        server_ident(name)
    }

    /// `ER_SP_DOES_NOT_EXIST` (errno 1305) — what a federated statement gets back for carrying a
    /// name only DataFusion knows.
    fn remote_refusal(&self, raw: &str, source: &str) -> Option<String> {
        json::lacks_the_name(raw).then(|| json::remote_refusal(raw, source))
    }

    fn writer(
        &self,
        read: Arc<dyn TableProvider>,
        at: &Remote,
        schema: SchemaRef,
    ) -> Result<Arc<dyn TableProvider>, String> {
        Ok(write::writer(self, read, at, schema))
    }

    async fn create_relation(&self, at: &Remote, schema: SchemaRef) -> Result<bool, String> {
        write::create(self, at, schema).await
    }

    async fn drop_relation(&self, at: &Remote) -> Result<(), String> {
        write::discard(self, at).await
    }

    /// Prepared rather than sent as text, which is what makes "one statement" a property of the
    /// protocol rather than a hope: `exec_drop` prepares, and a prepared statement carries exactly
    /// one, so a second one smuggled past the parser is refused by the driver rather than run.
    async fn execute_text(&self, text: &str) -> Result<u64, String> {
        let conn = self
            .pool
            .connect_direct()
            .await
            .map_err(|e| format!("Cannot reach the server: {e}"))?;
        let mut held = conn.conn.lock().await;
        held.exec_drop(text, ()).await.map_err(|e| e.to_string())?;
        Ok(held.affected_rows())
    }
}

/// [`json`]'s own table as the engine's generic lens over it — built once, because the table is
/// constant and the map is read per refusal.
static JSON_SUPPORT: LazyLock<FunctionMap> = LazyLock::new(json::support);

/// Whether the server calls this relation kind a view.
///
/// The one place `information_schema`'s words are read (`BASE TABLE`, `VIEW`, `SYSTEM VIEW`), so
/// the data-sources tree's Tables / Views split and DataFusion's own answer cannot disagree.
/// Anything else is a table, which is the safe way for the fallthrough to point: a relation
/// listed under the wrong heading still reads.
fn is_view(table_type: &str) -> bool {
    matches!(table_type, "VIEW" | "SYSTEM VIEW")
}

/// **One round trip for the whole catalog shape**, and the privilege filter is the server's own:
/// `information_schema.tables` shows an account only the relations it holds some privilege on, so
/// a listing is already "every database this account can see" without a predicate of ours.
///
/// The server's own schemas are left out. They are visible to every account and would add a
/// hundred relations to every listing; `DataGrip` hides them for the same reason, and nothing stops
/// a query naming one (registration is not what resolves a schema).
const RELATIONS_QUERY: &str = "\
SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
FROM information_schema.tables \
WHERE TABLE_SCHEMA NOT IN ('information_schema', 'mysql', 'performance_schema', 'sys') \
ORDER BY 1, 2";

/// `name` as a statement the server parses may say it: backticks, with an embedded backtick
/// doubled.
///
/// The server's own rule, which is neither SQL's nor DataFusion's — which is the whole reason
/// [`SourceCatalog::server_ident`] is a method. Quoted always rather than only where it is needed,
/// because the reserved words are the server's and no local table knows them.
fn server_ident(name: &str) -> ServerIdent {
    ServerIdent::spelled(format!("`{}`", name.replace('`', "``")))
}

/// One data source's password, read **per connect**: never cached here, never held past the pool
/// it is for.
///
/// The read is blocking — a keystore call can wait on a platform lock or on the user — so it goes
/// through `spawn_blocking` rather than stalling a runtime worker.
///
/// # Errors
///
/// If the store could not be read, or holds nothing: a def that expects a password and a machine
/// that has none is the ordinary case for a colleague who has just pulled the project, and the
/// sentence names both places one can come from.
async fn password(
    request: SecretRequest,
    secrets: Arc<dyn SecretProvider>,
) -> Result<SecretString, String> {
    let asked = request.clone();
    let read = spawn_blocking(move || secrets.secret(&asked))
        .await
        .map_err(|e| format!("Reading the password for '{}' failed: {e}", request.source))??;
    match read {
        Some(secret) => Ok(SecretString::from(secret.expose().to_string())),
        None => Err(no_secret("password", &request)),
    }
}

/// The pool itself, with every failure turned into a sentence naming what to fix.
///
/// No `db` parameter, which is what makes the data source server-wide: the driver connects with no
/// default database and every statement names the database it means, so all of them are schemas
/// of one source.
async fn build_pool(
    conn: &SourceDef,
    address: &MyAddress<'_>,
    settings: &MySettings,
    password: Option<SecretString>,
) -> Result<MySQLConnectionPool, ConnectRefusal> {
    let mut params = to_secret_map(HashMap::from([
        ("host".to_string(), address.host.to_string()),
        ("tcp_port".to_string(), address.port.to_string()),
        ("user".to_string(), settings.user.clone()),
        ("sslmode".to_string(), settings.ssl.clone()),
    ]));
    if let Some(password) = password {
        params.insert("pass".to_string(), password);
    }
    MySQLConnectionPool::new(params)
        .await
        .map_err(|e| refused(conn, address, settings, e))
}

/// Why a login did not happen, in the terms of the thing to fix.
///
/// The crate's own prose is good and is kept wherever it already names the fault; the two arms
/// rewritten here are the ones it cannot word as well as we can, because it does not know there is
/// a data source editor behind them. Nothing in any of it is a password.
///
/// **The rejected-credential arm carries a [`ConnectFault`] as well as its sentence**, so the
/// editor's `PASSWORD` row can say the stored value was turned away. The recognition is the
/// server's own error code — 1045, access denied — which is what the crate matches to produce
/// this variant; the sentence names the user as well, because that code does not separate a wrong
/// password from a user the server does not know.
fn refused(
    conn: &SourceDef,
    address: &MyAddress<'_>,
    settings: &MySettings,
    e: pool::Error,
) -> ConnectRefusal {
    match e {
        pool::Error::InvalidHostOrPortError { host, port, .. } => format!(
            "Cannot reach a MySQL server at '{host}:{port}'. Check the address, and that the \
             server is running."
        )
        .into(),
        pool::Error::InvalidUsernameOrPassword => ConnectRefusal::rejected(
            format!(
                "The server refused the user '{}' at '{}:{}'. Check the user and its password.",
                settings.user, address.host, address.port
            ),
            PASSWORD,
        ),
        other => format!("Cannot connect to '{}': {other}", conn.named()).into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::source::Support;

    /// The `information_schema` words that mean a view, read in one place — and everything else
    /// reads as a table rather than as a failure.
    #[test]
    fn the_information_schema_words_that_mean_view_are_the_two_view_kinds() {
        assert!(is_view("VIEW") && is_view("SYSTEM VIEW"));
        for table in ["BASE TABLE", "TEMPORARY", ""] {
            assert!(!is_view(table), "'{table}' is not a view");
        }
    }

    /// The support map and the rewrite read **one** table: a member the rewrite can spell is
    /// `Mapped` here, and one it cannot is `Unmapped` with whatever the family has to add.
    #[test]
    fn the_function_map_is_the_familys_own_answer() {
        let map = &*JSON_SUPPORT;
        assert_eq!(map.support("json_as_text"), Some(&Support::Mapped));
        assert_eq!(map.support("json_contains"), Some(&Support::Mapped));
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
    /// outside knowing what a `MySQL` address looks like.
    #[test]
    fn the_address_rule_is_reached_through_the_trait() {
        assert_eq!(My.check_address("db.internal:3306"), Ok(()));
        let why = My.check_address("db.internal").expect_err("no port");
        assert!(why.contains("needs a port"), "{why}");
        let why = My
            .check_address("db.internal:3306/analytics")
            .expect_err("a database segment");
        assert!(why.contains("source.database.table"), "{why}");
    }

    /// **Backticks, doubled** — the server's own spelling, and the reason `server_ident` is a
    /// method rather than one rule for every source.
    #[test]
    fn an_identifier_bound_for_the_server_is_backquoted() {
        assert_eq!(server_ident("orders").as_str(), "`orders`");
        assert_eq!(server_ident("select").as_str(), "`select`");
        assert_eq!(server_ident("we`ird").as_str(), "`we``ird`");
    }
}
