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

use async_io::Timer;
use freya::components::CircularLoader;
use freya::prelude::*;
use freya::query::QueryStateData;
use freya::radio::{use_radio_station, RadioStation};
use strata_core::engine::profile::CatalogProfile;
use strata_core::engine::stopped_on_purpose;
use strata_core::util::iso8601;
use strata_model::{CatalogKind, Origin};

use super::model::{
    completeness, fact_rows, nested_fields, scan_age, scan_footnote, with_scan, ColumnFacts,
    FormatBadge, NestedField,
};
use super::{InspectorTheme, PANEL_PAD};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{use_profile, ScanId};
use crate::apps::project::state::{Chan, SessionState};
use crate::apps::project::views::{use_profile_actions, ProfileActions, ProfileTarget};
use crate::components::badge::Badge;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{
    ACTION_HEIGHT, HAIRLINE, PROGRESS_HOLD, R_2, R_3, R_XS, SP_1, SP_2, SP_3, SP_4, SP_5, SP_6,
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
                    // The three runs wrap rather than truncate: a long dtype (`Timestamp`,
                    // `Decimal`) beside a long owner name would otherwise push "from …" out of
                    // a narrow panel.
                    .content(Content::wrap_spacing(SP_3))
                    .spacing(SP_3)
                    .child(Badge::value(self.facts.dtype.clone(), swatch).radius(BADGE_RADIUS))
                    .child(
                        Badge::tag(self.facts.format.label(), self.format_color())
                            .radius(BADGE_RADIUS)
                            // The format badge hugs like a value run, not like a tag: it sits
                            // beside the dtype and the two must read as one pair.
                            .padding(Gaps::new(SP_1, SP_3, SP_1, SP_3)),
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
            FormatBadge::Parquet => t.format_parquet_color,
            FormatBadge::Csv => t.format_csv_color,
            FormatBadge::Json => t.format_json_color,
            FormatBadge::Arrow => t.format_arrow_color,
            FormatBadge::View => t.format_view_color,
            FormatBadge::Other(_) => t.meta_color,
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
                            // The rule sits *between* rows: on the last one it would double up
                            // with the box's own bottom edge.
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
            // The bar carries no numbers, so the numbers are its tooltip: how many rows are
            // null, out of how many, and which side of the split is which.
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
            // Keyed on the **entry**, not on the request: a ↻ re-scan has to keep the numbers it
            // is replacing on screen, and it can only do that from a scope that survives the new
            // request (see `ScannedStatistics::held`). Switching entries *is* a remount, so one
            // table's numbers can never be held over another's.
            Some(scan) => ScannedStatistics {
                facts: self.facts.clone(),
                theme: self.theme.clone(),
                scan,
                key: DiffKey::None,
            }
            .key(&self.facts.owner)
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
/// ## The zone never shows less than it did a moment ago
///
/// A re-scan is a *new* request, hence a new cache key, hence a `Pending` entry — so on its own it
/// would blank the facts box and put a spinner where the numbers were, then swap back a moment
/// later. On a small table that whole round trip is a flicker: the eye reads it as a glitch rather
/// than as a state.
///
/// Two halves, and they interlock — the second is only possible because of the first:
///
/// 1. **The last settled numbers stay live** ([`held`](Self::held)). They were true as of their own
///    timestamp, and the header says when that was, so there is nothing dishonest about leaving
///    them up while the next pass runs.
/// 2. **The re-scan only announces itself once it has outlasted [`PROGRESS_HOLD`]** — the same hold
///    the catalog row's registration spinner serves. Inside it, a re-scan is invisible: numbers,
///    then different numbers.
///
/// A **first** scan is deliberately exempt from the hold: there is nothing to hold onto, and a
/// press that shows nothing for 400ms reads as a press that missed.
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
        // The one colour this component takes from the **shared ramp** rather than from its own
        // theme: a failure is semantic, and must follow the app-wide error ramp wherever it
        // appears (AGENTS.md §3).
        let danger = tones().error;

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

        // The last numbers this entry settled — what a re-scan keeps on screen (see the type doc).
        // Remembered in an effect rather than during render, and only when it actually moves, so a
        // settle costs one render and not two.
        let mut held = use_state(|| None::<CatalogProfile>);
        use_side_effect(move || {
            if let QueryStateData::Settled { res: Ok(p), .. } = &*query.read().state() {
                held.set_if_modified(Some(p.clone()));
            }
        });

        // Whether the scan in flight has outlasted the hold, and so is worth saying out loud.
        // Re-armed from zero on every entry into (and exit from) a scan — the same shape, and the
        // same reasoning, as the catalog row's status slot.
        let running = matches!(state, Scan::Running);
        let announced = use_state(|| false);
        let pending = use_state(|| None::<TaskHandle>);
        use_side_effect_with_deps(&running, move |running| {
            let mut announced = announced;
            let mut pending = pending;
            if let Some(task) = pending.write().take() {
                task.cancel();
            }
            announced.set_if_modified(false);
            if *running {
                pending.set(Some(spawn(async move {
                    Timer::after(PROGRESS_HOLD).await;
                    announced.set_if_modified(true);
                })));
            }
        });

        let kind = self.facts.owner_kind();
        let owner = self.facts.owner.clone();
        let t = &self.theme;
        let previous = held.read();
        let cancel = {
            let owner = owner.clone();
            move |_| {
                // Both halves, because they answer different questions: the engine stops paying
                // for the scan, and dropping the request is what puts the zone back to offering
                // one — there is no result of *this* scan, so nothing else would be honest.
                engine.cancel_profile(&owner);
                actions.clear(kind, &owner);
            }
        };

        // The facts + controls half is identical for settled and held numbers, which is the whole
        // point: a re-scan in progress is not a different-looking panel.
        let settled = |profile: &CatalogProfile, tail: Option<Element>| {
            // The scan's facts fold into the one list, matched on `StatKey` — so a fact can never
            // appear twice, and the completeness bar picks up a *counted* null count wherever the
            // footer had none.
            let facts = with_scan(self.facts.clone(), profile);
            let footnotes = rect()
                .width(Size::fill())
                .vertical()
                .spacing(SP_3)
                // What a nested field's absent facts mean, since the box above it would
                // otherwise read as a scan that found nothing.
                .maybe_child(facts.child.then(|| {
                    Path::new(NESTED_NOTE)
                        .color(t.note_color)
                        .wrap()
                        .into_element()
                }))
                .child(Path::new(scan_footnote(profile)).color(t.meta_color))
                .maybe_child(tail);
            zone(
                &facts,
                t,
                Some(scan_controls(
                    t,
                    profile,
                    owner.clone(),
                    kind,
                    session,
                    actions,
                )),
                Some(footnotes.into_element()),
            )
        };

        match shown(&state, previous.as_ref(), *announced.read()) {
            // Nothing on screen yet, so the press says so at once — no hold.
            Shown::FirstScan => zone(&self.facts, t, None, Some(running_row(t, SCANNING, cancel))),
            // A re-scan slow enough to be worth saying: the numbers stay, the row explains why
            // they are still the old ones, and Cancel is right there.
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
                            // A 1px optical nudge onto the first line of the prose beside it — an alignment
                            // nudge, which the design keeps literal.
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
                    // A failed *re*-scan keeps what it was replacing, for the same reason a
                    // running one does — and the ↻ in the header is the retry.
                    Some(profile) => settled(profile, Some(reason.into_element())),
                    // A failed first scan says why and offers the retry, which is one press: the
                    // request is still on the row, so it goes straight through the confirm.
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
    Failed(String),
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
    Failed(&'a str, Option<&'a CatalogProfile>),
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
        // Stopped on purpose is not a failure to report: the zone falls back to whatever it had,
        // or to offering the scan again.
        Scan::Failed(e) if stopped_on_purpose(e) => match held {
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

// Was this scan stopped deliberately rather than broken?
//
// `cancelled` is what a press of Cancel settles — and, for the moment before the store drops the
// request, what a scan aborted by a re-registration settles. `superseded` is a re-scan replacing
// this one. Neither is news the user needs told: they asked for it, or the app did on their behalf.
//
// The rule itself is the **engine's** (`engine::stopped_on_purpose`), because the strings are: this
// used to be a local `== "cancelled" || starts_with("superseded")`, and the event log grew a second
// copy that had already drifted (it caught the cancel but not the supersede). One definition, beside
// the constants that produce it.

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
                        .padding((SP_3, PANEL_PAD))
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
        true => {
            "Running the view's query in full would compute distinct counts, means and medians."
        }
        false => "Reading every file would compute distinct counts, means and medians.",
    };

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
                        .spacing(SP_3)
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
        .spacing(SP_2)
        // The age is coarse on purpose (`scan_age`), so the exact instant is its tooltip — the
        // same trade the completeness bar makes with its own numbers. ISO-8601, UTC, from the one
        // place that prints instants (`util::iso8601`).
        .child(
            TooltipContainer::new(Tooltip::new_text(iso8601(profile.at)))
                .position(AttachedPosition::Bottom)
                .child(Meta::new(scan_age(profile.at)).color(t.meta_color)),
        )
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
    /// screen, or to offering the scan again where there was nothing. Both strings are the
    /// engine's, and this is the arm a re-registration's abort lands in for the moment before the
    /// store drops the request.
    #[test]
    fn a_scan_stopped_on_purpose_reports_nothing() {
        let previous = profile(5);
        for stopped in ["cancelled", "superseded by a newer scan"] {
            let scan = Scan::Failed(stopped.to_string());
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
        let scan = Scan::Failed("Schema error: No field named x".to_string());
        assert_eq!(
            shown(&scan, Some(&previous), true),
            Shown::Failed("Schema error: No field named x", Some(&previous))
        );
        assert_eq!(
            shown(&scan, None, true),
            Shown::Failed("Schema error: No field named x", None),
            "a failed first scan has nothing to fall back to, so it offers the retry"
        );
    }
}
