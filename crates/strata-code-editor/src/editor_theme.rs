use freya_components::{
    define_theme,
    theming::{
        component_themes::Theme,
        macros::Preference,
    },
};
use freya_core::prelude::Color;

use crate::editor_ui::CodeEditor;
use crate::syntax::SyntaxKind;

define_theme! {
    for = CodeEditor; theme_field = theme;
    pub Editor {
        %[fields]
        background: Color,
        gutter_selected: Color,
        gutter_unselected: Color,
        gutter_border: Color,
        line_selected_background: Color,
        cursor: Color,
        highlight: Color,
        text: Color,
        whitespace: Color,
    }
}

define_theme! {
    for = CodeEditor; theme_field = syntax_theme;

    pub EditorSyntax {
        %[fields]
        text: Color,
        whitespace: Color,
        attribute: Color,
        boolean: Color,
        comment: Color,
        constant: Color,
        constructor: Color,
        escape: Color,
        function: Color,
        function_macro: Color,
        function_method: Color,
        keyword: Color,
        label: Color,
        module: Color,
        number: Color,
        operator: Color,
        property: Color,
        punctuation: Color,
        punctuation_bracket: Color,
        punctuation_delimiter: Color,
        punctuation_special: Color,
        string: Color,
        string_escape: Color,
        string_special: Color,
        tag: Color,
        text_literal: Color,
        text_reference: Color,
        text_title: Color,
        text_uri: Color,
        text_emphasis: Color,
        type_: Color,
        variable: Color,
        variable_builtin: Color,
        variable_parameter: Color,
    }
}

impl EditorTheme {
    pub fn dark() -> Self {
        Self {
            background: Color::from_rgb(29, 32, 33),
            gutter_selected: Color::from_rgb(235, 235, 235),
            gutter_unselected: Color::from_rgb(135, 135, 135),
            gutter_border: Color::from_rgb(60, 60, 60),
            line_selected_background: Color::from_rgb(55, 55, 55),
            cursor: Color::WHITE,
            highlight: Color::from_rgb(80, 80, 80),
            text: Color::WHITE,
            whitespace: Color::from_af32rgb(0.2, 223, 191, 142),
        }
    }

    pub fn light() -> Self {
        Self {
            background: Color::from_rgb(246, 248, 250),
            gutter_selected: Color::from_rgb(36, 41, 46),
            gutter_unselected: Color::from_rgb(140, 149, 159),
            gutter_border: Color::from_rgb(210, 213, 218),
            line_selected_background: Color::from_rgb(234, 238, 242),
            cursor: Color::from_rgb(36, 41, 46),
            highlight: Color::from_rgb(200, 225, 255),
            text: Color::from_rgb(36, 41, 46),
            whitespace: Color::from_af32rgb(0.3, 106, 115, 125),
        }
    }
}

impl EditorSyntaxTheme {
    pub fn dark() -> Self {
        Self {
            text: Color::from_rgb(235, 219, 178),
            whitespace: Color::from_af32rgb(0.2, 223, 191, 142),
            attribute: Color::from_rgb(131, 165, 152),
            boolean: Color::from_rgb(211, 134, 155),
            comment: Color::from_rgb(146, 131, 116),
            constant: Color::from_rgb(211, 134, 155),
            constructor: Color::from_rgb(250, 189, 47),
            escape: Color::from_rgb(254, 128, 25),
            function: Color::from_rgb(152, 192, 124),
            function_macro: Color::from_rgb(131, 165, 152),
            function_method: Color::from_rgb(152, 192, 124),
            keyword: Color::from_rgb(251, 73, 52),
            label: Color::from_rgb(211, 134, 155),
            module: Color::from_rgb(250, 189, 47),
            number: Color::from_rgb(211, 134, 155),
            operator: Color::from_rgb(104, 157, 96),
            property: Color::from_rgb(152, 192, 124),
            punctuation: Color::from_rgb(104, 157, 96),
            punctuation_bracket: Color::from_rgb(254, 128, 25),
            punctuation_delimiter: Color::from_rgb(104, 157, 96),
            punctuation_special: Color::from_rgb(131, 165, 152),
            string: Color::from_rgb(184, 187, 38),
            string_escape: Color::from_rgb(254, 128, 25),
            string_special: Color::from_rgb(184, 187, 38),
            tag: Color::from_rgb(131, 165, 152),
            text_literal: Color::from_rgb(235, 219, 178),
            text_reference: Color::from_rgb(131, 165, 152),
            text_title: Color::from_rgb(250, 189, 47),
            text_uri: Color::from_rgb(104, 157, 96),
            text_emphasis: Color::from_rgb(235, 219, 178),
            type_: Color::from_rgb(250, 189, 47),
            variable: Color::from_rgb(235, 219, 178),
            variable_builtin: Color::from_rgb(211, 134, 155),
            variable_parameter: Color::from_rgb(235, 219, 178),
        }
    }

