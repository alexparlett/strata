//! The **command palette** (P6-01, ⌘K) — the window's one discovery surface, and the only place
//! a command with no chord, or one the user has rebound away, can still be reached by name.
//!
//! Three parts: `model` is what it can offer and what a query narrows that to (pure, unit-tested);
//! `row` is how one result looks; and this file is the surface — the always-mounted ⌘K node, the
//! overlay card it raises, and the `command_palette` theme both wear.
//!
//! **The chord node and the overlay are different nodes.** [`CommandPalette`] is mounted for the
//! project's whole life and draws one thing when closed: the rect carrying the ⌘K listener. Only
//! [`PaletteOverlay`] is conditional, which is what resets the query and the active row per open by
//! construction. Two nodes rather than one because an element holds one handler per event name, and
//! the overlay needs a `GlobalKeyDown` barrier of its own.
//!
//! Nothing on the always-mounted node subscribes to a store: a palette that re-rendered the project
//! root on every keystroke in every tab would be an expensive way to draw nothing.
//!
//! **The search field owns the keyboard.** Freya's `Input` `stop_propagation`s *and*
//! `prevent_default`s every key but Enter/Escape/Tab, and `prevent_default` on `KeyDown` cancels
//! the derived `GlobalKeyDown` — which is what makes the palette a genuine modal barrier, but also
//! means ↑↓, Enter, Escape and ⌘K have to be handled in `on_pre_key_down` (see [`PaletteKey`]).
//!
//! **Swallowing a chord is not the default; it has to be done.** `freya-edit` inserts a
//! `Key::Character` whatever modifiers are held, so a barrier that let unrecognised keys through
//! would have ⌘S type an "s" into the query. [`PaletteKey::Inert`] closes that — and it is why
//! editing chords are deliberately *not* swallowed: ⌘A and ⌘V belong to the field.

mod model;
mod row;

use std::rc::Rc;

use freya::components::{
    define_theme, get_theme, use_scroll_controller, Input, ScrollConfig, ScrollController,
    ScrollView,
};
use freya::prelude::*;
use freya::radio::use_radio_station;
use strata_core::config::{Command, Settings};
use strata_core::keymap::{hint, resolve};
use strata_model::RightPane;

use self::model::{Entry, Index};
use self::row::{GroupHead, PaletteRow};
use crate::apps::project::commands::{use_palette_ctx, PaletteCtx};
use crate::apps::project::state::{Chan, ProjChan, ProjectState};
use crate::apps::project::views::{open_saved_query, view_row};
use crate::components::divider::Divider;
use crate::components::icon::{Icon, IconName};
use crate::components::keycap::KeyCap;
use crate::components::metrics::{R_4, SP_2, SP_3, SP_4, SP_5, SP_8};
use crate::components::typography::{InputTypography, Meta, MonoValue};
use crate::keymap::{chord_from_event, on_command};
use crate::state::{use_config, use_config_station, ConfigChan};

define_theme!(
    %[no_ext]
    %[component]
    pub CommandPalette {
        %[fields]
        /// The card itself (canvas `--c-pop`) and its hairline.
        background: Color,
        border_fill: Color,
        /// The scrim over the window behind it.
        backdrop: Color,
        /// A group heading, and the footer's legend — both the canvas's `--c-label`.
        label_color: Color,
        /// The active row's fill, and the two text tones a row moves between.
        row_active_background: Color,
        row_active_color: Color,
        row_color: Color,
        /// A resting row's glyph (the active one takes the accent) and its mono detail.
        icon_color: Color,
        sub_color: Color,
        /// The `ESC` chip beside the search field — a step back from a row's own hint, because
        /// it labels the way out rather than answering the query.
        esc_color: Color,
        /// The card's drop shadow (canvas `0 32px 90px rgba(0,0,0,.62)`).
        shadow: Color,
    }
);

