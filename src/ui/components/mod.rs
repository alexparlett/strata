//! Reusable UI component library — the shared building blocks the app is composed
//! from, independent of any one feature or `AppState`. Each component owns its own
//! look and behaviour; callers hand in content (`children`) and callbacks.
//!
//! First up is the **overlay** family (A3, egui-style containers you mount
//! conditionally and hand `children` to — see `docs/OVERLAY_ARCHITECTURE.md`): the
//! [`Popup`] (anchored menu) and [`Dialog`] (centred, scrimmed) containers plus the
//! [`MenuItem`] / [`MenuSep`] menu primitives. `Window` follows.

mod dialog;
mod menu;
mod popup;

pub use dialog::Dialog;
pub use menu::{MenuItem, MenuSep};
pub use popup::{Point, Popup};
