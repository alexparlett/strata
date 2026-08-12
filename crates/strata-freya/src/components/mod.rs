//! Strata's design-system components — reusable, theme-authorable widgets built to the
//! `design-handoff/` comps. Each owns its `define_theme!` theme (default registered in
//! `crate::theme`), so its colours follow the role vocabulary via the mapping table like
//! every built-in.
//!
//! Sizes the design system fixes across components — and the spacing and radius scale every
//! surface snaps to — live in [`metrics`], not on any one component: a constant scoped to a
//! component is a constant every other consumer has to reach *into* that component for, which is
//! how one surface's number quietly becomes the app's.

pub mod avatar;
pub mod badge;
pub mod dialog;
pub mod divider;
pub mod dot;
pub mod form;
pub mod icon;
pub mod keycap;
pub mod metrics;
pub mod run_button;
pub mod segmented_toggle;
pub mod sidebar_row;
pub mod toggle_button;
pub mod tones;
pub mod tool_button;
pub mod toolbar;
pub mod type_palette;
pub mod typography;
pub mod window;
// NB: the bespoke `icon_button` is retired — icon buttons are now Freya's `Button` variants
// (`.flat()` / `.outline()`) wrapping an `Icon`. The old `icon_button.rs` is an orphan (unreferenced,
// not compiled) and can be deleted.
