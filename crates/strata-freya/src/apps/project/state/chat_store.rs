//! **What a conversation is on disk** (AS-07) — the document under `.strata/chats/`, and the
//! reads and writes that put it there.
//!
//! The satellite beside it ([`chat`](super::chat)) is the live value; this module is the only
//! thing that knows its stored shape. Split out for the reason every satellite's IO is:
//! [`Chats`](super::chat::Chats) has no project root and no reporter, and giving it either to
//! save a file would make the one value the whole window folds into an IO type.
//!
//! ## Two lists, and both of them travel
//!
//! A stored conversation carries **the turns the pane paints and the memory the model reads**.
//! It has to carry both, because neither reconstructs the other: the transcript has no copy of a
//! resolved `@`-mention body, of a tool's answer, or of the reasoning parts a provider wants
//! handed back, and the model's list has no step cards. Storing only the transcript would
//! restore a conversation you can read and cannot continue — the *appearance* of one, which is
//! what AS-04 refused to ship.
//!
//! The memory rides as [`Conversation::to_json`]'s own value: `genai`'s serde shape at the
//! pinned version, not a vocabulary of ours mirroring it. That is the framework-native choice
//! and it has one consequence worth stating — an upgrade that moves that shape is a change to
//! this document, and the fallback is [`Read::Memoryless`] rather than a lost conversation.
//!
//! ## Nothing here is fatal
//!
//! A document this build cannot use never takes the pane down. An unknown version is skipped, a
//! corrupt file is skipped, and a memory that will not parse still yields the transcript — the
//! user's own record — with a fresh memory under it. Three tiers, one rule: the worst outcome is
//! losing what the model remembered, never what the user wrote.

use std::cmp::Reverse;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string, Value};
use strata_agent::assistant::Conversation;
use strata_core::project::{chat_files, chat_path, delete_chat, save_chat};
use uuid::Uuid;

use super::chat::{Chat, Pick, Turn};

/// The stored document's version.
///
/// Bumped when the shape changes in a way an older build must not read — including a `genai`
/// upgrade that moves the memory's own shape. An unrecognised version is a file this build
/// **skips**, never one it tries and fails to parse.
pub const CHAT_VERSION: u32 = 1;

/// One conversation, as `.strata/chats/<uuid>.json`.
#[derive(Serialize, Deserialize)]
pub struct ChatDoc {
    pub version: u32,
    pub id: Uuid,
    pub title: String,
    pub created_ms: u64,
    pub updated_ms: u64,
    pub pick: Pick,
    /// How many exchanges it holds — **in the head**, so the switcher can say "N messages"
    /// without reading the turns.
    pub messages: usize,
    #[serde(default)]
    pub turns: Vec<Turn>,
    /// The model's own memory, [`Conversation::to_json`]'s value verbatim.
    #[serde(default)]
    pub memory: Value,
}

/// What the switcher lists: everything a row shows, and nothing a row does not.
///
/// A separate deserialize target rather than a `ChatDoc` with the big fields dropped afterwards.
/// Listing must not read every transcript, and serde walks past the `turns` and `memory` tokens
/// without building them when nothing asks for them — so a window with twenty stored
/// conversations parses twenty heads, not twenty megabytes.
#[derive(Clone, Debug, Deserialize)]
pub struct ChatHead {
    pub version: u32,
    pub id: Uuid,
    pub title: String,
    pub updated_ms: u64,
    #[serde(default)]
    pub pick: Pick,
    #[serde(default)]
    pub messages: usize,
}

/// How a stored conversation came back.
pub enum Read {
    /// Whole: the transcript and the memory the model reads back. Continuable.
    Full(Box<ChatDoc>),
    /// The transcript, with a memory this build could not read.
    ///
    /// Still opened, and still sendable — the next turn simply starts the model's memory over.
    /// The alternative is refusing the user their own record because a provider vocabulary
    /// moved, which is the wrong thing to lose.
    Memoryless(Box<ChatDoc>),
}

impl Read {
    /// The conversation and the memory to open it with — a fresh one when this build could not
    /// read what was stored.
    pub fn into_parts(self) -> (ChatDoc, Conversation) {
        match self {
            Read::Full(doc) => {
                let memory = Conversation::from_json(doc.memory.clone()).unwrap_or_default();
                (*doc, memory)
            }
            Read::Memoryless(doc) => (*doc, Conversation::new()),
        }
    }
}

