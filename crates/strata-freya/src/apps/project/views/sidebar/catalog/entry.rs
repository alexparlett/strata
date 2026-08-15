//! The workspace's content rows: a table / view **entry**, one **column** row (nested or not), and
//! a **saved-query** row.
//!
//! Tables and views share [`entry_row`] and the column rows below it, because a view's columns
//! *are* columns — clickable, selectable, expandable when nested. In the Dioxus sidebar these were
//! two copies that differed only by omission (view rows had no click handler at all, so clicking one
//! silently did nothing), which is exactly what a second copy of a list buys you.
//!
//! **A row here holds no state that identifies it.** The tree is virtualized, so a scope is a
//! *slot* rather than a row: scrolling hands slot 3 a different entry, and anything the slot
//! remembered would be remembered about the wrong row. What a slot may keep is what is true of the
//! slot (its measured width) or what is tagged with whose it is
//! ([`use_status`](super::row::use_status)); what belongs to one row across scrolling — which saved
//! query is being renamed — lives on the pane's [`TreeCtx`](super::TreeCtx).

use freya::prelude::*;
use freya::query::QueryStateData;
use strata_model::{CatalogKind, ColRef, RightPane};
use uuid::Uuid;

use super::menu::{
    open_saved_query, query_menu, rename_saved_query, table_menu, use_catalog_actions, view_menu,
};
use super::node::{Column, Entry, Place};
use super::row::{
    actions_button, fold_plan, name_width, tip, Folds, Row, StatusMark, ICON_SLOT, INDENT,
    ROW_HEIGHT,
};
use super::view::{body, RowBody, RowCtx};
use super::{CatalogTheme, TreeCtx};
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::{use_profile, ProfileTarget, ScanId};
use crate::apps::project::state::Chan;
use crate::components::badge::Badge;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_2, SP_3, STATUS_DOT};
use crate::components::type_palette::kind_color;
use crate::components::typography::{Body, InputTypography, Meta, MonoValue};

/// What the **profiling** spinner says — its own words, because the registration spinner beside it
/// means something else entirely (a scan is minutes of work the user asked for; a registration is a
/// metadata read they didn't).
const PROFILING: &str = "Profiling…";

/// The marker a table row carries when Strata owns its data (ED-04).
///
/// **Not cosmetic.** Tables of both origins live in one group under one glyph, and the difference
/// between them is what a drop means: one origin's Drop removes a def and leaves the user's files
/// alone, the other's deletes the only copy of the data. The row is where that has to be legible,
/// because the row is what gets right-clicked.
const INTERNAL_BADGE: &str = "INTERNAL";
/// What that marker means, on hover and to a screen reader.
const INTERNAL_TIP: &str = "Strata stores this table's data in the project";

/// One workspace entry — a table or a view.
pub fn entry_row(at: &Place, entry: &Entry, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let mut measured = cx.measured;
    let actions = cx.catalog.clone();

    let (kind, name) = (entry.kind, entry.name.clone());
    let target = ProfileTarget::Workspace {
        kind,
        name: name.clone(),
    };
    let icon_color = match kind {
        CatalogKind::View => cx.theme.view_color,
        CatalogKind::Query => cx.theme.query_color,
        CatalogKind::Table if entry.internal => cx.theme.internal_color,
        CatalogKind::Table => cx.theme.table_color,
    };
    let folds = fold_plan(
        measured(),
        name_width(&name, cx.advance),
        entry.internal,
        ICON_SLOT,
    );

    let build_menu = {
        let name = name.clone();
        move || match kind {
            CatalogKind::View => view_menu(&actions, name.clone()),
            _ => table_menu(&actions, name.clone()),
        }
    };
    let menu_for_row = build_menu.clone();
    let (open, path) = (at.open, at.path.clone());
    let toggle = move |_: Event<PressEventData>| tree.toggle(&path, open);

    let row = Row::new(at.depth, cx.theme.clone())
        .disclosure(at.disclosure())
        .on_press(toggle.clone())
        .on_toggle(toggle)
        .on_context_menu(move |_: Event<PressEventData>| {
            ContextMenu::open(menu_for_row());
        })
        .on_sized(move |e: Event<SizedEventData>| {
            measured.set_if_modified(e.area.width());
        })
        .trailing(actions_button(build_menu))
        .maybe_child(folds.mark.then(|| {
            Icon::new(IconName::for_catalog(kind))
                .color(icon_color)
                .size(14.)
                .into_element()
        }))
        .child(
            MonoValue::new(name)
                .color(cx.theme.name_color)
                .width(Size::flex(1.))
                .text_overflow(TextOverflow::Ellipsis),
        )
        .maybe_child(folds.badge.then(|| {
            tip(INTERNAL_TIP)
                .child(
                    rect()
                        .a11y_alt(INTERNAL_TIP)
                        .child(Badge::tag(INTERNAL_BADGE, cx.theme.internal_color).into_element()),
                )
                .into_element()
        }))
        .maybe_child(folds.status.then(|| {
            rect()
                .width(Size::px(STATUS_DOT))
                .cross_align(Alignment::Center)
                .maybe_child(match entry.scan {
                    Some(scan) => Some(
                        ProfileStatus {
                            target: target.clone(),
                            scan,
                            settled: cx.status.clone(),
                            theme: cx.theme.clone(),
                        }
                        .into_element(),
                    ),
                    None => cx.status.as_ref().map(|s| s.glyph(&cx.theme)),
                })
                .into_element()
        }));

    let mut out = body(row);
    out.extend(
        watched_scan(folds, entry.scan).map(|scan| ProfileWatch { target, scan }.into_element()),
    );
    out
}

