//! `.strata/` project persistence — the **defs** half (P4-13).
//!
//! A project is a folder with a `.strata/` directory inside it: the durable, shareable
//! catalog **definitions** in `project.json` (committed) and the local working session in
//! `session.json` (gitignored; owned by the session-persistence slice, not this module).
//! The defs ([`TableDef`] / [`ViewDef`] / [`SavedQuery`]) are pure — what registration
//! learns about them (columns, status) lives on the UI project store's rows and is
//! re-derived when the engine re-registers a project on open.
//!
//! Paths in `sources` are stored **project-relative** where they sit inside the project
//! folder (portable — the file can be committed and checked out elsewhere), and resolved
//! to absolute against the project folder when handed to the engine / filesystem:
//! [`resolve_source`] / [`relativize`]. A def that names a **connection** (W7 · 04) stores
//! its sources relative to that bucket instead, and [`resolve_source`] is where the two
//! rules meet.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string, to_string_pretty};
use strata_model::{ConnectionDef, HistoryEntry, SavedQuery, SessionSnapshot, TableDef, ViewDef};
use uuid::Uuid;

use crate::util::{
    collapse_sql, sweep_stale_temp_dirs, sweep_stale_temps, write_atomic, TEMP_GLOB,
};

/// The project directory name inside a project folder.
pub const STRATA_DIR: &str = ".strata";
const PROJECT_JSON: &str = "project.json";
const SESSION_JSON: &str = "session.json";
/// Where a `session.json` that read fine and isn't a session is kept aside (see
/// [`corrupt_session_path`] and [`SessionLoadError::Corrupt`] — a file that merely couldn't
/// be *read* is never moved). Named here, beside the session file itself, because two
/// places have to agree on it: the window that moves the file aside, and
/// [`ensure_gitignore`] — a `.gitignore` line matches literally, so `session.json` does
/// **not** cover this name and the kept file would otherwise surface as untracked in a
/// committed project folder.
const SESSION_JSON_CORRUPT: &str = "session.json.corrupt";
const HISTORY_JSONL: &str = "history.jsonl";
/// Where an **internal table**'s data lives, under `.strata/` (ED-04): one directory per table,
/// holding the Arrow IPC files a `CREATE TABLE` spooled. Gitignored as a whole ([`TABLES_GLOB`]),
/// which is the *point*: the def travels with `project.json` and the data does not.
///
/// One name for one layout, because three things have to agree on it — the engine that writes
/// under it, the def whose source path names it ([`internal_source`]), and the `.gitignore` line.
const TABLES_DIR: &str = "tables";
/// The `.gitignore` line covering [`TABLES_DIR`]. A trailing slash, so it ignores the directory
/// rather than a file that happens to be called `tables`.
const TABLES_GLOB: &str = "tables/";
/// Where a **conversation** lives, under `.strata/` (AS-07): one JSON document per chat, named
/// for its own id.
///
/// A directory of documents rather than one file, because a conversation is rewritten every time
/// it grows and a single `chats.json` would make every turn in every chat rewrite every other
/// one. Not `.jsonl` either: a chat is a document with a head and tens of turns, not an unbounded
/// append log.
const CHATS_DIR: &str = "chats";
/// The `.gitignore` line covering [`CHATS_DIR`], and it is not cosmetic: a transcript quotes the
/// user's own data — column names, values, whatever the assistant read back in prose — while
/// `project.json` beside it is a *committed* file, so `.strata/` is a directory people have in
/// their repos.
const CHATS_GLOB: &str = "chats/";

/// The committed definitions — the shape of `.strata/project.json`.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct ProjectDefs {
    #[serde(default)]
    pub name: String,
    /// The remote object stores this project reads from (W7). **Committed with the rest**,
    /// which `docs/CONNECTIONS_SPEC.md` §5 had left open between here and the gitignored
    /// session: a connection carries no secret material at all — a profile *name* and a key
    /// **file path** are references to the reader's own machine, not credentials — so there
    /// is nothing here a colleague may not have, and a catalog whose tables live in a bucket
    /// is not shareable if the bucket isn't.
    #[serde(default)]
    pub connections: Vec<ConnectionDef>,
    #[serde(default)]
    pub tables: Vec<TableDef>,
    #[serde(default)]
    pub views: Vec<ViewDef>,
    #[serde(default)]
    pub saved_queries: Vec<SavedQuery>,
}

/// The `.strata/` dir of the project folder `root`.
pub fn strata_dir(root: &Path) -> PathBuf {
    root.join(STRATA_DIR)
}

/// Where the project folder `root` stores **internal table** data (ED-04) — `.strata/tables`.
/// Absolute, because it is handed to the engine, which resolves nothing.
pub fn tables_dir(root: &Path) -> PathBuf {
    strata_dir(root).join(TABLES_DIR)
}

/// The **project-relative** source path an internal table's def stores for the directory
/// `slug` — `.strata/tables/<slug>/`.
///
/// Relative by construction rather than through [`relativize`], because it is relative by
/// construction: the directory is inside the project folder and always will be. That is what
/// makes the def portable, and it is the whole reason a clone of the project loads the def and
/// then reports honestly that its data is not here.
pub fn internal_source(slug: &str) -> String {
    format!("{STRATA_DIR}/{TABLES_DIR}/{slug}/")
}

/// Whether a project already exists in folder `root` (a `.strata/project.json`).
/// Distinguishes "open existing" from "scaffold new", so a corrupt-but-present file is
/// surfaced as a load error rather than silently overwritten.
pub fn exists_at(root: &Path) -> bool {
    strata_dir(root).join(PROJECT_JSON).exists()
}

/// Load the defs from project folder `root`. `Err` when the file can't be read (missing
/// included — an absent `project.json` means "not a project here", which [`exists_at`]
/// answers before this is called) or doesn't parse. The two are not split the way
/// [`load_session`]'s are, because the caller does the same thing either way: there is no
/// project without its defs, so every `Err` fails the open loud, and nothing here moves,
/// replaces or overwrites the file. Catalog lists come back sorted ([`name_ord`]) — the
/// file's order is just whatever it was last written in.
pub fn load_defs(root: &Path) -> Result<ProjectDefs, String> {
    let path = strata_dir(root).join(PROJECT_JSON);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut defs: ProjectDefs = from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    // **Migrated on the way in**, before anything reads one: an HTTP connection written before
    // its address carried a scheme stored the authority alone, and would now read as a URL with
    // none. This is the one place project defs come off disk, so it is the one place that has to
    // know (`ConnectionDef::migrated` is the rule).
    defs.connections = defs
        .connections
        .into_iter()
        .map(ConnectionDef::migrated)
        .collect();
    // Connections sort on their address, which *is* their name (`ConnectionDef`): the same
    // ordering rule, over the field that carries identity here.
    defs.connections
        .sort_by(|a, b| name_ord(&a.address, &b.address));
    defs.tables.sort_by(|a, b| name_ord(&a.name, &b.name));
    defs.views.sort_by(|a, b| name_ord(&a.name, &b.name));
    defs.saved_queries
        .sort_by(|a, b| name_ord(&a.name, &b.name));
    Ok(defs)
}

