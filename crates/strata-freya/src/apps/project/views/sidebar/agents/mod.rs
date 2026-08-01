//! The **Agents** sidebar pane (AA-03b) — what each connected agent is doing in this project,
//! and the one gesture that brings its work into the user's own editor.
//!
//! Built to the canvas (`Strata.dc.html`, `data-pane="agents"`).
//!
//! ## Why it is a pane and not a drawer tab
//!
//! The canvas states the rule and it is a good one: a **drawer** is an ephemeral log you
//! consult — Problems, Events, History — while this is a live, navigable tree of connected
//! things you press *into*, which is the catalog's job description. So it sits in the rail's
//! top group, and the drawer keeps its three tabs.
//!
//! ## Only connected agents appear
//!
//! A client that disconnects takes its query sessions with it, so the pane answers "what is
//! working on my project right now" rather than becoming a second history. Two consequences
//! that look like omissions and are not: no row wears a connected mark (a mark with one
//! possible value is decoration implying a distinction the data does not carry — the History
//! drawer's cards lost their status dot for the same reason), and the rail's count badge and
//! this list are therefore the same fact.
//!
//! ## The shape is vocabulary the app already has
//!
//! Freya's [`TreeItem`] for the agent and session levels — indent, disclosure arrow, hover and
//! a11y for free, with our own chevron and no indent guides — and the History drawer's card for
//! a run. Not only economy: the History card is *already* the answer to "a past query you can
//! act on", which is exactly what a run row is.
//!
//! ## A press promotes into a **new** tab
//!
//! Pressing a run opens its SQL in a fresh tab, focused, through the editor's own
//! [`actions`](crate::apps::project::views::workbench::editor::actions) — never into the tab the
//! user is working in, which is the precise harm this pane exists to prevent and would be
//! reintroduced by a load-into-active-tab gesture. There is deliberately **no** double-press to
//! run: promoting is putting a query where the user can read it, and pressing Run is their next
//! decision, not this row's.
//!
//! The pane is read-only in the other direction too. Nothing here closes an agent's session or
//! cancels its run — those are the agent's own, and a control that reached into somebody else's
//! work would be this task's own argument, pointed backwards.

mod run;

use freya::components::{
    define_theme, get_theme, use_theme, Disclosure, ScrollView, Tooltip, TooltipContainer,
    TreeConfig, TreeItem, TreeThemePartial,
};
use freya::prelude::*;
use freya::radio::use_radio;
use strata_agent::QuerySessionId;

use self::run::RunCard;
use crate::agent::RunOutcome;
use crate::apps::project::state::{AgentRun, AgentsCtx, Chan, ConnectedAgent, SessionState};
use crate::components::icon::{Icon, IconName};
use crate::components::typography::{Meta, MonoValue, Prose};

define_theme!(
    %[component]
    pub Agents {
        %[fields]
        /// An agent's name — the brightest run in the pane (canvas `--c-text`).
        name_color: Color,
        /// A session row's label (`--c-text3`).
        session_color: Color,
        /// Every structural glyph: the two chevrons, and the header's info mark (`--c-faint`).
        chevron_color: Color,
        /// The most recessive run: a session's tallies and a card's age (`--c-faint`).
        meta_color: Color,
        /// A run's **figures** — one step forward from [`meta_color`](Self::meta_color),
        /// because what a query cost is the row's own data rather than its furniture
        /// (`--c-muted`). The drawer's `value_color` distinction, on the same kind of row.
        figures_color: Color,
        /// A run's SQL preview — the thing a reader is actually scanning for (`--c-text2`).
        sql_color: Color,
        /// A card's hover surface, since a press is its only affordance.
        ///
        /// **A named divergence from the canvas** (`--c-surface2`): the app's `surface_hover`,
        /// for the reason the History card's own fill states — `surface_secondary` is pure white
        /// in Daylight, so a fill one step off it reads as no hover at all, and this card is
        /// that card.
        card_hover_fill: Color,
        /// The empty state's tile, its edge, and its copy. The glyph inside wears
        /// [`chevron_color`](Self::chevron_color), like every other recessive mark here.
        empty_background: Color,
        empty_border_fill: Color,
        empty_color: Color,
    }
);

