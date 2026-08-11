//! The composer — what the user pinned, what they are typing, and **what this conversation is
//! talking to**.
//!
//! ## The footer is a per-conversation pick, not a setting
//!
//! Model and effort here override Settings' defaults for *this* conversation and never write
//! them back: changing the model mid-conversation is a decision about this conversation, and a
//! control that moved what every new chat starts on would be a different control entirely.
//!
//! **Provider is chosen by choosing a model**, grouped under the enabled providers — one control
//! rather than two, because a model belongs to exactly one provider and a picker that let them
//! disagree would offer selections that cannot be sent. The offer is `Listings::offer`, the one
//! copy of the reported ∪ {the current pick} rule (AS-06), so an endpoint with no `/models`
//! cannot strand a working setup.
//!
//! **Effort renders only when the *model* has rungs.** Reasoning is a model capability, so
//! changing model within one provider can add or remove the control — absent, never disabled,
//! because a dimmed control implies a value you could reach.
//!
//! ## A focused `Input` owns the keyboard
//!
//! So the `@`-completion is driven from **this field** rather than from the popup (AGENTS.md §3):
//! the arrow keys reach the focused input and nothing else, and moving focus to the list would
//! stop the very typing that narrows it. `on_pre_key_down` claims the three keys the list is
//! entitled to and hands everything else back to `Input::key_down_default` — the field's own rule,
//! called rather than restated, so what stops a keystroke reaching the window's global listeners
//! stays in one place. Enter is `on_submit` whenever the list is not up.
//!
//! ## Send becomes stop while a turn streams
//!
//! One control, because they are one decision. Stopping drops the turn's task, which *is* AS-02's
//! cancel and the engine's abort; the transcript keeps what had streamed, marked stopped.

use freya::components::{Menu, MenuButton, Tooltip, TooltipContainer};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_agent::assistant::{efforts, label};
use strata_core::ai::{Ai, Effort, ProviderKind};

use super::mention::{AttachPicker, MentionPicker, Mentions};
use super::ChatTheme;
use crate::apps::project::state::{
    blocked, send, store, use_report, Anchor, Chan, ChatsCtx, Pick, ProjChan, ProjectState,
    SessionState, Stores,
};
use crate::components::icon::{Icon, IconName};
use crate::components::tool_button::ToolButton;
use crate::components::typography::{InputTypography, Meta, Prose};
use crate::state::{needs_asking, refresh, use_config, AppCtx, Ask, ConfigChan};
use crate::theme::{use_roles, Role};

/// The composer's inset and the gap between its rows (canvas `--sp-4` / `--sp-3`).
const PAD: Gaps = Gaps::new_all(14.);
const ROW_GAP: f32 = 8.;
/// The model dropdown's width, and the room a name has inside it.
const MODEL_MENU_W: f32 = 250.;
const MODEL_ROW_CHROME: f32 = 40.;
/// The provider list is names only, so it needs less room than the model list.
const PROVIDER_MENU_W: f32 = 170.;
/// How wide a footer trigger's label may run before it ellipsizes. Three of them share the row,
/// so none may take it.
const TRIGGER_MAX_W: f32 = 130.;

#[derive(PartialEq)]
pub struct Composer {
    pub theme: ChatTheme,
    /// The pane's measured height, so the field's ceiling is a fraction of what is on screen
    /// rather than a number picked in the dark. `0.` until the first layout, which
    /// [`ceiling`] reads as "no measurement yet" and floors.
    pub pane_height: State<f32>,
}

