//! The workspace's rows: a table / view **entry** that expands to its column tree, one **column**
//! row (nested or not), and a **saved-query** row.
//!
//! Tables and views share `EntryRow` and the column list below it, because a view's columns *are*
//! columns — clickable, selectable, expandable when nested. In the Dioxus sidebar these were two
//! copies that differed only by omission (view rows had no click handler at all, so clicking one
//! silently did nothing), which is exactly what a second copy of a list buys you.

use freya::components::Disclosure;
use freya::prelude::*;
use freya::query::QueryStateData;
use freya::radio::use_radio;
use strata_model::{CatalogKind, ColRef, ColumnInfo, RightPane};
use uuid::Uuid;

use super::columns::{flatten_cols, ColRow};
use super::menu::{
    open_saved_query, query_menu, rename_saved_query, table_menu, use_catalog_actions, view_menu,
};
use super::row::{
    actions_button, fold_plan, name_width, tip, use_status, Folds, Row, StatusMark, ICON_SLOT,
    INDENT, ROW_HEIGHT,
};
use super::{CatalogTheme, TreeCtx};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{use_profile, ScanId};
use crate::apps::project::state::{
    use_catalog_selection, Chan, ProjChan, ProjectState, Reg, SessionState,
};
use crate::components::badge::Badge;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_2, SP_3, STATUS_DOT};
use crate::components::type_palette::{kind_color, type_palette};
use crate::components::typography::{Body, InputTypography, Meta, MonoValue};
use crate::keymap::on_command;
use crate::state::use_config_station;
use strata_core::config::Command;

/// How deep a workspace entry sits: the workspace node, then its group.
const ENTRY_DEPTH: usize = 2;
/// What the **profiling** spinner says — its own words, because the registration spinner beside
/// it means something else entirely (a scan is minutes of work the user asked for; a
/// registration is a metadata read they didn't).
const PROFILING: &str = "Profiling…";

/// The marker a table row carries when Strata owns its data (ED-04).
///
/// **Not cosmetic.** Tables of both origins live in one group under one glyph, and the
/// difference between them is what a drop means: one origin's Drop removes a def and leaves the
/// user's files alone, the other's deletes the only copy of the data. The row is where that has
/// to be legible, because the row is what gets right-clicked.
const INTERNAL_BADGE: &str = "INTERNAL";
/// What that marker means, on hover and to a screen reader.
const INTERNAL_TIP: &str = "Strata stores this table's data in the project";

/// What a catalog entry (a table or a view) resolved to for rendering.
///
/// **`columns` is empty on a row that is closed**, not merely on one with nothing to show: the
/// only thing a closed row asks of them is `expandable`, and cloning a whole `ColumnInfo` tree per
/// render to answer one `bool` is what a sidebar full of tables pays for every chevron press
/// anywhere in the pane.
///
/// A struct rather than the three-variant enum this was: `Failed` was constructed twice and
/// matched nowhere — every reader treated it as "not waiting, no columns", which is what it is.
struct EntryState {
    waiting: bool,
    expandable: bool,
    columns: Vec<ColumnInfo>,
    partitions: Vec<(String, String)>,
}

/// One workspace entry — a table or a view — and, when open, its column tree.
#[derive(PartialEq)]
pub struct EntryRow {
    kind: CatalogKind,
    name: String,
    /// This row's node path, `ws/tables/orders` — the expansion key, the prefix its columns are
    /// keyed under, and what a jump from an object-store link addresses it by.
    path: String,
    theme: CatalogTheme,
    key: DiffKey,
}

impl EntryRow {
    pub fn new(kind: CatalogKind, name: String, group: &str, theme: CatalogTheme) -> Self {
        let path = format!("{group}/{name}");
        Self {
            kind,
            name,
            path,
            theme,
            key: DiffKey::None,
        }
    }
}

impl KeyExt for EntryRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for EntryRow {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
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

