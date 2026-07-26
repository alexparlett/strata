//! The per-window stores (Radio): the **Session** (open tabs + arrangement) and the
//! **Project** (the open project's catalog defs — the save targets).
//! See `docs/FREYA_STATE_ARCHITECTURE.md` §2–§4.

mod catalog;
mod channel;
mod diagnostics;
mod history;
mod hooks;
mod project;
mod session;

pub use catalog::{
    catalog_settled, use_catalog, use_catalog_rescan, use_catalog_selection,
    use_init_catalog_selection, Catalog, CatalogRescan, CatalogSelection,
};
/// Only tests name these: they stand the catalog's context signals up by hand, where the window
/// goes through `use_init_catalog` / `use_init_catalog_rescan`. Production code reaches
/// `CatalogState` through `use_catalog()`'s methods, never by naming the type.
#[cfg(test)]
pub use catalog::{CatalogState, ScanRequest, ScanScope};
pub use channel::Chan;
pub use diagnostics::use_diagnostics;
pub use history::use_history_recording;
pub use hooks::{
    refresh_catalog, refresh_table, use_autosave, use_init_history, use_init_project,
    use_init_session,
};
pub use project::{ProjChan, ProjectState, Reg};
pub use session::{ProblemGroup, SessionState, Stamp};