    pub fn light() -> Self {
        Self {
            text: Color::from_rgb(36, 41, 46),
            whitespace: Color::from_af32rgb(0.3, 106, 115, 125),
            attribute: Color::from_rgb(0, 92, 197),
            boolean: Color::from_rgb(0, 92, 197),
            comment: Color::from_rgb(106, 115, 125),
            constant: Color::from_rgb(0, 92, 197),
            constructor: Color::from_rgb(111, 66, 193),
            escape: Color::from_rgb(227, 98, 9),
            function: Color::from_rgb(111, 66, 193),
            function_macro: Color::from_rgb(111, 66, 193),
            function_method: Color::from_rgb(111, 66, 193),
            keyword: Color::from_rgb(215, 58, 73),
            label: Color::from_rgb(215, 58, 73),
            module: Color::from_rgb(227, 98, 9),
            number: Color::from_rgb(0, 92, 197),
            operator: Color::from_rgb(215, 58, 73),
            property: Color::from_rgb(0, 92, 197),
            punctuation: Color::from_rgb(36, 41, 46),
            punctuation_bracket: Color::from_rgb(36, 41, 46),
            punctuation_delimiter: Color::from_rgb(36, 41, 46),
            punctuation_special: Color::from_rgb(215, 58, 73),
            string: Color::from_rgb(3, 47, 98),
            string_escape: Color::from_rgb(227, 98, 9),
            string_special: Color::from_rgb(3, 47, 98),
            tag: Color::from_rgb(34, 134, 58),
            text_literal: Color::from_rgb(3, 47, 98),
            text_reference: Color::from_rgb(0, 92, 197),
            text_title: Color::from_rgb(0, 92, 197),
            text_uri: Color::from_rgb(3, 47, 98),
            text_emphasis: Color::from_rgb(36, 41, 46),
            type_: Color::from_rgb(111, 66, 193),
            variable: Color::from_rgb(36, 41, 46),
            variable_builtin: Color::from_rgb(0, 92, 197),
            variable_parameter: Color::from_rgb(227, 98, 9),
        }
    }
}

impl Default for EditorTheme {
    fn default() -> Self {
        Self::light()
    }
}

impl Default for EditorSyntaxTheme {
    fn default() -> Self {
        Self::light()
    }
}

impl EditorSyntaxTheme {
    /// The colour for a highlight class — the editor's kind→colour map, applied at render time so
    /// the buffer stays theme-independent.
    pub fn color(&self, kind: SyntaxKind) -> Color {
        match kind {
            SyntaxKind::Text => self.text,
            SyntaxKind::Whitespace => self.whitespace,
            SyntaxKind::Attribute => self.attribute,
            SyntaxKind::Boolean => self.boolean,
            SyntaxKind::Comment => self.comment,
            SyntaxKind::Constant => self.constant,
            SyntaxKind::Constructor => self.constructor,
            SyntaxKind::Escape => self.escape,
            SyntaxKind::Function => self.function,
            SyntaxKind::FunctionMacro => self.function_macro,
            SyntaxKind::FunctionMethod => self.function_method,
            SyntaxKind::Keyword => self.keyword,
            SyntaxKind::Label => self.label,
            SyntaxKind::Module => self.module,
            SyntaxKind::Number => self.number,
            SyntaxKind::Operator => self.operator,
            SyntaxKind::Property => self.property,
            SyntaxKind::Punctuation => self.punctuation,
            SyntaxKind::PunctuationBracket => self.punctuation_bracket,
            SyntaxKind::PunctuationDelimiter => self.punctuation_delimiter,
            SyntaxKind::PunctuationSpecial => self.punctuation_special,
            SyntaxKind::String => self.string,
            SyntaxKind::StringEscape => self.string_escape,
            SyntaxKind::StringSpecial => self.string_special,
            SyntaxKind::Tag => self.tag,
            SyntaxKind::TextLiteral => self.text_literal,
            SyntaxKind::TextReference => self.text_reference,
            SyntaxKind::TextTitle => self.text_title,
            SyntaxKind::TextUri => self.text_uri,
            SyntaxKind::TextEmphasis => self.text_emphasis,
            SyntaxKind::Type => self.type_,
            SyntaxKind::Variable => self.variable,
            SyntaxKind::VariableBuiltin => self.variable_builtin,
            SyntaxKind::VariableParameter => self.variable_parameter,
        }
    }
}

