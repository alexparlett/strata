//! One run an agent made: what it asked, what it cost, and the press that brings it into the
//! user's editor.
//!
//! The History drawer's card in the sidebar's width, and the same card on purpose — it is the
//! same thing, a past query you can act on.
//!
//! ## The press opens a **new** tab
//!
//! Through the editor's own [`actions::open_sql`], so a promoted agent query is an ordinary tab
//! holding ordinary text: editable, runnable, saveable, undoable. A fresh tab rather than the
//! active one is the load-bearing half — overwriting the buffer the user is working in is the
//! precise harm this whole pane exists to prevent, and a load-into-active gesture (which is what
//! the History drawer does, because *there* the user asked for it by being in that tab) would
//! put it straight back.
//!
//! There is deliberately **no double-press to run**. Promoting is putting a query where the user
//! can read it; pressing Run is their next decision, not this row's — and the tab it lands in
//! has a Run button an inch away.
//!
//! ## What differs from a History row
//!
//! An **outcome**. The satellite records failures and stops as well as successes (unlike
//! history, which records only successful data runs), so the figures line has four shapes and a
//! leading dot to tell them apart. The stop arm is why the driver judges `stopped_on_purpose` on
//! the way in rather than leaving a string for this to read: a supersede painted red is a fault
//! the user never had.

use freya::prelude::*;
use freya::radio::Radio;
use strata_core::util::{ago, collapse_sql, fmt_int, now_secs, plural};

use super::AgentsTheme;
use crate::agent::RunOutcome;
use crate::apps::project::state::{Chan, SessionState};
use crate::apps::project::views::workbench::editor::actions;
use crate::components::dot::Dot;
use crate::components::tones::tones;
use crate::components::typography::{Meta, Path};
use crate::theme::{use_roles, Role};

/// A card's inner padding (canvas `--sp-3` / `--sp-4`), its radius (`--r-2`) and the gap under
/// it (`--sp-1`).
const CARD_PAD_Y: f32 = 6.;
const CARD_PAD_X: f32 = 8.;
const CARD_RADIUS: f32 = 6.;
const CARD_GAP: f32 = 2.;
/// The gap between the figures line and the SQL under it (`--sp-2`), and within that line
/// (`--sp-3`).
const META_GAP: f32 = 4.;
const META_SPACING: f32 = 6.;
/// How many lines of the SQL preview are shown before it truncates (`-webkit-line-clamp: 2`).
const PREVIEW_LINES: usize = 2;
/// The status dot's diameter.
const DOT: f32 = 6.;

#[derive(PartialEq)]
pub struct RunCard {
    /// The session store the press writes through — taken as a prop rather than consumed here,
    /// so a session's whole list of cards shares one subscription.
    pub session: Radio<SessionState, Chan>,
    pub sql: String,
    pub outcome: RunOutcome,
    /// Unix seconds at dispatch.
    pub at: u64,
    pub theme: AgentsTheme,
    pub key: DiffKey,
}

