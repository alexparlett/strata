//! Reusable UI component library — the shared building blocks the app is composed
//! from, independent of any one feature or `AppState`. Each component owns its own
//! look and behaviour; callers hand in content (`children`) and callbacks.
//!
//! The **overlay** family (A3, egui-style — see `docs/OVERLAY_ARCHITECTURE.md`). The base
//! is [`Popup`]: a dumb fixed-position card. Everything composes it:
//! - **menu / dropdown** = [`Backdrop`] `{ Popup { … } }` (backdrop owns dismiss + Esc + focus);
//! - **tooltip** = [`Tooltip`] = `Popup` + the pointer-transparent `ds-float` class.
//!
//! [`Dialog`] (centred, scrimmed) + [`Window`] (non-modal floating panel) are the other
//! containers; [`MenuItem`] / [`MenuSep`] are the shared menu rows.
//!
//! On the base (S29, design system — see `docs/DESIGN_SYSTEM.md`): [`Select`] (single-
//! select dropdown, trigger + `.ds-menu` card) and [`ContextMenu`] (right-click menu),
//! both `Backdrop { Popup }` internally. The S27 lint hover is a `Tooltip` (neutral
//! `.ds-tooltip` chrome + a red icon).

mod checkbox;
mod context_menu;
mod dialog;
mod dropdown_menu;
mod menu;
mod popup;
mod select;
mod tooltip;
mod window;

pub use checkbox::Checkbox;
pub use context_menu::ContextMenu;
pub use dialog::Dialog;
pub use dropdown_menu::DropdownMenu;
pub use menu::{MenuItem, MenuSep};
pub use popup::{Backdrop, Point, Popup};
pub use select::{Select, SelectOption};
pub use tooltip::Tooltip;
pub use window::{WinGeom, Window};
