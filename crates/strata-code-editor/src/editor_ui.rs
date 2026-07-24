use std::borrow::Cow;

use freya_components::{get_theme_or_default, scrollviews::{
    ScrollConfig,
    ScrollController,
    ScrollEvent,
    ScrollView,
    use_scroll_controller,
    VirtualScrollView,
}};
use freya_core::prelude::*;
use freya_edit::{EditableEvent, TextEditor};
use torin::{
    gaps::Gaps,
    position::Position,
    prelude::{Alignment, Area, Content},
    size::Size as TorinSize,
};
use crate::{
    completion::{
        flip_and_clamp,
        is_ident_char,
        trigger_after_edit,
        CompletionItem,
        CompletionRequest,
        CompletionState,
        OpenCompletion,
        TriggerDecision,
    },
    editor_data::{CodeEditorData, DecorationSeverity},
    editor_line::EditorLineUI,
    editor_theme::{
        EditorTheme,
        EditorThemePartial,
        EditorThemePreference,
        EditorSyntaxThemePartial,
    },
};
use crate::editor_theme::EditorSyntaxThemePreference;
use crate::prelude::EditorSyntaxTheme;

#[derive(PartialEq, Clone)]
pub struct CodeEditor {
    editor: Writable<CodeEditorData>,
    /// Per-instance override; the `code_editor` theme supplies the default.
    font_size: Option<f32>,
    /// Per-instance override; the `code_editor` theme supplies the default.
    line_height: Option<f32>,
    read_only: bool,
    gutter: bool,
    show_whitespace: bool,
    highlight_current_line: bool,
    /// Per-instance override; the `code_editor` theme supplies the default.
    font_family: Option<Cow<'static, str>>,
    /// Per-instance override; the `code_editor` theme supplies the default.
    font_weight: Option<i32>,
    a11y_id: AccessibilityId,
    a11y_auto_focus: bool,
    pub(crate) theme: Option<EditorThemePartial>,
    pub(crate) syntax_theme: Option<EditorSyntaxThemePartial>,
    on_pre_key_down: Callback<Event<KeyboardEventData>, bool>,
    /// The completion provider — `None` keeps the whole feature off.
    on_completions: Option<Callback<CompletionRequest, Vec<CompletionItem>>>,
}

impl CodeEditor {
    /// Creates a new editor UI component with the given writable data.
    ///
    /// The editor's type (family · size · weight · line height) comes from the `code_editor`
    /// theme; the builder methods below are per-instance overrides.
    pub fn new(editor: impl Into<Writable<CodeEditorData>>, a11y_id: AccessibilityId) -> Self {
        Self {
            editor: editor.into(),
            font_size: None,
            line_height: None,
            read_only: false,
            gutter: true,
            show_whitespace: true,
            highlight_current_line: true,
            font_family: None,
            font_weight: None,
            a11y_id,
            a11y_auto_focus: false,
            theme: None,
            syntax_theme: None,
            on_pre_key_down: Callback::new(|e: Event<KeyboardEventData>| {
                e.stop_propagation();
                if let Key::Named(NamedKey::Tab) = &e.key {
                    e.prevent_default();
                }
                true
            }),
            on_completions: None,
        }
    }

    /// Overrides the theme's font size.
    pub fn font_size(mut self, size: f32) -> Self {
        self.font_size = Some(size);
        self
    }

    /// Overrides the theme's line height multiplier (relative to font size).
    pub fn line_height(mut self, height: f32) -> Self {
        self.line_height = Some(height);
        self
    }

    /// Overrides the theme's font weight.
    pub fn font_weight(mut self, weight: i32) -> Self {
        self.font_weight = Some(weight);
        self
    }

    /// Sets whether the editor is read-only.
    pub fn read_only(mut self, read_only: bool) -> Self {
        self.read_only = read_only;
        self
    }

    /// Sets whether the gutter (line numbers) is visible.
    pub fn gutter(mut self, gutter: bool) -> Self {
        self.gutter = gutter;
        self
    }

    /// Sets whether leading whitespace characters are rendered visually.
    pub fn show_whitespace(mut self, show_whitespace: bool) -> Self {
        self.show_whitespace = show_whitespace;
        self
    }

