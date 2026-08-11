//! The window's **chat** satellite (AS-04) — the conversations the assistant is having in this
//! project, and the record behind the right pane's chat.
//!
//! A context signal rather than a store, on the same terms as [`Log`](super::log::Log),
//! [`History`](super::history::History) and [`Agents`](super::agents::Agents): one append wakes
//! exactly one reader (the pane, when it is mounted), so nothing here needs surgical per-channel
//! updates.
//!
//! ## Why it is not `SessionState`
//!
//! [`Agents`](super::agents::Agents)' reasoning, applied to the other agent. **Nothing here
//! reaches `session.json`** — a transcript is not part of the arrangement of the window, and a
//! reopened project restoring half a conversation whose model memory went with the process would
//! be restoring the *appearance* of one. Persistence is AS-07's, and it is a real feature rather
//! than a serde derive: what has to survive is the `Conversation` the model reads back, not the
//! blocks the pane paints. This module is shaped so that lands as a writer hooked to
//! [`Chats::settle`] rather than as a retrofit.
//!
//! **Nothing here reaches `.strata/history.jsonl` either.** The assistant's own runs are not the
//! user's; a run enters history the ordinary way, when the user promotes a card into a tab and
//! presses Run. That is the *adoption* rule, and it is why an [`Offer`](Block::Offer)'s Run press
//! goes through the editor's own funnel rather than dispatching anything here.
//!
//! ## Recorded by its observer
//!
//! No producer hook, for the log's reason: a turn's events describe things already finished and
//! cannot be re-derived. [`send`] spawns the one task that watches a turn and folds every
//! [`TurnEvent`] into the conversation it belongs to, and that task is the only writer of a
//! [`Reply`].
//!
//! ## What a turn's cancel is
//!
//! Dropping the task. `Running` holds `tokio_util`'s own drop guard, so the future going away
//! *is* the turn's cancel and the engine's abort — there is no second stop path to keep in step
//! with this one. What the fold still owes the transcript is the truth: a cancelled turn stays,
//! marked as stopped, because a conversation that erases what it was doing when you stopped it
//! is a conversation you cannot audit.
//!
//! ## Everything here is bounded
//!
//! A conversation has no natural end and neither does a list of them, so both are capped, oldest
//! first — a transcript is a scrollback, the way the event log is.

use std::sync::{Arc, Mutex};

use freya::prelude::{use_drop, use_provide_context, State, TaskHandle};
use serde_json::Value;
use strata_agent::assistant::{Conversation, Facts, Settle, TurnEvent};
use strata_core::ai::{Effort, ProviderKind};
use strata_core::engine::CANCELLED;
use strata_core::util::now_hms;
use strata_model::{CatalogKind, TabId};
use uuid::Uuid;

/// How many conversations one window keeps. Past it the oldest is dropped — which is why the
/// switcher lists them newest-last and a delete is an explicit press: nothing here should be
/// able to lose a conversation the user is looking at.
const MAX_CHATS: usize = 20;

/// How many turns one conversation keeps. The model's own memory is bounded by the provider's
/// context window and by AS-02's result caps; this is the *pane's* bound, so a conversation left
/// running all day cannot grow the window without limit.
const MAX_TURNS: usize = 200;

/// A conversation's identity — minted per window, which is all a key needs to be.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct ChatId(pub u64);

/// **What a conversation is talking to** — the composer footer's pick, per conversation.
///
/// Runtime state on this satellite and never config: changing the model mid-conversation is a
/// decision about *this* conversation, and writing it back to `Ai::default_model` would move
/// what every new chat starts on. Seeded from those defaults when a chat opens, read at send
/// time into AS-02's per-send `Selection`.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Pick {
    /// `None` is a real state the pane renders honestly: nothing is enabled yet, or the pick's
    /// provider has since been turned off in Settings.
    pub provider: Option<ProviderKind>,
    /// As the provider spells it. Empty is "nothing chosen", which the send refuses by name.
    pub model: String,
    /// `None` sends no preference and takes the model's own default — and is the only valid
    /// value for a model with no rungs.
    pub effort: Option<Effort>,
}

