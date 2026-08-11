//! The launcher's view model: the recents, filtered by the search box and split into the
//! **PINNED** and **RECENT** groups the canvas shows.
//!
//! Pure over an [`AppConfig`] snapshot, so the grouping + filter rules are testable without
//! a window: they're the part with actual behaviour (what "matches" means, whether a group
//! renders, which empty state is the right one).

use strata_core::config::AppConfig;
use strata_core::util::contains_lowercased;

/// One project row: everything the row paints, resolved from the config once.
#[derive(Clone, PartialEq)]
pub struct ProjectRow {
    pub name: String,
    /// The project folder — [`RecentProject::path`](strata_core::config::RecentProject::path),
    /// handed straight to the open path.
    pub path: String,
    pub pinned: bool,
    /// Whether this project already has a window (the accent avatar; opening focuses it).
    pub open: bool,
}

/// The launcher list for a query: the two groups, in canvas order.
#[derive(Clone, PartialEq, Default)]
pub struct ProjectList {
    pub pinned: Vec<ProjectRow>,
    pub recent: Vec<ProjectRow>,
}

impl ProjectList {
    /// Every recent project that matches `query`, split into pinned and unpinned. A row
    /// matches on **name or path**, case-insensitively (the Dioxus launcher's predicate);
    /// an empty / whitespace query matches everything.
    pub fn build(config: &AppConfig, query: &str) -> Self {
        let needle = query.trim().to_lowercase();
        let rows = config.recent_projects.iter().filter(|r| {
            needle.is_empty()
                || contains_lowercased(&r.name, &needle)
                || contains_lowercased(&r.path, &needle)
        });
        let mut list = Self::default();
        for r in rows {
            let row = ProjectRow {
                name: r.name.clone(),
                path: r.path.clone(),
                pinned: r.pinned,
                open: config.open_projects.contains(&r.path),
            };
            if row.pinned {
                list.pinned.push(row);
            } else {
                list.recent.push(row);
            }
        }
        list
    }

    pub fn is_empty(&self) -> bool {
        self.pinned.is_empty() && self.recent.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use strata_core::config::RecentProject;

    fn config() -> AppConfig {
        let recent = |name: &str, path: &str, pinned: bool| RecentProject {
            name: name.into(),
            path: path.into(),
            last_opened: 0,
            pinned,
        };
        AppConfig {
            recent_projects: vec![
                recent("sample_2024", "/Users/a/Development/sample_2024", false),
                recent("prod_metrics", "/Users/a/Development/prod_metrics", true),
                recent("ml_features", "/Users/a/data/ml_features", false),
            ],
            open_projects: vec!["/Users/a/data/ml_features".into()],
            ..AppConfig::default()
        }
    }

    #[test]
    fn splits_pinned_from_recent_keeping_config_order() {
        let list = ProjectList::build(&config(), "");
        assert_eq!(
            list.pinned.iter().map(|r| &r.name).collect::<Vec<_>>(),
            ["prod_metrics"]
        );
        assert_eq!(
            list.recent.iter().map(|r| &r.name).collect::<Vec<_>>(),
            ["sample_2024", "ml_features"]
        );
    }

    #[test]
    fn open_projects_are_marked() {
        let list = ProjectList::build(&config(), "");
        assert!(
            list.recent
                .iter()
                .find(|r| r.name == "ml_features")
                .unwrap()
                .open
        );
        assert!(
            !list
                .recent
                .iter()
                .find(|r| r.name == "sample_2024")
                .unwrap()
                .open
        );
    }

    #[test]
    fn filter_matches_name_or_path_case_insensitively() {
        // By name…
        let list = ProjectList::build(&config(), "METRICS");
        assert_eq!(list.pinned.len(), 1);
        assert!(list.recent.is_empty());
        // …and by path, which is the only thing distinguishing these two.
        let list = ProjectList::build(&config(), "/data/");
        assert!(list.pinned.is_empty());
        assert_eq!(
            list.recent.iter().map(|r| &r.name).collect::<Vec<_>>(),
            ["ml_features"]
        );
        // A whitespace-only query is not a filter.
        assert_eq!(ProjectList::build(&config(), "   ").recent.len(), 2);
    }

    #[test]
    fn no_match_is_an_empty_list() {
        assert!(ProjectList::build(&config(), "zzz").is_empty());
        assert!(ProjectList::build(&AppConfig::default(), "").is_empty());
    }
}
