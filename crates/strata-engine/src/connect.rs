//! **The contract every connection registers under**, written once: a connection is applied to
//! the session or it is taken back, never both and never neither.
//!
//! Two arms register two different things — an object store keyed by the URL its identity
//! derives ([`store`](super::store)) and a source's catalog keyed by its SQL name
//! ([`sources`](super::sources)) — and their take-backs are different code, because the
//! registries are.
//! What is *not* different, and is therefore here rather than in each of them, is the rule:
//! **on `Err`, whatever this connection last registered comes out.**
//!
//! That rule has a burn scar behind it. `store`'s first version of the split registered on `Ok`
//! and simply returned on `Err`, which silently dropped the deregistration — and the test whose
//! whole subject is "a refused reconnect leaves nothing behind" went red against a stand-in that
//! could never have passed it. A contract restated per provider is a contract with a place to go
//! wrong; the take-back is passed *in*, so a caller cannot forget to have one.
//!
//! Why it matters: a connection's outcome folds onto a single `Reg` row, so a connection cannot
//! be both refused and live. Leaving the old registration behind would produce exactly that — a
//! row reading `Failed` over a bucket, or a catalog, that the engine still answers for.

use std::sync::Arc;

use datafusion::catalog::CatalogProvider;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::*;
use object_store::ObjectStore;

/// What one connection puts on the session — the two registries a `ConnectionDef` can reach.
pub enum Registration {
    /// An object store, under the connection's own URL: what a scan of `s3://acme-lake/…`
    /// resolves through (W7).
    ObjectStore(ObjectStoreUrl, Arc<dyn ObjectStore>),
    /// A catalog, under the name the connection chose: what `pg.public.orders` resolves
    /// through (DB).
    Catalog(String, Arc<dyn CatalogProvider>),
}

/// Apply `prepared`, or run `take_back` and report why — see the module docs.
///
/// `take_back` is called for **every** failure, including one raised before anything could have
/// been registered: a connection that has never worked simply has nothing to take back, and both
/// arms' removals are silent about a key with nothing behind it. Making the caller distinguish
/// would be one more thing to get right per provider, for no behaviour.
pub fn settle(
    ctx: &SessionContext,
    prepared: Result<Registration, String>,
    take_back: impl FnOnce(),
) -> Result<(), String> {
    match prepared {
        Ok(Registration::ObjectStore(url, store)) => {
            ctx.register_object_store(url.as_ref(), store);
            Ok(())
        }
        Ok(Registration::Catalog(name, catalog)) => {
            ctx.register_catalog(name, catalog);
            Ok(())
        }
        Err(why) => {
            take_back();
            Err(why)
        }
    }
}