/// The pane's scroll inset, matching the catalog's.
const BODY_PAD: Gaps = Gaps::new(8., 8., 12., 8.);
/// The disclosure glyph both tree levels use — **our** chevrons, not the fork's built-in arrow,
/// so the pane's structural marks match every other tree-ish surface in the app (the catalog's
/// sections, the inspector's nested fields).
fn chevron(open: bool, color: Color) -> Element {
    Icon::new(match open {
        true => IconName::ChevronDown,
        false => IconName::ChevronRight,
    })
    .color(color)
    .size(11.)
    .into_element()
}

/// The rows' own dress: no indent guides, and no selection fill, because nothing here is
/// selectable — a tree row's arrow and hover come from the shared `tree` theme, and these are
/// the two things this tree has no use for. Setting `guide_fill` transparent is the component's
/// own documented way to indent without guides.
fn rows() -> TreeThemePartial {
    TreeThemePartial::new()
        .guide_fill(Color::TRANSPARENT)
        .item_padding(Gaps::new(0., 6., 0., 2.))
}

/// One level of tree indentation, and the row height both tree levels share. Provided as the
/// `TreeConfig` every [`TreeItem`] here reads, so the two levels cannot drift apart.
const INDENT: f32 = 14.;
const ROW_HEIGHT: f32 = 28.;
/// How far a run card sits in from its session row: two levels of indent plus the arrow's own
/// slot, so a card's dot lines up under its session's label.
const RUN_INDENT: f32 = INDENT * 3.;
/// The gap under one agent's whole block (`--sp-2`).
const AGENT_GAP: f32 = 4.;
/// The empty state's inset (canvas `--sp-7 --sp-6`) — generous at the top, because it sits
/// where the first row would rather than in the middle of the panel.
const EMPTY_PAD: Gaps = Gaps::new(32., 24., 32., 24.);

/// The Agents tree — the sidebar body under the pane header.
#[derive(PartialEq)]
pub struct Agents {
    pub theme: Option<AgentsThemePartial>,
}

impl Agents {
    pub fn new() -> Self {
        Self { theme: None }
    }
}

impl Component for Agents {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&self.theme, AgentsThemePreference, "agents");
        let agents = use_consume::<AgentsCtx>();
        // The metrics both tree levels share, set once here rather than per row — `TreeItem`
        // reads them from context (its `TreeConfig`), and outside a `Tree` nothing else would
        // provide them.
        use_provide_context(|| TreeConfig {
            indent: INDENT,
            item_height: ROW_HEIGHT,
        });

        if agents.read().len() == 0 {
            return Empty { theme }.into_element();
        }

        // Cloned out, so the satellite's read guard is dropped before any element is built — a
        // group renders from a snapshot of one agent, not from a borrow held across the tree.
        let groups: Vec<Element> = agents
            .read()
            .agents()
            .map(|agent| {
                AgentGroup {
                    agent: agent.clone(),
                    theme: theme.clone(),
                    key: DiffKey::None,
                }
                .key(agent.id.0)
                .into_element()
            })
            .collect();

        rect()
            .expanded()
            .child(
                ScrollView::new().child(
                    rect()
                        .width(Size::fill())
                        .vertical()
                        .padding(BODY_PAD)
                        .children(groups),
                ),
            )
            .into_element()
    }
}

/// The pane header's ⓘ, which is where the query-session model is explained — the one concept in
/// this surface a user has no other way to learn. Mounted by the sidebar shell beside the
/// `AGENTS` label.
#[derive(PartialEq)]
pub struct AgentsHint;