/// One thing the user pinned to their next question — an `@`-mention, or one of the three
/// friction entries that open the pane with context already attached.
///
/// **Two shapes, and the split is which facts can go stale.** An entry resolves at *send* time,
/// because a schema is large and the catalog moves under a pane left open; a tab or a result
/// carries what the surface that pinned it already had on screen, because nothing else can see
/// it — a run's error lives in that run's own query entry, not in any store this could read.
#[derive(Clone, PartialEq, Debug)]
pub enum Anchor {
    /// A catalog table or view — its `describe_table` result, fetched when the send goes.
    ///
    /// The **kind** rides along because the block that reaches the model names it, and calling a
    /// view a table is telling the model something untrue about the user's catalog. One tool
    /// answers for both, so this is the only place the difference survives.
    Entry { name: String, kind: CatalogKind },
    /// A saved query — its SQL, read from the catalog store when the send goes.
    SavedQuery { id: Uuid, name: String },
    /// A query tab — its SQL when the send goes, plus the error it had failed with when it was
    /// pinned (the failed-run entry's, and `None` for a tab pinned any other way).
    Tab {
        id: TabId,
        name: String,
        error: Option<String>,
    },
    /// A settled run — schema, row count and the page the results pane had in hand.
    Result { name: String, body: String },
}

impl Anchor {
    /// The chip's text, in the composer and in the sent message.
    pub fn label(&self) -> String {
        match self {
            Anchor::Entry { name, .. } => format!("@{name}"),
            Anchor::SavedQuery { name, .. } => format!("@{name}"),
            Anchor::Tab { name, .. } => format!("@{name}"),
            Anchor::Result { name, .. } => format!("@{name}"),
        }
    }
}

/// One tool round, as the transcript shows it — a **citation** for whatever the prose beside it
/// claims.
///
/// Every figure here is the engine's own (`elapsed_ms`, the exact row total, the stop's wording):
/// AS-02's prompt says no number in prose without a run behind it, and this card is what makes
/// that auditable. Nothing on it is re-derived or re-measured.
#[derive(Clone, PartialEq, Debug)]
pub struct Step {
    /// The provider's id for the call, so its settle finds it.
    pub call: String,
    pub tool: String,
    /// The arguments the call **ran with**, which AS-02 has already scoped — quoting the model's
    /// own request could name a project the run never touched.
    pub arguments: Value,
    /// `None` until the tool answers.
    pub failed: Option<bool>,
    pub facts: Facts,
}

/// One piece of an assistant turn, in the order it arrived.
///
/// A flat list rather than "prose plus a tool trail": the model speaks, calls a tool, speaks
/// again, and a transcript that hoisted every card to the bottom would put its reasoning out of
/// order with its evidence.
#[derive(Clone, PartialEq, Debug)]
pub enum Block {
    /// Markdown, deltas appended as they stream. SQL the assistant is only *explaining* lives
    /// here, as an ordinary code block with no Run press.
    Prose(String),
    /// A tool round (above). **Boxed**: a step carries the call's JSON arguments, its facts and
    /// three strings, and a turn is mostly prose — sizing every block by the largest arm would
    /// make a paragraph cost what a tool call does.
    Step(Box<Step>),
    /// **One executable statement**, handed over through `offer_sql` and already checked against
    /// the catalog and the editor's policy. Deliberately produces no [`Step`] beside it: an
    /// offer is not a step, and a card describing the call would be the same thing said twice.
    Offer(String),
}

/// One exchange in a conversation.
#[derive(Clone, PartialEq, Debug)]
pub enum Turn {
    User {
        text: String,
        /// The chips as they were sent, so the transcript records what the user was pointing at
        /// *when they asked*.
        chips: Vec<String>,
        /// The local wall clock this was sent at, `HH:MM:SS` — formatted **once, at the send**,
        /// because that is when the clock said what it said. The event log's own rule.
        at: String,
    },
    Reply(Reply),
}

impl Turn {
    /// When this turn happened, as the transcript shows it.
    pub fn at(&self) -> &str {
        match self {
            Turn::User { at, .. } => at,
            Turn::Reply(reply) => &reply.at,
        }
    }
}

/// The assistant's half of one exchange.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Reply {
    pub blocks: Vec<Block>,
    /// When the turn was started — see [`Turn::User::at`]. The *start*, not the settle, so the
    /// two halves of one exchange are stamped with the same moment the user pressed send.
    pub at: String,
    /// The sentence for a turn that did not answer — [`Settle::note`]'s, verbatim, so a stop and
    /// a failure are never described in this module's own words.
    pub note: Option<String>,
    /// Whether the turn is over. While `false` the composer's send is a stop.
    pub settled: bool,
}

