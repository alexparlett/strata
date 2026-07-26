//! Strata's design-system components — reusable, theme-authorable widgets built to the
//! `design-handoff/` comps. Each owns its `define_theme!` theme (default registered in
//! `crate::theme`), so its colours follow the sheet and are overridable like every built-in.
//!
//! Sizes the design system fixes across components live here, not on any one of them: a constant
//! scoped to a component is a constant every other consumer has to reach *into* that component
//! for, which is how one surface's number quietly becomes the app's.

/// A **committing action button's** height — a dialog's Cancel / confirm pair (stamped by the
/// action strip) and the column inspector's scan card. Freya's `button_layout` hugs its label
/// (≈28px), which reads as squashed; with a dialog strip's `--sp-4` above and below, this is
/// also what makes that strip the comps' 58px.
///
/// Those two are the only consumers today. The workbench's empty-state CTA is the same kind of
/// button and still hugs — worth folding in when that surface is next touched.
///
/// Deliberately **not** themeable: it is a design-system invariant, not a dress a theme author
/// gets to retune — a taller button would break the strip's 58px and every layout built on it.
pub const ACTION_HEIGHT: f32 = 34.;

/// How long a wait must last before it is worth **telling the user about**.
///
/// Below this, announcing progress costs more than it buys: the spinner and the thing it replaced
/// both flash past, and the eye reads the flicker rather than the state. Past it, the wait is news
/// in its own right.
///
/// Shared, because two surfaces serve exactly the same hold and a number scoped to one of them is
/// a number the other has to reach *into* it for: the catalog row's registration spinner (a
/// metadata read, usually far inside this window — see `sidebar/catalog/entry.rs`) and the column
/// inspector's re-scan row (a profile the user asked for again, over numbers already on screen).
///
/// It is **not** a hold on work the user just started with nothing to show yet — a first profile
/// says so at once, or the press looks like it missed.
pub const PROGRESS_HOLD: std::time::Duration = std::time::Duration::from_millis(400);

pub mod avatar;
pub mod badge;
pub mod dialog;
pub mod divider;
pub mod dot;
pub mod icon;
pub mod run_button;
pub mod segmented_toggle;
pub mod sidebar_row;
pub mod toggle_button;
pub mod type_palette;
pub mod typography;
// NB: the bespoke `icon_button` is retired — icon buttons are now Freya's `Button` variants
// (`.flat()` / `.outline()`) wrapping an `Icon`. The old `icon_button.rs` is an orphan (unreferenced,
// not compiled) and can be deleted.
