//! **Settings ▸ Appearance & behaviour ▸ Data display** (P4-05, design `Settings.dc.html`) —
//! the four settings that shape the results grid: row density, zebra striping, the starting
//! column width, and the `LIMIT` generated queries carry.
//!
//! Every control writes [`SettingsCtx::draft`] and stops there; the footer's Apply is what
//! commits, and each setting already has its consumer on the other side of that commit — the
//! grid reads density / zebra / width straight off the config store (`DataGrid::render`), and
//! the catalog's View-table action reads the row limit. So there is no wiring here beyond the
//! draft: the pane is the control these settings were built without.
//!
//! The column-width bounds are `strata_core::config`'s, which are the same numbers the grid
//! clamps to — a field offering a width the grid then silently corrects would be a field that
//! lies about what it sets.

use freya::prelude::*;
use strata_core::config::{COL_WIDTH_MAX, COL_WIDTH_MIN};

use crate::apps::settings::views::field::{edit_draft, NumberField, Setting, SettingList};
use crate::apps::settings::views::Pane;
use crate::apps::settings::SettingsCtx;
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};

#[derive(PartialEq)]
pub struct DataDisplayPane;

impl Component for DataDisplayPane {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        // Read in a block: the guard has to be gone before anything below takes a write one on
        // the same `State`.
        let (compact, zebra, col_width, row_limit) = {
            let draft = ctx.draft.read();
            (
                draft.density_compact,
                draft.zebra,
                draft.default_col_width,
                draft.row_limit,
            )
        };

        let body = SettingList::new()
            .child(
                Setting::stacked("Row density", Density { compact })
                    .hint("Controls row height in the results grid and catalog."),
            )
            .child(
                Setting::switch("Alternating row colors", zebra, move |_| {
                    edit_draft(ctx, |s| s.zebra = !s.zebra)
                })
                .hint(
                    "Shades every other row in the results grid for easier scanning.",
                ),
            )
            .child(
                Setting::stacked(
                    "Default column width",
                    NumberField::new(col_width as i64, "px", move |px: i64| {
                        edit_draft(ctx, |s| s.default_col_width = px as f64)
                    })
                    .min(COL_WIDTH_MIN as i64)
                    .max(COL_WIDTH_MAX as i64),
                )
                .hint(
                    "Starting width for result-grid columns before you resize them. \
                     Drag a column's edge to override it for that column, or double-click \
                     the edge to auto-fit.",
                ),
            )
            .child(
                Setting::stacked(
                    "Default row limit",
                    NumberField::new(row_limit as i64, "rows", move |rows: i64| {
                        edit_draft(ctx, |s| s.row_limit = rows as usize)
                    }),
                )
                .hint(
                    "New queries are generated with this LIMIT so a stray SELECT * cannot \
                     pull a whole file into memory. Set to 0 for no limit.",
                ),
            );

        Pane::new(body)
    }
}

/// Row density: the two-segment Comfortable/Compact pill over `Settings::density_compact`.
///
/// A segmented toggle rather than a switch because neither density is the "off" one — the
/// setting names a choice between two dresses, not a feature you turn on.
#[derive(PartialEq)]
struct Density {
    compact: bool,
}

impl Component for Density {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let set = move |compact: bool| edit_draft(ctx, |s| s.density_compact = compact);

        // The pill hugs its segments, so it needs a hug-content parent of its own — a bare
        // `SegmentedToggle` in the setting's fill-width column would stretch across the pane.
        rect().horizontal().child(
            SegmentedToggle::new()
                .child(
                    ToggleSegment::text("Comfortable")
                        .selected(!self.compact)
                        .on_press(move |_| set(false)),
                )
                .child(
                    ToggleSegment::text("Compact")
                        .selected(self.compact)
                        .on_press(move |_| set(true)),
                ),
        )
    }
}
