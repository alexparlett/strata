use std::collections::HashMap;

use crate::state::use_config_station;
use strata_core::config::Command;

use strata_model::TabId;

use super::menu::tab_context_menu;
use crate::apps::project::close::TabCloser;
use crate::apps::project::state::{Chan, SessionState};
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::metrics::{SP_3, SP_4};
use crate::components::tones::tones;
use crate::components::typography::{Body, InputTypography};
use crate::keymap::on_command;
use freya::components::DragZone;
use freya::prelude::*;
use freya::radio::use_radio;

/// The tab (and strip row) height. Fixed rather than `fill` so a tab keeps its height inside the
/// hug-content `DragZone` it wraps itself in for drag-reorder. Matches the tab bar's `px(38)` less its
/// 1px bottom divider.
pub(in crate::apps::project::views::workbench) const TAB_HEIGHT: f32 = 37.0;

define_theme!(
    %[component]
    pub Tab {
        %[fields]
        background: Color,
        hover_background: Color,
        active_background: Color,
        color: Color,
        active_color: Color,
        accent: Color,
        /// The resting close glyph. A field rather than a role read beside the destructure: once a
        /// component has a theme, every colour it paints is one of that theme's own (AGENTS.md §3),
        /// and the × had been the one exception.
        close: Color,
    }
);

/// One tab in the workspace strip. The active tab takes the editor's background + a 2px top accent
/// bar, so it reads as seated over the editor pane below; resting (and, later, hover) colours come
/// from the `tab` theme. The trailing slot is the close affordance — a × on a clean tab, an unsaved
/// dot when the tab is dirty.
///
/// A self-contained unit: it switches / closes / renames itself, owns its own right-click context menu
/// (scoped to itself) and its own [`DragZone`] (so it can disable dragging while renaming). The strip
/// only coordinates the drop target + reorder maths. Rename state is component-local — never shared.
#[derive(PartialEq)]
pub struct Tab {
    id: TabId,
    name: String,
    active: bool,
    dirty: bool,
    /// The strip `ScrollView`'s controller, so an active tab can reveal itself (`scroll_to_item`).
    controller: ScrollController,
    /// Shared strip map the tab reports its measured area into (drag hit-testing).
    areas: State<HashMap<TabId, Area>>,
    key: DiffKey,
    pub theme: Option<TabThemePartial>,
}

impl Tab {
    pub fn new(
        id: TabId,
        name: String,
        active: bool,
        dirty: bool,
        controller: ScrollController,
        areas: State<HashMap<TabId, Area>>,
    ) -> Self {
        Self {
            id,
            name,
            active,
            dirty,
            controller,
            areas,
            key: DiffKey::None,
            theme: None,
        }
        .key(id)
    }
}

impl KeyExt for Tab {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for Tab {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let mut areas = self.areas;
        let mut radio = use_radio::<SessionState, Chan>(Chan::Tabs);
        let mut renaming = use_state(|| false);
        let draft = use_state(String::new);
        let a11y = use_a11y();
        let config = use_config_station();
        let closer = use_consume::<TabCloser>();
        let mut hovered = use_state(|| false);

        let active = use_reactive(&self.active);

        let mut area = use_state(|| None::<Area>);
        let has_area = use_memo(move || area.read().is_some());
        let controller = self.controller;
        use_side_effect(move || {
            let active = *active.read();
            let ready = has_area();
            if active && ready {
                if let Some(a) = *area.peek() {
                    let mut controller = controller;
                    controller.scroll_to_item(a);
                }
            }
        });

        let TabTheme {
            background,
            active_background,
            color,
            active_color,
            accent,
            ..
        } = get_theme!(&self.theme, TabThemePreference, "tab");

        let dot_color = tones().warning;

        let (bg, fg, accent_fill) = if self.active {
            (active_background, active_color, accent)
        } else {
            (background, color, Color::TRANSPARENT)
        };

        let close_button = TooltipContainer::new(Tooltip::new_text(if self.dirty {
            "Unsaved changes — click to close"
        } else {
            "Close tab"
        }))
        .position(AttachedPosition::Bottom)
        .child(
            Button::new()
                .flat()
                .width(Size::px(16.))
                .height(Size::px(16.))
                .on_press(move |e: Event<PressEventData>| {
                    e.stop_propagation();
                    closer.close(radio, config, id);
                })
                .child(Icon::new(IconName::Close).size(11.)),
        );

        let show_x = !self.dirty || hovered();
        let close = rect()
            .width(Size::px(16.))
            .height(Size::px(16.))
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .maybe(!show_x, |el| el.child(Dot::new(dot_color)))
            .maybe(show_x, |el| el.child(close_button));

