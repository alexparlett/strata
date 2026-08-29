//! **A plain HTTP origin.** The store over a whole origin URL, which is what an HTTP connection's
//! address is.
//!
//! The arm with no authorisation at all, and the one [`reachable`](super::reachable) exempts from
//! the probe. Its address is already a URL, so the scheme it was typed with is the answer to
//! whether plain HTTP is allowed — `allow_http` is derived here rather than offered as a setting,
//! because there is nothing for the user to decide that they have not already decided by typing
//! `http://`.

use std::sync::Arc;

use object_store::http::HttpBuilder;
use object_store::{ClientConfigKey, ObjectStore};

use strata_model::ConnectionDef;

use super::built;

/// The origin's store: the URL, the derived plain-HTTP toggle and the client options.
///
/// Every way it can fail is a way of describing the connection wrong, which is what lets
/// [`connect`](super::connect) treat the registration itself as one line with one meaning.
pub(super) fn build(
    conn: &ConnectionDef,
    options: &[(ClientConfigKey, String)],
) -> Result<Arc<dyn ObjectStore>, String> {
    let origin = conn.address.trim();
    let mut builder = HttpBuilder::new().with_url(origin).with_config(
        ClientConfigKey::AllowHttp,
        origin.starts_with("http://").to_string(),
    );
    for (key, value) in options {
        builder = builder.with_config(*key, value);
    }
    built(conn, builder.build())
}
