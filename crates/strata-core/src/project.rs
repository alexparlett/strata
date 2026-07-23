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

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use strata_model::{SavedQuery, TableDef, ViewDef};

/// The project directory name inside a project folder.
pub const STRATA_DIR: &str = ".strata";
const PROJECT_JSON: &str = "project.json";

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
    let mut defs: ProjectDefs =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    defs.tables.sort_by(|a, b| name_ord(&a.name, &b.name));
    defs.views.sort_by(|a, b| name_ord(&a.name, &b.name));
    defs.saved_queries.sort_by(|a, b| name_ord(&a.name, &b.name));
    Ok(defs)
}

/// Write the defs into `root`'s `.strata/` dir, creating it and its `.gitignore`
/// (ignoring the local `session.json`) if needed.
pub fn save_defs(root: &Path, defs: &ProjectDefs) -> Result<(), String> {
    let dir = strata_dir(root);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    ensure_gitignore(&dir);
    let json = serde_json::to_string_pretty(defs).map_err(|e| e.to_string())?;
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

/// Write `.strata/.gitignore` (ignoring the local session) if it's not there yet.
fn ensure_gitignore(dir: &Path) {
    let gi = dir.join(".gitignore");
    if !gi.exists() {
        let _ = fs::write(gi, "session.json\n");
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
pub fn name_ord(a: &str, b: &str) -> std::cmp::Ordering {
    a.chars()
        .flat_map(char::to_lowercase)
        .cmp(b.chars().flat_map(char::to_lowercase))
        // Names differing only in case still need a total order.
        .then_with(|| a.cmp(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh temp project folder, cleaned up on drop.
    struct TempRoot(PathBuf);
    impl TempRoot {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("strata-project-test-{tag}-{}", std::process::id()));
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
        // The local session is gitignored from the start.
        let gi = fs::read_to_string(strata_dir(&root.0).join(".gitignore")).unwrap();
        assert_eq!(gi, "session.json\n");
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
            id: uuid::Uuid::new_v4(),
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

    #[test]
    fn source_paths_resolve_and_relativize() {
        let root = Path::new("/proj");
        assert_eq!(resolve_source(root, "events"), "/proj/events");
        assert_eq!(resolve_source(root, "/abs/data.parquet"), "/abs/data.parquet");
        assert_eq!(relativize(root, "/proj/events"), "events");
        assert_eq!(relativize(root, "/elsewhere/x.csv"), "/elsewhere/x.csv");
        // Round trip: what the engine gets resolves back to what the file stores.
        assert_eq!(relativize(root, &resolve_source(root, "sub/dir")), "sub/dir");
    }
}