impl Component for Composer {
    fn render(&self) -> impl IntoElement {
        let theme = self.theme.clone();
        let roles = use_roles();
        let assistant = use_consume::<crate::apps::project::state::AssistantCtx>();
        let mut chats = use_consume::<ChatsCtx>();
        let config = use_config(ConfigChan::Settings);
        let session = use_radio::<SessionState, Chan>(Chan::Tabs);
        let project = use_radio::<ProjectState, ProjChan>(ProjChan::Tables);
        let mut text = use_state(String::new);
        // The bar draws the focus ring, so it has to know when the field has focus — which means
        // minting the id here and handing it to the `Input`, rather than letting it mint its own.
        let a11y = use_hook(AccessibilityId::new_unique);
        let focus = use_focus(a11y);
        // The expand toggle: half the pane at rest, two thirds expanded — IntelliJ's two stops.
        let mut expanded = use_state(|| false);
        // The `@`-completion, driven from this field because this is where the keyboard is.
        let mut selected = use_state(|| 0usize);
        let dismissed = use_state(|| None::<usize>);
        let caret = use_state(|| 0usize);
        // The catalog's other two channels, read for the subscription: an offer list that did not
        // notice a new view is a list that completes names the next send cannot resolve.
        let views = use_radio::<ProjectState, ProjChan>(ProjChan::Views);
        let queries = use_radio::<ProjectState, ProjChan>(ProjChan::Queries);

        // **The highlight is a property of the token, so the token changing resets it.** Any
        // keystroke re-narrows the list, and the row that was second under the old text has
        // nothing to do with the row that is second under the new. Escape is undone by the same
        // rule, because a mention still being typed is a question not yet finished.
        use_side_effect(move || {
            let _ = text.read();
            let mut dismissed = dismissed;
            selected.set(0);
            dismissed.set(None);
        });

        let ai: Ai = config.read().settings.ai.clone();
        let (id, pick, pinned, running) = {
            let held = chats.read();
            let chat = held.active();
            (
                chat.id,
                chat.pick.clone(),
                chat.pinned.clone(),
                chat.is_running(),
            )
        };
        let refusal = blocked(&assistant, &ai, &pick);

        // What `@` is offering right now. Computed here, once, and handed to both the list that
        // draws it and the key handler that takes from it.
        let mut mentions = Mentions {
            id,
            chats,
            text,
            caret,
            field: a11y,
            selected,
            dismissed,
        };
        let offered = {
            let _ = views.read();
            let _ = queries.read();
            mentions.offered(&project.read(), &session.read())
        };

        // The one funnel both the button and the Enter key press.
        let stores = Stores { session, project };
        let report = use_report();
        let root = project.read().root.clone();
        let mut fire = {
            let ai = ai.clone();
            move || {
                let question = text.peek().clone();
                // **Only a send empties the box.** Every refusal in `send` is silent — a turn
                // already streaming, nothing configured, nothing typed — and clearing on one
                // destroys a message the user still has to send, from an Enter that looked like
                // it worked.
                if send(&assistant, chats, stores, report, &ai, id, question) {
                    text.set(String::new());
                }
            }
        };

        // The chips the next send carries, each removable where it is shown.
        let chips = (!pinned.is_empty()).then(|| Chips {
            id,
            pinned,
            theme: theme.clone(),
        });

        // **The field grows with what is typed, then scrolls.** `Input::multiline` wraps the text
        // and takes its own height from it; `max_height` is where that stops and the text starts
        // scrolling instead. Enter sends and Shift+Enter is a newline — the fork's rule, stated
        // once there rather than re-decided at this call site.
        //
        // **Undressed, because the bar around it is the box.** The field's own background, border
        // and focus ring are cleared per instance rather than in the shared `input` theme (every
        // other flat input in the app still wants them); the bar below carries all three, so
        // there is one outline and it lights up around everything the message is made of —
        // IntelliJ's shape, and the reason the chips and the actions live *inside* it.
        let field = InputTypography::body(
            Input::new(text)
                .flat()
                .a11y_id(a11y)
                .theme_colors(
                    InputColorsThemePartial::new()
                        .background(Color::TRANSPARENT)
                        .focus_background(Color::TRANSPARENT)
                        .border_fill(Color::TRANSPARENT)
                        .focus_border_fill(Color::TRANSPARENT),
                )
                .multiline(true)
                .max_height(ceiling(*self.pane_height.read(), expanded()))
                // **Expanded is a size, not a bigger ceiling.** Raising only the cap leaves an
                // empty box exactly as short as it was, which is what made the toggle look
                // broken: the floor is what makes the press do something with nothing typed.
                .maybe(expanded(), |input| {
                    input.min_height(ceiling(*self.pane_height.read(), true))
                })
                .width(Size::fill())
                .placeholder("Ask about your data…")
                // **Bound two-way**, so an `@`-completion is an edit at a position: the token
                // under the caret is what narrows the list, the token's span is what an accept
                // replaces, and the caret lands after the name. Without it the completion can
                // only be the tail of the buffer, which is wrong the moment a mention is typed
                // mid-sentence.
                .caret(caret)
                // **The completion list claims three keys before the field sees them**, because a
                // focused `Input` owns the keyboard (AGENTS.md §3) and `on_submit` would
                // otherwise send the message the Enter was meant to complete. Everything the list
                // does not take goes to the field's own rule rather than a second copy of it.
                .on_pre_key_down({
                    let offered = offered.clone();
                    move |e: Event<KeyboardEventData>| match mentions.claim(&e, &offered) {
                        true => {
                            e.stop_propagation();
                            e.prevent_default();
                            false
                        }
                        false => Input::key_down_default(e),
                    }
                })
                .on_submit({
                    let mut fire = fire.clone();
                    move |_: String| fire()
                }),
        )
        .width(Size::fill());

        // The bar: the field, then the chips it will send with, then its own actions — one box
        // with one border, which is what the focus ring belongs around.
        let bar = rect()
            .width(Size::fill())
            .vertical()
            .corner_radius(8.)
            .background(theme.card_background)
            .border(Border::new().width(1.).fill(match focus().is_focused() {
                true => roles.get(Role::BorderFocused),
                false => theme.card_border_fill,
            }))
            // A painted border is not laid out, so the inset carries it (AGENTS.md §3).
            .padding(Gaps::new_all(6.))
            .spacing(6.)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .spacing(4.)
                    .child(rect().width(Size::flex(1.)).child(field))
                    // Top-right of the bar, beside the first line rather than centred on a block
                    // that may be twenty lines tall.
                    .child(
                        ToolButton::new(
                            match expanded() {
                                true => IconName::ChevronsDown,
                                false => IconName::ChevronsUp,
                            },
                            match expanded() {
                                true => "Shrink the message box",
                                false => "Expand the message box",
                            },
                        )
                        .on_press(move |_| expanded.toggle()),
                    ),
            )
            .maybe_child(chips)
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(4.)
                    .child(AttachPicker {
                        id,
                        theme: theme.clone(),
                    })
                    .child(rect().width(Size::flex(1.)))
                    .child(match running {
                        true => ToolButton::new(IconName::Stop, "Stop")
                            .color(roles.get(Role::Error))
                            // **A stopped turn is stored too.** `stop` settles the reply marked
                            // cancelled, and a conversation whose last turn was stopped is
                            // exactly as much the user's record as one that answered —
                            // cancelled is never failed, on disk as much as on screen.
                            .on_press(move |_| {
                                chats.write().stop(id);
                                let root = root.clone();
                                spawn_forever(async move { store(&root, chats, report, id).await });
                            }),
                        false => ToolButton::new(IconName::Play, "Send")
                            .color(roles.get(Role::Accent))
                            .enabled(refusal.is_none())
                            .on_press(move |_| fire()),
                    }),
            );

        // The completion list sits **above** the bar, opening upward: the composer is at the
        // bottom of the pane, so a list under it would be off screen. It draws nothing at all
        // unless a mention is being typed and something matches it.
        let bar = Attached::new(bar)
            .top()
            .align_start()
            .offset(4.)
            .child(MentionPicker {
                mentions,
                offered,
                theme: theme.clone(),
            });

        rect()
            .width(Size::fill())
            .vertical()
            .padding(PAD)
            .spacing(ROW_GAP)
            .child(bar)
            // What is missing, named — never a dead send button.
            .maybe_child(refusal.as_ref().map(|why| {
                Prose::new(why.note())
                    .color(theme.meta_color)
                    .width(Size::fill())
                    .wrap()
            }))
            // Below the bar, not inside it: what the conversation is talking to is a property of
            // the conversation, where everything in the bar is a property of this message.
            .child(Footer {
                id,
                pick,
                ai,
                theme,
            })
    }
}

