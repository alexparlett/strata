//! **One send** (AS-04) — how the composer's press becomes a turn, and the one task that folds
//! what comes back into the transcript.
//!
//! The window's half of AS-02: everything about *running* a turn is the assistant's, and
//! everything about what this project's conversation is pointing at is here.
//!
//! ## What the funnel does, in order
//!
//! 1. Resolve the pinned anchors that can be read from a store — a tab's SQL now, a saved
//!    query's text — **on the render thread**, because they are one map lookup each.
//! 2. Record the question in the transcript, so the message is on screen before anything
//!    network-shaped happens.
//! 3. Spawn one task, which resolves the anchors that need a tool round (a table's schema),
//!    reads the API key **off the render thread**, starts the turn, and drains its events into
//!    [`Chats::fold`].
//!
//! ## The key is read on a worker and lives as long as the request
//!
//! `strata_core::secret`'s own rule: config holds a [`SecretRef`], the keystore call blocks, and
//! the value is read per use rather than cached. The pane never holds a key at all — it hands
//! the reference to the task, which resolves it on a thread and passes it straight into the
//! [`Selection`].
//!
//! ## Cancel is dropping the task
//!
//! `Running` carries `tokio_util`'s drop guard, so the future going away *is* the turn's cancel
//! and the in-flight tool's engine abort. [`Chats::stop`] cancels the task and marks the reply
//! stopped, because the turn's own `Settled(Cancelled)` is exactly what was just dropped.
//!
//! ## A second send is refused, not queued
//!
//! A conversation with a turn in flight has a **stop** where its send was, so the only way here
//! is a race between the press and the settle. Refusing is the honest answer: replacing the
//! running task would silently cancel a turn the user never stopped, and queueing would send a
//! question against a conversation the model has not finished writing.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use freya::prelude::{spawn, spawn_forever, TaskHandle};
use freya::radio::Radio;
use strata_agent::assistant::{Ask, Assistant, ContextBlock, Scope, Selection, TurnEvent};
use strata_agent::wire::{DescribeTableParams, SeverityWire, ValidateParams};
use strata_agent::StrataTools;
use strata_core::ai::Ai;
use strata_core::project::clear_chats;
use strata_core::secret::SecretRef;
use strata_model::CatalogKind;
use uuid::Uuid;

use super::chat::{Anchor, Block, ChatId, ChatsCtx, Pick, RowKey, Turn};
use super::chat_store;
use super::log::{log_event, LogLevel};
use super::persist::{persisted, ProjectFile, ReportCtx};
use super::project::ProjectState;
use super::session::{QueryTab, SessionState};
use super::{Chan, ProjChan};
use crate::agent::AgentDirectory;
use crate::task::offload;

/// The window's assistant handles: the runtime it sends on, its vocabulary, and which project
/// its calls land in.
///
/// **One [`StrataTools`] per window, minted `in_app`.** That is what makes every turn in every
/// conversation the same agent holding the same query sessions, and what keeps the assistant out
/// of the Agents pane without anybody comparing an identity (AA-03c).
#[derive(Clone)]
pub struct AssistantCtx {
    /// `None` when the assistant's runtime could not be built — a fault the pane states rather
    /// than one that takes the window down with it.
    pub assistant: Option<Rc<Assistant>>,
    pub tools: StrataTools<AgentDirectory>,
    pub scope: Scope,
}

/// Two of these are always the same handles — the tools are minted once per window mount and the
/// runtime is the app's. Written out because neither field has a `PartialEq` of its own.
impl PartialEq for AssistantCtx {
    fn eq(&self, other: &Self) -> bool {
        match (&self.assistant, &other.assistant) {
            (Some(a), Some(b)) => Rc::ptr_eq(a, b) && self.scope == other.scope,
            (None, None) => self.scope == other.scope,
            _ => false,
        }
    }
}

