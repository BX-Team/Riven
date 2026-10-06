use std::ops::Range;
use std::sync::Arc;

use gpui_kit::base::input::{
    FoldRange, HighlightStyleResolver, InputEdit, InputHighlighter, InputHighlighterFactory, Rope,
};
use gpui_kit::{Context, FontWeight, HighlightStyle, Hsla, SharedString, Window};

use crate::gui::theme::Palette;

/// How a config file is lexed, from its extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavor {
    /// TOML, `.cfg`, `.ini`, `.properties`: `#`/`;` comments, `[headers]`, `key = value`.
    Toml,
    Json,
    Yaml,
    /// KubeJS scripts and `.json5`/`.snbt`-like files with `//` comments.
    Script,
    Plain,
}

impl Flavor {
    pub fn of(file_name: &str) -> Self {
        let ext = file_name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("toml" | "cfg" | "ini" | "properties" | "conf") => Self::Toml,
            Some("json" | "mcmeta") => Self::Json,
            Some("yml" | "yaml") => Self::Yaml,
            Some("js" | "ts" | "json5" | "snbt" | "zs") => Self::Script,
            _ => Self::Plain,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Toml => "toml",
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::Script => "script",
            Self::Plain => "text",
        }
    }

    fn from_name(name: &str) -> Self {
        match name {
            "toml" => Self::Toml,
            "json" => Self::Json,
            "yaml" => Self::Yaml,
            "script" => Self::Script,
            _ => Self::Plain,
        }
    }

    pub fn language(self) -> SharedString {
        self.name().into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Token {
    Comment,
    String,
    Number,
    Keyword,
    Key,
    Header,
}

impl Token {
    fn style_name(self) -> &'static str {
        match self {
            Self::Comment => "comment",
            Self::String => "string",
            Self::Number => "number",
            Self::Keyword => "keyword",
            Self::Key => "property",
            Self::Header => "title",
        }
    }
}

const KEYWORDS: &[&str] = &[
    "true",
    "false",
    "null",
    "yes",
    "no",
    "on",
    "off",
    "const",
    "let",
    "var",
    "function",
    "return",
    "if",
    "else",
    "for",
    "while",
    "new",
    "this",
    "undefined",
];

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.' || b >= 0x80
}

/// Byte ranges of the tokens in `text`; everything between them is plain.
pub fn lex(text: &str, flavor: Flavor) -> Vec<(Range<usize>, Token)> {
    let mut out = Vec::new();
    if flavor == Flavor::Plain {
        return out;
    }
    let bytes = text.as_bytes();
    let line_comment = |at: usize| -> bool {
        match flavor {
            Flavor::Toml | Flavor::Yaml => {
                bytes[at] == b'#' || (flavor == Flavor::Toml && bytes[at] == b';')
            }
            Flavor::Script | Flavor::Json => bytes[at..].starts_with(b"//"),
            Flavor::Plain => false,
        }
    };
    let mut line_start = true;
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'\n' {
            line_start = true;
            i += 1;
            continue;
        }
        if b == b' ' || b == b'\t' || b == b'\r' {
            i += 1;
            continue;
        }
        let at_line_start = std::mem::replace(&mut line_start, false);
        let end_of_line = text[i..].find('\n').map_or(bytes.len(), |n| i + n);
        if line_comment(i) {
            out.push((i..end_of_line, Token::Comment));
            i = end_of_line;
            continue;
        }
        if matches!(flavor, Flavor::Script | Flavor::Json) && bytes[i..].starts_with(b"/*") {
            let end = text[i + 2..]
                .find("*/")
                .map_or(bytes.len(), |n| i + 2 + n + 2);
            out.push((i..end, Token::Comment));
            i = end;
            continue;
        }
        if flavor == Flavor::Toml && at_line_start && b == b'[' {
            out.push((i..end_of_line, Token::Header));
            i = end_of_line;
            continue;
        }
        if b == b'"' || b == b'\'' || (b == b'`' && flavor == Flavor::Script) {
            let mut j = i + 1;
            while j < bytes.len() && bytes[j] != b && bytes[j] != b'\n' {
                j += if bytes[j] == b'\\' { 2 } else { 1 };
            }
            let end = (j + 1).min(bytes.len());
            let after = text[end..].trim_start_matches([' ', '\t']);
            let key = (flavor == Flavor::Json || flavor == Flavor::Yaml) && after.starts_with(':')
                || flavor == Flavor::Toml && at_line_start && after.starts_with('=');
            out.push((i..end, if key { Token::Key } else { Token::String }));
            i = end;
            continue;
        }
        if is_word(b) {
            let mut j = i;
            while j < bytes.len() && is_word(bytes[j]) {
                j += 1;
            }
            let word = &text[i..j];
            let after = text[j..].trim_start_matches([' ', '\t']);
            let token = if at_line_start
                && match flavor {
                    Flavor::Toml => after.starts_with('='),
                    Flavor::Yaml => after.starts_with(':'),
                    _ => false,
                } {
                Some(Token::Key)
            } else if word.parse::<f64>().is_ok() || word.starts_with("0x") {
                Some(Token::Number)
            } else if KEYWORDS.contains(&word) {
                Some(Token::Keyword)
            } else {
                None
            };
            if let Some(token) = token {
                out.push((i..j, token));
            }
            i = j;
            continue;
        }
        i += text[i..].chars().next().map_or(1, char::len_utf8);
    }
    out
}

