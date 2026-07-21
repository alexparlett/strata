use freya::components::use_theme;
use freya::prelude::*;
use freya::radio::{use_radio, Radio};

use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Caption, Prose};

/// A flat 28×28 icon button — the cluster's building block. The icon takes no explicit colour, so it
/// inherits the button's (hover-reactive) `color`. Callers add the `.on_press`.
fn cluster_button(icon: IconName, icon_size: f32) -> Button {
    Button::new()
        .flat()
        .width(Size::px(28.))
        .height(Size::px(28.))
        .child(Icon::new(icon).size(icon_size))
}

/// The tab strip's pinned right cluster: **new-query** · **quick-navigate** · **overflow**. The two
/// menus are their own components below — this is just the row.
#[derive(PartialEq)]
pub struct TabControls;

impl TabControls {
    pub fn new() -> Self {
        Self
    }
}

impl Component for TabControls {
    fn render(&self) -> impl IntoElement {
        let mut radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let new_tab = cluster_button(IconName::Plus, 15.).on_press(move |_| {
            radio.write().open_blank();
        });

        rect()
            .horizontal()
            .cross_align(Alignment::Center)
            .height(Size::fill())
            .spacing(2.)
            .padding(Gaps::new(0., 8., 0., 8.))
            .child(new_tab)
            .child(NavMenu)
            .child(OverflowMenu)
    }
}

/// Resolved colours for the quick-navigate rows, read off the active theme. `Copy` (all `Color`), so
/// it's passed by value into the row builders — no borrow to leak into the returned `impl IntoElement`.
#[derive(Clone, Copy)]
struct NavPalette {
    dot_dirty: Color,
    dot_active: Color,
    dot_idle: Color,
    active_fg: Color,
    row_fg: Color,
    faint: Color,
}

impl NavPalette {
    fn read() -> Self {
        let theme = use_theme();
        let c = &theme.read().colors;
        Self {
            dot_dirty: c.warning,
            dot_active: c.primary,
            dot_idle: c.text_placeholder,
            active_fg: c.text_primary,
            row_fg: c.text_secondary,
            faint: c.text_placeholder,
        }
    }

    /// The status-dot colour for a tab (dirty wins over active over idle).
    fn dot(&self, active: bool, dirty: bool) -> Color {
        if dirty {
            self.dot_dirty
        } else if active {
            self.dot_active
        } else {
            self.dot_idle
        }
    }

    /// The name colour for a row (brighter when it's the active tab). The row *background* is the
    /// `MenuItem`'s job (hover / select), not ours.
    fn name_fg(&self, active: bool) -> Color {
        if active {
            self.active_fg
        } else {
            self.row_fg
        }
    }
}

/// The quick-navigate dropdown: a ⌄ trigger opening a searchable tab switcher — a filter box over one
/// [`tab_row`] per open tab, capped at 10 with a "+N more" hint. Anchored bottom-end so it opens
/// down-and-left (no right-edge overflow).
#[derive(PartialEq)]
struct NavMenu;

impl Component for NavMenu {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);
        let query = use_state(String::new);
        let radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let palette = NavPalette::read();

        // Open tabs + status, filtered by the search box. Read once; the guard drops before we build.
        // The filter runs over *all* tabs, in strip order.
        let needle = query.read().to_lowercase();
        let matches: Vec<(TabId, String, bool, bool)> = {
            let s = radio.read();
            s.order
             .iter()
             .filter_map(|id| {
                 s.tabs
                  .get(id)
                  .map(|t| (*id, t.name.clone(), s.active == Some(*id), t.is_dirty()))
             })
             .filter(|(_, name, _, _)| needle.is_empty() || name.to_lowercase().contains(&needle))
             .collect()
        };
        let is_empty = matches.is_empty();
        // Always cap the visible list at 10; when more than 10 match you narrow with a more specific
        // name. The filter runs over *all* tabs (not just the first 10), so any tab — including ones
        // past the first 10 in the strip — is reachable by name. (Dioxus sorts by last-viewed, which
        // the Freya session doesn't track yet.)
        let overflow = matches.len().saturating_sub(10);

        let rows = matches.into_iter().take(10).fold(
            rect().vertical().spacing(2.),
            |col, (id, name, active, dirty)| {
                col.child(tab_row(id, name, active, dirty, palette, radio, open))
            },
        );

        let panel = rect()
            .vertical()
            .width(Size::px(300.))
            .child(nav_search(query, palette.faint))
            .maybe(is_empty, |el| el.child(nav_notice("No matching tabs", palette.faint)))
            .maybe(!is_empty, |el| el.child(rows))
            .maybe(overflow > 0, |el| {
                el.child(nav_notice(
                    format!("+{overflow} more..."),
                    palette.faint,
                ))
            });

        Attached::new(cluster_button(IconName::ChevronDown, 14.).on_press(move |_| open.toggle()))
            .bottom()
            .align_end()
            .maybe_child(open().then(|| Menu::new().on_close(move |_| open.set(false)).child(panel)))
    }
}