        let content = rect()
            .height(Size::px(TAB_HEIGHT))
            .width(Size::auto())
            .vertical()
            .content(Content::Fit)
            .background(bg)
            .on_sized(move |e: Event<SizedEventData>| {
                area.set(Some(e.area));
                areas.write().insert(id, e.area);
            })
            .on_press(move |e: Event<PressEventData>| {
                if *renaming.read() {
                    return;
                }
                if let PressEventData::Mouse(m) = e.data() {
                    if m.button != Some(MouseButton::Left) {
                        return;
                    }
                    if EventsCombos::pressed(m.global_location).is_double() {
                        renaming.set(true);
                        return;
                    }
                }
                radio.write().switch(id);
            })
            .on_secondary_down(move |e: Event<PressEventData>| {
                e.stop_propagation();
                ContextMenu::open_from_down(tab_context_menu(id, radio, renaming, closer, config));
            })
            .maybe(*renaming.read(), |el| {
                el.on_global_key_down(on_command(config, Command::Cancel, move || {
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
                            radio.write().rename(id, draft.peek().clone());
                            renaming.set(false);
                        }
                    }
                })
            })
            .child(
                rect()
                    .width(Size::fill_minimum())
                    .height(Size::px(2.))
                    .min_height(Size::px(2.))
                    .background(accent_fill),
            )
            .child({
                let row = rect()
                    .height(Size::flex(1.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .padding(Gaps::new(0., SP_4, 0., SP_4))
                    .spacing(SP_3);
                if *renaming.read() {
                    row.child(TabRename {
                        id,
                        name: self.name.clone(),
                        draft,
                        renaming,
                        a11y,
                    })
                } else {
                    row.child(Body::new(self.name.clone()).color(fg))
                        .child(close)
                }
            });

        DragZone::new(id, content)
            .drag_element(rect().height(Size::px(TAB_HEIGHT)).child(TabChrome::new(
                self.name.clone(),
                self.active,
                self.dirty,
            )))
            .show_while_dragging(false)
            .enabled(!*renaming.read())
            .key(id)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The tab's inline-rename field: a fixed-width box that scrolls when the name is longer —
/// matching the Dioxus `.tab-rename` (118px, body font via [`InputTypography`], since the `Input`
/// paints no font of its own) so the text matches the tab name. Enter commits; Escape and
/// click-outside are the tab root's, which still owns `draft`.
///
/// **Its own component so the draft is seeded before the input mounts.** `Input` takes its
/// starting selection from the value it is created with, so seeding reactively — as this did —
/// hands it an empty string and then syncs the name in as a plain edit, leaving the caret at
/// position 0 and the first keystroke landing *in front of* the name being renamed. Mounting only
/// while renaming makes `use_hook` the honest place to seed, and the selection lands with it.
#[derive(PartialEq)]
struct TabRename {
    id: TabId,
    name: String,
    draft: State<String>,
    renaming: State<bool>,
    a11y: AccessibilityId,
}

impl Component for TabRename {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let mut draft = self.draft;
        let mut renaming = self.renaming;
        let mut radio = use_radio::<SessionState, Chan>(Chan::Tabs);

        let seed = self.name.clone();
        use_hook(move || draft.set(seed.clone()));

        InputTypography::body(
            Input::new(draft)
                .a11y_id(self.a11y)
                .flat()
                .compact()
                .auto_focus(true)
                .select_all_on_init(true)
                .width(Size::px(118.))
                .on_submit(move |value: String| {
                    radio.write().rename(id, value);
                    renaming.set(false);
                }),
        )
    }
}

/// The static visual of a tab — background, top accent, name, trailing dot/× — with no interactivity,
/// hooks or measurement. Used as the drag ghost so it matches a real [`Tab`] exactly (same `tab`
/// theme, structure and padding).
#[derive(PartialEq)]
pub struct TabChrome {
    name: String,
    active: bool,
    dirty: bool,
    pub theme: Option<TabThemePartial>,
}

impl TabChrome {
    pub fn new(name: String, active: bool, dirty: bool) -> Self {
        Self {
            name,
            active,
            dirty,
            theme: None,
        }
    }
}

impl Component for TabChrome {
    fn render(&self) -> impl IntoElement {
        let TabTheme {
            background,
            active_background,
            color,
            active_color,
            accent,
            close,
            ..
        } = get_theme!(&self.theme, TabThemePreference, "tab");

        let (dot_color, x_color) = (tones().warning, close);

        let (bg, fg, accent_fill) = if self.active {
            (active_background, active_color, accent)
        } else {
            (background, color, Color::TRANSPARENT)
        };

        let close = rect()
            .width(Size::px(16.))
            .height(Size::px(16.))
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .maybe(self.dirty, |el| el.child(Dot::new(dot_color)))
            .maybe(!self.dirty, |el| el.child(Body::new("×").color(x_color)));

        rect()
            .height(Size::px(TAB_HEIGHT))
            .width(Size::auto())
            .vertical()
            .content(Content::Fit)
            .background(bg)
            .child(
                rect()
                    .width(Size::fill_minimum())
                    .height(Size::px(2.))
                    .min_height(Size::px(2.))
                    .background(accent_fill),
            )
            .child(
                rect()
                    .height(Size::flex(1.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .padding(Gaps::new(0., SP_4, 0., SP_4))
                    .spacing(SP_3)
                    .child(Body::new(self.name.clone()).color(fg))
                    .child(close),
            )
    }
}
