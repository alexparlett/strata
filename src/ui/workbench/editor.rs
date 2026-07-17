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

use crate::action::panel::Resizer;
use crate::action::{dispatch, Action};
use crate::session::WorkspaceStoreExt;
use crate::sql::{Catalog, Completion, CompletionKind};
use crate::ui::code_editor::{CodeEditor, Decoration};
use crate::ui::components::{
    Caption, Icon, IconButton, IconButtonVariant, Meta, MonoValue, Popup, Prose, Rect,
};
use crate::ui::icons::{IconName, IconSize};

/// The open completion popup for this editor.
#[derive(Clone, PartialEq)]
struct Completing {
    items: Vec<Completion>,
    sel: usize,
    /// Caret line/column (0-based) for anchoring the menu.
    line: usize,
    col: usize,
}

/// The lint hover popover (S27): the diagnostic message under the pointer. `x`/`y` are
/// the client-px anchor (just below the squiggled token's start), stable while the
/// pointer stays on the same token so the popover doesn't jitter with the cursor.
#[derive(Clone, PartialEq)]
struct LintHover {
    message: String,
    x: f64,
    y: f64,
}

/// A compact (28px) toolbar icon button that dispatches `action`.
fn tool_btn(action: Action, title: &str, icon: IconName) -> Element {
    rsx! {
        IconButton {
            variant: IconButtonVariant::Toolbar,
            icon: icon,
            compact: true,
            title: "{title}",
            onclick: move |_| dispatch(action.clone()),
        }
    }
}

