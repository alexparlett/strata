//! **The contract every data source registers under**, written once: a data source is applied to
//! the session or it is taken back, never both and never neither.
//!
//! Two arms register two different things — an object store keyed by the URL its identity
//! derives ([`store`](super::store)) and a source's catalog keyed by its SQL name
//! ([`sources`](super::sources)) — and their take-backs are different code, because the
//! registries are.
//! What is *not* different, and is therefore here rather than in each of them, is the rule:
//! **on `Err`, whatever this data source last registered comes out.**
//!
//! That rule has a burn scar behind it. `store`'s first version of the split registered on `Ok`
//! and simply returned on `Err`, which silently dropped the deregistration — and the test whose
//! whole subject is "a refused reconnect leaves nothing behind" went red against a stand-in that
//! could never have passed it. A contract restated per provider is a contract with a place to go
//! wrong; the take-back is passed *in*, so a caller cannot forget to have one.
//!
//! Why it matters: a data source's outcome folds onto a single `Reg` row, so a data source cannot
//! be both refused and live. Leaving the old registration behind would produce exactly that — a
//! row reading `Failed` over a bucket, or a catalog, that the engine still answers for.

use std::sync::Arc;

use datafusion::catalog::CatalogProvider;
use datafusion::execution::object_store::ObjectStoreUrl;
use datafusion::prelude::*;
use object_store::ObjectStore;

/// What one data source puts on the session — the two registries a `SourceDef` can reach.
pub enum Registration {
    /// An object store, under the data source's own URL, and the catalog its tables are placed
    /// in, under the data source's own name.
    ///
    /// One arm because they are one act: a data source that put its store on the session and not
    /// its catalog would resolve every path and hold no table.
    Store {
        at: ObjectStoreUrl,
        store: Arc<dyn ObjectStore>,
        catalog: String,
        provider: Arc<dyn CatalogProvider>,
    },
    /// A catalog, under the name the data source chose: what `pg.public.orders` resolves
    /// through (DB).
    Catalog(String, Arc<dyn CatalogProvider>),
}

/// Apply `prepared`, or run `take_back` and report why — see the module docs.
///
/// `take_back` is called for **every** failure, including one raised before anything could have
/// been registered: a data source that has never worked simply has nothing to take back, and both
/// arms' removals are silent about a key with nothing behind it. Making the caller distinguish
/// would be one more thing to get right per provider, for no behaviour.
pub fn settle(
    ctx: &SessionContext,
    prepared: Result<Registration, String>,
    take_back: impl FnOnce(),
) -> Result<(), String> {
    match prepared {
        Ok(Registration::Store {
            at,
            store,
            catalog,
            provider,
        }) => {
            ctx.register_object_store(at.as_ref(), store);
            ctx.register_catalog(catalog, provider);
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
