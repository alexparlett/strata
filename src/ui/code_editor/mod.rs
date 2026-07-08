//! Vendored code editor (S26a) — a controlled **textarea-over-highlight** surface,
//! based on `dioxus-code-editor` 0.1.2, **desktop-only**, extended with the surface the
//! upstream crate doesn't expose:
//!
//! - **`oncaret`** — the caret byte offset after each edit (derived from the input
//!   diff, no JS: the changed range's new end *is* the caret). Enough to drive
//!   completion-on-type (S7); click/arrow caret moves are a later `eval` refinement.
//! - **`onkeydown`** — pass-through so the completion popup can intercept
//!   ↑/↓/Enter/Esc/Tab before the textarea acts.
//! - **`decorations`** — an overlay layer of squiggles/marks by byte range (S25),
//!   positioned with monospace `ch` metrics over the same text.
//!
//! Built on `dioxus_code::advanced` (`Buffer`/`TokenSpan`/`CodeThemeStyles`) — the same
//! public blocks the upstream component uses. Base layout CSS lives in `assets/main.css`
//! (`.dxc-editor*`), replacing the crate's injected stylesheet. See
//! `docs/SQL_LANGUAGE_SPEC.md` §1a and `docs/EDITOR_LANG_SPIKE.md`.

use std::cell::RefCell;
use std::ops::Range;
use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_code::advanced::{Buffer, CodeThemeStyles, TokenSpan};
use dioxus_code::{CodeTheme, Language, SourceCode};

/// A visual mark over a byte range of the source (S25 squiggles; future gutter marks).
#[derive(Clone, PartialEq)]
pub struct Decoration {
    pub range: Range<usize>,
    pub severity: crate::diagnostics::Severity,
}

#[derive(Props, Clone, PartialEq)]
pub struct CodeEditorProps {
    #[props(into)]
    pub value: String,
    #[props(default = Language::Rust)]
    pub language: Language,
    #[props(default, into)]
    pub theme: CodeTheme,
    #[props(default = true)]
    pub line_numbers: bool,
    #[props(default = false)]
    pub read_only: bool,
    #[props(default = false)]
    pub spellcheck: bool,
    #[props(into, default)]
    pub placeholder: String,
    #[props(into, default)]
    pub class: String,
    /// Called with the full editor text after each input event.
    #[props(default = EventHandler::new(|_: String| {}))]
    pub oninput: EventHandler<String>,
    /// Called with the caret byte offset after each input (derived from the diff).
    #[props(default = EventHandler::new(|_: usize| {}))]
    pub oncaret: EventHandler<usize>,
    /// Raw keydown on the textarea (for the completion popup to intercept nav keys).
    #[props(default = EventHandler::new(|_: KeyboardEvent| {}))]
    pub onkeydown: EventHandler<KeyboardEvent>,
    /// Textarea blur — the consumer uses this to dismiss the completion popup on
    /// click-off. (Clicking the popup itself `prevent_default`s mousedown, so focus —
    /// and thus this — isn't lost.)
    #[props(default = EventHandler::new(|_: FocusEvent| {}))]
    pub onblur: EventHandler<FocusEvent>,
    /// Squiggle/mark overlays by byte range.
    #[props(default)]
    pub decorations: Vec<Decoration>,
    /// Extra nodes rendered *inside* the viewport (after the textarea) — used for the
    /// completion popup, which needs the editor's text coordinate system and the
    /// `--dxc-editor-*` CSS vars (both absent outside `.dxc-editor`).
    pub children: Element,
}

struct EditorBuffer {
    buffer: Option<Buffer>,
    language: Language,
    /// Last value we rendered — diffed against the next input to find the caret.
    last: String,
}

