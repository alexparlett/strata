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
//! Above the tree is the **search box** (P4-09), and while it has a query it *replaces* the tree
//! rather than filtering it — the canvas's arrangement, and the honest one: a hit can be a
//! property on a page the tree only names, so the results are a list of settings and not a
//! narrower list of categories. Everything it knows about is the settings index (`search.rs`), and
//! picking a hit is the one place a jump is performed ([`follow`]).

use std::collections::HashSet;

use freya::components::{
    use_is_active, ActivableRoute, Input, SideBarItem, SideBarItemThemePartial,
};
use freya::prelude::*;
use freya::router::*;
use strata_core::config::Command;

use crate::apps::settings::views::PropRows;
use crate::apps::settings::{
    search, settings_theme, Category, Hit, NavGroup, SettingsCtx, CATEGORIES,
};
use crate::components::divider::Divider;
use crate::components::form::Reveal;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{R_1, SP_1, SP_2, SP_3, SP_4};
use crate::components::typography::{Caption, Control, InputTypography, Path};
use crate::keymap::on_command;
use crate::state::use_config_station;

/// The rail's width (canvas `width: 244px`), the hairline included.
const RAIL_WIDTH: f32 = 244.;

/// A top-level row's own inset from the rail's edge (canvas `--sp-4`).
const RAIL_INSET: f32 = SP_4;

/// Every row's corner (canvas `--r-1`) and the gap to the next one (canvas `--sp-1`).
const ROW_RADIUS: f32 = R_1;
const ROW_GAP: f32 = SP_1;

/// A group heading's padding (canvas `--sp-3 --sp-4`).
const HEADING_PADDING: Gaps = Gaps::new(SP_3, SP_4, SP_3, RAIL_INSET);

/// The heading's disclosure chevron and the gap after it (canvas `10px` + `--sp-2`), and the
/// column the two together occupy. Derived rather than restated, so the indents below can't
/// drift from the chevron they are measured against.
const CHEVRON_SIZE: f32 = 10.;
const CHEVRON_GAP: f32 = SP_2;
const CHEVRON_COLUMN: f32 = CHEVRON_SIZE + CHEVRON_GAP;

/// Where a group heading's **label** starts — past its own inset and its chevron. Both row
/// indents below are measured from here, because that is the line the eye reads the tree
/// against; neither is a number of its own.
const LABEL_ORIGIN: f32 = RAIL_INSET + CHEVRON_COLUMN;

/// How far a page is set in past its heading's label (`--sp-3`). The canvas nests by `--sp-2`
/// (its rows land at 30px); at this size that read as a rounding error rather than a level, so
/// the step is one token wider — a deliberate divergence, and the only number to change if the
/// nesting wants adjusting again.
const NEST_STEP: f32 = SP_3;

/// A grouped category: its heading's label, plus one nesting step.
const ROW_PADDING: Gaps = Gaps::new(SP_3, SP_4, SP_3, LABEL_ORIGIN + NEST_STEP);

/// An **ungrouped** category (Keymap) still indents, to its heading's label but no further: it
/// has no chevron of its own, so at the rail inset its label would start in the chevron column
/// and read as a third heading rather than as a peer of the pages. The canvas spells the same
/// thing out as `calc(--sp-4 + --sp-2 + 10px)`.
const UNGROUPED_PADDING: Gaps = Gaps::new(SP_3, SP_4, SP_3, LABEL_ORIGIN);

/// The gap under the search box, before whatever it is standing over (canvas `--sp-4`).
const SEARCH_GAP: f32 = SP_4;

/// A result row's inset (canvas `--sp-3 --sp-4`) and the gap between the two lines it holds
/// (`--sp-1`).
const RESULT_PADDING: Gaps = Gaps::new(SP_3, SP_4, SP_3, SP_4);
const RESULT_LINE_GAP: f32 = SP_1;

/// The empty state's inset (canvas `--sp-4`).
const NO_RESULTS_PADDING: Gaps = Gaps::new_all(SP_4);

#[derive(PartialEq)]
pub struct Nav;

impl Component for Nav {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let collapsed = use_state(HashSet::<NavGroup>::new);
        let query = use_state(String::new);
        let config = use_config_station();
        let reveal = use_consume::<Reveal>();
        let engine = use_consume::<SettingsCtx>().engine;

        let hits = search(&query.read());
        let searching = !query.read().trim().is_empty();
        let body = match (searching, hits.is_empty()) {
            (false, _) => {
                let mut tree = rect().key("tree").width(Size::fill()).vertical();
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
                tree.into_element()
            }
            (true, false) => {
                let mut results = rect().key("results").width(Size::fill()).vertical();
                for hit in hits {
                    results = results.child(
                        ResultRow {
                            hit,
                            reveal,
                            engine,
                            query,
                            key: DiffKey::None,
                        }
                        .key(hit.id()),
                    );
                }
                results.into_element()
            }
            (true, true) => rect()
                .key("no-results")
                .width(Size::fill())
                .padding(NO_RESULTS_PADDING)
                .child(
                    Caption::new("No settings match your search.")
                        .color(theme.hint_color)
                        .width(Size::fill())
                        .wrap(),
                )
                .into_element(),
        };