        let is_open = tree.is_open(&self.path);
        let resolved = {
            let p = radio.read();
            match self.kind {
                CatalogKind::View => p.views.iter().find(|v| v.def.name == self.name).map(|v| {
                    let columns = v.reg.ready().map(|info| info.columns.as_slice());
                    (
                        EntryState {
                            waiting: matches!(v.reg, Reg::Loading),
                            expandable: columns.is_some_and(|c| !c.is_empty()),
                            columns: match is_open {
                                true => columns.unwrap_or_default().to_vec(),
                                false => Vec::new(),
                            },
                            partitions: Vec::new(),
                        },
                        p.view_problem(v),
                        false,
                    )
                }),
                _ => p.tables.iter().find(|t| t.def.name == self.name).map(|t| {
                    let columns = t.reg.ready().map(|meta| meta.columns.as_slice());
                    (
                        EntryState {
                            waiting: matches!(t.reg, Reg::Loading),
                            expandable: columns.is_some_and(|c| !c.is_empty()),
                            columns: match is_open {
                                true => columns.unwrap_or_default().to_vec(),
                                false => Vec::new(),
                            },
                            partitions: t.def.partition_cols.clone(),
                        },
                        ProjectState::table_problem(t),
                        t.def.origin.is_internal(),
                    )
                }),
            }
        };
        let waiting = resolved.as_ref().is_some_and(|(s, ..)| s.waiting);
        let problem = resolved.as_ref().and_then(|(_, p, _)| p.clone());
        let status = use_status(waiting, problem);

        let mut measured = use_state(|| f32::INFINITY);
        let mut row_area = use_state(|| None::<Area>);
        use_reveal(tree, self.path.clone(), row_area);
        let name_width = name_width(&self.name);

        let Some((state, _, internal)) = resolved else {
            return rect();
        };

        let icon = IconName::for_catalog(self.kind);
        let icon_color = match self.kind {
            CatalogKind::View => self.theme.view_color,
            CatalogKind::Query => self.theme.query_color,
            CatalogKind::Table if internal => self.theme.internal_color,
            CatalogKind::Table => self.theme.table_color,
        };

        let folds = fold_plan(measured(), name_width, internal, ICON_SLOT);

        let build_menu = {
            let kind = self.kind;
            let name = self.name.clone();
            move || match kind {
                CatalogKind::View => view_menu(&actions, name.clone()),
                _ => table_menu(&actions, name.clone()),
            }
        };
        let menu_for_row = build_menu.clone();

        let expandable = state.expandable;
        let toggle = {
            let path = self.path.clone();
            move |_: Event<PressEventData>| tree.toggle(&path, is_open)
        };

        let row = Row::new(ENTRY_DEPTH, self.theme.clone())
            .disclosure(match expandable {
                true => Disclosure::from_expanded(is_open),
                false => Disclosure::Leaf,
            })
            .on_press(toggle.clone())
            .on_toggle(toggle)
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .on_sized(move |e: Event<SizedEventData>| {
                measured.set_if_modified(e.area.width());
                row_area.set_if_modified(Some(e.area));
            })
            .trailing(actions_button(build_menu))
            .maybe_child(
                folds
                    .mark
                    .then(|| Icon::new(icon).color(icon_color).size(14.).into_element()),
            )
            .child(
                MonoValue::new(self.name.clone())
                    .color(self.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
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
            }));

        let body = is_open.then(|| {
            let expanded = tree.open.read();
            let mut rows = Vec::new();
            flatten_cols(
                &self.path,
                &[],
                0,
                &state.columns,
                &state.partitions,
                &expanded,
                &mut rows,
            );
            drop(expanded);
            rows.into_iter()
                .map(|r| {
                    let key = r.key.clone();
                    ColumnRow::new(self.kind, self.name.clone(), r, self.theme.clone())
                        .key(key)
                        .into_element()
                })
                .collect::<Vec<_>>()
        });

        rect()
            .width(Size::fill())
            .vertical()
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
            .maybe(body.is_some(), |el| el.children(body.unwrap_or_default()))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// Answer a **jump** at this row: an object-store node's child is a link to a workspace def three
/// levels away, and pressing it opens the ancestors and names the row's path.
///
/// An **effect over the reveal slot and the row's last measured area**, not a layout handler.
/// Opening the ancestors is a store write, so a row that was already on screen has no geometry
/// change to report — a handler waiting for one never fires, and the request then goes off at the
/// next unrelated relayout and yanks the pane mid-gesture. Reading both states here means the
/// scroll happens whichever of the two lands last, and the slot is cleared by whoever answers it.
///
/// `area` is the row's own, so an expanded entry scrolls to its name rather than to the bottom of
/// its column block.
fn use_reveal(tree: TreeCtx, path: String, area: State<Option<Area>>) {
    use_side_effect(move || {
        if tree.reveal.read().as_deref() != Some(path.as_str()) {
            return;
        }
        let Some(area) = *area.read() else {
            return;
        };
        let (mut scroll, mut reveal) = (tree.scroll, tree.reveal);
        scroll.scroll_to_item(area);
        reveal.set(None);
    });
}

