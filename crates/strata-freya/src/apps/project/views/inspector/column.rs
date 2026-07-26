//! The inspector's body once a column has resolved: its title, the note a derived column
//! carries, the shape of a nested type, and the STATISTICS zone — the facts box, the
//! completeness bar, and the scan.
//!
//! Built to the `Strata.dc.html` inspector canvas. The zone has **four states**, all through one
//! frame ([`zone`]) so the box can't shift under the user as a scan settles: the offer of a scan
//! ([`scan_card`]), the scan running ([`running_row`]), what it found (its facts folded into the
//! one list, under the canvas's age / view-as-query / ↻ controls), and a failure with the retry
//! beside it.
//!
//! **Deliberately not built: the canvas's distribution bars.** The profile carries no
//! distribution data — the scan computes distinct / min / max / mean / median (`core::profile`),
//! and bins would need boundaries, which need min/max first, i.e. a *second* full pass over data
//! we have just told the user is expensive to read once. The canvas's bars were prototype seed
//! data; a bar drawn from anything else here would be the fabrication this panel exists to
//! avoid. Recorded in the P3-09 task file and DEV_TASKS D4.

use freya::components::{use_theme, CircularLoader};
use freya::prelude::*;
use freya::query::QueryStateData;
use freya::radio::{use_radio_station, RadioStation};
use strata_core::engine::profile::CatalogProfile;
use strata_model::{CatalogKind, Origin};

use super::model::{
    completeness, fact_rows, nested_fields, scan_age, scan_footnote, with_scan, ColumnFacts,
    NestedField, SourceFormat,
};
use super::{InspectorTheme, PANEL_PAD};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{use_profile, ScanId};
use crate::apps::project::state::{Chan, SessionState};
use crate::apps::project::views::{use_profile_actions, ProfileActions, ProfileTarget};
use crate::components::badge::Badge;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::type_palette::{kind_color, type_palette, TypePaletteTheme};
use crate::components::typography::{Body, Control, Eyebrow, Meta, MonoValue, Path, Prose};
use crate::components::ACTION_HEIGHT;

/// Corner radius of the facts box and the profile card (canvas `--r-3`); the smaller boxes use
/// `--r-2`, and the badges `--r-xs`.
const BOX_RADIUS: f32 = 10.;
const PANEL_RADIUS: f32 = 8.;
const BADGE_RADIUS: f32 = 4.;
/// A nested field row's height, and the indent one nesting level adds.
const FIELD_HEIGHT: f32 = 27.;
const FIELD_INDENT: f32 = 13.;
/// The completeness track.
const TRACK_HEIGHT: f32 = 8.;
/// The profile card's icon tile, and the alpha its accent tint carries (canvas: 13%).
const TILE_SIZE: f32 = 36.;
const TILE_TINT: u8 = 33;
/// How wide the profile card's copy may run before it wraps (canvas `max-width: 230px`).
const CARD_COPY_WIDTH: f32 = 230.;

/// A 1px bottom-edge-only rule — the hairline *between* two rows of a box.
fn row_rule() -> BorderWidth {
    BorderWidth {
        top: 0.,
        right: 0.,
        bottom: 1.,
        left: 0.,
    }
}

/// A 1px top-edge-only rule — the STATISTICS zone's boundary.
fn zone_rule() -> BorderWidth {
    BorderWidth {
        top: 1.,
        right: 0.,
        bottom: 0.,
        left: 0.,
    }
}

#[derive(PartialEq)]
pub struct ColumnPanel {
    pub facts: ColumnFacts,
    pub theme: InspectorTheme,
}

impl Component for ColumnPanel {
    fn render(&self) -> impl IntoElement {
        // The palette is resolved **here**, once, and passed down: it is a theme read, which is
        // a hook, and two of the three sections below are conditional — reading it inside
        // `nested_box` would make the hook count depend on whether the selected column happens
        // to be nested. Every other colour comes off `self.theme`, which needs no hook at all.
        let palette = type_palette();
        let swatch = kind_color(self.facts.kind, &palette);
        let fields = nested_fields(&self.facts.children);

        rect()
            .width(Size::fill())
            .vertical()
            .child(self.title(swatch))
            // A view's column is defined by a query, not by a file — so the panel says why the
            // facts box below it has only a type in it, rather than leaving the emptiness to
            // read as a bug.
            .maybe_child(self.facts.derived.then(|| self.derived_note()))
            .maybe_child((!fields.is_empty()).then(|| self.nested_box(fields, &palette)))
            // The STATISTICS zone is its own component because its scanned half **subscribes**:
            // a hook can't be conditional, and whether there is a scan to watch is exactly what
            // varies here (see `Statistics`).
            .child(Statistics {
                facts: self.facts.clone(),
                theme: self.theme.clone(),
            })
    }
}

