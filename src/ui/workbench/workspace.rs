//! The **workspace** — the active tab's content: the SQL `Editor` above the
//! `Results` viewer (grid / explain / chart / error), split by a drag handle.
//! (A tab's *data* is `crate::state::Workspace`; this component is its *view*.)

use dioxus::prelude::*;

use crate::action::panel::resize_handle;
use crate::state::{AppState, ResizeTarget};

#[component]
pub(crate) fn Workspace() -> Element {
    let state = use_context::<Signal<AppState>>();
    rsx! {
        super::editor::Editor {}
        {resize_handle(state, ResizeTarget::Editor)}
        super::results::Results {}
    }
}
