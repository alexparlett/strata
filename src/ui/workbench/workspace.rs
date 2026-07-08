//! The **workspace** — a single tab's content: the SQL `Editor` (with its inline
//! query toolbar) above the `Results` viewer (grid / explain / chart / error),
//! split by a drag handle. (A tab's *data* is `crate::session::Workspace`; this
//! component is its *view*.)
//!
//! Every open workspace's view is mounted at once — the inactive ones are hidden
//! with `display:none` rather than unmounted — so each controlled editor keeps its
//! own `sql` lens binding and a tab switch is a pure show/hide (no remount).

use dioxus::prelude::*;
use dioxus_stores::Store;

use crate::action::panel::resize_handle;
use crate::session::WorkspaceStoreExt;
use crate::state::{AppState, ResizeTarget};

#[component]
pub(crate) fn Workspace(ws: Store<crate::session::Workspace>, active: bool) -> Element {
    let state = use_context::<Signal<AppState>>();
    // Keep this tab's Problems diagnostics in step with *its* SQL.
    use_revalidate(state, ws);
    // Hidden (but mounted) when this isn't the active tab.
    let style = if active {
        "display:flex;flex:1;flex-direction:column;min-height:0"
    } else {
        "display:none"
    };
    let ws_id = ws.id().cloned();
    rsx! {
        div { style: "{style}",
            super::editor::Editor { ws }
            {resize_handle(state, ResizeTarget::Editor)}
            super::results::Results { ws_id }
        }
    }
}

/// Recompute this tab's static diagnostics (Problems) as *its* SQL changes,
/// debounced. Scoped per-tab: the effect subscribes to only this workspace's `sql`
/// lens (so editing one tab never revalidates another, each tab has its own debounce,
/// and a programmatic edit to a background tab still revalidates it). The catalog is
/// read non-reactively (`peek`) so the effect isn't woken by unrelated `AppState`
/// changes. Runs `crate::sql::analyze` and stores the result; no query is executed.
fn use_revalidate(state: Signal<AppState>, ws: Store<crate::session::Workspace>) {
    let mut generation = use_signal(|| 0u64);
    use_effect(move || {
        // Subscribe to the session store so this fires on *every* edit — the proven
        // pattern from `use_persist_session` (a bare `ws.sql().cloned()` read here did
        // not reliably wake the effect). We still revalidate only THIS tab.
        let store = crate::session::store();
        let _sub = store.read();
        let id = ws.id().cloned();
        let sql = ws.sql().cloned();
        let catalog = {
            let st = state.peek();
            crate::sql::Catalog::build(&st.project.tables, &st.project.views, st.functions.clone())
        };
        let g = {
            let mut w = generation.write();
            *w += 1;
            *w
        };
        spawn(async move {
            // Debounce a burst of keystrokes into one validation pass.
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            if *generation.peek() != g {
                return; // superseded by a newer edit
            }
            let diags = crate::sql::analyze(&sql, &catalog);
            crate::diagnostics::set(id, diags);
        });
    });
}
