//! What a `MySQL` data source is described by: the keys it declares, and how a def's config map
//! reads as them.
//!
//! The address is one of those keys, so the whole form is here — including the sentence explaining
//! each row.
//!
//! The declaration is the only place these names are written. The editor draws its rows from it,
//! [`MySettings::read`] parses a def by it, and the conformance tests check the two agree — so a
//! key added here reaches the form and the data source together.
//!
//! **Nothing here holds a secret value.** The password is a declared key like any other, and what
//! that means is that its *value* goes to the keystore or comes from the environment; the def
//! records only that it is set.

use std::collections::BTreeMap;

use crate::sources::source::{Field, SourceSetting};

/// The key a `MySQL` data source's password is filed under, in the keystore and in the def's
/// expectation set.
pub const PASSWORD: &str = "password";

/// The environment variable a `MySQL` client conventionally reads a password from.
pub const PASSWORD_ENV: &[&str] = &["MYSQL_PWD"];

/// The port a `MySQL` server answers on when nobody says otherwise — quoted in the refusal that
/// asks for one, never assumed for a def that omits it.
const DEFAULT_PORT: u16 = 3306;

/// The SSL modes the driver understands, in `MySQL`'s own spellings and weakest first, which reads
/// as a dial rather than a list.
///
/// The value is handed to the driver as written, so a rename here is a data source that fails with
/// 'Invalid value for parameter sslmode'. `MySQL`'s client names two more — `verify_ca` and
/// `verify_identity` — which this driver does not implement, so they are not offered: a picker
/// entry that always fails is worse than a mode nobody can choose.
pub const SSL_MODES: &[&str] = &["disabled", "preferred", "required"];

/// What an unset [`SSL`](SSL_MODES) mode means — `MySQL`'s own client default: encrypt when the
/// server offers it, and do not verify the certificate.
pub const SSL_DEFAULT: &str = "preferred";

/// The sections this source's rows sit in.
const CONNECTION: Option<&str> = Some("CONNECTION");
const SSL: Option<&str> = Some("SSL");

/// Every row a `MySQL` data source has, in the order it has them.
///
/// The address is a declared key like the rest, so nothing about a `MySQL` data source is written
/// down in the editor.
pub const SETTINGS: &[SourceSetting] = &[
    SourceSetting {
        key: "address",
        label: "ADDRESS",
        field: Field::Text,
        group: CONNECTION,
        required: true,
        default: None,
        when: None,
        hint: Some("The server. The port is not assumed, and a database on it is a schema"),
        placeholder: Some("localhost:3306"),
    },
    SourceSetting {
        key: "user",
        label: "USER",
        field: Field::Text,
        group: CONNECTION,
        required: true,
        default: None,
        when: None,
        hint: Some(
            "The account to log in as. What it may read is what this source shows, so changing \
             it is a different source",
        ),
        placeholder: None,
    },
    SourceSetting {
        key: PASSWORD,
        label: "PASSWORD",
        field: Field::Secret,
        group: CONNECTION,
        required: false,
        default: None,
        when: None,
        hint: None,
        placeholder: None,
    },
    SourceSetting {
        key: "ssl",
        label: "SSL MODE",
        field: Field::Choice(SSL_MODES),
        group: SSL,
        required: false,
        default: Some(SSL_DEFAULT),
        when: None,
        hint: Some(
            "Handed to the driver as written. 'preferred' encrypts without checking the \
             certificate",
        ),
        placeholder: None,
    },
];

/// One data source's settings, read off the def's config map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MySettings {
    /// The account this data source logs in as. Half its identity in practice: two accounts on one
    /// server are granted two sets of databases, and the grants are what the listing shows.
    pub user: String,
    /// The driver's own word for how the data source is encrypted.
    pub ssl: String,
}

impl MySettings {
    /// Reads `config` as this source's settings, defaulting what it does not say.
    ///
    /// # Errors
    ///
    /// If a required key is missing or a value is one the driver has no mode for — the two things
    /// a form can be wrong about. Everything else is the server's to judge.
    pub fn read(config: &BTreeMap<String, String>) -> Result<Self, String> {
        let value = |key: &str| config.get(key).map(|v| v.trim()).unwrap_or_default();
        let user = value("user");
        check_user(user)?;
        let ssl = match value("ssl") {
            "" => SSL_DEFAULT,
            mode => mode,
        };
        if !SSL_MODES.contains(&ssl) {
            return Err(format!(
                "'{ssl}' is not an SSL mode. Use one of {}.",
                SSL_MODES.join(", ")
            ));
        }
        Ok(Self {
            user: user.to_string(),
            ssl: ssl.to_string(),
        })
    }
}

/// Whether `user` is an account this data source can log in as.
///
/// Only emptiness is refused, and that is the whole rule: the driver takes the account as a typed
/// parameter rather than interpolating it into a connection string, so there is no spelling that
/// changes which server or account is reached. The `PostgreSQL` source refuses four characters for
/// exactly that reason, and this one has no equivalent to refuse.
pub fn check_user(user: &str) -> Result<(), String> {
    match user.trim().is_empty() {
        true => Err("This data source has no user.".into()),
        false => Ok(()),
    }
}

/// A data source's address, taken apart — the **one** parse of `host:port`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MyAddress<'a> {
    /// Unbracketed: `[::1]` arrives here as `::1`, which is what the driver's hostname takes.
    pub host: &'a str,
    pub port: u16,
}

