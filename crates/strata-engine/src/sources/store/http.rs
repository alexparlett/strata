//! **A plain HTTP origin.** The store over a whole origin URL, which is what an HTTP data source's
//! address is.
//!
//! The registrant with no authorisation at all, and the one exempt from the reachability probe:
//! its store lists over WebDAV `PROPFIND`, which most file origins do not implement (MinIO
//! included), so probing one would refuse working data sources for a verb their server was never
//! going to answer. An HTTP data source names a whole origin and its table names the object, so the
//! table's own registration tests its reachability.
//!
//! Its address is already a URL, so the scheme it was typed with is the answer to whether plain
//! HTTP is allowed — derived here rather than offered as a setting, because there is nothing for
//! the user to decide that they have not already decided by typing `http://`.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use object_store::http::HttpBuilder;
use object_store::ClientConfigKey;

use strata_model::SourceDef;

use super::{built, check_http_url, client_options, client_settings};
use crate::secrets::SecretProvider;
use crate::sources::source::{
    ConnectRefusal, DataSource, Field, SourceKind, SourceMode, SourceSetting, Sourced,
};

const OWN: &[SourceSetting] = &[SourceSetting {
    key: "address",
    label: "URL",
    field: Field::Text,
    group: Some("ORIGIN"),
    required: true,
    default: None,
    when: None,
    hint: Some("The whole origin, scheme included. A path belongs to the table that reads it"),
    placeholder: Some("https://files.example.com"),
}];

static SETTINGS: LazyLock<Vec<SourceSetting>> =
    LazyLock::new(|| [OWN, &client_settings()].concat());

/// A public HTTP(S) origin.
#[derive(Debug)]
pub struct Http;

impl SourceKind for Http {
    const NAME: &'static str = "http";
    const LABEL: &'static str = "HTTP";
    const BADGE: &'static str = "HTTP";
    const MODE: SourceMode = SourceMode::Store;
    /// Two of these on one address would register a single URL between them, so tables under
    /// either would resolve through whichever went in last.
    const UNIQUE: &'static [&'static str] = &["address"];
    /// Its address already carries one, and `http` and `https` are two different origins that only
    /// the person typing knows between — so this is what the *registry* keys on while the address
    /// is what a path hangs off.
    const SCHEME: Option<&'static str> = Some("http");
}

#[async_trait]
impl DataSource for Http {
    fn settings(&self) -> &'static [SourceSetting] {
        &SETTINGS
    }

    /// A whole origin, scheme and all — and **userinfo is refused rather than trimmed**, because a
    /// `https://user:pass@host` pasted into the box would put a password in the committed
    /// `project.json`.
    fn check_address(&self, address: &str) -> Result<(), String> {
        check_http_url(address)
    }

    async fn connect(
        &self,
        def: &SourceDef,
        _secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, ConnectRefusal> {
        let origin = def.setting("address");
        let mut builder = HttpBuilder::new().with_url(origin).with_config(
            ClientConfigKey::AllowHttp,
            origin.starts_with("http://").to_string(),
        );
        for (key, value) in client_options(def) {
            builder = builder.with_config(key, value);
        }
        Ok(Sourced::Store {
            store: built(def, builder.build())?,
        })
    }
}
