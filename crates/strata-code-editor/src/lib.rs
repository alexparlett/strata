//! Strata's code-editor surface — **vendored** from `freya-code-editor` (marc2332/freya, MIT).
//!
//! We own this layer because every knob we need is hardcoded upstream and the features we want
//! (diagnostic squiggles, an autocomplete popup) have no render surface there. The editing *engine*
//! (`freya-edit`) and the grammar (`tree-sitter`) remain upstream dependencies — we own only the
//! render + syntax glue, kept close to upstream so diffs stay legible.

pub mod completion;
pub mod constants;
pub mod editor_data;
pub mod editor_line;
pub mod editor_theme;
pub mod editor_ui;
pub mod languages;
pub mod metrics;
pub mod syntax;

pub use tree_sitter;

pub mod prelude {
    pub use ropey::Rope;

    pub use crate::{
        completion::{CompletionItem, CompletionItemKind, CompletionRequest},
        constants::{BASE_FONT_SIZE, MAX_FONT_SIZE},
        editor_data::{CodeEditorData, Decoration, DecorationSeverity},
        editor_line::EditorLineUI,
        editor_theme::{
            CodeEditorThemeExt, EditorSyntaxTheme, EditorSyntaxThemePartial,
            EditorSyntaxThemePartialExt, EditorSyntaxThemePreference, EditorTheme,
            EditorThemePartial, EditorThemePartialExt, EditorThemePreference, SYNTAX_SCOPES,
        },
        editor_ui::CodeEditor,
        languages::EditorLanguage,
        metrics::EditorMetrics,
        syntax::{
            InputEditExt, RopeChunkIter, RopeTextProvider, SyntaxBlocks, SyntaxHighlighter,
            SyntaxLine, TextNode,
        },
    };
}