/// Reads `address` as `host:port`, or says what is wrong with it in the field's own terms.
///
/// **The whole server, and no database on it.** A `MySQL` database is a namespace inside the
/// server and every one the account can read is a schema of this source, addressed as
/// `source.database.table` — the `DataGrip` model. Naming one in the address would make the other
/// databases unreachable and would put the same server in the project twice to reach two of them,
/// so it is refused rather than ignored.
///
/// The port is not optional and not defaulted to 3306: a def whose address reads `db.internal`
/// while it means `:3306` shows one thing and connects to another. Userinfo is refused because the
/// account is its own declared key.
pub fn parse_address(address: &str) -> Result<MyAddress<'_>, String> {
    let address = address.trim();
    if address.is_empty() {
        return Err("This data source has no server.".into());
    }
    if address.chars().any(char::is_whitespace) {
        return Err("A MySQL address can't contain spaces.".into());
    }
    if address.contains("://") {
        return Err("A MySQL address is 'host:port', without a scheme. Drop the '://'.".into());
    }
    if let Some(at) = address.find('@') {
        return Err(format!(
            "A MySQL address can't carry a user or password. Drop '{}' and set the user in its \
             own field.",
            &address[..=at],
        ));
    }
    if let Some((server, _)) = address.split_once('/') {
        return Err(format!(
            "A MySQL address is the server alone: write '{server}'. Every database the user can \
             read is a schema of this source, queried as 'source.database.table'."
        ));
    }
    let Some((host, port)) = address.rsplit_once(':') else {
        return Err(format!(
            "A MySQL data source needs a port: write '{address}:{DEFAULT_PORT}'."
        ));
    };
    let host = host
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty() {
        return Err("A MySQL data source needs a host.".into());
    }
    let port = match port.parse::<u16>() {
        Ok(port) if port > 0 => port,
        _ => return Err(format!("'{port}' is not a port number.")),
    };
    Ok(MyAddress { host, port })
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

    /// **A `MySQL` address is a server and nothing else** — the difference from `PostgreSQL` that
    /// the whole schema model follows from, so the refusal for a database segment names where the
    /// database went rather than saying the address is malformed.
    #[test]
    fn an_address_is_a_server_with_no_database_on_it() {
        for good in [
            "db.internal:3306",
            "localhost:3306",
            "127.0.0.1:65535",
            "::1:3306",
            "[::1]:3306",
        ] {
            assert!(parse_address(good).is_ok(), "{good}");
        }
        let why = parse_address("db.internal:3306/shop").expect_err("a database segment");
        assert!(
            why.contains("'db.internal:3306'") && why.contains("source.database.table"),
            "the refusal names the address to keep and where the database went: {why}"
        );
        for (bad, why) in [
            ("", "no server"),
            ("   ", "no server"),
            ("db.internal :3306", "spaces"),
            ("mysql://db:3306", "://"),
            ("reader@db:3306", "user or password"),
            ("db.internal", "needs a port"),
            (":3306", "needs a host"),
            ("db.internal:0", "not a port number"),
            ("db.internal:my", "not a port number"),
            ("db.internal:99999", "not a port number"),
        ] {
            let message = parse_address(bad).expect_err(bad);
            assert!(message.to_lowercase().contains(why), "{bad}: {message}");
        }
    }

    /// The port a missing one is asked for by is the server's own default, quoted into the fix.
    #[test]
    fn the_missing_port_refusal_names_the_default() {
        assert_eq!(
            parse_address("db.internal"),
            Err("A MySQL data source needs a port: write 'db.internal:3306'.".into())
        );
    }

    /// A def says what it says and the declaration answers for the rest — including the client's
    /// own default, which is what an unset mode means rather than "no encryption".
    #[test]
    fn the_settings_default_the_way_the_keys_declare() {
        let read =
            MySettings::read(&config(&[("user", "reader")])).expect("a user is all it needs");
        assert_eq!(read.ssl, SSL_DEFAULT);
        assert_eq!(read.user, "reader");

        assert!(
            MySettings::read(&config(&[])).is_err(),
            "the user is required"
        );
        let why = MySettings::read(&config(&[("user", "reader"), ("ssl", "verify_ca")]))
            .expect_err("not a mode this driver has");
        assert!(why.contains("preferred"), "{why}");
    }

    /// The declaration and the reader are one vocabulary: every key the form draws is one the
    /// reader looks for, and the mode choices are the ones it accepts.
    #[test]
    fn every_declared_key_is_one_the_reader_reads() {
        let declared: Vec<&str> = SETTINGS.iter().map(|key| key.key).collect();
        assert_eq!(
            declared,
            vec!["address", "user", "password", "ssl"],
            "the address is a setting like the rest — `parse_address` reads it, `MySettings` the \
             others"
        );
        assert_eq!(
            SETTINGS
                .iter()
                .find(|key| key.key == "ssl")
                .map(|key| key.field),
            Some(Field::Choice(SSL_MODES))
        );
        assert_eq!(
            SETTINGS
                .iter()
                .find(|key| key.key == PASSWORD)
                .map(|key| key.field),
            Some(Field::Secret),
            "the password is a declared key, so its value never reaches the def"
        );
        assert!(
            SSL_MODES.contains(&SSL_DEFAULT),
            "the default has to be one of the offered words"
        );
        for mode in SSL_MODES {
            assert!(
                MySettings::read(&config(&[("user", "r"), ("ssl", mode)])).is_ok(),
                "{mode}"
            );
        }
    }
}
