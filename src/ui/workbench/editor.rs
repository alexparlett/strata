//! The SQL editor pane — a `dioxus-code` `CodeEditor`, keyed to remount on tab
//! switch / programmatic edits so it re-seeds its textarea from the new value.

use dioxus::prelude::*;
use dioxus_code_editor::CodeEditor;

use crate::state::AppState;

#[component]
pub(crate) fn Editor() -> Element {
    let mut state = use_context::<Signal<AppState>>();
    let sql = state.read().active_sql();
    let editor_h = state.read().editor_h;
    let (ws_id, epoch) = {
        let s = state.read();
        let ws_id = s
            .project
            .workspaces
            .get(s.project.active_ws)
            .map(|w| w.id)
            .unwrap_or(0);
        (ws_id, s.editor_epoch)
    };

    rsx! {
        section { style: "flex:none;background:var(--main);",
            // The key is on the wrapper element (not the `CodeEditor` itself): on a
            // tab switch / programmatic edit the whole subtree is torn down and the
            // editor remounts from the new `value` — the id covers switches, the
            // epoch covers Format/Clear/load. (`dioxus-code-editor` seeds its
            // textarea only on mount, so it must be a real remount.)
            div {
                key: "{ws_id}-{epoch}",
                style: "height:{editor_h}px;background:var(--main);border-bottom:1px solid var(--line);overflow:auto;",
                CodeEditor {
                    value: sql.clone(),
                    language: crate::ui::lang("sql"),
                    theme: crate::ui::code_theme(),
                    line_numbers: true,
                    spellcheck: false,
                    placeholder: "SELECT * FROM your_table LIMIT 100;",
                    class: "ps-sql",
                    // Write by the tab id this editor rendered for — never "active".
                    oninput: move |v: String| state.write().set_sql_for(ws_id, v),
                }
            }
        }
    }
}
