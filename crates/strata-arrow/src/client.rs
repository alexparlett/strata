//! The **client-option catalogue**: every tunable of the HTTP client an object store connection
//! may set, and the check the connection editor and the store build both make against it.
//!
//! Building the store is the engine's (`strata_engine::store`); the catalogue is here because the
//! editor's picker offers from it and the refusal it shows has to be the same one the build makes.

use std::collections::BTreeMap;

/// One tunable of the HTTP client every object store is built on — `object_store`'s
/// [`ClientConfigKey`](object_store::ClientConfigKey), with the sentence the editor's picker shows beside it.
///
/// The **name** is `ClientConfigKey`'s own (`AsRef<str>`), so what a connection stores is what
/// `from_str` reads back; the description is ours, because the crate's is a doc comment and not a
/// value.
#[derive(PartialEq, Eq, Debug)]
pub struct ClientKey {
    pub name: &'static str,
    pub what: &'static str,
}

/// Every client option a connection may set, in the order the picker offers them.
///
/// A written-down table, because `ClientConfigKey` has no enumeration of itself — the same shape
/// and the same reason as [`ENGINE_KEYS`](crate::config::ENGINE_KEYS) for DataFusion's settings. It
/// is kept honest from the other side: [`check_client_config`] parses every name through
/// `ClientConfigKey::from_str`, so a typo here is a test failure rather than an option that
/// silently never applies (`tests::every_offered_client_key_is_one_object_store_knows`).
///
/// Two of `object_store`'s keys are deliberately **absent**, and for the same reason in both
/// halves: they are already said elsewhere, and a second control for one setting is two controls
/// that can disagree. `allow_http` is the S3 provider's own
/// [`S3Store::allow_http`](strata_model::S3Store::allow_http) toggle, and on an HTTP connection it
/// is the **scheme the user typed** (the store's build derives it); `default_content_type`
/// describes an upload, and nothing here writes.
pub const CLIENT_KEYS: &[ClientKey] = &[
    ClientKey {
        name: "timeout",
        what: "Whole-request timeout, from connect to the last byte of the body (30s, 500ms)",
    },
    ClientKey {
        name: "connect_timeout",
        what: "Timeout for the connect phase alone",
    },
    ClientKey {
        name: "pool_idle_timeout",
        what: "How long an idle connection is kept alive",
    },
    ClientKey {
        name: "pool_max_idle_per_host",
        what: "Maximum idle connections kept per host",
    },
    ClientKey {
        name: "allow_invalid_certificates",
        what: "Trust any TLS certificate. Every site, including expired ones - a last resort",
    },
    ClientKey {
        name: "proxy_url",
        what: "HTTP proxy to send requests through",
    },
    ClientKey {
        name: "proxy_ca_certificate",
        what: "PEM certificate authority for the proxy connection",
    },
    ClientKey {
        name: "proxy_excludes",
        what: "Hosts that bypass the proxy",
    },
    ClientKey {
        name: "user_agent",
        what: "User-Agent header this connection sends",
    },
    ClientKey {
        name: "http1_only",
        what: "Use HTTP/1 only",
    },
    ClientKey {
        name: "http2_only",
        what: "Use HTTP/2 only",
    },
    ClientKey {
        name: "http2_keep_alive_interval",
        what: "How often to send an HTTP/2 keep-alive ping",
    },
    ClientKey {
        name: "http2_keep_alive_timeout",
        what: "How long to wait for a keep-alive ping to be acknowledged",
    },
    ClientKey {
        name: "http2_keep_alive_while_idle",
        what: "Keep sending HTTP/2 pings while the connection is idle",
    },
    ClientKey {
        name: "http2_max_frame_size",
        what: "Maximum HTTP/2 frame size",
    },
    ClientKey {
        name: "randomize_addresses",
        what: "Shuffle resolved addresses, spreading connections over more servers",
    },
];

/// The catalogue entry for `name`, if this is a client option a connection may set.
pub fn client_key(name: &str) -> Option<&'static ClientKey> {
    CLIENT_KEYS.iter().find(|k| k.name == name)
}

/// Whether a connection's client options are ones the store will accept — **the same call the
/// connection editor makes**, so an option refused at the field is refused here in the same words.
///
/// Two failures, and neither is `object_store`'s to report: a name it has never heard of would be
/// dropped on the floor by `from_str` at build time with nothing said, and a blank value would be
/// handed to a parser that expects a duration or a boolean. Both are told here, naming the key.
pub fn check_client_config(config: &BTreeMap<String, String>) -> Result<(), String> {
    for (name, value) in config {
        if client_key(name).is_none() {
            return Err(format!(
                "'{name}' is not a client option Strata can set on a connection."
            ));
        }
        if value.trim().is_empty() {
            return Err(format!("The client option '{name}' has no value."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use object_store::ClientConfigKey;

    use super::*;

    /// **Every name we offer is one `object_store` answers to.** The catalogue is written down
    /// (`ClientConfigKey` cannot enumerate itself), so this is what keeps it from drifting: a
    /// typo would otherwise be a picker entry that parses to nothing and silently never applies.
    #[test]
    fn every_offered_client_key_is_one_object_store_knows() {
        for key in CLIENT_KEYS {
            let parsed: ClientConfigKey = key
                .name
                .parse()
                .unwrap_or_else(|_| panic!("object_store does not know '{}'", key.name));
            assert_eq!(parsed.as_ref(), key.name);
            assert!(!key.what.is_empty(), "{} has no description", key.name);
        }
        for absent in ["allow_http", "default_content_type"] {
            assert!(
                absent.parse::<ClientConfigKey>().is_ok(),
                "still a real key, just not offered"
            );
            assert!(client_key(absent).is_none(), "{absent} is not offered");
        }
    }
}
