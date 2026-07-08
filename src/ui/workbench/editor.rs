//! The SQL editor pane — an inline query toolbar (Run · Format · Clear ·
//! Save-as-view · Save-query) over the vendored, *controlled* `CodeEditor` bound to
//! this workspace's `sql` lens, plus **autocomplete** (S7) and inline **squiggles**
//! (S25) driven by `crate::sql`.
//!
//! The editor is controlled (`value` + `oninput` ↔ `ws.sql()`). Completion is
//! component-local (only the active tab receives input): the editor's `oncaret`
//! (caret from the input diff) recomputes completions, `onkeydown` drives the popup's
//! ↑/↓/Enter/Tab/Esc, and the menu is anchored at the caret via monospace metrics.

use dioxus::prelude::*;
use dioxus_stores::Store;

use crate::action::{dispatch, Action};
use crate::session::WorkspaceStoreExt;
use crate::sql::{Catalog, Completion, CompletionKind};
use crate::state::AppState;
use crate::ui::code_editor::{CodeEditor, Decoration};
use crate::ui::icons;

/// The open completion popup for this editor.
#[derive(Clone, PartialEq)]
struct Completing {
    items: Vec<Completion>,
    sel: usize,
    /// Caret line/column (0-based) for anchoring the menu.
    line: usize,
    col: usize,
}

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
    let dirty = ws.read().is_dirty();
    let ws_id = ws.id().cloned();
    let running = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .map(|e| e.read().running)
        .unwrap_or(false);
    // This tab's diagnostics → inline squiggles + Run-gating (S25). Reactive.
    let problems = crate::diagnostics::problems_for(ws_id);
    let has_errors = problems.iter().any(|d| d.is_error());
    let decorations: Vec<Decoration> = problems
        .into_iter()
        .filter_map(|d| {
            d.span.map(|range| Decoration {
                range,
                severity: d.severity,
            })
        })
        .collect();

    // Component-local completion state (per tab; only the active one is edited).
    let mut comp = use_signal(|| None::<Completing>);
    // Debounce generation for completion (like validation).
    let comp_gen = use_signal(|| 0u64);

    rsx! {
        section { style: "flex:none;background:var(--main);",
            div { class: "ed-toolbar",
                if running {
                    button {
                        class: "btn cancel",
                        style: "height:28px;",
                        title: "Cancel query (Esc)",
                        onclick: move |_| dispatch(state, Action::CancelQuery),
                        {icons::stop(12)}
                        "Cancel"
                        span { class: "kbd", style: "background:rgba(7,16,25,.22);color:inherit;border:none;margin-left:2px;", "Esc" }
                    }
                } else {
                    button {
                        class: "btn accent",
                        style: "height:28px;",
                        disabled: has_errors,
                        title: if has_errors { "Fix the validation errors to run" } else { "Run query (⌘/Ctrl+Enter)" },
                        onclick: move |_| if !has_errors { dispatch(state, Action::RunQuery) },
                        {icons::play(13)}
                        "Run"
                        span { class: "kbd", style: "background:rgba(7,16,25,.22);color:inherit;border:none;margin-left:2px;", "⌘↵" }
                    }
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
                style: "position:relative;height:{editor_h}px;background:var(--main);border-bottom:1px solid var(--line);overflow:auto;",
                CodeEditor {
                    value: ws.sql().cloned(),
                    language: crate::ui::lang("sql"),
                    theme: crate::ui::code_theme(),
                    line_numbers: true,
                    spellcheck: false,
                    placeholder: "SELECT * FROM your_table LIMIT 100;",
                    class: "ps-sql",
                    decorations,
                    oninput: move |v: String| ws.sql().set(v),
                    oncaret: move |caret: usize| refresh_completion(state, ws, comp, comp_gen, caret),
                    onkeydown: move |e: KeyboardEvent| handle_completion_key(ws, comp, e),
                    onblur: move |_| comp.set(None),
                    // Completion popup — rendered inside the viewport (children slot) so it
                    // shares the text coordinate system + `--dxc-editor-*` vars.
                    {completion_menu(ws, comp)}
                }
            }
        }
    }
}