impl Reply {
    /// Append a delta to the trailing prose block, starting one if the last thing that happened
    /// was a tool round.
    fn say(&mut self, delta: &str) {
        match self.blocks.last_mut() {
            Some(Block::Prose(text)) => text.push_str(delta),
            _ => self.blocks.push(Block::Prose(delta.to_string())),
        }
    }

    /// Fill in the step this settle belongs to, matched on the provider's own call id.
    fn settle_step(&mut self, call: &str, failed: bool, facts: Facts) {
        for block in self.blocks.iter_mut().rev() {
            if let Block::Step(step) = block {
                if step.call == call {
                    step.failed = Some(failed);
                    step.facts = facts;
                    return;
                }
            }
        }
    }
}

/// One conversation.
pub struct Chat {
    pub id: ChatId,
    /// What the switcher calls it: "New chat" until the first send, then that question, folded.
    pub title: String,
    pub pick: Pick,
    pub turns: Vec<Turn>,
    /// What the composer is holding but has not sent.
    pub pinned: Vec<Anchor>,
    /// The model's own memory — AS-02 reads it once per turn and commits to it once, at the
    /// settle. Shared with the turn task, which is why it is behind a lock rather than in the
    /// blocks above: the pane renders from the events, the provider reads from this.
    pub memory: Arc<Mutex<Conversation>>,
    /// The turn in flight.
    ///
    /// **Cancelling it is the cancel, and dropping it is nothing at all.** `TaskHandle` is `Copy`
    /// with no `Drop` — the fork says so in as many words — so letting a `Chat` go does *not*
    /// stop its turn: the future stays in the runtime holding AS-02's `Running`, and only when
    /// *that* is dropped does its guard abort the engine. Every path that lets a conversation go
    /// therefore has to call [`Chats::stop`] first, which is why `delete`, the over-cap eviction
    /// and the subtree's own teardown all route through it.
    pub running: Option<TaskHandle>,
}

impl Chat {
    fn new(id: ChatId, pick: Pick) -> Chat {
        Chat {
            id,
            title: "New chat".into(),
            pick,
            turns: Vec::new(),
            pinned: Vec::new(),
            memory: Arc::new(Mutex::new(Conversation::new())),
            running: None,
        }
    }

    /// Whether a turn is streaming — what flips the composer's send into a stop.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// How many exchanges the switcher reports.
    pub fn message_count(&self) -> usize {
        self.turns.len()
    }

    fn reply_mut(&mut self) -> Option<&mut Reply> {
        match self.turns.last_mut() {
            Some(Turn::Reply(reply)) => Some(reply),
            _ => None,
        }
    }
}

/// Every conversation in this window, and which one is open.
pub struct Chats {
    chats: Vec<Chat>,
    active: ChatId,
    next: u64,
}

