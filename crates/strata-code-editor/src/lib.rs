//! Strata's code-editor surface — **vendored** from `freya-code-editor` (marc2332/freya, MIT).
//!
//! We own this layer because every knob we need is hardcoded upstream (block-only cursor, the
//! current-line highlight rule, gutter styling) and the features we want (diagnostic squiggles,
//! an autocomplete popup) have no render surface there at all. The editing *engine* (`freya-edit`:
//! `TextEditor`, history, selection, key processing) and the grammar (`tree-sitter`) remain
//! upstream dependencies — we only own the render + syntax-glue.
//!
//! Kept close to upstream on purpose so diffs stay legible; Strata-specific changes are called out
//! where they land (`editor_line` = cursor/gutter/highlight/squiggles, `editor_ui` = autocomplete).

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
        completion::{
            CompletionItem,
            CompletionItemKind,
            CompletionRequest,
        },
        constants::{
            BASE_FONT_SIZE,
            MAX_FONT_SIZE,
        },
        editor_data::{
            CodeEditorData,
            Decoration,
            DecorationSeverity,
        },
        editor_line::EditorLineUI,
        editor_theme::{
            CodeEditorThemeExt,
            EditorSyntaxTheme,
            EditorSyntaxThemePartial,
            EditorSyntaxThemePartialExt,
            EditorSyntaxThemePreference,
            EditorTheme,
            EditorThemePartial,
            EditorThemePartialExt,
            EditorThemePreference,
        },
        editor_ui::CodeEditor,
        languages::EditorLanguage,
        metrics::EditorMetrics,
        syntax::{
            InputEditExt,
            RopeChunkIter,
            RopeTextProvider,
            SyntaxBlocks,
            SyntaxHighlighter,
            SyntaxLine,
            TextNode,
        },
    };
}