/// This window's palette dress.
pub fn palette_theme() -> CommandPaletteTheme {
    get_theme!(
        &None::<CommandPaletteThemePartial>,
        CommandPaletteThemePreference,
        "command_palette"
    )
}

/// The card (canvas: 640 wide, 92vw and 62vh ceilings, `--r-4` corners), sat `TOP_INSET` down the
/// window rather than centred — a palette grows downward as you type, and a centred one would
/// slide its first row out from under the pointer.
const CARD_WIDTH: f32 = 640.;
const CARD_MAX_WIDTH: f32 = 92.;
const CARD_MAX_HEIGHT: f32 = 62.;
const CARD_RADIUS: f32 = R_4;
const TOP_INSET: f32 = SP_4;
/// The search row (canvas 54px, `0 var(--sp-5)`, `gap: var(--sp-4)`) and its glyph.
const SEARCH_HEIGHT: f32 = 54.;
const SEARCH_INSET: f32 = SP_5;
const SEARCH_GAP: f32 = SP_4;
const SEARCH_ICON: f32 = 17.;
/// The results body's own inset (canvas `var(--sp-3)`).
const BODY_INSET: f32 = SP_3;
/// The footer legend (canvas 38px, `0 var(--sp-5)`, `gap: var(--sp-5)`).
const FOOTER_HEIGHT: f32 = 38.;
const FOOTER_GAP: f32 = SP_5;
/// The empty state's breathing room and its glyph (canvas `var(--sp-8) 0`, 26px).
const EMPTY_INSET: f32 = SP_8;
const EMPTY_ICON: f32 = 26.;

const PLACEHOLDER: &str = "Search tables, columns, views \u{2014} or run a command\u{2026}";

/// Whether the palette is up — the slot the project root provides and the header's search button
/// writes, on the same terms as [`ConfigureRequest`](super::ConfigureRequest) and the two confirm
/// slots beside it.
///
/// A **named** alias rather than a bare `State<bool>`, because context resolves by type: an
/// unnamed one is the shape most likely to be silently answered by somebody else's flag later.
pub type PaletteOpen = State<bool>;

/// The palette, mounted for the project's life. Draws its ⌘K listener always and the card only
/// while `open` — see the module doc.
#[derive(PartialEq)]
pub struct CommandPalette {
    pub open: PaletteOpen,
}

impl Component for CommandPalette {
    fn render(&self) -> impl IntoElement {
        let config = use_config_station();
        let mut open = self.open;

        let ctx = use_palette_ctx();
        let run = EventHandler::new(move |entry: Entry| {
            perform(&ctx, &entry);
            open.set(false);
        });

        rect()
            .on_global_key_down(on_command(config, Command::CommandPalette, move || {
                let showing = *open.peek();
                open.set(!showing);
                true
            }))
            .maybe(*self.open.read(), |el| {
                el.child(PaletteOverlay {
                    open: self.open,
                    run: run.clone(),
                })
            })
    }
}

/// The card. Mounted only while open, so its query and its active row are fresh every time.
#[derive(PartialEq)]
struct PaletteOverlay {
    open: PaletteOpen,
    /// Perform an entry and dismiss — owned by [`CommandPalette`], which outlives this card. See
    /// the note there.
    run: EventHandler<Entry>,
}

