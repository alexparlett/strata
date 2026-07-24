use std::{
    borrow::Cow,
    ops::{Mul, Range},
};

use freya_core::prelude::*;
use freya_edit::{
    EditableEvent,
    EditorLine,
    TextEditor,
};
use smallvec::SmallVec;
use torin::{
    gaps::Gaps,
    prelude::Alignment,
    size::Size,
};

use crate::{
    editor_data::{CodeEditorData, Decoration, DecorationSeverity},
    editor_theme::{EditorSyntaxTheme, EditorTheme},
    syntax::TextNode,
};

#[derive(Clone, PartialEq)]
pub struct EditorLineUI {
    pub(crate) editor: Writable<CodeEditorData>,
    pub(crate) font_size: f32,
    pub(crate) line_height: f32,
    pub(crate) line_index: usize,
    pub(crate) read_only: bool,
    pub(crate) gutter: bool,
    pub(crate) show_whitespace: bool,
    pub(crate) highlight_current_line: bool,
    pub(crate) font_family: Cow<'static, str>,
    pub(crate) font_weight: i32,
    pub(crate) theme: EditorTheme,
    pub(crate) syntax_theme: EditorSyntaxTheme,
    pub(crate) a11y_id: AccessibilityId,
}

impl Component for EditorLineUI {
    fn render_key(&self) -> DiffKey {
        DiffKey::from(&self.line_index)
    }
    fn render(&self) -> impl IntoElement {
        let EditorLineUI {
            mut editor,
            font_size,
            line_height,
            line_index,
            read_only,
            gutter,
            show_whitespace,
            highlight_current_line,
            font_family,
            font_weight,
            theme,
            syntax_theme,
            a11y_id,
        } = self.clone();

        let holder = use_state(ParagraphHolder::default);
        

        let editor_data = editor.read();

        let longest_width = editor_data.metrics.longest_width;
        let line = editor_data.metrics.syntax_blocks.get_line(line_index);
        let highlights = editor_data.get_visible_selection(EditorLine::Paragraph(line_index));
        let gutter_width = font_size * 3.0;
        let is_line_selected = editor_data.cursor_row() == line_index;

        let on_tap = {
            let mut editor = editor.clone();
            let font_family = font_family.clone();
            move |e: Event<FocusPressEventData>| {
                let processed = editor.write_if(|mut editor_editor| {
                    editor_editor.process(
                        font_size,
                        &font_family,
                        font_weight,
                        EditableEvent::Down {
                            location: e.element_location(),
                            editor_line: EditorLine::Paragraph(line_index),
                            holder: &holder.read(),
                        },
                    )
                });
                if processed {
                    a11y_id.request_focus();
                }
            }
        };

        let on_pointer_move = {
            let font_family = font_family.clone();
            move |e: Event<PointerEventData>| {
                editor.write_if(|mut editor_editor| {
                    editor_editor.process(
                        font_size,
                        &font_family,
                        font_weight,
                        EditableEvent::Move {
                            location: e.element_location(),
                            editor_line: EditorLine::Paragraph(line_index),
                            holder: &holder.read(),
                        },
                    )
                });
                // Diagnostics hover: which glyph the pointer sits on plus the pointer's
                // x (the popup's anchor). Unchanged positions are a free `write_if`.
                let location = e.element_location();
                let local = holder.read().0.borrow().as_ref().map(|inner| {
                    let glyph = inner
                        .paragraph
                        .get_glyph_position_at_coordinate(
                            location.mul(inner.scale_factor).to_i32().to_tuple(),
                        )
                        .position as usize;
                    (glyph, location.x as f32)
                });
                editor.write_if(|mut editor_editor| {
                    editor_editor.update_hover(line_index, local)
                });
            }
        };

        let cursor_index = if read_only {
            None
        } else {
            is_line_selected.then(|| editor_data.cursor_col())
        };
        let gutter_color = theme.gutter_unselected;
        let visible_selection = match editor_data.get_selection() {
            None => false,
            Some((s, e)) if s != e => true,
            _ => false,
        };
        let line_background = if highlight_current_line && is_line_selected && !visible_selection {
            theme.line_selected_background
        } else {
            Color::TRANSPARENT
        };

        rect()
            .horizontal()
            .height(Size::px(line_height))
            .background(line_background)
            .font_size(font_size)
            .maybe(gutter, |el| {
                el.child(
                    rect()
                        .width(Size::px(gutter_width))
                        .height(Size::fill())
                        .horizontal()
                        .main_align(Alignment::Center)
                        .cross_align(Alignment::Center)
                        .margin(Gaps::new(0., 8., 0., 0.))
                        .child(
                            label()
                                .font_family(font_family.clone())
                                .font_weight(font_weight)
                                .color(gutter_color)
                                .text(format!("{}", line_index + 1)),
                        ),
                )
            })
            .child(
                paragraph()
                    .holder(holder.read().clone())
                    .on_pointer_move(on_pointer_move)
                    .on_focus_press(on_tap)
                    .cursor_color(theme.cursor)
                    .cursor_style(CursorStyle::Line)
                    .cursor_index(cursor_index)
                    .cursor_mode(CursorMode::Expanded)
                    .vertical_align(VerticalAlign::Center)
                    .highlights(highlights.map(|h| vec![h]))
                    .highlight_color(theme.highlight)
                    .width(Size::px(longest_width))
                    .min_width(Size::fill())
                    .height(Size::fill())
                    .font_family(font_family)
                    .font_weight(font_weight)
                    .max_lines(1)
                    .color(theme.text)
                    .spans_iter(line.iter().flat_map(|span| {
                        // A syntax run splits where a diagnostic decoration starts or
                        // ends — the decorated pieces get their wavy underline, the
                        // rest render exactly as before.
                        let mut spans: SmallVec<[Span<'static>; 2]> = SmallVec::new();
                        match &span.1 {
                            TextNode::Range(word_pos) => {
                                for (piece, severity) in
                                    decorate_range(word_pos.clone(), &editor_data.decorations)
                                {
                                    let text = editor_data.rope.slice(piece).to_string();
                                    spans.push(
                                        Span::new(Cow::Owned(text))
                                            .color(syntax_theme.color(span.0))
                                            .map(severity, |s, severity| {
                                                s.text_decoration(TextDecoration::Underline)
                                                    .text_decoration_style(
                                                        TextDecorationStyle::Wavy,
                                                    )
                                                    .text_decoration_color(
                                                        theme.diagnostic(severity),
                                                    )
                                            }),
                                    );
                                }
                            }
                            TextNode::LineOfChars { len, char } => {
                                let text = if show_whitespace {
                                    char.to_string().repeat(*len)
                                } else {
                                    " ".repeat(*len)
                                };
                                spans.push(
                                    Span::new(Cow::Owned(text)).color(syntax_theme.color(span.0)),
                                );
                            }
                        }
                        spans
                    })),
            )
    }
}

/// Split a syntax run's char `range` at the boundaries of the overlapping
/// `decorations`, yielding sub-ranges tagged with the highest-severity decoration
/// covering each (`None` for plain segments). The paragraph needs one [`Span`] per
/// styling change, so a squiggle inside a run becomes its own span; runs with no
/// overlapping decoration come back whole.
fn decorate_range(
    range: Range<usize>,
    decorations: &[Decoration],
) -> SmallVec<[(Range<usize>, Option<DecorationSeverity>); 2]> {
    let mut out = SmallVec::new();
    let overlapping: SmallVec<[&Decoration; 2]> = decorations
        .iter()
        .filter(|d| d.range.start < range.end && d.range.end > range.start)
        .collect();
    if overlapping.is_empty() {
        out.push((range, None));
        return out;
    }
    let mut cuts: SmallVec<[usize; 6]> = SmallVec::new();
    cuts.push(range.start);
    cuts.push(range.end);
    for d in &overlapping {
        for p in [d.range.start, d.range.end] {
            if p > range.start && p < range.end {
                cuts.push(p);
            }
        }
    }
    cuts.sort_unstable();
    cuts.dedup();
    for pair in cuts.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        // Boundaries all sit on cut points, so a decoration either covers the whole
        // sub-range or none of it; overlaps resolve to the highest severity.
        let severity = overlapping
            .iter()
            .filter(|d| d.range.start <= a && b <= d.range.end)
            .map(|d| d.severity)
            .max();
        out.push((a..b, severity));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deco(range: Range<usize>, severity: DecorationSeverity) -> Decoration {
        Decoration { range, severity, message: String::new() }
    }

    #[test]
    fn undecorated_run_stays_whole() {
        let out = decorate_range(0..10, &[deco(20..25, DecorationSeverity::Error)]);
        assert_eq!(out.as_slice(), &[(0..10, None)]);
    }

    #[test]
    fn decoration_inside_a_run_splits_it() {
        let out = decorate_range(0..10, &[deco(3..6, DecorationSeverity::Error)]);
        assert_eq!(
            out.as_slice(),
            &[
                (0..3, None),
                (3..6, Some(DecorationSeverity::Error)),
                (6..10, None),
            ]
        );
    }

    #[test]
    fn decoration_spanning_past_the_run_clamps_to_it() {
        // e.g. a multi-line squiggle: this line's runs only see their own slice.
        let out = decorate_range(5..10, &[deco(0..8, DecorationSeverity::Warning)]);
        assert_eq!(
            out.as_slice(),
            &[(5..8, Some(DecorationSeverity::Warning)), (8..10, None)]
        );
    }

    #[test]
    fn overlapping_decorations_resolve_to_the_highest_severity() {
        let out = decorate_range(
            0..10,
            &[
                deco(0..10, DecorationSeverity::Info),
                deco(4..6, DecorationSeverity::Error),
            ],
        );
        assert_eq!(
            out.as_slice(),
            &[
                (0..4, Some(DecorationSeverity::Info)),
                (4..6, Some(DecorationSeverity::Error)),
                (6..10, Some(DecorationSeverity::Info)),
            ]
        );
    }
}