/// **What the next send carries**, each attachment removable where it is shown.
///
/// Inside the bar rather than under it, because a pinned table is part of *this message* the way
/// the words are.
#[derive(PartialEq)]
struct Chips {
    id: crate::apps::project::state::ChatId,
    pinned: Vec<Anchor>,
    theme: ChatTheme,
}

impl Component for Chips {
    fn render(&self) -> impl IntoElement {
        let mut chats = use_consume::<ChatsCtx>();
        let (id, theme) = (self.id, self.theme.clone());
        self.pinned.iter().enumerate().fold(
            rect()
                .width(Size::fill())
                .horizontal()
                .content(Content::wrap_spacing(4.))
                .spacing(4.),
            |row, (at, anchor)| {
                row.child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(4.)
                        .corner_radius(4.)
                        .background(theme.chip_background)
                        .padding(Gaps::new(2., 4., 2., 6.))
                        .child(Meta::new(anchor.label()).color(theme.chip_color))
                        .child(
                            Button::new()
                                .flat()
                                .height(Size::px(16.))
                                .on_press(move |_| chats.write().unpin(id, at))
                                .child(Icon::new(IconName::Close).size(9.).color(theme.chip_color)),
                        ),
                )
            },
        )
    }
}

/// **What this conversation is talking to**: provider, model, and the rung the model offers.
///
/// Its own component rather than a row in [`Composer`], because it answers a different question
/// from everything above it — the bar is about *this message*, this is about the conversation —
/// and the three controls read each other's picks.
#[derive(PartialEq)]
struct Footer {
    id: crate::apps::project::state::ChatId,
    pick: Pick,
    ai: Ai,
    theme: ChatTheme,
}

