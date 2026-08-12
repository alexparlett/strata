//! The design system's **layout scale**, and the fixed sizes the app shares across surfaces.
//!
//! Deliberately **not** theme fields. The theme layer is colour + typography (see
//! `crate::theme::components`): a spacing step does not vary by theme, and a theme author who
//! could retune one could reflow every surface in the app from a JSON file. So the scale is
//! constants — one home, read by name, and a change here reflows consistently because nothing
//! restates the number.
//!
//! # The scale
//!
//! Straight from the design handoff (`Design.dc.html` §03): a nine-step spacing scale on a 4px
//! base grid and a five-step radius scale. Every padding, gap and corner radius in the app snaps
//! to one of them. The canvases hold to it too — across every `.dc.html` the only literal px
//! spacings left are a handful of hairlines.
//!
//! The names are the design's own (`--sp-*`, `--r-*`) rather than semantic ones (`GAP_SMALL`,
//! `RADIUS_CARD`): a step is a step, and a surface that wanted "the small gap" would be choosing
//! by a name this module invented instead of by the scale the canvas was drawn on. Where a
//! surface's use of a step is worth naming, it names it locally — `const CELL_INSET: f32 = SP_4;`
//! — which is the application, not a second scale.
//!
//! # The exceptions
//!
//! Three, all stated at their sites:
//!
//! - **Pills and circles.** The design keeps these at `999px` / `50%`; in Freya a corner radius is
//!   px, so it is half the extent — [`pill`], not a scale step.
//! - **Hairlines.** A 1px rule, edge or list separator is a stroke, not a gap.
//! - **Miniatures.** The Settings theme preview draws a scaled-down mock of the whole app
//!   (`apps::settings::views::theme`); its 4px and 5px runs are a drawing of the layout, not the
//!   layout.
//!
//! # The fixed sizes
//!
//! Below the scale: the component and chrome sizes more than one surface has to agree on — a
//! toolbar button, a title bar, a panel header. A constant scoped to one surface is a constant
//! every other consumer has to reach *into* that surface for, which is how one pane's number
//! quietly becomes the app's; four separate 26px title-bar buttons is what that looks like when
//! it has happened. A size only one surface has stays where it is used.
//!
//! P5-03's shared animation durations and easings land here too, beside [`PROGRESS_HOLD`].

use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// Spacing — `--sp-1…9`, a 4px base grid
// ---------------------------------------------------------------------------------------------

/// 2px — hairline gap, icon-to-text micro spacing.
pub const SP_1: f32 = 2.;
/// 4px — tight inset, chip padding.
pub const SP_2: f32 = 4.;
/// 8px — the default small gap, and a button's inset.
pub const SP_3: f32 = 8.;
/// 12px — control padding, the default medium gap.
pub const SP_4: f32 = 12.;
/// 16px — group gap.
pub const SP_5: f32 = 16.;
/// 24px — card and dialog padding.
pub const SP_6: f32 = 24.;
/// 32px — section spacing.
pub const SP_7: f32 = 32.;
/// 40px — large section.
pub const SP_8: f32 = 40.;
/// 48px — page rhythm.
pub const SP_9: f32 = 48.;

// ---------------------------------------------------------------------------------------------
// Radius — `--r-xs…4`
// ---------------------------------------------------------------------------------------------

/// 4px — chips, swatches, checkboxes.
pub const R_XS: f32 = 4.;
/// 6px — buttons, menu items.
pub const R_1: f32 = 6.;
/// 8px — cards, inputs, tiles.
pub const R_2: f32 = 8.;
/// 10px — panels, popovers.
pub const R_3: f32 = 10.;
/// 14px — dialogs, windows.
pub const R_4: f32 = 14.;

/// A **hairline** — a 1px rule, edge or list separator.
///
/// Off the spacing scale on purpose, and the design's canvases keep it literal too (`gap: 1px`):
/// a hairline is a stroke that happens to occupy a row, not the smallest gap. Snapping it to
/// [`SP_1`] would double every rule in the app.
pub const HAIRLINE: f32 = 1.;

/// The corner radius that makes a box of this extent a **pill or a circle** — the design's
/// `999px` / `50%`, which a px radius has to state as half the extent.
///
/// A function rather than a constant because the answer depends on the shape: a 15px badge and a
/// 4px progress track are both fully round and share no number. Written this way the site says
/// *circle* rather than `7.5`, which is the whole reason these are allowed off the scale.
pub const fn pill(extent: f32) -> f32 {
    extent / 2.
}

// ---------------------------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------------------------

/// A **toolbar icon button** — the 28x28 cluster size the design fixes for every icon-only
/// control in a pane header or a toolbar row.
pub const TOOL_SIZE: f32 = 28.;

/// A **panel header's** control — the smaller square a header row's own buttons take, so a
/// sidebar or drawer header stays shorter than a toolbar.
pub const HEADER_CONTROL: f32 = 24.;

/// A **list row's** hover action — the smallest of the three, because it sits inside a 30px row
/// and must not set that row's height.
pub const ROW_ACTION: f32 = 22.;

/// A **row's status glyph** — the catalog's and the connections pane's registration marker.
pub const STATUS_DOT: f32 = 12.;

/// A **status block's** glyph — the ⓘ / ✓ / ✕ mark a Configure or Connection window's status
/// line leads with.
pub const STATUS_GLYPH: f32 = 14.;

