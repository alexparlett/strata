//! The transcript's two cards — and they are two *because* they mean different things.
//!
//! - [`StepCard`] is a **citation**. A tool ran; here is what it ran and what it cost. Every
//!   figure on it is the engine's own (`elapsed_ms`, the exact row total, the stop's own
//!   wording), because AS-02's prompt says no number in prose without a run behind it and this
//!   card is what makes that auditable. A `run` gets the promote presses too, since its statement
//!   is a thing the user can take; every other tool is one line, expandable to what it answered.
//!
//! - [`OfferCard`] is **executable**. It arrives only from `offer_sql`, which validated the
//!   statement against the catalog and the *editor's* policy before this card existed — so a Run
//!   press cannot be offering something that will not parse, and it may legitimately carry a
//!   write the assistant is itself refused, because the user runs it under their own capability.
//!
//! Both promote through the editor's own [`actions::open_sql`], never by writing the user's
//! buffer. *Run* is the same funnel with Run pressed on arrival, which is what makes a promoted
//! query record into history like any user press — the **adoption** rule — while the assistant's
//! own runs never do.

use freya::prelude::*;
use freya::radio::Radio;
use strata_core::util::{collapse_sql, fmt_int, plural};

use super::ChatTheme;
use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::state::{Chan, SessionState, Step};
use crate::apps::project::views::workbench::editor::actions;
use crate::components::divider::Divider;
use crate::components::dot::Dot;
use crate::components::icon::{Icon, IconName};
use crate::components::tones::tones;
use crate::components::typography::{Meta, Path, Readout};
use crate::theme::{use_roles, Role};

pub(super) const CARD_RADIUS: f32 = 6.;
pub(super) const CARD_PAD: Gaps = Gaps::new(8., 10., 8., 10.);
/// The action bar under a card's body (canvas `var(--sp-2) var(--sp-3)`).
pub(super) const ACTIONS_PAD: Gaps = Gaps::new(4., 6., 4., 6.);
const DOT: f32 = 6.;
/// How many lines of a step's SQL preview show before it truncates.
const PREVIEW_LINES: usize = 2;

/// One tool round.
#[derive(PartialEq)]
pub struct StepCard {
    pub step: Step,
    pub session: Radio<SessionState, Chan>,
    pub theme: ChatTheme,
}

