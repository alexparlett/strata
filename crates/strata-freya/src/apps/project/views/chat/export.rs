//! **A conversation as a file** — the chat pane's Export, and the Markdown it writes.
//!
//! Markdown rather than the stored document, and the two are deliberately different things. The
//! JSON under `.strata/chats/` is what *Strata* reads back — it carries the model's own memory
//! and is meaningless outside the app. What a person exports is what they would paste into a
//! ticket or a wiki: the conversation, readable, with the evidence still attached.
//!
//! So this renders **more** than the copy button's `plain` and less than the store. A step card
//! becomes one line of the engine's own figures, because a transcript that dropped its citations
//! would be the assistant's claims with the evidence removed — and those figures are the whole
//! reason the card exists.

use std::fs;
use std::path::Path;

use freya::prelude::spawn;

use crate::apps::project::state::{log_event, Block, Chat, LogCtx, LogLevel, Step, Turn};
use crate::task::offload;

/// Ask for a destination, then write. Spawned, because both halves wait: the dialog on the user
/// and the write on a disk.
///
/// The outcome goes to the **event log**, which is where a write the user asked for is recorded
/// — not through the `persisted` funnel, whose subject is Strata falling behind on its own files.
/// A destination the user chose and a file Strata owes them are different kinds of fact.
///
/// **Never called from the menu item's own handler.** `spawn` binds the task to whichever scope
/// is current during dispatch, and a menu item's scope is unmounted the instant the menu closes —
/// so a press that fired this and then closed the menu dropped the task before it was ever
/// polled, and Export chat silently did nothing. The header therefore records the intent and
/// this runs from the header's own scope, which outlives the menu.
pub fn export_chat(chat: &Chat, log: LogCtx) {
    let document = markdown(chat);
    let suggested = file_name(&chat.title);
    spawn(async move {
        let picked = rfd::AsyncFileDialog::new()
            .set_title("Export chat")
            .set_file_name(&suggested)
            .save_file()
            .await
            .map(|handle| handle.path().to_path_buf());
        let Some(path) = picked else {
            return;
        };
        let written = offload(move || write(&path, &document).map(|()| path)).await;
        match written {
            Some(Ok(path)) => log_event(
                log,
                LogLevel::Ok,
                format!("Exported chat to {}", path.display()),
            ),
            Some(Err(why)) => log_event(log, LogLevel::Error, format!("Chat export failed: {why}")),
            None => {}
        }
    });
}

/// A plain write, not [`write_atomic`](strata_core::util::write_atomic): the destination is one
/// the user just named for this file, so there is no previous version of it to protect.
fn write(path: &Path, document: &str) -> Result<(), String> {
    fs::write(path, document).map_err(|e| format!("{}: {e}", path.display()))
}

/// The conversation as Markdown.
pub fn markdown(chat: &Chat) -> String {
    let mut out = format!("# {}\n", chat.title);
    if !chat.pick.model.is_empty() {
        out.push_str(&format!("\n{}\n", chat.pick.model));
    }
    for turn in &chat.turns {
        out.push('\n');
        match turn {
            Turn::User { text, chips, at } => {
                out.push_str(&format!("## You · {at}\n\n"));
                if !chips.is_empty() {
                    out.push_str(&format!("{}\n\n", chips.join(" ")));
                }
                out.push_str(text);
                out.push('\n');
            }
            Turn::Reply(reply) => {
                out.push_str(&format!("## Strata · {}\n", reply.at));
                for block in &reply.blocks {
                    out.push('\n');
                    match block {
                        Block::Prose(text) => {
                            out.push_str(text);
                            out.push('\n');
                        }
                        Block::Step(step) => out.push_str(&step_line(step)),
                        Block::Offer { sql, .. } => {
                            out.push_str(&format!("```sql\n{}\n```\n", sql.trim_end()));
                        }
                    }
                }
                if let Some(note) = &reply.note {
                    out.push_str(&format!("\n_{note}_\n"));
                }
            }
        }
    }
    out
}

/// One tool round: what was called, what it cost, and the statement it ran.
///
/// Every figure is the engine's own, exactly as the card shows it — this renders them, it does
/// not compute them.
fn step_line(step: &Step) -> String {
    let mut facts: Vec<String> = Vec::new();
    if let Some(rows) = step.facts.rows {
        facts.push(format!("{rows} rows"));
    }
    if let Some(ms) = step.facts.elapsed_ms {
        facts.push(format!("{ms} ms"));
    }
    if let Some(stopped) = &step.facts.stopped {
        facts.push(stopped.clone());
    } else if step.failed == Some(true) {
        facts.push("failed".into());
    } else if step.failed.is_none() {
        facts.push("did not finish".into());
    }
    let mut line = match facts.is_empty() {
        true => format!("**{}**\n", step.tool),
        false => format!("**{}** — {}\n", step.tool, facts.join(" · ")),
    };
    if let Some(sql) = &step.facts.sql {
        line.push_str(&format!("\n```sql\n{}\n```\n", sql.trim_end()));
    }
    line
}

