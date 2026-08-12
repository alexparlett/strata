//! **Settings ▸ AI ▸ MCP** (AA-04, design `Settings.dc.html`) — the control for the in-app
//! MCP server AA-03 ships dark: the switch, the port, and the token.
//!
//! Three rows and no more. A client-setup line and a live server status were both sketched and
//! both **descoped**: the first is one client's incantation (`claude mcp add …`) on a surface
//! that has no business favouring a client, and the README is where every client's setup
//! belongs; the second is a live reading this pane would have to poll for. The header's status
//! dot covered it until it was removed, so the app now shows agent-access liveness **nowhere**:
//! a server that cannot bind (a port another process holds) says so in the tracing log and not
//! on screen. Worth a Problems condition if it comes up — it is true now and retracts itself,
//! which is that surface's test — but not a second poll on this pane.
//!
//! Every control writes [`SettingsCtx::draft`] and stops there; the footer's Apply commits. The
//! reader on the other side of that commit is `agent::use_agent_server`, a reconciler every
//! workspace window mounts — so applying starts, stops or restarts the server without an app
//! restart, and this pane needs no lifecycle code of its own.
//!
//! **Regenerate is a draft edit, not an immediate write**, which is a deliberate divergence
//! from the canvas's subtext ("takes effect at once"). Two reasons, and the second settles it.
//! `Settings::merge_onto` diffs whole fields and `agent_access` is one field, so a token
//! committed behind the draft's back would be overwritten by the very next Apply that carried a
//! changed switch. And a credential every client depends on should have an undo — Cancel is it,
//! which is also why this action needs no confirm of its own.
//!
//! Each row is built from its [`Anchor`] (P4-09), which is where its title and subtext live.

use freya::prelude::*;
use strata_agent::mint_token;
use strata_core::config::{AGENT_PORT_MAX, AGENT_PORT_MIN};

use crate::apps::settings::views::Pane;
use crate::apps::settings::{Anchor, SettingsCtx};
use crate::components::form::{Form, NumberField, ValueField, FIELD_HEIGHT};
use crate::components::icon::{Icon, IconName};
use crate::components::tool_button::ToolButton;
use crate::components::typography::Control;

/// The canvas's numeric field (`width: 130px`) — the same box the system and data-display
/// panes' are.
const PORT_WIDTH: f32 = 130.;

/// The gap between the token box and the actions beside it, and between those actions.
const ACTION_GAP: f32 = 8.;

#[derive(PartialEq)]
pub struct McpPane;

impl Component for McpPane {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        // Whether the secret is on screen. Pane-local: a reveal is a glance, not a setting —
        // nothing outside this page reads it and nothing should persist it.
        let mut revealed = use_state(|| false);

        // Read in a block: the guard has to be gone before anything below takes a write one on
        // the same `State`.
        //
        // `minted` is the one thing the pane has to branch on. The token is empty until agent
        // access is first enabled — `agent::server::reconcile` returns on `!enabled` *before*
        // its minting branch — so on a fresh install this page is reached with nothing to show
        // and nothing to copy.
        let (enabled, port, minted) = {
            let draft = ctx.draft.read();
            (
                draft.agent_access.enabled,
                draft.agent_access.port,
                !draft.agent_access.token.is_empty(),
            )
        };

        // The token box is a **read-only display** of a value that lives in the draft, and
        // `ValueField` binds a `State<String>`. So it is mirrored in an effect rather than
        // seeded once: a `use_state` initializer runs on the first render only, and Regenerate
        // would leave the box showing the token it had just replaced.
        let mut token_box = use_state(String::new);
        use_side_effect(move || {
            let token = ctx.draft.read().agent_access.token.clone();
            token_box.set_if_modified(token);
        });

        let masked = !*revealed.read();
        let reveal_label = match masked {
            true => "Show the token",
            false => "Hide the token",
        };

        let body = Form::new()
            .preferences()
            .child(
                Anchor::AgentEnabled
                    .row()
                    .trailing()
                    .on_press(move |_: Event<PressEventData>| {
                        ctx.edit(|s| s.agent_access.enabled = !s.agent_access.enabled);
                    })
                    .child(Switch::new().toggled(enabled).on_toggle(move |()| {
                        ctx.edit(|s| s.agent_access.enabled = !s.agent_access.enabled);
                    })),
            )
            .child(
                Anchor::AgentPort.row().child(
                    NumberField::new(port as u32, AGENT_PORT_MIN as u32, AGENT_PORT_MAX as u32)
                        .width(Size::px(PORT_WIDTH))
                        // In range by construction: the field clamps before it reports, and the
                        // range is the port number's own.
                        .on_change(move |port: u32| {
                            ctx.edit(|s| s.agent_access.port = port as u16);
                        }),
                ),
            )
            .child(
                Anchor::AgentToken.row().child(
                    rect()
                        .width(Size::fill())
                        .horizontal()
                        .content(Content::Flex)
                        .cross_align(Alignment::Center)
                        .spacing(ACTION_GAP)
                        .child(
                            ValueField::new(token_box)
                                .width(Size::flex(1.))
                                .masked(masked)
                                // Said out loud, because a masked box and an empty one look
                                // identical: before the first enable there is no token, and a
                                // blank field with a Copy button beside it reads as a token
                                // that simply isn't being shown.
                                .placeholder("Not minted yet")
                                // A minted credential, not a field: it is displayed and copied,
                                // never typed. The actions beside it are the whole of what can
                                // be done to it.
                                .enabled(false),
                        )
                        // Named divergence from the canvas, which sets the reveal *inside* the
                        // box. An icon button in this app is a 28×28 square (`ToolButton`) and a
                        // value box stands at 30, so the canvas's in-box 24 would be a
                        // hand-rolled lookalike of the one control the app already has. Reveal
                        // and copy read as one cluster of actions on the value instead.
                        // Both gated on there being a token: revealing nothing is a no-op, and a
                        // Copy that puts an empty string on the clipboard is worse than one that
                        // is unavailable — it looks like it worked, and the `Authorization:
                        // Bearer ` it produces fails silently in the client days later.
                        .child(
                            ToolButton::new(IconName::Eye, reveal_label)
                                .outlined()
                                .enabled(minted)
                                .on_press(EventHandler::new(move |_: Event<PressEventData>| {
                                    let shown = *revealed.peek();
                                    revealed.set(!shown);
                                })),
                        )
                        .child(
                            ToolButton::new(IconName::Copy, "Copy the token")
                                .outlined()
                                .enabled(minted)
                                .on_press(EventHandler::new(move |_: Event<PressEventData>| {
                                    copy(&ctx.draft.peek().agent_access.token);
                                })),
                        )
                        .child(
                            Button::new()
                                .outline()
                                .height(Size::px(FIELD_HEIGHT))
                                .on_press(move |_: Event<PressEventData>| {
                                    ctx.edit(|s| s.agent_access.token = mint_token());
                                })
                                .child(
                                    rect()
                                        .horizontal()
                                        .cross_align(Alignment::Center)
                                        .spacing(6.)
                                        .child(Icon::new(IconName::Reload).size(13.))
                                        .child(Control::new("Regenerate")),
                                ),
                        ),
                ),
            );

        Pane::new(body)
    }
}

/// Land `text` on the system clipboard. Fire-and-forget, like every other copy in the app: a
/// failed copy is warned about, never raised as a condition.
fn copy(text: &str) {
    if let Err(err) = Clipboard::set(text.to_string()) {
        tracing::warn!("clipboard write failed: {err:?}");
    }
}