        rect()
            .width(Size::px(RAIL_WIDTH))
            .height(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .on_global_key_down(on_command(config, Command::Cancel, move || {
                let mut query = query;
                match query.peek().trim().is_empty() {
                    true => false,
                    false => {
                        query.set(String::new());
                        true
                    }
                }
            }))
            .child(
                rect()
                    .width(Size::flex(1.))
                    .height(Size::fill())
                    .vertical()
                    .content(Content::Flex)
                    .background(theme.nav_background)
                    .padding(Gaps::new(SP_4, SP_4, SP_4, SP_4))
                    .child(
                        InputTypography::body(
                            Input::new(query)
                                .placeholder("Search settings")
                                .leading(
                                    Icon::new(IconName::Search)
                                        .color(theme.chevron_color)
                                        .size(13.),
                                )
                                .on_submit(move |_: String| {
                                    if let Some(hit) = search(&query.peek()).first() {
                                        follow(*hit, reveal, engine, query);
                                    }
                                })
                                .width(Size::fill()),
                        )
                        .width(Size::fill()),
                    )
                    .child(rect().height(Size::px(SEARCH_GAP)))
                    .child(
                        ScrollView::new()
                            .width(Size::fill())
                            .height(Size::flex(1.))
                            .child(body),
                    ),
            )
            .child(Divider::vertical().color(theme.border_fill))
    }
}

/// Go to a hit: the page it is on, then whatever singles it out there.
///
/// The one place a jump happens — the pressed row and Enter both come through here, so "what
/// picking a result does" is one answer rather than two that can drift. Clearing the query is part
/// of it: the box empties, which is what brings the category tree back with the new page marked.
///
/// Every handle is a parameter because this runs from an event handler, where there is no scope left
/// to consume a context from.
fn follow(hit: Hit, reveal: Reveal, engine: State<PropRows>, query: State<String>) {
    let _ = RouterContext::get().replace(hit.route());
    match hit {
        Hit::Setting(anchor) => reveal.ask(anchor.id()),
        Hit::Property(entry) => {
            let mut engine = engine;
            engine.write().reveal(entry.key);
        }
        Hit::Page(_) => {}
    }
    let mut query = query;
    query.set(String::new());
}

/// One search hit: what it is called, over where it lives.
///
/// A [`SideBarItem`] like the category rows, so hover and keyboard focus are the rail's and not a
/// second lookalike — but with no [`ActivableRoute`], because a result is not a place you are.
#[derive(PartialEq)]
struct ResultRow {
    hit: Hit,
    /// What a press follows the hit with — see [`follow`].
    reveal: Reveal,
    engine: State<PropRows>,
    /// The search box, cleared by a press.
    query: State<String>,
    key: DiffKey,
}

/// Keyed on the hit's own identity, so retyping the query re-associates the rows with their hits
/// instead of sliding each one's hover state onto whatever took its place.
impl KeyExt for ResultRow {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for ResultRow {
    fn render(&self) -> impl IntoElement {
        let theme = settings_theme();
        let hit = self.hit;
        let (reveal, engine, query) = (self.reveal, self.engine, self.query);

        let location = match hit {
            Hit::Property(_) => Path::new(hit.location())
                .color(theme.hint_color)
                .width(Size::fill())
                .text_overflow(TextOverflow::Ellipsis)
                .into_element(),
            _ => Caption::new(hit.location())
                .color(theme.hint_color)
                .width(Size::fill())
                .text_overflow(TextOverflow::Ellipsis)
                .into_element(),
        };

        SideBarItem::new()
            .theme(row_theme(RESULT_PADDING, None))
            .on_press(move |_: Event<PressEventData>| follow(hit, reveal, engine, query))
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .spacing(RESULT_LINE_GAP)
                    .child(
                        Control::new(hit.label())
                            .color(theme.item_active_color)
                            .width(Size::fill())
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .child(location),
            )
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
        let theme = settings_theme();
        let group = self.group;
        let mut collapsed = self.collapsed;
        let open = !collapsed.read().contains(&group);

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
        let theme = settings_theme();
        let route = self.category.route.clone();
        let padding = if self.category.group.is_some() {
            ROW_PADDING
        } else {
            UNGROUPED_PADDING
        };

        let row = SideBarItem::new()
            .theme(row_theme(padding, Some(theme.item_active_background)))
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
        let theme = settings_theme();
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
