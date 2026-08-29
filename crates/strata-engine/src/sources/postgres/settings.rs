//! What a `PostgreSQL` connection is described by: the keys it declares, and how a def's config map
//! reads as them.
//!
//! The address is one of those keys ([`Slot::Address`]), so the whole form is here — including
//! the certificate box appearing only under the verifying modes, and the sentence explaining
//! each row.
//!
//! The declaration is the only place these names are written. The editor draws its rows from it,
//! [`PgSettings::read`] parses a def by it, and the conformance tests check the two agree — so a
//! key added here reaches the form and the connection together.
//!
//! **Nothing here holds a secret value.** The password is a declared key like any other, and what
//! that means is that its *value* goes to the keystore or comes from the environment; the def
//! records only that it is set.

use std::collections::BTreeMap;

use crate::sources::source::{ConnectionKey, Field, Slot, When};

/// The key a `PostgreSQL` connection's password is filed under, in the keystore and in the def's
/// expectation set.
pub const PASSWORD: &str = "password";

/// The environment variable a `PostgreSQL` client conventionally reads a password from.
pub const PASSWORD_ENV: &[&str] = &["PGPASSWORD"];

/// The SSL modes libpq names, in libpq's spellings and in its own order — weakest first, which
/// reads as a dial rather than a list.
///
/// The value is handed to the driver as written, so a rename here is a connection that fails with
/// 'Invalid parameter: sslmode'.
pub const SSL_MODES: &[&str] = &["disable", "prefer", "require", "verify-ca", "verify-full"];

/// The modes that read a root certificate — the one list, read by [`verifies`] and by the
/// certificate key's own [`Shown`], so what the form offers and what the driver does cannot
/// disagree.
pub const SSL_VERIFYING: &[&str] = &["verify-ca", "verify-full"];

/// The sections this source's rows sit in.
const CONNECTION: Option<&str> = Some("CONNECTION");
const SSL: Option<&str> = Some("SSL");

/// Every row a `PostgreSQL` connection has, in the order it has them.
///
/// The address is a declared key like the rest ([`Slot::Address`]) — its value lands on the def's
/// own field rather than in `config`, and everything else about the row is stated here, so
/// nothing about a `PostgreSQL` connection is written down in the editor.
pub const KEYS: &[ConnectionKey] = &[
    ConnectionKey {
        key: "address",
        label: "ADDRESS",
        field: Field::Text,
        slot: Slot::Address,
        group: CONNECTION,
        required: true,
        default: None,
        when: None,
        hint: Some("The server and the database on it. The port is not assumed"),
        placeholder: Some("localhost:5432/appdb"),
    },
    ConnectionKey {
        key: "user",
        label: "USER",
        field: Field::Text,
        slot: Slot::Setting,
        group: CONNECTION,
        required: true,
        default: None,
        when: None,
        hint: Some(
            "The role to log in as. Part of the connection's identity, so changing it is a \
             different connection",
        ),
        placeholder: None,
    },
    ConnectionKey {
        key: PASSWORD,
        label: "PASSWORD",
        field: Field::Secret,
        slot: Slot::Setting,
        group: CONNECTION,
        required: false,
        default: None,
        when: None,
        hint: None,
        placeholder: None,
    },
    ConnectionKey {
        key: "sslmode",
        label: "SSL MODE",
        field: Field::Choice(SSL_MODES),
        slot: Slot::Setting,
        group: SSL,
        required: false,
        default: Some("prefer"),
        when: None,
        hint: Some("Handed to the driver as written. 'prefer' encrypts when the server offers it"),
        placeholder: None,
    },
    ConnectionKey {
        key: "sslrootcert",
        label: "ROOT CERTIFICATE",
        field: Field::Path,
        slot: Slot::Setting,
        group: SSL,
        required: false,
        default: None,
        when: Some(When {
            key: "sslmode",
            values: SSL_VERIFYING,
        }),
        hint: Some("Blank uses this machine's own trust store"),
        placeholder: Some("/path/to/root.pem"),
    },
];

/// One connection's settings, read off the def's config map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PgSettings {
    /// The role this connection logs in as. Half its identity in practice: two roles over one
    /// database see two sets of schemas.
    pub user: String,
    /// libpq's own word for how the connection is encrypted.
    pub sslmode: String,
    /// A root-certificate file path, for the two verifying modes. The path, never the contents.
    pub sslrootcert: String,
}

/// Whether `sslmode` reads a root certificate — [`SSL_VERIFYING`] asked of one value.
pub fn verifies(sslmode: &str) -> bool {
    SSL_VERIFYING.contains(&sslmode)
}

