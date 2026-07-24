use std::{
    borrow::Cow,
    fmt::Display,
    ops::{
        Mul,
        Range,
    },
    time::Duration,
};

use freya_core::{
    elements::paragraph::ParagraphHolderInner,
    prelude::*,
};
use freya_edit::*;
use ropey::Rope;
use tree_sitter::InputEdit;

use crate::{
    languages::EditorLanguage,
    metrics::EditorMetrics,
    syntax::InputEditExt,
};

/// Severity of a diagnostic decoration — mapped to a squiggle colour by the
/// [`EditorTheme`](crate::editor_theme::EditorTheme) at render time, so the buffer
/// stays theme-independent (like syntax highlighting).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum DecorationSeverity {
    /// Lowest paint priority.
    Info,
    Warning,
    /// Highest paint priority — wins where ranges overlap.
    Error,
}

/// One diagnostic: a **char** range into the rope, its severity, and the message.
/// Rendered by [`EditorLineUI`](crate::editor_line::EditorLineUI) as a wavy underline
/// on the glyphs it covers, and surfaced by the editor's caret-line panel (the
/// floating "what's wrong here" context under the line being edited).
#[derive(Clone, PartialEq, Debug)]
pub struct Decoration {
    pub range: Range<usize>,
    pub severity: DecorationSeverity,
    pub message: String,
}

pub struct CodeEditorData {
    pub(crate) history: EditorHistory,
    pub rope: Rope,
    pub(crate) selection: TextSelection,
    pub(crate) last_saved_history_change: usize,
    pub(crate) metrics: EditorMetrics,
    pub(crate) dragging: TextDragging,
    pub(crate) scrolls: (i32, i32),
    pub(crate) pending_edit: Option<InputEdit>,
    pub language: Option<EditorLanguage>,
    /// The type last handed to [`measure`](Self::measure) (the mounted editor's
    /// theme-resolved font), so programmatic rewrites ([`set_text`](Self::set_text))
    /// can re-measure without their caller knowing the editor's type. `None` until
    /// first mounted (headless data never measures).
    pub(crate) measured_font: Option<(f32, String, i32)>,
    /// The editing-action chords (select all / copy / cut / paste / undo / redo)
    /// `process_key` responds to — the freya-edit layer matches these instead of
    /// hardcoding ⌘A/⌘C/⌘X/⌘V/⌘Z/⌘Y, so the app can drive them from its own
    /// configurable shortcuts (see [`set_edit_bindings`](Self::set_edit_bindings)).
    pub(crate) bindings: EditBindings,
    /// Diagnostic squiggles (char ranges), replaced wholesale by each validation pass
    /// (see [`set_decorations`](Self::set_decorations)). May lag the rope briefly while
    /// typing — rendering intersects them with real line spans, so drift is clamped.
    pub(crate) decorations: Vec<Decoration>,
    /// Monotonic **text version**: bumped by every rope mutation (typing, paste,
    /// delete, undo/redo, programmatic set). Deliberately not the history's change
    /// counter — that one is transaction-*grouped* (a typing burst within the merge
    /// window is one transaction), which is undo granularity, not text identity.
    pub(crate) revision: u64,
    /// The pointer's rest on a decorated span — what the hover popup keys off
    /// (`None` = no popup). Maintained by the per-line pointer handlers via
    /// [`update_hover`](Self::update_hover).
    pub(crate) hover: Option<Hover>,
}

/// Where the diagnostics popup anchors: the hovered char (whose covering decorations
/// it lists), its line, and the pointer's x within the line's paragraph. Frozen while
/// the pointer stays on the same decoration, so the popup doesn't chase the mouse.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct Hover {
    pub char: usize,
    pub deco: usize,
    pub line: usize,
    pub x: f32,
}

impl CodeEditorData {
    pub fn new(rope: Rope, language: impl Into<Option<EditorLanguage>>) -> Self {
        let mut data = Self {
            rope,
            selection: TextSelection::new_cursor(0),
            history: EditorHistory::new(Duration::from_secs(1)),
            last_saved_history_change: 0,
            metrics: EditorMetrics::new(),
            dragging: TextDragging::default(),
            scrolls: (0, 0),
            pending_edit: None,
            language: language.into(),
            measured_font: None,
            bindings: EditBindings::default(),
            decorations: Vec::new(),
            revision: 0,
            hover: None,
        };
        data.configure_highlighter();
        data
    }