impl Component for Footer {
    fn render(&self) -> impl IntoElement {
        let (id, pick, theme) = (self.id, self.pick.clone(), self.theme.clone());
        rect()
            .width(Size::fill())
            .horizontal()
            .content(Content::Flex)
            .cross_align(Alignment::Center)
            .spacing(6.)
            .child(ProviderPicker {
                id,
                pick: pick.clone(),
                ai: self.ai.clone(),
                theme: theme.clone(),
            })
            .child(ModelPicker {
                id,
                pick: pick.clone(),
                ai: self.ai.clone(),
                theme: theme.clone(),
            })
            .maybe_child(rungs(&pick).map(|rungs| EffortPicker {
                id,
                pick,
                rungs,
                theme,
            }))
    }
}

/// **How tall the message field may grow**: half the pane it sits in, or two thirds once
/// the bar's expand toggle is on.
///
/// A fraction rather than a pixel count, because the pane is resizable and a fixed ceiling is
/// either most of a narrow pane or a sliver of a tall one. [`MIN_CEILING`] is what covers the
/// frames before the pane has been measured — and a floor is right anyway, since a ceiling below
/// a couple of lines would make the box scroll from the second one.
///
/// Two stops, because the expand toggle in the bar's corner has two.
///
/// **The share is of the room the field can have, not of the whole pane.** Everything else in the
/// column — the header, the bar's own padding, the chips, the actions row, the footer pickers —
/// is [`COLUMN_CHROME`], and a share taken before subtracting it overruns the pane on anything
/// short: at two thirds the column only fitted above about 440px, so expanding a chat beside a
/// raised drawer drew the footer below the pane's bottom edge, where nothing can press it.
pub(super) fn ceiling(pane: f32, expanded: bool) -> f32 {
    let share = match expanded {
        true => 2. / 3.,
        false => 1. / 2.,
    };
    ((pane - COLUMN_CHROME).max(0.) * share).max(MIN_CEILING)
}

/// What the chat's column spends on everything that is not the message field: the header and its
/// rule, the composer's own inset and gaps, the bar's padding, the actions row and the footer's
/// pickers. Measured from the constants that draw them rather than guessed, and deliberately a
/// little generous — the cost of over-stating it is a slightly shorter field, and of
/// under-stating it a control laid out past the pane.
const COLUMN_CHROME: f32 = 200.;

/// The shortest ceiling [`ceiling`] will hand back — three lines of the composer's own type,
/// which is what an unmeasured pane and a very short one both get.
const MIN_CEILING: f32 = 60.;

