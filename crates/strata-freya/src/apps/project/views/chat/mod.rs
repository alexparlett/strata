//! The **chat pane** (AS-04) — the assistant's conversations, in the right pane (design
//! `Strata.dc.html`, the `AI CHAT` region).
//!
//! Three parts, top to bottom: a [`header`] that is also the chat switcher, the
//! [`transcript`], and the [`composer`] with the conversation's provider · model · effort
//! pick in its footer.
//!
//! ## What it is, next to an MCP client
//!
//! An MCP client is an agent working in the project from somewhere else, and nothing it runs
//! reaches the user's tabs. This one is the agent that is *part of the app*: the user is
//! looking at it, which is why "open this in a tab" is a wanted gesture here and an intrusion
//! there. The app tells the two apart by construction, through `StrataTools::in_app`, never by
//! comparing a name (AA-03c).
//!
//! ## Two kinds of card, and the difference is the whole point
//!
//! A [`Step`](crate::apps::project::state::Step) card is a **citation**: AS-02's prompt says no
//! number in prose without a run behind it, and the card under the paragraph is what makes that
//! auditable. Every figure on it is the engine's own.
//!
//! An **offer** card is executable. It arrives only when the assistant calls `offer_sql` — its
//! own tool, checked against the catalog and the *editor's* policy before the card exists — and
//! it deliberately has no step card beside it, because an offer is not a step. SQL the assistant
//! is merely *explaining* never comes this way: it stays in the prose, as an ordinary markdown
//! code block with no Run press. That distinction is the reason the tool exists.
//!
//! ## Promotion is two presses, and never an edit
//!
//! *Open in tab* and *Run* both go through the editor's own `actions::open_sql`, so a promoted
//! statement is an ordinary scratch tab:
//! editable, saveable, undoable. **Nothing here ever writes the user's buffer.** A fix arrives as
//! a new tab, because the buffer is often the only record of how a number was reached.
//!
//! ## Prose is the fork's own markdown viewer
//!
//! `MarkdownViewer` (AGENTS.md §3 — standard components first, one level up), themed through the
//! shared component table like every other built-in. It is **not** on the crate's
//! `markdown-code-editor` feature, which would pull in freya's `code-editor` — this app has its
//! own and deliberately does not use that one.
//!
//! A **fenced block is a card with a copy press**, through the fork's `code_block` hook: the
//! offer card's dress minus its Run, because this is SQL the assistant is *explaining* and the
//! whole point of `offer_sql` is that an executable statement arrives as its own card instead.
//! The viewer still owns the parse; the pane owns only the dress.

mod card;
mod composer;
mod export;
mod header;
mod mention;
mod transcript;

use freya::components::{
    define_theme, get_theme, use_scroll_controller, ScrollConfig, ScrollPosition, ScrollView,
};
use freya::prelude::*;
use freya::radio::{use_radio, RadioStation};

use self::composer::Composer;
use self::header::ChatHeader;
use self::transcript::Transcript;
use crate::apps::project::state::{Anchor, Chan, ChatsCtx, SessionState};
use crate::components::divider::Divider;
use crate::components::metrics::{PANE_BODY_MIN_W, SP_4, SP_5};
use crate::theme::{use_roles, Role};
use strata_core::util::plural;
use strata_model::RightPane;

use crate::apps::export::ExportTarget;

define_theme!(
    // `%[no_ext]` like the command palette's: there is exactly one chat pane and nothing
    // overrides its dress, so it does not need the per-instance `theme` field the default arm
    // generates an extension trait for.
    %[no_ext]
    %[component]
    pub Chat {
        %[fields]
        /// The pane's body (canvas `--c-surface`) and the rules across it.
        background: Color,
        border_fill: Color,
        /// The switcher's title, and the assistant's prose.
        title_color: Color,
        /// A turn's role eyebrow — YOU / STRATA — and the switcher rows' meta line.
        role_color: Color,
        /// The recessive run: a chip's text, a card's tallies, the empty state's copy.
        meta_color: Color,
        /// A step card's figures: what the call cost, one step brighter than
        /// [`meta_color`](Self::meta_color) because it is the card's own data rather than its
        /// furniture.
        figures_color: Color,
        /// A card's own surface and edge, and the offer card's SQL.
        card_background: Color,
        card_border_fill: Color,
        sql_color: Color,
        /// A pinned `@`-chip: an accent wash with the accent's own text, so a chip reads as
        /// something the user attached rather than as something the assistant said.
        chip_background: Color,
        chip_color: Color,
        /// The hover wash on every pressable row here (a switcher row, a card action).
        row_hover_fill: Color,
    }
);