    /// Sets whether the line under the cursor is tinted (with `line_selected_background`).
    pub fn highlight_current_line(mut self, highlight_current_line: bool) -> Self {
        self.highlight_current_line = highlight_current_line;
        self
    }

    /// Overrides the theme's font family.
    pub fn font_family(mut self, font_family: impl Into<Cow<'static, str>>) -> Self {
        self.font_family = Some(font_family.into());
        self
    }

    /// Sets whether the editor automatically receives focus.
    pub fn a11y_auto_focus(mut self, a11y_auto_focus: bool) -> Self {
        self.a11y_auto_focus = a11y_auto_focus;
        self
    }

    /// Sets a pre-handler called for each key event. Return `true` to let the editor process it,
    /// `false` to skip. The callback may call `stop_propagation()` / `prevent_default()` directly.
    pub fn on_pre_key_down(
        mut self,
        on_pre_key_down: impl Into<Callback<Event<KeyboardEventData>, bool>>,
    ) -> Self {
        self.on_pre_key_down = on_pre_key_down.into();
        self
    }

    /// Wires the completion provider: called **synchronously** whenever the buffer
    /// changes at a completion-worthy position (identifier chars, `.`, backspace
    /// while open) or on the manual trigger (⌃Space / ⌘Space). The editor owns the
    /// popup, its keys (↑/↓/Enter/Tab/Esc while open), placement + flip-up, and the
    /// accept edit; the provider only maps `(text, caret)` → candidates.
    pub fn on_completions(
        mut self,
        on_completions: impl Into<Callback<CompletionRequest, Vec<CompletionItem>>>,
    ) -> Self {
        self.on_completions = Some(on_completions.into());
        self
    }
}

