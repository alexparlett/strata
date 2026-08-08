//! The catalog's rows: a table / view **entry** that expands to its column tree, one **column**
//! row (nested or not), and a **saved-query** row.
//!
//! Tables and views share `EntryRow` and the column list below it, because a view's columns *are*
//! columns — clickable, selectable, expandable when nested. In the Dioxus sidebar these were two
//! copies that differed only by omission (view rows had no click handler at all, so clicking one
//! silently did nothing), which is exactly what a second copy of a list buys you.

use std::collections::HashSet;
use std::time::Duration;

use async_io::Timer;
use freya::components::CircularLoader;
use freya::prelude::*;
use freya::query::QueryStateData;
use freya::radio::use_radio;
use strata_model::{CatalogKind, ColRef};
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
use crate::components::sidebar_row::SidebarRow;
use crate::components::type_palette::{kind_color, type_palette};
use crate::components::typography::{scale, Body, InputTypography, Meta, MonoValue};
use crate::components::PROGRESS_HOLD;
use crate::keymap::on_command;
use crate::state::use_config_station;
use strata_core::config::Command;

/// Row heights + the column block's indent, from the design canvas.
const ENTRY_HEIGHT: f32 = 30.;
const COLUMN_HEIGHT: f32 = 25.;
/// Indent added per nesting level of a struct/list/map column.
const DEPTH_INDENT: f32 = 12.;
/// The chevron gutter on a column row — reserved whether or not the column has one, so names
/// line up.
const CHEVRON_SLOT: f32 = 11.;
/// The entry row's trailing **status glyph** — spinner or validity triangle, one slot, one size.
const STATUS_SIZE: f32 = 12.;
/// What the spinner says on hover (and to a screen reader).
const LOADING: &str = "Loading…";
/// How long a row must stay unanswered before it is worth spinning about. Most registrations land
/// well inside this, so the usual project open is a catalog that simply appears — no flicker of
/// spinners on the way in.
///
/// The design system's shared hold (`components::PROGRESS_HOLD`), because the inspector's re-scan
/// row serves the same one — see there for the half of the rule this row already had: a hold needs
/// something to hold *onto*, and what this slot holds is the last verdict it showed.
const SPINNER_DELAY: Duration = PROGRESS_HOLD;

/// The trailing ⋮ actions button — the canvas's 22×22. `pub(super)` because it is also the
/// column the interaction tests measure the rest of the trailing run against, and a second copy
/// of the number there is a second thing to keep in step.
pub(super) const ACTIONS_SIZE: f32 = 22.;
/// What the **profiling** spinner says — its own words, because the registration spinner beside
/// it means something else entirely (a scan is minutes of work the user asked for; a
/// registration is a metadata read they didn't).
const PROFILING: &str = "Profiling…";

/// A status glyph wearing its message as a tooltip. Dropped below, like the rest of the app's
/// overlays, so it can't cover the row above it in a dense list.
fn tip(message: impl Into<std::borrow::Cow<'static, str>>) -> TooltipContainer {
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
                .width(Size::px(ACTIONS_SIZE))
                .height(Size::px(ACTIONS_SIZE))
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

/// What the row's one trailing status column is saying. A type rather than a built element so
/// it can be compared — [`ProfileStatus`] holds it as a prop, and a component's props must be
/// `PartialEq` for the tree to diff them.
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
            // Named, not bare: P3-09 puts a *profiling* spinner in this same slot, and two
            // spinners meaning different things need to be tellable apart.
            StatusMark::Loading => tip(LOADING)
                .child(CircularLoader::new().size(STATUS_SIZE).a11y_alt(LOADING))
                .into_element(),
            StatusMark::Problem(reason) => tip(reason.clone())
                .child(
                    rect().a11y_alt(reason.clone()).child(
                        Icon::new(IconName::Warning)
                            .color(theme.warn_color)
                            .size(STATUS_SIZE),
                    ),
                )
                .into_element(),
        }
    }
}

