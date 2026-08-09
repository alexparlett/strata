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
//!
//! Each row is built from its [`Anchor`] (P4-09), which is where its title and subtext live — so
//! the search index and the pane cannot name the same setting differently, and a hit can flash the
//! row it points at.

use freya::prelude::*;
use strata_core::config::{COL_WIDTH_MAX, COL_WIDTH_MIN};

use crate::apps::settings::views::Pane;
use crate::apps::settings::{Anchor, SettingsCtx};
use crate::components::form::{Form, NumberField};
use crate::components::segmented_toggle::{SegmentedToggle, ToggleSegment};

/// The canvas's numeric field (`width: 130px`).
const FIELD_WIDTH: f32 = 130.;

/// The row limit is uncapped by design — `0` already means "no limit", so a huge number is
/// equivalent to it and harmless. The field's own type is the only bound there is.
const NO_ROW_CAP: u32 = u32::MAX;

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

        let body = Form::new()
            .preferences()
            .child(Anchor::Density.row().child(Density { compact }))
            .child(
                Anchor::Zebra
                    .row()
                    .trailing()
                    .on_press(move |_: Event<PressEventData>| ctx.edit(|s| s.zebra = !s.zebra))
                    .child(
                        Switch::new()
                            .toggled(zebra)
                            .on_toggle(move |()| ctx.edit(|s| s.zebra = !s.zebra)),
                    ),
            )
            .child(
                Anchor::ColumnWidth.row().child(
                    NumberField::new(col_width as u32, COL_WIDTH_MIN as u32, COL_WIDTH_MAX as u32)
                        .width(Size::px(FIELD_WIDTH))
                        .unit("px")
                        .on_change(move |px: u32| {
                            ctx.edit(|s| s.default_col_width = f64::from(px));
                        }),
                ),
            )
            .child(
                // Saturating, not `as`: a hand-edited config holding more than a u32 should show
                // the biggest number the field can offer, not wrap round to a small one.
                Anchor::RowLimit.row().child(
                    NumberField::new(row_limit.try_into().unwrap_or(NO_ROW_CAP), 0, NO_ROW_CAP)
                        .width(Size::px(FIELD_WIDTH))
                        .unit("rows")
                        .on_change(move |rows: u32| ctx.edit(|s| s.row_limit = rows as usize)),
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
        let set = move |compact: bool| ctx.edit(|s| s.density_compact = compact);

        // The **form** layout, not the compact toolbar one: inset rounded segments on the
        // recessed surface, at the height the canvas draws a settings-form control. The
        // toolbar pill is a different control that happens to share a component.
        //
        // The pill hugs its segments, so it needs a hug-content parent of its own — dropped
        // straight into the setting's fill-width column it would stretch across the pane.
        rect().horizontal().child(
            SegmentedToggle::new()
                .form()
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
