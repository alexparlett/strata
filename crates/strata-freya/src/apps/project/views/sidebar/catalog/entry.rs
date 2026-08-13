//! The catalog's rows: a table / view **entry** that expands to its column tree, one **column**
//! row (nested or not), and a **saved-query** row.
//!
//! Tables and views share `EntryRow` and the column list below it, because a view's columns *are*
//! columns — clickable, selectable, expandable when nested. In the Dioxus sidebar these were two
//! copies that differed only by omission (view rows had no click handler at all, so clicking one
//! silently did nothing), which is exactly what a second copy of a list buys you.

use std::borrow::Cow;
use std::collections::HashSet;
use std::time::Duration;

use async_io::Timer;
use freya::components::CircularLoader;
use freya::prelude::*;
use freya::query::QueryStateData;
use freya::radio::use_radio;
use strata_model::{CatalogKind, ColRef, RightPane};
use uuid::Uuid;

use super::columns::{flatten_cols, ColRow};
use super::menu::{
    open_saved_query, query_menu, rename_saved_query, table_menu, use_catalog_actions, view_menu,
};
use super::CatalogTheme;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{use_profile, ScanId};
use crate::apps::project::state::{
    use_catalog_selection, Chan, ProjChan, ProjectState, Reg, SessionState,
};
use crate::components::badge::Badge;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{PROGRESS_HOLD, ROW_ACTION, STATUS_DOT};
use crate::components::metrics::{SP_1, SP_2, SP_3, SP_4, SP_5};
use crate::components::sidebar_row::SidebarRow;
use crate::components::type_palette::{kind_color, type_palette};
use crate::components::typography::{scale, Body, InputTypography, Meta, MonoValue};
use crate::keymap::on_command;
use crate::state::use_config_station;
use strata_core::config::Command;
use strata_core::util::clip;

/// Row heights + the column block's indent, from the design canvas.
const ENTRY_HEIGHT: f32 = 30.;
const COLUMN_HEIGHT: f32 = 25.;
/// Indent added per nesting level of a struct/list/map column.
const DEPTH_INDENT: f32 = SP_4;
/// The chevron gutter on a column row — reserved whether or not the column has one, so names
/// line up.
const CHEVRON_SLOT: f32 = 11.;
/// What the spinner says on hover (and to a screen reader).
const LOADING: &str = "Loading…";
/// How long a row must stay unanswered before it is worth spinning about. Most registrations land
/// well inside this, so the usual project open is a catalog that simply appears — no flicker of
/// spinners on the way in.
///
/// The design system's shared hold (`components::metrics::PROGRESS_HOLD`), because the inspector's re-scan
/// row serves the same one — see there for the half of the rule this row already had: a hold needs
/// something to hold *onto*, and what this slot holds is the last verdict it showed.
const SPINNER_DELAY: Duration = PROGRESS_HOLD;

/// What the **profiling** spinner says — its own words, because the registration spinner beside
/// it means something else entirely (a scan is minutes of work the user asked for; a
/// registration is a metadata read they didn't).
const PROFILING: &str = "Profiling…";

/// A status glyph wearing its message as a tooltip. Dropped below, like the rest of the app's
/// overlays, so it can't cover the row above it in a dense list.
fn tip(message: impl Into<Cow<'static, str>>) -> TooltipContainer {
    TooltipContainer::new(Tooltip::new_text(message)).position(AttachedPosition::Bottom)
}

/// The row's **⋮ trigger** — the canvas's own affordance, and the menu's discoverable half: the
/// right-click opens the same one, but nothing on screen says so.
///
/// `stop_propagation` because it sits *inside* a pressable row — without it, opening the menu
/// would also toggle the entry's columns (or open the saved query).
///
/// The menu is built lazily, per press: its labels are a snapshot of the moment it opens (see
/// `menu.rs`), so building one per render would be both wasteful and wrong.
fn actions_button(menu: impl Fn() -> Menu + 'static) -> impl IntoElement {
    TooltipContainer::new(Tooltip::new_text("Actions"))
        .position(AttachedPosition::Bottom)
        .child(
            Button::new()
                .flat()
                .width(Size::px(ROW_ACTION))
                .height(Size::px(ROW_ACTION))
                .on_press(move |e: Event<PressEventData>| {
                    e.stop_propagation();
                    ContextMenu::open(menu());
                })
                .child(Icon::new(IconName::Dots).size(15.)),
        )
}

