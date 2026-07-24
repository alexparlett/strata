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
//! [`resolve_source`] / [`relativize`].

use std::cmp::Ordering;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{from_str, to_string, to_string_pretty};
use strata_model::{HistoryEntry, SavedQuery, SessionSnapshot, TableDef, ViewDef};

/// The project directory name inside a project folder.
pub const STRATA_DIR: &str = ".strata";
const PROJECT_JSON: &str = "project.json";
const SESSION_JSON: &str = "session.json";
const HISTORY_JSONL: &str = "history.jsonl";

/// The committed definitions — the shape of `.strata/project.json`.
#[derive(Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct ProjectDefs {
    #[serde(default)]
    pub name: String,
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

/// Whether a project already exists in folder `root` (a `.strata/project.json`).
/// Distinguishes "open existing" from "scaffold new", so a corrupt-but-present file is
/// surfaced as a load error rather than silently overwritten.
pub fn exists_at(root: &Path) -> bool {
    strata_dir(root).join(PROJECT_JSON).exists()
}

/// Load the defs from project folder `root`. `Err` when the file is missing or doesn't
/// parse. Catalog lists come back sorted ([`name_ord`]) — the file's order is just
/// whatever it was last written in.
pub fn load_defs(root: &Path) -> Result<ProjectDefs, String> {
    let path = strata_dir(root).join(PROJECT_JSON);
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut defs: ProjectDefs = from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    defs.tables.sort_by(|a, b| name_ord(&a.name, &b.name));
    defs.views.sort_by(|a, b| name_ord(&a.name, &b.name));
    defs.saved_queries
        .sort_by(|a, b| name_ord(&a.name, &b.name));
    Ok(defs)
}

/// Write the defs into `root`'s `.strata/` dir, creating it and its `.gitignore`
/// (ignoring the local `session.json`) if needed.
pub fn save_defs(root: &Path, defs: &ProjectDefs) -> Result<(), String> {
    let dir = strata_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    ensure_gitignore(&dir);
    let json = to_string_pretty(defs).map_err(|e| e.to_string())?;
    fs::write(dir.join(PROJECT_JSON), json).map_err(|e| e.to_string())
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

/// Load the [`SessionSnapshot`] from `root`'s `.strata/`. `Ok(None)` when there's no
/// session file yet (a fresh or never-saved project) — a first-class, expected state, not
/// an error. `Err` **only** when the file exists but doesn't parse, so the caller can log
/// it and fall back to a blank session rather than bricking the window on a corrupt file.
/// Concrete over the model type, exactly like [`load_defs`].
pub fn load_session(root: &Path) -> Result<Option<SessionSnapshot>, String> {
    let path = session_path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    from_str(&text)
        .map(Some)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Write the [`SessionSnapshot`] into `root`'s `.strata/` (gitignored), creating the
/// dir + its `.gitignore` if needed. The autosave side effect's sink (P4-14).
pub fn save_session(root: &Path, snapshot: &SessionSnapshot) -> Result<(), String> {
    let dir = strata_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    ensure_gitignore(&dir);
    let json = to_string_pretty(snapshot).map_err(|e| e.to_string())?;
    fs::write(session_path(root), json).map_err(|e| e.to_string())
}

/// The `.strata/history.jsonl` of the project folder `root` — the **local** append-only
/// query-history log (gitignored, per-user), separate from the committed defs.
pub fn history_path(root: &Path) -> PathBuf {
    strata_dir(root).join(HISTORY_JSONL)
}

/// Append one history entry as a JSON line to `root`'s `history.jsonl` (creating the dir +
/// `.gitignore` if needed). Append-only (DESIGN_SPEC §"History as `.jsonl`") so a completed
/// run is one cheap `O_APPEND` write, not a whole-file rewrite; [`load_history`] bounds the
/// file back down.
pub fn append_history(root: &Path, entry: &HistoryEntry) -> Result<(), String> {
    let dir = strata_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    ensure_gitignore(&dir);
    let mut line = to_string(entry).map_err(|e| e.to_string())?;
    line.push('\n');
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_path(root))
        .map_err(|e| e.to_string())?;
    file.write_all(line.as_bytes()).map_err(|e| e.to_string())
}

/// Load history for `root`, newest entries capped to `cap` (keep-last-N). Absent file →
/// empty (a project with no runs yet); a corrupt *line* is skipped, not fatal — one bad
/// append can't lose the whole log. Returns entries in **file order** (oldest → newest).
/// If the file has grown past `cap`, it's **rotated** in place to the kept window
/// (DESIGN_SPEC: "rotate to bound size").
pub fn load_history(root: &Path, cap: usize) -> Result<Vec<HistoryEntry>, String> {
    let path = history_path(root);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut entries: Vec<HistoryEntry> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| from_str(l).ok())
        .collect();
    if entries.len() > cap {
        entries.drain(0..entries.len() - cap);
        // Rewrite the file down to what we kept (rare — only when it overflowed).
        let mut out = String::new();
        for entry in &entries {
            if let Ok(line) = to_string(entry) {
                out.push_str(&line);
                out.push('\n');
            }
        }
        let _ = fs::write(&path, out);
    }
    Ok(entries)
}

