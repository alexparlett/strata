use std::borrow::Cow;

use tree_sitter::Language;

/// A language definition used for syntax highlighting.
///
/// Bring your own tree-sitter grammar and its highlights query, so the editor
/// can highlight any language without the crate depending on specific grammars.
///
/// The example is `ignore`d: grammar crates are the caller's dependency, not this
/// crate's.
///
/// ```ignore
/// # use strata_code_editor::prelude::EditorLanguage;
/// let language = EditorLanguage::new(
///     tree_sitter_rust::LANGUAGE,
///     tree_sitter_rust::HIGHLIGHTS_QUERY,
/// );
/// ```
#[derive(Clone)]
pub struct EditorLanguage {
    pub language: Language,
    pub highlights_query: Cow<'static, str>,
}

impl EditorLanguage {
    pub fn new(
        language: impl Into<Language>,
        highlights_query: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            language: language.into(),
            highlights_query: highlights_query.into(),
        }
    }
}