/// The completion popup as an element for the editor's viewport children slot — an
/// empty node when closed. Positioned in viewport coords: below the caret line,
/// `col` chars in (+8px text padding). Reads `comp`, so the editor re-renders when it
/// changes.
fn completion_menu(
    ws: Store<crate::session::Workspace>,
    mut comp: Signal<Option<Completing>>,
) -> Element {
    let snap = comp.read();
    let Some(c) = snap.as_ref() else {
        return rsx! {};
    };
    let next_line = c.line + 1;
    let col = c.col;
    let sel = c.sel;
    let items = c.items.clone();
    drop(snap);

    rsx! {
        div {
            class: "sql-comp",
            style: "top:calc(var(--dxc-editor-line-height) * {next_line});left:calc({col}ch + 8px);",
            for (i, item) in items.into_iter().enumerate() {
                {
                    let accept = item.clone();
                    rsx! {
                        div {
                            key: "c{i}",
                            class: if i == sel { "sql-comp-row sel" } else { "sql-comp-row" },
                            // mousedown (not click) so the textarea doesn't blur first.
                            onmousedown: move |e| {
                                e.prevent_default();
                                apply_completion(ws, &accept);
                                comp.set(None);
                            },
                            span { class: "sql-comp-kind", "{kind_glyph(item.kind)}" }
                            span { class: "sql-comp-label", "{item.label}" }
                            if let Some(d) = &item.detail {
                                span { class: "sql-comp-detail", "{d}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Recompute completions at `caret`, **debounced 500ms** (like validation) so the
/// popup doesn't flash on every keystroke. Shows only while typing a word.
fn refresh_completion(
    state: Signal<AppState>,
    ws: Store<crate::session::Workspace>,
    mut comp: Signal<Option<Completing>>,
    mut comp_gen: Signal<u64>,
    caret: usize,
) {
    let sql = ws.sql().cloned();
    let typing = sql
        .get(..caret)
        .and_then(|s| s.chars().last())
        .map(|c| c.is_alphanumeric() || c == '_' || c == '.')
        .unwrap_or(false);
    if !typing {
        // A word boundary (space, punctuation, …) dismisses the popup immediately.
        comp.set(None);
        return;
    }
    let catalog = {
        let st = state.peek();
        Catalog::build(&st.project.tables, &st.project.views, st.functions.clone())
    };
    let g = {
        let mut w = comp_gen.write();
        *w += 1;
        *w
    };
    spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if *comp_gen.peek() != g {
            return; // superseded by a newer keystroke
        }
        let items = crate::sql::complete(&sql, caret, &catalog);
        if items.is_empty() {
            comp.set(None);
        } else {
            let (line, col) = line_col(&sql, caret);
            comp.set(Some(Completing {
                items,
                sel: 0,
                line,
                col,
            }));
        }
    });
}

/// Popup keyboard nav; returns having `prevent_default`ed when it consumed the key.
fn handle_completion_key(
    ws: Store<crate::session::Workspace>,
    mut comp: Signal<Option<Completing>>,
    e: KeyboardEvent,
) {
    let Some(mut c) = comp.peek().clone() else {
        return;
    };
    let n = c.items.len();
    match e.key() {
        Key::ArrowDown => {
            c.sel = (c.sel + 1) % n;
            comp.set(Some(c));
            e.prevent_default();
        }
        Key::ArrowUp => {
            c.sel = (c.sel + n - 1) % n;
            comp.set(Some(c));
            e.prevent_default();
        }
        Key::Enter | Key::Tab => {
            let item = c.items[c.sel].clone();
            apply_completion(ws, &item);
            comp.set(None);
            e.prevent_default();
        }
        Key::Escape => {
            comp.set(None);
            e.prevent_default();
        }
        // Space dismisses the popup (word boundary) — but is still inserted.
        Key::Character(s) if s == " " => {
            comp.set(None);
        }
        _ => {}
    }
}

/// Replace the completion's `replace` span in the tab's SQL with its insert text.
fn apply_completion(ws: Store<crate::session::Workspace>, item: &Completion) {
    let mut sql = ws.sql().cloned();
    if item.replace.start <= sql.len() && item.replace.end <= sql.len() {
        sql.replace_range(item.replace.clone(), &item.insert);
        ws.sql().set(sql);
    }
}

fn kind_glyph(kind: CompletionKind) -> &'static str {
    match kind {
        CompletionKind::Table => "T",
        CompletionKind::View => "V",
        CompletionKind::Column => "·",
        CompletionKind::Function => "ƒ",
        CompletionKind::Keyword => "K",
    }
}

/// 0-based (line, column) for a byte offset (column in chars = mono `ch` units).
fn line_col(sql: &str, off: usize) -> (usize, usize) {
    let off = off.min(sql.len());
    let (mut line, mut col) = (0usize, 0usize);
    for (i, ch) in sql.char_indices() {
        if i >= off {
            break;
        }
        if ch == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}