impl Chats {
    /// A window's chats always contain at least one: a pane with no conversation is a dead end,
    /// so opening the pane never has to decide whether to make one.
    pub fn new(pick: Pick) -> Chats {
        Chats {
            chats: vec![Chat::new(ChatId(0), pick)],
            active: ChatId(0),
            next: 1,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Chat> {
        self.chats.iter()
    }

    pub fn active_id(&self) -> ChatId {
        self.active
    }

    /// The open conversation. Always present: [`new`](Chats::new) starts with one and
    /// [`delete`](Chats::delete) never empties the list.
    pub fn active(&self) -> &Chat {
        self.get(self.active)
            .expect("the active chat is always in the list")
    }

    pub fn get(&self, id: ChatId) -> Option<&Chat> {
        self.chats.iter().find(|chat| chat.id == id)
    }

    pub fn get_mut(&mut self, id: ChatId) -> Option<&mut Chat> {
        self.chats.iter_mut().find(|chat| chat.id == id)
    }

    /// Open a new conversation on `pick` (the Settings defaults, resolved by the caller) and
    /// make it active.
    pub fn open(&mut self, pick: Pick) -> ChatId {
        let id = ChatId(self.next);
        self.next += 1;
        self.chats.push(Chat::new(id, pick));
        if self.chats.len() > MAX_CHATS {
            // The evicted conversation goes the same way a deleted one does, for the same reason.
            let evicted = self.chats[0].id;
            self.stop(evicted);
            self.chats.remove(0);
        }
        self.active = id;
        id
    }

    pub fn show(&mut self, id: ChatId) {
        if self.get(id).is_some() {
            self.active = id;
        }
    }

    /// Delete a conversation. **Deleting the last one opens a fresh one** rather than leaving
    /// the pane with nothing to show — the canvas's rule, and the reason `active` can be a plain
    /// id rather than an `Option`.
    pub fn delete(&mut self, id: ChatId, fresh: Pick) {
        // **Stop before dropping.** Dropping the `Chat` drops a `Copy` id and nothing else, so a
        // conversation deleted mid-turn would keep streaming — spending the user's tokens and
        // holding this project's engine — with the only handle that could stop it now gone.
        self.stop(id);
        self.chats.retain(|chat| chat.id != id);
        if self.chats.is_empty() {
            self.open(fresh);
            return;
        }
        if self.active == id {
            self.active = self.chats.last().expect("non-empty").id;
        }
    }

    /// Record the user's message and open the reply the fold will stream into.
    ///
    /// The conversation takes its **title** from the first thing asked in it, because that is
    /// what a switcher row has to say and nothing else in the transcript is shorter.
    pub fn ask(&mut self, id: ChatId, text: String, chips: Vec<String>) {
        let Some(chat) = self.get_mut(id) else {
            return;
        };
        if chat.turns.is_empty() {
            chat.title = strata_core::util::collapse_sql(&text);
        }
        let at = now_hms();
        chat.turns.push(Turn::User {
            text,
            chips,
            at: at.clone(),
        });
        chat.turns.push(Turn::Reply(Reply {
            at,
            ..Reply::default()
        }));
        chat.pinned.clear();
        while chat.turns.len() > MAX_TURNS {
            chat.turns.remove(0);
        }
    }

    /// Fold one event of `id`'s in-flight turn into its reply — **the** writer of a [`Reply`].
    pub fn fold(&mut self, id: ChatId, event: TurnEvent) {
        let Some(chat) = self.get_mut(id) else {
            return;
        };
        let Some(reply) = chat.reply_mut() else {
            return;
        };
        match event {
            // The send is on its way. Nothing to record: the empty reply is already on screen,
            // which is what says the turn started.
            TurnEvent::Started => {}
            TurnEvent::Delta(text) => reply.say(&text),
            TurnEvent::Runnable(sql) => reply.blocks.push(Block::Offer(sql)),
            TurnEvent::ToolCall {
                call,
                tool,
                arguments,
            } => reply.blocks.push(Block::Step(Box::new(Step {
                call,
                tool,
                arguments,
                failed: None,
                facts: Facts::default(),
            }))),
            TurnEvent::ToolSettled {
                call,
                failed,
                facts,
                ..
            } => reply.settle_step(&call, failed, facts),
            TurnEvent::Settled(settle) => self.settle(id, settle),
        }
    }

    /// End `id`'s turn: the settle's own sentence, and the running handle dropped.
    ///
    /// **AS-07 hooks here.** This is the one place a conversation is known to be complete, which
    /// is what a persistence writer needs and what a per-event writer could not tell.
    pub fn settle(&mut self, id: ChatId, settle: Settle) {
        let Some(chat) = self.get_mut(id) else {
            return;
        };
        chat.running = None;
        if let Some(reply) = chat.reply_mut() {
            reply.note = settle.note();
            reply.settled = true;
            // **A step whose answer will never come is closed here.** Cancelling the task is
            // exactly what guarantees the matching `ToolSettled` never arrives, so a card opened
            // by `ToolCall` would sit at `failed: None` — which the transcript draws as
            // "running…" — under a reply headed "Stopped." for the life of the window. The
            // wording is the engine's own, the same one a stopped run carries everywhere else.
            for block in &mut reply.blocks {
                if let Block::Step(step) = block {
                    if step.failed.is_none() {
                        step.failed = Some(false);
                        step.facts.stopped = Some(CANCELLED.to_string());
                    }
                }
            }
        }
    }

    /// Hold the task driving `id`'s turn. Dropping the previous one — there should not be one —
    /// would cancel it, which is why [`send`](super::chat_send::send) refuses a second send
    /// rather than replacing this.
    pub fn set_running(&mut self, id: ChatId, task: TaskHandle) {
        if let Some(chat) = self.get_mut(id) {
            chat.running = Some(task);
        }
    }

    /// Stop `id`'s turn. Cancelling the task drops AS-02's `Running`, whose guard is the cancel;
    /// the reply keeps everything that had already streamed, marked stopped — the turn task's
    /// own `Settled(Cancelled)` never arrives, because it is what we just dropped.
    pub fn stop(&mut self, id: ChatId) {
        let Some(chat) = self.get_mut(id) else {
            return;
        };
        let Some(task) = chat.running.take() else {
            return;
        };
        task.cancel();
        self.settle(id, Settle::Cancelled);
    }

    /// Stop every turn in flight — what the project subtree calls on its way out.
    ///
    /// A turn's task is spawned on the **app root** (it has to outlive the pane, so a
    /// backgrounded conversation keeps streaming), but the state it writes belongs to this
    /// subtree. Without this, a re-root or an engine restart mid-turn leaves a root-scoped task
    /// writing a `State` whose owner has been dropped, which panics rather than fizzling.
    pub fn stop_all(&mut self) {
        let running: Vec<ChatId> = self
            .chats
            .iter()
            .filter(|chat| chat.is_running())
            .map(|chat| chat.id)
            .collect();
        for id in running {
            self.stop(id);
        }
    }

    /// Pin an anchor to the open conversation's next send, unless it is already pinned.
    pub fn pin(&mut self, id: ChatId, anchor: Anchor) {
        if let Some(chat) = self.get_mut(id) {
            if !chat.pinned.contains(&anchor) {
                chat.pinned.push(anchor);
            }
        }
    }

    pub fn unpin(&mut self, id: ChatId, at: usize) {
        if let Some(chat) = self.get_mut(id) {
            if at < chat.pinned.len() {
                chat.pinned.remove(at);
            }
        }
    }
}

/// The window's chats, provided into its subtree.
pub type ChatsCtx = State<Chats>;

/// Stand this project's chat satellite up and provide it. Call once in the window root.
///
/// The seed pick is the caller's, because it is a read of the **config** store — which this
/// module deliberately knows nothing about, so that changing what a new chat starts on stays one
/// question answered in one place.
pub fn use_init_chats(seed: Pick) -> ChatsCtx {
    let chats = use_provide_context(|| State::create(Chats::new(seed)));
    // **Every turn is stopped when this subtree goes.** See [`Chats::stop_all`]: the tasks are
    // root-scoped and would outlive the state they write.
    use_drop(move || {
        let mut chats = chats;
        chats.write().stop_all();
    });
    chats
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick() -> Pick {
        Pick {
            provider: Some(ProviderKind::Anthropic),
            model: "claude-sonnet-4-5".into(),
            effort: None,
        }
    }

    fn tool_call(call: &str) -> TurnEvent {
        TurnEvent::ToolCall {
            call: call.into(),
            tool: "run".into(),
            arguments: Value::Null,
        }
    }

    /// Prose either side of a tool round stays either side of it: a transcript that hoisted the
    /// cards would put the assistant's reasoning out of order with its evidence.
    #[test]
    fn blocks_keep_the_order_they_arrived_in() {
        let mut chats = Chats::new(pick());
        let id = chats.active_id();
        chats.ask(id, "why?".into(), vec![]);

        chats.fold(id, TurnEvent::Delta("Check".into()));
        chats.fold(id, TurnEvent::Delta("ing.".into()));
        chats.fold(id, tool_call("c1"));
        chats.fold(id, TurnEvent::Delta("Because.".into()));

        let Some(Turn::Reply(reply)) = chats.active().turns.last() else {
            panic!("a reply is open");
        };
        assert!(matches!(&reply.blocks[0], Block::Prose(t) if t == "Checking."));
        assert!(matches!(&reply.blocks[1], Block::Step(s) if s.call == "c1"));
        assert!(matches!(&reply.blocks[2], Block::Prose(t) if t == "Because."));
    }

    /// A settle lands on **its own** call, not on the newest step — two tools in flight is the
    /// ordinary case and a card showing another call's figures would be a false citation.
    #[test]
    fn a_settle_finds_the_call_it_belongs_to() {
        let mut chats = Chats::new(pick());
        let id = chats.active_id();
        chats.ask(id, "?".into(), vec![]);
        chats.fold(id, tool_call("c1"));
        chats.fold(id, tool_call("c2"));

        chats.fold(
            id,
            TurnEvent::ToolSettled {
                call: "c1".into(),
                tool: "run".into(),
                failed: false,
                facts: Facts {
                    rows: Some(7),
                    ..Facts::default()
                },
            },
        );

        let Some(Turn::Reply(reply)) = chats.active().turns.last() else {
            panic!("a reply is open");
        };
        let steps: Vec<&Step> = reply
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Step(s) => Some(s.as_ref()),
                _ => None,
            })
            .collect();
        assert_eq!(steps[0].facts.rows, Some(7));
        assert_eq!(steps[1].failed, None, "c2 is still running");
    }