/// A filename for this conversation: its title, reduced to something every filesystem accepts.
///
/// Titles come from the user's first question, so they hold spaces, punctuation and whatever
/// else was typed. Anything that is not a letter, a digit or a dash becomes a dash, runs
/// collapse, and an empty result falls back to a name rather than to `.md` on its own.
fn file_name(title: &str) -> String {
    let mut name = String::new();
    for ch in title.chars() {
        match ch.is_alphanumeric() {
            true => name.extend(ch.to_lowercase()),
            false if !name.ends_with('-') => name.push('-'),
            false => {}
        }
    }
    let name = name.trim_matches('-');
    let name: String = name.chars().take(60).collect();
    match name.trim_matches('-').is_empty() {
        true => "chat.md".to_string(),
        false => format!("{}.md", name.trim_matches('-')),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use strata_agent::assistant::Facts;

    use super::*;
    use crate::apps::project::state::{Chats, Pick, Reply};

    fn chat_with(blocks: Vec<Block>, note: Option<&str>) -> Chats {
        let mut chats = Chats::new(Pick::default());
        let id = chats.active_id();
        chats.ask(id, "how many orders?".into(), vec!["@orders".into()]);
        let chat = chats.get_mut(id).expect("the chat");
        if let Some(Turn::Reply(reply)) = chat.turns.last_mut() {
            *reply = Reply {
                blocks,
                at: reply.at.clone(),
                note: note.map(str::to_string),
                settled: true,
            };
        }
        chats
    }

    /// The export is the conversation **with its evidence**: prose, the statement, and the
    /// engine's own figures for the call behind it.
    #[test]
    fn an_export_keeps_the_prose_the_sql_and_the_figures() {
        let chats = chat_with(
            vec![
                Block::Prose("Twelve orders.".into()),
                Block::Step(Box::new(Step {
                    call: "c1".into(),
                    tool: "run".into(),
                    arguments: json!({}),
                    failed: Some(false),
                    facts: Facts {
                        sql: Some("SELECT count(*) FROM orders".into()),
                        rows: Some(12),
                        elapsed_ms: Some(7),
                        ..Facts::default()
                    },
                })),
                Block::Offer {
                    sql: "SELECT * FROM orders".into(),
                    stale: false,
                },
            ],
            None,
        );
        let out = markdown(chats.active());

        assert!(out.starts_with("# how many orders?"), "{out}");
        assert!(out.contains("## You · "), "{out}");
        assert!(out.contains("@orders"), "{out}");
        assert!(out.contains("Twelve orders."), "{out}");
        assert!(out.contains("**run** — 12 rows · 7 ms"), "{out}");
        assert!(
            out.contains("```sql\nSELECT count(*) FROM orders\n```"),
            "{out}"
        );
        assert!(out.contains("```sql\nSELECT * FROM orders\n```"), "{out}");
    }

    /// A stopped turn exports as stopped, in the settle's own words — cancelled is never failed,
    /// in a file as much as on screen.
    #[test]
    fn a_stopped_turn_exports_marked_stopped() {
        let chats = chat_with(vec![Block::Prose("Checking".into())], Some("Stopped."));
        let out = markdown(chats.active());
        assert!(out.contains("_Stopped._"), "{out}");
    }

    /// A step that was stopped says so in the engine's wording rather than reading as a failure.
    #[test]
    fn a_stopped_step_is_not_dressed_as_a_failure() {
        let chats = chat_with(
            vec![Block::Step(Box::new(Step {
                call: "c1".into(),
                tool: "run".into(),
                arguments: json!({}),
                failed: Some(false),
                facts: Facts {
                    stopped: Some("the user stopped this run".into()),
                    ..Facts::default()
                },
            }))],
            Some("Stopped."),
        );
        let out = markdown(chats.active());
        assert!(out.contains("**run** — the user stopped this run"), "{out}");
        assert!(!out.contains("failed"), "{out}");
    }

    /// A title is whatever the user typed, and a filename is not.
    #[test]
    fn a_filename_survives_whatever_was_asked() {
        assert_eq!(file_name("How many orders?"), "how-many-orders.md");
        assert_eq!(file_name("a/b\\c:d"), "a-b-c-d.md");
        assert_eq!(file_name("   "), "chat.md");
        assert_eq!(file_name("???"), "chat.md");
        assert!(file_name(&"x".repeat(200)).len() <= 63);
    }
}