/// The scan the **row itself** must subscribe to, if any — what it mounts a subscriber-only
/// [`ProfileWatch`] for rather than leaving to [`ProfileStatus`] in the status column.
///
/// The subscription is what *dispatches* a scan, so it cannot be a function of whether there is
/// room to draw a spinner: a Profile asked for while the sidebar is narrow would otherwise mount
/// nothing and start nothing, and the user would have accepted the cost confirm for no work at all.
/// Exactly one of the two is mounted — `ProfileStatus` while the column is there, this while it is
/// folded — so the query is never subscribed twice and never subscribed by nobody.
///
/// Virtualization does not widen that rule to the whole pane. A row scrolled out of the window
/// unmounts its subscriber, but `use_query` deliberately does not cancel a running execution on
/// unmount, so the scan the user paid for finishes and the row re-attaches to it (or reads the
/// settled entry) on the way back in.
pub(super) fn watched_scan(folds: Folds, scan: Option<ScanId>) -> Option<ScanId> {
    scan.filter(|_| !folds.status)
}

/// The row's **profiling glyph** (P3-09) — a spinner for exactly as long as this entry's scan is in
/// flight, and the settled verdict otherwise.
///
/// Its own component because it **subscribes** to the scan, and a hook cannot be conditional: the
/// row mounts it only when there is a request to watch, which is also what keeps a sidebar full of
/// tables from subscribing (and, with an un-run entry, *dispatching*) a scan nobody asked for.
#[derive(PartialEq)]
struct ProfileStatus {
    target: ProfileTarget,
    scan: ScanId,
    /// What the column says when **no scan is running** — so the one slot is never empty while the
    /// row has something to report, and never holds two glyphs at once.
    settled: Option<StatusMark>,
    theme: CatalogTheme,
}

impl Component for ProfileStatus {
    fn render(&self) -> impl IntoElement {
        match scan_running(&self.target, self.scan) {
            true => tip(PROFILING)
                .child(CircularLoader::new().size(STATUS_DOT).a11y_alt(PROFILING))
                .into_element(),
            false => match &self.settled {
                Some(mark) => mark.glyph(&self.theme),
                None => rect().into_element(),
            },
        }
    }
}

/// The same subscription with **no glyph** — what the row mounts in place of [`ProfileStatus`] when
/// the fold plan has taken the status column away. See [`watched_scan`].
#[derive(PartialEq)]
struct ProfileWatch {
    target: ProfileTarget,
    scan: ScanId,
}

impl Component for ProfileWatch {
    fn render(&self) -> impl IntoElement {
        let _running = scan_running(&self.target, self.scan);
        rect()
    }
}

/// Subscribe to `target`'s scan and answer whether it is executing right now — the one hook both
/// [`ProfileStatus`] and [`ProfileWatch`] are built around, so the two cannot subscribe differently.
fn scan_running(target: &ProfileTarget, scan: ScanId) -> bool {
    let engine = use_consume::<EngineCtx>();
    let query = use_profile(&engine, target, scan);
    let reader = query.read();
    let running = matches!(
        &*reader.state(),
        QueryStateData::Pending | QueryStateData::Loading { .. }
    );
    drop(reader);
    running
}

