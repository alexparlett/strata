//! **Connections** — the persisted description of one remote object store a project reads
//! from (W7, `docs/CONNECTIONS_SPEC.md`). Exactly what `.strata/project.json` stores, like
//! the catalog defs beside it.
//!
//! The rule the whole feature is built around: **Strata never stores, prompts for, or reads
//! a secret.** A connection carries only non-secret metadata — a bucket, a region, an
//! endpoint — plus a *reference* to where credentials live (a named `~/.aws` profile, a
//! service-account key **file path**). Credentials resolve at query time from the host's own
//! provider chains, so nothing here is a key, and nothing here has to be gitignored: the
//! whole def is shareable, which is why it rides the committed `project.json` rather than
//! the local `session.json`.
//!
//! There is no arm for a secret, anywhere in this module. That is the enforcement: an
//! access-key field cannot be added without adding a variant that says so out loud.

use std::fmt;

use serde::{Deserialize, Serialize};

/// One project-scoped connection: the bucket it names, and the provider that serves it.
///
/// **Identity is [`url`](Self::url), not the bucket** — scheme *and* authority, which is
/// exactly what DataFusion's object-store registry keys on (see `strata_core::engine::store`).
/// The distinction is not academic: `s3://lake` and `gs://lake` share a bucket and are two
/// different connections over two different stores, so anything addressing one of them —
/// a registration outcome, a store row, a Configure dropdown — has to say which.
///
/// The bucket is stored as the **authority alone** (`acme-lake`, `example.com:8080`) and the
/// scheme comes from the provider. Storing the scheme-qualified string would be two statements
/// of one fact, and they can disagree: an `s3://` bucket under a GCS provider is a def that
/// reads one way and registers another.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
pub struct ConnectionDef {
    /// The bucket (S3 / GCS) or host (HTTP) — the URL **authority**, without a scheme and
    /// without a path.
    pub bucket: String,
    /// Which object store this is, and the settings that store takes.
    pub provider: Provider,
}

impl ConnectionDef {
    /// The URL this connection registers under — `s3://acme-lake`, `gs://lake`,
    /// `https://example.com`. Scheme + authority and nothing else, because that is the whole
    /// of what the object-store registry keys on.
    pub fn url(&self) -> String {
        format!("{}://{}", self.provider.scheme(), self.bucket)
    }
}

/// **A connection's provider, and the settings that provider takes** — one field, not a
/// provider string beside a settings bag.
///
/// The same argument as [`SourceFormat`](crate::SourceFormat): the two are not independent.
/// A region means nothing to the HTTP store, and a def carrying both a provider and every
/// provider's fields has states where they disagree — an S3 region set on a GCS bucket,
/// silently ignored, and shown by whatever surface renders it. Here the provider *is* the
/// settings, so that state cannot be written down.
///
/// Three providers, and deliberately no fourth: Azure was dropped in the spec's v11.
/// S3-compatible stores (Cloudflare R2, MinIO, Alibaba OSS, Tencent COS) ride [`S3`](Self::S3)
/// via its [`endpoint`](S3Store::endpoint) rather than each becoming a provider of its own.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "provider", rename_all = "lowercase")]
pub enum Provider {
    S3(S3Store),
    Gcs(GcsStore),
    /// A public HTTP(S) origin. No settings and no auth: reads are anonymous.
    Http,
}

impl Provider {
    /// The URL scheme this provider registers under, and the one a source path has to carry
    /// to reach it.
    pub fn scheme(&self) -> &'static str {
        match self {
            Self::S3(_) => "s3",
            Self::Gcs(_) => "gs",
            Self::Http => "https",
        }
    }
}

/// How a provider is **named to the user** — `S3` / `GCS` / `HTTP`.
///
/// Deliberately not [`scheme`](Provider::scheme), which is the URL's word (`gs`, `https`) and
/// belongs to the registry rather than to a reader. The two say different things about the same
/// value and both are needed, which is why the product's name lives here and not at whichever
/// surface happened to want it first: the Connections pane's row badge and the Configure
/// window's connection picker (W7 · 04) have to agree, and a name typed twice is a name that
/// can disagree.
impl fmt::Display for Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::S3(_) => "S3",
            Self::Gcs(_) => "GCS",
            Self::Http => "HTTP",
        })
    }
}

/// How an S3 (or S3-compatible) bucket is reached.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct S3Store {
    /// **Required**, and load-bearing: `object_store` does not derive a bucket's region
    /// reliably (arrow-rs#2795) and silently defaults to `us-east-1`, which reads a
    /// different bucket's worth of nothing. `strata_core::engine::store` refuses a blank one
    /// rather than letting that default stand.
    pub region: String,
    pub auth: S3Auth,
    /// An S3-**compatible** endpoint (R2 / MinIO / OSS / COS). Empty means AWS itself.
    pub endpoint: String,
    /// Allow a plain-`http` endpoint — a MinIO on a workstation. Only meaningful with
    /// [`endpoint`](Self::endpoint) set; AWS is HTTPS.
    pub allow_http: bool,
}

/// Where S3 credentials come from. Every mode is secret-free by construction — there is no
/// variant carrying a key, a secret or a token, and that absence is the feature.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum S3Auth {
    /// The host's own chain, whatever it happens to be: environment variables, then the
    /// `~/.aws` profiles, SSO, `credential_process`, web identity, ECS, IMDS.
    #[default]
    Ambient,
    /// One **named** profile from `~/.aws/config` — a reference to the user's own AWS
    /// configuration, never its contents.
    Profile { name: String },
    /// Unsigned requests: a public bucket.
    Anonymous,
}

