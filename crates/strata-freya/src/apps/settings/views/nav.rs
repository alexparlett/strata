//! The Settings window's **category rail** — the collapsible tree down the left, built from
//! [`CATEGORIES`].
//!
//! Rows are Freya's [`SideBarItem`], and a category row's **selection is the router's** —
//! [`ActivableRoute`] provides the `ActivableContext` that both the row's own dress and its
//! label's colour read through [`use_is_active`]. Nothing here compares a route to the
//! current one, and nothing carries a `selected` flag: the route *is* the selection.
//!
//! That is why these rows are not [`SidebarRow`](crate::components::sidebar_row::SidebarRow),
//! the preset the catalog and launcher rails share. It ends in its own `Activable`, and
//! `use_is_active` reads the **closest** provider — so an outer `ActivableRoute` would be
//! shadowed and silently do nothing. Its geometry is a `SideBarItemThemePartial` either way,
//! so what the preset actually saves here is one line of it, which is not worth giving up the
//! framework's own router integration for. The catalog and launcher rails keep it: they mark a
//! *selection*, not a location, and have no route to read.
//!
//! The group headings collapse, which is local state: a view preference with nothing else
//! depending on it, and the design doesn't persist it either.
//!
//! The canvas's search box above the tree belongs to P4-09 and is deliberately absent: a
//! search field that returns nothing is worse than none, and the index it filters is that
//! task's to build.

use std::collections::HashSet;

use freya::components::{use_is_active, ActivableRoute, SideBarItem, SideBarItemThemePartial};
use freya::prelude::*;
use freya::router::*;

use crate::apps::settings::{
    Category, NavGroup, SettingsThemePartial, SettingsThemePreference, CATEGORIES,
};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::Control;

/// The rail's width (canvas `width: 244px`), the hairline included.
const RAIL_WIDTH: f32 = 244.;

/// A top-level row's own inset from the rail's edge (canvas `--sp-4`).
const RAIL_INSET: f32 = 12.;

/// Every row's corner (canvas `--r-1`) and the gap to the next one (canvas `--sp-1`).
const ROW_RADIUS: f32 = 6.;
const ROW_GAP: f32 = 2.;

/// A group heading's padding (canvas `--sp-3 --sp-4`).
const HEADING_PADDING: Gaps = Gaps::new(8., 12., 8., RAIL_INSET);

/// The heading's disclosure chevron and the gap after it (canvas `10px` + `--sp-2`), and the
/// column the two together occupy. Derived rather than restated, so the indents below can't
/// drift from the chevron they are measured against.
const CHEVRON_SIZE: f32 = 10.;
const CHEVRON_GAP: f32 = 4.;
const CHEVRON_COLUMN: f32 = CHEVRON_SIZE + CHEVRON_GAP;

/// Where a group heading's **label** starts — past its own inset and its chevron. Both row
/// indents below are measured from here, because that is the line the eye reads the tree
/// against; neither is a number of its own.
const LABEL_ORIGIN: f32 = RAIL_INSET + CHEVRON_COLUMN;

/// How far a page is set in past its heading's label (`--sp-3`). The canvas nests by `--sp-2`
/// (its rows land at 30px); at this size that read as a rounding error rather than a level, so
/// the step is one token wider — a deliberate divergence, and the only number to change if the
/// nesting wants adjusting again.
const NEST_STEP: f32 = 8.;

/// A grouped category: its heading's label, plus one nesting step.
const ROW_PADDING: Gaps = Gaps::new(8., 12., 8., LABEL_ORIGIN + NEST_STEP);

/// An **ungrouped** category (Keymap) still indents, to its heading's label but no further: it
/// has no chevron of its own, so at the rail inset its label would start in the chevron column
/// and read as a third heading rather than as a peer of the pages. The canvas spells the same
/// thing out as `calc(--sp-4 + --sp-2 + 10px)`.
const UNGROUPED_PADDING: Gaps = Gaps::new(8., 12., 8., LABEL_ORIGIN);

#[derive(PartialEq)]
pub struct Nav;

impl Component for Nav {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        // Which headings are folded away. Collapsed-by-exception, so a group added later
        // shows up rather than hiding.
        let collapsed = use_state(HashSet::<NavGroup>::new);

        // Walk the categories in order, opening a heading whenever the group changes. The
        // list is contiguous by group (`model`'s test pins that), so one pass draws the whole
        // tree without grouping it first.
        let mut tree = rect().width(Size::fill()).vertical();
        let mut heading: Option<NavGroup> = None;
        for cat in CATEGORIES {
            if cat.group != heading {
                heading = cat.group;
                if let Some(group) = cat.group {
                    tree = tree.child(GroupHeading { group, collapsed });
                }
            }
            if cat.group.is_some_and(|g| collapsed.read().contains(&g)) {
                continue;
            }
            tree = tree.child(CategoryRow { category: cat });
        }

