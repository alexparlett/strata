//! The editor's Run control — a purpose-built button with three visual states (idle / disabled /
//! running). Themed via `define_theme!`; its colours are defined wholly in the theme file's
//! `components.run_button` (see `crate::theme`). Idle runs the query, running shows a stop glyph,
//! disabled is inert (its press never fires). The tooltip is the comp's `runTitle` — keymap-derived
//! per state ("Run (⌘↵)" / "Cancel query (Esc)"), "Enter a query to run" while disabled (a blank
//! buffer is the one disabled cause the Freya toolbar models).

use freya::prelude::*;
use strata_core::config::Command;

use crate::components::icon::{Icon, IconName};

define_theme!(
    %[component]
    pub RunButton {
        %[fields]
        background: Color,
        hover_background: Color,
        color: Color,
        disabled_background: Color,
        disabled_hover_background: Color,
        disabled_color: Color,
        running_background: Color,
        running_hover_background: Color,
        running_color: Color,
    }
);

/// Which of the three states the Run button is in.
#[derive(PartialEq, Clone, Copy)]
pub enum RunState {
    Idle,
    Disabled,
    Running,
}

#[derive(PartialEq)]
pub struct RunButton {
    state: RunState,
    theme: Option<RunButtonThemePartial>,
    on_press: Option<EventHandler<Event<PressEventData>>>,
}

impl RunButton {
    pub fn new(state: RunState) -> Self {
        Self {
            state,
            theme: None,
            on_press: None,
        }
    }

    /// The press action for the *current* state — run when idle, cancel when running
    /// (the caller decides; disabled swallows it).
    pub fn on_press(mut self, on_press: impl Into<EventHandler<Event<PressEventData>>>) -> Self {
        self.on_press = Some(on_press.into());
        self
    }
}

impl Component for RunButton {
    fn render(&self) -> impl IntoElement {
        let RunButtonTheme {
            background,
            hover_background,
            color,
            disabled_background,
            disabled_hover_background,
            disabled_color,
            running_background,
            running_hover_background,
            running_color,
        } = get_theme!(&self.theme, RunButtonThemePreference, "run_button");

        // (resting, hover, foreground) for the current state.
        let (base, hover, fg) = match self.state {
            RunState::Idle => (background, hover_background, color),
            RunState::Disabled => (disabled_background, disabled_hover_background, disabled_color),
            RunState::Running => (running_background, running_hover_background, running_color),
        };

        let mut hovered = use_state(|| false);
        // Disabled is inert — no hover response.
        let bg = if hovered() && self.state != RunState::Disabled {
            hover
        } else {
            base
        };
        // Running shows a stop glyph (click to cancel); idle/disabled show play.
        let icon = if self.state == RunState::Running {
            IconName::Stop
        } else {
            IconName::Play
        };

        let on_press = self.on_press.clone();
        let disabled = self.state == RunState::Disabled;

        // The comp's state-dependent `runTitle`. Both hints resolve unconditionally (hooks),
        // then the state picks.
        let run_title = crate::keymap::use_hint_title("Run", Command::RunQuery);
        let cancel_title = crate::keymap::use_hint_title("Cancel query", Command::Cancel);
        let title = match self.state {
            RunState::Idle => run_title,
            RunState::Running => cancel_title,
            RunState::Disabled => "Enter a query to run".to_string(),
        };

        TooltipContainer::new(Tooltip::new(title))
            .position(AttachedPosition::Bottom)
            .child(
                rect()
                    .width(Size::px(28.))
                    .height(Size::px(28.))
                    .corner_radius(6.)
                    .background(bg)
                    .center()
                    .on_pointer_enter(move |_| hovered.set(true))
                    .on_pointer_leave(move |_| hovered.set(false))
                    .map(on_press, move |el, on_press| {
                        el.on_press(move |e| {
                            if !disabled {
                                on_press.call(e);
                            }
                        })
                    })
                    .child(Icon::new(icon).color(fg).size(15.)),
            )
    }
}