/// The scan the **row itself** must subscribe to, if any — what it mounts a subscriber-only
/// [`ProfileWatch`] for rather than leaving to [`ProfileStatus`] in the status column.
///
/// The one rule that broke: the subscription is what *dispatches* a scan, so it cannot be a
/// function of whether there is room to draw a spinner. Exactly one of the two is mounted —
/// `ProfileStatus` while the column is there, this while it is folded — so the query is never
/// subscribed twice and never subscribed by nobody.
pub(super) fn watched_scan(folds: Folds, scan: Option<ScanId>) -> Option<ScanId> {
    scan.filter(|_| !folds.status)
}

/// The row's **profiling glyph** (P3-09) — a spinner for exactly as long as this entry's scan is
/// in flight, and the settled verdict otherwise.
///
/// Its own component because it **subscribes** to the scan, and a hook cannot be conditional: the
/// row mounts it only when there is a request to watch, which is also what keeps a sidebar full of
/// tables from subscribing (and, with an un-run entry, *dispatching*) a scan nobody asked for.
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
/// the inspector; the tree's own disclosure arrow (nested columns only) expands in place without
/// selecting.
#[derive(PartialEq)]
struct ColumnRow {
    owner_kind: CatalogKind,
    owner: String,
    row: ColRow,
    theme: CatalogTheme,
    key: DiffKey,
}

impl ColumnRow {
    fn new(owner_kind: CatalogKind, owner: String, row: ColRow, theme: CatalogTheme) -> Self {
        Self {
            owner_kind,
            owner,
            row,
            theme,
            key: DiffKey::None,
        }
    }
}

impl KeyExt for ColumnRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ColumnRow {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
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
        let expand_key = self.row.key.clone();
        let expanded = self.row.is_expanded;

        let part_chip = self.row.is_part.then(|| {
            Badge::tag("PART", self.theme.part_color)
                .background(self.theme.part_background)
                .into_element()
        });

        Row::new(ENTRY_DEPTH + 1 + self.row.depth, self.theme.clone())
            .disclosure(match self.row.has_children {
                true => Disclosure::from_expanded(self.row.is_expanded),
                false => Disclosure::Leaf,
            })
            .selected(selected)
            .on_toggle(move |_| tree.toggle(&expand_key, expanded))
            .on_press(move |_| {
                selection.set(Some(col.clone()));
                layout
                    .write_channel(Chan::Layout)
                    .open_right_pane(RightPane::Inspector);
            })
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

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// One saved query. Addressed by its stable `id` — the name is only a label — so a rename can't
/// dangle whatever holds it.
///
/// Pressing the row opens it in a tab, which is the canvas's own `title="Open in a new tab"`;
/// its menu (right-click or ⋮) adds Rename and Delete. Rename is **inline**, in the row itself,
/// exactly like the tab strip's: the menu item only flips this row's `renaming` flag and the row
/// reacts in its own scope, so the rename survives the menu closing.
///
/// **Unkeyed, unlike every other row in this tree**, and not by choice: keying it crashes the
/// fork's reconciler on the one gesture the key exists for. A rename re-sorts the list, so the
/// keyed row *moves*, and `Tree::apply_mutations` then unwraps a `moved` node its parent's child
/// list no longer holds (`freya-core/src/tree.rs:332`) — a panic where the unkeyed version merely
/// re-renders in place. Until that is fixed in the fork, this list reconciles positionally.
#[derive(PartialEq)]
pub struct SavedQueryRow {
    id: Uuid,
    name: String,
    theme: CatalogTheme,
    key: DiffKey,
}

impl SavedQueryRow {
    pub fn new(id: Uuid, name: String, theme: CatalogTheme) -> Self {
        Self {
            id,
            name,
            theme,
            key: DiffKey::None,
        }
    }
}

impl KeyExt for SavedQueryRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
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

        Row::new(ENTRY_DEPTH, self.theme.clone())
            .on_press(move |_| open_saved_query(&actions, id))
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .trailing(actions_button(build_menu))
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
            .into_element()
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The saved-query row **while it is being renamed**: the same box, with an input in place of
/// the label. Its own component so it can own the commit / cancel listeners — what it replaces
/// is a pressable row, and a row being renamed must not be.
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
            .height(Size::px(ROW_HEIGHT))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .padding(Gaps::new(0., SP_2, 0., (ENTRY_DEPTH + 1) as f32 * INDENT))
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
