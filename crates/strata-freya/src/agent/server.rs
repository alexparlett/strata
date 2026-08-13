//! **Server lifecycle**: what is listening right now, kept in step with the `agent_access`
//! setting.
//!
//! Off by default (the spec's "The in-app server") — AA-03 ships the capability dark and AA-04 builds the control, so
//! until then the way to turn it on is to edit `agent_access.enabled` in the app config.
//!
//! ## Why every workspace window reconciles the same slot
//!
//! The server is app-wide and the setting is app-global, but `main` has no scope to hook and
//! nothing else in this app runs once, reactively, above the windows. So the derivation is the
//! theme's shape rather than a new one: every window computes the *same* pure function of the
//! *same* global and reconciles the *same* slot, and being idempotent that is a no-op for all
//! but the first. It is mounted by the two **workspace** windows (project and launcher) because
//! there is always at least one of them alive — the launcher takes the last project's place —
//! and never by Settings, Export or Configure, each of which is a panel over one of those and
//! could not be the only window open.
//!
//! ## The token is minted here, once, and persisted
//!
//! A token that regenerated per launch would invalidate the `claude mcp add` line the user
//! pasted last time, so an empty one is minted into the config through the ordinary write path
//! and the resulting `Settings` notification brings this effect straight back round to start
//! the server with it. That is also why `AgentServer::start` refuses an empty token rather than
//! tolerating it: its guard is a byte compare, and an empty secret matches a bare
//! `Authorization: Bearer `.

use std::sync::Arc;

use freya::prelude::{use_side_effect, State, WritableUtils};
use strata_agent::{mint_token, AgentServer};
use strata_core::config::AgentAccess;

use crate::state::{use_config_channel, write_config, ConfigChan, ConfigStation};

use super::directory::AgentDirectory;
use super::AgentCtx;

/// The listening server, and the settings it was started with.
///
/// The settings ride along so "is the live server still the one the config asks for?" is one
/// comparison — a port or a regenerated token has to restart it, and `enabled` alone would not
/// say so.
pub struct Running {
    settings: AgentAccess,
    /// The listening server, never read — held only to be dropped. `AgentServer`'s `Drop` stops
    /// the listener, terminates every live MCP session and shuts its runtime down, so clearing
    /// the slot is the whole of "stop" — there is no call to forget.
    #[allow(dead_code, reason = "held for its Drop; see the field note")]
    server: AgentServer,
}

/// Keep the agent-access server in step with the setting for this window's lifetime. Call once
/// in a workspace window's root (see the module note on why more than one caller is correct).
pub fn use_agent_server(agent: AgentCtx, config: ConfigStation) {
    let settings = use_config_channel(config, ConfigChan::Settings);
    use_side_effect(move || {
        let (want, page_size) = {
            let cfg = settings.read();
            (cfg.settings.agent_access.clone(), cfg.settings.row_limit)
        };
        agent.directory.set_default_page_size(page_size);
        reconcile(agent.server, config, &agent.directory, want);
    });
}

/// Start, stop or restart the server so it matches `want`.
///
/// Every `peek` is read into a value before the `set` that follows it: a temporary in an `if`
/// condition holds its read borrow across the whole statement, which on the same
/// `GenerationalBox` is a runtime borrow panic.
fn reconcile(
    mut slot: State<Option<Running>>,
    config: ConfigStation,
    directory: &Arc<AgentDirectory>,
    want: AgentAccess,
) {
    if !want.enabled {
        let listening = slot.peek().is_some();
        if listening {
            slot.set(None);
            tracing::info!("agent access stopped");
        }
        return;
    }
    let unchanged = slot.peek().as_ref().is_some_and(|r| r.settings == want);
    if unchanged {
        return;
    }
    if want.token.is_empty() {
        write_config(config, &[ConfigChan::Settings], |cfg| {
            cfg.settings.agent_access.token = mint_token();
        });
        return;
    }
    slot.set(None);
    match AgentServer::start(want.port, want.token.clone(), Arc::clone(directory)) {
        Ok(server) => slot.set(Some(Running {
            settings: want,
            server,
        })),
        Err(e) => tracing::error!("{e}"),
    }
}
