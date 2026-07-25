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
use freya::radio::use_radio;
use strata_model::{CatalogKind, ColRef};
use uuid::Uuid;

use super::columns::{flatten_cols, ColRow};
use super::CatalogTheme;
use crate::apps::project::state::{
    use_catalog_selection, Chan, ProjChan, ProjectState, Reg, SessionState,
};
use crate::components::badge::Badge;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::sidebar_row::SidebarRow;
use crate::components::type_palette::{kind_color, type_palette};
use crate::components::typography::{Body, Meta, MonoValue};

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
const SPINNER_DELAY: Duration = Duration::from_millis(400);

/// A status glyph wearing its message as a tooltip. Dropped below, like the rest of the app's
/// overlays, so it can't cover the row above it in a dense list.
fn tip(message: impl Into<std::borrow::Cow<'static, str>>) -> TooltipContainer {
    TooltipContainer::new(Tooltip::new(message)).position(AttachedPosition::Bottom)
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
        // A view's validity is derived against the **live table rows**, and a table failing — or
        // being dropped — never touches the views channel. So a view row listens on TABLES too:
        // one store, two antennas, and this read *is* the subscription (the value itself is read
        // once, below, through `radio`).
        let tables_radio = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        if self.kind == CatalogKind::View {
            drop(tables_radio.read());
        }

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
                    (state, p.view_problem(v))
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
                    (state, ProjectState::table_problem(t))
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
        let waiting = matches!(resolved, Some((EntryState::Loading, _)));
        let problem = resolved.as_ref().and_then(|(_, p)| p.clone());

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
        let Some((state, _)) = resolved else {
            return rect();
        };

        let entry_key = format!("{:?}::{}", self.kind, self.name);
        let is_open = self.open_entries.read().contains(&entry_key);
        let mut open_entries = self.open_entries;
        let toggle_key = entry_key.clone();

        let (icon, icon_color) = match self.kind {
            CatalogKind::View => (IconName::Eye, self.theme.view_color),
            CatalogKind::Query => (IconName::Brackets, self.theme.query_color),
            CatalogKind::Table => (IconName::Database, self.theme.table_color),
        };

        // One trailing **status slot** and at most one glyph in it, with the words only on hover. A
        // settled row is clean, per the design. No status *text*: "failed" said strictly less than
        // the reason the triangle carries, and it cost the name half the row. Each glyph declares
        // its message as an **a11y label** too, so the explanation isn't mouse-only.
        let status = match (
            waiting && waited(),
            if waiting {
                held.read().clone()
            } else {
                problem
            },
        ) {
            // The wait has outlasted the hold, so it is now the thing worth saying. Named, not
            // bare: P3-09 puts a *profiling* spinner in reach of this same row, and two spinners
            // meaning different things need to be tellable apart.
            (true, _) => {
                Some(tip(LOADING).child(CircularLoader::new().size(STATUS_SIZE).a11y_alt(LOADING)))
            }
            // The settled verdict, or — mid-gap — the one still being held.
            (false, Some(reason)) => Some(
                tip(reason.clone()).child(
                    rect().a11y_alt(reason).child(
                        Icon::new(IconName::Warning)
                            .color(self.theme.warn_color)
                            .size(STATUS_SIZE),
                    ),
                ),
            ),
            (false, None) => None,
        };

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
            .maybe_child(status.map(|s| s.into_element()));

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
                    // The rail is a 1px sibling column rather than a left border: `BorderWidth`
                    // takes a single `f32` (all four edges), so a one-sided rule has to be drawn.
                    // Its height is the exact stack height — every column row is `COLUMN_HEIGHT`.
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
                                        .into()
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
            .child(row)
            .maybe_child(body)
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
#[derive(PartialEq)]
pub struct SavedQueryRow {
    #[allow(dead_code)] // Consumed by open / rename / delete (P3-06); identity is already correct.
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
        SidebarRow::new()
            .height(ENTRY_HEIGHT)
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
    }
}
