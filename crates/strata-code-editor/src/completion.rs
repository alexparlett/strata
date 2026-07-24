//! Completion popup model — the **generic** editor side of autocomplete. The editor
//! owns trigger detection, keyboard handling, and the popup; a provider callback
//! (wired by the host app) turns a [`CompletionRequest`] into [`CompletionItem`]s.
//! This module is the pure logic: item/state types, the after-edit trigger decision,
//! and the flip/clamp placement math. Rendering + event wiring live in `editor_ui`.
//!
//! The pipeline is deliberately **synchronous**: the provider runs inside the key
//! handler, in the same frame as the edit. There is no debounce, no spawned task, no
//! stale-result race — the popup can never lag or flicker behind the buffer.

use std::ops::Range;

use freya_core::prelude::{Key, Modifiers, NamedKey};

/// What a completion candidate is — drives the glyph chip + its theme colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompletionItemKind {
    Table,
    View,
    Column,
    Function,
    Keyword,
}

impl CompletionItemKind {
    /// The glyph rendered in the row's kind chip.
    pub fn glyph(&self) -> &'static str {
        match self {
            CompletionItemKind::Table => "▤",
            CompletionItemKind::View => "◑",
            CompletionItemKind::Column => "•",
            CompletionItemKind::Function => "ƒ",
            CompletionItemKind::Keyword => "K",
        }
    }
}

/// One completion candidate, as the provider returns it.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    /// Row text (and what filtering matched against).
    pub label: String,
    /// Text inserted on accept (may differ — functions append `(`, odd identifiers
    /// arrive pre-quoted).
    pub insert: String,
    pub kind: CompletionItemKind,
    /// Dim right-aligned annotation (`events · Utf8`, `view`, `keyword`, …).
    pub detail: Option<String>,
    /// **Byte** span of the partial word the accept replaces (`caret..caret` when
    /// the caret isn't inside a word).
    pub replace: Range<usize>,
}

/// What the editor hands the provider. `text` is the full buffer, `caret_byte` the
/// caret as a byte offset into it.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionRequest {
    pub text: String,
    pub caret_byte: usize,
    /// `true` for an explicit trigger (⌃Space / ⌘Space) — providers may widen the
    /// candidate set for a manual ask.
    pub manual: bool,
}

/// The popup's view state. `None` inside = hidden.
#[derive(Clone, Default, PartialEq)]
pub struct CompletionState {
    pub open: Option<OpenCompletion>,
}

#[derive(Clone, PartialEq)]
pub struct OpenCompletion {
    pub items: Vec<CompletionItem>,
    /// Selected row index (reset to 0 on every refilter — predictable, VS Code's
    /// default policy).
    pub selected: usize,
    /// Anchor: the line + char-column of the replace span's **start** (the word
    /// start) — the popup never slides while the tail of the word is typed.
    pub anchor_line: usize,
    pub anchor_col_chars: usize,
    /// Byte span being replaced (mirrors the items' span; kept for caret-leave
    /// detection).
    pub replace: Range<usize>,
}

impl CompletionState {
    pub fn close(&mut self) -> bool {
        let was = self.open.is_some();
        self.open = None;
        was
    }

    /// Move the selection by `delta` with wrap-around.
    pub fn step(&mut self, delta: isize) {
        if let Some(open) = &mut self.open {
            let len = open.items.len() as isize;
            if len > 0 {
                open.selected = ((open.selected as isize + delta % len + len) % len) as usize;
            }
        }
    }
}

/// Identifier characters for trigger/word purposes (matches SQL's notion well
/// enough; the provider's own tokenizer decides the actual replace span).
pub fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What to do with the popup after the editor processed a **text-changing** key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerDecision {
    /// Re-run the provider against the new buffer (opens the popup if it was
    /// closed).
    Recompute,
    /// Close the popup (a word-boundary character was typed).
    Close,
    /// Leave the popup as it is (it was closed; nothing re-opens it).
    None,
}

/// Decide the popup's fate after an edit. `was_open` gates the quieter paths:
/// backspace refilters an open popup but never opens a closed one, and a digit
/// continues a filter but doesn't start one (typing `123` shouldn't pop a list).
pub fn trigger_after_edit(key: &Key, modifiers: Modifiers, was_open: bool) -> TriggerDecision {
    let close_if_open = if was_open {
        TriggerDecision::Close
    } else {
        TriggerDecision::None
    };
    let plain = !modifiers.intersects(Modifiers::META | Modifiers::CONTROL | Modifiers::ALT);
    match key {
        Key::Character(s) if plain => {
            let mut chars = s.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if is_ident_char(c) || c == '.' => {
                    if !was_open && c.is_ascii_digit() {
                        TriggerDecision::None
                    } else {
                        TriggerDecision::Recompute
                    }
                }
                _ => close_if_open,
            }
        }
        Key::Named(NamedKey::Backspace) | Key::Named(NamedKey::Delete) => {
            if was_open {
                TriggerDecision::Recompute
            } else {
                TriggerDecision::None
            }
        }
        _ => close_if_open,
    }
}

