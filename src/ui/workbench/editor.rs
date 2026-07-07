//! The SQL editor pane — an inline query toolbar (Run · Format · Clear ·
//! Save-as-view · Save-query, relocated from the global header) over a *controlled*
//! `dioxus-code-editor` `CodeEditor` bound straight to this workspace's `sql` lens.
//!
//! The editor is controlled (`value` + `oninput` ↔ `ws.sql()`), so a keystroke
//! re-renders only this workspace and writes only its SQL — no key/remount, and no
//! cross-tab leak. The toolbar's actions still funnel through `dispatch`, acting on
//! the *active* workspace (this pane is only interactive when it's the active one).

use dioxus::prelude::*;
use dioxus_code_editor::CodeEditor;
use dioxus_stores::Store;

use crate::action::{dispatch, Action};
use crate::session::WorkspaceStoreExt;
use crate::state::AppState;
use crate::ui::icons;

/// A 30×30 toolbar icon button that dispatches `action`.
fn tool_btn(state: Signal<AppState>, action: Action, title: &str, icon: Element) -> Element {
    rsx! {
        button {
            class: "icon-btn",
            title: "{title}",
            onclick: move |_| dispatch(state, action.clone()),
            {icon}
        }
    }
}

#[component]
pub(crate) fn Editor(ws: Store<crate::session::Workspace>) -> Element {
    let state = use_context::<Signal<AppState>>();
    let editor_h = state.read().editor_h;
    // Emphasise Save when this workspace has unsaved changes (A6).
    // TODO(verify): reading the whole `Store<Workspace>` value to call the
    // plain-struct `is_dirty()` (needs origin + origin_hash + sql together).
    let dirty = ws.read().is_dirty();
    let running = {
        let id = ws.id().cloned();
        crate::runs::RUNS
            .resolve()
            .get(id)
            .map(|e| e.read().running)
            .unwrap_or(false)
    };

    rsx! {
        section { style: "flex:none;background:var(--main);",
            // Inline query toolbar (relocated from the global header, S4).
            div { class: "ed-toolbar",
                button {
                    class: "btn accent",
                    style: "height:28px;",
                    title: "Run query (⌘/Ctrl+Enter)",
                    disabled: running,
                    onclick: move |_| dispatch(state, Action::RunQuery),
                    {icons::play(13)}
                    if running { "Running…" } else { "Run" }
                    span { class: "kbd", style: "background:rgba(7,16,25,.22);color:inherit;border:none;margin-left:2px;", "⌘↵" }
                }
                div { style: "width:1px;height:18px;background:var(--line);margin:0 2px;" }
                {tool_btn(state, Action::FormatSql, "Format SQL", icons::format(15))}
                {tool_btn(state, Action::ClearSql, "Clear editor", icons::trash(15))}
                div { class: "spacer" }
                {tool_btn(state, Action::SaveAsView, "Save as view", icons::eye(15))}
                button {
                    class: if dirty { "icon-btn dirty" } else { "icon-btn" },
                    title: "Save query (⌘S)",
                    onclick: move |_| dispatch(state, Action::SaveQuery),
                    {icons::save(15)}
                }
            }
            div {
                style: "height:{editor_h}px;background:var(--main);border-bottom:1px solid var(--line);overflow:auto;",
                // Controlled editor: bound directly to this workspace's `sql` lens.
                // A keystroke writes only `ws.sql`, re-rendering only this pane.
                CodeEditor {
                    value: ws.sql().cloned(),
                    language: crate::ui::lang("sql"),
                    theme: crate::ui::code_theme(),
                    line_numbers: true,
                    spellcheck: false,
                    placeholder: "SELECT * FROM your_table LIMIT 100;",
                    class: "ps-sql",
                    oninput: move |v: String| ws.sql().set(v),
                }
            }
        }
    }
}