impl Component for CodeEditor {
    fn render(&self) -> impl IntoElement {
        let CodeEditor {
            editor,
            font_size,
            line_height,
            read_only,
            gutter,
            show_whitespace,
            highlight_current_line,
            font_family,
            font_weight,
            a11y_id,
            a11y_auto_focus,
            theme,
            syntax_theme,
            on_pre_key_down,
            on_completions,
        } = self.clone();

        let theme = get_theme_or_default!(&theme, EditorThemePreference, "code_editor", || {
            EditorTheme::light().into()
        });

        let syntax_theme = get_theme_or_default!(&syntax_theme, EditorSyntaxThemePreference, "code_editor_syntax", || {
            EditorSyntaxTheme::light().into()
        });

        // The effective type: the `code_editor` theme's, unless the builder overrode it.
        let font_size = font_size.unwrap_or(theme.font_size);
        let font_weight = font_weight.unwrap_or(theme.font_weight);
        let font_family: Cow<'static, str> = font_family
            .unwrap_or_else(|| Cow::Owned(theme.font_family.clone()));
        let line_height = line_height.unwrap_or(theme.line_height);

        // Seed the metrics with the resolved type at mount — the editor owns its measurement
        // (callers don't know the theme's font). Edits re-measure through `process`.
        use_hook({
            let mut editor = editor.clone();
            let font_family = font_family.clone();
            move || {
                editor.write().measure(font_size, &font_family, font_weight);
            }
        });

        // Completion popup state (created before the scroll controller so its
        // on-scroll callback can close the popup — a wheel/scrollbar move detaches
        // the anchor from the text, and closing is the predictable answer).
        let completion = use_state(CompletionState::default);
        // Measurement plumbing: the editor's own window-space rect (anchor origin),
        // the popup's rect (outside-press detection), and each row's rect
        // (keyboard scroll-into-view). torin re-emits `Sized` on scroll, so these
        // stay fresh without any manual bookkeeping.
        let editor_area = use_state(|| None::<Area>);
        let popup_area = use_state(|| None::<Area>);
        let row_areas = use_state(Vec::<Option<Area>>::new);
        let popup_scroll = use_scroll_controller(ScrollConfig::default);

        // Keyboard navigation reveals the selected row: keyed on the *index* memo —
        // never the row areas themselves (scroll re-emits them; peeking avoids the
        // loop, exactly the tab-strip idiom).
        let selected_index = use_memo({
            let completion = completion;
            move || completion.read().open.as_ref().map(|o| o.selected)
        });
        use_side_effect({
            let mut popup_scroll = popup_scroll;
            move || {
                if let Some(sel) = *selected_index.read() {
                    if let Some(Some(area)) = row_areas.peek().get(sel) {
                        popup_scroll.scroll_to_item(*area);
                    }
                }
            }
        });

        let editor_data = editor.read();

        let scroll_controller = use_hook(|| {
            let notifier = State::create(());
            let requests = State::create(vec![]);
            ScrollController::managed(
                notifier,
                requests,
                State::create(Callback::new({
                    let mut editor = editor.clone();
                    let mut completion = completion;
                    move |ev| {
                        let changed = editor.write_if(|mut editor| {
                            let current = editor.scrolls;
                            match ev {
                                ScrollEvent::X(x) => {
                                    editor.scrolls.0 = x;
                                }
                                ScrollEvent::Y(y) => {
                                    editor.scrolls.1 = y;
                                }
                            }
                            current != editor.scrolls
                        });
                        if changed && completion.peek().open.is_some() {
                            completion.write().close();
                        }
                        changed
                    }
                })),
                State::create(Callback::new({
                    let editor = editor.clone();
                    move |_| {
                        let editor = editor.read();
                        editor.scrolls
                    }
                })),
            )
        });

        let line_height = (font_size * line_height).floor();
        let lines_len = editor_data.metrics.syntax_blocks.len();

        // The editor's viewport size — the flip/clamp bounds for the hover popup.
        let viewport = use_state(|| (0.0f32, 0.0f32));

        // The diagnostics hover popup: only while the pointer sits on a decorated
        // span (`hover`, maintained by the per-line pointer handlers), showing every
        // diagnostic covering that spot. Pointer off the span / out of the editor /
        // a new validation pass → gone. Opens bottom-right of the pointer; flips
        // above near the bottom edge and to the pointer's left near the right edge.
        let hover = editor_data.hover.clone();
        let hover_msgs: Vec<(DecorationSeverity, String)> = hover
            .as_ref()
            .map(|h| {
                editor_data
                    .decorations
                    .iter()
                    .filter(|d| d.range.contains(&h.char))
                    .map(|d| (d.severity, d.message.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // Built eagerly (the scroll-view closure below consumes `theme`). Absolute
        // offsets resolve against the parent's *inner* (padded) area — the same
        // origin the line rows stack from — so coordinates are line offset + the
        // current scrolls, no padding term. Flip/clamp works off estimated panel
        // metrics — layout hasn't run yet, and a few px of slack is invisible at
        // tooltip scale.
        // Suppressed while the completion popup is open (design rule — the two
        // overlays would collide at the caret line).
        let diagnostics_panel: Option<Element> = hover
            .filter(|_| !hover_msgs.is_empty() && completion.read().open.is_none())
            .map(|h| {
            const PANEL_MAX_W: f32 = 380.0;
            const TEXT_SIZE: f32 = 12.0;
            const ROW_H: f32 = 16.0;
            let (viewport_w, viewport_h) = *viewport.read();
            let panel_w_cap = if viewport_w > 0.0 {
                PANEL_MAX_W.min((viewport_w - 16.0).max(160.0))
            } else {
                PANEL_MAX_W
            };
            let text_w_cap = panel_w_cap - 46.0; // dot + spacing + padding + border
            // Estimated wrapped size (~6.5px per char at 12px UI type).
            let (mut est_w, mut est_h) = (0.0f32, 14.0f32);
            for (_, message) in &hover_msgs {
                let text_w = message.chars().count() as f32 * 6.5;
                est_w = est_w.max(text_w.min(text_w_cap) + 46.0);
                est_h += (text_w / text_w_cap).ceil().max(1.0) * ROW_H + 4.0;
            }

            let gutter_offset = crate::editor_line::gutter_offset(font_size, gutter);
            let pointer_x = editor_data.scrolls.0 as f32 + gutter_offset + h.x;
            let scroll_y = editor_data.scrolls.1 as f32;
            let line_top = h.line as f32 * line_height + scroll_y;

            let below = line_top + line_height + 2.0;
            let top = if viewport_h > 0.0
                && below + est_h > viewport_h - 4.0
                && line_top - est_h - 2.0 > 2.0
            {
                line_top - est_h - 2.0
            } else {
                below
            };
            let mut left = pointer_x + 6.0;
            if viewport_w > 0.0 && left + est_w > viewport_w - 4.0 {
                left = pointer_x - est_w - 6.0;
            }
            let left = left.max(4.0);

            rect()
                .position(Position::new_absolute().top(top).left(left))
                .background(theme.panel_background)
                .border(Border::new().width(1.).fill(theme.panel_border))
                .corner_radius(8.)
                .padding(Gaps::new(6., 10., 6., 10.))
                .spacing(4.)
                .max_width(TorinSize::px(panel_w_cap))
                .children(hover_msgs.into_iter().map(|(severity, message)| {
                    rect()
                        .horizontal()
                        .cross_align(Alignment::Start)
                        .spacing(8.)
                        .child(
                            rect()
                                .width(TorinSize::px(8.))
                                .height(TorinSize::px(8.))
                                .corner_radius(99.)
                                .margin(Gaps::new(4., 0., 0., 0.))
                                .background(theme.diagnostic(severity)),
                        )
                        .child(
                            label()
                                .text(message)
                                .color(theme.text)
                                .font_size(TEXT_SIZE)
                                .max_width(TorinSize::px(text_w_cap)),
                        )
                        .into_element()
                }))
                .into_element()
        });

        // The completion popup — 300×≤224 with 30px rows (the committed design),
        // anchored at the **word start** so it never slides while the word's tail is
        // typed. Overlay layer + global position: escapes the editor pane and paints
        // above the results split; flip-up + horizontal clamp against the window.
        let completion_popup: Option<Element> = completion.read().open.as_ref().map(|open| {
            const POPUP_W: f32 = 480.0;
            const MAX_H: f32 = 224.0;
            const ROW_H: f32 = 30.0;
            const PAD: f32 = 4.0;

            let popup_h = (open.items.len() as f32 * ROW_H + PAD * 2.0).min(MAX_H);
            let gutter_offset = crate::editor_line::gutter_offset(font_size, gutter);
            let char_width = editor_data.metrics.char_width;
            let origin = (*editor_area.read())
                .map(|a| (a.min_x(), a.min_y()))
                .unwrap_or((0.0, 0.0));
            // The root rect pads 12px vertically; local line math is relative to the
            // padded inner area, so add it back for window space.
            let local_x = editor_data.scrolls.0 as f32
                + gutter_offset
                + open.anchor_col_chars as f32 * char_width;
            let anchor_top = origin.1
                + 12.0
                + open.anchor_line as f32 * line_height
                + editor_data.scrolls.1 as f32;
            let root = *Platform::get().root_size.peek();
            let (top, left) = flip_and_clamp(
                anchor_top,
                anchor_top + line_height,
                popup_h,
                POPUP_W,
                origin.0 + local_x,
                (root.width, root.height),
            );

            let selected = open.selected;
            let rows = open.items.iter().enumerate().map(|(i, item)| {
                let kind_color = theme.completion_kind(item.kind);
                let mut chip_tint = kind_color;
                chip_tint = chip_tint.with_a(36); // ~14% — the design's glyph-chip tint
                rect()
                    .horizontal()
                    // Flex content so the spacer below actually expands — without it
                    // the detail column collapses onto the label.
                    .content(Content::Flex)
                    .cross_align(Alignment::Center)
                    .width(TorinSize::fill())
                    .height(TorinSize::px(ROW_H))
                    .corner_radius(6.)
                    .padding(Gaps::new_symmetric(0., 6.))
                    .spacing(8.)
                    .maybe(i == selected, |el| {
                        el.background(theme.completion_selected_background)
                    })
                    .on_sized({
                        let mut row_areas = row_areas;
                        move |e: Event<SizedEventData>| {
                            if let Some(slot) = row_areas.write().get_mut(i) {
                                *slot = Some(e.area);
                            }
                        }
                    })
                    .on_pointer_enter({
                        let mut completion = completion;
                        move |_: Event<PointerEventData>| {
                            if let Some(o) = &mut completion.write().open {
                                o.selected = i;
                            }
                        }
                    })
                    .on_pointer_down({
                        let mut editor = editor.clone();
                        let mut completion = completion;
                        let mut row_areas = row_areas;
                        let on_completions = on_completions.clone();
                        move |_: Event<PointerEventData>| {
                            if let Some(o) = &mut completion.write().open {
                                o.selected = i;
                            }
                            accept_completion(
                                &mut editor,
                                &mut completion,
                                &mut row_areas,
                                on_completions.as_ref(),
                            );
                        }
                    })
                    .child(
                        rect()
                            .width(TorinSize::px(17.))
                            .height(TorinSize::px(17.))
                            .corner_radius(4.)
                            .main_align(Alignment::Center)
                            .cross_align(Alignment::Center)
                            .background(chip_tint)
                            .child(
                                label()
                                    .text(item.kind.glyph())
                                    .color(kind_color)
                                    .font_family(font_family.clone())
                                    .font_size(10.)
                                    .font_weight(600),
                            ),
                    )
                    .child(
                        label()
                            .text(item.label.clone())
                            .color(theme.text)
                            .font_family(font_family.clone())
                            .font_size(12.5)
                            .max_lines(1)
                            .max_width(TorinSize::px(200.)),
                    )
                    .child(rect().width(TorinSize::flex(1.)))
                    // Capped + single-line so a long signature can't collide with the
                    // name — a guaranteed gap between them.
                    .maybe_child(item.detail.clone().map(|detail| {
                        label()
                            .text(detail)
                            .color(theme.completion_detail)
                            .font_family(font_family.clone())
                            .font_size(10.)
                            .max_lines(1)
                            .max_width(TorinSize::px(POPUP_W - 224.))
                            .into_element()
                    }))
                    .into_element()
            });

            rect()
                .layer(Layer::Overlay)
                .position(Position::new_global().top(top).left(left))
                .width(TorinSize::px(POPUP_W))
                .height(TorinSize::px(popup_h))
                .background(theme.completion_background)
                .border(Border::new().width(1.).fill(theme.completion_border))
                .corner_radius(8.)
                .padding(Gaps::new_all(PAD))
                .on_sized({
                    let mut popup_area = popup_area;
                    move |e: Event<SizedEventData>| {
                        popup_area.set(Some(e.area));
                    }
                })
                .child(
                    ScrollView::new_controlled(popup_scroll)
                        .show_scrollbar(true)
                        .children(rows.collect::<Vec<Element>>()),
                )
                .into_element()
        });

        let on_key_up = {
            let mut editor = editor.clone();
            let font_family = font_family.clone();
            move |e: Event<KeyboardEventData>| {
                editor.write_if(|mut editor| {
                    editor.process(
                        font_size,
                        &font_family,
                        font_weight,
                        EditableEvent::KeyUp { key: &e.key },
                    )
                });
            }
        };

        let on_key_down = {
            let mut editor = editor.clone();
            let font_family = font_family.clone();
            let mut completion = completion;
            let mut row_areas = row_areas;
            let on_completions = on_completions.clone();
            move |e: Event<KeyboardEventData>| {
                const LINES_JUMP_ALT: usize = 5;
                const LINES_JUMP_CONTROL: usize = 3;

                let key = e.key.clone();
                let modifiers = e.modifiers;

                // ---- completion, part 1: claim keys while the popup is open + the
                // manual trigger. Runs before the app's pre-key gate and before any
                // editor processing; `prevent_default` also cancels the derived
                // global events (Esc must not cancel a running query, Enter must
                // not fire ⌘↵-adjacent bindings), and returning here means the
                // editor never sees the key (no newline on accept, no indent on
                // Tab, no caret move on ↑/↓).
                if let Some(provider) = &on_completions {
                    let plain = !modifiers
                        .intersects(Modifiers::META | Modifiers::CONTROL | Modifiers::ALT);
                    if completion.peek().open.is_some() {
                        match &key {
                            Key::Named(NamedKey::ArrowDown) if plain => {
                                e.prevent_default();
                                e.stop_propagation();
                                completion.write().step(1);
                                return;
                            }
                            Key::Named(NamedKey::ArrowUp) if plain => {
                                e.prevent_default();
                                e.stop_propagation();
                                completion.write().step(-1);
                                return;
                            }
                            // Accept only unmodified — a chorded Enter (⌘↵ Run)
                            // belongs to the app's keymap, popup or no popup.
                            Key::Named(NamedKey::Enter) | Key::Named(NamedKey::Tab) if plain => {
                                e.prevent_default();
                                e.stop_propagation();
                                accept_completion(
                                    &mut editor,
                                    &mut completion,
                                    &mut row_areas,
                                    Some(provider),
                                );
                                return;
                            }
                            Key::Named(NamedKey::Escape) => {
                                e.prevent_default();
                                e.stop_propagation();
                                completion.write().close();
                                return;
                            }
                            _ => {}
                        }
                    }
                    // ⌃Space / ⌘Space — by physical code, so keyboard layouts can't
                    // hide it. (⌘Space usually belongs to Spotlight; it works where
                    // the user has remapped that.)
                    if e.code == Code::Space
                        && modifiers.intersects(Modifiers::META | Modifiers::CONTROL)
                    {
                        e.prevent_default();
                        e.stop_propagation();
                        recompute_completion(
                            &editor,
                            &mut completion,
                            &mut row_areas,
                            provider,
                            true,
                            false,
                        );
                        return;
                    }
                }

                if !on_pre_key_down.call(e) {
                    return;
                }

                let (rev_before, cursor_before) = {
                    let d = editor.peek();
                    (d.revision(), d.cursor_pos())
                };

                editor.write_if(|mut editor| {
                    let lines_jump = (line_height * LINES_JUMP_ALT as f32).ceil() as i32;
                    let min_height = -(lines_len as f32 * line_height) as i32;
                    let max_height = 0; // TODO, this should be the height of the viewport
                    let current_scroll = editor.scrolls.1;

                    let events = match &key {
                        Key::Named(NamedKey::ArrowUp) if modifiers.contains(Modifiers::ALT) => {
                            let jump = (current_scroll + lines_jump).clamp(min_height, max_height);
                            editor.scrolls.1 = jump;
                            (0..LINES_JUMP_ALT)
                                .map(|_| EditableEvent::KeyDown {
                                    key: &key,
                                    modifiers,
                                })
                                .collect::<Vec<EditableEvent>>()
                        }
                        Key::Named(NamedKey::ArrowDown) if modifiers.contains(Modifiers::ALT) => {
                            let jump = (current_scroll - lines_jump).clamp(min_height, max_height);
                            editor.scrolls.1 = jump;
                            (0..LINES_JUMP_ALT)
                                .map(|_| EditableEvent::KeyDown {
                                    key: &key,
                                    modifiers,
                                })
                                .collect::<Vec<EditableEvent>>()
                        }
                        Key::Named(NamedKey::ArrowDown) | Key::Named(NamedKey::ArrowUp)
                            if modifiers.contains(Modifiers::CONTROL) =>
                        {
                            (0..LINES_JUMP_CONTROL)
                                .map(|_| EditableEvent::KeyDown {
                                    key: &key,
                                    modifiers,
                                })
                                .collect::<Vec<EditableEvent>>()
                        }
                        _ => vec![EditableEvent::KeyDown {
                            key: &key,
                            modifiers,
                        }],
                    };

                    let mut changed = false;

                    for event in events {
                        changed |= editor.process(font_size, &font_family, font_weight, event);
                    }

                    changed
                });

                // ---- completion, part 2: react to what the key did — in the same
                // frame, synchronously (the provider is a pure in-process function;
                // there is nothing to debounce and nothing that can arrive stale).
                if let Some(provider) = &on_completions {
                    let (rev_after, cursor_after) = {
                        let d = editor.peek();
                        (d.revision(), d.cursor_pos())
                    };
                    let was_open = completion.peek().open.is_some();
                    if rev_after != rev_before {
                        // What the key means for the popup is the trigger table's
                        // call; what the new position *offers* (including nothing —
                        // mid-literal `1.`, strings, comments) is entirely the
                        // provider's. The editor makes no grammar judgments.
                        match trigger_after_edit(&key, modifiers, was_open) {
                            TriggerDecision::Recompute => recompute_completion(
                                &editor,
                                &mut completion,
                                &mut row_areas,
                                provider,
                                false,
                                false,
                            ),
                            TriggerDecision::Close => {
                                completion.write().close();
                            }
                            TriggerDecision::None => {}
                        }
                    } else if was_open && cursor_after != cursor_before {
                        // Caret-only move (←/→, Home/End): refilter while it stays
                        // within the anchor word, close the moment it leaves.
                        if caret_within_anchor(&editor, &completion) {
                            recompute_completion(
                                &editor,
                                &mut completion,
                                &mut row_areas,
                                provider,
                                false,
                                false,
                            );
                        } else {
                            completion.write().close();
                        }
                    }
                }
            }
        };

        let on_global_pointer_press = {
            let mut editor = editor.clone();
            let font_family = font_family.clone();
            let mut completion = completion;
            move |e: Event<PointerEventData>| {
                // A press anywhere but the popup dismisses it. A press *on* a row
                // accepted already — non-global handlers run before globals.
                if completion.peek().open.is_some() {
                    let p = e.global_location();
                    let inside = popup_area
                        .peek()
                        .is_some_and(|a| a.contains((p.x as f32, p.y as f32).into()));
                    if !inside {
                        completion.write().close();
                    }
                }
                editor.write_if(|mut editor_editor| {
                    editor_editor.process(font_size, &font_family, font_weight, EditableEvent::Release)
                });
            }
        };

        // Leaving the editor entirely must drop the hover popup — the per-line move
        // handlers only fire while the pointer is still over some line.
        let on_pointer_leave = {
            let mut editor = editor.clone();
            move |_: Event<PointerEventData>| {
                editor.write_if(|mut editor_editor| editor_editor.update_hover(0, None));
            }
        };

        rect()
            .a11y_auto_focus(a11y_auto_focus)
            .a11y_focusable(true)
            .a11y_id(a11y_id)
            .a11y_role(AccessibilityRole::TextInput)
            .expanded()
            .background(theme.background)
            .padding(Gaps::new_symmetric(12., 0.))
            .maybe(!read_only, |el| {
                el.on_key_down(on_key_down).on_key_up(on_key_up)
            })
            .on_global_pointer_press(on_global_pointer_press)
            .on_pointer_leave(on_pointer_leave)
            .on_sized({
                let mut viewport = viewport;
                let mut editor_area = editor_area;
                move |e: Event<SizedEventData>| {
                    viewport.set((e.area.size.width, e.area.size.height));
                    // Window-space origin for the completion popup's global position.
                    editor_area.set(Some(e.area));
                }
            })
            .child(
                VirtualScrollView::new(move |line_index, _| {
                    EditorLineUI {
                        editor: editor.clone(),
                        font_size,
                        line_height,
                        line_index,
                        read_only,
                        gutter,
                        show_whitespace,
                        highlight_current_line,
                        font_family: font_family.clone(),
                        font_weight,
                        theme: theme.clone(),
                        syntax_theme: syntax_theme.clone(),
                        a11y_id,
                    }
                    .into()
                })
                .scroll_controller(scroll_controller)
                .length(lines_len)
                .item_size(line_height),
            )
            .maybe_child(diagnostics_panel)
            .maybe_child(completion_popup)
    }
}

/// Snapshot the buffer + caret, run the provider, and open/refresh the popup from
/// the result — synchronously, in the caller's frame. An empty result closes it.
/// Selection resets to the top on every refilter (predictable; the VS Code policy).
/// `fresh_only` opens only when the provider reports an empty replace span
/// (`caret..caret` — a fresh position with nothing typed): the accept-chain's
/// gate, derived from the provider's own answer, no grammar sniffing here.
fn recompute_completion(
    editor: &Writable<CodeEditorData>,
    completion: &mut State<CompletionState>,
    row_areas: &mut State<Vec<Option<Area>>>,
    provider: &Callback<CompletionRequest, Vec<CompletionItem>>,
    manual: bool,
    fresh_only: bool,
) {
    let (text, caret_byte) = {
        let d = editor.peek();
        let cursor_char = d.rope.utf16_cu_to_char(d.cursor_pos());
        (d.rope.to_string(), d.rope.char_to_byte(cursor_char))
    };
    let items = provider.call(CompletionRequest {
        text,
        caret_byte,
        manual,
    });
    // Provider invariant: every item in one response shares the same replace span
    // (the partial word under the caret).
    debug_assert!(
        items.windows(2).all(|w| w[0].replace == w[1].replace),
        "completion items must share one replace span"
    );
    let mut open = (!items.is_empty()).then(|| {
        let d = editor.peek();
        let replace = items[0].replace.clone();
        let start_char = d.rope.byte_to_char(replace.start.min(d.rope.len_bytes()));
        let anchor_line = d.rope.char_to_line(start_char);
        let anchor_col_chars = start_char - d.rope.line_to_char(anchor_line);
        OpenCompletion {
            items,
            selected: 0,
            anchor_line,
            anchor_col_chars,
            replace,
        }
    });
    if fresh_only && open.as_ref().is_some_and(|o| o.replace.start != o.replace.end) {
        open = None;
    }
    {
        let mut areas = row_areas.write();
        areas.clear();
        areas.resize(open.as_ref().map(|o| o.items.len()).unwrap_or(0), None);
    }
    completion.write().open = open;
}

/// Apply the selected candidate: replace its byte span (converted to the editor's
/// UTF-16 space) with the insert text — one undo step, caret at the insert's end —
/// then close and **re-ask the provider**. If the caret landed at a fresh position
/// (empty replace span — after `FROM `, inside `sum(`), the popup chains straight
/// into the next offer (the DataGrip flow); if it landed at a word end (a plain
/// identifier accept) or somewhere the provider offers nothing (`LIMIT `, `AS `),
/// it stays closed. The gate is the provider's own answer — the editor never
/// inspects the inserted text.
fn accept_completion(
    editor: &mut Writable<CodeEditorData>,
    completion: &mut State<CompletionState>,
    row_areas: &mut State<Vec<Option<Area>>>,
    provider: Option<&Callback<CompletionRequest, Vec<CompletionItem>>>,
) {
    let item = {
        let st = completion.peek();
        st.open
            .as_ref()
            .and_then(|o| o.items.get(o.selected).cloned())
    };
    let Some(item) = item else {
        completion.write().close();
        return;
    };
    editor.write_if(|mut d| {
        let len_bytes = d.rope.len_bytes();
        let start_char = d.rope.byte_to_char(item.replace.start.min(len_bytes));
        let end_char = d.rope.byte_to_char(item.replace.end.min(len_bytes));
        let range = d.rope.char_to_utf16_cu(start_char)..d.rope.char_to_utf16_cu(end_char);
        d.replace_range(range, &item.insert);
        true
    });
    completion.write().close();
    if let Some(provider) = provider {
        recompute_completion(editor, completion, row_areas, provider, false, true);
    }
}

/// Whether the caret still sits inside the popup's anchor word — from the anchored
/// start byte through the end of the identifier run beginning there.
fn caret_within_anchor(
    editor: &Writable<CodeEditorData>,
    completion: &State<CompletionState>,
) -> bool {
    let Some(start_byte) = completion.peek().open.as_ref().map(|o| o.replace.start) else {
        return false;
    };
    let d = editor.peek();
    let caret_byte = d
        .rope
        .char_to_byte(d.rope.utf16_cu_to_char(d.cursor_pos()));
    if caret_byte < start_byte {
        return false;
    }
    let start_char = d.rope.byte_to_char(start_byte.min(d.rope.len_bytes()));
    let mut end_char = start_char;
    for ch in d.rope.chars_at(start_char) {
        if is_ident_char(ch) {
            end_char += 1;
        } else {
            break;
        }
    }
    caret_byte <= d.rope.char_to_byte(end_char)
}