/// Render a live conversation as its document. `None` when it has never been asked anything —
/// a fresh "New chat" is not a conversation yet, and the store is not the place to record that
/// the pane was open.
pub fn document(chat: &Chat) -> Option<ChatDoc> {
    let created_ms = chat.created_ms?;
    // A poisoned lock is a turn task that panicked; the transcript is still worth storing, so
    // this reads what it can and stores an empty memory rather than propagating the panic.
    let memory = chat
        .memory
        .lock()
        .ok()
        .and_then(|memory| memory.to_json().ok())
        .unwrap_or(Value::Null);
    Some(ChatDoc {
        version: CHAT_VERSION,
        id: chat.uuid,
        title: chat.title.clone(),
        created_ms,
        updated_ms: chat.updated_ms,
        pick: chat.pick.clone(),
        messages: chat.turns.len(),
        turns: chat.turns.clone(),
        memory,
    })
}

/// What the switcher would list for a live conversation — how a demoted one goes back to the
/// shelf without a read of the file it was just written to. `None` for one never asked anything.
pub fn head_of(chat: &Chat) -> Option<ChatHead> {
    chat.created_ms?;
    Some(ChatHead {
        version: CHAT_VERSION,
        id: chat.uuid,
        title: chat.title.clone(),
        updated_ms: chat.updated_ms,
        pick: chat.pick.clone(),
        messages: chat.turns.len(),
    })
}

/// Write one conversation. A chat with nothing in it writes nothing and is not an error.
pub fn save(root: &Path, chat: &Chat) -> Result<(), String> {
    let Some(doc) = document(chat) else {
        return Ok(());
    };
    write(root, &doc)
}

/// Write a document that has already been rendered — what a caller uses when the render and the
/// write happen in different places (the send task renders on the UI executor and writes on a
/// worker, so the `Chat` itself cannot cross).
pub fn write(root: &Path, doc: &ChatDoc) -> Result<(), String> {
    let json = to_string(doc).map_err(|e| e.to_string())?;
    save_chat(root, &doc.id, &json)
}

/// Forget one stored conversation.
pub fn forget_chat(root: &Path, id: &Uuid) -> Result<(), String> {
    delete_chat(root, id)
}

/// Every stored conversation's head, **newest first**, rotated down to `cap`.
///
/// Rotation happens here, on `load_history`'s precedent and for its reason: the cap is a
/// *retention* setting, so lowering it in Settings has to shrink the stored set rather than
/// merely show less of it. What falls past the cap is deleted, oldest first.
///
/// A file this build cannot read is skipped with a log line and does not count against the cap —
/// it is not a conversation as far as this build is concerned, and letting it evict one that is
/// would be a corrupt file costing a good one.
pub fn load_heads(root: &Path, cap: usize) -> Result<Vec<ChatHead>, String> {
    let mut heads: Vec<ChatHead> = Vec::new();
    for path in chat_files(root)? {
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                tracing::warn!("read conversation {}: {e}", path.display());
                continue;
            }
        };
        match from_str::<ChatHead>(&text) {
            Ok(head) if head.version == CHAT_VERSION => heads.push(head),
            Ok(head) => tracing::warn!(
                "conversation {} is version {}, not {CHAT_VERSION}; skipped",
                path.display(),
                head.version
            ),
            Err(e) => tracing::warn!("conversation {} will not parse: {e}", path.display()),
        }
    }
    heads.sort_by_key(|head| Reverse(head.updated_ms));
    for head in heads.iter().skip(cap) {
        // Best-effort: a delete that fails leaves the file for the next open to retry, which is
        // the same shape `load_history`'s rotation takes.
        let _ = delete_chat(root, &head.id);
    }
    heads.truncate(cap);
    Ok(heads)
}

