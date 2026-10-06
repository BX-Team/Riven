/// A block of a project description, simplified for plain text drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Heading(String),
    Paragraph(String),
    Item(String),
}

/// Drops HTML tags, images and link targets, keeping the words a reader sees.
fn inline(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        if c == '<' {
            match rest.find('>') {
                Some(end) => rest = &rest[end + 1..],
                None => rest = "",
            }
            continue;
        }
        if rest.starts_with("![") {
            rest = skip_link(&rest[1..]).unwrap_or(&rest[2..]);
            continue;
        }
        if c == '['
            && let Some((text, after)) = link_text(rest)
        {
            out.push_str(&inline(text));
            rest = after;
            continue;
        }
        if rest.starts_with("**") || rest.starts_with("__") || rest.starts_with("~~") {
            rest = &rest[2..];
            continue;
        }
        if c == '`' {
            rest = &rest[1..];
            continue;
        }
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// `[text](url)` at the start of `s`: the text and what follows the link.
fn link_text(s: &str) -> Option<(&str, &str)> {
    let close = s.find("](")?;
    let end = s[close..].find(')')? + close;
    Some((&s[1..close], &s[end + 1..]))
}

fn skip_link(s: &str) -> Option<&str> {
    link_text(s).map(|(_, after)| after)
}

/// Splits Markdown into headings, paragraphs and list items; code blocks and tables are left out.
pub fn blocks(markdown: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut code = false;
    let flush = |paragraph: &mut Vec<String>, out: &mut Vec<Block>| {
        let text = paragraph.join(" ");
        paragraph.clear();
        if !text.trim().is_empty() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_keep_words_and_drop_markup() {
        let md = "# Sodium\n\n<p align=\"center\"><img src=\"x.png\"></p>\n\n![banner](https://x/b.png)\n\
                  A **fast** [rendering](https://x) engine\nfor `Minecraft`.\n\n- one\n2. two\n\n\
                  ```\ncode\n```\n| a | b |\n|---|---|\n---\n## Links &amp; more";
        assert_eq!(
            blocks(md),
            [
                Block::Heading("Sodium".into()),
                Block::Paragraph("A fast rendering engine for Minecraft.".into()),
                Block::Item("one".into()),
                Block::Item("two".into()),
                Block::Heading("Links & more".into()),
            ]
        );
    }
}
