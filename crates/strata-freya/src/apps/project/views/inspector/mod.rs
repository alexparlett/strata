//! The right **column inspector** (P3-08): what the catalog's selected column actually is, and
//! the facts its source actually reported.
//!
//! ## Only real facts
//!
//! Every number in this panel was *read*, never derived from what happens to be on screen. The
//! Dioxus inspector used to compute Rows / Nulls / Distinct / Min / Max from the current page of
//! the current tab's query and present them as column facts (DEV_TASKS U9); they described one
//! page of one query. What replaced them is the two-tier model this panel renders: **free
//! metadata** — footer-derived, so it varies by source format and is often absent entirely — and,
//! with P3-09, what a full **scan** computed. A fact never appears in both, and an absent fact is
//! an absent row rather than a blank one. The derivation lives in [`model`], where it can be
//! tested without a window; this module is the frame and [`column`] the body.
//!
//! ## What is deliberately not here
//!
//! The STATISTICS zone's scan half belongs to **P3-09**: the age / view-as-query / re-scan
//! controls, the distribution bars, the running state, and the `Profile table` action itself.
//! Its call-to-action card *is* rendered, in full dress and with no press handler, so the zone
//! keeps the shape the canvas specifies and the wiring has somewhere to land — the same call the
//! row menus' parked `Profile table` item will make (`sidebar/catalog/menu.rs`).

mod column;
mod model;
#[cfg(test)]
mod tests;

use freya::components::{define_theme, get_theme, ScrollView};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::CatalogKind;

use self::column::ColumnPanel;
use self::model::{inspect, Inspected};
use crate::apps::project::state::{
    use_catalog_selection, Chan, ProjChan, ProjectState, SessionState,
};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Eyebrow, Prose};

define_theme!(
    %[component]
    pub Inspector {
        %[fields]
        /// The panel's own surface.
        background: Color,
        /// Section eyebrows (COLUMN INSPECTOR · NESTED FIELDS · STATISTICS) and the facts
        /// box's keys.
        label_color: Color,
        /// The inspected column's name.
        name_color: Color,
        /// A fact's value.
        value_color: Color,
        /// A nested field's name — one step back from the column's own.
        field_color: Color,
        /// "from <owner>".
        meta_color: Color,
        /// Explanatory copy: the derived-column note, the profile card's line, and the
        /// completeness label.
        note_color: Color,
        /// Box borders, and the completeness track.
        border_fill: Color,
        /// The hairline between rows, and the rule above the statistics zone.
        divider_fill: Color,
        /// The facts box's rows and the profile card.
        box_background: Color,
        /// The nested-fields box's rows — a step below the facts box, like the canvas.
        field_background: Color,
        /// The completeness percentage.
        emphasis_color: Color,
        /// The share of rows that carry a value.
        fill_color: Color,
        /// The share that is null.
        null_color: Color,
        /// The scan card's icon tile — the glyph, and (at a fixed alpha) the tint behind it.
        /// Authored as a reference to the sheet's accent, so it tracks the theme without this
        /// component reaching into the palette for it.
        tile_color: Color,
        /// The source-format badge, per format. A closed set: the badge is coloured, and a
        /// theme can only name the formats it knows (anything else wears `meta_color`).
        format_parquet_color: Color,
        format_csv_color: Color,
        format_json_color: Color,
        format_arrow_color: Color,
        format_view_color: Color,
    }
);

/// The panel's outer padding, and the gap between its stacked sections (canvas `--sp-4`).
const PANEL_PAD: f32 = 12.;
/// The header row's height, matching the sidebar's and the workbench toolbar's.
const HEADER_HEIGHT: f32 = 40.;

#[derive(PartialEq)]
pub struct Inspector {
    pub theme: Option<InspectorThemePartial>,
}

impl Inspector {
    pub fn new() -> Self {
        Self { theme: None }
    }
}

impl Component for Inspector {
    fn render(&self) -> impl IntoElement {
        let layout = use_radio::<SessionState, Chan>(Chan::Layout);
        let selection = use_catalog_selection();
        // **One source for every colour this panel paints.** The `inspector` theme owns them,
        // including the surface and the rule under the header — the sheet is reached for only
        // where a value is *semantic* (success / warning / error), and nothing here is. Reading
        // `colors.border` as well would have been the same value under a second name.
        let theme = get_theme!(&self.theme, InspectorThemePreference, "inspector");

        let selected = selection.read().clone();
        // The selection says which collection owns it, so the panel listens on **that** section's
        // channel: with a table column selected, a view registering never wakes it.
        //
        // Switching selection re-points the antenna, but does *not* drop the old subscription —
        // `Radio::subscribe_if_not` adds this reactive context to the new channel's listener set
        // and nothing removes it from the previous one (`freya-radio/src/hooks/use_radio.rs`).
        // So a panel that has inspected both kinds ends up woken by either. Harmless (the render
        // re-derives from whatever the selection now is), but don't read this as a guarantee.
        let channel = match selected.as_ref().map(|c| c.kind) {
            Some(CatalogKind::View) => ProjChan::Views,
            _ => ProjChan::Tables,
        };
        let project = use_radio::<ProjectState, ProjChan>(channel);

        // Resolve against the store and drop the read guard before any element is built.
        let inspected = selected.as_ref().map(|col| inspect(&project.read(), col));

        let body = match inspected {
            None => note("Select a column to inspect.", theme.note_color),
            Some(Inspected::Column(facts)) => ColumnPanel {
                facts: *facts,
                theme: theme.clone(),
            }
            .into_element(),
            // A row mid-re-scan has no verdict yet, so the panel says so rather than showing the
            // facts it had a moment ago as if they still stood.
            Some(Inspected::Loading) => note("Loading…", theme.note_color),
            Some(Inspected::Failed(reason)) => note(reason, theme.note_color),
            Some(Inspected::Gone(reason)) => note(reason, theme.note_color),
        };

        rect()
            .expanded()
            .background(theme.background)
            .vertical()
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::px(HEADER_HEIGHT))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::SpaceBetween)
                    .padding((0., PANEL_PAD))
                    .child(Eyebrow::new("COLUMN INSPECTOR").color(theme.label_color))
                    .child(
                        Button::new()
                            .flat()
                            .width(Size::px(24.))
                            .height(Size::px(24.))
                            .on_press(move |_| {
                                let mut layout = layout;
                                layout.write_channel(Chan::Layout).close_inspector();
                            })
                            .child(Icon::new(IconName::Close).size(13.)),
                    ),
            )
            .child(Divider::horizontal().color(theme.border_fill))
            // The header holds still; only the body scrolls, so the panel's own title and its
            // collapse stay reachable however long a struct's field list runs.
            .child(ScrollView::new().child(body))
    }
}

/// A plain line of copy where the panel has no column to describe — nothing selected, a row
/// still registering, a refused table, or a selection the catalog has moved out from under.
fn note(text: impl Into<String>, color: Color) -> Element {
    rect()
        .width(Size::fill())
        .padding(Gaps::new(24., PANEL_PAD, 24., PANEL_PAD))
        .child(Prose::new(text).color(color).wrap())
        .into_element()
}