/// Write the defs into `root`'s `.strata/` dir, creating it and tidying it
/// ([`tidy_strata_dir`] — `.gitignore` + stale temps) if needed. Written **atomically**
/// ([`write_atomic`]): this is the user's whole catalog, so a crash mid-save must leave the
/// last good file rather than a truncated one.
pub fn save_defs(root: &Path, defs: &ProjectDefs) -> Result<(), String> {
    let dir = strata_dir(root);
    // Every arm names its path, like the loads: these strings reach the load-fault dialog
    // (a scaffold that fails is a failed open), where a bare OS error names no file.
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    tidy_strata_dir(&dir);
    let path = dir.join(PROJECT_JSON);
    let json = to_string_pretty(defs).map_err(|e| format!("{}: {e}", path.display()))?;
    write_atomic(&path, json.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Scaffold a **new** project in folder `root`: an empty defs file named after the
/// folder. Refuses to touch an existing project (see [`exists_at`]).
pub fn scaffold(root: &Path) -> Result<ProjectDefs, String> {
    if exists_at(root) {
        return Err(format!("{}: project already exists", root.display()));
    }
    let name = root
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "untitled".into());
    let defs = ProjectDefs {
        name,
        ..Default::default()
    };
    save_defs(root, &defs)?;
    Ok(defs)
}

/// The `.strata/session.json` of the project folder `root` — the **local** working
/// session (open tabs + arrangement), gitignored and owned by the session-persistence
/// slice (P4-14). Separate from the shared, committed [`project.json`](PROJECT_JSON).
pub fn session_path(root: &Path) -> PathBuf {
    strata_dir(root).join(SESSION_JSON)
}

/// The `.strata/session.json.corrupt` of `root` — where a session file that **was read and
/// is not a session** ([`SessionLoadError::Corrupt`]) is moved aside on restore, so the
/// autosave that follows the open can't overwrite the only copy of the user's tabs. One
/// fixed name, not a numbered series: the *current* corruption is the one worth keeping.
/// Gitignored like the session itself ([`ensure_gitignore`]).
///
/// Only for damaged data. A session file that merely *couldn't be read*
/// ([`SessionLoadError::Unreadable`]) must be left exactly where it is — its contents are
/// unknown and probably fine, and setting it aside would turn a transient IO failure into
/// permanent tab loss.
pub fn corrupt_session_path(root: &Path) -> PathBuf {
    strata_dir(root).join(SESSION_JSON_CORRUPT)
}

/// Why a project's `session.json` didn't load. A type rather than a message because the
/// caller must **act** differently on the two: one of them licenses setting the file aside
/// and overwriting it, and the other forbids it.
#[derive(Debug)]
pub enum SessionLoadError {
    /// **Damaged data.** The bytes were read and are not a [`SessionSnapshot`] (not UTF-8,
    /// or not the JSON we write). Re-reading can only produce the same answer, so the file
    /// is a write-off: the caller may move it aside ([`corrupt_session_path`]) and open a
    /// blank session, and the autosave that follows overwrites nothing of value.
    Corrupt(String),
    /// **A failed read.** Permission denied, an IO error, a network mount that has gone
    /// away, a file whose data hasn't landed yet on a synced volume. The contents are
    /// unknown and very probably intact, so the caller must **not** move, replace or
    /// overwrite the file — opening a blank session is fine, *persisting* that blank
    /// session over the file is how a transient failure becomes permanent tab loss.
    Unreadable(String),
}

impl fmt::Display for SessionLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Corrupt(m) => write!(f, "corrupt: {m}"),
            Self::Unreadable(m) => write!(f, "unreadable: {m}"),
        }
    }
}

/// Load the [`SessionSnapshot`] from `root`'s `.strata/`. Three outcomes, and the caller is
/// expected to treat all three differently:
///
/// * `Ok(None)` — **no session file.** A fresh or never-saved project: a first-class,
///   expected state, not an error. Open blank and autosave normally.
/// * `Err(`[`SessionLoadError::Corrupt`]`)` — the file is there and isn't a session. Log
///   it, set it aside ([`corrupt_session_path`]) and open blank, rather than bricking the
///   window on a file a kill mid-autosave could have produced.
/// * `Err(`[`SessionLoadError::Unreadable`]`)` — the file couldn't be read at all. Log it
///   and leave the file exactly where it is; see the variant for what the caller owes it.
///
/// The split is drawn on the **bytes**, not on an `io::ErrorKind`: a read that fails is
/// `Unreadable`, and bytes that aren't UTF-8 or aren't a snapshot are `Corrupt`. That is
/// why this reads with [`fs::read`] and decodes itself — [`fs::read_to_string`] folds a
/// decode failure into the same `io::Error` as a disk failure, which is exactly the
/// conflation this type exists to undo.
///
/// Concrete over the model type, exactly like [`load_defs`].
pub fn load_session(root: &Path) -> Result<Option<SessionSnapshot>, SessionLoadError> {
    let path = session_path(root);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(SessionLoadError::Unreadable(format!(
                "{}: {e}",
                path.display()
            )))
        }
    };
    // Past the read, everything is the file's fault, not the filesystem's.
    let text = String::from_utf8(bytes)
        .map_err(|e| SessionLoadError::Corrupt(format!("{}: {e}", path.display())))?;
    from_str(&text)
        .map(Some)
        .map_err(|e| SessionLoadError::Corrupt(format!("{}: {e}", path.display())))
}