impl ColumnPanel {
    /// The column's identity: its swatch and name, then its dtype, where it came from, and
    /// which of the two owns it.
    fn title(&self, swatch: Color) -> Element {
        let t = &self.theme;
        rect()
            .width(Size::fill())
            .vertical()
            .padding(Gaps::new(16., PANEL_PAD, 16., PANEL_PAD))
            .spacing(8.)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .child(Dot::new(swatch).size(9.).square())
                    .child(
                        MonoValue::new(self.facts.name.clone())
                            .color(t.name_color)
                            .width(Size::flex(1.))
                            .text_overflow(TextOverflow::Ellipsis),
                    ),
            )
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    // The three runs wrap rather than truncate: a long dtype (`Timestamp`,
                    // `Decimal`) beside a long owner name would otherwise push "from …" out of
                    // a narrow panel.
                    .content(Content::wrap_spacing(8.))
                    .spacing(8.)
                    .child(Badge::value(self.facts.dtype.clone(), swatch).radius(BADGE_RADIUS))
                    .child(
                        Badge::tag(self.facts.format.label(), self.format_color())
                            .radius(BADGE_RADIUS)
                            // The format badge hugs like a value run, not like a tag: it sits
                            // beside the dtype and the two must read as one pair.
                            .padding(Gaps::new(2., 8., 2., 8.)),
                    )
                    .child(Path::new(format!("from {}", self.facts.owner)).color(t.meta_color)),
            )
            .into_element()
    }

    /// The badge tone for this column's source format. An unknown format keeps the recessive
    /// tone rather than borrowing a colour that means something else.
    fn format_color(&self) -> Color {
        let t = &self.theme;
        match self.facts.format {
            SourceFormat::Parquet => t.format_parquet_color,
            SourceFormat::Csv => t.format_csv_color,
            SourceFormat::Json => t.format_json_color,
            SourceFormat::Arrow => t.format_arrow_color,
            SourceFormat::View => t.format_view_color,
            SourceFormat::Other(_) => t.meta_color,
        }
    }

    fn derived_note(&self) -> Element {
        let t = &self.theme;
        rect()
            .width(Size::fill())
            .margin(Gaps::new(0., PANEL_PAD, 0., PANEL_PAD))
            .padding(PANEL_PAD)
            .corner_radius(PANEL_RADIUS)
            .background(t.box_background)
            .border(Border::new().width(1.).fill(t.border_fill))
            .child(
                Path::new(
                    "Derived column, defined by the view's query. There are no files under it, \
                     so the source reports no statistics.",
                )
                .color(t.note_color)
                .wrap(),
            )
            .into_element()
    }

    /// NESTED FIELDS — the shape of a struct / list / map column, at every depth.
    fn nested_box(&self, fields: Vec<NestedField>, palette: &TypePaletteTheme) -> Element {
        let t = &self.theme;
        let last = fields.len().saturating_sub(1);
        rect()
            .width(Size::fill())
            .vertical()
            .padding(Gaps::new(16., PANEL_PAD, 8., PANEL_PAD))
            .spacing(8.)
            .child(Eyebrow::new("NESTED FIELDS").color(t.label_color))
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .corner_radius(PANEL_RADIUS)
                    .overflow(Overflow::Clip)
                    .border(Border::new().width(1.).fill(t.border_fill))
                    .children(fields.into_iter().enumerate().map(|(i, f)| {
                        let hue = kind_color(f.kind, palette);
                        rect()
                            .width(Size::fill())
                            .height(Size::px(FIELD_HEIGHT))
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .spacing(8.)
                            .padding((0., PANEL_PAD))
                            .background(t.field_background)
                            .child(rect().width(Size::px(f.depth as f32 * FIELD_INDENT)))
                            .child(Dot::new(hue).size(6.).square())
                            .child(
                                MonoValue::new(f.name)
                                    .color(t.field_color)
                                    .width(Size::flex(1.))
                                    .text_overflow(TextOverflow::Ellipsis),
                            )
                            .child(Meta::new(f.dtype).color(hue))
                            // The rule sits *between* rows: on the last one it would double up
                            // with the box's own bottom edge.
                            .maybe(i < last, |el| {
                                el.border(Border::new().width(row_rule()).fill(t.divider_fill))
                            })
                            .into()
                    })),
            )
            .into_element()
    }
}

