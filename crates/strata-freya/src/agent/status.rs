//! The header's **agent-access status dot** (AA-03): whether the server is listening, and
//! whether anything is paired with it.
//!
//! ## Why this is polled, when almost nothing else in the app is
//!
//! Every other live fact here is written by whoever observed it and read reactively. This one
//! cannot be: the fact is *rmcp's*, held in `LocalSessionManager`'s own map, and a session is
//! created and destroyed inside `service.handle(req)` — below our `serve`, with nothing on our
//! side of the seam to notice. The alternatives were both worse than a sample. Wrapping
//! `SessionManager` to count `create_session` / `close_session` is ten pass-through methods to
//! learn a number that is already `pub`; a channel from the server thread needs a receiver, and
//! a receiver can only be taken once, which is exactly what a status shown in *every* project
//! window must not depend on.
//!
//! So the dot samples. It costs a `peek` and an uncontended `try_read` every
//! [`POLL`], and it costs it **only while a server is configured**: [`AgentStatusDot`] renders
//! [`AgentDot`] — which is where the hooks live — only when the setting is on, so the default
//! (off) app has no timer at all. `try_read` rather than a wait for the same reason: a status
//! light is not worth a frame, and a sample that could not be taken leaves the last one
//! standing rather than reporting a disconnection that did not happen.

use std::time::Duration;

use async_io::Timer;
use freya::components::{use_theme, AttachedPosition, Tooltip, TooltipContainer};
use freya::prelude::*;

use crate::components::dot::Dot;
use crate::state::{use_config, ConfigChan};

use super::AgentCtx;

/// How often the dot re-samples. Slow on purpose: a client pairing or going is not something
/// anyone is watching for to the second, and this ticks for as long as a window is open.
const POLL: Duration = Duration::from_secs(2);

/// The dot's diameter — smaller than the 30×30 buttons beside it, because it is a state, not a
/// control.
const SIZE: f32 = 8.;

/// What the dot says.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Presence {
    /// The setting is on but nothing is listening — the port was taken, or the server has not
    /// come up yet. A condition worth showing: it is the one case where the user has asked for
    /// agent access and has not got it.
    Down,
    /// Listening, with nothing paired.
    Waiting,
    /// At least one MCP client is paired.
    Connected(usize),
}

impl Presence {
    fn tooltip(self) -> String {
        match self {
            Presence::Down => "Agent access is enabled but not listening. See the log".into(),
            Presence::Waiting => "Agent access is listening. No agent is connected".into(),
            Presence::Connected(1) => "1 agent is connected".into(),
            Presence::Connected(n) => format!("{n} agents are connected"),
        }
    }
}

/// The dot, or nothing at all.
///
/// Two components rather than one `maybe`, because the hooks belong to the half that is
/// conditional: a component that is not mounted runs no timer, which is what makes the feature
/// free for everyone who has not turned it on.
#[derive(PartialEq)]
pub struct AgentStatusDot {
    pub agent: AgentCtx,
}

impl Component for AgentStatusDot {
    fn render(&self) -> impl IntoElement {
        let enabled = use_config(ConfigChan::Settings)
            .read()
            .settings
            .agent_access
            .enabled;
        rect().maybe_child(enabled.then(|| AgentDot {
            agent: self.agent.clone(),
        }))
    }
}

#[derive(PartialEq)]
struct AgentDot {
    agent: AgentCtx,
}

impl Component for AgentDot {
    fn render(&self) -> impl IntoElement {
        let theme = use_theme();
        let (success, warning, idle) = {
            // The sheet's semantic ramp, reached directly: a status tone follows the app-wide
            // colours wherever it appears (AGENTS.md §3).
            let t = theme.read();
            (
                t.colors().success,
                t.colors().warning,
                t.colors().text_placeholder,
            )
        };
        let mut presence = use_state(|| Presence::Waiting);
        let agent = self.agent.clone();
        use_hook(move || {
            spawn(async move {
                loop {
                    // `set_if_modified`, so a repaint costs a render only when the answer
                    // actually moved — this runs for the life of the window.
                    if let Some(sampled) = sample(&agent) {
                        presence.set_if_modified(sampled);
                    }
                    Timer::after(POLL).await;
                }
            });
        });

        let presence = *presence.read();
        let color = match presence {
            Presence::Down => warning,
            Presence::Waiting => idle,
            Presence::Connected(_) => success,
        };
        TooltipContainer::new(Tooltip::new_text(presence.tooltip()))
            .position(AttachedPosition::Bottom)
            .child(
                // Padded to the cluster's touch height so the tooltip has something to hover
                // and the dot still reads as a small mark.
                rect()
                    .height(Size::px(30.))
                    .padding((0., 4.))
                    .center()
                    .child(Dot::new(color).size(SIZE)),
            )
    }
}

/// One sample, or `None` when the session count could not be read without waiting — in which
/// case the caller keeps what it last saw. A server that is *not* running answers
/// [`Presence::Down`] outright, since that needs no lock.
fn sample(agent: &AgentCtx) -> Option<Presence> {
    let server = agent.server.peek();
    let Some(running) = server.as_ref() else {
        return Some(Presence::Down);
    };
    Some(match running.clients()? {
        0 => Presence::Waiting,
        n => Presence::Connected(n),
    })
}