/// Ensure `.strata/.gitignore` ignores the local, per-user files — the working session and
/// the query-history log — adding any that are missing while preserving other lines. Run
/// from every local-file write, so an older `.gitignore` (session-only) gets upgraded.
fn ensure_gitignore(dir: &Path) {
    let gi = dir.join(".gitignore");
    let existing = fs::read_to_string(&gi).unwrap_or_default();
    let mut lines: Vec<&str> = existing.lines().collect();
    let mut changed = false;
    for wanted in [SESSION_JSON, HISTORY_JSONL] {
        if !lines.iter().any(|l| l.trim() == wanted) {
            lines.push(wanted);
            changed = true;
        }
    }
    if changed {
        let mut out = lines.join("\n");
        out.push('\n');
        let _ = fs::write(&gi, out);
    }
}

/// Resolve a (possibly project-relative) source path to an absolute path for the
/// engine / filesystem, joining relative entries onto `root` (the project folder).
pub fn resolve_source(root: &Path, p: &str) -> String {
    let path = Path::new(p);
    if path.is_absolute() {
        return p.to_string();
    }
    root.join(p).to_string_lossy().into_owned()
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
    use strata_model::{Origin, ResultsView, TabId, TabSnapshot, WindowGeom};
    use uuid::Uuid;

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
        // The local, per-user files are gitignored from the start.
        let gi = fs::read_to_string(strata_dir(&root.0).join(".gitignore")).unwrap();
        assert_eq!(gi, "session.json\nhistory.jsonl\n");
        // `assert!` over `assert_eq!` here and below: the model types are serde
        // vocabulary and deliberately don't derive `Debug`.
        let loaded = load_defs(&root.0).unwrap();
        assert!(loaded == defs);
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
        };
        save_session(&root.0, &snap).unwrap();
        // (`SessionSnapshot` is serde vocabulary and doesn't derive `PartialEq` — check fields.)
        let loaded = load_session(&root.0).unwrap().unwrap();
        assert_eq!(loaded.tabs.len(), 2);
        assert_eq!(loaded.tabs[0].text, "SELECT 1");
        assert_eq!(loaded.active, Some(id));
        assert_eq!(loaded.window.unwrap().width, 800.0);
        // The session file is gitignored the moment it's written (alongside history).
        let gi = fs::read_to_string(strata_dir(&root.0).join(".gitignore")).unwrap();
        assert_eq!(gi, "session.json\nhistory.jsonl\n");
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

    #[test]
    fn corrupt_session_is_an_error_not_a_silent_none() {
        let root = TempRoot::new("session-corrupt");
        let dir = strata_dir(&root.0);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("session.json"), "{ not json").unwrap();
        // A present-but-unparseable file must surface (the caller logs + falls back to a
        // blank session), never masquerade as "no session".
        assert!(load_session(&root.0).is_err());
    }

    #[test]
    fn source_paths_resolve_and_relativize() {
        let root = Path::new("/proj");
        assert_eq!(resolve_source(root, "events"), "/proj/events");
        assert_eq!(
            resolve_source(root, "/abs/data.parquet"),
            "/abs/data.parquet"
        );
        assert_eq!(relativize(root, "/proj/events"), "events");
        assert_eq!(relativize(root, "/elsewhere/x.csv"), "/elsewhere/x.csv");
        // Round trip: what the engine gets resolves back to what the file stores.
        assert_eq!(
            relativize(root, &resolve_source(root, "sub/dir")),
            "sub/dir"
        );
    }
}