impl PgSettings {
    /// Reads `config` as this source's settings, defaulting what it does not say.
    ///
    /// # Errors
    ///
    /// If a required key is missing or a value is one the connection string cannot carry — the
    /// two things a form can be wrong about. Everything else is the server's to judge.
    pub fn read(config: &BTreeMap<String, String>) -> Result<Self, String> {
        let value = |key: &str| config.get(key).map(|v| v.trim()).unwrap_or_default();
        let user = value("user");
        check_user(user)?;
        let sslmode = match value("sslmode") {
            "" => "prefer",
            mode => mode,
        };
        if !SSL_MODES.contains(&sslmode) {
            return Err(format!(
                "'{sslmode}' is not an SSL mode. Use one of {}.",
                SSL_MODES.join(", ")
            ));
        }
        Ok(Self {
            user: user.to_string(),
            sslmode: sslmode.to_string(),
            sslrootcert: value("sslrootcert").to_string(),
        })
    }

    /// Whether this connection's mode reads [`sslrootcert`](Self::sslrootcert).
    pub fn verifies(&self) -> bool {
        verifies(&self.sslmode)
    }
}

/// Whether `user` is a role this connection can actually log in as.
///
/// **Refused by name, because the layer below refuses it namelessly.** A role holding a space or
/// an `=` produces `user=read only dbname=…`, which the parser rejects in words naming neither the
/// field nor the value.
pub fn check_user(user: &str) -> Result<(), String> {
    let user = user.trim();
    if user.is_empty() {
        return Err("This connection has no user.".into());
    }
    if user.contains('@') {
        return Err("A PostgreSQL user can't contain '@'.".into());
    }
    check_conn_value("user", user)
}

/// A database connection's address, taken apart — the **one** parse of `host:port/database`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PgAddress<'a> {
    /// Unbracketed: `[::1]` arrives here as `::1`, which is what a driver's `host=` takes.
    pub host: &'a str,
    pub port: u16,
    pub database: &'a str,
}

/// Reads `address` as `host:port/database`, or says what is wrong with it in the field's own
/// terms.
///
/// The port is not optional and not defaulted to 5432: a def whose address reads
/// `db.internal/analytics` while it means `:5432` shows one thing and connects to another.
/// Userinfo is refused because the role is its own declared key.
///
/// The **connection-string** rules ([`check_conn_value`]) apply to the host and the database for
/// the same reason they apply to the user: all three are interpolated into a libpq string with no
/// quoting.
pub fn parse_address(address: &str) -> Result<PgAddress<'_>, String> {
    if address.is_empty() {
        return Err("This connection has no server.".into());
    }
    if address.chars().any(char::is_whitespace) {
        return Err("A PostgreSQL address can't contain spaces.".into());
    }
    if address.contains("://") {
        return Err(
            "A PostgreSQL address is 'host:port/database', without a scheme. Drop the '://'."
                .into(),
        );
    }
    if let Some(at) = address.find('@') {
        return Err(format!(
            "A PostgreSQL address can't carry a user or password. Drop '{}' and set the user in \
             its own field.",
            &address[..=at],
        ));
    }
    let Some((server, database)) = address.split_once('/') else {
        return Err(
            "A PostgreSQL connection needs a database: write 'host:5432/analytics'.".into(),
        );
    };
    if database.is_empty() {
        return Err(
            "A PostgreSQL connection needs a database: write 'host:5432/analytics'.".into(),
        );
    }
    if database.contains('/') {
        return Err("A PostgreSQL address names one database, so it has one '/'.".into());
    }
    check_conn_value("database", database)?;
    let Some((host, port)) = server.rsplit_once(':') else {
        return Err(format!(
            "A PostgreSQL connection needs a port: write '{server}:5432/{database}'."
        ));
    };
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() {
        return Err("A PostgreSQL connection needs a host.".into());
    }
    check_conn_value("host", host)?;
    let port = match port.parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => return Err(format!("'{port}' is not a port number.")),
    };
    Ok(PgAddress {
        host,
        port,
        database,
    })
}

