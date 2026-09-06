use freya_engine::prelude::{
    SkTextDecoration,
    TextDecorationStyle as SkTextDecorationStyle,
};

/// A line drawn through, under or over text.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Default, Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum TextDecoration {
    /// No decoration. This is the default.
    #[default]
    None,
    /// A line below the text.
    Underline,
    /// A line above the text.
    Overline,
    /// A line through the middle of the text.
    LineThrough,
}

impl TextDecoration {
    pub fn pretty(&self) -> String {
        match self {
            Self::None => "none".to_string(),
            Self::Underline => "underline".to_string(),
            Self::Overline => "overline".to_string(),
            Self::LineThrough => "line-through".to_string(),
        }
    }
}

impl From<TextDecoration> for SkTextDecoration {
    fn from(value: TextDecoration) -> Self {
        match value {
            TextDecoration::None => SkTextDecoration::NO_DECORATION,
            TextDecoration::Underline => SkTextDecoration::UNDERLINE,
            TextDecoration::Overline => SkTextDecoration::OVERLINE,
            TextDecoration::LineThrough => SkTextDecoration::LINE_THROUGH,
        }
    }
}

/// How a [`TextDecoration`] line is drawn.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Default, Debug, Copy, Clone, Hash, PartialEq, Eq)]
pub enum TextDecorationStyle {
    /// A single straight line. This is the default.
    #[default]
    Solid,
    /// Two parallel straight lines.
    Double,
    /// A dotted line.
    Dotted,
    /// A dashed line.
    Dashed,
    /// A wavy line (e.g. diagnostic squiggles).
    Wavy,
}

impl TextDecorationStyle {
    pub fn pretty(&self) -> String {
        match self {
            Self::Solid => "solid".to_string(),
            Self::Double => "double".to_string(),
            Self::Dotted => "dotted".to_string(),
            Self::Dashed => "dashed".to_string(),
            Self::Wavy => "wavy".to_string(),
        }
    }
}

impl From<TextDecorationStyle> for SkTextDecorationStyle {
    fn from(value: TextDecorationStyle) -> Self {
        match value {
            TextDecorationStyle::Solid => SkTextDecorationStyle::Solid,
            TextDecorationStyle::Double => SkTextDecorationStyle::Double,
            TextDecorationStyle::Dotted => SkTextDecorationStyle::Dotted,
            TextDecorationStyle::Dashed => SkTextDecorationStyle::Dashed,
            TextDecorationStyle::Wavy => SkTextDecorationStyle::Wavy,
        }
    }
}
