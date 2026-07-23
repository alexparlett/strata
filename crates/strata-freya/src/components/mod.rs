//! Strata's design-system components — reusable, theme-authorable widgets built to the
//! `design-handoff/` comps. Each owns its `define_theme!` theme (default registered in
//! `crate::theme`), so its colours follow the sheet and are overridable like every built-in.

pub mod divider;
pub mod icon;
pub mod run_button;
pub mod segmented_toggle;
pub mod toggle_button;
pub mod dot;
pub mod typography;
// NB: the bespoke `icon_button` is retired — icon buttons are now Freya's `Button` variants
// (`.flat()` / `.outline()`) wrapping an `Icon`. The old `icon_button.rs` is an orphan (unreferenced,
// not compiled) and can be deleted.
