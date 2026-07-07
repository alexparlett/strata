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
    use_revalidate(ws);
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
/// lens, so editing one tab never revalidates another and each tab carries its own
/// debounce; a programmatic edit to a background tab (format, history-load) still
/// revalidates it. Fixing a typo clears its problem on the next keystroke — with no
/// query run. Mounted once per tab (every `Workspace` view is mounted).
fn use_revalidate(ws: Store<crate::session::Workspace>) {
    let mut generation = use_signal(|| 0u64);
    use_effect(move || {
        // Subscribe to just this tab's id + sql (lens reads).
        let id = ws.id().cloned();
        let sql = ws.sql().cloned();
        let g = {
            let mut w = generation.write();
            *w += 1;
            *w
        };
        spawn(async move {
            // Debounce a burst of keystrokes into one validation pass.
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            if *generation.peek() != g {
                return; // superseded by a newer edit
            }
            crate::diagnostics::revalidate(id, &sql);
        });
    });
}