/// The search row: a single filter input with the magnifier *inside* it (Freya's `Input.leading`).
/// The input paints no font of its own, so it inherits the UI body font we set here (family + 12.5);
/// text/placeholder colours come from the `input` theme (`text_primary` / `text_placeholder`).
fn nav_search(query: State<String>, faint: Color) -> impl IntoElement {
    rect()
        .width(Size::fill())
        .padding(Gaps::new(8., 8., 8., 8.))
        .font_family("IBM Plex Sans")
        .font_size(12.5)
        .child(
            Input::new(query)
                .leading(Icon::new(IconName::Search).color(faint).size(14.))
                .placeholder("Find a query tab…")
                .compact()
                .auto_focus(true)
                .width(Size::fill()),
        )
}

/// One switcher row: status dot + name (flex) + close ×. The row press switches to the tab and
/// closes the menu; the × closes the tab (and stops the press so it doesn't also switch).
fn tab_row(
    id: TabId,
    name: String,
    active: bool,
    dirty: bool,
    palette: NavPalette,
    mut radio: Radio<SessionState, Chan>,
    mut open: State<bool>,
) -> impl IntoElement {
    let fg = palette.name_fg(active);
    let close_fg = palette.faint;
    // `MenuItem` supplies the row background from the `menu_item` theme: transparent → hover_background
    // on hover → select_background when `.selected()` (the blue active highlight).
    MenuItem::new()
        .selected(active)
        .padding((6., 8.))
        .on_press(move |_| {
            radio.write().switch(id);
            open.set(false);
        })
        .child(
            rect()
                .horizontal()
                .content(Content::Flex)
                .cross_align(Alignment::Center)
                .spacing(8.)
                .width(Size::fill())
                .child(Dot {
                    color: palette.dot(active, dirty),
                })
                .child(
                    rect()
                        .width(Size::flex(1.))
                        .child(Prose::new(name).color(fg).text_overflow(TextOverflow::Ellipsis)))
                .child(
                    rect()
                        .width(Size::px(20.))
                        .height(Size::px(20.))
                        .corner_radius(4.)
                        .main_align(Alignment::Center)
                        .cross_align(Alignment::Center)
                        .on_press(move |e: Event<PressEventData>| {
                            e.stop_propagation();
                            radio.write().close_one(id);
                        })
                        .child(Icon::new(IconName::Close).color(close_fg).size(12.)),
                ),
        )
}

/// A centred muted notice line (empty state / "+N more" footer).
fn nav_notice(text: impl Into<String>, faint: Color) -> impl IntoElement {
    rect()
        .width(Size::fill())
        .main_align(Alignment::Center)
        .cross_align(Alignment::Center)
        .padding(Gaps::new(8., 8., 8., 8.))
        .child(Caption::new(text.into()).color(faint))
}

/// The overflow (⋯) dropdown — whole-strip actions. (TODO: disabled reopen when nothing's closed;
/// close-all / close-others.)
#[derive(PartialEq)]
struct OverflowMenu;

impl Component for OverflowMenu {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| false);
        let mut radio = use_radio::<SessionState, Chan>(Chan::Tabs);

        let menu = Menu::new()
            .on_close(move |_| open.set(false))
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        radio.write().close_all();
                        open.set(false);
                    })
                    .child(Prose::new("Close all tabs")),
            )
            .child(
                MenuButton::new()
                    .on_press(move |_| {
                        radio.write().reopen_last();
                        open.set(false);
                    })
                    .child(Prose::new("Reopen closed tab")),
            );

        Attached::new(cluster_button(IconName::Dots, 15.).on_press(move |_| open.toggle()))
            .bottom()
            .align_end()
            .maybe_child(open().then(|| menu))
    }
}