/// What a new conversation starts on: Settings' defaults, with the provider dropped if it is no
/// longer enabled.
///
/// **Resolved here rather than trusted**, because `Ai::default_provider` can name a provider the
/// user has since turned off — and in Settings a disabled provider also loses its key, so
/// "disabled" and "no longer usable" are one state rather than two the pane has to tell apart.
pub fn seed_pick(ai: &Ai) -> Pick {
    let provider = ai.default_provider.filter(|kind| ai.is_enabled(*kind));
    Pick {
        provider,
        model: match provider {
            Some(_) => ai.default_model.trim().to_string(),
            // A model without the provider that serves it is not a pick.
            None => String::new(),
        },
        effort: provider.and(ai.default_effort),
    }
}

/// Why a conversation cannot send right now — the composer's honest degradation, stated before
/// a press rather than reported after one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Missing {
    /// The assistant's runtime never started.
    Runtime,
    /// Nothing is enabled, or this conversation's provider has since been turned off.
    Provider,
    /// A provider, but no model chosen for it.
    Model,
}

impl Missing {
    /// What the composer says, in the IDE register, naming the page that fixes it.
    pub fn note(&self) -> &'static str {
        match self {
            Missing::Runtime => "The assistant could not start.",
            Missing::Provider => {
                "No AI provider is enabled. Turn one on in Settings > AI > Providers."
            }
            Missing::Model => {
                "No model is chosen. Pick one below, or set a default in Settings > AI > Chat."
            }
        }
    }
}

/// Whether `pick` can send, given what Settings currently holds.
pub fn blocked(ctx: &AssistantCtx, ai: &Ai, pick: &Pick) -> Option<Missing> {
    match ctx.assistant {
        None => Some(Missing::Runtime),
        Some(_) => blocked_pick(ai, pick),
    }
}

/// The half of [`blocked`] that is only about the pick and the config — split out because it is
/// the whole rule for everything except a runtime that never started, and because it can then be
/// tested without a window.
fn blocked_pick(ai: &Ai, pick: &Pick) -> Option<Missing> {
    match pick.provider {
        None => Some(Missing::Provider),
        Some(kind) if !ai.is_enabled(kind) => Some(Missing::Provider),
        Some(_) if pick.model.trim().is_empty() => Some(Missing::Model),
        Some(_) => None,
    }
}

/// Everything one send needs from the window, taken at render time the way every other action
/// funnel here takes its handles.
#[derive(Clone, Copy)]
pub struct Stores {
    pub session: Radio<SessionState, Chan>,
    pub project: Radio<ProjectState, ProjChan>,
}