/// The completeness bar — present only when a **real** null count exists (see [`completeness`]),
/// footer-read or scan-counted. It is never computed off the result page, which is what it used
/// to be.
fn completeness_bar(facts: &ColumnFacts, t: &InspectorTheme) -> Option<Element> {
    let fill = completeness(facts)?;
    let filled = fill.filled as f32;
    let nulls = 1. - filled;

    let track = rect()
        .width(Size::fill())
        .height(Size::px(TRACK_HEIGHT))
        .corner_radius(TRACK_HEIGHT / 2.)
        .overflow(Overflow::Clip)
        .background(t.border_fill)
        .horizontal()
        .content(Content::Flex)
        // Flex weights rather than percentage widths, so the two shares divide the track
        // exactly however the panel is resized. A zero share contributes no segment at all.
        .maybe(filled > 0., |el| {
            el.child(
                rect()
                    .width(Size::flex(filled))
                    .height(Size::fill())
                    .background(t.fill_color),
            )
        })
        .maybe(nulls > 0., |el| {
            el.child(
                rect()
                    .width(Size::flex(nulls))
                    .height(Size::fill())
                    .background(t.null_color),
            )
        });

    Some(
        rect()
            .width(Size::fill())
            .vertical()
            .spacing(8.)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::SpaceBetween)
                    .child(Meta::new("Completeness").color(t.note_color))
                    .child(Meta::new(fill.label()).color(t.emphasis_color)),
            )
            // The bar carries no numbers, so the numbers are its tooltip: how many rows are
            // null, out of how many, and which side of the split is which.
            .child(
                TooltipContainer::new(Tooltip::new(fill.detail()))
                    .position(AttachedPosition::Bottom)
                    .width(Size::fill())
                    .child(track),
            )
            .into_element(),
    )
}

/// The **STATISTICS zone** — every fact known about the column, the completeness bar, and one of
/// three tails: the offer of a scan, the scan running, or what the scan found.
///
/// Its own component because the scanned half **subscribes** to a query, and a hook cannot be
/// conditional: whether there is a scan to watch is precisely what varies. So this half renders
/// the un-scanned zone, and [`ScannedStatistics`] — mounted only when the entry carries a
/// request, keyed on it — renders the rest.
#[derive(PartialEq)]
struct Statistics {
    facts: ColumnFacts,
    theme: InspectorTheme,
}

impl Component for Statistics {
    fn render(&self) -> impl IntoElement {
        // Gathered here rather than inside `scan_card`: the card is a plain function, and these
        // are hooks — they have to run in a component's scope. What the card's handler captures
        // is `Copy` handles taken at render time.
        let actions = use_profile_actions();
        match self.facts.scan {
            None => zone(
                &self.facts,
                &self.theme,
                None,
                Some(scan_card(&self.facts, &self.theme, actions)),
            ),
            // Keyed on the request, so a ↻ re-scan remounts on the new key rather than showing
            // the previous scan's numbers while the next one runs.
            Some(scan) => ScannedStatistics {
                facts: self.facts.clone(),
                theme: self.theme.clone(),
                scan,
                key: DiffKey::None,
            }
            .key(scan)
            .into_element(),
        }
    }
}

/// The zone once a scan has been asked for: it subscribes to that scan and renders the state it
/// is in.
///
/// **freya-query is the cache** (port plan §4): the numbers live in the entry keyed by the
/// request, the dedup is that key's identity — two subscribers (this and the catalog row's
/// spinner) attach to one execution — and the running state is `query.read().state()`. Nothing
/// here is stored, so nothing here can go stale behind the store's back.
#[derive(PartialEq)]
struct ScannedStatistics {
    facts: ColumnFacts,
    theme: InspectorTheme,
    scan: ScanId,
    key: DiffKey,
}

