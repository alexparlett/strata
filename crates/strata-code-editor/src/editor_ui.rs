use std::borrow::Cow;

use freya_components::{get_theme, get_theme_or_default, scrollviews::{
    ScrollController,
    ScrollEvent,
    VirtualScrollView,
}};
use freya_core::prelude::*;
use freya_edit::EditableEvent;
use torin::gaps::Gaps;
use crate::{
    editor_data::CodeEditorData,
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

        let editor_data = editor.read();

        let scroll_controller = use_hook(|| {
            let notifier = State::create(());
            let requests = State::create(vec![]);
            ScrollController::managed(
                notifier,
                requests,
                State::create(Callback::new({
                    let mut editor = editor.clone();
                    move |ev| {
                        editor.write_if(|mut editor| {
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
                        })
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
            move |e: Event<KeyboardEventData>| {
                const LINES_JUMP_ALT: usize = 5;
                const LINES_JUMP_CONTROL: usize = 3;

                let key = e.key.clone();
                let modifiers = e.modifiers;

                if !on_pre_key_down.call(e) {
                    return;
                }

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
            }
        };

        let on_global_pointer_press = {
            let mut editor = editor.clone();
            let font_family = font_family.clone();
            move |_: Event<PointerEventData>| {
                editor.write_if(|mut editor_editor| {
                    editor_editor.process(font_size, &font_family, font_weight, EditableEvent::Release)
                });
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
    }
}