impl Component for PaletteOverlay {
    fn render(&self) -> impl IntoElement {
        let theme = palette_theme();
        let config_station = use_config_station();
        let settings = use_config(ConfigChan::Settings);

        let project = use_radio_station::<ProjectState, ProjChan>();
        let offer = use_hook(move || Rc::new(Index::new(&project.peek())));

        let query = use_state(String::new);
        let active = use_state(|| 0usize);
        let controller = use_scroll_controller(ScrollConfig::default);

        let results = offer.search(&query.read());
        let rows = &results.rows;
        let current = (*active.read()).min(rows.len().saturating_sub(1));

        let open = self.open;
        let close = move || {
            let mut open = open;
            open.set(false);
        };
        let run = {
            let rows = rows.clone();
            let owner = self.run.clone();
            EventHandler::new(move |index: usize| {
                if let Some(entry) = rows.get(index) {
                    owner.call(entry.clone());
                }
            })
        };
        let len = rows.len();
        let step = move |delta: isize| {
            if len == 0 {
                return;
            }
            let mut active = active;
            active.set((current as isize + delta).rem_euclid(len as isize) as usize);
        };

        let card = rect()
            .width(Size::px(CARD_WIDTH))
            .max_width(Size::window_percent(CARD_MAX_WIDTH))
            .max_height(Size::window_percent(CARD_MAX_HEIGHT))
            .corner_radius(CARD_RADIUS)
            .background(theme.background)
            .border(Border::new().width(1.).fill(theme.border_fill))
            .shadow(Shadow::new().y(32.).blur(90.).color(theme.shadow))
            .overflow(Overflow::Clip)
            .content(Content::Flex)
            .vertical()
            .a11y_role(AccessibilityRole::Dialog)
            .child(SearchRow {
                query,
                on_key: EventHandler::new({
                    let run = run.clone();
                    move |key: PaletteKey| match key {
                        PaletteKey::Toggle | PaletteKey::Dismiss => close(),
                        PaletteKey::Down => step(1),
                        PaletteKey::Up => step(-1),
                        PaletteKey::Pick => run.call(current),
                        PaletteKey::Typed => {
                            let mut active = active;
                            active.set(0);
                        }
                        PaletteKey::Caret | PaletteKey::Inert => {}
                    }
                }),
            })
            .child(Divider::horizontal().color(theme.border_fill))
            .child(
                ScrollView::new_controlled(controller)
                    .height(Size::flex(1.))
                    .scroll_with_arrows(false)
                    .child(match results.is_empty() {
                        true => NoMatches.into_element(),
                        false => {
                            let mut body = rect()
                                .width(Size::fill())
                                .vertical()
                                .padding(Gaps::new(BODY_INSET, BODY_INSET, BODY_INSET, BODY_INSET));
                            let set_active = EventHandler::new(move |i| {
                                let mut active = active;
                                active.set(i);
                            });
                            let settings = settings.read();
                            for (index, entry) in rows.iter().enumerate() {
                                if let Some(group) = results.heading(index) {
                                    body = body.child(GroupHead { group });
                                }
                                body = body.child(Reveal {
                                    key: entry.id(),
                                    active: index == current,
                                    controller,
                                    row: PaletteRow {
                                        hint: shortcut(entry, &settings.settings),
                                        entry: entry.clone(),
                                        index,
                                        active: index == current,
                                        set_active: set_active.clone(),
                                        run: run.clone(),
                                    }
                                    .into_element(),
                                });
                            }
                            body.into_element()
                        }
                    }),
            )
            .child(Divider::horizontal().color(theme.border_fill))
            .child(Footer);

        rect()
            .layer(Layer::Overlay)
            .position(Position::new_global())
            .on_global_key_down({
                let station = config_station;
                move |e: Event<KeyboardEventData>| {
                    let chord = chord_from_event(&e);
                    let command = chord.and_then(|c| resolve(&station.peek().settings, &c));
                    if matches!(command, Some(Command::CommandPalette | Command::Cancel)) {
                        close();
                    }
                    e.prevent_default();
                }
            })
            .child(
                rect()
                    .on_press(move |_| close())
                    .position(Position::new_global().top(0.).left(0.))
                    .width(Size::window_percent(100.))
                    .height(Size::window_percent(100.))
                    .background(theme.backdrop),
            )
            .child(
                rect()
                    .position(Position::new_global().top(0.).left(0.))
                    .width(Size::window_percent(100.))
                    .height(Size::window_percent(100.))
                    .cross_align(Alignment::Center)
                    .vertical()
                    .child(rect().height(Size::window_percent(TOP_INSET)))
                    .child(card),
            )
    }
}