/// **A rung belongs to a model, so a pick that moves either half re-asks the ladder.** A ladder
/// is `(provider, model)` — two providers can serve one name and offer different rungs, or none —
/// and [`EffortPicker`] is not rendered for a model with no rungs, so a rung left behind has
/// nothing on screen to clear it and `Brain::resolve` refuses every send with `NoSuchEffort`.
///
/// Both pickers ask here rather than each writing the rule out, which is what let them drift into
/// two spellings of it: one filtered the held rung, the other tested `Effort::Low` in its place
/// when none was held. Settings' own AI ▸ Chat guards the same thing on both axes.
fn keep_rung(kind: ProviderKind, model: &str, rung: Option<Effort>) -> Option<Effort> {
    rung.filter(|rung| efforts(kind, model).contains(rung))
}

/// The rungs this conversation's **model** offers — `None` when it has none, which is what the
/// footer draws no control for.
fn rungs(pick: &Pick) -> Option<&'static [Effort]> {
    let kind = pick.provider?;
    let rungs = efforts(kind, &pick.model);
    (!rungs.is_empty()).then_some(rungs)
}

/// **The provider.** Its own control, because one list of every enabled provider's every model
/// is unreadable the moment two providers are on — a dozen names under each heading, in a 340px
/// pane. Two narrow questions beat one long list.
///
/// Changing provider keeps the model **only if the new provider serves it**; otherwise the pick
/// is cleared and the model control says so, rather than carrying a name the next send is refused
/// for.
#[derive(PartialEq)]
struct ProviderPicker {
    id: crate::apps::project::state::ChatId,
    pick: Pick,
    ai: Ai,
    theme: ChatTheme,
}

impl Component for ProviderPicker {
    fn render(&self) -> impl IntoElement {
        let mut chats = use_consume::<ChatsCtx>();
        let app = use_consume::<AppCtx>();
        let mut open = use_state(|| false);
        let theme = self.theme.clone();
        let id = self.id;
        let picked = self.pick.provider;
        let listings = app.listings;

        let menu = self.ai.enabled().fold(
            Menu::new()
                .min_width(Size::px(PROVIDER_MENU_W))
                .on_close(move |()| open.set(false)),
            |menu, kind| {
                menu.child(
                    MenuButton::new()
                        .on_press(move |_| {
                            let serves: Vec<String> = listings.peek().models(kind).to_vec();
                            chats.write().repick(id, |pick| {
                                if pick.provider == Some(kind) {
                                    return;
                                }
                                pick.provider = Some(kind);
                                // The model belongs to a provider, so one that the new provider
                                // does not serve is not a pick any more — and a rung is a
                                // property of the model, so it goes with it. Both are dropped
                                // rather than left to be refused at the next send.
                                if !serves.iter().any(|name| name == &pick.model) {
                                    pick.model = String::new();
                                }
                                // The rung is re-asked against the new ladder even when the
                                // name survives, because the ladder is the pair.
                                pick.effort = keep_rung(kind, &pick.model, pick.effort);
                            });
                            open.set(false);
                        })
                        .child(
                            Meta::new(label(kind))
                                .color(match picked == Some(kind) {
                                    true => theme.chip_color,
                                    false => theme.title_color,
                                })
                                .width(Size::px(PROVIDER_MENU_W - MODEL_ROW_CHROME)),
                        ),
                )
            },
        );

        picker_trigger(
            "Provider for this chat",
            match picked {
                Some(kind) => label(kind).to_string(),
                None => "no provider".to_string(),
            },
            &self.theme,
            open,
            menu,
        )
    }
}

/// **The model**, from what the picked provider reports serving.
#[derive(PartialEq)]
struct ModelPicker {
    id: crate::apps::project::state::ChatId,
    pick: Pick,
    ai: Ai,
    theme: ChatTheme,
}