/// Whether `value` is one a libpq connection string can carry — the rule the user, the host and
/// the database all share.
///
/// **Refused by name, because the layer below refuses it namelessly or not at all.** The driver's
/// parameters are assembled by plain interpolation and its parser reads `\` as an escape and `'`
/// as a quote, so a database named `sales\2024` parses as `sales2024` — the app would connect to
/// and federate a **different database** with nothing saying so. `PostgreSQL` creates all of these
/// happily; they simply cannot be dialled through this stack.
fn check_conn_value(noun: &str, value: &str) -> Result<(), String> {
    if value.chars().any(char::is_whitespace) {
        return Err(format!("A PostgreSQL {noun} can't contain spaces."));
    }
    match value.chars().find(|c| matches!(c, '=' | '\'' | '\\')) {
        Some(bad) => Err(format!("A PostgreSQL {noun} can't contain '{bad}'.")),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    /// **A database address is `host:port/database`**, and every part is required: the port
    /// because a `PostgreSQL` off 5432 is the ordinary case for a container or a pooler, the
    /// database because there is no server-wide connection to make.
    #[test]
    fn an_address_is_a_server_and_a_database() {
        for good in [
            "db.internal:5432/analytics",
            "localhost:5432/postgres",
            "127.0.0.1:65535/a",
            "::1:5432/analytics",
            "[::1]:5432/analytics",
        ] {
            assert!(parse_address(good).is_ok(), "{good}");
        }
        for (bad, why) in [
            ("", "no server"),
            ("db.internal:5432 /analytics", "spaces"),
            ("postgres://db:5432/analytics", "://"),
            ("reader@db:5432/analytics", "user or password"),
            ("db.internal:5432", "needs a database"),
            ("db.internal:5432/", "needs a database"),
            ("db.internal/analytics", "needs a port"),
            (":5432/analytics", "needs a host"),
            ("db.internal:0/analytics", "not a port number"),
            ("db.internal:pg/analytics", "not a port number"),
            ("db.internal:99999/analytics", "not a port number"),
            ("db.internal:5432/a/b", "one '/'"),
        ] {
            let message = parse_address(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
    }

    /// **A role is checked on the same terms as an address**: a space or an `=` in the user fails
    /// not as "that user is wrong" but as a connection string the parser cannot read.
    #[test]
    fn a_user_is_one_the_connection_string_can_carry() {
        for good in ["reader", "app_user", "analytics-ro", "READER"] {
            assert_eq!(check_user(good), Ok(()), "{good}");
        }
        for (bad, why) in [
            ("", "no user"),
            ("   ", "no user"),
            ("read only", "spaces"),
            ("user=x", "'='"),
            ("o'brien", "'''"),
            ("dom\\user", "'\\'"),
            ("reader@db", "'@'"),
        ] {
            let message = check_user(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
    }

    /// A def says what it says and the declaration answers for the rest — including libpq's own
    /// default, which is what an unset mode means rather than "no encryption".
    #[test]
    fn the_settings_default_the_way_the_keys_declare() {
        let read =
            PgSettings::read(&config(&[("user", "reader")])).expect("a user is all it needs");
        assert_eq!(read.sslmode, "prefer", "libpq's own default");
        assert!(read.sslrootcert.is_empty());
        assert!(!read.verifies());

        let verifying = PgSettings::read(&config(&[
            ("user", "reader"),
            ("sslmode", "verify-full"),
            ("sslrootcert", "/certs/rds.pem"),
        ]))
        .expect("a verifying connection");
        assert!(verifying.verifies());
        assert_eq!(verifying.sslrootcert, "/certs/rds.pem");

        assert!(
            PgSettings::read(&config(&[])).is_err(),
            "the user is required"
        );
        let why = PgSettings::read(&config(&[("user", "reader"), ("sslmode", "maybe")]))
            .expect_err("not a mode");
        assert!(why.contains("verify-full"), "{why}");
    }

    /// **The certificate is offered by the modes that read it, and by no other**, off the one
    /// list [`verifies`] answers from — so a box that can do nothing is never on screen, and what
    /// the form offers cannot drift from what the driver does.
    #[test]
    fn the_certificate_is_shown_by_the_modes_that_read_it() {
        let cert = KEYS
            .iter()
            .find(|key| key.key == "sslrootcert")
            .expect("the declaration");
        let when = cert.when.expect("offered conditionally");
        assert_eq!(when.key, "sslmode");
        assert_eq!(when.values, SSL_VERIFYING);
        for mode in SSL_MODES {
            assert_eq!(
                when.values.contains(mode),
                verifies(mode),
                "'{mode}': the form and the driver disagree about reading a certificate"
            );
        }
    }

    /// The declaration and the reader are one vocabulary: every key the form draws is one the
    /// reader looks for, and the mode choices are the ones it accepts.
    #[test]
    fn every_declared_key_is_one_the_reader_reads() {
        let declared: Vec<&str> = KEYS
            .iter()
            .filter(|key| key.slot == Slot::Setting)
            .map(|key| key.key)
            .collect();
        assert_eq!(
            declared,
            vec!["user", "password", "sslmode", "sslrootcert"],
            "the address is declared too, but it is `parse_address`'s to read and not this"
        );
        let modes = KEYS
            .iter()
            .find(|key| key.key == "sslmode")
            .map(|key| key.field);
        assert_eq!(modes, Some(Field::Choice(SSL_MODES)));
        assert_eq!(
            KEYS.iter()
                .find(|key| key.key == PASSWORD)
                .map(|key| key.field),
            Some(Field::Secret),
            "the password is a declared key, so its value never reaches the def"
        );
        for mode in SSL_MODES {
            assert!(
                PgSettings::read(&config(&[("user", "r"), ("sslmode", mode)])).is_ok(),
                "{mode}"
            );
        }
    }
}
