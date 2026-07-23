use crate::apps::project::query::{QuerySpec, RunId};
use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::apps::project::views::workbench::editor::toolbar::EditorToolbar;
use crate::components::divider::Divider;
use freya::components::use_theme;
use freya::prelude::{rect, use_a11y, ChildrenExt, Component, ContainerSizeExt, ContainerWithContentExt, Content, IntoElement, IntoWritable, Size, State};
use freya::radio::use_radio;
use strata_code_editor::prelude::{CodeEditor, CodeEditorData, EditorLanguage, Rope};

/// One tab's editor pane: the toolbar above the `CodeEditor`, then a bottom divider. Slices a
/// `Writable<CodeEditorData>` straight into the store on `Chan::Tab(id)`. Carries the
/// workbench's Run trigger down to the toolbar (which writes a press into it), plus the
/// `running` mirror the toolbar reads for its Run→Cancel flip.
#[derive(PartialEq)]
pub struct EditorTab {
    pub id: TabId,
    pub request: State<Option<QuerySpec>>,
    pub running: State<Option<RunId>>,
}

impl EditorTab {
    pub fn new(id: TabId, request: State<Option<QuerySpec>>, running: State<Option<RunId>>) -> Self {
        Self { id, request, running }
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
        let border = use_theme().read().colors.border;

        rect()
            .expanded()
            .vertical()
            .content(Content::Flex)
            .child(EditorToolbar { id, request: self.request, running: self.running })
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .child(
                        // Type (family · size · weight · line height) comes from the
                        // `code_editor` theme — the editor dresses and measures itself.
                        CodeEditor::new(editor.into_writable(), a11y_id)
                            .a11y_auto_focus(true)
                            .gutter(true)
                            .show_whitespace(false)
                            .highlight_current_line(false),
                    )
            )
            .child(Divider::horizontal().color(border))
    }
}
