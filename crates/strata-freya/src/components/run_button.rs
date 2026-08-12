//! The editor's Run control — a purpose-built button with three visual states (idle / disabled /
//! running). Themed via `define_theme!`; its colours are the `run_button` rows of the mapping
//! table (see `crate::theme`). Idle runs the query, running shows a stop glyph,
//! disabled is inert (its press never fires). The tooltip is the comp's `runTitle` — keymap-derived
//! per state ("Run (⌘↵)" / "Cancel query (Esc)"), "Enter a query to run" while disabled (a blank
//! buffer is the one disabled cause the Freya toolbar models).

use freya::prelude::*;
use strata_core::config::Command;

use crate::components::icon::{Icon, IconName};
use crate::components::metrics::R_1;
use crate::keymap::use_hint_title;

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
        /// The keyboard focus ring, shared by all three states: it says where the keyboard is,
        /// which is not something Run/Stop/disabled answer differently.
        focus_border_fill: Color,
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
            focus_border_fill,
        } = get_theme!(&self.theme, RunButtonThemePreference, "run_button");

        // (resting, hover, foreground) for the current state.
        let (base, hover, fg) = match self.state {
            RunState::Idle => (background, hover_background, color),
            RunState::Disabled => (
                disabled_background,
                disabled_hover_background,
                disabled_color,
            ),
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

        let a11y_id = use_a11y();
        let focus = use_focus(a11y_id);
        // Only the keyboard gets a ring: a press focuses the button too, so an any-focus ring
        // would sit on Run for the rest of the session after the first click.
        let focus_ring = (focus() == Focus::Keyboard).then(|| {
            Border::new()
                .fill(focus_border_fill)
                .width(2.)
                .alignment(BorderAlignment::Inner)
        });

        // The comp's state-dependent `runTitle`. Both hints resolve unconditionally (hooks),
        // then the state picks.
        let run_title = use_hint_title("Run", Command::RunQuery);
        let cancel_title = use_hint_title("Cancel query", Command::Cancel);
        let title = match self.state {
            RunState::Idle => run_title,
            RunState::Running => cancel_title,
            RunState::Disabled => "Enter a query to run".to_string(),
        };

        TooltipContainer::new(Tooltip::new_text(title.clone()))
            .position(AttachedPosition::Bottom)
            .child(
                rect()
                    .width(Size::px(28.))
                    .height(Size::px(28.))
                    .corner_radius(R_1)
                    .background(bg)
                    .border(focus_ring)
                    .center()
                    .a11y_id(a11y_id)
                    .a11y_focusable(!disabled)
                    .a11y_role(AccessibilityRole::Button)
                    // The tooltip names the button for the pointer; the same string names it
                    // for the keyboard, which is now a tab stop and would otherwise announce
                    // as an unlabelled button (the child is a raw-SVG `Icon`). It lands on
                    // *this* rect because this is the focusable node.
                    .a11y_alt(title)
                    .on_pointer_enter(move |_| hovered.set(true))
                    .on_pointer_leave(move |_| hovered.set(false))
                    // `on_press` covers the OS activation keys as well as the pointer, so
                    // focusing on press is all a keyboard operator needs (Freya's own `Button`
                    // does the same).
                    .map(on_press, move |el, on_press| {
                        el.on_press(move |e| {
                            if !disabled {
                                a11y_id.request_focus();
                                on_press.call(e);
                            }
                        })
                    })
                    .child(Icon::new(icon).color(fg).size(15.)),
            )
    }
}