/// The `INTERNAL` badge's laid-out width plus the row gap it brings with it — `Eyebrow` at 10
/// over the badge's 4px side padding, measured rather than guessed
/// (`the_internal_badge_folds_before_the_name_truncates`).
const BADGE_SLOT: f32 = 63. + 8.;
/// Everything on the row that is neither the name nor the badge: the row's padding (8 + 4), the
/// chevron (11), the entity icon (14), the status column ([`STATUS_SIZE`]), the ⋮
/// ([`ACTIONS_SIZE`]) and the four gaps between those five items.
const ROW_FIXED: f32 = 8. + 4. + 11. + 14. + STATUS_SIZE + ACTIONS_SIZE + (4. * 8.);
/// Advance width of the mono face, as a fraction of its point size.
///
/// The name is [`MonoValue`] — a **monospace** role — so its natural width is exactly its
/// character count times one advance, and that is the whole reason this fold can be arithmetic
/// rather than a measurement: Freya lays the name out at the width it is *given* (it is
/// `Size::flex(1.)`), so nothing downstream can report what it would have wanted. 0.6 em is
/// JetBrains Mono's advance and the standard one for the genre; the point size comes from the
/// theme's own scale rather than this file, so retuning the type scale retunes the fold with it.
const MONO_ADVANCE: f32 = 0.6;