/// Place the popup: below the anchor line by default, flipped above when the space
/// below the window bottom is short (and above actually fits), clamped to the
/// window horizontally. All coordinates window-space logical.
pub fn flip_and_clamp(
    anchor_top: f32,
    anchor_bottom: f32,
    popup_h: f32,
    popup_w: f32,
    left: f32,
    root: (f32, f32),
) -> (f32, f32) {
    const GAP: f32 = 3.0;
    const EDGE: f32 = 4.0;
    let (root_w, root_h) = root;
    let below = anchor_bottom + GAP;
    let above = anchor_top - GAP - popup_h;
    let top = if below + popup_h <= root_h - EDGE || above < EDGE {
        below
    } else {
        above
    };
    let left = left.min(root_w - popup_w - EDGE).max(EDGE);
    (top, left)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chr(s: &str) -> Key {
        Key::Character(s.into())
    }

    #[test]
    fn ident_chars_and_dot_recompute() {
        assert_eq!(
            trigger_after_edit(&chr("a"), Modifiers::empty(), false),
            TriggerDecision::Recompute
        );
        assert_eq!(
            trigger_after_edit(&chr("_"), Modifiers::empty(), false),
            TriggerDecision::Recompute
        );
        assert_eq!(
            trigger_after_edit(&chr("."), Modifiers::empty(), false),
            TriggerDecision::Recompute
        );
    }

    #[test]
    fn digits_filter_but_do_not_open() {
        assert_eq!(
            trigger_after_edit(&chr("1"), Modifiers::empty(), false),
            TriggerDecision::None
        );
        assert_eq!(
            trigger_after_edit(&chr("1"), Modifiers::empty(), true),
            TriggerDecision::Recompute
        );
    }

    #[test]
    fn word_boundaries_close_only_an_open_popup() {
        assert_eq!(
            trigger_after_edit(&chr(" "), Modifiers::empty(), true),
            TriggerDecision::Close
        );
        assert_eq!(
            trigger_after_edit(&chr(","), Modifiers::empty(), false),
            TriggerDecision::None
        );
    }

    #[test]
    fn backspace_refilters_open_never_opens_closed() {
        let bs = Key::Named(NamedKey::Backspace);
        assert_eq!(
            trigger_after_edit(&bs, Modifiers::empty(), true),
            TriggerDecision::Recompute
        );
        assert_eq!(
            trigger_after_edit(&bs, Modifiers::empty(), false),
            TriggerDecision::None
        );
    }

    #[test]
    fn modified_chars_are_not_triggers() {
        assert_eq!(
            trigger_after_edit(&chr("v"), Modifiers::META, true),
            TriggerDecision::Close
        );
        assert_eq!(
            trigger_after_edit(&chr("a"), Modifiers::ALT, false),
            TriggerDecision::None
        );
    }

    #[test]
    fn newline_closes() {
        assert_eq!(
            trigger_after_edit(&Key::Named(NamedKey::Enter), Modifiers::empty(), true),
            TriggerDecision::Close
        );
    }

    #[test]
    fn selection_steps_wrap() {
        let mut st = CompletionState {
            open: Some(OpenCompletion {
                items: vec![
                    CompletionItem {
                        label: "a".into(),
                        insert: "a".into(),
                        kind: CompletionItemKind::Column,
                        detail: None,
                        replace: 0..0,
                    };
                    3
                ],
                selected: 0,
                anchor_line: 0,
                anchor_col_chars: 0,
                replace: 0..0,
            }),
        };
        st.step(-1);
        assert_eq!(st.open.as_ref().unwrap().selected, 2);
        st.step(1);
        st.step(1);
        assert_eq!(st.open.as_ref().unwrap().selected, 1);
    }

    #[test]
    fn placement_prefers_below() {
        // Anchor line at y 100..120 in an 800-tall window; 200-tall popup fits below.
        let (top, _) = flip_and_clamp(100.0, 120.0, 200.0, 300.0, 50.0, (1200.0, 800.0));
        assert_eq!(top, 123.0);
    }

    #[test]
    fn placement_flips_up_near_the_bottom() {
        let (top, _) = flip_and_clamp(700.0, 720.0, 200.0, 300.0, 50.0, (1200.0, 800.0));
        assert_eq!(top, 700.0 - 3.0 - 200.0);
    }

    #[test]
    fn placement_stays_below_when_above_cannot_fit_either() {
        // Tiny window: below overflows but above would be off-screen — stay below.
        let (top, _) = flip_and_clamp(10.0, 30.0, 200.0, 300.0, 50.0, (1200.0, 180.0));
        assert_eq!(top, 33.0);
    }

    #[test]
    fn placement_clamps_horizontally() {
        let (_, left) = flip_and_clamp(100.0, 120.0, 200.0, 300.0, 1100.0, (1200.0, 800.0));
        assert_eq!(left, 1200.0 - 300.0 - 4.0);
        let (_, left) = flip_and_clamp(100.0, 120.0, 200.0, 300.0, -20.0, (1200.0, 800.0));
        assert_eq!(left, 4.0);
    }
}
