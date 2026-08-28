//! The window's **chat** satellite (AS-04) — the conversations the assistant is having in this
//! project, and the record behind the right pane's chat.
//!
//! A context signal rather than a store, on the same terms as [`Log`](super::log::Log),
//! [`History`](super::history::History) and [`Agents`](super::agents::Agents): one append wakes
//! exactly one reader (the pane, when it is mounted), so nothing here needs surgical per-channel
//! updates.
//!
//! **Nothing here reaches `session.json`** — a transcript is not part of the arrangement of the
//! window. It does reach `.strata/chats/` through [`chat_store`](super::chat_store), which owns the
//! document, and what is stored is **both** lists: the turns below *and* the [`Conversation`] the
//! model reads back, because a restored conversation has to be continuable and neither list
//! reconstructs the other. Restoring only the transcript restores the *appearance* of a
//! conversation. The writes hang off the three places a conversation is known to have stopped
//! changing: [`Chats::settle`], the stop press, and this subtree's teardown.
//!
//! **Nothing here reaches `.strata/history.jsonl` either.** The assistant's runs are not the
//! user's; a run enters history the ordinary way, when the user promotes a card into a tab and
//! presses Run — which is why an [`Offer`](Block::Offer)'s Run press goes through the editor's own
//! funnel.
//!
//! **Recorded by its observer**, for the log's reason: a turn's events describe things already
//! finished and cannot be re-derived. [`send`] spawns the one task that folds every [`TurnEvent`]
//! into its conversation, and that task is the only writer of a [`Reply`].
//!
//! **A turn's cancel is dropping the task.** `Running` holds `tokio_util`'s own drop guard, so the
//! future going away *is* the cancel and the engine's abort. What the fold still owes the
//! transcript is the truth: a cancelled turn stays, marked stopped, because a conversation that
//! erases what it was doing when you stopped it cannot be audited.
//!
//! **Everything here is bounded.** A conversation has no natural end and neither does a list of
//! them, so both are capped oldest-first — a transcript is a scrollback, like the event log.

use std::cmp::Reverse;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use freya::prelude::{use_drop, use_provide_context, State, TaskHandle};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use strata_agent::assistant::{Conversation, Facts, Settle, TurnEvent};
use strata_core::ai::{Effort, ProviderKind, CHATS_MIN};
use strata_core::util::{now_hms, now_ms};
use strata_engine::StopReason;
use strata_model::{CatalogKind, TabId};
use uuid::Uuid;

use crate::state::ConfigStation;

use super::chat_store::{self, head_of, ChatDoc, ChatHead};
use super::persist::{persisted, ProjectFile, ReportCtx};

/// How many conversations one window holds **open** at once. Past it the oldest is demoted to
/// the shelf, not dropped: its document is already stored, so the switcher still lists it and a
/// press brings it back. What bounds the *stored* set is the user's `max_chats` setting, which is
/// a different question — this one is only about how much transcript a window keeps in memory.
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
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
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
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
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
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
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
    Offer {
        sql: String,
        /// Whether the catalog has moved out from under it since (AS-07).
        ///
        /// A statement is checked when the card is made, and a conversation read back from disk
        /// may be older than the table it names. So a restored offer is re-checked once, and one
        /// that no longer holds **loses its press and says nothing**: it renders as the ordinary
        /// code block the assistant's explanatory SQL already renders as. An error against a
        /// statement the user never ran is a complaint about the catalog moving, which is not a
        /// fault and not news.
        ///
        /// **Never stored.** It is an answer about the catalog *as it is now*, not a fact about
        /// the conversation: persisted, a card retired because a table was missing would stay
        /// retired after that table came back, since the re-check only ever sets it. `skip`
        /// rather than `skip_serializing` so a file written by an older build reads as runnable
        /// and is re-checked on the next open, which is the correct answer either way.
        #[serde(skip)]
        stale: bool,
    },
}

/// One exchange in a conversation.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
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
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
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
    /// The identity that outlives the window (AS-07) — the stored document's name.
    ///
    /// Beside [`ChatId`] rather than instead of it: the id is this window's key, minted per
    /// window and cheap to compare, while this is what the same conversation is called in every
    /// window that ever holds it. A hydrated conversation takes a **fresh** id and keeps this.
    pub uuid: Uuid,
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
    /// When it was first asked something, epoch millis — `None` while it has never been asked.
    ///
    /// A conversation nobody has spoken to is worth no file: the pane always holds one fresh
    /// "New chat", and writing that on every open would litter the store with empties.
    pub created_ms: Option<u64>,
    /// When it last settled something worth storing, epoch millis. The list's sort key, because
    /// "newest first" means the one you last worked in.
    pub updated_ms: u64,
    /// Whether the stored document is behind this value.
    ///
    /// Set by everything that changes what would be written — a send, a fold, a pick — and
    /// cleared by the write. It is what makes the teardown pass cheap (a conversation settled an
    /// hour ago writes nothing) and what catches a **pick** changed after the last settle, which
    /// a settle-only writer would lose.
    pub dirty: bool,
}