    /// **A stopped turn keeps what it said.** Erasing it would leave a conversation you cannot
    /// audit, and the note is the settle's own word rather than this module's.
    #[test]
    fn stopping_keeps_the_turn_and_marks_it() {
        let mut chats = Chats::new(pick());
        let id = chats.active_id();
        chats.ask(id, "count the rows".into(), vec![]);
        chats.fold(id, TurnEvent::Delta("Working".into()));

        chats.settle(id, Settle::Cancelled);

        let Some(Turn::Reply(reply)) = chats.active().turns.last() else {
            panic!("a reply is open");
        };
        assert!(matches!(&reply.blocks[0], Block::Prose(t) if t == "Working"));
        assert_eq!(reply.note.as_deref(), Settle::Cancelled.note().as_deref());
        assert!(reply.settled);
        assert!(!chats.active().is_running());
    }

    /// Deleting the last conversation opens a fresh one: a pane with nothing to show is a dead
    /// end, and it is what lets `active` be an id rather than an `Option`.
    #[test]
    fn deleting_the_last_chat_opens_another() {
        let mut chats = Chats::new(pick());
        let first = chats.active_id();

        chats.delete(first, pick());

        assert_eq!(chats.iter().count(), 1);
        assert_ne!(chats.active_id(), first);
        assert!(chats.active().turns.is_empty());
    }

