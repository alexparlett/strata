//! The right **column inspector** (P3-08): what the catalog's selected column actually is, and
//! the facts its source actually reported.
//!
//! ## Only real facts
//!
//! Every number in this panel was *read*, never derived from what happens to be on screen. The
//! Dioxus inspector used to compute Rows / Nulls / Distinct / Min / Max from the current page of
//! the current tab's query and present them as column facts (`DEV_TASKS` U9); they described one
//! page of one query. What replaced them is the two-tier model this panel renders: **free
//! metadata** — footer-derived, so it varies by source format and is often absent entirely — and,
//! with P3-09, what a full **scan** computed. A fact never appears in both, and an absent fact is
//! an absent row rather than a blank one. The derivation lives in [`model`], where it can be
//! tested without a window; this module is the frame and [`column`](mod@column) the body.
//!
//! ## Two owners, one panel
//!
//! A selected column belongs either to a **workspace** table or view — resolved against the
//! project store, which is where its registration answer lives — or to a **relation inside a
//! database connection's catalog** (DB-07), which has no store row at all: a database answers for
//! itself, so its columns are an introspection this panel subscribes to. Both arms produce the
//! same [`ColumnFacts`](model::ColumnFacts), so everything below the resolution — the title, the
//! nested-fields box, the STATISTICS zone and the scan — is written once.
//!
//! The remote arm's columns come from the same `use_remote_schemas` the tree reads, on the one
//! relation this panel is looking at — never dispatched at all while the selection is a workspace
//! column, which is every other moment. It is that hook and not the query entry, for the reason
//! stated there: the entry re-keys on every catalog epoch, and reading it directly would replace a
//! panel showing settled scan numbers with "Loading…" because an unrelated view was saved.
//!
//! ## What is deliberately not here
//!
//! The canvas's **distribution bars**: the profile carries no distribution data, and binning would
//! need a second full pass over data the confirm has just called expensive to read once
//! ([`column`](mod@column) has the full reasoning).

mod column;
mod model;
#[cfg(test)]
mod tests;

use freya::components::{define_theme, get_theme, ScrollView};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_model::{CatalogKind, ColOwner};

use self::column::ColumnPanel;
use self::model::{inspect, inspect_remote, Inspected};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::use_remote_schemas;
use crate::apps::project::state::{
    use_catalog, use_catalog_selection, use_remote_scans, Chan, ProjChan, ProjectState,
    SessionState,
};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{PANE_BODY_MIN_W, RIGHT_PANE_HEADER_HEIGHT};
use crate::components::metrics::{SP_3, SP_4, SP_6};
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
        /// The same slot on a column read through a **database connection**, where the badge names
        /// the connection rather than a format — Strata has no reader for it at all. It wears the
        /// tone the tree's provider badge wears, because it says the same thing in the same words.
        format_database_color: Color,
    }
);

/// The panel's outer padding, and the gap between its stacked sections (canvas `--sp-4`).
const PANEL_PAD: f32 = SP_4;

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
        let theme = get_theme!(&self.theme, InspectorThemePreference, "inspector");

        let selected = selection.read().clone();
        let channel = match selected.as_ref().map(|c| &c.owner) {
            Some(ColOwner::Entry {
                kind: CatalogKind::View,
                ..
            }) => ProjChan::Views,
            _ => ProjChan::Tables,
        };
        let project = use_radio::<ProjectState, ProjChan>(channel);
        let remote_scans = use_remote_scans();

        let engine = use_consume::<EngineCtx>();
        let epoch = use_catalog().read().epoch();
        let relation = match selected.as_ref().map(|c| &c.owner) {
            Some(ColOwner::Remote(relation)) => Some(relation.clone()),
            _ => None,
        };
        let described = use_remote_schemas(&engine, relation.iter().cloned().collect(), epoch);

        let inspected = selected.as_ref().map(|col| match &col.owner {
            ColOwner::Entry { kind, name } => {
                let scan = project.read().profile_scan(*kind, name);
                inspect(&project.read(), col, *kind, name, scan)
            }
            ColOwner::Remote(relation) => {
                let scan = remote_scans.read().get(relation).copied();
                inspect_remote(col, relation, described.get(relation), scan)
            }
        });

        let body = match inspected {
            None => note("Select a column to inspect.", theme.note_color),
            Some(Inspected::Column(facts)) => ColumnPanel {
                facts: *facts,
                theme: theme.clone(),
            }
            .into_element(),
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
                    .height(Size::px(RIGHT_PANE_HEADER_HEIGHT))
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
                    .padding((0., PANEL_PAD))
                    .child(
                        rect().width(Size::flex(1.)).child(
                            Eyebrow::new("COLUMN INSPECTOR")
                                .color(theme.label_color)
                                .text_overflow(TextOverflow::Ellipsis),
                        ),
                    )
                    .child(
                        Button::new()
                            .flat()
                            .width(Size::px(24.))
                            .height(Size::px(24.))
                            .on_press(move |_| {
                                let mut layout = layout;
                                layout.write_channel(Chan::Layout).close_right_pane();
                            })
                            .child(Icon::new(IconName::Close).size(13.)),
                    ),
            )
            .child(Divider::horizontal().color(theme.border_fill))
            .child(
                ScrollView::new().child(
                    rect()
                        .width(Size::fill())
                        .min_width(Size::px(PANE_BODY_MIN_W))
                        .child(body),
                ),
            )
    }
}

/// A plain line of copy where the panel has no column to describe — nothing selected, a row
/// still registering, a refused table, or a selection the catalog has moved out from under.
fn note(text: impl Into<String>, color: Color) -> Element {
    rect()
        .width(Size::fill())
        .padding(Gaps::new(SP_6, PANEL_PAD, SP_6, PANEL_PAD))
        .child(Prose::new(text).color(color).wrap())
        .into_element()
}
