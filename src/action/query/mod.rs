//! Query / results / saved-query action handlers, grouped by concern and re-exported here so
//! `action::dispatch` (and `catalog::menu_action`) keep calling `query::<handler>` unchanged.
//! Each submodule owns one concern; this file is just the wiring.

mod copy;
mod export;
mod paging;
mod run;
mod saved;
mod sql;
mod view;

pub use copy::*;
pub use export::*;
pub use paging::*;
pub use run::*;
pub use saved::*;
pub use sql::*;
pub use view::*;