impl KeyExt for RunCard {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for RunCard {
    fn render(&self) -> impl IntoElement {
        let mut hovered = use_state(|| false);
        let session = self.session;
        let sql = self.sql.clone();
        // Semantic (and the accent for "working"), so the outcome tones follow the app-wide ramp
        // wherever they appear — the one place this surface reads the shared ramp rather than its
        // own theme (AGENTS.md §3).
        let tones = tones();
        let (success, warning, error, accent) = (
            tones.ok,
            tones.warning,
            tones.error,
            use_roles().get(Role::Accent),
        );
        let figures_color = self.theme.figures_color;
        let (dot, figures, figures_color) = match &self.outcome {
            RunOutcome::Running => (accent, "running…".to_string(), figures_color),
            // **Returned of matched**, which is two facts and not one: `run` injects no `LIMIT`,
            // so the total is exact and the page is what the agent actually read back.
            RunOutcome::Rows {
                returned,
                total,
                elapsed_ms,
            } => (
                success,
                format!(
                    "{} of {} · {} ms",
                    fmt_int(*returned),
                    plural(*total as usize, "row"),
                    fmt_int(*elapsed_ms)
                ),
                figures_color,
            ),
            RunOutcome::Plan { analyze } => (
                success,
                match analyze {
                    true => "explained with analyze".to_string(),
                    false => "explained".to_string(),
                },
                figures_color,
            ),
            // The engine's own wording, never restated — the event log's rule, so one stop
            // cannot be described two ways.
            RunOutcome::Stopped(reason) => (warning, reason.clone(), figures_color),
            RunOutcome::Failed(message) => (error, message.clone(), error),
        };

        rect()
            .width(Size::fill())
            .vertical()
            .margin(Gaps::new(0., 0., CARD_GAP, 0.))
            .padding((CARD_PAD_Y, CARD_PAD_X))
            .corner_radius(CARD_RADIUS)
            .background(match hovered() {
                true => self.theme.card_hover_fill,
                false => Color::TRANSPARENT,
            })
            .on_pointer_enter(move |_| hovered.set(true))
            .on_pointer_leave(move |_| hovered.set(false))
            .on_press(move |_| {
                actions::open_sql(session, &sql);
            })
            .child(
                rect()
                    .width(Size::fill())
                    .horizontal()
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .spacing(META_SPACING)
                    .margin(Gaps::new(0., 0., META_GAP, 0.))
                    .child(Dot::new(dot).size(DOT))
                    .child(
                        Meta::new(figures)
                            .color(figures_color)
                            .width(Size::flex(1.))
                            .max_lines(1)
                            .text_overflow(TextOverflow::Ellipsis),
                    )
                    .child(
                        Meta::new(ago(now_secs().saturating_sub(self.at)))
                            .color(self.theme.meta_color),
                    ),
            )
            // `Path` for its **type**, not its name: the scale's mono·400·11 slot is exactly the
            // preview's spec, and a role fixes the type, not the subject.
            .child(
                Path::new(collapse_sql(&self.sql))
                    .color(self.theme.sql_color)
                    .width(Size::fill())
                    .max_lines(PREVIEW_LINES)
                    .text_overflow(TextOverflow::Ellipsis),
            )
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The card driven the way the pane drives it — over a real session store, so the promotion is
/// testable end to end.
#[cfg(test)]
mod tests {
    use freya::components::get_theme;
    use freya::radio::{use_radio, RadioStation};
    use freya_testing::TestingRunner;
    use strata_core::theme::load;

    use super::super::{AgentsThemePartial, AgentsThemePreference};
    use super::*;
    use crate::theme::strata_theme;

    const SQL: &str = "SELECT country,\n       sum(amount)\nFROM events\nGROUP BY 1";

    fn app() -> impl IntoElement {
        use_init_theme(|| strata_theme(&load("midnight")));
        let theme = get_theme!(&None::<AgentsThemePartial>, AgentsThemePreference, "agents");
        let session = use_radio::<SessionState, Chan>(Chan::Tabs);
        rect().expanded().child(RunCard {
            session,
            sql: SQL.into(),
            outcome: RunOutcome::Rows {
                returned: 100,
                total: 4821,
                elapsed_ms: 132,
            },
            at: now_secs(),
            theme,
            key: DiffKey::None,
        })
    }

    fn runner() -> (TestingRunner, RadioStation<SessionState, Chan>) {
        TestingRunner::new(
            app,
            (260., 300.).into(),
            |r| {
                r.provide_root_context(|| {
                    RadioStation::<SessionState, Chan>::create(SessionState::default())
                })
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

    fn click_preview(runner: &mut TestingRunner) {
        let text = collapse_sql(SQL);
        let area = runner
            .find(|node, element| {
                Label::try_downcast(element)
                    .filter(|l| l.text == text)
                    .map(|_| node.layout().area)
            })
            .expect("the SQL preview is on screen");
        let point = (
            (area.min_x() + area.width() / 2.) as f64,
            (area.min_y() + area.height() / 2.) as f64,
        );
        runner.move_cursor(point);
        runner.click_cursor(point);
        settle(runner);
    }

    /// **A press opens a new tab and leaves the user's alone.** The gesture AA-03b replaced an
    /// automatic tab open with — and the half that matters is the *new*: the tab the user was
    /// working in keeps its text and keeps being theirs.
    #[test]
    fn pressing_a_run_opens_it_in_a_new_tab() {
        let (mut runner, session) = runner();
        let mut store = session;
        let mine = store.write_channel(Chan::Tabs).open_blank();
        if let Some(tab) = store.write_channel(Chan::Tab(mine)).tabs.get_mut(&mine) {
            tab.editor.set_text("SELECT keep_me");
        }
        settle(&mut runner);

        click_preview(&mut runner);

        let after = session.peek();
        assert_eq!(after.tabs.len(), 2, "a fresh tab, not the one I was in");
        assert_eq!(
            after.tabs.get(&mine).unwrap().text(),
            "SELECT keep_me",
            "my buffer is untouched"
        );
        let opened = after.active.expect("the new tab takes focus");
        assert_ne!(opened, mine);
        assert_eq!(after.tabs.get(&opened).unwrap().text(), SQL);
        // Promoting is not running: the Run button in that tab is the user's next decision.
        assert!(after.request(opened).is_none());
    }

    /// Two presses are two tabs, not a load-then-run: there is no double-press gesture here, so
    /// nothing has to tell one press from two.
    #[test]
    fn a_second_press_opens_a_second_tab_and_still_runs_nothing() {
        let (mut runner, session) = runner();
        settle(&mut runner);

        click_preview(&mut runner);
        click_preview(&mut runner);

        let after = session.peek();
        assert_eq!(after.tabs.len(), 2);
        assert!(after.tabs.values().all(|t| t.text() == SQL));
        assert!(after.tabs.keys().all(|id| after.request(*id).is_none()));
    }

    /// A settled run states **what came back of what matched**, and how long it took — two facts,
    /// because `run` injects no `LIMIT`, so the total is exact and the page is what the agent
    /// actually read.
    #[test]
    fn a_row_states_returned_of_matched() {
        let (mut runner, _) = runner();
        settle(&mut runner);

        let texts = texts(&runner);
        assert!(
            texts.iter().any(|t| t == "100 of 4,821 rows · 132 ms"),
            "{texts:?}"
        );
    }
}