/// Write the [`SessionSnapshot`] into `root`'s `.strata/` (gitignored), creating and
/// tidying the dir ([`tidy_strata_dir`]) if needed. The autosave side effect's sink
/// (P4-14) — it fires shortly after every edit, so the write is **atomic**
/// ([`write_atomic`]): a kill or power
/// loss lands on one of those writes eventually, and a truncated `session.json` would cost
/// the user their open tabs.
pub fn save_session(root: &Path, snapshot: &SessionSnapshot) -> Result<(), String> {
    let dir = strata_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    tidy_strata_dir(&dir);
    let json = to_string_pretty(snapshot).map_err(|e| e.to_string())?;
    let path = session_path(root);
    write_atomic(&path, json.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// The `.strata/history.jsonl` of the project folder `root` — the **local** append-only
/// query-history log (gitignored, per-user), separate from the committed defs.
pub fn history_path(root: &Path) -> PathBuf {
    strata_dir(root).join(HISTORY_JSONL)
}

/// Append one history entry as a JSON line to `root`'s `history.jsonl` (creating and
/// tidying the dir — [`tidy_strata_dir`] — if needed). Append-only (`DESIGN_SPEC` §"History as `.jsonl`") so a completed
/// run is one cheap `O_APPEND` write, not a whole-file rewrite; [`load_history`] bounds the
/// file back down.
///
/// This is the write for a run of a query the log does **not** already hold. A re-run has to move
/// an entry, which an append cannot do — that one goes through [`save_history`].
pub fn append_history(root: &Path, entry: &HistoryEntry) -> Result<(), String> {
    let dir = strata_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    tidy_strata_dir(&dir);
    let mut line = to_string(entry).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path(root))
        .map_err(|e| e.to_string())?;
    file.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

/// Replace `root`'s history log with `entries` (**file order**, oldest → newest) — the write a
/// re-run needs, where [`append_history`] is the write a new query needs.
///
/// Re-running a query moves its entry rather than adding one, so the line the log already holds
/// for it is now stale and no append can take it back. Rewriting is the only way to keep the file
/// free of entries the app considers impossible, and it is bounded work: the caller passes the
/// capped in-memory list, not a file it has to read first.
pub fn save_history(root: &Path, entries: &[HistoryEntry]) -> Result<(), String> {
    let dir = strata_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    tidy_strata_dir(&dir);
    let mut out = String::new();
    for entry in entries {
        out.push_str(&to_string(entry).map_err(|e| e.to_string())?);
        out.push('\n');
    }
    let path = history_path(root);
    write_atomic(&path, out.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Load history for `root`: the newest `cap` **distinct** queries, in file order (oldest →
/// newest). Absent file → empty (a project with no runs yet); a corrupt *line* is skipped, not
/// fatal — one bad append can't lose the whole log.
///
/// **Distinct, not merely last-N.** The log is append-only, so re-running one query writes a
/// line every time; a plain keep-last-N would then hand back a window of the same statement
/// repeated, and the cap — the user's `max_history`, "how many recent queries to keep" — would
/// silently mean something else. So repeats collapse to their **newest** occurrence
/// ([`collapse_sql`] is the key, shared with the History drawer's preview so two kept entries can
/// never render identically), and the cap counts what is left.
///
/// Compaction rides the same path: whenever anything was dropped — a duplicate, an overflowing
/// entry or a corrupt line — the file is **rewritten** to exactly what was kept (`DESIGN_SPEC`:
/// "rotate to bound size"), which is what stops an append-only log of one repeated query growing
/// without bound.
///
/// The rewrite goes through [`write_atomic`], so it is all-or-nothing: a crash mid-rotation
/// leaves the un-rotated log, never a truncated one. It is **not** locked against other writers,
/// and that residual race is deliberate: every other writer opens the log `O_APPEND`
/// ([`append_history`]), so an entry another window appends between this read and the rename
/// lands in the file we then replace (now unlinked) and is lost. Bounded by design — only the
/// runs completed in that millisecond, and only from history, which is regenerable. The
/// alternative is a lock file every append has to take, which is not worth it here.
pub fn load_history(root: &Path, cap: usize) -> Result<Vec<HistoryEntry>, String> {
    let path = history_path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let lines = text.lines().filter(|l| !l.trim().is_empty()).count();
    let parsed: Vec<HistoryEntry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| from_str(l).ok())
        .collect();
    let entries = newest_distinct(parsed, cap);
    if entries.len() != lines {
        // Best-effort: a rewrite that fails just leaves the longer log, which the next load
        // retries.
        let mut out = String::new();
        for entry in &entries {
            if let Ok(line) = to_string(entry) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        let _ = write_atomic(&path, out.as_bytes());
    }
    Ok(entries)
}

/// The newest `cap` entries with distinct SQL, back in oldest → newest order.
///
/// Walked from the newest backwards, so the occurrence that survives a repeat is the **most
/// recent** one — the run whose figures and timestamp are the ones worth keeping, and the
/// position the drawer should show it at.
fn newest_distinct(entries: Vec<HistoryEntry>, cap: usize) -> Vec<HistoryEntry> {
    let mut seen = HashSet::new();
    let mut kept: Vec<HistoryEntry> = Vec::new();
    for entry in entries.into_iter().rev() {
        if kept.len() >= cap {
            break;
        }
        if seen.insert(collapse_sql(&entry.sql)) {
            kept.push(entry);
        }
    }
    kept.reverse();
    kept
}

/// Discard `root`'s query history — the History drawer's **Clear** (P3-14).
///
/// The log is *removed* rather than truncated to zero bytes: an absent file is already how
/// [`load_history`] spells "no runs yet" (a project that has never run one), so removing it is
/// the same state by the same path, and the next [`append_history`] recreates it. An absent
/// file is therefore success, not an error — Clear is idempotent.
pub fn clear_history(root: &Path) -> Result<(), String> {
    let path = history_path(root);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The `.strata/chats/` of the project folder `root` — where AS-07's conversations live.
///
/// A conversation belongs to a **project**, not to the app: it refers to that project's tables,
/// its tabs and its results, and means nothing beside a different one.
pub fn chats_dir(root: &Path) -> PathBuf {
    strata_dir(root).join(CHATS_DIR)
}

/// Where conversation `id` is stored. The id is a [`Uuid`] rather than a name, so a filename
/// cannot be anything but a filename — there is no user text on this path to escape.
pub fn chat_path(root: &Path, id: &Uuid) -> PathBuf {
    chats_dir(root).join(format!("{id}.json"))
}

/// Write one conversation document, whole and atomically.
///
/// [`write_atomic`] for the same reason [`save_session`] uses it: this fires at every turn
/// boundary, so a kill lands on one of these writes eventually, and a truncated document would
/// cost the user the conversation rather than the turn.
pub fn save_chat(root: &Path, id: &Uuid, json: &str) -> Result<(), String> {
    let dir = strata_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    tidy_strata_dir(&dir);
    let chats = chats_dir(root);
    fs::create_dir_all(&chats).map_err(|e| e.to_string())?;
    let path = chat_path(root, id);
    write_atomic(&path, json.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))
}

/// Every stored conversation document, in no particular order — the caller sorts by what it
/// reads out of the heads.
///
/// An absent directory is **empty, not an error**: it is how a project that has never held a
/// conversation spells itself, exactly as an absent `history.jsonl` spells "no runs yet". A
/// [`write_atomic`] temp is skipped by extension, so a load racing a write never tries to parse
/// a half-written document.
pub fn chat_files(root: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = chats_dir(root);
    let entries = match fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    Ok(entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .filter(|path| {
            // `.{name}.{pid}.{seq}.tmp` ends in `.tmp`, but a temp written *for* a chat document
            // is named after it, so filter on the leading dot the temp name carries.
            !path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with('.'))
        })
        .collect())
}

/// Forget one conversation — the switcher's per-row delete.
///
/// An absent file is success: the row and the file are two records of one thing, and a delete
/// that has already happened is the state the caller asked for.
pub fn delete_chat(root: &Path, id: &Uuid) -> Result<(), String> {
    let path = chat_path(root, id);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Discard every stored conversation in `root` — the chat pane's **Clear conversations**.
///
/// The documents are removed one by one rather than the directory as a whole, so a file this
/// build could not parse is still cleared and nothing outside the store is touched. Idempotent
/// on the same terms as [`clear_history`]: an absent directory is already "no conversations".
pub fn clear_chats(root: &Path) -> Result<(), String> {
    for path in chat_files(root)? {
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Ok(())
}

/// The housekeeping every durable `.strata/` write does on its way in: keep the
/// `.gitignore` current, and clear out any [`write_atomic`] temp a killed process left
/// behind ([`sweep_stale_temps`] decides which of those are safe to touch), in `.strata/` and in
/// each subdirectory that is written the same way. Both are best-effort and neither can fail the
/// save that called it.
///
/// It rides the write path rather than the project open so it is self-contained — the dir
/// exists by this point, and a `read_dir` of a five-entry directory is nothing beside the
/// `fsync` the write is about to do.
fn tidy_strata_dir(dir: &Path) {
    ensure_gitignore(dir);
    sweep_stale_temps(dir);
    // The same housekeeping one level down, for the other thing published by rename: a CTAS
    // that was killed between spooling its Arrow files and renaming the directory into place
    // leaves a `.tmp-…` under `tables/` (ED-04). Cheap — a `read_dir` of a directory holding
    // one entry per internal table — and skipped entirely on the usual project, which has none.
    sweep_stale_temp_dirs(&dir.join(TABLES_DIR));
    // And the other subdirectory published by rename: a conversation's document is written with
    // `write_atomic`, whose temp lands *beside the target* — inside `chats/`, which the sweep of
    // `.strata/` itself never reaches. One per interrupted write, and nothing else would ever
    // remove them.
    sweep_stale_temps(&dir.join(CHATS_DIR));
}

/// Ensure `.strata/.gitignore` ignores the local, per-user files — the working session,
/// the copy kept aside when that session won't parse, the query-history log, the internal
/// tables' data ([`TABLES_GLOB`]), the saved conversations ([`CHATS_GLOB`]) and the in-flight
/// temp of any [`write_atomic`] ([`TEMP_GLOB`]) — adding any that are missing while preserving
/// other lines. Run from every local-file write, so an older `.gitignore` (session-only) gets
/// upgraded, which is what lets a new entry reach existing projects with no migration.
///
/// `tables/` is the one entry that is not merely per-user noise: it is the design (ED-04). An
/// internal table's **def** is committed like every other, and its data is not, so a colleague
/// who clones the project gets the row and an honest "no data in this copy" against it.
///
/// The names are literal but [`TEMP_GLOB`] is a pattern, and both are taken from the one
/// place that defines them, because a gitignore line matches literally: `session.json` does
/// not cover `session.json.corrupt`, and nothing here covers a temp whose name carries the
/// writer's pid.
///
/// Rewritten atomically like the rest: it's a read-modify-write of a file the user may have
/// added their own lines to, so a truncating write could lose them.
fn ensure_gitignore(dir: &Path) {
    let gi = dir.join(".gitignore");
    let existing = fs::read_to_string(&gi).unwrap_or_default();
    let mut lines: Vec<&str> = existing.lines().collect();
    let mut changed = false;
    for wanted in [
        SESSION_JSON,
        SESSION_JSON_CORRUPT,
        HISTORY_JSONL,
        TABLES_GLOB,
        CHATS_GLOB,
        TEMP_GLOB,
    ] {
        if !lines.iter().any(|l| l.trim() == wanted) {
            lines.push(wanted);
            changed = true;
        }
    }
    if changed {
        let mut out = lines.join("\n");
        out.push('\n');
        let _ = write_atomic(&gi, out.as_bytes());
    }
}

/// Resolve one of a table def's sources to what the engine reads: composed onto its
/// **connection's** bucket when the def names one ([`TableDef::connection`]), and otherwise
/// joined onto `root` (the project folder) where it is relative.
///
/// **One function, taking the connection**, rather than a local rule with a remote one beside
/// it. The two answers are mutually exclusive and every caller has the def in hand, so a
/// resolver that could not see the connection would silently give the wrong one: `s3://` is not
/// an absolute *path*, so a bucket-relative source handed to the local rule comes back as
/// `<project>/events/2024`, which registers as a missing folder on the user's own disk.
///
/// `connection` is a [`ConnectionDef::url`](strata_model::ConnectionDef::url) — scheme and
/// authority — so the composition is that URL, a separator, and the source. Both sides are
/// trimmed of the separator first: a bucket URL never carries one and a path typed with a
/// leading `/` means the bucket root, not an empty first segment.
pub fn resolve_source(root: &Path, connection: Option<&str>, p: &str) -> String {
    let Some(url) = connection else {
        let path = Path::new(p);
        if path.is_absolute() {
            return p.to_string();
        }
        return root.join(p).to_string_lossy().into_owned();
    };
    format!(
        "{}/{}",
        url.trim_end_matches('/'),
        p.trim_start_matches('/')
    )
}

/// Split a location that names an object store into `(connection URL, bucket-relative source)` —
/// [`resolve_source`]'s remote arm read backwards, for the one caller that arrives with the
/// composed string rather than with the two halves: a typed
/// `CREATE EXTERNAL TABLE … LOCATION 's3://acme-lake/events/2024/'` (ED-10).
///
/// `None` for a location with no scheme, which is the local rule's — a path, relative to the
/// project folder or absolute. **Not a guess about intent**: the Configure window's LOCATION
/// toggle is an explicit choice precisely because a *typed path* must never be re-read as remote,
/// and this is the other case, where the scheme is the only thing the statement says about where
/// the files are. A caller still has to check the URL against the project's own connections; this
/// answers what the location was written as, not whether it can be read.
///
/// Kept beside [`resolve_source`] so the composition rule has one home in both directions — a
/// round-trip is asserted in this module's tests. The split is at the first `/` after the scheme,
/// which is exactly where [`ConnectionDef::url`](strata_model::ConnectionDef::url) stops.
pub fn split_remote(location: &str) -> Option<(String, String)> {
    let (scheme, rest) = location.split_once("://")?;
    if scheme.is_empty() {
        return None;
    }
    let (authority, path) = match rest.split_once('/') {
        Some((authority, path)) => (authority, path),
        // A bucket with nothing under it. Answered rather than refused, because "the location
        // names no path inside the bucket" is the caller's sentence to write, not a parse failure.
        None => (rest, ""),
    };
    Some((format!("{scheme}://{authority}"), path.to_string()))
}

/// If `abs` sits inside `root`, return it relative to `root` (portable, stored in
/// `project.json`); otherwise keep it absolute.
pub fn relativize(root: &Path, abs: &str) -> String {
    if let Ok(rel) = Path::new(abs).strip_prefix(root) {
        let r = rel.to_string_lossy();
        if !r.is_empty() {
            return r.into_owned();
        }
    }
    abs.to_string()
}

/// Case-insensitive alphabetical ordering for catalog names — how tables, views and
/// saved queries are presented. Kept sorted at the mutation points (not at render), so
/// index-addressed rows can't desync and an upsert can't shuffle rows under the user.
pub fn name_ord(a: &str, b: &str) -> Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
        // Names differing only in case still need a total order.
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::process;
    use std::time::{Duration, SystemTime};
    use strata_model::{ChartConfig, Layout, Origin, ResultsView, TabId, TabSnapshot, WindowGeom};

    /// A fresh temp project folder, cleaned up on drop.
    struct TempRoot(PathBuf);
    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = env::temp_dir().join(format!("strata-project-test-{tag}-{}", process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn scaffold_then_load_round_trips() {
        let root = TempRoot::new("scaffold");
        assert!(!exists_at(&root.0));
        let defs = scaffold(&root.0).unwrap();
        assert!(exists_at(&root.0));
        assert!(defs.name.starts_with("strata-project-test-scaffold"));
        // Scaffolding is refused where a project already exists.
        assert!(scaffold(&root.0).is_err());
        // The local, per-user files are gitignored from the start — including the copy a
        // failed session restore keeps aside and the in-flight temp of an atomic write,
        // neither of which the `session.json` line covers.
        let gi = fs::read_to_string(strata_dir(&root.0).join(".gitignore")).unwrap();
        assert_eq!(
            gi,
            "session.json\nsession.json.corrupt\nhistory.jsonl\ntables/\nchats/\n.*.tmp\n"
        );
        // `assert!` over `assert_eq!` here and below: the model types are serde
        // vocabulary and deliberately don't derive `Debug`.
        let loaded = load_defs(&root.0).unwrap();
        assert!(loaded == defs);
    }

    /// **A `project.json` written before an HTTP address carried its scheme still opens.** The
    /// file below is exactly the older shape — `bucket`, and the authority alone — and this is the
    /// one path every project comes off disk through, so it is where the migration has to happen.
    /// Without it the connection loads as a URL with no scheme and is refused on the next open.
    #[test]
    fn an_older_http_connection_is_migrated_on_load() {
        let root = TempRoot::new("http-migrate");
        fs::create_dir_all(strata_dir(&root.0)).unwrap();
        fs::write(
            strata_dir(&root.0).join(PROJECT_JSON),
            r#"{"name":"old","connections":[
                 {"bucket":"example.com:8080","provider":{"provider":"http"}},
                 {"bucket":"acme-lake","provider":{"provider":"s3","region":"eu-west-2"}}
               ],"tables":[],"views":[],"saved_queries":[]}"#,
        )
        .unwrap();

        let defs = load_defs(&root.0).unwrap();
        let urls: Vec<String> = defs.connections.iter().map(ConnectionDef::url).collect();
        assert_eq!(urls, ["s3://acme-lake", "https://example.com:8080"]);
        // Every one of them is an address its provider will still accept, which is the whole
        // point: the migration exists so an old file does not become an amber row.
        for conn in &defs.connections {
            assert!(conn.provider.check_address(&conn.address).is_ok());
        }
    }

    #[test]
    fn save_load_round_trips_defs_sorted() {
        let root = TempRoot::new("roundtrip");
        let mut defs = ProjectDefs {
            name: "p".into(),
            ..Default::default()
        };
        for name in ["zeta", "Alpha", "midge"] {
            defs.views.push(ViewDef {
                name: name.into(),
                sql: format!("SELECT '{name}'"),
            });
        }
        defs.saved_queries.push(SavedQuery {
            id: Uuid::new_v4(),
            name: "q".into(),
            sql: "select 1".into(),
            meta: "—".into(),
        });
        save_defs(&root.0, &defs).unwrap();
        let loaded = load_defs(&root.0).unwrap();
        let names: Vec<&str> = loaded.views.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["Alpha", "midge", "zeta"]);
        // Ids round-trip — a saved query keeps its identity across save/load.
        assert!(loaded.saved_queries == defs.saved_queries);
    }

    #[test]
    fn saved_queries_without_ids_get_one_minted_on_load() {
        let root = TempRoot::new("legacy-ids");
        let dir = strata_dir(&root.0);
        fs::create_dir_all(&dir).unwrap();
        // A pre-id file, as the old app wrote it.
        fs::write(
            dir.join("project.json"),
            r#"{ "name": "p", "saved_queries": [{ "name": "q", "sql": "select 1", "meta": "—" }] }"#,
        )
        .unwrap();
        let loaded = load_defs(&root.0).unwrap();
        assert_eq!(loaded.saved_queries.len(), 1);
        // Minted per load until saved; saving pins it.
        save_defs(&root.0, &loaded).unwrap();
        let again = load_defs(&root.0).unwrap();
        assert!(again.saved_queries[0].id == loaded.saved_queries[0].id);
    }

    #[test]
    fn missing_project_is_a_load_error() {
        let root = TempRoot::new("missing");
        assert!(load_defs(&root.0).is_err());
    }

    /// One tab for a session snapshot.
    fn tab(name: &str, text: &str) -> TabSnapshot {
        TabSnapshot {
            id: TabId::new(),
            name: name.into(),
            origin: Origin::Scratch,
            text: text.into(),
            view: ResultsView::Grid,
            chart: ChartConfig::default(),
        }
    }

    #[test]
    fn session_round_trips_and_absence_is_none_not_error() {
        let root = TempRoot::new("session");
        // No file yet → Ok(None), a first-class state (a fresh / never-saved project).
        assert!(load_session(&root.0).unwrap().is_none());

        let t = tab("query 1", "SELECT 1");
        let id = t.id;
        let snap = SessionSnapshot {
            tabs: vec![t, tab("events", "SELECT 2")],
            active: Some(id),
            window: Some(WindowGeom {
                x: 10.0,
                y: 20.0,
                width: 800.0,
                height: 600.0,
            }),
            layout: Layout::default(),
        };
        save_session(&root.0, &snap).unwrap();
        // (`SessionSnapshot` is serde vocabulary and doesn't derive `PartialEq` — check fields.)
        let loaded = load_session(&root.0).unwrap().unwrap();
        assert_eq!(loaded.tabs.len(), 2);
        assert_eq!(loaded.tabs[0].text, "SELECT 1");
        assert_eq!(loaded.active, Some(id));
        assert_eq!(loaded.window.unwrap().width, 800.0);
        // The session file is gitignored the moment it's written (alongside its
        // kept-aside corrupt copy, history, and any stranded write temp).
        let gi = fs::read_to_string(strata_dir(&root.0).join(".gitignore")).unwrap();
        assert_eq!(
            gi,
            "session.json\nsession.json.corrupt\nhistory.jsonl\ntables/\nchats/\n.*.tmp\n"
        );
    }

    /// **A session file written before a per-tab facet existed still loads**, with that facet
    /// at its default. Every one of them is `#[serde(default)]` for this reason, and the file
    /// on a user's disk is older than the code reading it after *every* release — so the case
    /// is the normal one, not an edge.
    ///
    /// Written as literal JSON rather than by serializing an older struct, because the point is
    /// what is *on disk*: a round-trip through today's types could not reproduce a file missing
    /// today's fields.
    #[test]
    fn a_session_file_predating_a_tab_field_loads_with_its_default() {
        let root = TempRoot::new("session-old");
        let id = TabId::new();
        // No `view`, no `chart`, no `layout`, no `window` — the shape before P2-07 and Rz2.
        let text = format!(
            r#"{{"tabs":[{{"id":"{}","name":"query 1","origin":"Scratch","text":"SELECT 1"}}],"active":"{}"}}"#,
            id.0, id.0
        );
        fs::create_dir_all(strata_dir(&root.0)).unwrap();
        fs::write(session_path(&root.0), text).unwrap();

        let loaded = load_session(&root.0)
            .unwrap()
            .expect("an old file still loads");
        assert_eq!(loaded.tabs.len(), 1);
        assert_eq!(loaded.tabs[0].text, "SELECT 1");
        assert_eq!(loaded.tabs[0].view, ResultsView::Grid);
        assert_eq!(loaded.tabs[0].chart, ChartConfig::default());
        assert_eq!(loaded.active, Some(id));
    }

    /// One history entry (timestamps irrelevant to the file-ordering tests).
    fn run(sql: &str, rows: u64) -> HistoryEntry {
        HistoryEntry {
            sql: sql.into(),
            ts_ms: 0,
            elapsed_ms: 0,
            rows,
        }
    }

    #[test]
    fn history_appends_and_loads_in_file_order() {
        let root = TempRoot::new("history");
        // Absent → empty, not an error.
        assert!(load_history(&root.0, 100).unwrap().is_empty());

        for i in 0..3 {
            append_history(&root.0, &run(&format!("SELECT {i}"), i)).unwrap();
        }
        let loaded = load_history(&root.0, 100).unwrap();
        let sqls: Vec<&str> = loaded.iter().map(|r| r.sql.as_str()).collect();
        assert_eq!(
            sqls,
            ["SELECT 0", "SELECT 1", "SELECT 2"],
            "oldest → newest"
        );
        // history.jsonl is gitignored alongside the session (upgraded in place).
        let gi = fs::read_to_string(strata_dir(&root.0).join(".gitignore")).unwrap();
        assert!(gi.lines().any(|l| l == "history.jsonl"));
    }

    #[test]
    fn history_rotates_to_cap_on_load_and_skips_corrupt_lines() {
        let root = TempRoot::new("history-rotate");
        for i in 0..10 {
            append_history(&root.0, &run(&format!("q{i}"), i)).unwrap();
        }
        // A garbage line mid-file must be skipped, not abort the whole load.
        let path = history_path(&root.0);
        let mut text = fs::read_to_string(&path).unwrap();
        text.push_str("{ not json\n");
        fs::write(&path, text).unwrap();

        let loaded = load_history(&root.0, 3).unwrap();
        let sqls: Vec<&str> = loaded.iter().map(|r| r.sql.as_str()).collect();
        assert_eq!(
            sqls,
            ["q7", "q8", "q9"],
            "keeps the last `cap` valid entries"
        );
        // Rotation rewrote the file down to the kept window (and dropped the garbage line).
        let after = load_history(&root.0, 100).unwrap();
        assert_eq!(after.len(), 3);
    }

    /// **Repeats collapse to their newest occurrence, and the cap counts what is left.** The
    /// point of doing it in this order: a log full of one hammered query must not hand back a
    /// window of that query repeated, with every other query aged out of the cap.
    #[test]
    fn history_keeps_the_newest_of_each_query_and_caps_the_distinct_ones() {
        let root = TempRoot::new("history-distinct");
        append_history(&root.0, &run("SELECT a", 1)).unwrap();
        append_history(&root.0, &run("SELECT b", 2)).unwrap();
        for i in 0..20 {
            append_history(&root.0, &run("SELECT * FROM events", 100 + i)).unwrap();
        }

        let loaded = load_history(&root.0, 3).unwrap();
        let sqls: Vec<&str> = loaded.iter().map(|r| r.sql.as_str()).collect();
        assert_eq!(
            sqls,
            ["SELECT a", "SELECT b", "SELECT * FROM events"],
            "the hammered query takes one slot, not all three"
        );
        assert_eq!(
            loaded.last().unwrap().rows,
            119,
            "and it is the newest run of it that survived"
        );

        // The load compacted the file to exactly what it kept, so an append-only log of one
        // repeated query can't grow without bound.
        let text = fs::read_to_string(history_path(&root.0)).unwrap();
        assert_eq!(text.lines().filter(|l| !l.trim().is_empty()).count(), 3);
    }

    /// Layout is not identity: the same statement re-indented is one entry, so the drawer can
    /// never show two rows whose collapsed previews read identically.
    #[test]
    fn history_dedupe_ignores_layout() {
        let root = TempRoot::new("history-layout");
        append_history(&root.0, &run("SELECT a,\n       b\nFROM t", 1)).unwrap();
        append_history(&root.0, &run("SELECT a, b FROM t", 2)).unwrap();

        let loaded = load_history(&root.0, 100).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].rows, 2, "the newest layout is the one kept");
    }

    /// **We write LF and read either.** Both writers terminate with a literal `'\n'` — never
    /// the platform's ending — because this is a data format, not a document: a log written on
    /// one machine and read on another must not depend on which wrote it, and the file is
    /// gitignored, so git's own translation never sees it.
    ///
    /// Reading is the tolerant half. `str::lines()` splits on `\n` *and* strips a trailing
    /// `\r`, so a log some other tool has converted to CRLF still parses — pinned here because
    /// it is the whole reason the strict writer is safe, and it is invisible at the call site.
    #[test]
    fn history_writes_lf_and_reads_crlf() {
        let root = TempRoot::new("history-endings");
        append_history(&root.0, &run("SELECT a", 1)).unwrap();
        append_history(&root.0, &run("SELECT b", 2)).unwrap();

        let path = history_path(&root.0);
        let text = fs::read_to_string(&path).unwrap();
        assert!(!text.contains('\r'), "the writer must emit LF, never CRLF");

        // Now hand it the CRLF version some other tool might leave behind.
        fs::write(&path, text.replace('\n', "\r\n")).unwrap();
        let loaded = load_history(&root.0, 100).unwrap();
        let sqls: Vec<&str> = loaded.iter().map(|r| r.sql.as_str()).collect();
        assert_eq!(sqls, ["SELECT a", "SELECT b"], "CRLF must still parse");
    }

    /// `save_history` is the write a re-run needs — it replaces the log rather than adding to
    /// it, so the entry that moved leaves no stale line behind.
    #[test]
    fn saving_history_replaces_the_log() {
        let root = TempRoot::new("history-save");
        append_history(&root.0, &run("SELECT a", 1)).unwrap();
        append_history(&root.0, &run("SELECT b", 2)).unwrap();

        save_history(&root.0, &[run("SELECT b", 2), run("SELECT a", 3)]).unwrap();

        let loaded = load_history(&root.0, 100).unwrap();
        let sqls: Vec<&str> = loaded.iter().map(|r| r.sql.as_str()).collect();
        assert_eq!(
            sqls,
            ["SELECT b", "SELECT a"],
            "order is the file's, oldest first"
        );
        assert_eq!(loaded[1].rows, 3, "and the rewritten entry won");
    }

    /// Clear discards the log, and is idempotent — clearing an already-clear project (or one
    /// that has never run a query) is success, not an error, because "no file" is what the
    /// loader already reads as "no history".
    #[test]
    fn clearing_history_removes_the_log_and_is_idempotent() {
        let root = TempRoot::new("history-clear");
        append_history(&root.0, &run("SELECT 1", 1)).unwrap();
        assert_eq!(load_history(&root.0, 100).unwrap().len(), 1);

        clear_history(&root.0).unwrap();
        assert!(load_history(&root.0, 100).unwrap().is_empty());
        assert!(!history_path(&root.0).exists());
        clear_history(&root.0).unwrap();

        // …and the log comes back on the next run, rather than the project being left unable
        // to record one.
        append_history(&root.0, &run("SELECT 2", 1)).unwrap();
        assert_eq!(load_history(&root.0, 100).unwrap().len(), 1);
    }

    /// A conversation is one document under `.strata/chats/`, and the directory is gitignored
    /// the moment the first one is written — a transcript quotes the user's own data, and
    /// `.strata/` is a directory people have in their repos.
    #[test]
    fn a_chat_is_one_document_and_the_directory_is_gitignored() {
        let root = TempRoot::new("chats");
        // Absent → no conversations, not an error.
        assert!(chat_files(&root.0).unwrap().is_empty());

        let one = Uuid::new_v4();
        let two = Uuid::new_v4();
        save_chat(&root.0, &one, "{\"version\":1}").unwrap();
        save_chat(&root.0, &two, "{\"version\":1}").unwrap();

        let mut found: Vec<String> = chat_files(&root.0)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        found.sort();
        let mut want = vec![format!("{one}.json"), format!("{two}.json")];
        want.sort();
        assert_eq!(found, want);
        assert_eq!(
            fs::read_to_string(chat_path(&root.0, &one)).unwrap(),
            "{\"version\":1}"
        );

        let gi = fs::read_to_string(strata_dir(&root.0).join(".gitignore")).unwrap();
        assert!(gi.lines().any(|l| l == "chats/"), "{gi}");
    }

    /// A killed write leaves its temp *inside* `chats/`, so the sweep has to reach in there — the
    /// `.strata/` pass never sees it, and nothing else would ever remove it.
    ///
    /// Back-dated past the staleness threshold, because that threshold is the whole safety rule:
    /// a temp from another pid that is only seconds old may still be a write in flight.
    #[test]
    fn a_stranded_chat_temp_is_swept_by_the_next_write() {
        let root = TempRoot::new("chats-temp");
        let one = Uuid::new_v4();
        save_chat(&root.0, &one, "{}").unwrap();
        // A temp named the way `write_atomic` names one, from a pid that is not ours.
        let stranded = chats_dir(&root.0).join(format!(".{one}.json.1.0.tmp"));
        fs::write(&stranded, b"half a document").unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 60 * 60);
        fs::File::options()
            .write(true)
            .open(&stranded)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(stranded.exists());

        save_chat(&root.0, &Uuid::new_v4(), "{}").unwrap();
        assert!(!stranded.exists(), "the temp was not swept");
        // …and the real documents are untouched.
        assert_eq!(chat_files(&root.0).unwrap().len(), 2);
    }

    /// Both ways a conversation is forgotten are idempotent, because a row and its file are two
    /// records of one thing: a delete that already happened is the state the caller asked for.
    #[test]
    fn forgetting_a_chat_is_idempotent_one_at_a_time_or_all_at_once() {
        let root = TempRoot::new("chats-clear");
        let one = Uuid::new_v4();
        let two = Uuid::new_v4();
        save_chat(&root.0, &one, "{}").unwrap();
        save_chat(&root.0, &two, "{}").unwrap();

        delete_chat(&root.0, &one).unwrap();
        assert_eq!(chat_files(&root.0).unwrap().len(), 1);
        delete_chat(&root.0, &one).unwrap();

        clear_chats(&root.0).unwrap();
        assert!(chat_files(&root.0).unwrap().is_empty());
        clear_chats(&root.0).unwrap();

        // …and the store comes back on the next turn rather than the project being left unable
        // to keep one.
        save_chat(&root.0, &one, "{}").unwrap();
        assert_eq!(chat_files(&root.0).unwrap().len(), 1);
    }

    /// A present-but-unparseable file must surface as **damage**, never masquerade as "no
    /// session": that `Corrupt` is what licenses the caller to keep the file aside and open
    /// blank.
    #[test]
    fn an_unparseable_session_is_corrupt_not_unreadable() {
        let root = TempRoot::new("session-corrupt");
        let dir = strata_dir(&root.0);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("session.json"), "{ not json").unwrap();
        assert!(matches!(
            load_session(&root.0),
            Err(SessionLoadError::Corrupt(_))
        ));
        // Bytes that aren't even UTF-8 are damage too, not a failed read — the distinction
        // `read_to_string` cannot make, which is why the load decodes the bytes itself.
        fs::write(dir.join("session.json"), [0x7b, 0xff, 0xfe]).unwrap();
        assert!(matches!(
            load_session(&root.0),
            Err(SessionLoadError::Corrupt(_))
        ));
    }

    /// A file that can't be *read* — permissions, an IO error, a mount that went away — is
    /// a different answer entirely: its contents are unknown and probably fine, so the
    /// caller must leave it alone. (A directory in the session file's place fails the read
    /// on every platform and regardless of privilege, which `chmod 000` does not.)
    #[test]
    fn a_session_that_cannot_be_read_is_unreadable_not_corrupt() {
        let root = TempRoot::new("session-unreadable");
        let dir = strata_dir(&root.0);
        fs::create_dir_all(dir.join("session.json")).unwrap();
        assert!(matches!(
            load_session(&root.0),
            Err(SessionLoadError::Unreadable(_))
        ));
    }

    /// The `.strata/` entries of `root`, sorted.
    fn strata_entries(root: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(strata_dir(root))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    /// Every durable write goes temp-file + rename, so no `.tmp` may outlive a save —
    /// including the rotating history load, which replaces an append-only file.
    #[test]
    fn durable_writes_strand_no_temp_files() {
        let root = TempRoot::new("atomic");
        scaffold(&root.0).unwrap();
        save_defs(&root.0, &load_defs(&root.0).unwrap()).unwrap();
        save_session(
            &root.0,
            &SessionSnapshot {
                tabs: vec![tab("query 1", "SELECT 1")],
                active: None,
                window: None,
                layout: Layout::default(),
            },
        )
        .unwrap();
        for i in 0..4 {
            append_history(&root.0, &run(&format!("q{i}"), i)).unwrap();
        }
        assert_eq!(load_history(&root.0, 2).unwrap().len(), 2, "rotated");
        assert_eq!(
            strata_entries(&root.0),
            [
                ".gitignore",
                "history.jsonl",
                "project.json",
                "session.json"
            ]
        );
    }

    /// A save that can't be written must leave the last good `project.json` in place — the
    /// whole point of writing through the temp: the target isn't touched until the rename.
    #[cfg(unix)]
    #[test]
    fn a_failed_save_keeps_the_previous_defs() {
        use std::os::unix::fs::PermissionsExt;
        let root = TempRoot::new("atomic-fail");
        let defs = scaffold(&root.0).unwrap();
        let dir = strata_dir(&root.0);
        // Read-only `.strata/` — the temp can't be created, so the write fails before the
        // rename (`create_dir_all` on an existing dir still succeeds).
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o500)).unwrap();
        let res = save_defs(
            &root.0,
            &ProjectDefs {
                name: "clobbered".into(),
                ..Default::default()
            },
        );
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        assert!(res.is_err());
        assert!(load_defs(&root.0).unwrap() == defs);
        assert_eq!(strata_entries(&root.0), [".gitignore", "project.json"]);
    }

    /// A kill between an atomic write's `create` and its `rename` has no error path to run,
    /// so it strands a temp in `.strata/`. The next save clears it — this is the wiring
    /// half; which temps are *safe* to remove is `crate::util`'s own tests.
    #[test]
    fn a_save_sweeps_a_temp_a_dead_writer_left_behind() {
        use std::time::{Duration, SystemTime};
        let root = TempRoot::new("sweep-wiring");
        scaffold(&root.0).unwrap();
        // Another process's temp (ours is never swept — it could be a write in flight),
        // back-dated well past the staleness threshold.
        let stranded = strata_dir(&root.0).join(format!(
            ".session.json.{}.0.tmp",
            process::id().wrapping_add(1)
        ));
        let file = fs::File::create(&stranded).unwrap();
        file.set_times(
            fs::FileTimes::new().set_modified(SystemTime::now() - Duration::from_secs(48 * 3600)),
        )
        .unwrap();
        drop(file);

        save_session(
            &root.0,
            &SessionSnapshot {
                tabs: vec![tab("query 1", "SELECT 1")],
                active: None,
                window: None,
                layout: Layout::default(),
            },
        )
        .unwrap();

        assert!(!stranded.exists(), "the stranded temp is gone");
        assert_eq!(
            strata_entries(&root.0),
            [".gitignore", "project.json", "session.json"]
        );
    }

    #[test]
    fn source_paths_resolve_and_relativize() {
        let root = Path::new("/proj");
        assert_eq!(resolve_source(root, None, "events"), "/proj/events");
        assert_eq!(
            resolve_source(root, None, "/abs/data.parquet"),
            "/abs/data.parquet"
        );
        assert_eq!(relativize(root, "/proj/events"), "events");
        assert_eq!(relativize(root, "/elsewhere/x.csv"), "/elsewhere/x.csv");
        // Round trip: what the engine gets resolves back to what the file stores.
        assert_eq!(
            relativize(root, &resolve_source(root, None, "sub/dir")),
            "sub/dir"
        );
    }

    /// A source over a connection is composed onto that bucket and **never** onto the project
    /// folder — the whole reason this function takes the connection. `s3://` is not an absolute
    /// path, so the local rule would have turned every one of these into `/proj/…`.
    #[test]
    fn a_source_over_a_connection_is_composed_onto_its_bucket() {
        let root = Path::new("/proj");
        let s3 = Some("s3://acme-lake");
        assert_eq!(
            resolve_source(root, s3, "events/2024/**/*.parquet"),
            "s3://acme-lake/events/2024/**/*.parquet"
        );
        // One separator, wherever the user put theirs: a leading `/` means the bucket root.
        assert_eq!(
            resolve_source(root, s3, "/events/"),
            "s3://acme-lake/events/"
        );
        assert_eq!(
            resolve_source(root, Some("s3://acme-lake/"), "events/"),
            "s3://acme-lake/events/"
        );
        // An HTTP connection is a whole origin, and composes exactly the same way.
        assert_eq!(
            resolve_source(root, Some("http://aserver:8484"), "data/a.csv"),
            "http://aserver:8484/data/a.csv"
        );
    }

    /// The split is the composition read backwards, and the round trip is the claim: whatever
    /// [`split_remote`] takes apart, [`resolve_source`] puts back byte for byte. A typed
    /// `LOCATION` arrives composed (ED-10), and it has to reach the def as the pair every other
    /// path already holds.
    #[test]
    fn splitting_a_remote_location_is_the_composition_read_backwards() {
        let root = Path::new("/proj");
        for location in [
            "s3://acme-lake/events/2024/**/*.parquet",
            "gs://lake/daily/",
            "http://aserver:8484/data/a.csv",
            // A bucket with nothing under it: answered as an empty source, so the caller can say
            // what is missing rather than a parse saying nothing at all.
            "s3://acme-lake",
        ] {
            let (url, source) = split_remote(location).expect("a scheme");
            assert_eq!(
                resolve_source(root, Some(&url), &source).trim_end_matches('/'),
                location.trim_end_matches('/'),
                "{location}"
            );
        }
        assert_eq!(
            split_remote("s3://acme-lake/events/"),
            Some(("s3://acme-lake".into(), "events/".into()))
        );
        // A path is not a URL, and neither is a Windows drive letter — the separator is `://`,
        // never a bare colon.
        assert_eq!(split_remote("/proj/events/"), None);
        assert_eq!(split_remote("events/2024"), None);
        assert_eq!(split_remote("C:\\data\\events"), None);
    }
}
