//! **Google Cloud Storage.** The bucket's store, and the three ways one is authorised.
//!
//! No credential bridge of its own: `object_store` resolves the ambient chain (the metadata
//! server, `GOOGLE_APPLICATION_CREDENTIALS`, `gcloud`'s own application-default file) and a
//! service account is a **path** to a key file the OS already lets the user read. Nothing here
//! takes a secret value, which is why it declares no [`Field::Secret`] key.

use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};

use strata_model::SourceDef;

use super::{built, check_gcs_bucket, client_options, client_settings, probe};
use crate::secrets::SecretProvider;
use crate::sources::source::{
    ConnectRefusal, DataSource, Field, SourceKind, SourceMode, SourceSetting, Sourced, When,
};

/// How this bucket is authorised — GCS's three, in its own words.
pub const AUTH: &[&str] = &["ambient", "service-account", "anonymous"];
const KEY_FILE: &[&str] = &["service-account"];

const BUCKET: Option<&str> = Some("BUCKET");
const AUTH_GROUP: Option<&str> = Some("AUTHENTICATION");

const OWN: &[SourceSetting] = &[
    SourceSetting {
        key: "address",
        label: "BUCKET",
        field: Field::Text,
        group: BUCKET,
        required: true,
        default: None,
        when: None,
        hint: Some("The bucket name alone. A path belongs to the table that reads it"),
        placeholder: Some("my-bucket"),
    },
    SourceSetting {
        key: "auth",
        label: "AUTHENTICATION",
        field: Field::Choice(AUTH),
        group: AUTH_GROUP,
        required: false,
        default: Some("ambient"),
        when: None,
        hint: Some(
            "'ambient' is Application Default Credentials: the metadata server, \
             GOOGLE_APPLICATION_CREDENTIALS, or gcloud's own file",
        ),
        placeholder: None,
    },
    SourceSetting {
        key: "service_account_path",
        label: "SERVICE-ACCOUNT FILE",
        field: Field::Path,
        group: AUTH_GROUP,
        required: true,
        default: None,
        when: Some(When {
            key: "auth",
            values: KEY_FILE,
        }),
        hint: Some("A path to the key file. The JSON is never read into or stored by Strata"),
        placeholder: Some("/path/to/service-account.json"),
    },
];

static SETTINGS: LazyLock<Vec<SourceSetting>> =
    LazyLock::new(|| [OWN, &client_settings()].concat());

/// Google Cloud Storage.
#[derive(Debug)]
pub struct Gcs;

impl SourceKind for Gcs {
    const NAME: &'static str = "gcs";
    const LABEL: &'static str = "GCS";
    const BADGE: &'static str = "GCS";
    const MODE: SourceMode = SourceMode::Store;
    /// Two of these on one address would register a single URL between them, so tables under
    /// either would resolve through whichever went in last.
    const UNIQUE: &'static [&'static str] = &["address"];
    /// `gs`, not `gcs`: the kind is named the way a person says it and the scheme the way a path
    /// is written, which is exactly why the two are separate consts.
    const SCHEME: Option<&'static str> = Some("gs");
}

#[async_trait]
impl DataSource for Gcs {
    fn settings(&self) -> &'static [SourceSetting] {
        &SETTINGS
    }

    /// <https://cloud.google.com/storage/docs/buckets#naming>. Looser than S3 in four places — a
    /// dotted name may run longer, and underscores are allowed — so the two rules are written
    /// separately rather than one being reused for both.
    fn check_address(&self, address: &str) -> Result<(), String> {
        check_gcs_bucket(address)
    }

    async fn connect(
        &self,
        def: &SourceDef,
        _secrets: Arc<dyn SecretProvider>,
    ) -> Result<Sourced, ConnectRefusal> {
        let value = |key: &str| def.config.get(key).map(|v| v.trim()).unwrap_or_default();
        let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(def.setting("address"));
        builder = match value("auth") {
            "anonymous" => builder.with_skip_signature(true),
            "service-account" => {
                let path = value("service_account_path");
                if path.is_empty() {
                    return Err("This GCS data source needs a service-account file.".into());
                }
                builder.with_service_account_path(path)
            }
            _ => builder,
        };
        for (key, value) in client_options(def) {
            builder = builder.with_config(GoogleConfigKey::Client(key), value);
        }
        let store = built(def, builder.build())?;
        probe(&store, || {
            format!("The bucket '{}' did not answer.", def.setting("address"))
        })
        .await?;
        Ok(Sourced::Store { store })
    }
}