impl Component for StepCard {
    fn render(&self) -> impl IntoElement {
        let tones = tones();
        let accent = use_roles().get(Role::Accent);
        let step = &self.step;
        let theme = &self.theme;

        // **A stop is a status, never a failure.** `failed` is the fault flag and a stop does not
        // set it, so the two arms are read in that order and the stop keeps the engine's own
        // wording rather than being restated here.
        let (dot, figures, figures_color) = match (&step.failed, &step.facts.stopped) {
            (None, _) => (accent, "running…".to_string(), theme.figures_color),
            (Some(_), Some(reason)) => (tones.warning, reason.clone(), theme.figures_color),
            (Some(true), None) => (tones.error, "the call failed".to_string(), tones.error),
            (Some(false), None) => (tones.ok, cost(step), theme.figures_color),
        };

        let card = rect()
            .width(Size::fill())
            .vertical()
            .corner_radius(CARD_RADIUS)
            .background(theme.card_background)
            .border(Border::new().width(1.).fill(theme.card_border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .vertical()
                    // A painted border is not laid out, so the body's own inset carries it.
                    .padding(CARD_PAD)
                    .spacing(4.)
                    .child(
                        rect()
                            .width(Size::fill())
                            .horizontal()
                            .content(Content::Flex)
                            .cross_align(Alignment::Center)
                            .spacing(6.)
                            .child(Dot::new(dot).size(DOT))
                            .child(Meta::new(step.tool.clone()).color(theme.role_color))
                            .child(
                                Meta::new(figures)
                                    .color(figures_color)
                                    .width(Size::flex(1.))
                                    .max_lines(1)
                                    .text_overflow(TextOverflow::Ellipsis),
                            ),
                    )
                    // The statement, for the tools that take one — collapsed to the History
                    // drawer's one-liner, which is the same preview an agent's run gets.
                    .maybe_child(step.facts.sql.as_ref().map(|sql| {
                        Path::new(collapse_sql(sql))
                            .color(theme.sql_color)
                            .width(Size::fill())
                            .max_lines(PREVIEW_LINES)
                            .text_overflow(TextOverflow::Ellipsis)
                    })),
            );

        // Only a statement can be promoted, so only a card that has one carries the presses.
        match &step.facts.sql {
            None => card,
            Some(sql) => card
                .child(Divider::horizontal().color(theme.card_border_fill))
                .child(Actions {
                    sql: sql.clone(),
                    session: self.session,
                    theme: theme.clone(),
                }),
        }
    }
}

/// What a settled call cost — **returned rows and the engine's own elapsed**, and nothing when
/// the tool counts neither (a `describe_table` has no rows and no statement).
fn cost(step: &Step) -> String {
    match (step.facts.rows, step.facts.elapsed_ms) {
        (Some(rows), Some(ms)) => format!("{} · {} ms", plural(rows, "row"), fmt_int(ms)),
        (Some(rows), None) => plural(rows, "row"),
        (None, Some(ms)) => format!("{} ms", fmt_int(ms)),
        (None, None) => "done".to_string(),
    }
}

/// A statement the assistant handed over through `offer_sql`, with its two presses.
#[derive(PartialEq)]
pub struct OfferCard {
    pub sql: String,
    pub session: Radio<SessionState, Chan>,
    pub theme: ChatTheme,
}

impl Component for OfferCard {
    fn render(&self) -> impl IntoElement {
        rect()
            .width(Size::fill())
            .vertical()
            .corner_radius(CARD_RADIUS)
            .background(self.theme.card_background)
            .border(Border::new().width(1.).fill(self.theme.card_border_fill))
            .child(
                rect()
                    .width(Size::fill())
                    .padding(CARD_PAD)
                    // In full, not collapsed: this is a statement the user is being asked to run,
                    // so what they read has to be what runs.
                    .child(
                        Readout::new(self.sql.clone())
                            .color(self.theme.sql_color)
                            .width(Size::fill())
                            .wrap(),
                    ),
            )
            .child(Divider::horizontal().color(self.theme.card_border_fill))
            .child(Actions {
                sql: self.sql.clone(),
                session: self.session,
                theme: self.theme.clone(),
            })
    }
}

/// The two promote presses, shared by both cards so they can never drift apart.
///
/// **Two presses, Snowflake's Run/Add shape.** In a data tool the check on a statement is the
/// grid updating, not a diff read — so *Run* opens the tab and runs it, and *Open in tab* leaves
/// the decision with the user. Both land in a **new**, focused tab: nothing here writes the
/// buffer the user is working in.
#[derive(PartialEq)]
struct Actions {
    sql: String,
    session: Radio<SessionState, Chan>,
    theme: ChatTheme,
}

impl Component for Actions {
    fn render(&self) -> impl IntoElement {
        let session = self.session;
        let engine = use_consume::<EngineCtx>();
        let color = self.theme.meta_color;

        let action = move |icon: IconName, text: &'static str, run: bool, sql: String| {
            Button::new()
                .flat()
                .height(Size::px(24.))
                .on_press({
                    let engine = engine.clone();
                    move |_| {
                        let id = actions::open_sql(session, &sql);
                        if run {
                            // The editor's own Run, on the tab that was just opened — so a promoted
                            // query is an ordinary press in every respect, history included.
                            actions::run_query(&engine, session, id);
                        }
                    }
                })
                .child(
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Center)
                        .spacing(6.)
                        .child(Icon::new(icon).size(12.).color(color))
                        .child(Meta::new(text).color(color)),
                )
        };

        rect()
            .width(Size::fill())
            .horizontal()
            .cross_align(Alignment::Center)
            .padding(ACTIONS_PAD)
            .spacing(4.)
            .child(action(
                IconName::Plus,
                "Open in tab",
                false,
                self.sql.clone(),
            ))
            .child(action(IconName::Play, "Run", true, self.sql.clone()))
    }
}
