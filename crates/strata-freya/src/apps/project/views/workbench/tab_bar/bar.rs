use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::apps::project::views::workbench::tab_bar::controls::TabControls;
use crate::apps::project::views::workbench::tab_bar::drag;
use crate::apps::project::views::workbench::tab_bar::tab::{Tab, TAB_HEIGHT};
use crate::components::divider::Divider;
use crate::components::typography::Body;
use freya::components::{define_theme, get_theme, use_drag, use_scroll_controller, use_theme, DropZone, ScrollConfig, ScrollView};
use freya::prelude::{rect, use_state, Alignment, Area, ChildrenExt, Color, Component, ContainerExt, ContainerSizeExt, ContainerWithContentExt, Content, Direction, Element, Event, EventHandlersExt, Gaps, IntoElement, PointerEventData, Size, SizedEventData, StyleExt, WritableUtils};
use freya::radio::use_radio;
use std::collections::HashMap;

/// How close (px) to a strip edge a drag must reach before it auto-scrolls, and how far it scrolls
/// per pointer move. Ported from the Dioxus strip (52 / 16).
const EDGE_MARGIN: f32 = 52.0;
const EDGE_STEP: f32 = 16.0;


define_theme!(
    %[component]
    pub TabBar {
        %[fields]
        background: Color,
        divider_fill: Color,
    }
);

/// The workspace tab strip: the surface-coloured rail across the top of the workbench, holding one
/// [`Tab`] per open query in session order, over a 1px bottom divider. Reads the Session store on
/// `Chan::Tabs`, so opening / switching / closing / reordering a tab re-renders the strip.
///
/// Layout is the kanban shape: the whole strip is one [`DropZone`] (the drop target) holding a
/// horizontal [`ScrollView`] of tabs. Each tab is a self-contained [`Tab`] that owns its own drag +
/// right-click menu; the strip only wires the drop target and the reorder maths (the placeholder slot
/// + edge-scroll). The pinned right cluster (new · navigate · overflow) is [`TabControls`].
#[derive(PartialEq)]
pub struct TabBar {
    pub theme: Option<TabBarThemePartial>,
}

impl TabBar {
    pub fn new() -> Self {
        Self { theme: None }
    }
}