/// One column row — a top-level column or an expanded nested field. Selecting it is what drives the
/// inspector; the tree's own disclosure arrow (nested columns only) expands in place without
/// selecting.
pub fn column_row(at: &Place, column: &Column, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let mut selection = cx.selection;
    let mut layout = cx.layout;

    let col = ColRef {
        owner: column.owner.clone(),
        path: column.row.path.clone(),
    };
    let selected = selection.read().as_ref() == Some(&col);

    let swatch = kind_color(column.row.kind, &cx.palette);
    let (open, path) = (at.open, at.path.clone());

    let part_chip = column.row.is_part.then(|| {
        Badge::tag("PART", cx.theme.part_color)
            .background(cx.theme.part_background)
            .into_element()
    });

    body(
        Row::new(at.depth, cx.theme.clone())
            .disclosure(at.disclosure())
            .selected(selected)
            .on_toggle(move |_: Event<PressEventData>| tree.toggle(&path, open))
            .on_press(move |_: Event<PressEventData>| {
                selection.set(Some(col.clone()));
                layout
                    .write_channel(Chan::Layout)
                    .open_right_pane(RightPane::Inspector);
            })
            .child(Dot::new(swatch).size(6.).square())
            .child(
                MonoValue::new(column.row.name.clone())
                    .color(cx.theme.column_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            )
            .maybe_child(part_chip)
            .child(Meta::new(column.row.dtype.clone()).color(swatch)),
    )
}

/// One saved query. Addressed by its stable `id` — the name is only a label — so a rename can't
/// dangle whatever holds it.
///
/// Pressing the row opens it in a tab, which is the canvas's own `title="Open in a new tab"`; its
/// menu (right-click or ⋮) adds Rename and Delete. Rename is **inline**, in the row itself, exactly
/// like the tab strip's — but the flag naming which row is renaming lives on `TreeCtx`, not in a
/// slot: in a virtualized tree a scope is a slot, and a slot that remembered "I am being renamed"
/// would hand that to whichever query scrolled into it.
pub fn saved_query_row(at: &Place, id: Uuid, name: &str, cx: &RowCtx) -> RowBody {
    let tree = cx.tree;
    let actions = cx.catalog.clone();

    if *tree.renaming.read() == Some(id) {
        return body(QueryRename {
            depth: at.depth,
            id,
            theme: cx.theme.clone(),
        });
    }

    let build_menu = {
        let actions = actions.clone();
        let name = name.to_string();
        move || query_menu(&actions, id, name.clone(), tree.renaming, tree.draft)
    };
    let menu_for_row = build_menu.clone();

    body(
        Row::new(at.depth, cx.theme.clone())
            .on_press(move |_: Event<PressEventData>| open_saved_query(&actions, id))
            .on_context_menu(move |_: Event<PressEventData>| {
                ContextMenu::open(menu_for_row());
            })
            .trailing(actions_button(build_menu))
            .child(
                Icon::new(IconName::Brackets)
                    .color(cx.theme.query_color)
                    .size(14.),
            )
            .child(
                Body::new(name.to_string())
                    .color(cx.theme.name_color)
                    .width(Size::flex(1.))
                    .text_overflow(TextOverflow::Ellipsis),
            ),
    )
}

/// The saved-query row **while it is being renamed**: the same box, with an input in place of the
/// label. Its own component so it can own the commit / cancel listeners — what it replaces is a
/// pressable row, and a row being renamed must not be.
///
/// Enter commits (the input's `on_submit`) and a press anywhere outside the row commits too, like a
/// blur. **Escape is the pane's**, not this row's: a listener here would go with the row the moment
/// it scrolled out of the virtualized window, leaving a rename nothing could cancel.
#[derive(PartialEq)]
struct QueryRename {
    depth: usize,
    id: Uuid,
    theme: CatalogTheme,
}

impl Component for QueryRename {
    fn render(&self) -> impl IntoElement {
        let tree = use_consume::<TreeCtx>();
        let id = self.id;
        let draft = tree.draft;
        let mut area = use_state(|| None::<Area>);
        let a11y = use_a11y();
        let actions = use_catalog_actions();

        let outside_actions = actions.clone();
        let mut renaming = tree.renaming;

        rect()
            .width(Size::fill())
            .height(Size::px(ROW_HEIGHT))
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .padding(Gaps::new(0., SP_2, 0., (self.depth + 1) as f32 * INDENT))
            .on_sized(move |e: Event<SizedEventData>| area.set(Some(e.area)))
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
                        renaming.set(None);
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
                        renaming.set(None);
                    }),
            ))
    }
}