/// **Send one message.** The composer's press and the ⌘↵ chord both land here.
///
/// Does nothing when the conversation already has a turn in flight, or when the question is
/// empty with nothing pinned — the second because a send with no content spends a request to be
/// told so.
///
/// **Answers whether it sent**, because every refusal here is silent and the caller has to know:
/// the composer clears its field on a send, and clearing it on a refusal destroys a message the
/// user still has to send.
pub fn send(
    ctx: &AssistantCtx,
    mut chats: ChatsCtx,
    stores: Stores,
    report: ReportCtx,
    ai: &Ai,
    id: ChatId,
    question: String,
) -> bool {
    let Some(assistant) = ctx.assistant.clone() else {
        return false;
    };
    let (pick, pinned, memory) = {
        let held = chats.peek();
        let Some(chat) = held.get(id) else {
            return false;
        };
        if chat.is_running() {
            return false;
        }
        (chat.pick.clone(), chat.pinned.clone(), chat.memory.clone())
    };
    let question = question.trim().to_string();
    if question.is_empty() && pinned.is_empty() {
        return false;
    }
    if blocked(ctx, ai, &pick).is_some() {
        return false;
    }

    // The two halves of the selection that come from config: where the provider is, and the
    // reference to the key. The **reference**, never the secret — it is resolved on the worker
    // below, and lives exactly as long as the request.
    let setup = pick.provider.and_then(|kind| ai.setup(kind));
    let base_url = setup
        .map(|setup| setup.base_url.trim().to_string())
        .filter(|url| !url.is_empty());
    let key: Option<SecretRef> = setup.and_then(|setup| setup.key.clone());

    // Anchors a store can answer, read now: the tab's text as it stands at the press, and the
    // saved query's own SQL.
    let (ready, wanted) = split_anchors(&pinned, stores);

    let chips = pinned.iter().map(Anchor::label).collect();
    chats.write().ask(id, question.clone(), chips);

    let tools = ctx.tools.clone();
    let scope = ctx.scope.clone();
    let root = stores.project.read().root.clone();
    let task: TaskHandle = spawn_forever(async move {
        // The anchors that need a tool round. Asked through the assistant's own vocabulary, so
        // a pinned table is described by exactly the tool the model would have called — and the
        // round it saves is one the model would otherwise spend on a fact already on screen.
        let mut context = ready;
        for (name, kind) in wanted {
            let described = tools
                .describe_table(DescribeTableParams {
                    name: name.clone(),
                    // **Scoped, like the model's own calls.** Left unset, `describe_table`
                    // resolves against every registered window, so with two projects open this
                    // pre-round comes back `Ambiguous` and the pin attaches an error naming the
                    // other project instead of the schema.
                    project: scope.project.clone(),
                    ..DescribeTableParams::default()
                })
                .await;
            let body = match described {
                Ok(result) => serde_json::to_string_pretty(&result)
                    .unwrap_or_else(|e| format!("This table could not be described: {e}")),
                // The taxonomy's own words. A pin the catalog can no longer answer for is worth
                // saying out loud: the alternative is a question that reads as if the schema
                // were attached when it was not.
                Err(e) => format!("This table could not be described: {e}"),
            };
            context.push(ContextBlock {
                // Named as what it is: one tool answers for both, and telling the model a view
                // is a table is telling it something untrue about the user's catalog.
                label: format!("{} '{name}'", noun(kind)),
                body,
            });
        }

        // The keystore read, on a thread. A `SecretRef` that resolves to nothing is not an
        // error — it means no key is set, which the provider table then answers for the kind in
        // its own words (its environment variable, or a refusal naming it).
        let api_key = match key {
            Some(reference) => offload(move || reference.get().ok().flatten())
                .await
                .flatten(),
            None => None,
        };

        let mut selection = Selection::new(
            pick.provider.expect("blocked() refused a pick with none"),
            pick.model,
        );
        selection.base_url = base_url;
        selection.api_key = api_key;
        selection.effort = pick.effort;

        let ask = Ask { question, context };
        let mut running = assistant.send(tools, selection, scope, memory, ask);
        let mut settled = false;
        while let Some(event) = running.next().await {
            settled |= matches!(event, TurnEvent::Settled(_));
            chats.write().fold(id, event);
        }
        // **A stream that ends without settling still settles the conversation.** `Settled` is
        // normally the last event, but it does not arrive if the turn's task is dropped by the
        // assistant's own shutdown — and then `running` would stay set forever: the composer
        // would show Stop instead of Send and refuse every later question in this conversation.
        // `Running::settle` is the one that answers for a task that did not report.
        if !settled {
            let outcome = running.settle().await;
            chats.write().settle(id, outcome);
        }
        // **The conversation is stored at the turn boundary** (AS-07), and this is the boundary:
        // AS-02 commits the turn's messages to the model's memory *before* it emits `Settled`,
        // so by the time that fold returned both lists were complete and agreed. A per-delta
        // write would be a file rewrite several times a second for nothing.
        //
        // **Still inside the turn**, which is what makes it safe. This task is root-scoped so a
        // backgrounded conversation keeps streaming, and the only thing that stops a root task
        // writing state its subtree has since dropped is `Chats::stop_all` — which reaches it
        // through `Chat::running`. So the handle is released *after* the write, by `finish`,
        // rather than by the settle: a turn is not over until its record is on disk.
        store(&root, chats, report, id).await;
        chats.write().finish(id);
    });
    chats.write().set_running(id, task);
    true
}

