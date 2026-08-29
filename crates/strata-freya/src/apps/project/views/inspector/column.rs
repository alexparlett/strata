//! The inspector's body once a column has resolved: its title, the note a derived column
//! carries, the shape of a nested type, and the STATISTICS zone — the facts box, the
//! completeness bar, and the scan.
//!
//! Built to the `Strata.dc.html` inspector canvas. Every state goes through one frame ([`zone`]),
//! so the box can't shift under the user as a scan settles: the offer of a scan ([`scan_card`]),
//! the scan running ([`running_row`]), what it found (its facts folded into the one list, under
//! the canvas's age / view-as-query / ↻ controls), and a failure with the retry beside it.
//!
//! Which of those is showing is [`shown`] — a pure state machine over one rule: **the zone never
//! shows less than it did a moment ago.** A re-scan keeps the numbers it is replacing and stays
//! silent unless it outlasts the hold, so re-profiling a small table changes the numbers rather
//! than flashing a spinner where they were. See [`ScannedStatistics`].
//!
//! **Deliberately not built: the canvas's distribution bars.** The profile carries no
//! distribution data — the scan computes distinct / min / max / mean / median (`core::profile`),
//! and bins would need boundaries, which need min/max first, i.e. a *second* full pass over data
//! we have just told the user is expensive to read once. The canvas's bars were prototype seed
//! data; a bar drawn from anything else here would be the fabrication this panel exists to
//! avoid. Recorded in the P3-09 task file and `DEV_TASKS` D4.

use freya::components::CircularLoader;
use freya::prelude::*;
use freya::query::QueryStateData;
use freya::radio::{use_radio_station, RadioStation};
use strata_arrow::profile::stats_footnote;
use strata_core::util::iso8601;
use strata_engine::EngineError;
use strata_model::{CatalogProfile, Origin};

use super::model::{
    completeness, fact_rows, nested_fields, scan_age, scan_footnote, with_scan, ColumnFacts,
    FormatBadge, NestedField,
};
use super::{InspectorTheme, PANEL_PAD};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{use_profile, ProfileTarget, ScanId};
use crate::apps::project::state::{Chan, SessionState};
use crate::apps::project::views::{profile_verb, use_profile_actions, ProfileActions};
use crate::components::badge::Badge;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{
    use_progress_hold, ACTION_HEIGHT, HAIRLINE, R_2, R_3, R_XS, SP_1, SP_2, SP_3, SP_4, SP_5, SP_6,
};
use crate::components::tones::tones;
use crate::components::type_palette::{kind_color, type_palette, TypePaletteTheme};
use crate::components::typography::{Body, Control, Eyebrow, Meta, MonoValue, Path, Prose};