impl Chat {
    fn new(id: ChatId, pick: Pick) -> Chat {
        Chat {
            id,
            uuid: Uuid::new_v4(),
            title: "New chat".into(),
            pick,
            turns: Vec::new(),
            pinned: Vec::new(),
            memory: Arc::new(Mutex::new(Conversation::new())),
            running: None,
            created_ms: None,
            updated_ms: 0,
            dirty: false,
        }
    }

    /// Whether this conversation has ever been asked anything — what tells a fresh "New chat"
    /// from one whose turns were all evicted.
    pub fn is_stored(&self) -> bool {
        self.created_ms.is_some()
    }

    /// Whether a turn is streaming — what flips the composer's send into a stop.
    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// The reply of the turn in flight, if the last turn is one.
    fn reply(&self) -> Option<&Reply> {
        match self.turns.last() {
            Some(Turn::Reply(reply)) => Some(reply),
            _ => None,
        }
    }

    fn reply_mut(&mut self) -> Option<&mut Reply> {
        match self.turns.last_mut() {
            Some(Turn::Reply(reply)) => Some(reply),
            _ => None,
        }
    }
}

/// One row of the chat switcher — a conversation this window is holding, or one it has stored.
///
/// The two are listed together because to the user they are one list: which of them happens to
/// have its transcript in memory right now is not a distinction the pane should draw.
#[derive(Clone, PartialEq, Debug)]
pub struct ChatRow {
    pub key: RowKey,
    pub title: String,
    /// As the provider spells it; empty when nothing was ever picked.
    pub model: String,
    pub messages: usize,
    pub current: bool,
    /// What it sorts on. Not shown.
    order: u64,
}

/// Which conversation a row is — and what pressing it has to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RowKey {
    /// Held in this window: pressing it is a switch.
    Live(ChatId),
    /// Stored but not loaded: pressing it is a read.
    Shelved(Uuid),
}

/// Every conversation in this window, and which one is open.
pub struct Chats {
    chats: Vec<Chat>,
    /// The conversations that are **stored but not loaded** (AS-07).
    ///
    /// The switcher lists these beside the live ones, and pressing one hydrates it. Heads rather
    /// than transcripts, because listing must not read every stored document — and because a
    /// window holding twenty full transcripts to render twenty rows is paying for a list in
    /// memory it only ever paints one of.
    shelf: Vec<ChatHead>,
    /// What the last eviction demoted, waiting for its caller to write it. See
    /// [`evict`](Chats::evict) — this value does no IO of its own.
    shed: Vec<Chat>,
    active: ChatId,
    next: u64,
}

impl Chats {
    /// A window's chats always contain at least one: a pane with no conversation is a dead end,
    /// so opening the pane never has to decide whether to make one.
    pub fn new(pick: Pick) -> Chats {
        Chats {
            chats: vec![Chat::new(ChatId(0), pick)],
            shelf: Vec::new(),
            shed: Vec::new(),
            active: ChatId(0),
            next: 1,
        }
    }