/// Whether the row folds its `INTERNAL` badge away, given the row's width and what the name
/// would take unconstrained.
///
/// **This is `components::toolbar`'s policy, not a rule of its own** (AGENTS.md §3: one fold
/// policy for every row). That row is `[ leading run (flexible, ellipsizes) ][ items (fold
/// tail-first) ][ pinned tail ]`, and the space it offers the items is the row minus its padding,
/// minus the pinned tail, **minus the leading run's floor** — so an item folds while the leading
/// run is still whole, and the leading run goes on ellipsizing afterwards. A catalog row is that
/// shape: the name is the leading run, the badge is the one foldable item, and the status column
/// and ⋮ are the pinned tail that never folds.
///
/// What this row contributes is its leading floor: **the name's own natural width**, so the badge
/// goes the moment it would cost a character rather than at some shared constant. That the name
/// can be measured by arithmetic at all is [`MONO_ADVANCE`]'s doing, and stating a width is what
/// every `ToolbarItem` does anyway.
///
/// Two earlier versions of this got it wrong in opposite directions, and both are worth not
/// repeating: a flat 80px floor let a long name ellipsize while the badge sat there, and a
/// "the name does not fit either way, so keep the badge" case rendered a badge beside an empty
/// name. There is no width at which keeping the badge is worth a character of the name — the
/// icon's own tint already says the row is internal, and the icon never folds.
pub(super) fn folds_badge(row_width: f32, name_width: f32) -> bool {
    name_width > row_width - ROW_FIXED - BADGE_SLOT
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
    fn render(&self) -> impl IntoElement {
        // Subscribe on this entry's own section channel, so the row flips `Loading → Ready`
        // in place as its registration answer lands.
        let channel = match self.kind {
            CatalogKind::View => ProjChan::Views,
            _ => ProjChan::Tables,
        };
        let radio = use_radio::<ProjectState, ProjChan>(channel);
        let actions = use_catalog_actions();
        // A view's validity is derived against the **live table rows**, and a table failing — or
        // being dropped — never touches the views channel. So a view row listens on TABLES too:
        // one store, two antennas, and this read *is* the subscription (the value itself is read
        // once, below, through `radio`).
        let tables_radio = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        if self.kind == CatalogKind::View {
            drop(tables_radio.read());
        }

        // Whether this row has a scan in flight to spin about (P3-09) — read on the same channel
        // as everything else here, since asking for one writes the row.
        let scan = radio.read().profile_scan(self.kind, &self.name);

        // Resolve this row's state and its validity out of the store, cloning what we render, so
        // the read guard drops before any element is built.
        let resolved = {
            let p = radio.read();
            match self.kind {
                CatalogKind::View => p.views.iter().find(|v| v.def.name == self.name).map(|v| {
                    let state = match &v.reg {
                        Reg::Loading => EntryState::Loading,
                        Reg::Failed(_) => EntryState::Failed,
                        Reg::Ready(info) => EntryState::Ready {
                            columns: info.columns.clone(),
                            // A view has no partition columns.
                            partitions: Vec::new(),
                        },
                    };
                    // A view has no origin: its data is whatever its query reads.
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
                    // Off the **def**, so the marker is there whatever registration answered —
                    // a `Reg::Failed` internal row is exactly the one whose origin the reader
                    // most needs (its data is not in this copy of the project).
                    (
                        state,
                        ProjectState::table_problem(t),
                        t.def.origin.is_internal(),
                    )
                }),
            }
        };
        // **The status slot holds still.** A *settled* answer always applies at once: a row that
        // comes back clean drops its triangle the moment the answer lands, and one that comes back
        // broken says why. What is deliberately not immediate is the gap in between — while a row
        // is unanswered the slot keeps whatever it last showed, for `SPINNER_DELAY`. Registering is
        // metadata-only (`register_external`: infer the schema, list the files), so the usual pass
        // is far inside that window and nothing in the pane moves at all; without the hold, ↻ on a
        // broken row would blink its triangle off and back on, and the empty slot in between would
        // read as "fine" — a claim the row cannot make while it has no answer. Past the hold the
        // wait is news in its own right (a partitioned tree of thousands of files, or an object
        // store) and the spinner takes the slot.
        //
        // Both bits of state are armed here, above the early return, so hook order can't depend on
        // the row still being in the store.
        let waiting = matches!(resolved, Some((EntryState::Loading, ..)));
        let problem = resolved.as_ref().and_then(|(_, p, _)| p.clone());

        // Whether the wait has outlasted the hold. Re-armed from zero on every entry into (and exit
        // from) the wait — but *not* on a re-scan of a row that was already waiting, whose wait
        // never stopped and whose spinner therefore shouldn't blink.
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

        // The verdict to keep showing through the gap: the last one that actually settled.
        let held = use_state(|| None::<String>);
        use_side_effect_with_deps(&(waiting, problem.clone()), move |(waiting, problem)| {
            if !waiting {
                let mut held = held;
                held.set_if_modified(problem.clone());
            }
        });

        // The row was dropped from the store between the section's read and ours.
        let Some((state, _, internal)) = resolved else {
            return rect();
        };

        let entry_key = format!("{:?}::{}", self.kind, self.name);
        let is_open = self.open_entries.read().contains(&entry_key);
        let mut open_entries = self.open_entries;
        let toggle_key = entry_key.clone();

        // The glyph is the shared mapping (the palette lists the same things); the tint is this
        // surface's own — and a table Strata owns takes its own entity colour, because the
        // catalog shows both origins in one section under one glyph and the icon is the mark
        // that never folds when the pane narrows.
        let icon = IconName::for_catalog(self.kind);
        let icon_color = match self.kind {
            CatalogKind::View => self.theme.view_color,
            CatalogKind::Query => self.theme.query_color,
            CatalogKind::Table if internal => self.theme.internal_color,
            CatalogKind::Table => self.theme.table_color,
        };

        // What the row's one status column is saying, with the words only on hover. A settled row
        // is clean, per the design. No status *text*: "failed" said strictly less than the reason
        // the triangle carries, and it cost the name half the row.
        let status = match (
            waiting && waited(),
            if waiting {
                held.read().clone()
            } else {
                problem
            },
        ) {
            // The wait has outlasted the hold, so it is now the thing worth saying.
            (true, _) => Some(StatusMark::Loading),
            // The settled verdict, or — mid-gap — the one still being held.
            (false, Some(reason)) => Some(StatusMark::Problem(reason)),
            (false, None) => None,
        };

        // **The badge folds before the name truncates.** Freya has no container query, so the row
        // measures itself and the next render acts on it — one `State` per row, written only when
        // the answer actually flips, so a resize does not re-render rows whose answer has not
        // changed. Seeded showing: the first paint keeps the badge, and a row too narrow for it
        // drops it on the frame after, which is the right way round — a marker that flashes away
        // is better than a name that arrives clipped.
        let mut folded = use_state(|| false);
        let fold_badge = internal && folded();
        // What this name would take if nothing constrained it. Mono, so it is arithmetic — see
        // [`MONO_ADVANCE`].
        let name_width = self.name.chars().count() as f32 * scale().data_value.size * MONO_ADVANCE;

        // One menu, two triggers (right-click the row, or press its ⋮) — a fresh snapshot each
        // time it is opened.
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
            .child(Icon::new(icon).color(icon_color).size(14.))
            // The name absorbs the slack and truncates, so the status run stays visible however
            // long the table is called.
            .child(
                MonoValue::new(self.name.clone())
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            // After the name and before the status column, so it reads as part of what the
            // row *is* rather than as something that happened to it — and **folded away
            // before the name truncates**, because a name the reader cannot finish is a worse
            // loss than a marker the icon's own tint already carries.
            .maybe_child((internal && !fold_badge).then(|| {
                tip(INTERNAL_TIP)
                    .child(rect().a11y_alt(INTERNAL_TIP).child(
                        Badge::tag(INTERNAL_BADGE, self.theme.internal_color).into_element(),
                    ))
                    .into_element()
            }))
            // **One** trailing status column, fixed width, always present. A scan is asked
            // for from *here* (the row's menu) and can run for minutes with the inspector
            // closed, so the row is the only thing that can say it is happening — but the
            // spinner and the validity triangle are one question asked twice, never two
            // marks at once, and they were two children before this. That cost the badge
            // its position: a row that had ever been profiled kept a mounted, idle
            // `ProfileStatus` in the run, so everything left of it sat 20px further in than
            // on a row that had not. One reserved slot instead, so every row's marks line up
            // in a column whatever any of them is doing.
            .child(
                rect()
                    .width(Size::px(STATUS_SIZE))
                    .cross_align(Alignment::Center)
                    .maybe_child(match scan {
                        // Mounted for the subscription as much as for the glyph — see
                        // `ProfileStatus` — and it renders `status` whenever no scan is
                        // running, so the slot never holds two things and never holds none
                        // it could have filled.
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
                    }),
            )
            .child(actions_button(build_menu));

        // The column block: an indented run hung off a hairline rail, exactly the canvas's
        // `border-left` treatment.
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
                    // The rail is a 1px sibling column, drawn to the exact stack height — every
                    // column row is `COLUMN_HEIGHT`, so the rule ends where the rows do.
                    // Not `Size::fill()`: the wrapper hugs its content, so `fill` would resolve
                    // against the scroll viewport and stretch the whole block to its height.
                    let rail_height = rows.len() as f32 * COLUMN_HEIGHT;
                    Some(
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .margin(Gaps::new(2., 0., 8., 16.))
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
                                    .padding(Gaps::new(0., 0., 0., 8.))
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
                // Nothing to list yet (or ever): the row's own status label already says why.
                _ => None,
            })
            .flatten();

        rect()
            .width(Size::fill())
            .vertical()
            .margin(Gaps::new(0., 0., 2., 0.))
            // The measurement behind the badge fold. `set_if_modified` on the *answer*, not on
            // the width: a drag across the pane's whole range writes at most twice.
            .on_sized(move |e: Event<SizedEventData>| {
                folded.set_if_modified(folds_badge(e.area.width(), name_width));
            })
            .child(row)
            .maybe_child(body)
    }
}