    /// Deleting a background chat leaves the open one open; deleting the open one falls back to
    /// the newest remaining rather than to nothing.
    #[test]
    fn deleting_moves_the_selection_only_when_it_has_to() {
        let mut chats = Chats::new(pick());
        let first = chats.active_id();
        let second = chats.open(pick());
        chats.show(first);

        chats.delete(second, pick());
        assert_eq!(
            chats.active_id(),
            first,
            "deleting another leaves mine open"
        );

        let third = chats.open(pick());
        chats.delete(third, pick());
        assert_eq!(chats.active_id(), first);
    }

    /// A conversation is named by the first thing asked in it, and keeps that name after.
    #[test]
    fn the_first_question_names_the_chat() {
        let mut chats = Chats::new(pick());
        let id = chats.active_id();
        assert_eq!(chats.active().title, "New chat");

        chats.ask(id, "Which countries drive revenue?".into(), vec![]);
        assert_eq!(chats.active().title, "Which countries drive revenue?");

        chats.settle(id, Settle::Answered);
        chats.ask(id, "and by month?".into(), vec![]);
        assert_eq!(chats.active().title, "Which countries drive revenue?");
    }

    /// A send clears the chips it sent, and the transcript keeps them on the user's own turn —
    /// what they were pointing at when they asked.
    #[test]
    fn sending_moves_the_chips_onto_the_message() {
        let mut chats = Chats::new(pick());
        let id = chats.active_id();
        chats.pin(
            id,
            Anchor::Entry {
                name: "events".into(),
                kind: CatalogKind::Table,
            },
        );
        chats.pin(
            id,
            Anchor::Entry {
                name: "events".into(),
                kind: CatalogKind::Table,
            },
        );
        assert_eq!(chats.active().pinned.len(), 1, "pinning twice pins once");

        let chips = chats
            .active()
            .pinned
            .iter()
            .map(Anchor::label)
            .collect::<Vec<_>>();
        chats.ask(id, "what is in it?".into(), chips);

        assert!(chats.active().pinned.is_empty());
        let Some(Turn::User { chips, .. }) = chats.active().turns.first() else {
            panic!("the question is first");
        };
        assert_eq!(chips, &["@events".to_string()]);
    }
}