/// What a key in the search field meant to the palette, once the chord has been resolved.
#[derive(Clone, PartialEq, Debug)]
enum PaletteKey {
    /// ⌘K again — the command is a toggle.
    Toggle,
    Dismiss,
    Up,
    Down,
    Pick,
    /// A key that changes the query — the field is about to take it as text, and the lit row
    /// goes back to the top because the list is about to be re-narrowed.
    Typed,
    /// Caret movement inside the query: the field's, but it changes no text, so the lit row
    /// stays where the user put it.
    Caret,
    /// A key the palette neither uses nor wants typed: another command's chord, or a key with no
    /// text in it (Tab, PageUp, a bare modifier). Swallowed, and the lit row left alone.
    ///
    /// The chord half is what makes the palette a real barrier rather than one in name only:
    /// Freya's editable inserts a `Key::Character` **whatever modifiers are held** (only the six
    /// `EditBindings` chords are consumed ahead of it, `freya-edit`'s `text_editor.rs`), so
    /// without this ⌘S would type an "s" into the query. Editing chords are deliberately *not*
    /// here — those are the field's own.
    Inert,
}

/// The field, and the whole keyboard model with it (module doc).
///
/// It reports a [`PaletteKey`] and nothing else — an outcome rather than an `Event<T>`, because
/// nothing it hands back is the keyboard event, it is what the palette decided that event meant
/// (`Dialog` types its dismiss/confirm the same way). It deliberately does not report the row
/// count either: the handler that consumes this is defined in the same scope that built the list.
#[derive(PartialEq)]
struct SearchRow {
    query: State<String>,
    on_key: EventHandler<PaletteKey>,
}

impl Component for SearchRow {
    fn render(&self) -> impl IntoElement {
        let theme = palette_theme();
        let icon_color = theme.icon_color;
        let config = use_config_station();
        let on_key = self.on_key.clone();

        rect()
            .height(Size::px(SEARCH_HEIGHT))
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(SEARCH_GAP)
            .padding(Gaps::new(0., SEARCH_INSET, 0., SEARCH_INSET))
            .child(
                Icon::new(IconName::Search)
                    .size(SEARCH_ICON)
                    .color(icon_color),
            )
            .child(
                InputTypography::body(
                    Input::new(self.query)
                        .flat()
                        .auto_focus(true)
                        .width(Size::flex(1.))
                        .placeholder(PLACEHOLDER)
                        .on_pre_key_down(move |e: Event<KeyboardEventData>| {
                            let chord = chord_from_event(&e);
                            let command = chord.and_then(|c| resolve(&config.peek().settings, &c));
                            let key = match (command, &e.key) {
                                (Some(Command::CommandPalette), _) => PaletteKey::Toggle,
                                (Some(Command::Cancel), _) => PaletteKey::Dismiss,
                                (_, Key::Named(NamedKey::ArrowDown)) => PaletteKey::Down,
                                (_, Key::Named(NamedKey::ArrowUp)) => PaletteKey::Up,
                                (_, Key::Named(NamedKey::Enter)) => PaletteKey::Pick,
                                (Some(cmd), _) if !cmd.is_edit() => PaletteKey::Inert,
                                (
                                    _,
                                    Key::Named(
                                        NamedKey::ArrowLeft
                                        | NamedKey::ArrowRight
                                        | NamedKey::Home
                                        | NamedKey::End,
                                    ),
                                ) => PaletteKey::Caret,
                                (
                                    _,
                                    Key::Character(_)
                                    | Key::Named(NamedKey::Backspace | NamedKey::Delete),
                                ) => PaletteKey::Typed,
                                _ => PaletteKey::Inert,
                            };
                            let to_field = matches!(key, PaletteKey::Typed | PaletteKey::Caret);
                            on_key.call(key);
                            e.stop_propagation();
                            e.prevent_default();
                            to_field
                        }),
                )
                .width(Size::flex(1.)),
            )
            .child(KeyCap::chip("ESC").color(theme.esc_color))
    }
}