/// How a GCS bucket is reached. Native to `object_store`; no extra SDK.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(default)]
pub struct GcsStore {
    pub auth: GcsAuth,
}

/// Where GCS credentials come from — secret-free like [`S3Auth`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub enum GcsAuth {
    /// Application Default Credentials: `GOOGLE_APPLICATION_CREDENTIALS`, then the gcloud
    /// ADC file, then the GCE/GKE metadata server.
    #[default]
    Ambient,
    /// A service-account JSON key **file path**. The path, never the key: inline SA JSON is
    /// exactly the secret this feature refuses to hold.
    ServiceAccount { path: String },
    /// Unsigned requests: a public bucket.
    Anonymous,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> ConnectionDef {
        serde_json::from_str(json).expect("a connection def")
    }

    /// The scheme is the provider's, not a second field — so `url()` is the registry key and
    /// there is no def whose scheme and provider disagree.
    #[test]
    fn the_url_is_the_provider_scheme_over_the_bucket() {
        let s3 = ConnectionDef {
            bucket: "acme-lake".into(),
            provider: Provider::S3(S3Store::default()),
        };
        assert_eq!(s3.url(), "s3://acme-lake");
        assert_eq!(
            ConnectionDef {
                bucket: "lake".into(),
                provider: Provider::Gcs(GcsStore::default()),
            }
            .url(),
            "gs://lake"
        );
        assert_eq!(
            ConnectionDef {
                bucket: "example.com:8080".into(),
                provider: Provider::Http,
            }
            .url(),
            "https://example.com:8080"
        );
    }

    /// The product's name and the URL's word are different strings for the same provider, and
    /// both are load-bearing: the badge says `GCS` where the registry key says `gs`. Pinned so a
    /// later edit cannot quietly collapse one into the other.
    #[test]
    fn a_provider_is_named_for_the_reader_and_schemed_for_the_registry() {
        for (provider, name, scheme) in [
            (Provider::S3(S3Store::default()), "S3", "s3"),
            (Provider::Gcs(GcsStore::default()), "GCS", "gs"),
            (Provider::Http, "HTTP", "https"),
        ] {
            assert_eq!(provider.to_string(), name);
            assert_eq!(provider.scheme(), scheme);
        }
    }

    #[test]
    fn each_provider_round_trips_with_its_own_settings() {
        for def in [
            ConnectionDef {
                bucket: "acme-lake".into(),
                provider: Provider::S3(S3Store {
                    region: "eu-west-2".into(),
                    auth: S3Auth::Profile {
                        name: "analytics".into(),
                    },
                    endpoint: "https://s3.example.net".into(),
                    allow_http: true,
                }),
            },
            ConnectionDef {
                bucket: "lake".into(),
                provider: Provider::Gcs(GcsStore {
                    auth: GcsAuth::ServiceAccount {
                        path: "/keys/reader.json".into(),
                    },
                }),
            },
            ConnectionDef {
                bucket: "example.com".into(),
                provider: Provider::Http,
            },
        ] {
            let json = serde_json::to_string(&def).expect("serialize");
            assert_eq!(parse(&json), def, "{json}");
        }
    }

    /// The persisted shape is the one `docs/CONNECTIONS_SPEC.md` §5 describes: a `provider`
    /// tag beside that provider's own non-secret fields. Pinned as literal JSON because the
    /// file is committed and shared — a round-trip through today's structs could not catch a
    /// tag or a field name changing under it.
    #[test]
    fn the_persisted_shape_is_the_tagged_provider() {
        let json = serde_json::to_string(&ConnectionDef {
            bucket: "acme-lake".into(),
            provider: Provider::S3(S3Store {
                region: "eu-west-2".into(),
                auth: S3Auth::Profile {
                    name: "analytics".into(),
                },
                ..Default::default()
            }),
        })
        .expect("serialize");
        assert_eq!(
            json,
            r#"{"bucket":"acme-lake","provider":{"provider":"s3","region":"eu-west-2","auth":{"mode":"profile","name":"analytics"},"endpoint":"","allow_http":false}}"#
        );
        assert_eq!(
            serde_json::to_string(&Provider::Http).expect("serialize"),
            r#"{"provider":"http"}"#
        );
        assert_eq!(
            serde_json::to_string(&GcsAuth::ServiceAccount {
                path: "/keys/r.json".into()
            })
            .expect("serialize"),
            r#"{"mode":"service-account","path":"/keys/r.json"}"#
        );
    }

    /// A provider's settings are all `#[serde(default)]`, so a def written before a setting
    /// existed still loads — the same rule the session snapshot's per-tab facets follow, and
    /// for the same reason: the file on disk is older than the code reading it after every
    /// release.
    #[test]
    fn a_def_predating_a_setting_loads_with_its_default() {
        let def =
            parse(r#"{"bucket":"acme-lake","provider":{"provider":"s3","region":"us-east-1"}}"#);
        assert_eq!(
            def.provider,
            Provider::S3(S3Store {
                region: "us-east-1".into(),
                auth: S3Auth::Ambient,
                endpoint: String::new(),
                allow_http: false,
            })
        );
        assert_eq!(
            parse(r#"{"bucket":"lake","provider":{"provider":"gcs"}}"#).provider,
            Provider::Gcs(GcsStore {
                auth: GcsAuth::Ambient
            })
        );
    }
}