impl Component for AgentsHint {
    fn render(&self) -> impl IntoElement {
        let theme = get_theme!(&None::<AgentsThemePartial>, AgentsThemePreference, "agents");
        TooltipContainer::new(Tooltip::new_text(
            "MCP clients connected to this project. Each runs its queries in its own query \
             session, never in your tabs. Press a run to open its SQL in a new tab.",
        ))
        .position(AttachedPosition::Bottom)
        .child(
            rect()
                .width(Size::px(14.))
                .height(Size::px(14.))
                .center()
                .child(
                    Icon::new(IconName::Info)
                        .color(theme.chevron_color)
                        .size(13.),
                ),
        )
    }
}

/// Nothing connected. Not a fault and not a prompt to go and configure something — the setting
/// may well be on with nobody paired — so the copy says what to do rather than what is wrong.
///
/// **Top-aligned, not centred** (canvas: `padding: --sp-7 --sp-6` and no vertical centring).
/// A pane's empty state sits where its first row would, so switching panes doesn't move the
/// reader's eye down the panel and back — which is also why the copy is a sentence rather than
/// the drawer's centred `DrawerEmpty` glyph-and-label, whose frame *is* a centred box.
#[derive(PartialEq)]
struct Empty {
    theme: AgentsTheme,
}

impl Component for Empty {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .vertical()
            .cross_align(Alignment::Center)
            .padding(EMPTY_PAD)
            .spacing(12.)
            .child(
                rect()
                    .width(Size::px(40.))
                    .height(Size::px(40.))
                    .corner_radius(10.)
                    .background(self.theme.empty_background)
                    .border(Border::new().width(1.).fill(self.theme.empty_border_fill))
                    .center()
                    .child(
                        Icon::new(IconName::Agent)
                            .color(self.theme.chevron_color)
                            .size(19.),
                    ),
            )
            .child(
                Prose::new(
                    "No agents connected. Point an MCP client at this project and its query \
                     sessions appear here.",
                )
                .color(self.theme.empty_color)
                .max_width(Size::px(210.))
                .wrap()
                .align(TextAlign::Center),
            )
    }
}

/// One connected agent and everything it is working on.
#[derive(PartialEq)]
struct AgentGroup {
    agent: ConnectedAgent,
    theme: AgentsTheme,
    key: DiffKey,
}