impl Component for ModelPicker {
    fn render(&self) -> impl IntoElement {
        let mut chats = use_consume::<ChatsCtx>();
        let app = use_consume::<AppCtx>();
        let probes = app.probes;
        let mut open = use_state(|| false);
        let theme = self.theme.clone();
        let id = self.id;
        let chosen = self.pick.model.clone();
        let picked = self.pick.provider;

        // **Opening the picker refreshes a stale list, in the background** — AS-06's rule reached
        // through AS-06's own funnel, guard included, so this surface and Settings ▸ AI ▸ Chat ask
        // the same question the same way. The cached list renders immediately and stays usable
        // throughout, and only the **picked** provider is asked: the others are not on screen.
        //
        // **Above the no-provider arm, and unconditional**, because a hook must run the same
        // number of times on every render of a scope (AGENTS.md §3). Registered below an early
        // return it was skipped while the conversation had no provider, so picking the first one
        // grew the hook count and Freya panicked on the next render — the "you cannot call them
        // conditionally" arm. The provider is a *value* the effect reads, never a reason not to
        // register it.
        let listings = app.listings;
        let ai = self.ai.clone();
        use_side_effect(move || {
            let Some(kind) = picked else {
                return;
            };
            if !*open.read() {
                return;
            }
            // Subscribed on purpose: the settled write lands here, and `needs_asking` then stops
            // it going round again.
            let _ = listings.read();
            if needs_asking(listings, probes, kind) {
                refresh(listings, probes, Ask::from_config(&ai, kind));
            }
        });

        let Some(kind) = picked else {
            // No provider, no models to offer: the provider control beside this one is where the
            // fix is, so this says what it has rather than opening on nothing.
            return picker_trigger(
                "Pick a provider first",
                "no model".to_string(),
                &self.theme,
                open,
                Menu::new(),
            )
            .into_element();
        };

        let offered = app.listings.read().offer(kind, &chosen);
        let menu = offered.iter().fold(
            Menu::new()
                .min_width(Size::px(MODEL_MENU_W))
                .on_close(move |()| open.set(false)),
            |menu, name| {
                let name = name.clone();
                let current = name == chosen;
                // A model that reasons says so, because that is the one property of a name that
                // changes which controls appear beside it.
                let reasons = !efforts(kind, &name).is_empty();
                menu.child(
                    MenuButton::new()
                        .on_press({
                            let name = name.clone();
                            move |_| {
                                let name = name.clone();
                                chats.write().repick(id, |pick| {
                                    // A rung the new model does not offer is dropped, not kept
                                    // out of sight.
                                    pick.effort = keep_rung(kind, &name, pick.effort);
                                    pick.model = name;
                                });
                                open.set(false);
                            }
                        })
                        .child(
                            rect()
                                .width(Size::px(MODEL_MENU_W - MODEL_ROW_CHROME))
                                .horizontal()
                                .content(Content::Flex)
                                .cross_align(Alignment::Center)
                                .spacing(6.)
                                .child(
                                    Meta::new(name)
                                        .color(match current {
                                            true => theme.chip_color,
                                            false => theme.title_color,
                                        })
                                        .width(Size::flex(1.))
                                        .max_lines(1)
                                        .text_overflow(TextOverflow::Ellipsis),
                                )
                                .maybe_child(
                                    reasons.then(|| Meta::new("REASONS").color(theme.meta_color)),
                                ),
                        ),
                )
            },
        );

        picker_trigger(
            "Model for this chat",
            match chosen.is_empty() {
                true => "no model".to_string(),
                false => chosen,
            },
            &self.theme,
            open,
            menu,
        )
        .into_element()
    }
}

/// The footer pickers' shared trigger: a flat, recessive label with a chevron, opening its menu
/// upward. One shape for all three, so they read as one row of the same kind of control.
fn picker_trigger(
    tip: &'static str,
    text: String,
    theme: &ChatTheme,
    mut open: State<bool>,
    menu: Menu,
) -> impl IntoElement {
    let trigger = Button::new()
        .flat()
        .height(Size::px(24.))
        .on_press(move |_| open.toggle())
        .child(
            rect()
                .horizontal()
                .cross_align(Alignment::Center)
                .spacing(4.)
                .child(
                    Meta::new(text)
                        .color(theme.meta_color)
                        .max_width(Size::px(TRIGGER_MAX_W))
                        .max_lines(1)
                        .text_overflow(TextOverflow::Ellipsis),
                )
                .child(
                    Icon::new(IconName::ChevronDown)
                        .size(10.)
                        .color(theme.meta_color),
                ),
        );

    Attached::new(
        TooltipContainer::new(Tooltip::new_text(tip))
            .position(AttachedPosition::Top)
            .child(trigger),
    )
    .top()
    .align_start()
    .offset(4.)
    .maybe_child(open().then_some(menu))
}

