use std::ops::Range;

use gpui_kit::{
    AnyElement, App, FontWeight, HighlightStyle, InteractiveText, IntoElement, ParentElement as _,
    Styled as _, StyledText, UnderlineStyle, div, px, relative,
};

use moonglass::h_flex;
use moonglass::theme::ActiveTheme as _;

/// A Modrinth project page with its description split into blocks once.
pub struct Described {
    pub page: riven_sources::modrinth::ProjectPage,
    pub about: Vec<Block>,
}

/// Blocks of a description shown before it is cut off.
const SHOWN: usize = 80;

impl Described {
    pub fn new(page: riven_sources::modrinth::ProjectPage) -> Self {
        let mut about = blocks(&page.body);
        about.truncate(SHOWN);
        Self { page, about }
    }
}

impl std::ops::Deref for Described {
    type Target = riven_sources::modrinth::ProjectPage;

    fn deref(&self) -> &Self::Target {
        &self.page
    }
}

/// A block of a project description, simplified for plain text drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(Rich),
    Paragraph(Rich),
    Item(Rich),
}

/// Text with the byte ranges that are bold or link somewhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rich {
    pub text: String,
    pub bold: Vec<Range<usize>>,
    pub links: Vec<(Range<usize>, String)>,
}

impl Rich {
    fn push(&mut self, s: &str) {
        for c in s.chars() {
            if c.is_whitespace() {
                if !self.text.is_empty() && !self.text.ends_with(' ') {
                    self.text.push(' ');
                }
            } else {
                self.text.push(c);
            }
        }
    }

    /// Appends `other` after a space, shifting its ranges.
    fn append(&mut self, other: Rich) {
        if !self.text.is_empty() && !other.text.is_empty() {
            self.text.push(' ');
        }
        let at = self.text.len();
        self.text.push_str(&other.text);
        self.bold
            .extend(other.bold.into_iter().map(|r| r.start + at..r.end + at));
        self.links.extend(
            other
                .links
                .into_iter()
                .map(|(r, url)| (r.start + at..r.end + at, url)),
        );
    }

    /// Drops the trailing space and ranges that ended up empty.
    fn finish(mut self) -> Self {
        let len = self.text.trim_end().len();
        self.text.truncate(len);
        let fit = |r: &mut Range<usize>| {
            r.end = r.end.min(len);
            r.start < r.end
        };
        self.bold.retain_mut(fit);
        self.links.retain_mut(|(r, _)| fit(r));
        self
    }

    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

const ENTITIES: [(&str, &str); 5] = [
    ("&nbsp;", " "),
    ("&amp;", "&"),
    ("&lt;", "<"),
    ("&gt;", ">"),
    ("&quot;", "\""),
];

/// Keeps the words a reader sees, with bold spans and links; drops HTML tags and images.
fn inline(line: &str) -> Rich {
    let mut out = Rich::default();
    walk(line, &mut out);
    out.finish()
}

fn walk(line: &str, out: &mut Rich) {
    let mut rest = line;
    let mut bold: Option<usize> = None;
    let mut anchor: Option<(usize, String)> = None;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            let Some(end) = rest.find('>') else {
                break;
            };
            let tag = &rest[1..end];
            if is_url(tag) {
                link(out, tag, tag.to_owned());
            } else if tag.starts_with("a ") || tag.starts_with("A ") {
                anchor = href(tag).map(|url| (out.text.len(), url));
            } else if tag.eq_ignore_ascii_case("/a")
                && let Some((start, url)) = anchor.take()
            {
                out.links.push((start..out.text.len(), url));
            }
            rest = &rest[end + 1..];
            continue;
        }
        if rest.starts_with("![") {
            rest = link_parts(&rest[1..]).map_or(&rest[2..], |(_, _, after)| after);
            continue;
        }
        if c == '['
            && let Some((text, url, after)) = link_parts(rest)
        {
            let start = out.text.len();
            walk(text, out);
            if is_url(url) && out.text.trim_end().len() > start {
                out.links
                    .push((start..out.text.trim_end().len(), url.to_owned()));
            }
            rest = after;
            continue;
        }
        if rest.starts_with("**") || rest.starts_with("__") {
            match bold.take() {
                Some(start) => out.bold.push(start..out.text.len()),
                None => bold = Some(out.text.len()),
            }
            rest = &rest[2..];
            continue;
        }
        if rest.starts_with("~~") {
            rest = &rest[2..];
            continue;
        }
        if c == '`' {
            rest = &rest[1..];
            continue;
        }
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let end = rest
                .find(|c: char| c.is_whitespace() || matches!(c, '<' | ')' | ']' | '"'))
                .unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            link(out, url, url.to_owned());
            rest = &rest[url.len()..];
            continue;
        }
        if c == '&'
            && let Some((entity, text)) = ENTITIES.iter().find(|(e, _)| rest.starts_with(e))
        {
            out.push(text);
            rest = &rest[entity.len()..];
            continue;
        }
        out.push(&rest[..c.len_utf8()]);
        rest = &rest[c.len_utf8()..];
    }
}