    /// Reconfigures the highlighter with the current language. Highlighting is theme-independent;
    /// colours are applied by the editor at render time.
    fn configure_highlighter(&mut self) {
        self.metrics
            .highlighter
            .set_language(self.language.as_ref());
    }

    /// Sets the language used for syntax highlighting, or disables it with `None`.
    pub fn set_language(&mut self, language: impl Into<Option<EditorLanguage>>) {
        self.language = language.into();
        self.configure_highlighter();
    }

    pub fn is_edited(&self) -> bool {
        self.history.current_change() != self.last_saved_history_change
    }

    /// A marker of the buffer's **text** state: it moves on every edit / undo / redo
    /// but not on cursor or selection changes — what validation's change-gate
    /// compares, so caret traffic never re-validates. Per **mutation**, not per
    /// history transaction: the history groups a typing burst into one undo step,
    /// which would make a burst read as a single change and starve the change-gate.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn mark_as_saved(&mut self) {
        self.last_saved_history_change = self.history.current_change();
    }

    pub fn parse(&mut self) {
        let edit = self.pending_edit.take();
        self.metrics.run_parser(&self.rope, edit);
    }

    pub fn measure(&mut self, font_size: f32, font_family: &str, font_weight: i32) {
        self.measured_font = Some((font_size, font_family.to_string(), font_weight));
        self.metrics
            .measure_longest_line(font_size, font_family, font_weight, &self.rope);
    }

    /// Replace the whole buffer with `text` — the programmatic edit behind Format /
    /// Clear. Goes through the normal edit path (history-tracked, so undo restores the
    /// previous text), collapses the cursor to the end, and rebuilds highlighting +
    /// the longest-line measurement from scratch (a whole-buffer rewrite has no useful
    /// incremental state).
    pub fn set_text(&mut self, text: &str) {
        // A programmatic rewrite is its own undo step (and its own history change, so
        // dirty tracking sees it) — never merged into the last typing burst.
        self.history.seal_transaction();
        let len = self.rope.len_utf16_cu();
        if len > 0 {
            self.remove(0..len);
        }
        if !text.is_empty() {
            self.insert(text, 0);
        }
        self.selection = TextSelection::new_cursor(self.rope.len_utf16_cu());
        self.dragging = TextDragging::default();
        self.pending_edit = None;
        // A whole-buffer rewrite invalidates every span; validation re-derives them.
        self.decorations.clear();
        self.hover = None;
        self.metrics.highlighter.invalidate_tree();
        self.parse();
        if let Some((size, family, weight)) = self.measured_font.clone() {
            self.measure(size, &family, weight);
        }
    }

    /// Replace the diagnostic squiggles wholesale from **byte** spans into the current
    /// text (the shape diagnostics arrive in). Spans are clamped to the rope and
    /// converted to char ranges; empty/out-of-range spans are dropped. Returns whether
    /// anything changed (`write_if`-friendly), so a validation pass can apply
    /// unconditionally without spurious re-renders.
    pub fn set_decorations(
        &mut self,
        spans: impl IntoIterator<Item=(Range<usize>, DecorationSeverity, String)>,
    ) -> bool {
        let len_bytes = self.rope.len_bytes();
        let decorations: Vec<Decoration> = spans
            .into_iter()
            .filter_map(|(span, severity, message)| {
                let (start, end) = (span.start.min(len_bytes), span.end.min(len_bytes));
                if start >= end {
                    return None;
                }
                let start = self.rope.byte_to_char(start);
                // `byte_to_char` floors a mid-char byte; keep at least one char covered,
                // clamped back inside the rope.
                let end = self
                    .rope
                    .byte_to_char(end)
                    .max(start + 1)
                    .min(self.rope.len_chars());
                (start < end).then(|| Decoration { range: start..end, severity, message })
            })
            .collect();
        if self.decorations == decorations {
            return false;
        }
        self.decorations = decorations;
        // The spans under the mouse may be gone/moved — don't leave a popup pinned to
        // stale facts; the next pointer move re-establishes it.
        self.hover = None;
        true
    }

    /// Track the mouse for the diagnostics hover popup: `local` is the glyph position
    /// within line `line_index` plus the pointer's x in the paragraph, or `None` when
    /// the pointer left the text. A hover exists only while the pointer sits on a
    /// decorated span — never during a drag-selection — and its anchor freezes while
    /// the pointer stays on the same decoration, so the popup doesn't chase the mouse.
    /// Returns whether it changed (`write_if`-friendly; unchanged moves are free).
    pub fn update_hover(&mut self, line_index: usize, local: Option<(usize, f32)>) -> bool {
        let target = if self.dragging.clicked || self.decorations.is_empty() {
            None
        } else {
            local.and_then(|(local_utf16, x)| {
                let line_start = self.rope.char_to_utf16_cu(self.rope.line_to_char(
                    line_index.min(self.rope.len_lines().saturating_sub(1)),
                ));
                let at = (line_start + local_utf16).min(self.rope.len_utf16_cu());
                let ch = self.rope.utf16_cu_to_char(at);
                self.decorations
                    .iter()
                    .position(|d| d.range.contains(&ch))
                    .map(|deco| Hover { char: ch, deco, line: line_index, x })
            })
        };
        match (&self.hover, &target) {
            (None, None) => false,
            // Same decoration under the pointer — keep the frozen anchor.
            (Some(h), Some(t)) if h.deco == t.deco => false,
            _ => {
                self.hover = target;
                true
            }
        }
    }

    /// Replace the editing-action chords [`process`](Self::process) responds to.
    /// Returns whether they changed (`write_if`-friendly), so a settings sync can run
    /// unconditionally without spurious re-renders.
    pub fn set_edit_bindings(&mut self, bindings: EditBindings) -> bool {
        if self.bindings == bindings {
            return false;
        }
        self.bindings = bindings;
        true
    }

    /// Undo the last edit through the full edit path — history revert, selection
    /// restore, re-highlight, re-measure. The programmatic twin of the keystroke path,
    /// for dispatch that arrives outside the keyboard (an Edit menu item, a command
    /// palette action). Returns whether anything changed.
    pub fn undo_edit(&mut self) -> bool {
        let Some(selection) = TextEditor::undo(self) else {
            return false;
        };
        self.selection = selection;
        self.refresh_after_history();
        true
    }

    /// Redo the last undone edit (see [`undo_edit`](Self::undo_edit)).
    pub fn redo_edit(&mut self) -> bool {
        let Some(selection) = TextEditor::redo(self) else {
            return false;
        };
        self.selection = selection;
        self.refresh_after_history();
        true
    }

    /// Post-history bookkeeping, mirroring what [`process`](Self::process) does after a
    /// `TEXT_CHANGED` key event: re-parse, re-measure with the mounted font, and drop
    /// any in-flight drag state.
    fn refresh_after_history(&mut self) {
        self.parse();
        if let Some((size, family, weight)) = self.measured_font.clone() {
            self.measure(size, &family, weight);
        }
        self.dragging = TextDragging::default();
    }

    pub fn process(
        &mut self,
        font_size: f32,
        font_family: &str,
        font_weight: i32,
        edit_event: EditableEvent,
    ) -> bool {
        let mut processed = false;
        match edit_event {
            EditableEvent::Down {
                location,
                editor_line,
                holder,
            } => {
                let holder = holder.0.borrow();
                let ParagraphHolderInner {
                    paragraph,
                    scale_factor,
                } = holder.as_ref().unwrap();

                let current_selection = self.selection().clone();

                if self.dragging.shift || self.dragging.clicked {
                    self.selection_mut().set_as_range();
                } else {
                    self.clear_selection();
                }

                if &current_selection != self.selection() {
                    processed = true;
                }

                self.dragging.clicked = true;

                let char_position = paragraph.get_glyph_position_at_coordinate(
                    location.mul(*scale_factor).to_i32().to_tuple(),
                );
                let press_selection =
                    self.measure_selection(char_position.position as usize, editor_line);

                let new_selection = match EventsCombos::pressed(location) {
                    PressEventType::Quadruple => {
                        TextSelection::new_range((0, self.rope.len_utf16_cu()))
                    }
                    PressEventType::Triple => {
                        let line = self.char_to_line(press_selection.pos());
                        let line_char = self.line_to_char(line);
                        let line_len = self.line(line).unwrap().utf16_len();
                        TextSelection::new_range((line_char, line_char + line_len))
                    }
                    PressEventType::Double => {
                        let range = self.find_word_boundaries(press_selection.pos());
                        TextSelection::new_range(range)
                    }
                    PressEventType::Single => press_selection,
                };

                if *self.selection() != new_selection {
                    *self.selection_mut() = new_selection;
                    processed = true;
                }
            }
            EditableEvent::Move {
                location,
                editor_line,
                holder,
            } => {
                if self.dragging.clicked {
                    let paragraph = holder.0.borrow();
                    let ParagraphHolderInner {
                        paragraph,
                        scale_factor,
                    } = paragraph.as_ref().unwrap();

                    let dist_position = location.mul(*scale_factor);

                    // Calculate the end of the highlighting
                    let dist_char = paragraph
                        .get_glyph_position_at_coordinate(dist_position.to_i32().to_tuple());
                    let to = dist_char.position as usize;

                    if self.get_selection().is_none() {
                        self.selection_mut().set_as_range();
                        processed = true;
                    }

                    let current_selection = self.selection().clone();

                    let new_selection = self.measure_selection(to, editor_line);

                    // Update the cursor if it has changed
                    if current_selection != new_selection {
                        *self.selection_mut() = new_selection;
                        processed = true;
                    }
                }
            }
            EditableEvent::Release => {
                self.dragging.clicked = false;
            }
            EditableEvent::KeyDown { key, modifiers } => {
                match key {
                    // Handle dragging
                    Key::Named(NamedKey::Shift) => {
                        self.dragging.shift = true;
                    }
                    // Handle editing
                    _ => {
                        let event = self.process_key(key, &modifiers, true, true, true, true);
                        if event.contains(TextEvent::TEXT_CHANGED) {
                            self.parse();
                            self.measure(font_size, font_family, font_weight);
                            self.dragging = TextDragging::default();
                        }
                        if !event.is_empty() {
                            processed = true;
                        }
                    }
                }
            }
            EditableEvent::KeyUp { key, .. } => {
                if *key == Key::Named(NamedKey::Shift) {
                    self.dragging.shift = false;
                }
            }
        };
        processed
    }
}