/// Wraps a resolved [`EditorTheme`] into a preference of `Specific` values.
impl From<EditorTheme> for EditorThemePreference {
    fn from(theme: EditorTheme) -> Self {
        Self {
            background: Preference::Specific(theme.background),
            gutter_selected: Preference::Specific(theme.gutter_selected),
            gutter_unselected: Preference::Specific(theme.gutter_unselected),
            gutter_border: Preference::Specific(theme.gutter_border),
            line_selected_background: Preference::Specific(theme.line_selected_background),
            cursor: Preference::Specific(theme.cursor),
            highlight: Preference::Specific(theme.highlight),
            text: Preference::Specific(theme.text),
            whitespace: Preference::Specific(theme.whitespace),
        }
    }
}

impl From<EditorSyntaxTheme> for EditorSyntaxThemePreference {
    fn from(value: EditorSyntaxTheme) -> Self {
        Self {
            text: Preference::Specific(value.text),
            whitespace: Preference::Specific(value.whitespace),
            attribute: Preference::Specific(value.attribute),
            boolean: Preference::Specific(value.boolean),
            comment: Preference::Specific(value.comment),
            constant: Preference::Specific(value.constant),
            constructor: Preference::Specific(value.constructor),
            escape: Preference::Specific(value.escape),
            function: Preference::Specific(value.function),
            function_macro: Preference::Specific(value.function_macro),
            function_method: Preference::Specific(value.function_method),
            keyword: Preference::Specific(value.keyword),
            label: Preference::Specific(value.label),
            module: Preference::Specific(value.module),
            number: Preference::Specific(value.number),
            operator: Preference::Specific(value.operator),
            property: Preference::Specific(value.property),
            punctuation: Preference::Specific(value.punctuation),
            punctuation_bracket: Preference::Specific(value.punctuation_bracket),
            punctuation_delimiter: Preference::Specific(value.punctuation_delimiter),
            punctuation_special: Preference::Specific(value.punctuation_special),
            string: Preference::Specific(value.string),
            string_escape: Preference::Specific(value.string_escape),
            string_special: Preference::Specific(value.string_special),
            tag: Preference::Specific(value.tag),
            text_literal: Preference::Specific(value.text_literal),
            text_reference: Preference::Specific(value.text_reference),
            text_title: Preference::Specific(value.text_title),
            text_uri: Preference::Specific(value.text_uri),
            text_emphasis: Preference::Specific(value.text_emphasis),
            type_: Preference::Specific(value.type_),
            variable: Preference::Specific(value.variable),
            variable_builtin: Preference::Specific(value.variable_builtin),
            variable_parameter: Preference::Specific(value.variable_parameter),
        }
    }
}

/// Registers code editor themes into a [`Theme`], e.g. `dark_theme().with_dark_code_editor()`.
pub trait CodeEditorThemeExt {
    fn with_dark_code_editor(self) -> Self;
    fn with_light_code_editor(self) -> Self;
}

impl CodeEditorThemeExt for Theme {
    fn with_dark_code_editor(mut self) -> Self {
        self.set(
            "code_editor",
            EditorThemePreference::from(EditorTheme::dark()),
        );
        self.set("code_editor_syntax", EditorSyntaxTheme::dark());
        self
    }

    fn with_light_code_editor(mut self) -> Self {
        self.set(
            "code_editor",
            EditorThemePreference::from(EditorTheme::light()),
        );
        self.set("code_editor_syntax", EditorSyntaxTheme::light());
        self
    }
}