/// The marker a table row carries when Strata owns its data (ED-04).
///
/// **Not cosmetic.** Tables of both origins live in one section under one glyph, and the
/// difference between them is what a drop means: one origin's Drop removes a def and leaves the
/// user's files alone, the other's deletes the only copy of the data. The row is where that has
/// to be legible, because the row is what gets right-clicked.
///
/// A badge rather than a second icon, because the catalog already marks a row that is special in
/// a way that matters this way (a partition column's `PART` chip), and because a new glyph would
/// be a new asset to say what one word says. Toned from the table entity colour rather than a
/// status tone: it is a fact about the table, not a warning about it.
const INTERNAL_BADGE: &str = "INTERNAL";
/// What that marker means, on hover and to a screen reader. `INTERNAL` is the word the app uses
/// everywhere else for this (the router's own `INSERT targets internal tables`), but it does not
/// on its own say the thing the user has to know before dropping the row.
const INTERNAL_TIP: &str = "Strata stores this table's data in the project";

/// How much of a refusal this tooltip will show, and where the rest of it is.
///
/// **The limit lives here because the limit is this surface's.** It used to live in the engine
/// (`register_error`'s 240-character cut), which meant a constraint belonging to a sidebar
/// overlay was applied to the string *every* consumer read — so the Problems drawer, which wraps,
/// and its copy button, which exists to put the message on the clipboard, both handed back a
/// sentence cut mid-clause. The engine's message is whole again; a tooltip that cannot hold it
/// says so and names the surface that can.
///
/// Most refusals are one short sentence Strata wrote ("Reads orders, which is no longer in the
/// catalog.") and are shown entire, with no pointer — the pointer appears exactly when something
/// was left out, which is what makes it worth reading.
const TIP_CHARS: usize = 160;
const TIP_MORE: &str = "\nSee Problems for the full message.";

/// What the row's one trailing status column is saying. A type rather than a built element so
/// it can be compared, and because the row draws it in more than one place.
#[derive(Clone, PartialEq)]
enum StatusMark {
    /// The registration has not answered and the hold has expired (see [`SPINNER_DELAY`]).
    Loading,
    /// The settled verdict — what is wrong with this row, in the words the store recorded.
    Problem(String),
}

impl StatusMark {
    /// The glyph, wearing its message as a tooltip *and* an a11y label, so the explanation is
    /// never mouse-only. Dropped below like the rest of the app's overlays, so it cannot cover
    /// the row above it in a dense list.
    fn glyph(&self, theme: &CatalogTheme) -> Element {
        match self {
            StatusMark::Loading => tip(LOADING)
                .child(CircularLoader::new().size(STATUS_DOT).a11y_alt(LOADING))
                .into_element(),
            StatusMark::Problem(reason) => {
                let shown = tip_text(reason);
                tip(shown.clone())
                    .child(
                        rect().a11y_alt(shown).child(
                            Icon::new(IconName::Warning)
                                .color(theme.warn_color)
                                .size(STATUS_DOT),
                        ),
                    )
                    .into_element()
            }
        }
    }
}

/// `reason` as much as this tooltip will show — whole when it fits, and otherwise clipped with
/// the pointer to where the rest is (see [`TIP_CHARS`]).
///
/// [`clip`](strata_core::util::clip) is the app's one clipping funnel, so this cannot disagree
/// with the grid or the value tree about where a clipped string stops.
fn tip_text(reason: &str) -> String {
    match clip(reason, TIP_CHARS) {
        Cow::Borrowed(whole) => whole.to_string(),
        Cow::Owned(cut) => format!("{cut}{TIP_MORE}"),
    }
}