/// The gap between turns (canvas `--sp-5`) and the transcript's inset (`--sp-4`).
const TURN_GAP: f32 = SP_5;
const BODY_PAD: Gaps = Gaps::new_all(SP_4);

/// Read the pane's theme. One lookup, so no surface below reaches for a second.
pub fn chat_theme() -> ChatTheme {
    get_theme!(&None::<ChatThemePartial>, ChatThemePreference, "chat")
}

#[derive(PartialEq)]
pub struct ChatPane;

impl Component for ChatPane {
    fn render(&self) -> impl IntoElement {
        let theme = chat_theme();
        let mut session = use_radio::<SessionState, Chan>(Chan::Layout);
        let chats = use_consume::<ChatsCtx>();
        let border = use_roles().get(Role::Border);
        // The pane's own height, so the composer's field can be bounded by a fraction of it —
        // it is resizable, so a fixed ceiling would be wrong at most sizes. Written only on an
        // actual change, since every layout pass reports one.
        let mut pane_height = use_state(|| 0.);
        // **Follow the conversation, unless the reader has scrolled away from it.**
        //
        // A transcript that always jumped to the bottom would yank the view out from under
        // someone reading back through it — and one that never did would leave a streaming
        // answer writing off the bottom of the pane. So the rule is the reader's own position:
        // stick while they are at the end, stop the moment they are not, and resume when they
        // come back. `is_at_end` is the fork's, beside `is_scrollable`, because the viewport and
        // the content size it compares are the scrollable's own and nothing here should measure
        // them a second time.
        let mut scroll = use_scroll_controller(ScrollConfig::default);
        let mut following = use_state(|| true);
        // **Re-asked on the scroll position and nothing else.** `is_at_end` peeks, so this
        // decides only when the position moves — which is the reader or this very follower, and
        // never the content growing. Keyed on the content size instead, the first answer too tall
        // for the pane would read as the reader having scrolled away and following would stop
        // before it ever started.
        let position = use_reactive(&<(i32, i32)>::from(scroll));
        use_side_effect(move || {
            let _ = position.read();
            let at_end = scroll.is_at_end(Direction::Vertical);
            if *following.peek() != at_end {
                following.set(at_end);
            }
        });
        use_side_effect(move || {
            // Wake on anything the transcript shows — a new turn, a delta, a settled card.
            let _ = chats.read();
            if *following.peek() {
                scroll.scroll_to(ScrollPosition::End, Direction::Vertical);
            }
        });

        // `Content::Flex`, because the transcript between the header and the composer is the
        // one thing here that takes the slack — and `Size::flex` is only divided by a parent
        // whose content is `Flex` (AGENTS.md §3). Without it the transcript claims the column
        // and the composer is laid out past the bottom edge.
        rect()
            .expanded()
            .min_width(Size::px(PANE_BODY_MIN_W))
            .vertical()
            .content(Content::Flex)
            .background(theme.background)
            .on_sized(move |e: Event<SizedEventData>| {
                if *pane_height.peek() != e.area.height() {
                    pane_height.set(e.area.height());
                }
            })
            .child(ChatHeader {
                theme: theme.clone(),
                on_close: EventHandler::new(move |()| {
                    session.write_channel(Chan::Layout).close_right_pane();
                }),
            })
            .child(Divider::horizontal().color(border))
            // The transcript takes the slack; the composer is a fixed foot. A `ScrollView` here
            // rather than on each turn, so a streaming delta grows one scrolling body.
            .child(
                rect().width(Size::fill()).height(Size::flex(1.)).child(
                    ScrollView::new_controlled(scroll)
                        .width(Size::fill())
                        .height(Size::fill())
                        .child(
                            rect()
                                .width(Size::fill())
                                .vertical()
                                .padding(BODY_PAD)
                                .spacing(TURN_GAP)
                                .child(Transcript {
                                    chats,
                                    theme: theme.clone(),
                                }),
                        ),
                ),
            )
            // **No rule above the composer.** The field draws its own box, so a divider between
            // it and the transcript is a second edge a few pixels from the first — the header
            // keeps its rule because a title strip has no other boundary.
            .child(Composer { theme, pane_height })
    }
}