impl Component for TabBar {
    fn render(&self) -> impl IntoElement {
        let TabBarTheme {
            background,
            divider_fill,
        } = get_theme!(&self.theme, TabBarThemePreference, "tab_bar");

        // Read the strip structure once, into owned tuples, so the read guard drops before we build
        // elements. Structural / active changes on `Chan::Tabs` re-render us.
        let mut radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let tabs: Vec<(TabId, String, bool, bool)> = {
            let s = radio.read();
            s.order
             .iter()
             .filter_map(|id| {
                 s.tabs
                  .get(id)
                  .map(|t| (*id, t.name.clone(), s.active == Some(*id), t.is_dirty()))
             })
             .collect()
        };

        // One `ScrollController` both drives the horizontal scroll and lets the *active* tab reveal
        // itself (`scroll_to_item`): it's handed to the ScrollView (`new_controlled`) and to each tab.
        let controller = use_scroll_controller(ScrollConfig::default);
        // Each tab reports its measured area here (drag hit-testing); the strip viewport is measured
        // on the shell (edge-scroll). Both are peeked from the pointer handler, never read in render.
        let areas = use_state(HashMap::<TabId, Area>::new);
        let mut viewport = use_state(Area::default);

        // Drag-reorder state. `use_drag` says whether a tab is being dragged (and which); `insert` is
        // the slot the drop placeholder sits in, in *dragged-excluded* strip order (so it feeds
        // `move_tab` directly). Reading `dragging`/`insert` here re-renders the strip as the drag
        // progresses, so the placeholder tracks the pointer and the dragged tab collapses out.
        let dragging = use_drag::<TabId>();
        let dragged = dragging();
        let mut insert = use_state(|| None::<usize>);
        let insert_at = dragged.and(insert());
        // The dragged tab's name sizes the placeholder (rendered invisibly); its inverse surface fills.
        let dragged_name = dragged
            .and_then(|d| tabs.iter().find(|(i, ..)| *i == d).map(|(_, n, ..)| n.clone()))
            .unwrap_or_default();
        let slot_bg = use_theme().read().colors.surface_inverse;

        // While a drag is live, one global pointer handler drives both the placeholder slot and
        // edge-scroll. It hit-tests the *remaining* tabs (dragged excluded, matching how the strip
        // renders and what `move_tab` expects) via `drag::insert_slot`, and scrolls when the pointer
        // nears a strip edge via `drag::edge_scroll`. All maths live in `super::drag`; this only wires.
        let order_h: Vec<TabId> = tabs.iter().map(|(id, ..)| *id).collect();
        let pointer_move = move |e: Event<PointerEventData>| {
            let Some(drag_id) = *dragging.peek() else {
                return;
            };
            let x = e.global_location().x as f32;
            let ordered: Vec<Area> = {
                let a = areas.peek();
                order_h
                    .iter()
                    .filter(|id| **id != drag_id)
                    .map(|id| a.get(id).copied().unwrap_or_default())
                    .collect()
            };
            let slot = drag::insert_slot(x, &ordered);
            if *insert.peek() != Some(slot) {
                insert.set(Some(slot));
            }
            let delta = drag::edge_scroll(x, *viewport.peek(), EDGE_MARGIN, EDGE_STEP);
            if delta != 0.0 {
                let (sx, _): (i32, i32) = controller.into();
                let mut c = controller;
                c.scroll_to_x(sx + delta as i32);
            }
        };

        // Build the track's children in strip order, interleaving the placeholder at `insert_at`
        // (counted over the tabs that stay put). The dragged tab still renders — `show_while_dragging`
        // collapses it to nothing while its ghost follows the cursor — so it keeps hosting the drag.
        let mut children: Vec<Element> = Vec::new();
        let mut rem_idx = 0usize;
        for (id, name, active, dirty) in tabs.iter().cloned() {
            let is_dragged = dragged == Some(id);
            if !is_dragged && insert_at == Some(rem_idx) {
                children.push(drop_slot(dragged_name.clone(), slot_bg).into());
            }
            // Each tab is a self-contained `Tab` — it owns its own `DragZone` (ghost, collapse-while-
            // dragging, drag-disabled-while-renaming). The strip only places the drop placeholder.
            children.push(Tab::new(id, name, active, dirty, controller, areas).into());
            if !is_dragged {
                rem_idx += 1;
            }
        }
        if insert_at == Some(rem_idx) {
            children.push(drop_slot(dragged_name, slot_bg).into());
        }

        let track = ScrollView::new_controlled(controller)
            .direction(Direction::Horizontal)
            .show_scrollbar(false)
            .drag_scrolling(false)
            .children(children);

        // The drop target = the whole strip. Releasing over it commits to the placeholder slot
        // (`insert`, dragged-excluded order); `move_tab` no-ops a drop back onto the tab's own gap.
        let drop_zone = DropZone::new(track, move |dragged: TabId| {
            if let Some(ins) = *insert.peek() {
                radio.write().move_tab(dragged, ins);
            }
            insert.set(None);
        });

        // The shell takes the flex(1) space (so the right cluster stays pinned) and gives the auto
        // DropZone / fill ScrollView their bounds. It carries the viewport measurement + the drag
        // pointer handler, both on this stationary box (not inside the scrolling content).
        let tabs_area = rect()
            .width(Size::flex(1.))
            .height(Size::fill())
            .on_sized(move |e: Event<SizedEventData>| viewport.set(e.area))
            .on_global_pointer_move(pointer_move)
            .child(drop_zone);

        // The strip row: tabs (flex) + the pinned right cluster (new / navigate / overflow).
        let row = rect()
            .width(Size::fill())
            .height(Size::flex(1.0))
            .horizontal()
            .content(Content::Flex)
            .child(tabs_area)
            .child(Divider::vertical().color(divider_fill))
            .child(TabControls::new());

        rect()
            .background(background)
            .content(Content::Flex)
            .vertical()
            .width(Size::fill())
            .height(Size::px(38.0))
            .child(row)
            .child(Divider::horizontal().color(divider_fill))
    }
}

/// The drop indicator: the placeholder gap that opens at the insert slot while a tab is dragged. A
/// solid inverse-surface fill, sized (invisibly) to the dragged tab's name so it's roughly the tab's
/// width.
fn drop_slot(name: String, bg: Color) -> impl Into<Element> {
    rect()
        .height(Size::px(TAB_HEIGHT))
        .cross_align(Alignment::Center)
        .padding(Gaps::new(0., 12., 0., 12.))
        .background(bg)
        .child(Body::new(name).color(Color::TRANSPARENT))
}