/// What each optional item costs the row: its own width plus the one gap it brings with it
/// (`SidebarRow`'s spacing is 8).
const BADGE_SLOT: f32 = 63. + 8.;
const ICON_SLOT: f32 = 14. + 8.;
const STATUS_SLOT: f32 = STATUS_DOT + 8.;
/// What the row can never give up: its padding (8 + 4), the chevron that expands it (11), the ⋮
/// that opens its menu (22), and the two gaps around the name. The chevron and the ⋮ are the
/// pinned tail in `components::toolbar`'s sense — they are how the row is *used*, so a row too
/// narrow to show them is worse than one showing nothing else.
const ROW_PINNED: f32 = 8. + 4. + 11. + ROW_ACTION + (2. * 8.);
/// Advance width of the mono face, as a fraction of its point size.
///
/// The name is [`MonoValue`] — a **monospace** role — so its natural width is exactly its
/// character count times one advance, and that is the whole reason this fold can be arithmetic
/// rather than a measurement: Freya lays the name out at the width it is *given* (it is
/// `Size::flex(1.)`), so nothing downstream can report what it would have wanted. 0.6 em is
/// JetBrains Mono's advance and the standard one for the genre; the point size comes from the
/// theme's own scale rather than this file, so retuning the type scale retunes the fold with it.
const MONO_ADVANCE: f32 = 0.6;

/// Which of the row's optional items survive at a given width.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) struct Folds {
    pub badge: bool,
    pub icon: bool,
    pub status: bool,
}

/// The row's fold plan — `components::toolbar`'s policy, ranked for this row.
///
/// That component's row is `[ leading run (flexible, ellipsizes) ][ items (fold lowest-rank
/// first) ][ pinned tail ]`, and the space it offers the items is the row minus its padding,
/// minus the pinned tail, minus the **leading run's floor**. So items fold while the leading run
/// is still whole, and the leading run ellipsizes only once they have all gone. A catalog row is
/// that shape: the name is the leading run, its floor is its own natural width, the chevron and
/// the ⋮ are pinned, and everything else folds — **in this order, least informative first**:
///
/// 1. the `INTERNAL` badge — a marker the icon's tint repeats, and the only item that is pure
///    reinforcement;
/// 2. the entity icon — decoration once the row's section already says what kind it is;
/// 3. the status glyph — information, not decoration, so it outranks both of the above and goes
///    last of the three.
///
/// The name never sets a floor of its own: below the point where everything has folded it goes
/// on ellipsizing, because a chrome row folds rather than spills (AGENTS.md §3).
pub(super) fn fold_plan(row_width: f32, name_width: f32, internal: bool) -> Folds {
    let needs = |f: Folds| {
        ROW_PINNED
            + name_width
            + if f.badge { BADGE_SLOT } else { 0. }
            + if f.icon { ICON_SLOT } else { 0. }
            + if f.status { STATUS_SLOT } else { 0. }
    };
    let mut folds = Folds {
        badge: internal,
        icon: true,
        status: true,
    };
    if needs(folds) <= row_width {
        return folds;
    }
    folds.badge = false;
    if needs(folds) <= row_width {
        return folds;
    }
    folds.icon = false;
    if needs(folds) <= row_width {
        return folds;
    }
    folds.status = false;
    folds
}

/// The scan the **row itself** must subscribe to, if any — what it mounts a subscriber-only
/// [`ProfileWatch`] for rather than leaving to [`ProfileStatus`] in the status column.
///
/// The one rule that broke: the subscription is what *dispatches* a scan, so it cannot be a
/// function of whether there is room to draw a spinner. Exactly one of the two is mounted —
/// `ProfileStatus` while the column is there, this while it is folded — so the query is never
/// subscribed twice and never subscribed by nobody.
///
/// It returns the scan rather than a `bool` so the **whole** condition lives here: an earlier
/// version took `scanning: bool` and was called with a literal `true`, the "is there a scan"
/// half being done by a `filter` at the call site — which left half its truth table unreachable
/// through the production path, exactly the too-strong-looking guard this was extracted to
/// avoid.
pub(super) fn watched_scan(folds: Folds, scan: Option<ScanId>) -> Option<ScanId> {
    scan.filter(|_| !folds.status)
}

/// What a catalog entry (a table or a view) resolved to for rendering: its columns, its partition
/// columns, and the registration state that produced them.
enum EntryState {
    Loading,
    Ready {
        columns: Vec<strata_model::ColumnInfo>,
        partitions: Vec<(String, String)>,
    },
    Failed,
}

/// One catalog entry — a table or a view — and, when open, its column tree.
#[derive(PartialEq)]
pub struct EntryRow {
    kind: CatalogKind,
    name: String,
    open_entries: State<HashSet<String>>,
    expanded_cols: State<HashSet<String>>,
    theme: CatalogTheme,
}