/// Corner radius of the facts box and the profile card (canvas `--r-3`); the smaller boxes use
/// `--r-2`, and the badges `--r-xs`.
const BOX_RADIUS: f32 = R_3;
const PANEL_RADIUS: f32 = R_2;
const BADGE_RADIUS: f32 = R_XS;
/// A nested field row's height, and the indent one nesting level adds.
const FIELD_HEIGHT: f32 = 27.;
const FIELD_INDENT: f32 = SP_4;
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
        let palette = type_palette();
        let swatch = kind_color(self.facts.kind, &palette);
        let fields = nested_fields(&self.facts.children);

        rect()
            .width(Size::fill())
            .vertical()
            .child(self.title(swatch))
            .maybe_child(self.facts.derived.then(|| self.derived_note()))
            .maybe_child((!fields.is_empty()).then(|| self.nested_box(fields, &palette)))
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
            .padding(Gaps::new(SP_5, PANEL_PAD, SP_5, PANEL_PAD))
            .spacing(SP_3)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(SP_3)
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
                    .content(Content::wrap_spacing(SP_3))
                    .spacing(SP_3)
                    .child(Badge::value(self.facts.dtype.clone(), swatch).radius(BADGE_RADIUS))
                    .child(
                        Badge::tag(self.facts.format.label(), self.format_color())
                            .radius(BADGE_RADIUS)
                            .padding(Gaps::new(SP_1, SP_3, SP_1, SP_3)),
                    )
                    .child(Path::new(format!("from {}", self.facts.owner())).color(t.meta_color)),
            )
            .into_element()
    }

    /// The badge tone for this column's source format. An unknown format keeps the recessive
    /// tone rather than borrowing a colour that means something else.
    fn format_color(&self) -> Color {
        let t = &self.theme;
        match self.facts.format {
            FormatBadge::Parquet => t.format_parquet_color,
            FormatBadge::Csv => t.format_csv_color,
            FormatBadge::Json => t.format_json_color,
            FormatBadge::Arrow => t.format_arrow_color,
            FormatBadge::View => t.format_view_color,
            FormatBadge::Source(_) => t.format_database_color,
            FormatBadge::Other(_) => t.meta_color,
        }
    }

    /// Why this column has no free facts under its type — which is a different sentence for each
    /// of the two owners that report none, because the reason really is different: a view's
    /// columns are defined by its query, and a remote relation's bytes are the server's.
    fn derived_note(&self) -> Element {
        let t = &self.theme;
        let copy = match &self.facts.target {
            ProfileTarget::Remote { .. } => {
                "Read through a source. The server reports the column's type; \
                 anything more is a scan."
            }
            ProfileTarget::Workspace { .. } => {
                "Derived column, defined by the view's query. There are no files under it, \
                 so the source reports no statistics."
            }
        };
        rect()
            .width(Size::fill())
            .margin(Gaps::new(0., PANEL_PAD, 0., PANEL_PAD))
            .padding(PANEL_PAD)
            .corner_radius(PANEL_RADIUS)
            .background(t.box_background)
            .border(Border::new().width(1.).fill(t.border_fill))
            .child(Path::new(copy).color(t.note_color).wrap())
            .into_element()
    }

    /// NESTED FIELDS — the shape of a struct / list / map column, at every depth.
    fn nested_box(&self, fields: Vec<NestedField>, palette: &TypePaletteTheme) -> Element {
        let t = &self.theme;
        let last = fields.len().saturating_sub(1);
        rect()
            .width(Size::fill())
            .vertical()
            .padding(Gaps::new(SP_5, PANEL_PAD, SP_3, PANEL_PAD))
            .spacing(SP_3)
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
                            .spacing(SP_3)
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
                            .maybe(i < last, |el| {
                                el.border(Border::new().width(row_rule()).fill(t.divider_fill))
                            })
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
            .spacing(SP_3)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .main_align(Alignment::SpaceBetween)
                    .child(Meta::new("Completeness").color(t.note_color))
                    .child(Meta::new(fill.label()).color(t.emphasis_color)),
            )
            .child(
                TooltipContainer::new(Tooltip::new_text(fill.detail()))
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
        let actions = use_profile_actions();
        match self.facts.scan {
            None => zone(
                &self.facts,
                &self.theme,
                None,
                Some(scan_card(&self.facts, &self.theme, actions)),
            ),
            Some(scan) => ScannedStatistics {
                facts: self.facts.clone(),
                theme: self.theme.clone(),
                scan,
                key: DiffKey::None,
            }
            .key(self.facts.owner())
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
///
/// **The zone never shows less than it did a moment ago.** A re-scan is a new request, hence a new
/// cache key and a `Pending` entry, which on its own would blank the facts box and put a spinner
/// where the numbers were. Two interlocking halves:
///
/// 1. **The last settled numbers stay live** ([`held`](Self::held)) — they were true as of their
///    own timestamp, and the header says when that was.
/// 2. **The re-scan only announces itself once it has outlasted [`PROGRESS_HOLD`]**, so inside that
///    window a re-scan is invisible: numbers, then different numbers.
///
/// A **first** scan is exempt from the hold: there is nothing to hold onto, and a press that shows
/// nothing for 400ms reads as a press that missed.
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
        let target = self.facts.target.clone();
        let query = use_profile(&engine, &target, self.scan);
        let session = use_radio_station::<SessionState, Chan>();
        let actions = use_profile_actions();
        let danger = tones().error;

        let reader = query.read();
        let state = match &*reader.state() {
            QueryStateData::Pending | QueryStateData::Loading { .. } => Scan::Running,
            QueryStateData::Settled { res: Ok(p), .. } => Scan::Done(p.clone()),
            QueryStateData::Settled { res: Err(e), .. } => Scan::Failed(e.clone()),
        };
        drop(reader);

        let mut held = use_state(|| None::<CatalogProfile>);
        use_side_effect(move || {
            if let QueryStateData::Settled { res: Ok(p), .. } = &*query.read().state() {
                held.set_if_modified(Some(p.clone()));
            }
        });

        let running = matches!(state, Scan::Running);
        let announced = use_progress_hold(running);

        let t = &self.theme;
        let previous = held.read();
        let cancel = {
            let target = target.clone();
            move |_| {
                engine.catalog().cancel_profile(&target.sql_name());
                actions.clear(&target);
            }
        };

        let settled = |profile: &CatalogProfile, tail: Option<Element>| {
            let facts = with_scan(self.facts.clone(), profile);
            let footnotes = rect()
                .width(Size::fill())
                .vertical()
                .spacing(SP_3)
                .maybe_child(facts.child.then(|| {
                    Path::new(NESTED_NOTE)
                        .color(t.note_color)
                        .wrap()
                        .into_element()
                }))
                .child(Path::new(scan_footnote(profile)).color(t.meta_color))
                .maybe_child(
                    stats_footnote(target.profiled())
                        .map(|note| Path::new(note).color(t.meta_color).wrap().into_element()),
                )
                .maybe_child(tail);
            zone(
                &facts,
                t,
                Some(scan_controls(t, profile, &target, session, actions)),
                Some(footnotes.into_element()),
            )
        };

        match shown(&state, previous.as_ref(), announced) {
            Shown::FirstScan => zone(&self.facts, t, None, Some(running_row(t, SCANNING, cancel))),
            Shown::ReScan(profile) => settled(profile, Some(running_row(t, RESCANNING, cancel))),
            Shown::Facts(profile) => settled(profile, None),
            Shown::Failed(error, previous) => {
                let reason = rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .spacing(SP_3)
                    .child(
                        rect()
                            .margin((HAIRLINE, 0., 0., 0.))
                            .child(Icon::new(IconName::Alert).color(danger).size(14.)),
                    )
                    .child(
                        Prose::new(error.to_string())
                            .color(t.note_color)
                            .width(Size::flex(1.))
                            .wrap(),
                    );
                match previous {
                    Some(profile) => settled(profile, Some(reason.into_element())),
                    None => zone(
                        &self.facts,
                        t,
                        None,
                        Some(
                            rect()
                                .width(Size::fill())
                                .vertical()
                                .spacing(SP_4)
                                .child(reason)
                                .child(scan_card(&self.facts, t, actions))
                                .into_element(),
                        ),
                    ),
                }
            }
            Shown::Offer => zone(
                &self.facts,
                t,
                None,
                Some(scan_card(&self.facts, t, actions)),
            ),
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
    Failed(EngineError),
}

/// What the zone actually puts on screen.
#[derive(PartialEq, Debug)]
enum Shown<'a> {
    /// A scan is running and there is nothing yet to show — say so immediately.
    FirstScan,
    /// A scan is running over numbers already on screen, and has outlasted the hold: keep them,
    /// and say a re-scan is why they are still the old ones.
    ReScan(&'a CatalogProfile),
    /// Numbers. Either just settled, or the previous ones standing in for a re-scan too quick to
    /// be worth announcing.
    Facts(&'a CatalogProfile),
    /// The scan failed, over whatever was on screen before it ran.
    Failed(&'a EngineError, Option<&'a CatalogProfile>),
    /// Nothing to show and nothing running — offer the scan.
    Offer,
}

/// **The zone never shows less than it did a moment ago.** Pure, and tested as such: this is the
/// rule the user sees, and it is easier to get wrong than to read.
///
/// `held` is the last numbers this entry settled; `announced` is whether the scan in flight has
/// outlasted [`PROGRESS_HOLD`]. Inside that hold a re-scan is *invisible* — the numbers simply
/// change when the new ones land — which is only possible because `held` keeps them there.
fn shown<'a>(scan: &'a Scan, held: Option<&'a CatalogProfile>, announced: bool) -> Shown<'a> {
    match scan {
        Scan::Done(profile) => Shown::Facts(profile),
        Scan::Running => match held {
            None => Shown::FirstScan,
            Some(previous) if announced => Shown::ReScan(previous),
            Some(previous) => Shown::Facts(previous),
        },
        Scan::Failed(EngineError::Stopped(_)) => match held {
            Some(previous) => Shown::Facts(previous),
            None => Shown::Offer,
        },
        Scan::Failed(e) => Shown::Failed(e, held),
    }
}

/// What an absent set of facts means on a nested field: the scan ran, and deliberately says
/// nothing about it (`with_scan` refuses the lookup — see there for why).
const NESTED_NOTE: &str =
    "The scan describes top-level columns, so it reports nothing for a nested field.";

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
        .margin(Gaps::new(SP_2, 0., 0., 0.))
        .padding(Gaps::new(SP_5, PANEL_PAD, SP_6, PANEL_PAD))
        .spacing(SP_4)
        .border(Border::new().width(zone_rule()).fill(t.divider_fill))
        .child(
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .child(Eyebrow::new("STATISTICS").color(t.label_color))
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
                        .padding((SP_3, PANEL_PAD))
                        .background(t.box_background)
                        .child(Eyebrow::new(row.label).color(t.label_color))
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
    let target = facts.target.clone();
    let copy = match (&target, facts.derived) {
        (ProfileTarget::Remote { .. }, _) => {
            "One statement on the database would compute distinct counts and means."
        }
        (_, true) => {
            "Running the view's query in full would compute distinct counts, means and medians."
        }
        (_, false) => "Reading every file would compute distinct counts, means and medians.",
    };
    let verb = profile_verb(target.kind());

    rect()
        .width(Size::fill())
        .vertical()
        .cross_align(Alignment::Center)
        .spacing(SP_4)
        .corner_radius(BOX_RADIUS)
        .padding((SP_6, SP_5))
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
                .filled()
                .theme_layout(ButtonLayoutThemePartial::default().height(Size::px(ACTION_HEIGHT)))
                .on_press(move |_| actions.ask(&target))
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(SP_3)
                        .child(Icon::new(IconName::Chart).size(14.))
                        .child(Control::new(verb)),
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
    label: &'static str,
    on_cancel: impl Into<EventHandler<Event<PressEventData>>>,
) -> Element {
    rect()
        .width(Size::fill())
        .horizontal()
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .spacing(PANEL_PAD)
        .corner_radius(BOX_RADIUS)
        .padding((SP_5, PANEL_PAD))
        .background(t.box_background)
        .border(Border::new().width(1.).fill(t.border_fill))
        .child(CircularLoader::new().size(15.).a11y_alt(label))
        .child(
            Body::new(label)
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

/// What the running row says — and what a screen reader hears from its spinner, so the two can't
/// disagree.
///
/// The two are worth telling apart: `Scanning…` sits over an empty zone, while `Re-scanning…` sits
/// *under numbers that are still on screen* and is the sentence that explains why they haven't
/// changed yet.
const SCANNING: &str = "Scanning…";
const RESCANNING: &str = "Re-scanning…";

/// The settled scan's header controls: how old it is, the query that produced it, and ↻.
///
/// They live in the STATISTICS header rather than in a card, because once a scan exists the card
/// has nothing left to offer — this is the canvas's own arrangement.
fn scan_controls(
    t: &InspectorTheme,
    profile: &CatalogProfile,
    target: &ProfileTarget,
    session: RadioStation<SessionState, Chan>,
    actions: ProfileActions,
) -> Element {
    let sql = profile.sql.clone();
    let name = target.label();
    let rescan = target.clone();
    rect()
        .horizontal()
        .cross_align(Alignment::Center)
        .spacing(SP_2)
        .child(
            TooltipContainer::new(Tooltip::new_text(iso8601(profile.at)))
                .position(AttachedPosition::Bottom)
                .child(Meta::new(scan_age(profile.at)).color(t.meta_color)),
        )
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
        .child(control_button(IconName::Reload, "Re-scan", move |_| {
            actions.start(&rescan);
        }))
        .into_element()
}

/// One of the zone header's ghost icon buttons — the inspector's own 24×24 (the canvas's
/// `_IB24`), with its title as a tooltip.
fn control_button(
    icon: IconName,
    title: &'static str,
    on_press: impl Into<EventHandler<Event<PressEventData>>>,
) -> impl IntoElement {
    TooltipContainer::new(Tooltip::new_text(title))
        .position(AttachedPosition::Bottom)
        .child(
            Button::new()
                .flat()
                .width(Size::px(24.))
                .height(Size::px(24.))
                .on_press(on_press)
                .child(Icon::new(icon).size(13.)),
        )
}

/// The zone's one behavioural rule, as a state machine: **it never shows less than it did a moment
/// ago.** Worth testing pure, because every case here was reported as a flicker before it was a
/// rule, and none of them is visible in a screenshot of the settled panel.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::SystemTime;

    use strata_engine::StopReason;

    use super::*;

    fn profile(rows: u64) -> CatalogProfile {
        CatalogProfile {
            at: SystemTime::now(),
            rows,
            sql: "SELECT 1".into(),
            cols: BTreeMap::new(),
        }
    }

    /// A **first** scan says so at once: there is nothing to hold onto, and a press that shows
    /// nothing for the length of the hold reads as a press that missed.
    #[test]
    fn a_first_scan_announces_itself_immediately() {
        assert_eq!(shown(&Scan::Running, None, false), Shown::FirstScan);
        assert_eq!(shown(&Scan::Running, None, true), Shown::FirstScan);
    }

    /// **The flicker fix.** A re-scan of a small table settles well inside the hold, and for that
    /// whole time the zone is indistinguishable from settled — the numbers simply change when the
    /// new ones land. This is the case that used to blank the facts box and flash a spinner.
    #[test]
    fn a_quick_re_scan_is_invisible() {
        let previous = profile(5);
        assert_eq!(
            shown(&Scan::Running, Some(&previous), false),
            Shown::Facts(&previous),
            "inside the hold: the numbers already on screen, and no spinner at all"
        );
    }

    /// Past the hold the wait is news, so it is said — *under* the numbers, which stay put. The
    /// row is also what carries Cancel, which a long scan needs within reach.
    #[test]
    fn a_slow_re_scan_says_why_the_numbers_are_still_the_old_ones() {
        let previous = profile(5);
        assert_eq!(
            shown(&Scan::Running, Some(&previous), true),
            Shown::ReScan(&previous)
        );
    }

    /// Settled numbers win over anything held — that is what "something better replaced them"
    /// means.
    ///
    /// The settled profile is **cloned, not rebuilt**: `profile()` stamps `at: SystemTime::now()`
    /// and `CatalogProfile`'s `PartialEq` includes it, so two `profile(9)` calls compared equal only
    /// while both landed on the same instant — a ~1-in-12 flake, found when a full-suite run lost
    /// the race.
    #[test]
    fn settled_numbers_replace_the_held_ones() {
        let previous = profile(5);
        let fresh = profile(9);
        assert_eq!(
            shown(&Scan::Done(fresh.clone()), Some(&previous), true),
            Shown::Facts(&fresh)
        );
    }

    /// A scan **stopped on purpose** is not a failure to report: it falls back to whatever was on
    /// screen, or to offering the scan again where there was nothing. This is the arm a
    /// re-registration's abort lands in for the moment before the store drops the request.
    #[test]
    fn a_scan_stopped_on_purpose_reports_nothing() {
        let previous = profile(5);
        for stopped in [StopReason::Cancelled, StopReason::SupersededScan] {
            let scan = Scan::Failed(EngineError::Stopped(stopped));
            assert_eq!(shown(&scan, Some(&previous), true), Shown::Facts(&previous));
            assert_eq!(shown(&scan, None, true), Shown::Offer);
        }
    }

    /// A real failure says why — and a failed *re*-scan still keeps what it was replacing, for the
    /// same reason a running one does. Dropping numbers on a transient failure is the flicker
    /// again, one beat later.
    #[test]
    fn a_failure_says_why_and_keeps_whatever_it_was_replacing() {
        let previous = profile(5);
        let why = EngineError::Failed("Schema error: No field named x".to_string());
        let scan = Scan::Failed(why.clone());
        assert_eq!(
            shown(&scan, Some(&previous), true),
            Shown::Failed(&why, Some(&previous))
        );
        assert_eq!(
            shown(&scan, None, true),
            Shown::Failed(&why, None),
            "a failed first scan has nothing to fall back to, so it offers the retry"
        );
    }
}