fn link(out: &mut Rich, text: &str, url: String) {
    let start = out.text.len();
    out.push(text);
    out.links.push((start..out.text.len(), url));
}

fn is_url(s: &str) -> bool {
    (s.starts_with("https://") || s.starts_with("http://")) && !s.contains(char::is_whitespace)
}

/// The `href` of an `<a …>` tag.
fn href(tag: &str) -> Option<String> {
    let at = tag.find("href=")? + 5;
    let value = &tag[at..];
    let quote = value.chars().next().filter(|q| matches!(q, '"' | '\''))?;
    let value = &value[1..];
    let url = &value[..value.find(quote)?];
    is_url(url).then(|| url.to_owned())
}

/// `[text](url "title")` at the start of `s`, brackets nested: the text, the url, what follows.
fn link_parts(s: &str) -> Option<(&str, &str, &str)> {
    let close = matching(s, '[', ']')?;
    let end = matching(&s[close + 1..], '(', ')')? + close + 1;
    let url = s[close + 2..end].split_whitespace().next().unwrap_or("");
    Some((&s[1..close], url, &s[end + 1..]))
}

/// The index of the bracket closing the one `s` starts with.
fn matching(s: &str, open: char, close: char) -> Option<usize> {
    if !s.starts_with(open) {
        return None;
    }
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Splits Markdown into headings, paragraphs and list items; code blocks and tables are left out.
pub fn blocks(markdown: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut paragraph: Vec<Rich> = Vec::new();
    let mut code = false;
    let flush = |paragraph: &mut Vec<Rich>, out: &mut Vec<Block>| {
        let mut text = Rich::default();
        for line in paragraph.drain(..) {
            text.append(line);
        }
        if !text.is_empty() {
            out.push(Block::Paragraph(text));
        }
    };
    for raw in markdown.lines() {
        let line = raw.trim();
        if line.starts_with("```") || line.starts_with("~~~") {
            code = !code;
            flush(&mut paragraph, &mut out);
            continue;
        }
        if code
            || line.starts_with('|')
            || line.chars().all(|c| matches!(c, '-' | '*' | '_' | '=')) && !line.is_empty()
        {
            flush(&mut paragraph, &mut out);
            continue;
        }
        if line.is_empty() {
            flush(&mut paragraph, &mut out);
            continue;
        }
        let heading = line.trim_start_matches('#');
        if heading.len() < line.len() && heading.starts_with(' ') {
            flush(&mut paragraph, &mut out);
            let text = inline(heading);
            if !text.is_empty() {
                out.push(Block::Heading(text));
            }
            continue;
        }
        let item = ["- ", "* ", "+ "]
            .iter()
            .find_map(|m| line.strip_prefix(m))
            .or_else(|| {
                let digits = line.find(|c: char| !c.is_ascii_digit())?;
                (digits > 0).then(|| line[digits..].strip_prefix(". "))?
            });
        if let Some(item) = item {
            flush(&mut paragraph, &mut out);
            let text = inline(item);
            if !text.is_empty() {
                out.push(Block::Item(text));
            }
            continue;
        }
        let text = inline(line.trim_start_matches('>').trim());
        if !text.is_empty() {
            paragraph.push(text);
        }
    }
    flush(&mut paragraph, &mut out);
    out
}

/// Draws blocks as headings, paragraphs and bulleted items, with clickable links.
pub fn view<'a>(blocks: impl IntoIterator<Item = &'a Block>, cx: &App) -> Vec<AnyElement> {
    let c = cx.theme().colors;
    blocks
        .into_iter()
        .enumerate()
        .map(|(i, block)| match block {
            Block::Heading(text) => div()
                .pt(px(4.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(rich(i, text, cx))
                .into_any_element(),
            Block::Paragraph(text) => div()
                .text_color(c.text2)
                .line_height(relative(1.5))
                .child(rich(i, text, cx))
                .into_any_element(),
            Block::Item(text) => h_flex()
                .items_start()
                .gap(px(8.))
                .text_color(c.text2)
                .line_height(relative(1.5))
                .child(div().flex_none().child("•"))
                .child(div().flex_1().min_w_0().child(rich(i, text, cx)))
                .into_any_element(),
        })
        .collect()
}

fn rich(i: usize, text: &Rich, cx: &App) -> AnyElement {
    let c = cx.theme().colors;
    let bold = text.bold.iter().map(|r| {
        let style = HighlightStyle {
            font_weight: Some(FontWeight::SEMIBOLD),
            ..HighlightStyle::default()
        };
        (r.clone(), style)
    });
    let links = text.links.iter().map(|(r, _)| {
        let style = HighlightStyle {
            color: Some(c.accent),
            underline: Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(c.accent),
                wavy: false,
            }),
            ..HighlightStyle::default()
        };
        (r.clone(), style)
    });
    let styled =
        StyledText::new(text.text.clone()).with_highlights(merge(bold.chain(links).collect()));
    if text.links.is_empty() {
        return styled.into_any_element();
    }
    let urls: Vec<String> = text.links.iter().map(|(_, url)| url.clone()).collect();
    let ranges = text.links.iter().map(|(r, _)| r.clone()).collect();
    InteractiveText::new(("markdown", i), styled)
        .on_click(ranges, move |ix, _, cx| cx.open_url(&urls[ix]))
        .into_any_element()
}