impl EntryRow {
    pub fn new(
        kind: CatalogKind,
        name: String,
        open_entries: State<HashSet<String>>,
        expanded_cols: State<HashSet<String>>,
        theme: CatalogTheme,
    ) -> Self {
        Self {
            kind,
            name,
            open_entries,
            expanded_cols,
            theme,
        }
    }
}

impl Component for EntryRow {
    #[allow(clippy::too_many_lines)]
    fn render(&self) -> impl IntoElement {
        let channel = match self.kind {
            CatalogKind::View => ProjChan::Views,
            _ => ProjChan::Tables,
        };
        let radio = use_radio::<ProjectState, ProjChan>(channel);
        let actions = use_catalog_actions();
        let tables_radio = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        if self.kind == CatalogKind::View {
            drop(tables_radio.read());
        }

        let scan = radio.read().profile_scan(self.kind, &self.name);

        let resolved = {
            let p = radio.read();
            match self.kind {
                CatalogKind::View => p.views.iter().find(|v| v.def.name == self.name).map(|v| {
                    let state = match &v.reg {
                        Reg::Loading => EntryState::Loading,
                        Reg::Failed(_) => EntryState::Failed,
                        Reg::Ready(info) => EntryState::Ready {
                            columns: info.columns.clone(),
                            partitions: Vec::new(),
                        },
                    };
                    (state, p.view_problem(v), false)
                }),
                _ => p.tables.iter().find(|t| t.def.name == self.name).map(|t| {
                    let state = match &t.reg {
                        Reg::Loading => EntryState::Loading,
                        Reg::Failed(_) => EntryState::Failed,
                        Reg::Ready(meta) => EntryState::Ready {
                            columns: meta.columns.clone(),
                            partitions: t.def.partition_cols.clone(),
                        },
                    };
                    (
                        state,
                        ProjectState::table_problem(t),
                        t.def.origin.is_internal(),
                    )
                }),
            }
        };
        let waiting = matches!(resolved, Some((EntryState::Loading, ..)));
        let problem = resolved.as_ref().and_then(|(_, p, _)| p.clone());

        let waited = use_state(|| false);
        let pending = use_state(|| None::<TaskHandle>);
        use_side_effect_with_deps(&waiting, move |waiting| {
            let mut waited = waited;
            let mut pending = pending;
            if let Some(task) = pending.write().take() {
                task.cancel();
            }
            waited.set_if_modified(false);
            if *waiting {
                pending.set(Some(spawn(async move {
                    Timer::after(SPINNER_DELAY).await;
                    waited.set_if_modified(true);
                })));
            }
        });

        let held = use_state(|| None::<String>);
        use_side_effect_with_deps(&(waiting, problem.clone()), move |(waiting, problem)| {
            if !waiting {
                let mut held = held;
                held.set_if_modified(problem.clone());
            }
        });

        let mut measured = use_state(|| f32::INFINITY);
        let name_width = self.name.chars().count() as f32 * scale().data_value.size * MONO_ADVANCE;

        let Some((state, _, internal)) = resolved else {
            return rect();
        };

        let entry_key = format!("{:?}::{}", self.kind, self.name);
        let is_open = self.open_entries.read().contains(&entry_key);
        let mut open_entries = self.open_entries;
        let toggle_key = entry_key;

        let icon = IconName::for_catalog(self.kind);
        let icon_color = match self.kind {
            CatalogKind::View => self.theme.view_color,
            CatalogKind::Query => self.theme.query_color,
            CatalogKind::Table if internal => self.theme.internal_color,
            CatalogKind::Table => self.theme.table_color,
        };

        let status = match (
            waiting && waited(),
            if waiting {
                held.read().clone()
            } else {
                problem
            },
        ) {
            (true, _) => Some(StatusMark::Loading),
            (false, Some(reason)) => Some(StatusMark::Problem(reason)),
            (false, None) => None,
        };

        let folds = fold_plan(measured(), name_width, internal);

        let build_menu = {
            let kind = self.kind;
            let name = self.name.clone();
            move || match kind {
                CatalogKind::View => view_menu(&actions, name.clone()),
                _ => table_menu(&actions, name.clone()),
            }
        };
        let menu_for_row = build_menu.clone();

        let row = SidebarRow::new()
            .height(ENTRY_HEIGHT)
            .on_press(move |_| {
                let mut set = open_entries.write();
                if !set.insert(toggle_key.clone()) {
                    set.remove(&toggle_key);
                }
            })
            .child(
                Icon::new(if is_open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .color(self.theme.chevron_color)
                .size(11.),
            )
            .maybe_child(
                folds
                    .icon
                    .then(|| Icon::new(icon).color(icon_color).size(14.).into_element()),
            )
            .child(
                MonoValue::new(self.name.clone())
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .maybe_child(folds.badge.then(|| {
                tip(INTERNAL_TIP)
                    .child(rect().a11y_alt(INTERNAL_TIP).child(
                        Badge::tag(INTERNAL_BADGE, self.theme.internal_color).into_element(),
                    ))
                    .into_element()
            }))
            .maybe_child(folds.status.then(|| {
                rect()
                    .width(Size::px(STATUS_DOT))
                    .cross_align(Alignment::Center)
                    .maybe_child(match scan {
                        Some(scan) => Some(
                            ProfileStatus {
                                owner: self.name.clone(),
                                scan,
                                settled: status.clone(),
                                theme: self.theme.clone(),
                                key: DiffKey::None,
                            }
                            .key(scan)
                            .into_element(),
                        ),
                        None => status.as_ref().map(|s| s.glyph(&self.theme)),
                    })
                    .into_element()
            }))
            .child(actions_button(build_menu));

        let body = (is_open)
            .then(|| match &state {
                EntryState::Ready {
                    columns,
                    partitions,
                } => {
                    let expanded = self.expanded_cols.read();
                    let mut rows = Vec::new();
                    flatten_cols(
                        &self.name,
                        &[],
                        0,
                        columns,
                        partitions,
                        &expanded,
                        &mut rows,
                    );
                    drop(expanded);
                    let rail_height = rows.len() as f32 * COLUMN_HEIGHT;
                    Some(
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .margin(Gaps::new(SP_1, 0., SP_3, SP_5))
                            .child(
                                rect()
                                    .width(Size::px(1.))
                                    .height(Size::px(rail_height))
                                    .background(self.theme.rail_fill),
                            )
                            .child(
                                rect()
                                    .width(Size::flex(1.))
                                    .vertical()
                                    .padding(Gaps::new(0., 0., 0., SP_3))
                                    .children(rows.into_iter().map(|r| {
                                        ColumnRow::new(
                                            self.kind,
                                            self.name.clone(),
                                            r,
                                            self.expanded_cols,
                                            self.theme.clone(),
                                        )
                                    })),
                            )
                            .into_element(),
                    )
                }
                _ => None,
            })
            .flatten();

        rect()
            .width(Size::fill())
            .vertical()
            .margin(Gaps::new(0., 0., SP_1, 0.))
            .on_sized(move |e: Event<SizedEventData>| {
                measured.set_if_modified(e.area.width());
            })
            .child(row)
            .maybe_child(watched_scan(folds, scan).map(|scan| {
                ProfileWatch {
                    owner: self.name.clone(),
                    scan,
                    key: DiffKey::None,
                }
                .key(scan)
                .into_element()
            }))
            .maybe_child(body)
    }
}

/// The row's **profiling glyph** (P3-09) — a spinner for exactly as long as this entry's scan is
/// in flight, and the settled verdict otherwise.
///
/// Its own component because it **subscribes** to the scan, and a hook cannot be conditional: the
/// row mounts it only when there is a request to watch, which is also what keeps a sidebar full of
/// tables from subscribing (and, with an un-run entry, *dispatching*) a scan nobody asked for.
/// Two subscribers of one request — this and the inspector's zone — attach to the same execution
/// rather than starting a second, since freya-query counts executions in flight.
#[derive(PartialEq)]
struct ProfileStatus {
    owner: String,
    scan: ScanId,
    /// What the column says when **no scan is running** — so the one slot is never empty while
    /// the row has something to report, and never holds two glyphs at once.
    settled: Option<StatusMark>,
    theme: CatalogTheme,
    key: DiffKey,
}

impl KeyExt for ProfileStatus {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ProfileStatus {
    fn render(&self) -> impl IntoElement {
        match scan_running(&self.owner, self.scan) {
            true => tip(PROFILING)
                .child(CircularLoader::new().size(STATUS_DOT).a11y_alt(PROFILING))
                .into_element(),
            false => match &self.settled {
                Some(mark) => mark.glyph(&self.theme),
                None => rect().into_element(),
            },
        }
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The same subscription with **no glyph** — what the row mounts in place of [`ProfileStatus`]
/// when the fold plan has taken the status column away.
///
/// It exists because the subscription is what makes the scan *run*: a Profile asked for while the
/// sidebar is narrow would otherwise mount nothing and dispatch nothing, and the user would have
/// accepted the cost confirm for no work at all. Exactly one of the two is mounted at a time, so
/// the query is never subscribed twice.
#[derive(PartialEq)]
struct ProfileWatch {
    owner: String,
    scan: ScanId,
    key: DiffKey,
}

impl KeyExt for ProfileWatch {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ProfileWatch {
    fn render(&self) -> impl IntoElement {
        let _running = scan_running(&self.owner, self.scan);
        rect()
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// Subscribe to `owner`'s scan and answer whether it is executing right now — the one hook both
/// [`ProfileStatus`] and [`ProfileWatch`] are built around, so the two cannot subscribe
/// differently.
fn scan_running(owner: &str, scan: ScanId) -> bool {
    let engine = use_consume::<EngineCtx>();
    let query = use_profile(&engine, owner, scan);
    let reader = query.read();
    let running = matches!(
        &*reader.state(),
        QueryStateData::Pending | QueryStateData::Loading { .. }
    );
    drop(reader);
    running
}

/// One column row — a top-level column or an expanded nested field. Selecting it is what drives
/// the inspector; the chevron (nested columns only) expands in place without selecting.
#[derive(PartialEq)]
struct ColumnRow {
    owner_kind: CatalogKind,
    owner: String,
    row: ColRow,
    expanded_cols: State<HashSet<String>>,
    theme: CatalogTheme,
}

impl ColumnRow {
    fn new(
        owner_kind: CatalogKind,
        owner: String,
        row: ColRow,
        expanded_cols: State<HashSet<String>>,
        theme: CatalogTheme,
    ) -> Self {
        Self {
            owner_kind,
            owner,
            row,
            expanded_cols,
            theme,
        }
    }
}

impl Component for ColumnRow {
    fn render(&self) -> impl IntoElement {
        let mut selection = use_catalog_selection();
        let mut layout = use_radio::<SessionState, Chan>(Chan::Layout);

        let col = ColRef {
            kind: self.owner_kind,
            owner: self.owner.clone(),
            path: self.row.path.clone(),
        };
        let selected = selection
            .read()
            .as_ref()
            .is_some_and(|s| s.owner == col.owner && s.kind == col.kind && s.path == col.path);

        let swatch = kind_color(self.row.kind, &type_palette());

        let mut expanded_cols = self.expanded_cols;
        let expand_key = self.row.key.clone();
        let chevron = self.row.has_children.then(|| {
            rect()
                .on_press(move |e: Event<PressEventData>| {
                    e.stop_propagation();
                    let mut set = expanded_cols.write();
                    if !set.insert(expand_key.clone()) {
                        set.remove(&expand_key);
                    }
                })
                .child(
                    Icon::new(if self.row.is_expanded {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .color(self.theme.chevron_color)
                    .size(11.),
                )
                .into_element()
        });

        let part_chip = self.row.is_part.then(|| {
            Badge::tag("PART", self.theme.part_color)
                .background(self.theme.part_background)
                .into_element()
        });

        SidebarRow::new()
            .height(COLUMN_HEIGHT)
            .padding(Gaps::new(0., SP_3, 0., SP_3))
            .selected(selected)
            .on_press(move |_| {
                selection.set(Some(col.clone()));
                layout
                    .write_channel(Chan::Layout)
                    .open_right_pane(RightPane::Inspector);
            })
            .child(rect().width(Size::px(self.row.depth as f32 * DEPTH_INDENT)))
            .child(
                rect()
                    .width(Size::px(CHEVRON_SLOT))
                    .cross_align(Alignment::Center)
                    .maybe_child(chevron),
            )
            .child(Dot::new(swatch).size(6.).square())
            .child(
                MonoValue::new(self.row.name.clone())
                    .color(self.theme.column_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(part_chip)
            .child(Meta::new(self.row.dtype.clone()).color(swatch))
    }
}

/// One saved query. Addressed by its stable `id` — the name is only a label — so a rename can't
/// dangle whatever holds it.
///
/// Pressing the row opens it in a tab, which is the canvas's own `title="Open in a new tab"`;
/// its menu (right-click or ⋮) adds Rename and Delete. Rename is **inline**, in the row itself,
/// exactly like the tab strip's: the menu item only flips this row's `renaming` flag and the row
/// reacts in its own scope, so the rename survives the menu closing.
#[derive(PartialEq)]
pub struct SavedQueryRow {
    id: Uuid,
    name: String,
    theme: CatalogTheme,
}

impl SavedQueryRow {
    pub fn new(id: Uuid, name: String, theme: CatalogTheme) -> Self {
        Self { id, name, theme }
    }
}

impl Component for SavedQueryRow {
    fn render(&self) -> impl IntoElement {
        let actions = use_catalog_actions();
        let id = self.id;
        let renaming = use_state(|| false);

        if *renaming.read() {
            return QueryRename {
                id,
                name: self.name.clone(),
                renaming,
                theme: self.theme.clone(),
            }
            .into_element();
        }

        let build_menu = {
            let actions = actions.clone();
            let name = self.name.clone();
            move || query_menu(&actions, id, name.clone(), renaming)
        };
        let menu_for_row = build_menu.clone();

        SidebarRow::new()
            .height(ENTRY_HEIGHT)
            .on_press(move |_| open_saved_query(&actions, id))
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .child(
                Icon::new(IconName::Brackets)
                    .color(self.theme.query_color)
                    .size(14.),
            )
            .child(
                Body::new(self.name.clone())
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .child(actions_button(build_menu))
            .into_element()
    }
}

/// The saved-query row **while it is being renamed**: the same box, with an input in place of
/// the label. Its own component so it can own the commit / cancel listeners — what it replaces
/// is a `SidebarRow`, whose whole job is to be pressable, and a row being renamed must not be.
///
/// Enter commits (the input's `on_submit`); Escape cancels — consumed, so an Esc that ends a
/// rename doesn't also cancel a running query further down the dismiss chain; a press anywhere
/// outside the row commits, like a blur. The tab strip's rename behaves identically, and for the
/// same reasons.
#[derive(PartialEq)]
struct QueryRename {
    id: Uuid,
    name: String,
    renaming: State<bool>,
    theme: CatalogTheme,
}

impl Component for QueryRename {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let mut renaming = self.renaming;
        let mut draft = use_state(String::new);
        let mut area = use_state(|| None::<Area>);
        let a11y = use_a11y();
        let config = use_config_station();
        let actions = use_catalog_actions();

        let seed = self.name.clone();
        use_hook(move || draft.set(seed.clone()));

        let outside_actions = actions.clone();

        rect()
            .width(Size::fill())
            .height(Size::px(ENTRY_HEIGHT))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .padding(Gaps::new(0., SP_2, 0., SP_3))
            .margin(Gaps::new(0., 0., SP_1, 0.))
            .on_sized(move |e: Event<SizedEventData>| area.set(Some(e.area)))
            .on_global_key_down(on_command(config, Command::Cancel, move || {
                renaming.set(false);
                true
            }))
            .on_global_pointer_press(move |e: Event<PointerEventData>| {
                let p = e.data().global_location();
                if let Some(a) = *area.peek() {
                    let (px, py) = (p.x as f32, p.y as f32);
                    let outside = px < a.origin.x
                        || px > a.origin.x + a.size.width
                        || py < a.origin.y
                        || py > a.origin.y + a.size.height;
                    if outside {
                        let name = draft.peek().clone();
                        rename_saved_query(&outside_actions, id, &name);
                        renaming.set(false);
                    }
                }
            })
            .child(
                Icon::new(IconName::Brackets)
                    .color(self.theme.query_color)
                    .size(14.),
            )
            .child(InputTypography::body(
                Input::new(draft)
                    .a11y_id(a11y)
                    .flat()
                    .compact()
                    .auto_focus(true)
                    .select_all_on_init(true)
                    .width(Size::flex(1.))
                    .on_submit(move |value: String| {
                        rename_saved_query(&actions, id, &value);
                        renaming.set(false);
                    }),
            ))
    }
}