/// Read one stored conversation whole — what a switcher press does.
///
/// `Ok(None)` is "there is no such conversation, or this build cannot use the one there is": a
/// row whose file has gone or whose version is not ours resolves to nothing rather than to an
/// error the user has to dismiss.
pub fn load(root: &Path, id: &Uuid) -> Result<Option<Read>, String> {
    let path = chat_path(root, id);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let doc: ChatDoc = match from_str(&text) {
        Ok(doc) => doc,
        Err(e) => {
            tracing::warn!("conversation {} will not parse: {e}", path.display());
            return Ok(None);
        }
    };
    if doc.version != CHAT_VERSION {
        tracing::warn!(
            "conversation {} is version {}, not {CHAT_VERSION}; skipped",
            path.display(),
            doc.version
        );
        return Ok(None);
    }
    // The memory is the one part that may fail on its own terms — it is a provider vocabulary,
    // and the pin behind it can move. Failing it costs what the model remembered, never the
    // transcript.
    let readable = Conversation::from_json(doc.memory.clone()).is_ok();
    Ok(Some(if readable {
        Read::Full(Box::new(doc))
    } else {
        tracing::warn!(
            "conversation {} carries a memory this build cannot read; restored without it",
            path.display()
        );
        Read::Memoryless(Box::new(doc))
    }))
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::path::PathBuf;
    use std::process;

    use serde_json::json;
    use strata_agent::assistant::Facts;
    use strata_core::ai::{Effort, ProviderKind};

    use super::super::chat::{Block, Chats, Step};
    use super::*;

    /// A fresh temp project folder, cleaned up on drop.
    struct TempRoot(PathBuf);
    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = env::temp_dir().join(format!("strata-chat-store-{tag}-{}", process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            TempRoot(dir)
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A conversation with one asked turn, a step card and an offer.
    fn asked() -> Chats {
        let mut chats = Chats::new(Pick::default());
        let id = chats.active_id();
        chats.ask(id, "how many orders?".into(), vec!["@orders".into()]);
        let chat = chats.get_mut(id).expect("the chat");
        if let Some(Turn::Reply(reply)) = chat.turns.last_mut() {
            reply.blocks.push(Block::Prose("Twelve.".into()));
            reply.blocks.push(Block::Step(Box::new(Step {
                call: "call_1".into(),
                tool: "run".into(),
                arguments: json!({ "sql": "SELECT 1" }),
                failed: Some(false),
                facts: Facts {
                    rows: Some(12),
                    elapsed_ms: Some(7),
                    ..Facts::default()
                },
            })));
            reply.blocks.push(Block::Offer {
                sql: "SELECT 1".into(),
                stale: false,
            });
            reply.settled = true;
        }
        chats
    }

    /// The whole promise in one pass: what is written comes back, transcript and all.
    #[test]
    fn a_conversation_round_trips_through_the_store() {
        let root = TempRoot::new("round-trip");
        let chats = asked();
        let chat = chats.active();
        save(&root.0, chat).unwrap();

        let read = load(&root.0, &chat.uuid).unwrap().expect("it is there");
        assert!(matches!(read, Read::Full(_)));
        // The fixture's turns are built by hand, so its model memory is legitimately empty —
        // what matters here is that the stored document came back whole.
        let (doc, _) = read.into_parts();
        assert_eq!(doc.title, "how many orders?");
        assert_eq!(doc.messages, 2);
        assert_eq!(doc.turns, chat.turns);
        // The step's facts are the engine's own numbers, still.
        let has_facts = doc.turns.iter().any(|turn| match turn {
            Turn::Reply(reply) => reply.blocks.iter().any(|block| match block {
                Block::Step(step) => step.facts.rows == Some(12),
                _ => false,
            }),
            Turn::User { .. } => false,
        });
        assert!(has_facts, "the step's facts did not survive");
    }

    /// A conversation nobody has spoken to is not a conversation, and the store must not fill up
    /// with the empty one the pane always holds.
    #[test]
    fn a_never_asked_chat_writes_nothing() {
        let root = TempRoot::new("empty");
        let chats = Chats::new(Pick::default());
        save(&root.0, chats.active()).unwrap();
        assert!(load_heads(&root.0, 20).unwrap().is_empty());
        assert!(document(chats.active()).is_none());
    }

    /// Listing reads heads, newest first — and the head carries its own message count so a row
    /// never has to open a transcript to render.
    #[test]
    fn heads_list_newest_first_without_reading_the_turns() {
        let root = TempRoot::new("heads");
        for (i, stamp) in [10_u64, 30, 20].iter().enumerate() {
            let mut chats = asked();
            let id = chats.active_id();
            let chat = chats.get_mut(id).unwrap();
            chat.title = format!("chat {i}");
            chat.updated_ms = *stamp;
            save(&root.0, chat).unwrap();
        }
        let heads = load_heads(&root.0, 20).unwrap();
        let titles: Vec<&str> = heads.iter().map(|h| h.title.as_str()).collect();
        assert_eq!(titles, ["chat 1", "chat 2", "chat 0"], "newest first");
        assert!(heads.iter().all(|h| h.messages == 2));
    }

    /// The cap is **retention**: lowering it deletes what falls past it, oldest first, rather
    /// than merely showing less.
    #[test]
    fn the_cap_rotates_the_store_down_on_load() {
        let root = TempRoot::new("rotate");
        for stamp in 1_u64..=5 {
            let mut chats = asked();
            let id = chats.active_id();
            let chat = chats.get_mut(id).unwrap();
            chat.updated_ms = stamp;
            save(&root.0, chat).unwrap();
        }
        assert_eq!(chat_files(&root.0).unwrap().len(), 5);

        let heads = load_heads(&root.0, 2).unwrap();
        assert_eq!(heads.len(), 2);
        assert_eq!(heads[0].updated_ms, 5);
        assert_eq!(heads[1].updated_ms, 4);
        // The files themselves are gone, not merely unlisted.
        assert_eq!(chat_files(&root.0).unwrap().len(), 2);
    }

    /// A document from a build that is not this one is skipped, and the rest of the list still
    /// loads — the pane never goes down over a file it did not write.
    #[test]
    fn an_unknown_version_is_skipped_and_the_rest_still_load() {
        let root = TempRoot::new("version");
        let chats = asked();
        let good = chats.active();
        save(&root.0, good).unwrap();

        let future = Uuid::new_v4();
        let mut doc = document(good).unwrap();
        doc.version = CHAT_VERSION + 99;
        doc.id = future;
        save_chat(&root.0, &future, &to_string(&doc).unwrap()).unwrap();
        // …and one that is not a document at all.
        let junk = Uuid::new_v4();
        save_chat(&root.0, &junk, "{ not json").unwrap();

        let heads = load_heads(&root.0, 20).unwrap();
        assert_eq!(heads.len(), 1);
        assert_eq!(heads[0].id, good.uuid);
        assert!(load(&root.0, &future).unwrap().is_none());
        assert!(load(&root.0, &junk).unwrap().is_none());
        // A skipped file is not deleted by rotation either: it never counted.
        assert!(load(&root.0, &good.uuid).unwrap().is_some());
    }

    /// A memory this build cannot read costs the model's recollection and **not** the user's
    /// transcript — the conversation still opens, and still sends.
    #[test]
    fn an_unreadable_memory_still_yields_the_transcript() {
        let root = TempRoot::new("memoryless");
        let chats = asked();
        let chat = chats.active();
        let mut doc = document(chat).unwrap();
        doc.memory = json!({ "shape": "from some other version" });
        save_chat(&root.0, &doc.id, &to_string(&doc).unwrap()).unwrap();

        let read = load(&root.0, &chat.uuid).unwrap().expect("it is there");
        assert!(matches!(read, Read::Memoryless(_)));
        let (doc, memory) = read.into_parts();
        assert_eq!(doc.turns, chat.turns);
        assert!(memory.is_empty(), "it opens with a fresh memory");
    }

    /// An absent conversation is nothing, not an error — the row and the file are two records of
    /// one thing and either may go first.
    #[test]
    fn an_absent_conversation_reads_as_nothing() {
        let root = TempRoot::new("absent");
        assert!(load(&root.0, &Uuid::new_v4()).unwrap().is_none());
        forget_chat(&root.0, &Uuid::new_v4()).unwrap();
        assert!(load_heads(&root.0, 20).unwrap().is_empty());
    }

    /// A conversation restored from disk keeps talking to what it was talking to, rather than
    /// being re-seeded from whatever Settings says today.
    #[test]
    fn the_pick_travels_with_the_conversation() {
        let root = TempRoot::new("pick");
        let mut chats = asked();
        let id = chats.active_id();
        chats.get_mut(id).unwrap().pick = Pick {
            provider: Some(ProviderKind::Anthropic),
            model: "claude-sonnet-5".into(),
            effort: Some(Effort::High),
        };
        let chat = chats.active();
        save(&root.0, chat).unwrap();

        let (doc, _) = load(&root.0, &chat.uuid)
            .unwrap()
            .expect("it is there")
            .into_parts();
        assert_eq!(doc.pick.model, "claude-sonnet-5");
        assert_eq!(doc.pick.provider, Some(ProviderKind::Anthropic));
    }
}