impl KeyExt for ScannedStatistics {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ScannedStatistics {
    fn render(&self) -> impl IntoElement {
        let engine = use_consume::<EngineCtx>();
        let query = use_profile(&engine, &self.facts.owner, self.scan);
        let session = use_radio_station::<SessionState, Chan>();
        let actions = use_profile_actions();
        // The one colour this component takes from the **sheet** rather than from its own theme:
        // a failure is semantic, and must follow the app-wide error ramp wherever it appears
        // (AGENTS.md §3).
        let danger = use_theme().read().colors.error;

        // Cloned out, so the query's read guard is gone before any element is built. A `Loading`
        // entry never carries a previous value here — every request is a fresh key — so it is
        // simply the running state.
        let reader = query.read();
        let state = match &*reader.state() {
            QueryStateData::Pending | QueryStateData::Loading { .. } => Scan::Running,
            QueryStateData::Settled { res: Ok(p), .. } => Scan::Done(p.clone()),
            QueryStateData::Settled { res: Err(e), .. } => Scan::Failed(e.clone()),
        };
        drop(reader);

        let kind = self.facts.owner_kind();
        let owner = self.facts.owner.clone();
        let t = &self.theme;

        match state {
            Scan::Running => {
                let cancel = {
                    let engine = engine.clone();
                    let owner = owner.clone();
                    move |_| {
                        // Both halves, because they answer different questions: the engine stops
                        // paying for the scan, and dropping the request is what puts the zone
                        // back to offering one — there is no result, so nothing else would be
                        // honest to show.
                        engine.cancel_profile(&owner);
                        actions.clear(kind, &owner);
                    }
                };
                zone(&self.facts, t, None, Some(running_row(t, cancel)))
            }
            Scan::Done(profile) => {
                // The scan's facts fold into the one list, matched on `StatKey` — so a fact can
                // never appear twice, and the completeness bar below picks up a *counted* null
                // count wherever the footer had none.
                let facts = with_scan(self.facts.clone(), &profile);
                let tail = rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(8.)
                    // What a nested field's absent facts mean, since the box above it would
                    // otherwise read as a scan that found nothing.
                    .maybe_child(facts.child.then(|| {
                        Path::new(NESTED_NOTE)
                            .color(t.note_color)
                            .wrap()
                            .into_element()
                    }))
                    .child(Path::new(scan_footnote(&profile)).color(t.meta_color));
                zone(
                    &facts,
                    t,
                    Some(scan_controls(t, &profile, owner, kind, session, actions)),
                    Some(tail.into_element()),
                )
            }
            // A scan that was stopped on purpose is not a failure to report — the zone simply goes
            // back to offering one, exactly as if it had never been asked for.
            Scan::Failed(error) if stopped_on_purpose(&error) => zone(
                &self.facts,
                t,
                None,
                Some(scan_card(&self.facts, t, actions)),
            ),
            // A real failure says why and offers the retry, which is one press: the request is
            // still on the row, so this goes straight through the confirm (P3-10).
            Scan::Failed(error) => {
                let tail = rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(12.)
                    .child(
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .content(Content::Flex)
                            .spacing(8.)
                            .child(
                                rect()
                                    .margin((1., 0., 0., 0.))
                                    .child(Icon::new(IconName::Alert).color(danger).size(14.)),
                            )
                            .child(
                                Prose::new(error)
                                    .color(t.note_color)
                                    .width(Size::flex(1.))
                                    .wrap(),
                            ),
                    )
                    .child(scan_card(&self.facts, t, actions));
                zone(&self.facts, t, None, Some(tail.into_element()))
            }
        }
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// What the subscribed scan is doing right now — the three states `query.read().state()` can be
/// in, with the settled value taken out of the borrow.
enum Scan {
    Running,
    Done(CatalogProfile),
    Failed(String),
}

/// What an absent set of facts means on a nested field: the scan ran, and deliberately says
/// nothing about it (`with_scan` refuses the lookup — see there for why).
const NESTED_NOTE: &str =
    "The scan describes top-level columns, so it reports nothing for a nested field.";

/// Was this scan stopped deliberately rather than broken?
///
/// `cancelled` is what a press of Cancel settles — and, for the moment before the store drops the
/// request, what a scan aborted by a re-registration settles. `superseded` is a re-scan replacing
/// this one. Neither is news the user needs told: they asked for it, or the app did on their
/// behalf. Both strings are the engine's, and pinned by its own tests
/// (`engine::tests::a_scan_is_work_in_flight_and_cancel_stops_it`).
fn stopped_on_purpose(error: &str) -> bool {
    error == "cancelled" || error.starts_with("superseded")
}

/// The zone's frame: the eyebrow (with whatever the scan half puts beside it), the facts box,
/// the completeness bar, and a tail.
///
/// One layout for all four states, so the box can't shift under the user as a scan settles.
fn zone(
    facts: &ColumnFacts,
    t: &InspectorTheme,
    controls: Option<Element>,
    tail: Option<Element>,
) -> Element {
    let rows = fact_rows(facts);
    let last = rows.len().saturating_sub(1);

    rect()
        .width(Size::fill())
        .vertical()
        .margin(Gaps::new(4., 0., 0., 0.))
        .padding(Gaps::new(16., PANEL_PAD, 24., PANEL_PAD))
        .spacing(12.)
        .border(Border::new().width(zone_rule()).fill(t.divider_fill))
        .child(
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .child(Eyebrow::new("STATISTICS").color(t.label_color))
                // The controls take the slack and sit at the panel edge, as the canvas has them.
                .maybe_child(controls.map(|c| {
                    rect()
                        .width(Size::flex(1.))
                        .horizontal()
                        .main_align(Alignment::End)
                        .cross_align(Alignment::Center)
                        .child(c)
                        .into_element()
                })),
        )
        .child(
            rect()
                .width(Size::fill())
                .vertical()
                .corner_radius(BOX_RADIUS)
                .overflow(Overflow::Clip)
                .border(Border::new().width(1.).fill(t.border_fill))
                .children(rows.into_iter().enumerate().map(|(i, row)| {
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::Flex)
                        .cross_align(Alignment::Center)
                        .spacing(PANEL_PAD)
                        .padding((8., PANEL_PAD))
                        .background(t.box_background)
                        .child(Eyebrow::new(row.label).color(t.label_color))
                        // The value takes the slack and right-aligns, so a long Min/Max
                        // (a timestamp, a truncated string bound) truncates at the panel
                        // edge instead of pushing its own key off the row.
                        .child(
                            MonoValue::new(row.value)
                                .color(t.value_color)
                                .align(TextAlign::Right)
                                .width(Size::flex(1.))
                                .text_overflow(TextOverflow::Ellipsis),
                        )
                        .maybe(i < last, |el| {
                            el.border(Border::new().width(row_rule()).fill(t.divider_fill))
                        })
                        .into()
                })),
        )
        .maybe_child(completeness_bar(facts, t))
        .maybe_child(tail)
        .into_element()
}

/// The scan offer (**P3-09**) — the canvas's primary call to action, in its full dress.
///
/// The copy names what a scan *adds*, which is the honest pitch: on a Parquet column the footer
/// has already given min/max, and what it can never give is a distinct count. Pressing it routes
/// through the cost confirm (P3-10) exactly as the row menus' item does — one entry point, so
/// the two can't drift.
fn scan_card(facts: &ColumnFacts, t: &InspectorTheme, actions: ProfileActions) -> Element {
    let kind = facts.owner_kind();
    let owner = facts.owner.clone();
    let copy = match facts.derived {
        true => "Running the view's query in full would compute distinct counts, means and medians.",
        false => "Reading every file would compute distinct counts, means and medians.",
    };

    rect()
        .width(Size::fill())
        .vertical()
        .cross_align(Alignment::Center)
        .spacing(12.)
        .corner_radius(BOX_RADIUS)
        .padding((24., 16.))
        .background(t.box_background)
        .border(Border::new().width(1.).fill(t.border_fill))
        .child(
            rect()
                .width(Size::px(TILE_SIZE))
                .height(Size::px(TILE_SIZE))
                .corner_radius(PANEL_RADIUS)
                .center()
                .background(t.tile_color.with_a(TILE_TINT))
                .child(Icon::new(IconName::Chart).color(t.tile_color).size(17.)),
        )
        .child(
            Prose::new(copy)
                .color(t.note_color)
                .max_width(Size::px(CARD_COPY_WIDTH))
                .align(TextAlign::Center)
                .wrap(),
        )
        .child(
            Button::new()
                // The stock **filled** dress: the accent over inverse text, which is both
                // the canvas's `background: var(--accent); color: var(--c-onaccent)` and the
                // Run control's idle state. No `theme_colors` override — a call site that
                // restates colours the variant already resolves is a second copy of them.
                .filled()
                // A committing action is the design system's 34px everywhere — one number,
                // in `components`. The rest of the layout (padding, radius) stays the
                // `button_layout` theme's.
                .theme_layout(ButtonLayoutThemePartial::default().height(Size::px(ACTION_HEIGHT)))
                // The one entry point, shared with the row menus' item: a first scan asks the
                // cost confirm, a retry goes straight through (`ProfileActions::ask`).
                .on_press(move |_| actions.ask(kind, &owner))
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(8.)
                        .child(Icon::new(IconName::Chart).size(14.))
                        .child(Control::new(ProfileTarget::verb(kind))),
                ),
        )
        .into_element()
}

/// The zone while the scan runs: the canvas's spinner row, plus a Cancel — a scan is the most
/// expensive thing the app does, so stopping it has to be one press.
///
/// No Esc binding, deliberately: Esc already cancels the *query* while the results pane is
/// running, and one key that means "cancel whichever of two things you were thinking of" is worse
/// than a button that says which.
fn running_row(
    t: &InspectorTheme,
    on_cancel: impl Into<EventHandler<Event<PressEventData>>>,
) -> Element {
    rect()
        .width(Size::fill())
        .horizontal()
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .spacing(PANEL_PAD)
        .corner_radius(BOX_RADIUS)
        .padding((16., PANEL_PAD))
        .background(t.box_background)
        .border(Border::new().width(1.).fill(t.border_fill))
        .child(CircularLoader::new().size(15.).a11y_alt(SCANNING))
        .child(
            Body::new(SCANNING)
                .color(t.note_color)
                .width(Size::flex(1.))
                .text_overflow(TextOverflow::Ellipsis),
        )
        .child(
            Button::new()
                .flat()
                .on_press(on_cancel)
                .child(Control::new("Cancel")),
        )
        .into_element()
}

/// What the running row says — and what a screen reader hears from the spinner, so the two can't
/// disagree.
const SCANNING: &str = "Scanning…";

/// The settled scan's header controls: how old it is, the query that produced it, and ↻.
///
/// They live in the STATISTICS header rather than in a card, because once a scan exists the card
/// has nothing left to offer — this is the canvas's own arrangement.
fn scan_controls(
    t: &InspectorTheme,
    profile: &CatalogProfile,
    owner: String,
    kind: CatalogKind,
    session: RadioStation<SessionState, Chan>,
    actions: ProfileActions,
) -> Element {
    let sql = profile.sql.clone();
    let name = owner.clone();
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(4.)
        .child(Meta::new(scan_age(profile.at)).color(t.meta_color))
        // **View as query** — the profile is never a black box. Absent when the unparser
        // couldn't render an expression (`profile_sql` returns nothing then): no button beats a
        // button that opens a query which doesn't run.
        .maybe_child((!sql.is_empty()).then(|| {
            control_button(
                IconName::Brackets,
                "Open the profile query in a new tab",
                move |_| {
                    let mut session = session;
                    session.write_channel(Chan::Tabs).open_or_focus(
                        &format!("profile · {name}"),
                        sql.clone(),
                        Origin::Scratch,
                    );
                },
            )
        }))
        // ↻ **Re-scan** — an explicit re-scan, so it skips the cost confirm (P3-10) by going
        // straight to the request rather than through `ask`.
        .child(control_button(IconName::Reload, "Re-scan", move |_| {
            actions.start(kind, &owner);
        }))
        .into_element()
}

/// One of the zone header's ghost icon buttons — the inspector's own 24×24 (the canvas's
/// `_IB24`), with its title as a tooltip.
fn control_button(
    icon: IconName,
    title: &'static str,
    on_press: impl Into<EventHandler<Event<PressEventData>>>,
) -> Element {
    TooltipContainer::new(Tooltip::new(title))
        .position(AttachedPosition::Bottom)
        .child(
            Button::new()
                .flat()
                .width(Size::px(24.))
                .height(Size::px(24.))
                .on_press(on_press)
                .child(Icon::new(icon).size(13.)),
        )
        .into_element()
}