/// Scroll the lit row into view — the Engine grid's recipe, for the same reason: a row's area
/// lands a frame after it lights up, so the effect watches *whether* there is one and then peeks
/// it. `scroll_to_item` is a no-op once the row is fully visible, so this is safe every render.
///
/// Hover writes the same slot as ↑↓, so hovering a partly visible row nudges it in. That is the
/// accepted cost of one active row rather than two.
#[derive(PartialEq)]
struct Reveal {
    /// The entry this row shows — its diff key, so the measured area below belongs to *that*
    /// entry rather than to the position it happened to occupy.
    key: String,
    active: bool,
    controller: ScrollController,
    row: Element,
}

impl Component for Reveal {
    fn render_key(&self) -> DiffKey {
        DiffKey::from(&self.key)
    }

    fn render(&self) -> impl IntoElement {
        let mut area = use_state(|| None::<Area>);
        let has_area = use_memo(move || area.read().is_some());
        let active = use_reactive(&self.active);
        let controller = self.controller;
        use_side_effect(move || {
            if !*active.read() || !has_area() {
                return;
            }
            if let Some(area) = *area.peek() {
                let mut controller = controller;
                controller.scroll_to_item(area);
            }
        });

        rect()
            .width(Size::fill())
            .on_sized(move |e: Event<SizedEventData>| area.set(Some(e.area)))
            .child(self.row.clone())
    }
}

/// Nothing matched. A glyph and a line — there is nothing useful to suggest, and a palette that
/// proposes something when it has no answer is worse than one that says so.
#[derive(PartialEq)]
struct NoMatches;

impl Component for NoMatches {
    fn render(&self) -> impl IntoElement {
        let theme = palette_theme();
        rect()
            .width(Size::fill())
            .vertical()
            .cross_align(Alignment::Center)
            .spacing(SP_3)
            .padding(Gaps::new(EMPTY_INSET, 0., EMPTY_INSET, 0.))
            .child(
                Icon::new(IconName::Search)
                    .size(EMPTY_ICON)
                    .color(theme.label_color),
            )
            .child(Meta::new("No matches").color(theme.label_color))
    }
}

/// The legend under the list. Static: it describes keys, not state.
#[derive(PartialEq)]
struct Footer;

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let theme = palette_theme();
        let key = theme.sub_color;
        let legend = |cap: &str, what: &str| {
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(SP_2)
                .child(MonoValue::new(cap.to_string()).color(key))
                .child(Meta::new(what.to_string()).color(theme.label_color))
        };

        rect()
            .height(Size::px(FOOTER_HEIGHT))
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .spacing(FOOTER_GAP)
            .padding(Gaps::new(0., SEARCH_INSET, 0., SEARCH_INSET))
            .child(legend("\u{2191}\u{2193}", "navigate"))
            .child(legend("\u{21b5}", "select"))
            .child(legend("esc", "close"))
    }
}

/// The chord an entry answers to, for its row's hint. Only a command has one — a table is not
/// something you can bind a key to.
fn shortcut(entry: &Entry, settings: &Settings) -> Option<String> {
    match entry {
        Entry::Action(action) => action.route().key.map(|cmd| hint(settings, cmd)),
        _ => None,
    }
}

/// Run an entry. Each arm is the gesture the catalog or the registry already owns — never a
/// second implementation of one (see `commands`).
fn perform(ctx: &PaletteCtx, entry: &Entry) {
    match entry {
        Entry::Action(action) => action.run(ctx),
        Entry::Table { name, .. } | Entry::View { name, .. } => view_row(&ctx.catalog, name),
        Entry::Query { id, .. } => open_saved_query(&ctx.catalog, *id),
        Entry::Column { col, .. } => {
            let mut selection = ctx.selection;
            selection.set(Some(col.clone()));
            let mut session = ctx.catalog.session;
            session
                .write_channel(Chan::Layout)
                .open_right_pane(RightPane::Inspector);
        }
    }
}