impl KeyExt for AgentGroup {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for AgentGroup {
    fn render(&self) -> impl IntoElement {
        // Open by default and collapse-local, the catalog section's rule: which groups are
        // folded is a way of looking, not project data, so it neither persists nor reaches a
        // store.
        let mut open = use_state(|| true);
        let toggle = move |_: Event<PressEventData>| {
            let now = *open.peek();
            open.set(!now);
        };

        let sessions: Vec<Element> = match open() {
            false => Vec::new(),
            true => self
                .agent
                .sessions
                .iter()
                .map(|session| {
                    SessionGroup {
                        id: session.id,
                        ordinal: session.ordinal,
                        runs: session.runs.iter().cloned().collect(),
                        theme: self.theme.clone(),
                        key: DiffKey::None,
                    }
                    .key(session.id.0)
                    .into_element()
                })
                .collect(),
        };

        // **`on_press` only.** `on_toggle` exists for a tree that also *selects*, where opening
        // a row and choosing it are different intents — so the arrow consumes its own press to
        // keep them apart. Nothing here is selectable, so wiring it would only make the arrow
        // behave differently from the rest of the row it sits in.
        let row = TreeItem::new()
            .width(Size::fill())
            .depth(0)
            .theme(rows())
            .disclosure(Disclosure::from_expanded(open()))
            .arrow(chevron(open(), self.theme.chevron_color))
            .on_press(toggle)
            .child(
                rect()
                    .width(Size::flex(1.))
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(8.)
                    .child(
                        MonoValue::new(self.agent.name().to_string())
                            .color(self.theme.name_color)
                            .width(Size::flex(1.))
                            .max_lines(1)
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .child(
                        Meta::new(plural_of(self.agent.sessions.len(), "session"))
                            .color(self.theme.meta_color),
                    ),
            );

        rect()
            .width(Size::fill())
            .vertical()
            .margin(Gaps::new(0., 0., AGENT_GAP, 0.))
            // The **version** is what the tooltip is for. The canvas's title also said
            // "· connected", which every row in this pane is by construction — the same
            // tautology the removed status dot was.
            .child(match self.agent.detail() {
                Some(detail) => TooltipContainer::new(Tooltip::new_text(detail))
                    .position(AttachedPosition::Bottom)
                    .child(row)
                    .into_element(),
                None => row.into_element(),
            })
            .children(sessions)
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// One query session: its name, whether it is working, and its run trail.
#[derive(PartialEq)]
struct SessionGroup {
    id: QuerySessionId,
    ordinal: usize,
    runs: Vec<AgentRun>,
    theme: AgentsTheme,
    key: DiffKey,
}

impl KeyExt for SessionGroup {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for SessionGroup {
    fn render(&self) -> impl IntoElement {
        let mut open = use_state(|| true);
        let toggle = move |_: Event<PressEventData>| {
            let now = *open.peek();
            open.set(!now);
        };
        // The handle a card's press writes through — `Chan::Tabs` because that is what opening a
        // tab lives on, and one subscription for the whole list rather than one per card.
        let session = use_radio::<SessionState, Chan>(Chan::Tabs);
        // The satellite's own predicate — see `QuerySession::is_running` for why the pane
        // paints the driver's observation rather than asking the engine.
        let running = matches!(
            self.runs.first().map(|r| &r.outcome),
            Some(RunOutcome::Running)
        );
        // Read **unconditionally**, even though only a running session paints with it: a hook
        // reached from inside `running.then(|…|)` is a hook called a variable number of times
        // per render, which is the hook-order rule (AGENTS.md §3) and panics the whole app the
        // first time a session settles.
        let accent = use_theme().read().colors().primary;

        let cards: Vec<Element> = match open() {
            false => Vec::new(),
            true => self
                .runs
                .iter()
                .map(|run| {
                    RunCard {
                        session,
                        sql: run.sql.clone(),
                        outcome: run.outcome.clone(),
                        at: run.at,
                        theme: self.theme.clone(),
                        key: DiffKey::None,
                    }
                    .key(run.seq)
                    .into_element()
                })
                .collect(),
        };

        rect()
            .width(Size::fill())
            .vertical()
            .child(
                TreeItem::new()
                    .width(Size::fill())
                    .depth(1)
                    .theme(rows())
                    .disclosure(Disclosure::from_expanded(open()))
                    .arrow(chevron(open(), self.theme.chevron_color))
                    .on_press(toggle)
                    .child(
                        rect()
                            .width(Size::flex(1.))
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .spacing(6.)
                            .child(
                                // `Meta` for its type: the canvas asks for mono·500·11 and the
                                // scale's nearest slot is mono·500·10 — same family and weight,
                                // which is what distinguishes this label from the run preview
                                // below it (mono·400).
                                Meta::new(format!("Query session {}", self.ordinal))
                                    .color(self.theme.session_color)
                                    .width(Size::flex(1.))
                                    .max_lines(1)
                                    .text_overflow(TextOverflow::Ellipsis),
                            )
                            .maybe_child(running.then(|| {
                                // Accent, not a semantic slot: this says "working", which is
                                // neither good news nor bad.
                                Meta::new("RUNNING").color(accent)
                            }))
                            .child(
                                Meta::new(plural_of(self.runs.len(), "run"))
                                    .color(self.theme.meta_color),
                            ),
                    ),
            )
            // The cards sit one level in from their session row, by the tree's own indent, so a
            // run lines up under the session it belongs to without the card having to be a tree
            // row — which it cannot be: `Tree` rows are one fixed height and the canvas's card
            // is a figures line over a two-line SQL preview.
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    .padding(Gaps::new(0., 0., 0., RUN_INDENT))
                    .children(cards),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// `1 session` / `2 sessions` — [`plural`](strata_core::util::plural) without the thousands
/// grouping, which a count this small never needs and which would read oddly beside a chevron.
fn plural_of(n: usize, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        n => format!("{n} {noun}s"),
    }
}

/// The pane over a real satellite, mounted the way the sidebar mounts it.
///
/// These exist because they didn't. Nothing rendered anything above [`RunCard`], so a
/// **conditional hook** in `SessionGroup` — a `use_theme()` reached from inside
/// `running.then(…)`, which is a hook called a variable number of times per render — shipped
/// straight past a green suite and panicked the whole app the first time a session's run
/// settled. Every test here therefore drives a *transition*, not a snapshot: the bug was never
/// "the pane doesn't render", it was "the pane doesn't survive its own state changing".
#[cfg(test)]
mod tests {
    use freya::radio::RadioStation;
    use freya_testing::TestingRunner;
    use strata_agent::{Agent as AgentRef, AgentId, AgentIdentity};
    use strata_core::theme::load;
    use strata_core::util::collapse_sql;

    use super::*;
    use crate::apps::project::state::Agents as AgentsStore;
    use crate::theme::strata_theme;

    const SQL: &str = "SELECT country, sum(amount) FROM events GROUP BY 1";

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        rect().expanded().child(Agents::new())
    }

    type Handles = (AgentsCtx, RadioStation<SessionState, Chan>);

    fn runner() -> (TestingRunner, Handles) {
        TestingRunner::new(
            app,
            (260., 600.).into(),
            |r| {
                let agents = r.provide_root_context(|| State::create(AgentsStore::default()));
                let session = r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                });
                (agents, session)
            },
            1.,
        )
    }

    fn settle(runner: &mut TestingRunner) {
        for _ in 0..4 {
            runner.sync_and_update();
        }
    }

    fn texts(runner: &TestingRunner) -> Vec<String> {
        runner.find_many(|_, element| Label::try_downcast(element).map(|l| l.text.to_string()))
    }

    fn agent(name: &str) -> AgentRef {
        AgentRef {
            id: AgentId::new(),
            identity: AgentIdentity {
                name: name.into(),
                version: "2.1.4".into(),
            },
        }
    }

    /// **The regression.** A session renders with a run in flight, then that run settles — the
    /// `RUNNING` tag disappears, and the render that drops it must not drop a hook with it.
    #[test]
    fn a_session_survives_its_run_settling() {
        let (mut runner, (mut agents, _)) = runner();
        let who = agent("claude-code");
        let session = QuerySessionId::new();
        agents.write().opened(&who, session);
        let seq = agents.peek().next_run();
        agents.write().run_started(who.id, session, SQL.into());
        settle(&mut runner);
        assert!(texts(&runner).iter().any(|t| t == "RUNNING"));
        assert!(texts(&runner).iter().any(|t| t == "running…"));

        agents.write().run_settled(
            who.id,
            session,
            seq,
            RunOutcome::Rows {
                returned: 12,
                total: 4821,
                elapsed_ms: 132,
            },
        );
        settle(&mut runner);

        let after = texts(&runner);
        assert!(!after.iter().any(|t| t == "RUNNING"), "{after:?}");
        assert!(
            after.iter().any(|t| t == "12 of 4,821 rows · 132 ms"),
            "{after:?}"
        );
    }

    /// The tree the pane is: agent, then session, then run — with the counts each level states.
    #[test]
    fn the_pane_renders_agent_session_and_run() {
        let (mut runner, (mut agents, _)) = runner();
        let who = agent("claude-code");
        let first = QuerySessionId::new();
        agents.write().opened(&who, first);
        agents.write().opened(&who, QuerySessionId::new());
        agents.write().run_started(who.id, first, SQL.into());
        settle(&mut runner);

        let shown = texts(&runner);
        assert!(shown.iter().any(|t| t == "claude-code"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "2 sessions"), "{shown:?}");
        // Oldest session first, so the ordinals read 1 then 2 down the pane.
        assert!(shown.iter().any(|t| t == "Query session 1"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "Query session 2"), "{shown:?}");
        assert!(shown.iter().any(|t| t == "1 run"), "{shown:?}");
        assert!(
            shown.iter().any(|t| t.starts_with("SELECT country")),
            "{shown:?}"
        );
    }

    /// An agent arriving and then leaving — the other transition, and the one a disconnect
    /// drives. Empty is a state the pane goes *back* to, not just one it starts in.
    #[test]
    fn an_agent_arriving_and_leaving_swaps_the_empty_state() {
        let (mut runner, (mut agents, _)) = runner();
        settle(&mut runner);
        assert!(
            texts(&runner)
                .iter()
                .any(|t| t.starts_with("No agents connected")),
            "the empty state leads"
        );

        let who = agent("claude-code");
        agents.write().opened(&who, QuerySessionId::new());
        settle(&mut runner);
        assert!(texts(&runner).iter().any(|t| t == "claude-code"));

        agents.write().gone(who.id);
        settle(&mut runner);
        assert!(
            texts(&runner)
                .iter()
                .any(|t| t.starts_with("No agents connected")),
            "and it comes back when the last agent goes"
        );
    }

    /// **The other regression, and this one is geometry.** An expanded session used to claim the
    /// panel's whole remaining height — its block was a horizontal row holding a
    /// `Size::fill()`-height rule, and a `Fill` child in a hugging parent resolves against the
    /// space *available* to it rather than its content. On screen that was a screenful of nothing
    /// between one session's last run and the next session's row.
    ///
    /// Asserting on laid-out boxes rather than on the element tree is deliberate, as it is for
    /// the sidebar header's own tests: nothing was ever missing from the tree.
    #[test]
    fn an_expanded_session_does_not_claim_the_panel() {
        let (mut runner, (mut agents, _)) = runner();
        let who = agent("claude-code");
        let first = QuerySessionId::new();
        let second = QuerySessionId::new();
        agents.write().opened(&who, first);
        agents.write().opened(&who, second);
        agents.write().run_started(who.id, first, SQL.into());
        settle(&mut runner);

        let row = |runner: &TestingRunner, text: String| {
            runner
                .find(|node, element| {
                    Label::try_downcast(element)
                        .filter(|l| l.text == text)
                        .map(|_| node.layout().area)
                })
                .unwrap_or_else(|| panic!("no row {text:?}"))
        };
        let run = row(&runner, collapse_sql(SQL));
        let next = row(&runner, "Query session 2".to_string());

        // The next session follows its predecessor's last run, not the bottom of a 600pt panel.
        assert!(next.min_y() > run.min_y(), "session 2 is below the run");
        assert!(
            next.min_y() - run.max_y() < 40.,
            "expected session 2 just under the run, found a {}pt gap",
            next.min_y() - run.max_y()
        );
    }

    /// A failed run reads as its message, and a stop as the engine's own wording — never as one
    /// another, which is the distinction the driver judges once on the way in.
    #[test]
    fn a_failure_and_a_stop_read_as_themselves() {
        let (mut runner, (mut agents, _)) = runner();
        let who = agent("claude-code");
        let session = QuerySessionId::new();
        agents.write().opened(&who, session);
        for outcome in [
            RunOutcome::Failed("Schema error: No field named nope".into()),
            RunOutcome::Stopped("superseded by a newer run".into()),
        ] {
            let seq = agents.peek().next_run();
            agents.write().run_started(who.id, session, SQL.into());
            agents.write().run_settled(who.id, session, seq, outcome);
        }
        settle(&mut runner);

        let shown = texts(&runner);
        assert!(
            shown
                .iter()
                .any(|t| t == "Schema error: No field named nope"),
            "{shown:?}"
        );
        assert!(
            shown.iter().any(|t| t == "superseded by a newer run"),
            "{shown:?}"
        );
    }
}
