//! The **workbench** — the center working area: the tab strip (`Tabs`) plus the
//! active tab's `Workspace` (SQL editor + results). The parent `Workbench` renders
//! the strip and the active tab's content (or the no-tabs empty state).
//!
//! Panes below are each their own `#[component]` reading the focused per-window
//! stores directly, so they have independent reactive scopes — typing in the editor doesn't
//! re-render the results grid — and their transient UI signals (the tab menu, the
//! cell view, the rename draft) stay component-local. Submodules: `tabs` (`Tabs`),
//! `workspace` (`Workspace` = editor + results), `editor` (`Editor`), `results`
//! (`Results` switch + toolbar + pager + placeholders), `grid` (`ResultsGrid` +
//! nested-cell view), `plan_view` (`PlanView` + per-node `PlanNodeCard`). The
//! per-item cell render helper (`grid::render_cell`) stays a plain fn.
//!
//! Note the two `Workspace`s: `crate::session::Workspace` is a *tab's data*; the
//! `workspace::Workspace` component here is that tab's *view*.

use dioxus::prelude::*;
// Store collection methods (`.iter()` over the workspaces Vec store); the crate's
// own examples import this glob alongside the dioxus prelude.
use dioxus_stores::*;

// The `Store` derive on `Session`/`Workspace` generates these lens-accessor
// extension traits (`.workspaces()`, `.active()`, `.id()`, …); they must be in
// scope to iterate the session store and read per-workspace fields.
use crate::session::{SessionStoreExt, WorkspaceStoreExt};

mod editor;
pub(crate) mod grid;
mod plan_view;
mod results;
mod tabs;
mod workspace;

/// The center working area: the tab strip plus every open `Workspace`, each bound
/// to its own reactive sub-store (only the active one is visible). Renders the
/// no-tabs empty state when the session is empty.
///
/// Every workspace's view is mounted at once (hidden with CSS when inactive), so
/// each controlled `CodeEditor` stays bound to *its* workspace's `sql` lens —
/// editing an inactive tab can never leak into the active one, and switching tabs
/// is a pure show/hide with no editor remount.
#[component]
pub fn Workbench() -> Element {
    let sess = crate::session::store();
    // Read the active id + emptiness (both subscribe this component).
    let active = sess.active().cloned();
    let empty = sess.workspaces().read().is_empty();
    rsx! {
        main { class: "ps-main",
            tabs::Tabs {}
            if empty {
                results::EmptyState {}
            } else {
                // `sess.workspaces().iter()` yields a `Store<Workspace>` per entry;
                // `ws.id().cloned()` reads the id lens (bound once so `key` gets a
                // plain value, not the lens).
                for ws in sess.workspaces().iter() {
                    {
                        let id = ws.id().cloned();
                        rsx! {
                            workspace::Workspace { key: "{id}", ws, active: id == active }
                        }
                    }
                }
            }
        }
    }
}