/// The reasoning rung, offered only where the model has one.
#[derive(PartialEq)]
struct EffortPicker {
    id: crate::apps::project::state::ChatId,
    pick: Pick,
    rungs: &'static [Effort],
    theme: ChatTheme,
}

impl Component for EffortPicker {
    fn render(&self) -> impl IntoElement {
        let mut chats = use_consume::<ChatsCtx>();
        let mut open = use_state(|| false);
        let theme = self.theme.clone();
        let id = self.id;
        let current = self.pick.effort;

        let menu = self.rungs.iter().fold(
            Menu::new()
                .min_width(Size::px(140.))
                .on_close(move |()| open.set(false)),
            |menu, rung| {
                let rung = *rung;
                menu.child(
                    MenuButton::new()
                        .on_press(move |_| {
                            chats.write().repick(id, |pick| {
                                // Pressing the rung already set clears it — "no preference" is a
                                // real value (the model's own default) and otherwise there would
                                // be no way back to it.
                                pick.effort = match pick.effort {
                                    Some(set) if set == rung => None,
                                    _ => Some(rung),
                                };
                            });
                            open.set(false);
                        })
                        .child(
                            rect()
                                .width(Size::px(110.))
                                .horizontal()
                                .content(Content::Flex)
                                .cross_align(Alignment::Center)
                                .child(
                                    Meta::new(rung.label())
                                        .color(match current == Some(rung) {
                                            true => theme.chip_color,
                                            false => theme.title_color,
                                        })
                                        .width(Size::flex(1.)),
                                ),
                        ),
                )
            },
        );

        let trigger = Button::new()
            .flat()
            .height(Size::px(24.))
            .on_press(move |_| open.toggle())
            .child(
                rect()
                    .horizontal()
                    .cross_align(Alignment::Center)
                    .spacing(4.)
                    .child(
                        Meta::new(match current {
                            Some(rung) => format!("{} effort", rung.label().to_lowercase()),
                            None => "default effort".to_string(),
                        })
                        .color(self.theme.meta_color),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .size(10.)
                            .color(self.theme.meta_color),
                    ),
            );

        Attached::new(
            TooltipContainer::new(Tooltip::new_text("Reasoning effort for this chat"))
                .position(AttachedPosition::Top)
                .child(trigger),
        )
        .top()
        .align_start()
        .offset(4.)
        .maybe_child(open().then_some(menu))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The field's ceiling is a **fraction of the pane**, with the bar's expand toggle as its
    /// second stop — so it is right at every pane size rather than at the one somebody measured.
    /// The share is taken of what is left after the column's own chrome, so the footer stays
    /// inside the pane at the tall stop as well as the short one.
    #[test]
    fn the_field_stops_at_half_the_room_and_two_thirds_expanded() {
        assert_eq!(ceiling(600., false), 200.);
        assert_eq!(ceiling(900., false), 350.);
        assert_eq!(ceiling(800., true), 400.);
    }

    /// **A rung survives a pick only if the new pair still offers it**, and the question is asked
    /// the same way whichever half moved — which is the point of there being one place to ask it.
    #[test]
    fn a_rung_the_new_pair_does_not_offer_is_dropped() {
        let kind = ProviderKind::Anthropic;
        assert_eq!(
            keep_rung(kind, "claude-opus-5", Some(Effort::High)),
            Some(Effort::High),
            "a rung the model offers is kept"
        );
        assert_eq!(
            keep_rung(kind, "claude-opus-4-5", Some(Effort::Max)),
            None,
            "the keyword ladder has no top rung, so this one goes"
        );
        assert_eq!(
            keep_rung(kind, "claude-haiku-4-5", Some(Effort::Low)),
            None,
            "a model with no ladder at all offers nothing to keep"
        );
        assert_eq!(
            keep_rung(kind, "claude-haiku-4-5", None),
            None,
            "no rung held is no rung to ask about"
        );
    }

    /// Before the first layout the pane measures zero, and a very short pane would otherwise give
    /// a ceiling the box scrolls from its second line. Both land on the floor.
    #[test]
    fn an_unmeasured_or_tiny_pane_gets_the_floor() {
        assert_eq!(ceiling(0., false), MIN_CEILING);
        assert_eq!(ceiling(40., true), MIN_CEILING);
    }
}
