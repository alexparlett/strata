//! Reusable UI component library — the shared building blocks the app is composed
//! from, independent of any one feature or `AppState`. Each component owns its own
//! look and behaviour; callers hand in content (`children`) and callbacks.
//!
//! The **overlay** family (A3, egui-style containers you mount conditionally and hand
//! `children` to — see `docs/OVERLAY_ARCHITECTURE.md`): the [`Popup`] (anchored card /
//! optional dismiss backdrop), [`Dialog`] (centred, scrimmed), and [`Window`] (non-modal
//! floating panel) containers plus the [`MenuItem`] / [`MenuSep`] menu primitives.
//!
//! On top of `Popup` (S29, design system — see `docs/DESIGN_SYSTEM.md`): [`Select`] (the
//! single-select dropdown, trigger + `.ds-menu` card) and [`ContextMenu`] (right-click
//! menu). The lint hover popover (S27) is a `Popup{backdrop:false}` styled as a `.ds-callout`.

mod checkbox;
mod context_menu;
mod dialog;
mod menu;
mod popup;
mod select;
mod window;

pub use checkbox::Checkbox;
pub use context_menu::ContextMenu;
pub use dialog::Dialog;
pub use menu::{MenuItem, MenuSep};
pub use popup::{Point, Popup};
pub use select::{Select, SelectOption};
pub use window::{WinGeom, Window};