/// Editable syntax-highlighted code surface (vendored, extended).
#[component]
pub fn CodeEditor(props: CodeEditorProps) -> Element {
    let state = use_hook({
        let value = props.value.clone();
        let language = props.language;
        move || {
            Rc::new(RefCell::new(EditorBuffer {
                buffer: Buffer::new(language, value.clone()).ok(),
                language,
                last: value,
            }))
        }
    });

    // Re-highlight for the current value (replace-based; simple + robust for our sizes).
    let snapshot = {
        let mut slot = state.borrow_mut();
        if slot.language != props.language {
            slot.buffer = Buffer::new(props.language, props.value.clone()).ok();
            slot.language = props.language;
        }
        match slot.buffer.as_mut() {
            Some(buffer) => {
                if buffer.source() != props.value {
                    let _ = buffer.replace(props.value.clone());
                }
                buffer.highlighted()
            }
            None => SourceCode::new(props.language, props.value.clone()).into(),
        }
    };
    let lines = snapshot.lines();
    let line_count = lines.len();
    let class = editor_class(props.theme.clone(), props.line_numbers, &props.class);
    let textarea_value = props.value.clone();

    // Caret from the input diff: the byte offset where old and new first differ, plus
    // the inserted length — i.e. the end of the change = the caret. No DOM/eval.
    let state_for_input = state.clone();
    let oninput = props.oninput;
    let oncaret = props.oncaret;
    let on_input = move |ev: FormEvent| {
        let new_value = ev.value();
        let caret = {
            let mut slot = state_for_input.borrow_mut();
            let c = caret_after_diff(&slot.last, &new_value);
            slot.last = new_value.clone();
            c
        };
        oninput.call(new_value);
        oncaret.call(caret);
    };
    let onkeydown = props.onkeydown;
    let onblur = props.onblur;

    // Decoration overlay: single-line squiggles positioned by (line, col) in ch/em.
    let decos = decoration_boxes(&props.value, &props.decorations);

    rsx! {
        CodeThemeStyles { theme: props.theme.clone() }
        div { class: "{class}",
            if props.line_numbers {
                div { class: "dxc-editor-gutter", aria_hidden: "true",
                    for index in 0..line_count {
                        div { class: "dxc-editor-gutter-line", "{index + 1}" }
                    }
                }
            }
            div { class: "dxc-editor-viewport",
                div { class: "dxc-editor-highlight", aria_hidden: "true",
                    for line in lines {
                        div { class: "dxc-editor-line",
                            for segment in line {
                                if let Some(tag) = segment.tag() {
                                    TokenSpan { text: segment.text(), tag }
                                } else {
                                    span { "{segment.text()}" }
                                }
                            }
                        }
                    }
                }
                // Squiggle overlay (under the textarea, over the highlight).
                div { class: "dxc-editor-decorations", aria_hidden: "true",
                    for (i, d) in decos.into_iter().enumerate() {
                        div {
                            key: "d{i}",
                            class: "dxc-squiggle {d.class}",
                            style: "top:calc(var(--dxc-editor-line-height) * {d.line});left:calc({d.col}ch + 8px);width:{d.width}ch;",
                        }
                    }
                }
                textarea {
                    class: "dxc-editor-input",
                    readonly: props.read_only,
                    spellcheck: props.spellcheck,
                    wrap: "off",
                    placeholder: props.placeholder,
                    value: "{textarea_value}",
                    oninput: on_input,
                    onkeydown: move |e: KeyboardEvent| onkeydown.call(e),
                    onblur: move |e: FocusEvent| onblur.call(e),
                }
                // Consumer overlay (completion popup) — inside the viewport so it shares
                // the text coordinate system + the `--dxc-editor-*` vars.
                {props.children}
            }
        }
    }
}

fn editor_class(theme: CodeTheme, line_numbers: bool, extra: &str) -> String {
    let mut class = format!("dxc-editor {}", theme.classes());
    if !line_numbers {
        class.push_str(" dxc-editor-no-gutter");
    }
    if !extra.is_empty() {
        class.push(' ');
        class.push_str(extra);
    }
    class
}

/// The caret byte offset after replacing `old` with `new`: end of the changed region
/// in `new` (common prefix length + inserted bytes). Char-boundary safe.
fn caret_after_diff(old: &str, new: &str) -> usize {
    let ob = old.as_bytes();
    let nb = new.as_bytes();
    let mut start = 0;
    let shared = ob.len().min(nb.len());
    while start < shared && ob[start] == nb[start] {
        start += 1;
    }
    // Common suffix length (bounded so it can't cross `start`).
    let mut suf = 0;
    while suf < (nb.len() - start).min(ob.len().saturating_sub(start))
        && ob[ob.len() - 1 - suf] == nb[nb.len() - 1 - suf]
    {
        suf += 1;
    }
    let mut caret = nb.len() - suf;
    while caret < nb.len() && !new.is_char_boundary(caret) {
        caret += 1;
    }
    caret.min(new.len())
}

struct DecoBox {
    line: usize,
    col: usize,
    width: usize,
    class: &'static str,
}

/// Convert byte-range decorations into (line, col, width) boxes. Single-line only for
/// now — a range spanning newlines is drawn on its start line up to the line end.
fn decoration_boxes(sql: &str, decos: &[Decoration]) -> Vec<DecoBox> {
    let starts = line_starts(sql);
    decos
        .iter()
        .filter_map(|d| {
            let (line, col) = line_col(&starts, d.range.start);
            let end_line = line_col(&starts, d.range.end).0;
            let width = if end_line == line {
                line_col(&starts, d.range.end).1.saturating_sub(col).max(1)
            } else {
                // Multi-line: to end of the start line.
                let line_end = starts.get(line + 1).copied().unwrap_or(sql.len());
                line_col(&starts, line_end.saturating_sub(1)).1.saturating_sub(col).max(1)
            };
            let class = match d.severity {
                crate::diagnostics::Severity::Error => "err",
                crate::diagnostics::Severity::Warning => "warn",
                crate::diagnostics::Severity::Info => "info",
            };
            Some(DecoBox {
                line,
                col,
                width,
                class,
            })
        })
        .collect()
}

fn line_starts(sql: &str) -> Vec<usize> {
    let mut v = vec![0usize];
    for (i, b) in sql.bytes().enumerate() {
        if b == b'\n' {
            v.push(i + 1);
        }
    }
    v
}

/// 0-based (line, column) for a byte offset, using char columns (mono ch units).
fn line_col(starts: &[usize], offset: usize) -> (usize, usize) {
    let line = match starts.binary_search(&offset) {
        Ok(l) => l,
        Err(l) => l.saturating_sub(1),
    };
    (line, offset.saturating_sub(starts[line]))
}