/// Write one conversation to `.strata/chats/`, off the render thread, and clear its dirty mark.
///
/// **Offloaded**, because a transcript with a few tool rounds in it is a JSON document of real
/// size and this runs on the UI executor. The document is rendered from a peek and the write
/// happens on a worker, so the only thing on the render thread is the clone.
pub async fn store(root: &Path, mut chats: ChatsCtx, report: ReportCtx, id: ChatId) {
    let Some(doc) = chats.peek().get(id).and_then(chat_store::document) else {
        return;
    };
    let root = root.to_path_buf();
    // **The write is offloaded and the reporting is not.** `ReportCtx` holds this window's
    // reactive handles, which are not `Send` — so the worker does the file and the outcome is
    // reported back here, where the log and the fault store live. `offload` answers `None` only
    // when the work never ran at all: the worker thread could not start, or it panicked.
    let Some(outcome) = offload(move || chat_store::write(&root, &doc)).await else {
        return;
    };
    // **Cleared only once it has landed.** Clearing before the await loses the conversation
    // outright when the write is cancelled — the teardown pass writes `dirty` ones, and a chat
    // marked clean by a write that never finished is one it then skips. Safe on the far side
    // because the turn holds `Chat::running` until this returns, so `stop_all` can still reach it.
    if persisted(report, ProjectFile::Chats, || outcome) {
        if let Some(chat) = chats.write().get_mut(id) {
            chat.dirty = false;
        }
    }
}

/// Write whatever the last [`Chats::open`] or [`Chats::hydrate`] demoted to the shelf.
///
/// Eviction is the one place a conversation leaves the window without the teardown pass seeing
/// it, so a chat still marked dirty when it is shelved would have its last turn dropped and its
/// row would then point at a file older than the row claims. `Chats` does no IO, so it hands the
/// evicted conversations back and this writes them.
pub fn store_shed(root: &Path, mut chats: ChatsCtx, report: ReportCtx) {
    for chat in chats.write().shed() {
        if !chat.dirty {
            continue;
        }
        let Some(doc) = chat_store::document(&chat) else {
            continue;
        };
        let root = root.to_path_buf();
        spawn(async move {
            let Some(outcome) = offload(move || chat_store::write(&root, &doc)).await else {
                return;
            };
            persisted(report, ProjectFile::Chats, || outcome);
        });
    }
}

/// Forget a conversation, whichever kind of row it was: the window's copy and the stored one.
///
/// One funnel for both, because "delete this conversation" is one gesture and a row that
/// happened to be on the shelf rather than open is not a different intent. The satellite is
/// updated first and the file removed on a worker — the row must not sit there while a disk
/// waits.
pub fn discard(mut chats: ChatsCtx, root: PathBuf, report: ReportCtx, key: RowKey, fresh: Pick) {
    let uuid = match key {
        RowKey::Live(id) => {
            let uuid = chats.peek().get(id).map(|chat| chat.uuid);
            chats.write().delete(id, fresh);
            uuid
        }
        RowKey::Shelved(id) => {
            chats.write().forget(id);
            Some(id)
        }
    };
    store_shed(&root, chats, report);
    let Some(uuid) = uuid else {
        return;
    };
    // **Scope-bound**, like `clear_history`'s own writer: this task writes `report`'s satellites
    // after an await, and a root-scoped one would still be holding them if the subtree went away
    // in between (a re-root, an engine restart). `spawn` is dropped with the scope instead.
    spawn(async move {
        let Some(outcome) = offload(move || chat_store::forget_chat(&root, &uuid)).await else {
            return;
        };
        // **Recorded after it landed**, on the catalog drop's own rule: a line written before the
        // write says a thing happened that the store may then contradict.
        if persisted(report, ProjectFile::Chats, || outcome) {
            log_event(
                report.log,
                LogLevel::Info,
                "Deleted conversation".to_string(),
            );
        }
    });
}

/// Discard **every** conversation this project has stored — the pane's Clear.
///
/// Per project, not app-wide: a conversation belongs to the project it is about, and a control
/// that reached across every project a machine has ever opened would be promising a sweep it
/// cannot honestly perform (a project on a disk that is not mounted is unreachable by
/// construction). Clearing here says exactly what it does.
///
/// The window is reset first and the files removed on a worker, so the pane is empty the moment
/// the user confirms rather than when a disk finishes.
pub fn clear_all(mut chats: ChatsCtx, root: PathBuf, report: ReportCtx, fresh: Pick) {
    chats.write().clear(fresh);
    store_shed(&root, chats, report);
    spawn(async move {
        let Some(outcome) = offload(move || clear_chats(&root)).await else {
            return;
        };
        if persisted(report, ProjectFile::Chats, || outcome) {
            log_event(
                report.log,
                LogLevel::Info,
                "Cleared conversations".to_string(),
            );
        }
    });
}