impl Display for CodeEditorData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.rope.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::EditorLanguage;

    #[test]
    fn set_decorations_maps_bytes_to_chars_and_clamps() {
        let mut data =
            CodeEditorData::new(Rope::from_str("sél x\nfrom t"), None::<EditorLanguage>);

        let msg = || "boom".to_string();
        // "sél" is 4 bytes (é is 2) but 3 chars.
        assert!(data.set_decorations([(0..4, DecorationSeverity::Error, msg())]));
        assert_eq!(
            data.decorations,
            vec![Decoration { range: 0..3, severity: DecorationSeverity::Error, message: msg() }]
        );

        // Re-applying the same spans reports no change (write_if-friendly).
        assert!(!data.set_decorations([(0..4, DecorationSeverity::Error, msg())]));

        // A span past the end of the text is dropped, not panicked on.
        assert!(data.set_decorations([(100..104, DecorationSeverity::Warning, msg())]));
        assert!(data.decorations.is_empty());
    }

    /// The regression behind the "validation stops firing mid-burst" bug: the history
    /// groups a typing burst into one transaction, so its change counter is *undo*
    /// granularity — the revision must move on every mutation regardless.
    #[test]
    fn revision_bumps_per_mutation_not_per_history_transaction() {
        let mut data = CodeEditorData::new(Rope::from_str(""), None::<EditorLanguage>);
        let r0 = data.revision();
        data.insert("se", 0);
        let r1 = data.revision();
        // Same history transaction (within the merge window) — still a new revision.
        data.insert("lct", 2);
        let r2 = data.revision();
        assert!(r0 < r1 && r1 < r2);
        assert_eq!(
            data.history.current_change(),
            1,
            "precondition: the burst merged into one history transaction"
        );

        data.undo_edit();
        let r3 = data.revision();
        assert!(r2 < r3, "undo is a text change too");
    }
}