/// Highlights a config file by lexing it again on every edit; config files are small.
struct ConfigHighlighter {
    flavor: Flavor,
    tokens: Vec<(Range<usize>, Token)>,
}

impl InputHighlighter for ConfigHighlighter {
    fn language(&self) -> SharedString {
        self.flavor.language()
    }

    fn update(
        &mut self,
        _: Option<InputEdit>,
        text: &Rope,
        _: bool,
        _: &mut Window,
        _: &mut Context<gpui_kit::base::input::EditorState>,
    ) {
        self.tokens = lex(&text.to_string(), self.flavor);
    }

    fn styles(
        &self,
        range: &Range<usize>,
        resolver: &dyn HighlightStyleResolver,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let mut out = Vec::new();
        let mut at = range.start;
        let first = self.tokens.partition_point(|(r, _)| r.end <= range.start);
        for (r, token) in &self.tokens[first..] {
            if r.start >= range.end {
                break;
            }
            let start = r.start.max(range.start);
            let end = r.end.min(range.end);
            if start > at {
                out.push((at..start, HighlightStyle::default()));
            }
            let style = resolver.style(token.style_name()).unwrap_or_default();
            out.push((start..end, style));
            at = end;
        }
        if at < range.end {
            out.push((at..range.end, HighlightStyle::default()));
        }
        out
    }

    fn fold_ranges(&self, _: &Rope) -> Vec<FoldRange> {
        Vec::new()
    }
}

pub fn factory() -> InputHighlighterFactory {
    std::rc::Rc::new(|language: &str| {
        Some(Box::new(ConfigHighlighter {
            flavor: Flavor::from_name(language),
            tokens: Vec::new(),
        }) as Box<dyn InputHighlighter>)
    })
}

/// Syntax colors taken from the theme's tokens.
pub struct ThemeStyles {
    comment: Hsla,
    string: Hsla,
    number: Hsla,
    key: Hsla,
    header: Hsla,
}

impl ThemeStyles {
    pub fn new(c: &Palette) -> Arc<Self> {
        Arc::new(Self {
            comment: c.muted,
            string: c.ok,
            number: c.warn,
            key: c.accent,
            header: c.accent,
        })
    }
}

impl HighlightStyleResolver for ThemeStyles {
    fn style(&self, name: &str) -> Option<HighlightStyle> {
        let color = |c: Hsla| HighlightStyle {
            color: Some(c),
            ..HighlightStyle::default()
        };
        Some(match name {
            "comment" => HighlightStyle {
                font_style: Some(gpui_kit::FontStyle::Italic),
                ..color(self.comment)
            },
            "string" => color(self.string),
            "number" | "keyword" => color(self.number),
            "property" => color(self.key),
            "title" => HighlightStyle {
                font_weight: Some(FontWeight::BOLD),
                ..color(self.header)
            },
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(text: &str, flavor: Flavor) -> Vec<(&str, Token)> {
        lex(text, flavor)
            .into_iter()
            .map(|(r, t)| (&text[r], t))
            .collect()
    }

    #[test]
    fn toml_keys_values_headers_and_comments() {
        let text = "[worldgen]\n  disable = false # off\nname = \"a # not a comment\"\nn = -20\n";
        assert_eq!(
            tokens(text, Flavor::Toml),
            [
                ("[worldgen]", Token::Header),
                ("disable", Token::Key),
                ("false", Token::Keyword),
                ("# off", Token::Comment),
                ("name", Token::Key),
                ("\"a # not a comment\"", Token::String),
                ("n", Token::Key),
                ("-20", Token::Number),
            ]
        );
    }

    #[test]
    fn json_keys_differ_from_string_values_and_utf8_is_safe() {
        let text = "{\"имя\": \"значение\", \"n\": 1.5}";
        assert_eq!(
            tokens(text, Flavor::Json),
            [
                ("\"имя\"", Token::Key),
                ("\"значение\"", Token::String),
                ("\"n\"", Token::Key),
                ("1.5", Token::Number),
            ]
        );
    }
}