/// **Open the pane with something already pinned** — the one funnel behind every entry at a
/// point of friction: a failed run's error, the results toolbar's *Explain this result*, and a
/// catalog row's *Ask about this*.
///
/// Each of those is one press into *this* pane with a chip pre-filled, never a second surface —
/// which is the whole reason they are a funnel rather than three gestures. `open_right_pane`
/// rather than the rail's toggle, for `open_drawer`'s reason: a row that says "ask about this
/// table" has to mean it, so asking for the pane you are already looking at must not put it away.
///
/// The anchor lands on the **open** conversation, because that is the one the user is looking at.
/// A new chat for every entry would bury the thread they were in the middle of.
///
/// Takes the **station** rather than a subscribing handle: this only ever writes, and a funnel
/// that subscribed would wake every caller's surface on any layout change.
pub fn ask_about(
    mut session: RadioStation<SessionState, Chan>,
    mut chats: ChatsCtx,
    anchor: Anchor,
) {
    session
        .write_channel(Chan::Layout)
        .open_right_pane(RightPane::Chat);
    let id = chats.peek().active_id();
    chats.write().pin(id, anchor);
}

/// **What `@result` pins**: the settled run's schema, its exact row total, and the head of the
/// page the grid already had.
///
/// Built from the same [`ExportTarget`] the Download press acts on, because that value *is* the
/// settled run as this window knows it — schema, exact total (read from the run, never counted
/// from the grid) and real rows. Nothing here is fetched or re-counted.
///
/// Bounded, with the cut stated (AA-07's rule for every list-shaped answer): a page can be ten
/// thousand rows of wide cells, and a context block is re-sent on every later round of the turn.
pub fn result_anchor(target: &ExportTarget) -> Anchor {
    use std::fmt::Write;

    let mut body = String::new();
    let _ = writeln!(body, "{}.", plural(target.total, "row"));
    let _ = writeln!(body, "\nColumns:");
    for column in &target.columns {
        let _ = writeln!(body, "- {} {}", column.name, column.dtype);
    }
    let shown = target.sample.len().min(SAMPLE_ROWS);
    if shown > 0 {
        let _ = writeln!(
            body,
            "\nFirst {shown} of {} rows on the page in hand:",
            target.sample.len()
        );
        for row in target.sample.iter().take(shown) {
            let cells: Vec<&str> = row.iter().map(|cell| cell.text.as_str()).collect();
            let _ = writeln!(body, "{}", cells.join(" | "));
        }
    }
    Anchor::Result {
        // The bare name: `Anchor::label` is what puts the `@` on, for every arm.
        name: target.label.clone(),
        body,
    }
}

/// How many of the page's rows the `@result` anchor carries. Enough to see the shape of the
/// data, few enough that a wide page cannot dominate the turn's context.
const SAMPLE_ROWS: usize = 20;