    /// The same, with what the store had — a fresh conversation open in front of the ones this
    /// project already held.
    ///
    /// The fresh one stays active on purpose: reopening a project should not reopen whatever
    /// question was being asked when it was last closed, any more than it reopens a dialog. The
    /// stored conversations are *listed*, and one press away.
    pub fn restored(pick: Pick, shelf: Vec<ChatHead>) -> Chats {
        Chats {
            shelf,
            ..Chats::new(pick)
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Chat> {
        self.chats.iter()
    }

    /// Every conversation the switcher lists — open and stored together, **newest first**.
    ///
    /// Ordered here rather than in the pane because it is a fact about the conversations and not
    /// about how they are drawn, and because "newest" has one wrinkle worth stating in one place:
    /// a conversation nobody has asked anything sorts to the **top**, not the bottom. Its
    /// `updated_ms` is zero because nothing has happened in it yet, but it is the most recent
    /// thing the user did — they just pressed New chat.
    pub fn rows(&self) -> Vec<ChatRow> {
        let mut rows: Vec<ChatRow> = self
            .chats
            .iter()
            .map(|chat| ChatRow {
                key: RowKey::Live(chat.id),
                title: chat.title.clone(),
                model: chat.pick.model.clone(),
                messages: chat.turns.len(),
                current: chat.id == self.active,
                order: match chat.is_stored() {
                    true => chat.updated_ms,
                    false => u64::MAX,
                },
            })
            .chain(self.shelf.iter().map(|head| ChatRow {
                key: RowKey::Shelved(head.id),
                title: head.title.clone(),
                model: head.pick.model.clone(),
                messages: head.messages,
                current: false,
                order: head.updated_ms,
            }))
            .collect();
        rows.sort_by_key(|row| Reverse(row.order));
        rows
    }

    /// Take a stored conversation into the window and make it active.
    ///
    /// The hydrated conversation keeps its stored identity and its **pick** — what it was
    /// talking to, not what a new chat would start on — and takes a fresh [`ChatId`], which is
    /// only ever this window's key.
    pub fn hydrate(&mut self, doc: ChatDoc, memory: Conversation) -> ChatId {
        if let Some(live) = self.chats.iter().find(|chat| chat.uuid == doc.id) {
            let id = live.id;
            self.active = id;
            return id;
        }
        let id = ChatId(self.next);
        self.next += 1;
        self.shelf.retain(|head| head.id != doc.id);
        self.chats.push(Chat {
            id,
            uuid: doc.id,
            title: doc.title,
            pick: doc.pick,
            turns: doc.turns,
            pinned: Vec::new(),
            memory: Arc::new(Mutex::new(memory)),
            running: None,
            created_ms: Some(doc.created_ms),
            updated_ms: doc.updated_ms,
            dirty: false,
        });
        self.shed = self.evict();
        self.active = id;
        id
    }

    /// Forget every conversation, open and stored, and start again on one fresh chat.
    ///
    /// Every turn in flight is stopped first, for [`delete`](Chats::delete)'s reason: a `Chat`
    /// dropped without stopping keeps streaming with nothing left holding its handle.
    pub fn clear(&mut self, fresh: Pick) {
        self.stop_all();
        self.chats.clear();
        self.shelf.clear();
        self.open(fresh);
    }

    /// Drop a stored conversation from the shelf. The file is the caller's to remove — this
    /// value does no IO.
    pub fn forget(&mut self, id: Uuid) {
        self.shelf.retain(|head| head.id != id);
    }

    /// Edit what `id` is talking to, and mark it for the store.
    ///
    /// **One funnel for all three controls** (provider, model, effort), because a pick is part of
    /// what a stored conversation is: changing the model and closing the window has to leave the
    /// conversation talking to the model you chose. Writing `chat.pick` at a call site would skip
    /// the mark and lose exactly that.
    pub fn repick(&mut self, id: ChatId, edit: impl FnOnce(&mut Pick)) {
        let Some(chat) = self.get_mut(id) else {
            return;
        };
        let before = chat.pick.clone();
        edit(&mut chat.pick);
        if chat.pick != before {
            chat.dirty = true;
        }
    }

    /// Mark a restored offer whose statement no longer holds, so the card drops its press.
    ///
    /// Takes the **statement** as well as the position, and only marks a block that still carries
    /// it: the check is asynchronous, and a send landing in between appends turns while
    /// [`MAX_TURNS`] eviction shifts every index down. Marked by position alone, the answer about
    /// one card would retire another whose statement is fine.
    pub fn stale_offer(&mut self, id: ChatId, turn: usize, block: usize, checked: &str) {
        let Some(chat) = self.get_mut(id) else {
            return;
        };
        let Some(Turn::Reply(reply)) = chat.turns.get_mut(turn) else {
            return;
        };
        if let Some(Block::Offer { sql, stale }) = reply.blocks.get_mut(block) {
            if sql == checked {
                *stale = true;
            }
        }
    }

    /// Hold the window to [`MAX_CHATS`] live conversations, **demoting** rather than dropping.
    ///
    /// The oldest live conversation goes back to the shelf, where its stored document already
    /// is: over-cap eviction used to be a discard, and with a store behind it a discard would be
    /// the window deciding to forget something the user can still see listed.
    ///
    /// **Answers what it evicted**, because this value does no IO and a conversation still marked
    /// dirty has state the shelf's row would then misrepresent — its file is older than the row,
    /// or absent. The caller writes what comes back before the row is offered.
    #[must_use]
    fn evict(&mut self) -> Vec<Chat> {
        let mut evicted = Vec::new();
        while self.chats.len() > MAX_CHATS {
            let id = self.chats[0].id;
            self.stop(id);
            let chat = self.chats.remove(0);
            if let Some(head) = head_of(&chat) {
                self.shelf.push(head);
                evicted.push(chat);
            }
        }
        self.shelf.sort_by_key(|head| Reverse(head.updated_ms));
        evicted
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
        self.shed = self.evict();
        self.active = id;
        id
    }

    /// Take whatever the last [`open`](Chats::open) or [`hydrate`](Chats::hydrate) evicted, so
    /// the caller can write it before its shelved row is offered. Empty on every other call.
    pub fn shed(&mut self) -> Vec<Chat> {
        std::mem::take(&mut self.shed)
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
        self.stop(id);
        if let Some(chat) = self.get(id) {
            let uuid = chat.uuid;
            self.shelf.retain(|head| head.id != uuid);
        }
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
        chat.created_ms.get_or_insert_with(now_ms);
        chat.updated_ms = now_ms();
        chat.dirty = true;
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
            TurnEvent::Started => {}
            TurnEvent::Delta(text) => reply.say(&text),
            TurnEvent::Runnable(sql) => reply.blocks.push(Block::Offer { sql, stale: false }),
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

    /// End `id`'s turn: the settle's own sentence, and the reply closed.
    ///
    /// **AS-07 hooks here.** This is the one place a conversation is known to be complete, which
    /// is what a persistence writer needs and what a per-event writer could not tell.
    ///
    /// The running handle is **not** released here — [`finish`](Chats::finish) does that, once
    /// the turn's record is written. A turn's task is root-scoped so a backgrounded conversation
    /// keeps streaming, and `running` is the only thing [`stop_all`](Chats::stop_all) can reach
    /// it by; dropping the handle at the settle would leave the write that follows it holding
    /// state this subtree may drop underneath it.
    pub fn settle(&mut self, id: ChatId, settle: Settle) {
        let Some(chat) = self.get_mut(id) else {
            return;
        };
        chat.updated_ms = now_ms();
        chat.dirty = true;
        if let Some(reply) = chat.reply_mut() {
            reply.note = settle.note();
            reply.settled = true;
            for block in &mut reply.blocks {
                if let Block::Step(step) = block {
                    if step.failed.is_none() {
                        step.failed = Some(false);
                        step.facts.stopped = Some(StopReason::Cancelled.to_string());
                    }
                }
            }
        }
    }

    /// Release `id`'s turn handle — the last thing its task does, after its record is stored.
    ///
    /// Separate from [`settle`](Chats::settle) so the window between "the reply is closed" and
    /// "the conversation is on disk" is still a turn the teardown can cancel.
    pub fn finish(&mut self, id: ChatId) {
        if let Some(chat) = self.get_mut(id) {
            chat.running = None;
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
        let settled = self
            .get(id)
            .and_then(Chat::reply)
            .is_some_and(|reply| reply.settled);
        if !settled {
            self.settle(id, Settle::Cancelled);
        }
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

/// How many conversations a project keeps, in the window's list and on disk: the user's
/// `max_chats` setting, which is the *only* source — [`history_cap`](super::history::history_cap)
/// exactly, including the floor, because this cap drives a rotation that deletes.
pub fn chats_cap(config: ConfigStation) -> usize {
    config.peek().settings.ai.max_chats.max(CHATS_MIN)
}

/// Stand this project's chat satellite up and provide it. Call once in the window root.
///
/// The seed pick is the caller's, because it is a read of the **config** store — which this
/// module deliberately knows nothing about, so that changing what a new chat starts on stays one
/// question answered in one place. `root` and `report` are what the stored conversations need:
/// where they live, and where a write that failed is said out loud.
///
/// The heads load **synchronously at mount**, on `History::load`'s precedent — a bounded number
/// of small head parses, and the switcher has to be able to list them the first time it opens.
pub fn use_init_chats(seed: Pick, root: PathBuf, cap: usize, report: ReportCtx) -> ChatsCtx {
    let chats = use_provide_context({
        let root = root.clone();
        move || {
            let shelf = chat_store::load_heads(&root, cap).unwrap_or_else(|e| {
                tracing::error!("load conversations: {e}");
                Vec::new()
            });
            State::create(Chats::restored(seed, shelf))
        }
    });
    use_drop(move || {
        let mut chats = chats;
        chats.write().stop_all();
        let held = chats.peek();
        for chat in held.iter().filter(|chat| chat.dirty) {
            persisted(report, ProjectFile::Chats, || chat_store::save(&root, chat));
        }
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

    /// The rows that are stored rather than open — read through `rows`, which is the one list
    /// the pane reads too.
    fn shelved(chats: &Chats) -> Vec<ChatRow> {
        chats
            .rows()
            .into_iter()
            .filter(|row| matches!(row.key, RowKey::Shelved(_)))
            .collect()
    }

    fn head(id: Uuid, title: &str, updated_ms: u64) -> ChatHead {
        ChatHead {
            version: 1,
            id,
            title: title.into(),
            updated_ms,
            pick: pick(),
            messages: 4,
        }
    }

    /// The switcher lists open and stored conversations as **one** list, newest first — with a
    /// conversation nobody has asked anything at the top, because pressing New chat is the most
    /// recent thing that happened even though nothing has happened *in* it.
    #[test]
    fn the_switcher_lists_open_and_stored_together_newest_first() {
        let stored = Uuid::new_v4();
        let mut chats = Chats::restored(pick(), vec![head(stored, "last week", 10)]);
        let old = chats.active_id();
        chats.ask(old, "yesterday".into(), vec![]);

        let fresh = chats.open(pick());
        let rows = chats.rows();
        let titles: Vec<&str> = rows.iter().map(|row| row.title.as_str()).collect();
        assert_eq!(titles, ["New chat", "yesterday", "last week"]);
        assert_eq!(rows[0].key, RowKey::Live(fresh));
        assert_eq!(rows[2].key, RowKey::Shelved(stored));
        assert!(rows[0].current);
        assert_eq!(rows[2].messages, 4, "the head's own count, not a read");
    }

    /// Opening a stored conversation takes it off the shelf rather than listing it twice, and it
    /// keeps talking to whatever it was talking to.
    #[test]
    fn hydrating_moves_a_conversation_off_the_shelf_and_keeps_its_pick() {
        let stored = Uuid::new_v4();
        let mut chats = Chats::restored(Pick::default(), vec![head(stored, "stored", 10)]);
        let doc = ChatDoc {
            version: 1,
            id: stored,
            title: "stored".into(),
            created_ms: 1,
            updated_ms: 10,
            pick: pick(),
            messages: 0,
            turns: vec![],
            memory: Value::Null,
        };
        let id = chats.hydrate(doc, Conversation::new());

        assert!(!shelved(&chats)
            .iter()
            .any(|row| row.key == RowKey::Shelved(stored)));
        assert_eq!(chats.active_id(), id);
        let chat = chats.get(id).expect("hydrated");
        assert_eq!(chat.uuid, stored);
        assert_eq!(chat.pick.model, "claude-sonnet-4-5");
        assert!(!chat.dirty, "what was just read is what is on disk");
        assert_eq!(chats.rows().len(), 2, "the fresh one and the hydrated one");
    }

    /// Over-cap eviction **demotes**, because the evicted conversation is stored: dropping it
    /// would be the window deciding to forget something the user can still see listed.
    #[test]
    fn an_evicted_conversation_goes_to_the_shelf_rather_than_away() {
        let mut chats = Chats::new(pick());
        let first = chats.active_id();
        chats.ask(first, "the oldest question".into(), vec![]);
        for _ in 0..MAX_CHATS {
            chats.open(pick());
        }
        assert!(chats.get(first).is_none(), "it is no longer held open");
        let shelved = shelved(&chats);
        assert_eq!(shelved.len(), 1);
        assert_eq!(shelved[0].title, "the oldest question");
    }

    /// A send and a settle both leave the conversation needing a write; a fresh one does not,
    /// which is what keeps the teardown pass from writing empties.
    #[test]
    fn a_conversation_is_dirty_from_its_first_question_and_not_before() {
        let mut chats = Chats::new(pick());
        let id = chats.active_id();
        assert!(!chats.active().dirty);
        assert!(!chats.active().is_stored());

        chats.ask(id, "why?".into(), vec![]);
        assert!(chats.active().dirty);
        assert!(chats.active().is_stored(), "the first question stores it");
        assert!(chats.active().created_ms.is_some());

        chats.get_mut(id).expect("the chat").dirty = false;
        chats.settle(id, Settle::Answered);
        assert!(chats.active().dirty, "a settle is a write");
    }

    /// A restored offer the catalog has moved under drops its press, and says nothing.
    #[test]
    fn a_stale_offer_keeps_its_sql_and_loses_its_press() {
        let mut chats = Chats::new(pick());
        let id = chats.active_id();
        chats.ask(id, "show me".into(), vec![]);
        chats.fold(id, TurnEvent::Runnable("SELECT 1".into()));

        chats.stale_offer(id, 1, 0, "SELECT 1");
        chats.stale_offer(id, 1, 0, "SELECT 2");
        let Some(Turn::Reply(reply)) = chats.active().turns.get(1) else {
            panic!("a reply");
        };
        assert_eq!(
            reply.blocks[0],
            Block::Offer {
                sql: "SELECT 1".into(),
                stale: true
            }
        );
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