/// Open a stored conversation: read it, hydrate it, and re-check whatever it offers.
///
/// **Reopening reads a file and runs nothing.** The transcript, the step cards and their facts
/// are all recorded values, so nothing here touches the engine's data path and nothing dials
/// out. The one thing it does ask the catalog is whether each offered statement still plans —
/// see [`recheck_offers`], which is a plan and not a run.
pub fn open_stored(
    ctx: &AssistantCtx,
    mut chats: ChatsCtx,
    root: PathBuf,
    report: ReportCtx,
    id: Uuid,
) {
    let tools = ctx.tools.clone();
    let scope = ctx.scope.clone();
    let shed_root = root.clone();
    // Scope-bound for `discard`'s reason: everything after the read writes this subtree's state.
    spawn(async move {
        let Some(read) = offload(move || chat_store::load(&root, &id)).await else {
            return;
        };
        let read = match read {
            Ok(Some(read)) => read,
            // A row whose file has gone, or that this build cannot use, resolves to nothing —
            // `load` has already said so in the log, and there is nothing the user can do about
            // it from here.
            Ok(None) => return,
            Err(e) => {
                tracing::error!("open conversation: {e}");
                return;
            }
        };
        let (doc, memory) = read.into_parts();
        let opened = chats.write().hydrate(doc, memory);
        store_shed(&shed_root, chats, report);
        recheck_offers(&tools, &scope, chats, opened).await;
    });
}

/// Re-check every statement a restored conversation offers, and retire the ones that no longer
/// hold.
///
/// A card was checked when it was made, and the catalog can have moved since — a table dropped,
/// a view replaced. The check is `validate`: lints and a **dry plan**, the same one `offer_sql`
/// ran to make the card in the first place, so a restored card promises exactly what a live one
/// does. No run, no scan, no snapshot.
///
/// A statement that fails **loses its press silently**. Nothing is said, because nothing went
/// wrong: the user never ran it, and a complaint that their catalog changed is not news about
/// their conversation. The card falls back to the ordinary code block the assistant's
/// explanatory SQL already renders as.
async fn recheck_offers(
    tools: &StrataTools<AgentDirectory>,
    scope: &Scope,
    mut chats: ChatsCtx,
    id: ChatId,
) {
    let offers: Vec<(usize, usize, String)> = {
        let held = chats.peek();
        let Some(chat) = held.get(id) else {
            return;
        };
        chat.turns
            .iter()
            .enumerate()
            .filter_map(|(t, turn)| match turn {
                Turn::Reply(reply) => Some((t, reply)),
                Turn::User { .. } => None,
            })
            .flat_map(|(t, reply)| {
                reply
                    .blocks
                    .iter()
                    .enumerate()
                    .filter_map(move |(b, block)| match block {
                        Block::Offer { sql, stale: false } => Some((t, b, sql.clone())),
                        _ => None,
                    })
            })
            .collect()
    };
    for (turn, block, checked_sql) in offers {
        let checked = tools
            .validate(ValidateParams {
                sql: checked_sql.clone(),
                project: scope.project.clone(),
            })
            .await;
        let holds = match checked {
            Ok(result) => !result
                .diagnostics
                .iter()
                .any(|d| matches!(d.severity, SeverityWire::Error)),
            // The project could not be resolved at all. Not a fault of the statement, so the
            // card is left as it is rather than retired on a question that was never answered.
            Err(_) => true,
        };
        if !holds {
            chats.write().stale_offer(id, turn, block, &checked_sql);
        }
    }
}

