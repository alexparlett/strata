use std::collections::HashMap;

use strata_core::config::{Command, Settings};

use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Body, InputTypography};
use freya::components::{use_theme, DragZone};
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
        // This tab's own inline-rename state — local, never shared: whether it's being renamed and the
        // draft name. The context menu / double-click flip `renaming`; the input binds `draft`.
        let mut renaming = use_state(|| false);
        let mut draft = use_state(String::new);
        let a11y = use_a11y();
        let settings = use_consume::<State<Settings>>();
        let closer = use_consume::<crate::apps::project::close::TabCloser>();
        // Entering rename (from the menu or a double-click) just flips `renaming`. We react to that
        // here — in the tab's own scope, so it survives the menu closing: seed the draft with the
        // current name and focus the input (the focus lands after the input has mounted).
        let seed = self.name.clone();
        use_side_effect(move || {
            if renaming() {
                draft.set(seed.clone());
                a11y.request_focus();
            }
        });
        // Hover state for the close slot — a dirty tab's unsaved dot swaps to the × while hovered.
        let mut hovered = use_state(|| false);

        let active = use_reactive(&self.active);

        // Reveal the active tab in the strip: on activation and on our *first* area measurement (a
        // freshly mounted / reopened active tab's area lands a frame after activation), but not on
        // every later area change — torin re-emits `Sized` for every tab on scroll, and re-revealing
        // then would snap the active tab back. So the effect subscribes to `active` and to a memo of
        // *whether* we have an area: a `Memo<bool>` only notifies when `is_some()` flips (None -> Some),
        // never when the `Area` value changes, so scrolling never wakes us. We then peek the area for
        // the reveal. (`scroll_to_item` peeks internally, so its scroll write can't loop us.)
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

        // `hover_background` is themed for the (coming) hover state but not painted in this slice.
        let TabTheme {
            background,
            active_background,
            color,
            active_color,
            accent,
            ..
        } = get_theme!(&self.theme, TabThemePreference, "tab");

        // The unsaved dot is a semantic palette slot; the close × is a flat `Button`, so its icon
        // inherits that button's colour. `text_inverse` is the context menu's separator colour, read
        // here because the menu is built in an event handler (which can't read the theme).
        let theme = use_theme();
        let (dot_color, sep) = {
            let c = &theme.read().colors;
            (c.warning, c.text_inverse)
        };

        let (bg, fg, accent_fill) = if self.active {
            (active_background, active_color, accent)
        } else {
            (background, color, Color::TRANSPARENT)
        };

        // The close affordance: a flat 16×16 icon button (its icon inherits the flat-button colour +
        // hover tint). `stop_propagation` so pressing it closes the tab without also bubbling up to
        // the tab-body switch. Closes through the shared gate — the T2 confirm when this
        // tab's query is in flight. Its tooltip is the comp's dirty-aware `closeTitle`.
        let close_button = TooltipContainer::new(Tooltip::new(if self.dirty {
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
                    closer.close(radio, settings, id);
                })
                .child(Icon::new(IconName::Close).size(11.)),
        );

        // Trailing 16×16 close slot. A clean tab shows the × button outright; a dirty tab shows its
        // unsaved dot, which swaps to the × button while the slot is hovered so it stays one click to
        // close. The slot wrapper is always mounted, so it's what detects the hover.
        let show_x = !self.dirty || hovered();
        let close = rect()
            .width(Size::px(16.))
            .height(Size::px(16.))
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .maybe(!show_x, |el| el.child(Dot { color: dot_color }))
            .maybe(show_x, |el| el.child(close_button));

        // The tab's visual + interactions; wrapped in the tab's own `DragZone` below.
        let content = rect()
            // Width unset → the tab hugs its content (name + close + padding). Fixed height (not
            // `fill`) so it survives the hug-content DragZone. Vertical: accent + row.
            .height(Size::px(TAB_HEIGHT))
            .width(Size::auto())
            .vertical()
            .content(Content::Fit)
            .background(bg)
            // Measure ourselves: locally for the reveal (above) + into the shared strip map for drag
            // hit-testing.
            .on_sized(move |e: Event<SizedEventData>| {
                area.set(Some(e.area));
                areas.write().insert(id, e.area);
            })
            // A single click switches; a double-click (left mouse) renames. Right-click = context menu
            // (below). While already editing, the input owns clicks.
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
            // Right-click → this tab's context menu at the cursor, scoped to this tab.
            .on_secondary_down(move |e: Event<PressEventData>| {
                e.stop_propagation();
                ContextMenu::open_from_event(
                    &e,
                    super::menu::tab_context_menu(id, radio, sep, renaming, closer, settings),
                );
            })
            // While renaming: Escape cancels (consumed — an Esc that ends a rename must
            // not also cancel a running query further down the dismiss chain); a press
            // anywhere outside the tab commits (like a blur).
            .maybe(*renaming.read(), |el| {
                el.on_global_key_down(crate::keymap::on_command(
                    settings,
                    Command::Cancel,
                    move || {
                        renaming.set(false);
                        true
                    },
                ))
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
            // 2px top accent bar (active only) — the pinned-child idiom for a single edge.
            .child(
                rect()
                    .width(Size::fill_minimum())
                    .height(Size::px(2.))
                    .min_height(Size::px(2.))
                    .background(accent_fill),
            )
            // The label row fills the rest and centres vertically (padding 0 sp-4 · gap sp-3): the
            // inline rename input while editing, else the name + close slot.
            .child({
                let row = rect()
                    .height(Size::flex(1.))
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .padding(Gaps::new(0., 12., 0., 12.))
                    .spacing(8.);
                if *renaming.read() {
                    // A fixed-width box that scrolls when the name is longer — matching the Dioxus
                    // `.tab-rename` (118px, body font via `InputTypography` — the `Input` paints
                    // no font of its own) so the text matches the tab name. Enter commits
                    // (`on_submit`); Escape / click-outside are handled on the tab root.
                    row.child(
                        InputTypography::body(
                            Input::new(draft)
                                .a11y_id(a11y)
                                .flat()
                                .compact()
                                .auto_focus(true)
                                .width(Size::px(118.))
                                .on_submit(move |value: String| {
                                    radio.write().rename(id, value);
                                    renaming.set(false);
                                }),
                        ),
                    )
                } else {
                    row.child(Body::new(self.name.clone()).color(fg)).child(close)
                }
            });

        // The tab owns its own drag: the ghost is a matching `TabChrome`, the tab collapses out of the
        // strip while dragging (`show_while_dragging(false)`), and dragging is disabled while renaming.
        DragZone::new(id, content)
            .drag_element(
                rect()
                    .height(Size::px(TAB_HEIGHT))
                    .child(TabChrome::new(self.name.clone(), self.active, self.dirty)),
            )
            .show_while_dragging(false)
            .enabled(!*renaming.read())
            .key(id)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
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
        Self { name, active, dirty, theme: None }
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
            ..
        } = get_theme!(&self.theme, TabThemePreference, "tab");

        let (dot_color, x_color) = {
            let c = &use_theme().read().colors;
            (c.warning, c.disabled)
        };

        let (bg, fg, accent_fill) = if self.active {
            (active_background, active_color, accent)
        } else {
            (background, color, Color::TRANSPARENT)
        };

        // Static close glyph — the tab's resting look: unsaved dot when dirty, else ×.
        let close = rect()
            .width(Size::px(16.))
            .height(Size::px(16.))
            .main_align(Alignment::Center)
            .cross_align(Alignment::Center)
            .maybe(self.dirty, |el| el.child(Dot { color: dot_color }))
            .maybe(!self.dirty, |el| {
                el.child(label().text("×").font_size(13.).color(x_color))
            });

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
                    .padding(Gaps::new(0., 12., 0., 12.))
                    .spacing(8.)
                    .child(Body::new(self.name.clone()).color(fg))
                    .child(close),
            )
    }
}
