//! **Google Cloud Storage.** The bucket's store, and the three ways one is authorised.
//!
//! No credential bridge of its own: `object_store` resolves the ambient chain (the metadata
//! server, `GOOGLE_APPLICATION_CREDENTIALS`, `gcloud`'s own application-default file) and a
//! service account is a **path** to a key file the OS already lets the user read. Nothing here
//! takes a secret value.

use std::sync::Arc;

use object_store::gcp::{GoogleCloudStorageBuilder, GoogleConfigKey};
use object_store::{ClientConfigKey, ObjectStore};

use strata_model::{ConnectionDef, GcsAuth, GcsStore};

use super::built;

/// The bucket's store: the authorisation and the client options, in the one place GCS's rules are
/// written.
///
/// Every way it can fail is a way of describing the connection wrong, which is what lets
/// [`connect`](super::connect) treat the registration itself as one line with one meaning.
pub(super) fn build(
    conn: &ConnectionDef,
    gcs: &GcsStore,
    options: &[(ClientConfigKey, String)],
) -> Result<Arc<dyn ObjectStore>, String> {
    let mut builder = GoogleCloudStorageBuilder::new().with_bucket_name(conn.address.trim());
    builder = match &gcs.auth {
        GcsAuth::Ambient => builder,
        GcsAuth::ServiceAccount { path } => {
            let path = path.trim();
            if path.is_empty() {
                return Err("This GCS connection needs a service-account file.".into());
            }
            builder.with_service_account_path(path)
        }
        GcsAuth::Anonymous => builder.with_skip_signature(true),
    };
    for (key, value) in options {
        builder = builder.with_config(GoogleConfigKey::Client(*key), value);
    }
    built(conn, builder.build())
}