/// The row's **profiling** glyph (P3-09) — a spinner for exactly as long as this entry's scan is
/// in flight, and nothing at all once it settles.
///
/// Its own component because it **subscribes** to the scan, and a hook cannot be conditional: the
/// row mounts this only when there is a request to watch, which is also what keeps a sidebar full
/// of tables from subscribing (and, with an un-run entry, *dispatching*) a scan nobody asked for.
/// Two subscribers of one request — this and the inspector's zone — attach to the same execution
/// rather than starting a second, since freya-query counts executions in flight.
#[derive(PartialEq)]
struct ProfileStatus {
    owner: String,
    scan: ScanId,
    /// What the row's status column says when **no scan is running** — so the one slot is never
    /// empty while the row has something to report, and never holds two glyphs at once.
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
        let engine = use_consume::<EngineCtx>();
        let query = use_profile(&engine, &self.owner, self.scan);
        let reader = query.read();
        let running = matches!(
            &*reader.state(),
            QueryStateData::Pending | QueryStateData::Loading { .. }
        );
        drop(reader);

        // No delay hold, unlike the registration spinner: a scan is *known* to be slow — it is
        // the thing the user was warned about — so there is nothing to avoid flickering over, and
        // starting one has to look like it started. It outranks the settled mark while it runs,
        // because "this is being recomputed" is the newer fact about the same row.
        match running {
            true => tip(PROFILING)
                .child(CircularLoader::new().size(STATUS_SIZE).a11y_alt(PROFILING))
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
        // Compare the whole path, not the leaf name: by name alone, selecting `city` lit up every
        // `city` at any depth in the entry.
        let selected = selection
            .read()
            .as_ref()
            .is_some_and(|s| s.owner == col.owner && s.kind == col.kind && s.path == col.path);

        let swatch = kind_color(self.row.kind, &type_palette());

        let mut expanded_cols = self.expanded_cols;
        let expand_key = self.row.key.clone();
        // The chevron sits inside the pressable row, so its own press must stop there — otherwise
        // expanding a struct would also select it.
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
            .padding(Gaps::new(0., 8., 0., 8.))
            .selected(selected)
            .on_press(move |_| {
                selection.set(Some(col.clone()));
                // Selecting a column is also how the inspector is reopened once collapsed.
                layout.write_channel(Chan::Layout).open_inspector();
            })
            // Indent by depth, then a fixed chevron gutter so names align whether or not the
            // column is expandable.
            .child(rect().width(Size::px(self.row.depth as f32 * DEPTH_INDENT)))
            .child(
                rect()
                    .width(Size::px(CHEVRON_SLOT))
                    .cross_align(Alignment::Center)
                    .maybe_child(chevron),
            )
            .child(Dot::new(swatch).size(6.).square())
            // The name takes the slack and truncates; the PART chip and the dtype keep their
            // intrinsic width, so the type is always readable at the right edge.
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
        // This row's own inline-rename state — local, never shared. Flipped by the menu item;
        // the rename shell below owns everything that follows from it.
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
            // Pressing the row opens it — the canvas's own `title="Open in a new tab"` — through
            // the same action the menu's item runs, not a second copy of it.
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

        // Seed the draft with the name being replaced. `use_hook`, not an effect: this component
        // only exists while the row is being renamed, so mounting *is* the moment to seed — and
        // re-seeding on any later render would fight the typing.
        let seed = self.name.clone();
        use_hook(move || draft.set(seed.clone()));

        let outside_actions = actions.clone();

        rect()
            .width(Size::fill())
            .height(Size::px(ENTRY_HEIGHT))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(8.)
            // The row's own geometry (see `SidebarRow`), so committing doesn't shift the list.
            .padding(Gaps::new(0., 4., 0., 8.))
            .margin(Gaps::new(0., 0., 2., 0.))
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
                    // The name arrives selected, so the first keystroke replaces it — a rename
                    // opens over the old label, it doesn't invite you to type in front of it.
                    .select_all_on_init(true)
                    .width(Size::flex(1.))
                    .on_submit(move |value: String| {
                        rename_saved_query(&actions, id, &value);
                        renaming.set(false);
                    }),
            ))
    }
}
