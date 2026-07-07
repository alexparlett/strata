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