#[component]
pub(crate) fn Editor(ws: Store<crate::session::Workspace>) -> Element {
    // The editor owns its own height — a local reactive signal, not global state.
    let height = use_signal(|| 240.0);
    let dirty = ws.read().is_dirty();
    let ws_id = ws.id().cloned();
    let running = crate::runs::RUNS
        .resolve()
        .get(ws_id)
        .map(|e| e.read().running)
        .unwrap_or(false);
    // This tab's diagnostics → inline squiggles + Run-gating (S25). Reactive.
    // Run is blocked by any *validation* problem (a typo/syntax issue means the query
    // won't run); a lingering *execution* error doesn't block — you re-run to clear it.
    let problems = crate::diagnostics::problems_for(ws_id);
    let block_run = problems
        .iter()
        .any(|d| matches!(d.source, crate::diagnostics::DiagSource::Validation));
    let decorations: Vec<Decoration> = problems
        .iter()
        .filter_map(|d| {
            d.span.clone().map(|range| Decoration {
                range,
                severity: d.severity,
            })
        })
        .collect();
    // Kept for the hover popover's hit-test (message + span per diagnostic).
    let hover_problems = problems;

    // Component-local completion state (per tab; only the active one is edited).
    let mut comp = use_signal(|| None::<Completing>);
    // Debounce generation for completion (like validation).
    let comp_gen = use_signal(|| 0u64);
    // The lint hover popover (S27): the diagnostic message shown while the pointer is
    // over a squiggle. Component-local, like completion.
    let mut hover = use_signal(|| None::<LintHover>);

    rsx! {
        section { style: "flex:none;background:var(--main);border-bottom:1px solid var(--line);",
            div { class: "ed-toolbar",
                if running {
                    // Running: the primary slot collapses to a red Cancel (E4).
                    IconButton {
                        variant: IconButtonVariant::Primary,
                        class: "stop",
                        icon: IconName::Stop,
                        title: "Cancel query (Esc)",
                        onclick: move |_| dispatch(Action::CancelQuery),
                    }
                } else {
                    // Run (⌘↵) · Explain plan · Explain analyze — three icon buttons (E4,
                    // v19; the old split-button is retired). The Explain buttons just
                    // dispatch `RunExplain`; the wrap-with-EXPLAIN happens engine-side in
                    // the handler (`query::run_explain`) — the editor buffer is untouched.
                    IconButton {
                        variant: IconButtonVariant::Primary,
                        icon: IconName::Play,
                        disabled: block_run,
                        title: if block_run { "Fix the validation problems to run".to_string() } else { format!("Run query ({})", crate::keymap::hint(crate::config::Command::RunQuery)) },
                        onclick: move |_| if !block_run { dispatch(Action::RunQuery) },
                    }
                    IconButton {
                        variant: IconButtonVariant::Toolbar,
                        compact: true,
                        icon: IconName::List,
                        disabled: block_run,
                        title: "Explain plan",
                        onclick: move |_| if !block_run { dispatch(Action::RunExplain(false)) },
                    }
                    IconButton {
                        variant: IconButtonVariant::Toolbar,
                        compact: true,
                        icon: IconName::Stopwatch,
                        disabled: block_run,
                        title: "Explain analyze — runs the query and times each operator",
                        onclick: move |_| if !block_run { dispatch(Action::RunExplain(true)) },
                    }
                }
                div { style: "width:1px;height:18px;background:var(--line);margin:0 var(--sp-1);" }
                {tool_btn(Action::FormatSql, "Format SQL", IconName::Format)}
                {tool_btn(Action::ClearSql, "Clear editor", IconName::Trash)}
                // V29: the save pair is separated, not pushed right — the whole toolbar
                // sits left (run · explain │ edit │ save).
                div { style: "width:1px;height:18px;background:var(--line);margin:0 var(--sp-1);" }
                {tool_btn(Action::SaveAsView, "Save as view", IconName::Eye)}
                IconButton { icon: IconName::Save,
                    variant: IconButtonVariant::Toolbar,
                    compact: true,
                    dirty: dirty,
                    title: format!("Save query ({})", crate::keymap::hint(crate::config::Command::SaveQuery)),
                    onclick: move |_| dispatch(Action::SaveQuery),
                }
            }
            div {
                style: "position:relative;height:{height}px;background:var(--main);overflow:auto;",
                // The textarea's focus bubbles here (focusin/focusout) → hold the Select All
                // scope so ⌘A selects the editor text (and the Edit-menu item enables).
                onfocusin: move |_| crate::menu::set_select_all_scope(crate::menu::SelectAllScope::Input),
                onfocusout: move |_| crate::menu::set_select_all_scope(crate::menu::SelectAllScope::None),
                CodeEditor {
                    value: ws.sql().cloned(),
                    language: crate::ui::lang("sql"),
                    theme: crate::ui::code_theme(),
                    line_numbers: true,
                    spellcheck: false,
                    class: "ps-sql",
                    decorations,
                    oninput: move |v: String| ws.sql().set(v),
                    oncaret: move |caret: usize| refresh_completion(ws, comp, comp_gen, caret),
                    onkeydown: move |e: KeyboardEvent| handle_completion_key(ws, comp, comp_gen, e),
                    onblur: move |_| close_completion(comp, comp_gen),
                    // Clicking in the editor to move the caret keeps focus (no blur) — close too.
                    onmousedown: move |_| close_completion(comp, comp_gen),
                    // Hover a squiggle → show its diagnostic message (S27). Suppressed while
                    // the completion popup is open. Only writes `hover` when the anchored hit
                    // changes, so the frequent mousemove doesn't thrash re-renders.
                    onmousemove: move |e: MouseEvent| {
                        if comp.peek().is_some() {
                            if hover.peek().is_some() { hover.set(None); }
                            return;
                        }
                        let ep = e.element_coordinates();
                        let sql = ws.sql().cloned();
                        let next = hovered_lint(&sql, &hover_problems, ep.x, ep.y).map(
                            |(message, line, col)| {
                                // Token's client-px anchor = textarea client origin (cursor
                                // client − cursor element) + token's element-px position,
                                // one line below the token. Cursor-independent → no jitter.
                                let cp = e.client_coordinates();
                                let x = cp.x - ep.x + col as f64 * ED_CH_W + ED_PAD_X;
                                let y = cp.y - ep.y + (line + 1) as f64 * ED_LINE_H;
                                LintHover { message, x, y }
                            },
                        );
                        if *hover.peek() != next {
                            hover.set(next);
                        }
                    },
                    onmouseleave: move |_| {
                        if hover.peek().is_some() { hover.set(None); }
                    },
                    // Completion menu — inside the viewport (children slot) so it shares the
                    // text coordinate system + `--dxc-editor-*` vars.
                    {completion_menu(ws, comp)}
                }
            }
            // Lint hover popover (S27) — a fixed-positioned `Popup` in hover mode, so it
            // lives at section level rather than in the editor's text coordinate system.
            {lint_popover(hover, comp)}
        }
        // Bottom-edge resize handle — owns the editor's height (vs the results below).
        Resizer { axis_x: false, sign: 1.0, min: 92.0, max: 480.0, size: height }
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
                            Meta { class: "sql-comp-kind", "{kind_glyph(item.kind)}" }
                            MonoValue { class: "sql-comp-label", "{item.label}" }
                            if let Some(d) = &item.detail {
                                Caption { class: "sql-comp-detail", "{d}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The lint hover popover — the diagnostic message on the neutral `.ds-tooltip` card (red
/// icon + neutral text, per §07), pinned just below the squiggled token. A *point-anchored*
/// tooltip card (not a hover-a-trigger `Tooltip`), so it's a raw [`Popup`] with the
/// tooltip chrome + the pointer-transparent `ds-float` class. Empty when nothing is hovered,
/// or while the completion popup is open (they'd overlap — completion wins).
fn lint_popover(hover: Signal<Option<LintHover>>, comp: Signal<Option<Completing>>) -> Element {
    if comp.read().is_some() {
        return rsx! {};
    }
    let snap = hover.read();
    let Some(h) = snap.as_ref() else {
        return rsx! {};
    };
    let (x, y, msg) = (h.x, h.y, h.message.clone());
    drop(snap);

    rsx! {
        Popup {
            anchor: Rect::point(x, y),
            card_class: "ds-tooltip ds-float",
            // Neutral tooltip chrome (§07) with a red icon + neutral message — the design's
            // lint hover, *not* a colored callout.
            span { class: "ds-tt-ico", style: "color:var(--red2);", Icon { name: IconName::ErrCircle, size: IconSize::Sm } }
            Prose { class: "ds-tt-msg", "{msg}" }
        }
    }
}

// Editor text metrics for pointer→(line,col) hit-testing. Fixed by the `.ps-sql` CSS:
// 19px line-height, JetBrains Mono 12px (0.6em advance → 7.2px per char). The layers
// carry an 8px left text padding (`padding: 0 8px`).
const ED_LINE_H: f64 = 19.0;
const ED_CH_W: f64 = 7.2;
const ED_PAD_X: f64 = 8.0;

/// Hit-test a pointer position (element coords, px) against the diagnostics: return the
/// first whose squiggle box covers it, as `(message, line, col)` of the token start.
/// Column is approximate (monospace metric) but the box is at least one char wide and
/// diagnostics are sparse, so it's forgiving enough.
fn hovered_lint(
    sql: &str,
    problems: &[crate::diagnostics::Diagnostic],
    x: f64,
    y: f64,
) -> Option<(String, usize, usize)> {
    if x < ED_PAD_X || y < 0.0 {
        return None;
    }
    let line = (y / ED_LINE_H).floor() as usize;
    let col = ((x - ED_PAD_X) / ED_CH_W).floor() as usize;
    for d in problems {
        let range = match &d.span {
            Some(r) => r.clone(),
            None => continue,
        };
        let (dl, dc) = line_col(sql, range.start);
        let (el, ec) = line_col(sql, range.end);
        // Width to the token's end on its start line (rest-of-line for multi-line spans).
        let end_col = if el == dl { ec.max(dc + 1) } else { usize::MAX };
        if line == dl && col >= dc && col < end_col {
            return Some((d.message.clone(), dl, dc));
        }
    }
    None
}

/// Recompute completions at `caret`, **debounced 500ms** (like validation) so the
/// popup doesn't flash on every keystroke. Shows only while typing a word.
fn refresh_completion(
    ws: Store<crate::session::Workspace>,
    mut comp: Signal<Option<Completing>>,
    mut comp_gen: Signal<u64>,
    caret: usize,
) {
    let sql = ws.sql().cloned();
    // "Typing a word" = the char before the caret continues an identifier (per the
    // parser dialect), or is `.` (member access → re-trigger for that table's columns).
    let typing = sql
        .get(..caret)
        .and_then(|s| s.chars().last())
        .map(|c| crate::sql::is_word_char(c) || c == '.')
        .unwrap_or(false);
    if !typing {
        // A word boundary (space, punctuation, …) dismisses the popup immediately.
        close_completion(comp, comp_gen);
        return;
    }
    let catalog = {
        let store = crate::project::store();
        let st = store.peek();
        Catalog::build(&st.tables, &st.views, crate::engine::Engine::functions())
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

/// Close the popup **and invalidate any pending debounced completion** — otherwise a
/// task scheduled by the previous keystroke fires ~500ms later and re-opens it (why
/// space / click-away appeared not to close it).
fn close_completion(mut comp: Signal<Option<Completing>>, mut comp_gen: Signal<u64>) {
    *comp_gen.write() += 1;
    comp.set(None);
}

/// Popup keyboard nav; `prevent_default`s when it consumes the key.
fn handle_completion_key(
    ws: Store<crate::session::Workspace>,
    mut comp: Signal<Option<Completing>>,
    comp_gen: Signal<u64>,
    e: KeyboardEvent,
) {
    let Some(mut c) = comp.peek().clone() else {
        return;
    };
    let n = c.items.len();
    match e.key() {
        // prevent_default() first, before any signal writes / async work, so the
        // textarea's own handling (caret move, Tab insert, newline) is suppressed.
        Key::ArrowDown => {
            e.prevent_default();
            c.sel = (c.sel + 1) % n;
            comp.set(Some(c));
        }
        Key::ArrowUp => {
            e.prevent_default();
            c.sel = (c.sel + n - 1) % n;
            comp.set(Some(c));
        }
        Key::Enter | Key::Tab => {
            e.prevent_default();
            let item = c.items[c.sel].clone();
            apply_completion(ws, &item);
            close_completion(comp, comp_gen);
        }
        Key::Escape => {
            e.prevent_default();
            close_completion(comp, comp_gen);
        }
        // Space dismisses the popup and is NOT inserted (you don't want it doubled).
        Key::Character(s) if s == " " => {
            close_completion(comp, comp_gen);
            e.prevent_default();
        }
        // Any non-word character (per the dialect) dismisses the popup — punctuation,
        // operators, brackets, `.`, `;`, quotes — but still types. A following `.`
        // re-opens completion for that table's columns.
        Key::Character(s)
            if s.chars()
                .next()
                .map_or(false, |c| !crate::sql::is_word_char(c)) =>
        {
            close_completion(comp, comp_gen);
        }
        // Word characters: leave the popup open — refresh_completion updates it.
        _ => {}
    }
}

/// Replace the completion's `replace` span in the tab's SQL with its insert text, then
/// restore the caret to the end of the inserted word — the controlled-value reset would
/// otherwise jump it to the end of the buffer (so the next space lands there).
fn apply_completion(ws: Store<crate::session::Workspace>, item: &Completion) {
    let mut sql = ws.sql().cloned();
    if item.replace.start > sql.len() || item.replace.end > sql.len() {
        return;
    }
    let caret = item.replace.start + item.insert.len();
    sql.replace_range(item.replace.clone(), &item.insert);
    ws.sql().set(sql);
    // Deferred (setTimeout 0) so it runs *after* dioxus applies the controlled value
    // update to the textarea (that update is an async webview message; a bare rAF can
    // race ahead of it). Byte offset == UTF-16 offset for ASCII SQL.
    let js = format!(
        "setTimeout(() => {{ const t = document.querySelector('.dxc-editor-input:focus') \
         || document.activeElement; \
         if (t && t.classList && t.classList.contains('dxc-editor-input')) \
         t.setSelectionRange({caret}, {caret}); }}, 0);"
    );
    spawn(async move {
        let _ = dioxus::document::eval(&js).await;
    });
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
