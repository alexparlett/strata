//! The design system's **layout scale**, and the fixed sizes the app shares across surfaces.
//!
//! Constants, deliberately not theme fields: a spacing step does not vary by theme, and a theme
//! author who could retune one could reflow every surface from a JSON file. The names are the
//! design handoff's own (`--sp-*`, `--r-*`); a surface that wants to name a step names it locally
//! (`const CELL_INSET: f32 = SP_4;`). The three exceptions — [`pill`], [`HAIRLINE`], and the
//! Settings theme preview's miniature — say so at their sites.

use std::time::Duration;

use async_io::Timer;
use freya::prelude::*;

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
/// Off the spacing scale on purpose: a hairline is a stroke that happens to occupy a row, not the
/// smallest gap. Snapping it to [`SP_1`] would double every rule in the app.
pub const HAIRLINE: f32 = 1.;

/// The corner radius that makes a box of this extent a **pill or a circle** — the design's
/// `999px` / `50%`, which a px radius has to state as half the extent.
///
/// A function rather than a constant because the answer depends on the shape, and so the site says
/// *circle* rather than `7.5`.
pub const fn pill(extent: f32) -> f32 {
    extent / 2.
}

/// A **toolbar icon button** — the 28x28 cluster size for every icon-only control in a pane header
/// or toolbar row.
pub const TOOL_SIZE: f32 = 28.;

/// A **panel header's** control, so a sidebar or drawer header stays shorter than a toolbar.
pub const HEADER_CONTROL: f32 = 24.;

/// A **list row's** hover action — smallest of the three, because it must not set its 30px row's
/// height.
pub const ROW_ACTION: f32 = 22.;

/// A **row's status glyph** — the catalog's and the connections pane's registration marker.
pub const STATUS_DOT: f32 = 12.;

/// A **status block's** glyph — the ⓘ / ✓ / ✕ mark a Configure or Connection status line leads
/// with.
pub const STATUS_GLYPH: f32 = 14.;

/// A **committing action button's** height — a dialog's Cancel / confirm pair and the column
/// inspector's scan card.
///
/// Not themeable: a taller button would break the action strip's 58px and every layout built on it.
pub const ACTION_HEIGHT: f32 = 34.;

/// A **child window's title bar** — Settings, Configure, Export and the connection editor. The
/// launcher's is its own, shorter bar and says so where it is declared.
pub const TITLE_BAR_HEIGHT: f32 = 50.;

/// The **traffic lights' gutter**: how far in from the left edge a title bar may put content before
/// it collides with macOS's own window buttons. The OS's geometry, so it belongs to no scale step.
pub const TRAFFIC_LIGHT_GUTTER: f32 = 82.;

/// A **compact button** — shorter than [`ACTION_HEIGHT`] because it sits in chrome rather than
/// committing anything: title-bar tiles, the chat pane's title trigger, Settings' inline Revert.
pub const COMPACT_BUTTON: f32 = 26.;

/// The **sidebar's** header row (its pane title, filter and controls).
pub const SIDEBAR_HEADER_HEIGHT: f32 = 48.;

/// The **right side's** header row — the inspector's and the chat pane's, which are one row because
/// they are one slot. Shorter than [`SIDEBAR_HEADER_HEIGHT`] because that one carries a filter box.
pub const RIGHT_PANE_HEADER_HEIGHT: f32 = 40.;

/// The **drawer's** header row — a tab strip rather than a title.
pub const DRAWER_HEADER_HEIGHT: f32 = 36.;

/// The narrowest a **pane body** lays its content out at, however narrow its panel gets.
///
/// A panel has no usability minimum (P5-06), so this exists only to stop wrapping degrading into
/// one character per line: the longest word plus the widest pane inset, and no more. Sizing it to a
/// body's *content* width instead clips readable prose at reasonable panel widths.
pub const PANE_BODY_MIN_W: f32 = 132.;

/// A **row context menu's** width, which is what keeps 'Refresh table' and 'Open in new tab' on one
/// line.
pub const CONTEXT_MENU_WIDTH: f32 = 210.;

/// A **menu item's** leading glyph (15px icon, [`SP_4`] to the label).
pub const MENU_ICON: f32 = 15.;

/// The horizontal chrome around a **menu row**: the `menu_container` card's padding plus
/// `MenuButton`'s own.
///
/// A `Menu` takes only a **min** width and its container hugs its children, so capping the row at
/// `width - MENU_ROW_CHROME` is what fixes the card at the width asked for and gives the label a
/// bounded box to ellipsize in.
pub const MENU_ROW_CHROME: f32 = 32.;

/// A **settings-style option table's** header row — the engine and keymap tables, and the
/// connection editor's options table.
pub const TABLE_HEAD_HEIGHT: f32 = 32.;

/// The same table's **body row**, one step taller because a body row holds a control.
pub const TABLE_ROW_HEIGHT: f32 = 34.;

/// What an **option table** stands at while it is empty, so its empty line is centred in a real box.
pub const EMPTY_TABLE_HEIGHT: f32 = 88.;

/// The **stripe** an invalid table row is marked with down its leading edge.
pub const ERROR_STRIPE: f32 = 2.;

/// A **settings numeric field** — the boxes System, Data display and AI ▸ Chat put a count or a
/// duration in. One size, so they line up down a pane.
pub const SETTINGS_FIELD_WIDTH: f32 = 130.;

/// How long a wait must last before it is worth **telling the user about** — below this the spinner
/// and the thing it replaced both flash past.
///
/// Shared by the catalog row's registration spinner and the column inspector's re-scan row. Not a
/// hold on work the user just started with nothing to show yet: a first profile says so at once, or
/// the press looks like it missed.
pub const PROGRESS_HOLD: Duration = Duration::from_millis(400);

/// Has `active` lasted long enough to be worth reporting? The [`PROGRESS_HOLD`] rule as a hook,
/// so the surfaces that observe it cannot drift.
///
/// `false` until the wait outlasts the hold, and back to `false` the instant it ends — most work
/// lands well inside, so the ordinary case is a surface that simply appears rather than one that
/// flickers a spinner on the way in. The timer is armed per transition and **cancelled** when the
/// wait ends, or a settled row would spin one hold later.
///
/// Here rather than beside either caller because there are two: the data-sources tree's row
/// status slot and the inspector's scan zone had grown byte-identical copies.
pub fn use_progress_hold(active: bool) -> bool {
    let waited = use_state(|| false);
    let pending = use_state(|| None::<TaskHandle>);
    use_side_effect_with_deps(&active, move |active| {
        let mut waited = waited;
        let mut pending = pending;
        if let Some(task) = pending.write().take() {
            task.cancel();
        }
        waited.set_if_modified(false);
        if *active {
            pending.set(Some(spawn(async move {
                Timer::after(PROGRESS_HOLD).await;
                waited.set_if_modified(true);
            })));
        }
    });
    active && waited()
}
