use crate::apps::project::query::RunId;
use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::apps::project::views::workbench::editor::toolbar::EditorToolbar;
use crate::components::divider::Divider;
use freya::components::use_theme;
use freya::prelude::{
    rect, use_a11y, use_consume, use_side_effect, ChildrenExt, Component, ContainerSizeExt,
    ContainerWithContentExt, Content, ComponentKey, DiffKey, Event, IntoElement, IntoWritable, Key,
    KeyExt, KeyboardEventData, Modifiers, NamedKey, Size, State,
};
use freya::radio::use_radio;
use strata_code_editor::prelude::{CodeEditor, CodeEditorData, EditorLanguage, Rope};
use strata_core::config::{Command, Settings};

/// One tab's editor pane: the toolbar above the `CodeEditor`, then a bottom divider. Slices a
/// `Writable<CodeEditorData>` straight into the store on `Chan::Tab(id)`. Carries the
/// `running` mirror down to the toolbar for its Run→Cancel flip (the Run trigger itself is
/// the tab's own — `QueryTab::request`). The editor's pre-key gate keeps primary-held app
/// chords (⌘T / ⌘↵ / …) out of the buffer while letting them reach the keymap's global
/// listeners, and keeps the buffer's rebindable undo/redo chords (`EditBindings`) synced
/// from the settings so the text layer matches whatever the user bound.
#[derive(PartialEq)]
pub struct EditorTab {
    pub id: TabId,
    pub running: State<Option<RunId>>,
    pub key: DiffKey,
}

impl EditorTab {
    pub fn new(id: TabId, running: State<Option<RunId>>) -> Self {
        // Keyed by the tab: the pane renders in one fixed slot, and without a key a tab
        // switch would reuse the scope — the mounted `CodeEditor`'s props all compare equal
        // (`Writable` is always-equal), so it would keep the *previous* tab's buffer binding.
        Self { id, running, key: DiffKey::None }.key(id)
    }
}

impl KeyExt for EditorTab {
    fn write_key(&mut self) -> &mut DiffKey {
        &mut self.key
    }
}

impl Component for EditorTab {
    fn render(&self) -> impl IntoElement {
        let id = self.id;
        let a11y_id = use_a11y();
        let radio = use_radio::<SessionState, Chan>(Chan::Tab(id));
        // The slice must yield `&mut CodeEditorData` for *any* read/write the mounted `CodeEditor`
        // makes — including a commit that fires one event *after* the tab was closed (closing the
        // active tab via the nav-dropdown × runs `close_one` on the same click, before the editor's
        // commit-on-click-outside global handler). So the lens is total: a live tab yields its own
        // editor; a just-closed tab falls back to a throwaway scratch buffer (that write is moot).
        let editor = radio.slice_mut(Chan::Tab(id), move |s: &mut SessionState| {
            if s.tabs.contains_key(&id) {
                &mut s.tabs.get_mut(&id).unwrap().editor
            } else {
                s.scratch
                 .get_or_insert_with(|| CodeEditorData::new(Rope::from_str(""), None::<EditorLanguage>))
            }
        });
        let editor = editor.into_writable();
        let settings = use_consume::<State<Settings>>();
        // Keep the buffer's history chords in lockstep with the settings: freya-edit
        // matches `EditBindings` in `process_key` (no hardcoded ⌘Z/⌘Y left), so a
        // rebind in Settings retargets undo/redo live, without remounting the editor.
        {
            let mut editor = editor.clone();
            use_side_effect(move || {
                let bindings = crate::keymap::edit_bindings(&settings.read());
                editor.write_if(|mut data| data.set_edit_bindings(bindings));
            });
        }
        let border = use_theme().read().colors.border;

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .child(EditorToolbar { id, running: self.running })
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .child(
                        // Type (family · size · weight · line height) comes from the
                        // `code_editor` theme — the editor dresses and measures itself.
                        CodeEditor::new(editor, a11y_id)
                            .a11y_auto_focus(true)
                            .gutter(true)
                            .show_whitespace(false)
                            .highlight_current_line(false)
                            // Primary-held chords belong to the app keymap unless the
                            // editor owns them: skip the editor's processing —
                            // otherwise ⌘T types a "t" and ⌘↵ inserts a newline — while
                            // the global listeners still fire (only `prevent_default`
                            // would cancel those, and this calls only
                            // `stop_propagation`, like the default pre-handler). The
                            // editor owns exactly the chords that currently resolve to
                            // an editing command (`Command::is_edit` — select all /
                            // copy / cut / paste / undo / redo, all rebindable): those
                            // flow through to `process_key`, where the buffer's own
                            // `EditBindings` (synced from these same settings above)
                            // match them. Named keys keep flowing: Ctrl/Alt+arrows are
                            // editor navigation.
                            .on_pre_key_down(move |e: Event<KeyboardEventData>| {
                                e.stop_propagation();
                                if let Key::Named(NamedKey::Tab) = &e.key {
                                    e.prevent_default();
                                }
                                let primary = e
                                    .modifiers
                                    .intersects(Modifiers::META | Modifiers::CONTROL);
                                let editor_owned = crate::keymap::chord_from_event(&e)
                                    .and_then(|chord| {
                                        strata_core::keymap::resolve(&settings.peek(), &chord)
                                    })
                                    .is_some_and(Command::is_edit)
                                    || match &e.key {
                                        Key::Character(_) => false,
                                        Key::Named(NamedKey::Enter) => false,
                                        _ => true,
                                    };
                                !(primary && !editor_owned)
                            }),
                    )
            )
            .child(Divider::horizontal().color(border))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}
