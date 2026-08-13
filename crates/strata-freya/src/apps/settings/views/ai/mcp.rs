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
use crate::components::metrics::SP_3;
use crate::components::tool_button::ToolButton;
use crate::components::typography::Control;

/// The canvas's numeric field (`width: 130px`) — the same box the system and data-display
/// panes' are.
const PORT_WIDTH: f32 = 130.;

/// The gap between the token box and the actions beside it, and between those actions.
const ACTION_GAP: f32 = SP_3;

#[derive(PartialEq)]
pub struct McpPane;

impl Component for McpPane {
    fn render(&self) -> impl IntoElement {
        let ctx = use_consume::<SettingsCtx>();
        let mut revealed = use_state(|| false);

        let (enabled, port, minted) = {
            let draft = ctx.draft.read();
            (
                draft.agent_access.enabled,
                draft.agent_access.port,
                !draft.agent_access.token.is_empty(),
            )
        };

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
                                .placeholder("Not minted yet")
                                .enabled(false),
                        )
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
                                        .spacing(SP_3)
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