/// Split the pinned anchors into the blocks a store can answer now, and the table names that
/// need a `describe_table` round.
fn split_anchors(
    pinned: &[Anchor],
    stores: Stores,
) -> (Vec<ContextBlock>, Vec<(String, CatalogKind)>) {
    let mut ready = Vec::new();
    let mut wanted = Vec::new();
    for anchor in pinned {
        match anchor {
            Anchor::Entry { name, kind } => wanted.push((name.clone(), *kind)),
            Anchor::SavedQuery { id, name } => {
                if let Some(sql) = stores
                    .project
                    .read()
                    .saved_queries
                    .iter()
                    .find(|q| &q.id == id)
                    .map(|q| q.sql.clone())
                {
                    ready.push(ContextBlock {
                        label: format!("Saved query '{name}'"),
                        body: sql,
                    });
                }
            }
            Anchor::Tab { id, name, error } => {
                let Some(sql) = stores.session.read().tabs.get(id).map(QueryTab::text) else {
                    continue;
                };
                // The error is the surface's, captured when it pinned the tab: a run's failure
                // lives in that run's own query entry, which no store here can read.
                let body = match error {
                    Some(why) => format!("{sql}\n\nThis query failed with:\n{why}"),
                    None => sql,
                };
                ready.push(ContextBlock {
                    label: format!("Query tab '{name}'"),
                    body,
                });
            }
            Anchor::Result { name, body } => ready.push(ContextBlock {
                label: name.clone(),
                body: body.clone(),
            }),
        }
    }
    (ready, wanted)
}

/// What a catalog entry is called in the block that reaches the model.
fn noun(kind: CatalogKind) -> &'static str {
    match kind {
        CatalogKind::Table => "Table",
        CatalogKind::View => "View",
        CatalogKind::Query => "Saved query",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::ai::{Effort, ProviderKind, ProviderSetup};

    fn ai(enabled: &[ProviderKind]) -> Ai {
        Ai {
            providers: enabled
                .iter()
                .map(|kind| {
                    (
                        *kind,
                        ProviderSetup {
                            enabled: true,
                            ..ProviderSetup::default()
                        },
                    )
                })
                .collect(),
            default_provider: enabled.first().copied(),
            default_model: "claude-sonnet-4-5".into(),
            default_effort: Some(Effort::Medium),
            ..Ai::default()
        }
    }

    /// A new chat starts on the Settings defaults.
    #[test]
    fn a_new_chat_seeds_from_the_defaults() {
        let pick = seed_pick(&ai(&[ProviderKind::Anthropic]));
        assert_eq!(pick.provider, Some(ProviderKind::Anthropic));
        assert_eq!(pick.model, "claude-sonnet-4-5");
        assert_eq!(pick.effort, Some(Effort::Medium));
    }

    /// **A default naming a provider that is off is not a pick.** In Settings a disabled
    /// provider also loses its key, so carrying the name forward would seed every new chat with
    /// a selection whose first send is refused — and the model and rung would be answers about a
    /// provider nobody chose.
    #[test]
    fn a_disabled_default_seeds_nothing() {
        let mut ai = ai(&[ProviderKind::Anthropic]);
        ai.providers.clear();

        let pick = seed_pick(&ai);
        assert_eq!(pick.provider, None);
        assert!(pick.model.is_empty());
        assert_eq!(pick.effort, None);
    }

    /// The composer names what is missing, in order, rather than reporting one blanket
    /// "not configured" — the three states have three different fixes.
    #[test]
    fn blocked_names_the_missing_half() {
        let ai = ai(&[ProviderKind::Anthropic]);
        let with = |provider, model: &str| Pick {
            provider,
            model: model.into(),
            effort: None,
        };

        assert_eq!(
            blocked_pick(&ai, &with(None, "")),
            Some(Missing::Provider),
            "nothing enabled"
        );
        assert_eq!(
            blocked_pick(&ai, &with(Some(ProviderKind::Groq), "x")),
            Some(Missing::Provider),
            "the pick's provider was turned off"
        );
        assert_eq!(
            blocked_pick(&ai, &with(Some(ProviderKind::Anthropic), "  ")),
            Some(Missing::Model)
        );
        assert_eq!(
            blocked_pick(&ai, &with(Some(ProviderKind::Anthropic), "claude-opus-5")),
            None
        );
    }
}
