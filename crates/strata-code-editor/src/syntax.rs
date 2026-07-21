use std::ops::Range;

use ropey::Rope;
use rustc_hash::FxHashMap;
use smallvec::SmallVec;
use tree_sitter::{
    InputEdit,
    Language,
    Parser,
    Point,
    Query,
    QueryCursor,
    StreamingIterator,
    Tree,
};

use crate::languages::EditorLanguage;

/// A highlight class — the **theme-independent** result of parsing. One per capture, mapped to a
/// colour by the editor at render time (via `EditorSyntaxTheme`), so the buffer never holds a theme.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SyntaxKind {
    #[default]
    Text,
    Whitespace,
    Attribute,
    Boolean,
    Comment,
    Constant,
    Constructor,
    Escape,
    Function,
    FunctionMacro,
    FunctionMethod,
    Keyword,
    Label,
    Module,
    Number,
    Operator,
    Property,
    Punctuation,
    PunctuationBracket,
    PunctuationDelimiter,
    PunctuationSpecial,
    String,
    StringEscape,
    StringSpecial,
    Tag,
    TextLiteral,
    TextReference,
    TextTitle,
    TextUri,
    TextEmphasis,
    Type,
    Variable,
    VariableBuiltin,
    VariableParameter,
}

/// Maps a tree-sitter capture name to its highlight class (no theme involved).
fn capture_kind(name: &str) -> SyntaxKind {
    match name {
        "attribute" => SyntaxKind::Attribute,
        "boolean" => SyntaxKind::Boolean,
        "comment" | "comment.documentation" => SyntaxKind::Comment,
        "constant" | "constant.builtin" => SyntaxKind::Constant,
        "constructor" => SyntaxKind::Constructor,
        "escape" => SyntaxKind::Escape,
        "function" | "function.builtin" => SyntaxKind::Function,
        "function.macro" => SyntaxKind::FunctionMacro,
        "function.method" => SyntaxKind::FunctionMethod,
        "keyword" => SyntaxKind::Keyword,
        "label" => SyntaxKind::Label,
        "module" => SyntaxKind::Module,
        "number" => SyntaxKind::Number,
        "operator" => SyntaxKind::Operator,
        "property" => SyntaxKind::Property,
        "punctuation" => SyntaxKind::Punctuation,
        "punctuation.bracket" => SyntaxKind::PunctuationBracket,
        "punctuation.delimiter" => SyntaxKind::PunctuationDelimiter,
        "punctuation.special" => SyntaxKind::PunctuationSpecial,
        "string" => SyntaxKind::String,
        "string.escape" => SyntaxKind::StringEscape,
        "string.special" | "string.special.key" | "string.special.symbol" => {
            SyntaxKind::StringSpecial
        }
        "tag" => SyntaxKind::Tag,
        "text.literal" => SyntaxKind::TextLiteral,
        "text.reference" => SyntaxKind::TextReference,
        "text.title" => SyntaxKind::TextTitle,
        "text.uri" => SyntaxKind::TextUri,
        "text.emphasis" | "text.strong" => SyntaxKind::TextEmphasis,
        "type" | "type.builtin" => SyntaxKind::Type,
        "variable" => SyntaxKind::Variable,
        "variable.builtin" => SyntaxKind::VariableBuiltin,
        "variable.parameter" => SyntaxKind::VariableParameter,
        _ => SyntaxKind::Text,
    }
}

/// Tries exact match, then strips trailing dot-segments for hierarchical fallback.
fn resolve_capture_kind(name: &str) -> SyntaxKind {
    let kind = capture_kind(name);
    if kind != SyntaxKind::Text {
        return kind;
    }
    let mut candidate = name;
    while let Some(pos) = candidate.rfind('.') {
        candidate = &candidate[..pos];
        let k = capture_kind(candidate);
        if k != SyntaxKind::Text {
            return k;
        }
    }
    SyntaxKind::Text
}

pub enum TextNode {
    Range(Range<usize>),
    LineOfChars { len: usize, char: char },
}

pub type SyntaxLine = SmallVec<[(SyntaxKind, TextNode); 4]>;

#[derive(Default)]
pub struct SyntaxBlocks {
    blocks: FxHashMap<usize, SyntaxLine>,
}

impl SyntaxBlocks {
    pub fn push_line(&mut self, line: SyntaxLine) {
        self.blocks.insert(self.len(), line);
    }