        rect()
            .width(Size::px(RAIL_WIDTH))
            .height(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .child(
                rect()
                    .width(Size::flex(1.))
                    .height(Size::fill())
                    .vertical()
                    .background(theme.nav_background)
                    .padding(Gaps::new(12., 12., 12., 12.))
                    .child(tree),
            )
            .child(Divider::vertical().color(theme.border_fill))
    }
}

/// A collapsible heading over its categories. Pressing it folds the group away; the chevron
/// points right when folded and down when open, as the canvas's rotation does.
#[derive(PartialEq)]
struct GroupHeading {
    group: NavGroup,
    collapsed: State<HashSet<NavGroup>>,
}

impl Component for GroupHeading {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        let group = self.group;
        let mut collapsed = self.collapsed;
        let open = !collapsed.read().contains(&group);

        // A heading is not a destination, so it has no `ActivableRoute` and never lights up —
        // only the hover fill its `sidebar_item` theme already carries.
        SideBarItem::new()
            .theme(row_theme(HEADING_PADDING, None))
            .on_press(move |_: Event<PressEventData>| {
                let mut set = collapsed.write();
                if !set.remove(&group) {
                    set.insert(group);
                }
            })
            .child(
                row_content(CHEVRON_GAP)
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(CHEVRON_SIZE)
                        .color(theme.chevron_color),
                    )
                    .child(Control::new(group.label()).color(theme.group_color)),
            )
    }
}

/// One category, wrapped in the router's [`ActivableRoute`] so being *here* is what makes the
/// row look active — no flag to thread and nothing to keep in step with the route.
///
/// `exact`: `Route::Theme` is `/`, and every other route is its child by
/// [`Routable::is_child_of`], so the descendant match would light Theme up on every page.
/// These categories are flat peers; only an exact match means "you are here".
#[derive(PartialEq)]
struct CategoryRow {
    category: &'static Category,
}

impl Component for CategoryRow {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        let route = self.category.route.clone();
        let padding = if self.category.group.is_some() {
            ROW_PADDING
        } else {
            UNGROUPED_PADDING
        };

        let row = SideBarItem::new()
            .theme(row_theme(padding, Some(theme.item_active_background)))
            // `replace`, not `push`: the categories are peers, not a trail, so the window has
            // no back stack to grow (and nothing offers to walk one). The discarded `Result`
            // only ever reports a failed *external* navigation, which a `Route` cannot be.
            .on_press(move |_: Event<PressEventData>| {
                let _ = RouterContext::get().replace(route.clone());
            })
            .child(row_content(0.).child(CategoryLabel {
                label: self.category.label,
            }));

        ActivableRoute::new(self.category.route.clone(), row).exact(true)
    }
}

/// A category's label, which brightens when its row is the current route. Its own component
/// because [`use_is_active`] reads the closest [`ActivableRoute`] *from inside* the row it
/// wraps — which is the same context the row's own fill comes from, so the two can't disagree.
#[derive(PartialEq)]
struct CategoryLabel {
    label: &'static str,
}

impl Component for CategoryLabel {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(
            &None::<SettingsThemePartial>,
            SettingsThemePreference,
            "settings"
        );
        let color = if use_is_active() {
            theme.item_active_color
        } else {
            theme.item_color
        };
        Control::new(self.label).color(color)
    }
}

/// A rail row's geometry: the caller's inset, the shared corner and the gap to the next row.
/// The colours stay the `sidebar_item` theme's, so hover can't drift from the other rails —
/// except the *active* fill, which this rail alone paints with the accent tint (it marks where
/// you are, not what you picked).
fn row_theme(padding: Gaps, active_background: Option<Color>) -> SideBarItemThemePartial {
    let theme = SideBarItemThemePartial::new()
        .padding(padding)
        .corner_radius(CornerRadius::new_all(ROW_RADIUS))
        .margin(Gaps::new(0., 0., ROW_GAP, 0.));
    match active_background {
        Some(background) => theme.active_background(background),
        None => theme,
    }
}

/// A row's content box: full width and flexed, so a label truncates inside the row rather than
/// pushing it wider.
fn row_content(spacing: f32) -> Rect {
    rect()
        .width(Size::fill())
        .horizontal()
        .content(Content::Flex)
        .cross_align(Alignment::Center)
        .spacing(spacing)
}