impl TextEditor for CodeEditorData {
    type LinesIterator<'a>
        = LinesIterator<'a>
    where
        Self: 'a;

    fn lines(&self) -> Self::LinesIterator<'_> {
        unimplemented!("Unused.")
    }

    fn insert_char(&mut self, ch: char, idx: usize) -> usize {
        let idx_utf8 = self.utf16_cu_to_char(idx);
        let selection = self.selection.clone();

        // Capture byte offset and position before mutation for InputEdit.
        let start_byte = self.rope.char_to_byte(idx_utf8);
        let start_line = self.rope.char_to_line(idx_utf8);
        let start_line_byte = self.rope.line_to_byte(start_line);
        let start_col = start_byte - start_line_byte;

        let len_before_insert = self.rope.len_utf16_cu();
        self.rope.insert_char(idx_utf8, ch);
        let len_after_insert = self.rope.len_utf16_cu();

        let inserted_text_len = len_after_insert - len_before_insert;

        // Compute new end position after insertion.
        let new_end_char = idx_utf8 + 1; // one char inserted
        let new_end_byte = self.rope.char_to_byte(new_end_char);
        let new_end_line = self.rope.char_to_line(new_end_char);
        let new_end_line_byte = self.rope.line_to_byte(new_end_line);
        let new_end_col = new_end_byte - new_end_line_byte;

        self.pending_edit = Some(InputEdit::new_edit(
            start_byte,
            start_byte,
            new_end_byte,
            (start_line, start_col),
            (start_line, start_col),
            (new_end_line, new_end_col),
        ));

        self.history.push_change(HistoryChange::InsertChar {
            idx,
            ch,
            len: inserted_text_len,
            selection,
        });
        self.revision += 1;

        inserted_text_len
    }

    fn insert(&mut self, text: &str, idx: usize) -> usize {
        let idx_utf8 = self.utf16_cu_to_char(idx);
        let selection = self.selection.clone();

        // Capture byte offset and position before mutation for InputEdit.
        let start_byte = self.rope.char_to_byte(idx_utf8);
        let start_line = self.rope.char_to_line(idx_utf8);
        let start_line_byte = self.rope.line_to_byte(start_line);
        let start_col = start_byte - start_line_byte;

        let len_before_insert = self.rope.len_utf16_cu();
        self.rope.insert(idx_utf8, text);
        let len_after_insert = self.rope.len_utf16_cu();

        let inserted_text_len = len_after_insert - len_before_insert;

        // Compute new end position after insertion.
        let inserted_chars = text.chars().count();
        let new_end_char = idx_utf8 + inserted_chars;
        let new_end_byte = self.rope.char_to_byte(new_end_char);
        let new_end_line = self.rope.char_to_line(new_end_char);
        let new_end_line_byte = self.rope.line_to_byte(new_end_line);
        let new_end_col = new_end_byte - new_end_line_byte;

        self.pending_edit = Some(InputEdit::new_edit(
            start_byte,
            start_byte,
            new_end_byte,
            (start_line, start_col),
            (start_line, start_col),
            (new_end_line, new_end_col),
        ));

        self.history.push_change(HistoryChange::InsertText {
            idx,
            text: text.to_owned(),
            len: inserted_text_len,
            selection,
        });
        self.revision += 1;

        inserted_text_len
    }

    fn remove(&mut self, range_utf16: Range<usize>) -> usize {
        let range =
            self.utf16_cu_to_char(range_utf16.start)..self.utf16_cu_to_char(range_utf16.end);
        let text = self.rope.slice(range.clone()).to_string();
        let selection = self.selection.clone();

        // Capture byte offsets and positions before mutation for InputEdit.
        let start_byte = self.rope.char_to_byte(range.start);
        let old_end_byte = self.rope.char_to_byte(range.end);
        let start_line = self.rope.char_to_line(range.start);
        let start_line_byte = self.rope.line_to_byte(start_line);
        let start_col = start_byte - start_line_byte;
        let old_end_line = self.rope.char_to_line(range.end);
        let old_end_line_byte = self.rope.line_to_byte(old_end_line);
        let old_end_col = old_end_byte - old_end_line_byte;

        let len_before_remove = self.rope.len_utf16_cu();
        self.rope.remove(range);
        let len_after_remove = self.rope.len_utf16_cu();

        let removed_text_len = len_before_remove - len_after_remove;

        // After removal, new_end == start (the removed range collapses to a point).
        self.pending_edit = Some(InputEdit::new_edit(
            start_byte,
            old_end_byte,
            start_byte,
            (start_line, start_col),
            (old_end_line, old_end_col),
            (start_line, start_col),
        ));

        self.history.push_change(HistoryChange::Remove {
            idx: range_utf16.end - removed_text_len,
            text,
            len: removed_text_len,
            selection,
        });
        self.revision += 1;

        removed_text_len
    }

    fn char_to_line(&self, char_idx: usize) -> usize {
        self.rope.char_to_line(char_idx)
    }

    fn line_to_char(&self, line_idx: usize) -> usize {
        self.rope.line_to_char(line_idx)
    }

    fn utf16_cu_to_char(&self, utf16_cu_idx: usize) -> usize {
        self.rope.utf16_cu_to_char(utf16_cu_idx)
    }

    fn char_to_utf16_cu(&self, idx: usize) -> usize {
        self.rope.char_to_utf16_cu(idx)
    }

    fn line(&self, line_idx: usize) -> Option<Line<'_>> {
        let line = self.rope.get_line(line_idx);

        line.map(|line| Line {
            text: Cow::Owned(line.to_string()),
            utf16_len: line.len_utf16_cu(),
        })
    }

    fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    fn len_utf16_cu(&self) -> usize {
        self.rope.len_utf16_cu()
    }

    fn has_any_selection(&self) -> bool {
        self.selection.is_range()
    }

    fn get_selection(&self) -> Option<(usize, usize)> {
        match self.selection {
            TextSelection::Cursor(_) => None,
            TextSelection::Range { from, to } => Some((from, to)),
        }
    }

    fn set(&mut self, text: &str) {
        self.rope.remove(0..);
        self.rope.insert(0, text);
        self.revision += 1;
    }

    fn clear_selection(&mut self) {
        let end = self.selection().end();
        self.selection_mut().set_as_cursor();
        self.selection_mut().move_to(end);
    }

    fn set_selection(&mut self, (from, to): (usize, usize)) {
        self.selection = TextSelection::Range { from, to };
    }

    fn get_selected_text(&self) -> Option<String> {
        let (start, end) = self.get_selection_range()?;

        Some(self.rope.get_slice(start..end)?.to_string())
    }

    fn get_selection_range(&self) -> Option<(usize, usize)> {
        let (start, end) = match self.selection {
            TextSelection::Cursor(_) => return None,
            TextSelection::Range { from, to } => (from, to),
        };

        // Use left-to-right selection
        let (start, end) = if start < end {
            (start, end)
        } else {
            (end, start)
        };

        Some((start, end))
    }

    fn edit_bindings(&self) -> &EditBindings {
        &self.bindings
    }

    fn undo(&mut self) -> Option<TextSelection> {
        // Undo can make arbitrary changes — invalidate the tree for a full re-parse.
        self.pending_edit = None;
        self.metrics.highlighter.invalidate_tree();
        let undone = self.history.undo(&mut self.rope);
        if undone.is_some() {
            self.revision += 1;
        }
        undone
    }

    fn redo(&mut self) -> Option<TextSelection> {
        // Redo can make arbitrary changes — invalidate the tree for a full re-parse.
        self.pending_edit = None;
        self.metrics.highlighter.invalidate_tree();
        let redone = self.history.redo(&mut self.rope);
        if redone.is_some() {
            self.revision += 1;
        }
        redone
    }

    fn editor_history(&mut self) -> &mut EditorHistory {
        &mut self.history
    }

    fn selection(&self) -> &TextSelection {
        &self.selection
    }

    fn selection_mut(&mut self) -> &mut TextSelection {
        &mut self.selection
    }

    fn get_indentation(&self) -> u8 {
        4
    }
}