    pub fn get_line(&self, line: usize) -> &[(SyntaxKind, TextNode)] {
        self.blocks.get(&line).unwrap()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn clear(&mut self) {
        self.blocks.clear();
    }
}

struct LangConfig {
    language: Language,
    query: Query,
    capture_kinds: Vec<SyntaxKind>,
}

pub struct SyntaxHighlighter {
    parser: Parser,
    tree: Option<Tree>,
    config: Option<LangConfig>,
    cursor: QueryCursor,
}

impl Default for SyntaxHighlighter {
    fn default() -> Self {
        Self::new()
    }
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
            tree: None,
            config: None,
            cursor: QueryCursor::new(),
        }
    }

    /// Configures the language used for highlighting, or disables it with `None`. Theme-independent.
    pub fn set_language(&mut self, language: Option<&EditorLanguage>) {
        self.tree = None;
        self.config = language.and_then(|language| language.lang_config());
        if let Some(cfg) = &self.config {
            let _ = self.parser.set_language(&cfg.language);
        }
    }

    /// Discard the cached parse tree, forcing a full re-parse next time.
    pub fn invalidate_tree(&mut self) {
        self.tree = None;
    }

    /// Incrementally re-parse the rope and rebuild syntax blocks.
    pub fn parse(&mut self, rope: &Rope, syntax_blocks: &mut SyntaxBlocks, edit: Option<InputEdit>) {
        syntax_blocks.clear();

        if let Some(input_edit) = edit
            && let Some(tree) = &mut self.tree
        {
            tree.edit(&input_edit);
        }

        let new_tree = {
            let len = rope.len_bytes();
            self.parser.parse_with_options(
                &mut |byte_offset: usize, _position: Point| {
                    if byte_offset >= len {
                        return &[] as &[u8];
                    }
                    let (chunk, chunk_start, _, _) = rope.chunk_at_byte(byte_offset);
                    &chunk.as_bytes()[byte_offset - chunk_start..]
                },
                self.tree.as_ref(),
                None,
            )
        };

        if let Some(new_tree) = new_tree {
            if let Some(cfg) = &self.config {
                build_syntax_blocks(&new_tree, cfg, &mut self.cursor, rope, syntax_blocks);
            } else {
                build_plain_blocks(rope, syntax_blocks);
            }
            self.tree = Some(new_tree);
        } else {
            build_plain_blocks(rope, syntax_blocks);
        }
    }
}

pub trait InputEditExt {
    fn new_edit(
        start_byte: usize,
        old_end_byte: usize,
        new_end_byte: usize,
        start_position: (usize, usize),
        old_end_position: (usize, usize),
        new_end_position: (usize, usize),
    ) -> InputEdit;
}

impl InputEditExt for InputEdit {
    fn new_edit(
        start_byte: usize,
        old_end_byte: usize,
        new_end_byte: usize,
        start_position: (usize, usize),
        old_end_position: (usize, usize),
        new_end_position: (usize, usize),
    ) -> InputEdit {
        InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position: Point::new(start_position.0, start_position.1),
            old_end_position: Point::new(old_end_position.0, old_end_position.1),
            new_end_position: Point::new(new_end_position.0, new_end_position.1),
        }
    }
}

struct Span {
    start_byte: usize,
    end_byte: usize,
    kind: SyntaxKind,
}

fn build_syntax_blocks(
    tree: &Tree,
    cfg: &LangConfig,
    cursor: &mut QueryCursor,
    rope: &Rope,
    syntax_blocks: &mut SyntaxBlocks,
) {
    let root = tree.root_node();
    cursor.set_byte_range(0..usize::MAX);

    let mut spans: Vec<Span> = Vec::new();
    let mut captures = cursor.captures(&cfg.query, root, RopeTextProvider { rope });

    while let Some((match_result, capture_idx)) = {
        captures.advance();
        captures.get()
    } {
        let capture = &match_result.captures[*capture_idx];
        let node = capture.node;
        let kind = cfg.capture_kinds[capture.index as usize];
        spans.push(Span {
            start_byte: node.start_byte(),
            end_byte: node.end_byte(),
            kind,
        });
    }

    spans.sort_by_key(|s| s.start_byte);
    build_lines_from_spans(rope, &spans, syntax_blocks);
}

