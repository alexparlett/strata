use crate::apps::project::state::{Chan, SessionState, TabId};
use crate::apps::project::views::workbench::editor::toolbar::EditorToolbar;
use crate::components::divider::Divider;
use freya::components::use_theme;
use freya::prelude::{rect, use_a11y, ChildrenExt, Component, ContainerSizeExt, ContainerWithContentExt, Content, IntoElement, IntoWritable, Size};
use freya::radio::use_radio;
use strata_code_editor::prelude::{CodeEditor, CodeEditorData, EditorLanguage, Rope};

/// One tab's editor pane: the toolbar above the `CodeEditor`, then a bottom divider. Slices a
/// `Writable<CodeEditorData>` straight into the store on `Chan::Tab(id)`.
#[derive(PartialEq)]
pub struct EditorTab {
    pub id: TabId,
}

impl EditorTab {
    pub fn new(id: TabId) -> Self {
        Self { id }
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
            .child(EditorToolbar)
            .child(
                rect()
                    .width(Size::fill())
                    .height(Size::flex(1.))
                    .child(
                        CodeEditor::new(editor.into_writable(), a11y_id)
                            .a11y_auto_focus(true)
                            .font_size(12.)
                            .font_family("Jetbrains Mono")
                            .gutter(true)
                            .line_height(1.6)
                            .show_whitespace(false)
                            .highlight_current_line(false),
                    )
            )
            .child(Divider::horizontal().color(border))
    }
}
