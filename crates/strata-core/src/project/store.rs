//! Serialized project-definition writes shared by execution and presentation.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{save_defs, ProjectDefs};

/// A project's current definitions and their serialized persistence boundary.
#[derive(Clone)]
pub struct ProjectStore {
    root: PathBuf,
    defs: Arc<Mutex<ProjectDefs>>,
}

impl ProjectStore {
    /// Holds definitions already loaded or scaffolded from `root`.
    pub fn new(root: PathBuf, defs: ProjectDefs) -> Self {
        Self {
            root,
            defs: Arc::new(Mutex::new(defs)),
        }
    }

    /// The project folder these definitions belong to.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the current definitions, including changes awaiting a successful disk write.
    pub fn snapshot(&self) -> ProjectDefs {
        self.defs.lock().unwrap().clone()
    }

    /// Applies and saves a mutation under one lock, retaining failed writes for a later retry.
    pub fn update(&self, change: impl FnOnce(&mut ProjectDefs)) -> Result<(), String> {
        let mut defs = self.defs.lock().unwrap();
        change(&mut defs);
        save_defs(&self.root, &defs)
    }

    /// Saves edits relative to a caller's last projection without replacing unseen engine changes.
    pub fn merge(&self, before: &ProjectDefs, after: &ProjectDefs) -> Result<(), String> {
        self.update(|current| {
            if before.name != after.name {
                current.name.clone_from(&after.name);
            }
            merge_rows(&mut current.sources, &before.sources, &after.sources, |r| {
                r.name.to_lowercase()
            });
            merge_rows(&mut current.tables, &before.tables, &after.tables, |r| {
                r.name.to_lowercase()
            });
            merge_rows(&mut current.views, &before.views, &after.views, |r| {
                r.name.to_lowercase()
            });
            merge_rows(
                &mut current.saved_queries,
                &before.saved_queries,
                &after.saved_queries,
                |r| r.id,
            );
        })
    }
}

fn merge_rows<T: Clone + PartialEq, K: PartialEq>(
    current: &mut Vec<T>,
    before: &[T],
    after: &[T],
    key: fn(&T) -> K,
) {
    let same = |a: &T, b: &T| key(a) == key(b);
    current.retain(|row| {
        !before.iter().any(|old| same(row, old)) || after.iter().any(|new| same(row, new))
    });
    for row in after {
        if before.iter().find(|old| same(old, row)) == Some(row) {
            continue;
        }
        match current.iter_mut().find(|held| same(held, row)) {
            Some(held) => held.clone_from(row),
            None => current.push(row.clone()),
        }
    }
    current.sort_by_key(|row| {
        after
            .iter()
            .position(|new| same(row, new))
            .unwrap_or(after.len())
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_model::ViewDef;

    #[test]
    fn saved_queries_merge_by_identity_across_renames() {
        let first = strata_model::SavedQuery {
            id: uuid::Uuid::new_v4(),
            name: "same".into(),
            sql: "SELECT 1".into(),
            meta: String::new(),
        };
        let second = strata_model::SavedQuery {
            id: uuid::Uuid::new_v4(),
            ..first.clone()
        };
        let before = vec![first, second];
        let mut after = before.clone();
        after[0].name = "renamed".into();
        after[1].sql = "SELECT 2".into();
        let mut current = before.clone();
        merge_rows(&mut current, &before, &after, |r| r.id);
        assert!(current == after);
    }

    #[test]
    fn a_stale_projection_preserves_unseen_commits_and_applies_its_own_edits() {
        let root = std::env::temp_dir().join(format!("strata-merge-{}", std::process::id()));
        let before = ProjectDefs {
            views: vec![ViewDef {
                name: "old".into(),
                sql: "SELECT 1".into(),
            }],
            ..Default::default()
        };
        let store = ProjectStore::new(root.clone(), before.clone());
        store
            .update(|defs| {
                defs.views.clear();
                defs.views.push(ViewDef {
                    name: "new".into(),
                    sql: "SELECT 2".into(),
                });
            })
            .unwrap();
        let mut edited = before.clone();
        edited.name = "renamed".into();
        store.merge(&before, &edited).unwrap();
        let saved = super::super::load_defs(&root).unwrap();
        assert_eq!(saved.name, "renamed");
        assert_eq!(saved.views.len(), 1);
        assert_eq!(saved.views[0].name, "new");
        std::fs::remove_dir_all(root).unwrap();
    }
}