fn build_lines_from_spans(rope: &Rope, spans: &[Span], syntax_blocks: &mut SyntaxBlocks) {
    let total_lines = rope.len_lines();
    let mut span_idx = 0;

    for line_idx in 0..total_lines {
        let line_start_byte = rope.line_to_byte(line_idx);
        let line_slice = rope.line(line_idx);
        let line_byte_len = line_slice.len_bytes();
        let line_end_byte = line_start_byte + line_byte_len;

        let content_end_byte = {
            let chars = line_slice.len_chars();
            let mut end = line_end_byte;
            if chars > 0 && line_slice.char(chars - 1) == '\n' {
                end -= 1;
                if chars > 1 && line_slice.char(chars - 2) == '\r' {
                    end -= 1;
                }
            }
            end
        };

        while span_idx < spans.len() && spans[span_idx].end_byte <= line_start_byte {
            span_idx += 1;
        }

        let content_bytes = content_end_byte - line_start_byte;
        if content_bytes == 0 {
            syntax_blocks.push_line(SmallVec::new());
            continue;
        }

        let mut byte_kinds: SmallVec<[SyntaxKind; 256]> =
            smallvec::smallvec![SyntaxKind::Text; content_bytes];

        let mut si = span_idx;
        while si < spans.len() && spans[si].start_byte < content_end_byte {
            let span = &spans[si];
            si += 1;
            if span.end_byte <= line_start_byte {
                continue;
            }
            let s = span.start_byte.max(line_start_byte) - line_start_byte;
            let e = span.end_byte.min(content_end_byte) - line_start_byte;
            if s < e {
                for k in &mut byte_kinds[s..e] {
                    *k = span.kind;
                }
            }
        }

        let mut line_spans: SyntaxLine = SyntaxLine::new();
        let mut beginning_of_line = true;
        let mut run_start: usize = 0;

        while run_start < content_bytes {
            let run_kind = byte_kinds[run_start];
            let mut run_end = run_start + 1;
            while run_end < content_bytes && byte_kinds[run_end] == run_kind {
                run_end += 1;
            }

            let abs_start_byte = line_start_byte + run_start;
            let abs_end_byte = line_start_byte + run_end;
            let start_char = rope.byte_to_char(abs_start_byte);
            let end_char = rope.byte_to_char(abs_end_byte);

            if beginning_of_line {
                let slice = rope.slice(start_char..end_char);
                let is_whitespace = slice.chars().all(|c| c.is_whitespace() && c != '\n');
                if is_whitespace {
                    let len = end_char - start_char;
                    line_spans.push((
                        SyntaxKind::Whitespace,
                        TextNode::LineOfChars {
                            len,
                            char: '\u{00B7}',
                        },
                    ));
                    run_start = run_end;
                    continue;
                }
                beginning_of_line = false;
            }

            line_spans.push((run_kind, TextNode::Range(start_char..end_char)));
            run_start = run_end;
        }

        syntax_blocks.push_line(line_spans);
    }
}

fn build_plain_blocks(rope: &Rope, syntax_blocks: &mut SyntaxBlocks) {
    for (n, line) in rope.lines().enumerate() {
        let mut line_blocks = SmallVec::default();
        let start = rope.line_to_char(n);
        let end = line.len_chars();
        if end > 0 {
            line_blocks.push((SyntaxKind::Text, TextNode::Range(start..start + end)));
        }
        syntax_blocks.push_line(line_blocks);
    }
}

pub struct RopeTextProvider<'a> {
    rope: &'a Rope,
}

impl<'a> tree_sitter::TextProvider<&'a [u8]> for RopeTextProvider<'a> {
    type I = RopeChunkIter<'a>;

    fn text(&mut self, node: tree_sitter::Node) -> Self::I {
        let start = node.start_byte();
        let end = node.end_byte();
        RopeChunkIter {
            rope: self.rope,
            byte_offset: start,
            end_byte: end,
        }
    }
}

pub struct RopeChunkIter<'a> {
    rope: &'a Rope,
    byte_offset: usize,
    end_byte: usize,
}

impl<'a> Iterator for RopeChunkIter<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        if self.byte_offset >= self.end_byte {
            return None;
        }
        let (chunk, chunk_start, _, _) = self.rope.chunk_at_byte(self.byte_offset);
        let chunk_bytes = chunk.as_bytes();
        let offset_in_chunk = self.byte_offset - chunk_start;
        let available = &chunk_bytes[offset_in_chunk..];
        let remaining = self.end_byte - self.byte_offset;
        let slice = if available.len() > remaining {
            &available[..remaining]
        } else {
            available
        };
        self.byte_offset += slice.len();
        Some(slice)
    }
}

impl EditorLanguage {
    fn lang_config(&self) -> Option<LangConfig> {
        let language = self.language.clone();
        let query = Query::new(&language, self.highlights_query.as_ref()).ok()?;
        let capture_kinds: Vec<SyntaxKind> = query
            .capture_names()
            .iter()
            .map(|name| resolve_capture_kind(name))
            .collect();

        Some(LangConfig {
            language,
            query,
            capture_kinds,
        })
    }
}