/// The pane driven the way a window drives it — over real stores, so the states a user meets
/// first are the ones under test.
#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use freya::radio::{use_radio_station, RadioStation};
    use freya_testing::prelude::{
        Code, Key, KeyboardEventName, Modifiers, NamedKey, PlatformEvent,
    };
    use freya_testing::TestingRunner;
    use strata_agent::assistant::{Assistant, Scope, Settle, TurnEvent};
    use strata_agent::StrataTools;
    use strata_core::ai::{Ai, ProviderKind, ProviderSetup};
    use strata_core::config::AppConfig;
    use strata_core::models::Listings;
    use strata_core::project::ProjectDefs;
    use strata_core::theme::load;

    use super::composer::ceiling;
    use super::transcript::ACTIONS_H;
    use super::*;
    use crate::agent::{create_global_agent, AgentDirectory};
    use crate::apps::project::contexts::EngineCtx;
    use crate::apps::project::state::{
        seed_pick, AssistantCtx, Chats, Log, PersistFaults, ProjChan, ProjectState, SessionState,
    };
    use crate::apps::project::views::ChatDrop;
    use crate::components::metrics::TOOL_SIZE;
    use crate::menu::create_global_menu;
    use crate::platform::{create_global_open, create_global_windows};
    use crate::state::{create_global_theme_preview, AppCtx, ConfigStation, ModelListings, Probes};
    use crate::theme::{strata_theme, ThemesCtx};

    /// A config whose AI half is `ai` — everything the composer branches on.
    fn config(ai: Ai) -> AppConfig {
        let mut config = AppConfig::default();
        config.settings.ai = ai;
        config
    }

    /// One enabled provider with a model chosen, which is the configured case.
    fn configured() -> Ai {
        Ai {
            providers: [(
                ProviderKind::Anthropic,
                ProviderSetup {
                    enabled: true,
                    ..ProviderSetup::default()
                },
            )]
            .into_iter()
            .collect(),
            default_provider: Some(ProviderKind::Anthropic),
            default_model: "claude-sonnet-4-5".into(),
            default_effort: None,
            ..Ai::default()
        }
    }

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        // The pane's own two writes go through the station; everything else it reads.
        let _ = use_radio_station::<SessionState, Chan>();
        rect().expanded().child(ChatPane)
    }

    /// The pane's test size — the canvas's default width, and a height the composer has to fit
    /// inside.
    const PANE_W: f32 = 340.;
    const PANE_H: f32 = 700.;

    /// The pane over `ai`, plus the conversations handle — so a test can drive the transcript the
    /// way the turn task does, through `Chats`' own fold rather than by reaching into a view.
    fn runner_with_chats(ai: Ai) -> (TestingRunner, ChatsCtx) {
        let (mut runner, chats) = TestingRunner::new(
            app,
            (PANE_W, PANE_H).into(),
            move |r| {
                let seed = seed_pick(&ai);
                let config = r.provide_root_context(move || ConfigStation::create(config(ai)));
                r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                });
                r.provide_root_context(|| {
                    RadioStation::<ProjectState, ProjChan>::create(ProjectState::from_defs(
                        ProjectDefs::default(),
                        std::path::PathBuf::from("/tmp/strata-chat-test"),
                    ))
                });
                r.provide_root_context(EngineCtx::default);
                // The header reports through both halves of the write funnel — an export says
                // so in the log, a conversation the store could not write raises a condition.
                r.provide_root_context(|| State::create(Log::default()));
                r.provide_root_context(|| State::create(PersistFaults::default()));
                // The pane's destructive presses set this slot; the dialog that reads it is
                // mounted at the window root, which this harness does not stand up.
                r.provide_root_context(|| State::create(None::<ChatDrop>));
                let listings: ModelListings =
                    r.provide_root_context(|| State::create_global(Listings::default()));
                let probes = r.provide_root_context(|| State::create_global(Probes::default()));
                // A **real** runtime, so the composer's refusal reads as a user's would: the
                // runtime arm outranks the config arms, and a test that skipped it would only
                // ever see "the assistant could not start". A directory with no window
                // registered is enough beside it — the pane calls no tool while it renders.
                r.provide_root_context(move || AssistantCtx {
                    assistant: Assistant::new().ok().map(Rc::new),
                    tools: StrataTools::in_app(Arc::new(AgentDirectory::default())),
                    scope: Scope::default(),
                });
                let chats = r.provide_root_context(move || State::create(Chats::new(seed)));
                r.provide_root_context(move || AppCtx {
                    themes: ThemesCtx::discover(),
                    config,
                    windows: create_global_windows(),
                    preview: create_global_theme_preview(),
                    menu: create_global_menu(),
                    open: create_global_open(),
                    agent: create_global_agent(),
                    listings,
                    probes,
                    assistant: None,
                });
                chats
            },
            1.,
        );
        for _ in 0..4 {
            runner.sync_and_update();
        }
        (runner, chats)
    }

    /// [`runner_with_chats`] for the tests that only look at what is on screen.
    fn runner(ai: Ai) -> TestingRunner {
        runner_with_chats(ai).0
    }

    /// **How tall the field's box is** — the `Input`'s own rect, found by its accessibility role
    /// because that is the one thing on it that names what it is.
    ///
    /// Not the paragraph inside it: that is the whole wrapped block, scrolled part included, so
    /// measuring it would call a capped field thousands of pixels tall and a collapsed one fine.
    fn field_height(runner: &TestingRunner) -> f32 {
        field_area(runner).height()
    }

    /// The composer field's laid-out box.
    fn field_area(runner: &TestingRunner) -> Area {
        runner
            .find(|node, element| {
                (element.accessibility().builder.role() == AccessibilityRole::TextInput)
                    .then(|| node.layout().area)
            })
            .expect("the composer's field is on screen")
    }

    /// The field's laid-out width — pinned across a long message, because **text wraps and the
    /// box does not widen**: the only axis a composer scrolls is the one its lines run down.
    fn field_width(runner: &TestingRunner) -> f32 {
        field_area(runner).width()
    }

    /// Press the bar's expand toggle — the topmost 28x28 tool button **level with the field or
    /// below it**, which is the bar's first row. The header has tool buttons too (new chat,
    /// close), so "topmost on screen" would press one of those.
    ///
    /// Found by geometry rather than by name because an icon-only `ToolButton` has no accessible
    /// name yet: `Button` sets a role and no label, and naming it belongs on `Button` in the fork
    /// (see `components::tool_button`). When that lands, this finder becomes a name lookup.
    fn press_expand(runner: &mut TestingRunner) {
        let field = field_area(runner);
        let mut tools: Vec<Area> = runner.find_many(|node, element| {
            let area = node.layout().area;
            (element.accessibility().builder.role() == AccessibilityRole::Button
                && (area.width() - TOOL_SIZE).abs() < 0.5
                && (area.height() - TOOL_SIZE).abs() < 0.5
                && area.min_y() >= field.min_y() - 4.)
                .then_some(area)
        });
        tools.sort_by(|a, b| a.min_y().total_cmp(&b.min_y()));
        let area = tools.first().expect("the bar's expand toggle is on screen");
        let point = (
            (area.min_x() + area.width() / 2.) as f64,
            (area.min_y() + area.height() / 2.) as f64,
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        settle(runner);
    }

    /// Where the message whose text is `text` is laid out.
    fn message_area(runner: &TestingRunner, text: &str) -> Area {
        runner
            .find(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == text)
                    .map(|_| node.layout().area)
            })
            .expect("the message is in the transcript")
    }

    /// Focus the composer by pressing it, the way a user does — the field only takes keys once
    /// it has focus, so a test that skipped this would type into nothing and prove nothing.
    fn focus_field(runner: &mut TestingRunner) {
        let area = runner
            .find(|node, element| Paragraph::try_downcast(element).map(|_| node.layout().area))
            .expect("the composer's field is on screen");
        let point = (
            (area.min_x() + area.width() / 2.) as f64,
            (area.min_y() + area.height() / 2.) as f64,
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        settle(runner);
    }

    /// Type `lines` more lines into the focused composer — **through real events**, `Shift`+Enter
    /// included, so this exercises the newline binding as well as the growth it causes.
    fn type_lines(runner: &mut TestingRunner, lines: usize) {
        for n in 0..lines {
            if n > 0 {
                runner.send_event(PlatformEvent::Keyboard {
                    name: KeyboardEventName::KeyDown,
                    key: Key::Named(NamedKey::Enter),
                    code: Code::Enter,
                    modifiers: Modifiers::SHIFT,
                });
                runner.sync_and_update();
            }
            runner.write_text(format!("line {n}"));
        }
        settle(runner);
    }

    fn settle(runner: &mut TestingRunner) {
        for _ in 0..6 {
            runner.sync_and_update();
        }
    }

    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    /// A pane just opened says what it is for, and names its one conversation.
    #[test]
    fn an_empty_pane_invites_a_question() {
        let runner = runner(configured());
        let texts = texts(&runner);

        assert!(
            texts.iter().any(|t| t.starts_with("Ask about your tables")),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t == "New chat"), "{texts:?}");
        assert!(
            texts.iter().any(|t| t == "claude-sonnet-4-5"),
            "the footer names what this conversation is talking to: {texts:?}"
        );
    }

    /// **The composer is on screen**, which is not the same question as whether it rendered:
    /// the pane is a column with one flexing child between two fixed ones, and a column that is
    /// not `Content::Flex` lays the foot out past its own bottom edge — where it draws, is
    /// hit-testable by nothing, and reads as missing. Measured against the pane's height rather
    /// than merely asserted present, because that is the failure this pins.
    #[test]
    fn the_composer_is_laid_out_inside_the_pane() {
        let runner = runner(configured());
        // The field's text is an `Input`'s `paragraph()`, not a `label()` — the placeholder is
        // a span on it — so it is found the way the input actually renders.
        let box_ = runner
            .find(|node, element| {
                Paragraph::try_downcast(element)
                    .filter(|p| {
                        p.spans
                            .iter()
                            .any(|s| s.text.contains("Ask about your data"))
                    })
                    .map(|_| node.layout().area)
            })
            .expect("the composer's field is on screen");

        assert!(
            box_.max_y() <= PANE_H,
            "the composer is laid out past the pane's bottom edge: {box_:?}"
        );
        assert!(box_.width() > 0. && box_.height() > 0., "{box_:?}");
    }

    /// **The field grows with what is typed, and stops at the cap.** Three measurements of the
    /// same box, because "it rendered" and "it is the right size" are different questions and
    /// only the second is the one somebody typing into it asks.
    ///
    /// The numbers are deliberately relative — a line's height is the theme's, not this test's —
    /// so what is pinned is the *behaviour*: one line is about one line, several lines are
    /// taller, and a great many stop at [`ceiling`]'s half-pane rather than growing on.
    #[test]
    fn the_field_grows_with_its_text_and_stops_at_the_cap() {
        let mut runner = runner(configured());
        focus_field(&mut runner);

        let one = field_height(&runner);
        let field_width_at_rest = field_width(&runner);
        assert!(
            (10. ..60.).contains(&one),
            "an empty field is about one line tall, not a sliver and not a block: {one}"
        );

        type_lines(&mut runner, 4);
        let few = field_height(&runner);
        // Not a multiple: the box is lines *plus* its own padding, and the padding does not
        // grow with them. Three more lines is what is being asserted, not four times the height.
        assert!(
            few > one + 30.,
            "four lines is taller than one ({few} vs {one})"
        );

        type_lines(&mut runner, 200);
        let many = field_height(&runner);
        assert_eq!(
            field_width(&runner),
            field_width_at_rest,
            "a long message wraps rather than widening the box"
        );
        assert!(
            many <= ceiling(PANE_H, false) + 1.,
            "the field stops at half the pane rather than growing on: {many}"
        );
        assert!(many > few, "and it did grow to get there ({many} vs {few})");
    }

    /// **Expand does something with nothing typed.** The toggle is a *size*, not a bigger
    /// ceiling — raising only the cap leaves an empty box exactly as short as it was, which is
    /// what made the press look broken. Pressed on an empty composer, the field is two thirds of
    /// the pane.
    #[test]
    fn expanding_grows_an_empty_field_to_two_thirds() {
        let mut runner = runner(configured());
        let before = field_height(&runner);

        press_expand(&mut runner);

        let after = field_height(&runner);
        assert!(
            (after - ceiling(PANE_H, true)).abs() < 2.,
            "expanded is two thirds of the pane: {after} (was {before})"
        );

        press_expand(&mut runner);
        assert!(
            (field_height(&runner) - before).abs() < 2.,
            "and pressing it again puts the box back"
        );
    }

    /// **A live turn says it is working**, and stops saying it the moment it has anything else to
    /// show. A role eyebrow over nothing reads as a send that went nowhere.
    #[test]
    fn an_open_reply_says_it_is_thinking_until_it_has_something_to_say() {
        let (mut runner, mut chats) = runner_with_chats(configured());

        let id = chats.peek().active_id();
        chats.write().ask(id, "how many rows?".into(), vec![]);
        settle(&mut runner);
        assert!(
            texts(&runner).iter().any(|t| t == "Thinking…"),
            "{:?}",
            texts(&runner)
        );

        chats.write().fold(id, TurnEvent::Delta("Checking".into()));
        settle(&mut runner);
        assert!(
            !texts(&runner).iter().any(|t| t == "Thinking…"),
            "the first delta replaces it: {:?}",
            texts(&runner)
        );
    }

    /// **The transcript follows the conversation while the reader is at the end of it**, and a
    /// long one still shows its newest message rather than its first.
    #[test]
    fn the_transcript_shows_the_newest_message() {
        let (mut runner, mut chats) = runner_with_chats(configured());
        let id = chats.peek().active_id();

        // Enough turns that the body is taller than the pane, so what is on screen is a choice
        // rather than everything there is.
        for n in 0..24 {
            chats.write().ask(id, format!("question {n}"), vec![]);
            chats
                .write()
                .fold(id, TurnEvent::Delta(format!("answer {n}")));
            chats.write().settle(id, Settle::Answered);
        }
        settle(&mut runner);

        // **Measured, not listed.** Every turn is in the tree whether or not it is on screen, so
        // what says the transcript followed the conversation is where the messages sit.
        let newest = message_area(&runner, "question 23");
        let oldest = message_area(&runner, "question 0");
        assert!(
            newest.max_y() <= PANE_H && newest.min_y() >= 0.,
            "the newest question is in view: {newest:?}"
        );
        assert!(
            oldest.max_y() < newest.min_y(),
            "and the first one is above it, scrolled off: {oldest:?}"
        );
    }

    /// **A message's actions are always mounted and revealed by opacity** — never added and
    /// removed under the pointer.
    ///
    /// Building them on hover made them appear exactly where the cursor already was: the button
    /// materialised under the pointer, took it, the turn read as un-hovered, the button unmounted
    /// and the turn was hovered again. It flickered and could not be pressed. So what is pinned
    /// here is that the row is *there* at rest — its slot reserved, its children mounted — and
    /// merely transparent.
    ///
    /// The hover itself is **not** covered: the pointer enter/over handlers do not fire in
    /// `TestingRunner` (verified — the handler never runs under a `move_cursor`), so a test of
    /// the reveal would be a test of nothing dressed as a passing one. Closing that harness gap
    /// is its own job rather than something to fake around here.
    #[test]
    fn a_messages_actions_are_mounted_and_merely_transparent_at_rest() {
        let (mut runner, mut chats) = runner_with_chats(configured());
        let id = chats.peek().active_id();

        chats.write().ask(id, "hello".into(), vec![]);
        chats
            .write()
            .fold(id, TurnEvent::Delta("Hello back.".into()));
        chats.write().settle(id, Settle::Answered);
        settle(&mut runner);

        let stamps = texts(&runner)
            .into_iter()
            .filter(|t| t.len() == 8 && t.chars().filter(|c| *c == ':').count() == 2)
            .count();
        assert_eq!(
            stamps, 2,
            "one row per message, mounted whether shown or not"
        );

        // Every one of them is invisible until its own message is hovered.
        let hidden: Vec<f32> = runner.find_many(|node, element| {
            let area = node.layout().area;
            ((area.height() - ACTIONS_H).abs() < 0.5)
                .then(|| element.effect().and_then(|e| e.opacity).unwrap_or(1.))
                .filter(|opacity| *opacity < 1.)
        });
        assert_eq!(hidden.len(), 2, "both rows are transparent at rest");
    }

    /// **Never a dead send button.** With nothing enabled the composer says which page fixes it
    /// — and the model picker says "no model" rather than a name nobody chose.
    #[test]
    fn an_unconfigured_pane_names_what_is_missing() {
        let runner = runner(Ai::default());
        let texts = texts(&runner);

        assert!(
            texts
                .iter()
                .any(|t| t.contains("Settings > AI > Providers")),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t == "no model"), "{texts:?}");
    }
}
