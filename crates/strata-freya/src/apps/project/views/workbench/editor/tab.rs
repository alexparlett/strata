use crate::apps::project::contexts::EngineCtx;
use crate::apps::project::query::RunId;
use crate::apps::project::state::{use_catalog, Chan, ProjChan, ProjectState, SessionState};
use crate::apps::project::views::workbench::editor::toolbar::EditorToolbar;
use crate::components::divider::Divider;
use crate::keymap::{chord_from_event, edit_bindings};
use crate::state::{use_config, use_config_station, ConfigChan};
use crate::theme::{use_roles, Role};
use freya::prelude::{
    rect, use_a11y, use_consume, use_side_effect, use_state, ChildrenExt, Component, ComponentKey,
    ContainerSizeExt, ContainerWithContentExt, Content, DiffKey, Event, IntoElement, IntoWritable,
    Key, KeyExt, KeyboardEventData, Modifiers, NamedKey, Size, State,
};
use freya::radio::{use_radio, use_radio_station};
use strata_arrow::config::{effective, DIALECT_KEY};
use strata_code_editor::prelude::{
    CodeEditor, CodeEditorData, CompletionItem, CompletionItemKind, CompletionRequest,
    EditorLanguage, Rope,
};
use strata_core::config::Command;
use strata_core::keymap::resolve;
use strata_engine::sql;
use strata_model::TabId;

/// One tab's editor pane: the toolbar above the `CodeEditor`, then a bottom divider. Slices a
/// `Writable<CodeEditorData>` straight into the store on `Chan::Tab(id)`. Carries the
/// `running` mirror down to the toolbar for its Run→Cancel flip (the Run trigger itself is
/// the tab's own — `QueryTab::request`). The editor's pre-key gate keeps primary-held app
/// chords (⌘T / ⌘↵ / …) out of the buffer while letting them reach the keymap's global
/// listeners, and keeps the buffer's rebindable undo/redo chords (`EditBindings`) synced
/// from the settings so the text layer matches whatever the user bound.
///
/// It also **holds the completion snapshot**: one `sql::Symbols::build` off the store's rows and
/// the engine's `lang().bundle()`, re-assembled by a side effect keyed on the catalog generation
/// and nothing else. That snapshot is what makes `sql::complete` engine-free on the keystroke
/// path — see `docs/COMPLETION_SPEC.md` §8.
#[derive(PartialEq)]
pub struct EditorTab {
    pub id: TabId,
    pub running: State<Option<RunId>>,
    pub key: DiffKey,
}

impl EditorTab {
    pub fn new(id: TabId, running: State<Option<RunId>>) -> Self {
        Self {
            id,
            running,
            key: DiffKey::None,
        }
        .key(id)
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
        let editor = radio.slice_mut(Chan::Tab(id), move |s: &mut SessionState| {
            if s.tabs.contains_key(&id) {
                &mut s.tabs.get_mut(&id).unwrap().editor
            } else {
                s.scratch.get_or_insert_with(|| {
                    CodeEditorData::new(Rope::from_str(""), None::<EditorLanguage>)
                })
            }
        });
        let editor = editor.into_writable();
        let config = use_config_station();
        let settings = use_config(ConfigChan::Settings);
        {
            let mut editor = editor.clone();
            use_side_effect(move || {
                let bindings = edit_bindings(&settings.read().settings);
                editor.write_if(|mut data| data.set_edit_bindings(bindings));
            });
        }
        let engine = use_consume::<EngineCtx>();
        let project = use_radio_station::<ProjectState, ProjChan>();
        let generation = use_catalog();
        let mut catalog = use_state(sql::Symbols::default);
        {
            use_side_effect(move || {
                let p = project.read();
                let _ = generation.read();
                let dialect =
                    effective(&settings.read().settings.engine, DIALECT_KEY).unwrap_or_default();
                *catalog.write() = sql::Symbols::build(
                    p.tables.iter().map(|t| {
                        (
                            t.def.name.as_str(),
                            t.meta.as_ref().map(|m| m.columns.as_slice()).unwrap_or(&[]),
                            t.def.origin.is_internal(),
                        )
                    }),
                    p.views.iter().map(|v| {
                        (
                            v.def.name.as_str(),
                            v.info.as_ref().map(|i| i.columns.as_slice()).unwrap_or(&[]),
                        )
                    }),
                    engine.lang().bundle(),
                    dialect,
                );
            });
        }
        let on_completions = move |req: CompletionRequest| {
            sql::complete(&catalog.peek(), &req.text, req.caret_byte, req.manual)
                .into_iter()
                .map(to_completion_item)
                .collect::<Vec<_>>()
        };
        let border = use_roles().get(Role::Border);

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .child(EditorToolbar {
                id,
                running: self.running,
            })
            .child(
                rect().width(Size::fill()).height(Size::flex(1.)).child(
                    CodeEditor::new(editor, a11y_id)
                        .a11y_auto_focus(true)
                        .gutter(true)
                        .show_whitespace(false)
                        .highlight_current_line(false)
                        .on_completions(on_completions)
                        .on_pre_key_down(move |e: Event<KeyboardEventData>| {
                            e.stop_propagation();
                            if let Key::Named(NamedKey::Tab) = &e.key {
                                e.prevent_default();
                            }
                            let primary =
                                e.modifiers.intersects(Modifiers::META | Modifiers::CONTROL);
                            let editor_owned = chord_from_event(&e)
                                .and_then(|chord| resolve(&config.peek().settings, &chord))
                                .is_some_and(Command::is_edit)
                                || !matches!(
                                    &e.key,
                                    Key::Character(_) | Key::Named(NamedKey::Enter)
                                );
                            !(primary && !editor_owned)
                        }),
                ),
            )
            .child(Divider::horizontal().color(border))
    }

    fn render_key(&self) -> DiffKey {
        self.key.clone().or(self.default_key())
    }
}

/// The 1:1 map from the language service's candidate to the editor's row model.
fn to_completion_item(c: sql::Completion) -> CompletionItem {
    CompletionItem {
        label: c.label,
        insert: c.insert,
        kind: match c.kind {
            sql::CompletionKind::Table => CompletionItemKind::Table,
            sql::CompletionKind::View => CompletionItemKind::View,
            sql::CompletionKind::Column => CompletionItemKind::Column,
            sql::CompletionKind::Function => CompletionItemKind::Function,
            sql::CompletionKind::Keyword => CompletionItemKind::Keyword,
        },
        detail: c.detail,
        replace: c.replace,
    }
}