/// Splits overlapping highlights so a bold link keeps both styles.
fn merge(spans: Vec<(Range<usize>, HighlightStyle)>) -> Vec<(Range<usize>, HighlightStyle)> {
    let mut cuts: Vec<usize> = spans.iter().flat_map(|(r, _)| [r.start, r.end]).collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut out = Vec::new();
    for w in cuts.windows(2) {
        let piece = w[0]..w[1];
        let style = spans
            .iter()
            .filter(|(r, _)| r.start <= piece.start && piece.end <= r.end)
            .fold(None::<HighlightStyle>, |acc, (_, s)| {
                Some(acc.map_or(*s, |a| a.highlight(*s)))
            });
        if let Some(style) = style {
            out.push((piece, style));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(block: &Block) -> &str {
        match block {
            Block::Heading(t) | Block::Paragraph(t) | Block::Item(t) => &t.text,
        }
    }

    #[test]
    fn descriptions_keep_words_and_drop_markup() {
        let md = "# Sodium\n\n<p align=\"center\"><img src=\"x.png\"></p>\n\n![banner](https://x/b.png)\n\
                  A **fast** [rendering](https://x) engine\nfor `Minecraft`.\n\n- one\n2. two\n\n\
                  ```\ncode\n```\n| a | b |\n|---|---|\n---\n## Links &amp; more";
        let blocks = blocks(md);
        let text: Vec<&str> = blocks.iter().map(plain).collect();
        assert_eq!(
            text,
            [
                "Sodium",
                "A fast rendering engine for Minecraft.",
                "one",
                "two",
                "Links & more"
            ]
        );
        let Block::Paragraph(p) = &blocks[1] else {
            panic!()
        };
        assert_eq!(&p.text[p.bold[0].clone()], "fast");
        assert_eq!(p.links, [(7..16, "https://x".to_owned())]);
    }

    #[test]
    fn nested_and_html_links() {
        let rich = inline(
            "[![badge](https://img)](https://a) see [the [wiki]](https://w \"t\") \
             or <a href=\"https://h\">here</a>, https://raw.link.",
        );
        assert_eq!(rich.text, "see the [wiki] or here, https://raw.link.");
        let links: Vec<(&str, &str)> = rich
            .links
            .iter()
            .map(|(r, url)| (&rich.text[r.clone()], url.as_str()))
            .collect();
        assert_eq!(
            links,
            [
                ("the [wiki]", "https://w"),
                ("here", "https://h"),
                ("https://raw.link", "https://raw.link"),
            ]
        );
    }
}
