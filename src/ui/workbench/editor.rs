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
            div { style: "height:{editor_h}px;background:var(--main);border-bottom:1px solid var(--line);overflow:auto;",
                CodeEditor {
                    // Remount when the active content changes for a non-typing
                    // reason: the tab id covers switches, the epoch covers
                    // programmatic edits (Format/Clear/load). The editor seeds its
                    // textarea from `value` only on mount, so this re-seeds it.
                    key: "{ws_id}-{epoch}",
                    value: sql.clone(),
                    language: crate::ui::lang("sql"),
                    theme: crate::ui::code_theme(),
                    line_numbers: true,
                    spellcheck: false,
                    placeholder: "SELECT * FROM your_table LIMIT 100;",
                    class: "ps-sql",
                    oninput: move |v: String| state.write().set_active_sql(v),
                }
            }
        }
    }
}