/// A **committing action button's** height — a dialog's Cancel / confirm pair (stamped by the
/// action strip) and the column inspector's scan card. Freya's `button_layout` hugs its label
/// (≈28px), which reads as squashed; with a dialog strip's [`SP_4`] above and below, this is
/// also what makes that strip the comps' 58px.
///
/// Deliberately **not** themeable: it is a design-system invariant, not a dress a theme author
/// gets to retune — a taller button would break the strip's 58px and every layout built on it.
pub const ACTION_HEIGHT: f32 = 34.;

// ---------------------------------------------------------------------------------------------
// Window chrome
// ---------------------------------------------------------------------------------------------

/// A **child window's title bar** — Settings, Configure, Export and the connection editor. The
/// launcher's is its own, shorter bar and says so where it is declared.
pub const TITLE_BAR_HEIGHT: f32 = 50.;

/// The **traffic lights' gutter**: how far in from the left edge a title bar may put content
/// before it collides with macOS's own window buttons.
///
/// A literal on purpose — it is the OS's geometry, not the design's, so it belongs to no scale
/// step and snapping it would put a control under the close button.
pub const TRAFFIC_LIGHT_GUTTER: f32 = 82.;

/// A **compact button** — shorter than [`ACTION_HEIGHT`], because it sits in chrome rather than
/// committing anything: every child window's title-bar tile (square at this size), the chat
/// pane's title trigger, Settings' inline Revert, and the keymap pane's reset.
pub const COMPACT_BUTTON: f32 = 26.;

// ---------------------------------------------------------------------------------------------
// Panels
// ---------------------------------------------------------------------------------------------

/// The **sidebar's** header row (its pane title, filter and controls).
pub const SIDEBAR_HEADER_HEIGHT: f32 = 48.;

/// The **right side's** header row — the inspector's and the chat pane's, which are one row
/// because they are one slot (`Layout::right` is inspector *or* chat).
///
/// Genuinely shorter than [`SIDEBAR_HEADER_HEIGHT`], not drift to merge: the sidebar's header
/// carries a filter box and these carry a title. Whether the canvas wants them equal is P5-05's
/// call, and it is one edit here when it makes it.
pub const RIGHT_PANE_HEADER_HEIGHT: f32 = 40.;

/// The **drawer's** header row — the shortest of the three, a tab strip rather than a title.
pub const DRAWER_HEADER_HEIGHT: f32 = 36.;

/// The narrowest a **pane body** lays its content out at, however narrow its panel gets.
///
/// A panel has no usability minimum (P5-06 — see `apps::project::views::shell`), so a body given
/// its panel's width verbatim eventually gets less room than a single word. At that point wrapping
/// text degrades into **one character per line** — a column of letters down the panel — which is
/// the one outcome worth spending a floor to prevent.
///
/// So it is sized to **the longest word plus the widest pane inset**, and no more. Wrapping a
/// sentence one word per line is fine, and is what RustRover does at the same widths; the floor
/// exists only to stop the step past that. Sizing it to a body's *content* width instead (the
/// inspector's 230px scan card was the tempting number) clips readable prose at panel widths a
/// user would call perfectly reasonable, which is a worse failure than the one being fixed.
pub const PANE_BODY_MIN_W: f32 = 132.;

// ---------------------------------------------------------------------------------------------
// Menus and tables
// ---------------------------------------------------------------------------------------------

/// A **row context menu's** width — the design canvas's `min-width: 210px`, which is what keeps
/// "Refresh table" and "Open in new tab" on one line. The catalog's row menu and the connections
/// pane's are the same menu shape over different rows.
pub const CONTEXT_MENU_WIDTH: f32 = 210.;

/// A **menu item's** leading glyph (canvas: 15px icon, [`SP_4`] to the label).
pub const MENU_ICON: f32 = 15.;

/// The horizontal chrome around a **menu row**: the `menu_container` card's padding
/// ([`SP_2`] each side) plus `MenuButton`'s own ([`SP_4`] each side).
///
/// A `Menu` takes only a **min** width and its container hugs its children, so a long row would
/// otherwise stretch the card to the whole window. Capping the row at `width - MENU_ROW_CHROME`
/// is what fixes the card at the width asked for *and* gives the label a bounded box to
/// ellipsize in — the recipe the tab menus, the project switcher and the row menus all use.
pub const MENU_ROW_CHROME: f32 = 32.;

/// A **settings-style option table's** header row — the engine and keymap tables, and the
/// connection editor's options table.
pub const TABLE_HEAD_HEIGHT: f32 = 32.;

/// The same table's **body row**. One step taller than its header, because a body row holds a
/// control and a header holds a label.
pub const TABLE_ROW_HEIGHT: f32 = 34.;

/// What an **option table** stands at while it is empty — tall enough that its empty line is
/// centred in a real box rather than in a collapsed one.
pub const EMPTY_TABLE_HEIGHT: f32 = 88.;

/// The **stripe** an invalid table row is marked with down its leading edge.
pub const ERROR_STRIPE: f32 = 2.;

/// A **settings numeric field** (canvas `width: 130px`) — the boxes System, Data display and
/// AI ▸ Chat put a count or a duration in. One size, so they line up down a pane.
pub const SETTINGS_FIELD_WIDTH: f32 = 130.;

// ---------------------------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------------------------

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
pub const PROGRESS_HOLD: Duration = Duration::from_millis(400);
