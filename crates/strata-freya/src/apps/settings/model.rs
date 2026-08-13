//! The Settings window's **navigation tree** — the categories, the groups they sit under,
//! and the breadcrumb each one shows.
//!
//! Static data rather than a component, because three surfaces have to agree on it: the nav
//! rail draws it, the pane's breadcrumb reads it back off the current route, and (P4-09) the
//! search index will resolve a hit to the category that holds it. Spelling the tree out at
//! three call sites is how a category ends up in the rail under one name and in the
//! breadcrumb under another.

use crate::apps::settings::Route;

/// A collapsible heading in the nav. Not every category has one — Keymap sits at the top
/// level, because it is the only member its group would ever have.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum NavGroup {
    Appearance,
    Ai,
    Engine,
}

impl NavGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance & behaviour",
            Self::Ai => "AI",
            Self::Engine => "Engine",
        }
    }
}

/// One settings category: the route that shows it, the label the nav and breadcrumb use, and
/// the group heading it lives under.
#[derive(PartialEq, Eq, Debug)]
pub struct Category {
    pub route: Route,
    pub label: &'static str,
    pub group: Option<NavGroup>,
}

impl Category {
    /// The breadcrumb over the category's pane: `Appearance & behaviour › Theme`, or just the
    /// label for an ungrouped category.
    pub fn breadcrumb(&self) -> (Option<&'static str>, &'static str) {
        (self.group.map(NavGroup::label), self.label)
    }
}

/// Every category, in the order the nav lists them (design `Settings.dc.html`): the three
/// appearance-and-behaviour pages, then Keymap and Agent access on their own, then the
/// engine's.
pub const CATEGORIES: &[Category] = &[
    Category {
        route: Route::Theme,
        label: "Theme",
        group: Some(NavGroup::Appearance),
    },
    Category {
        route: Route::System,
        label: "System",
        group: Some(NavGroup::Appearance),
    },
    Category {
        route: Route::DataDisplay,
        label: "Data display",
        group: Some(NavGroup::Appearance),
    },
    Category {
        route: Route::Keymap,
        label: "Keymap",
        group: None,
    },
    Category {
        route: Route::Providers,
        label: "Providers",
        group: Some(NavGroup::Ai),
    },
    Category {
        route: Route::Chat,
        label: "Chat",
        group: Some(NavGroup::Ai),
    },
    Category {
        route: Route::Mcp,
        label: "MCP",
        group: Some(NavGroup::Ai),
    },
    Category {
        route: Route::Engine,
        label: "Properties",
        group: Some(NavGroup::Engine),
    },
];

/// The category a route shows. Every route has one by construction — see the test below,
/// which is what keeps that true as P4-04…P4-08 fill the panes in.
pub fn category(route: &Route) -> Option<&'static Category> {
    CATEGORIES.iter().find(|c| &c.route == route)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every route the window can be on is listed exactly once. A route missing from
    /// [`CATEGORIES`] is a page with no way to reach it and no breadcrumb; a route listed
    /// twice is two nav rows that both light up.
    #[test]
    fn every_route_has_exactly_one_category() {
        for route in [
            Route::Theme,
            Route::System,
            Route::DataDisplay,
            Route::Keymap,
            Route::Providers,
            Route::Chat,
            Route::Mcp,
            Route::Engine,
        ] {
            let hits = CATEGORIES.iter().filter(|c| c.route == route).count();
            assert_eq!(hits, 1, "{route:?} appears {hits} times in CATEGORIES");
        }
    }

    /// The nav renders group by group, so a group's members have to be contiguous: an
    /// interleaved list would draw the same heading twice.
    #[test]
    fn grouped_categories_are_contiguous() {
        let mut seen: Vec<Option<NavGroup>> = Vec::new();
        for cat in CATEGORIES {
            if seen.last() != Some(&cat.group) {
                assert!(
                    !seen.contains(&cat.group),
                    "{:?} is split across the list",
                    cat.group
                );
                seen.push(cat.group);
            }
        }
    }

    #[test]
    fn breadcrumbs_name_the_group_then_the_page() {
        let theme = category(&Route::Theme).unwrap();
        assert_eq!(
            theme.breadcrumb(),
            (Some("Appearance & behaviour"), "Theme")
        );
        assert_eq!(
            category(&Route::Keymap).unwrap().breadcrumb(),
            (None, "Keymap")
        );
    }
}
