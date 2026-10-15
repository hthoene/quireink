//! Markdown (`.md`, `.markdown`): turned into HTML line by line once, then
//! read like an HTML file (see [`crate::html`]). Headings, paragraphs,
//! lists, block quotes, code blocks, rules, emphasis, inline code and
//! links; the first heading is the title.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::document::{Block, Document, DocumentError, Link, Locator, Metadata, TocEntry};
use quireink_sdk::{Env, Storage};

use crate::PLUGIN;
use crate::html::HtmlDocument;

/// Changes whenever Markdown is turned into HTML differently.
const MD_VERSION: u32 = 1;

/// An open Markdown file.
#[derive(Debug)]
pub struct MarkdownDocument {
    html: HtmlDocument,
    metadata: Metadata,
}

fn fnv(data: &[u8]) -> u64 {
    data.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

impl MarkdownDocument {
    pub fn open(storage: &dyn Storage, path: &str, styles: bool) -> Result<Self, DocumentError> {
        let size = storage.size(path)?;
        let mut head = [0u8; 4096];
        let n = storage.read_at(path, 0, &mut head)?;
        let dir = format!(
            "{}/md-{:016x}",
            Env::plugin_dir(PLUGIN.as_str()),
            fnv(path.as_bytes())
        );
        let html_path = format!("{dir}/book.html");
        let stamp = format!("{MD_VERSION} {size} {:016x}\n", fnv(&head[..n]));
        let stamp_path = format!("{dir}/source");
        if quireink_sdk::read_file(storage, &stamp_path, 128).ok()
            != Some(stamp.clone().into_bytes())
            || storage.size(&html_path).is_err()
        {
            let _ = storage.create_dir(&dir);
            convert(storage, path, &html_path)?;
            storage.write(&stamp_path, stamp.as_bytes())?;
        }
        let html = HtmlDocument::open(storage, &html_path, styles)?;
        let mut metadata = html.metadata().clone();
        if metadata.title.as_deref().is_none_or(|t| t == "book") {
            let name = path.rsplit('/').next().unwrap_or(path);
            metadata.title = Some(name.rsplit_once('.').map_or(name, |(s, _)| s).to_string());
        }
        Ok(Self { html, metadata })
    }
}

/// Escapes text for HTML.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// A line's inline markup as HTML: `**bold**`, `*italic*`, `_italic_`,
/// `` `code` ``, `[text](url)`, `![alt](src)` (its alternative text).
fn inline(text: &str, out: &mut String) {
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let (mut bold, mut italic) = (false, false);
    let rest = |i: usize| -> String { chars[i..].iter().collect() };
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\\' if next.is_some_and(|n| n.is_ascii_punctuation()) => {
                escape(&next.unwrap_or_default().to_string(), out);
                i += 2;
            }
            '`' => {
                let end = chars[i + 1..].iter().position(|&c| c == '`');
                match end {
                    Some(end) => {
                        out.push_str("<code>");
                        escape(&chars[i + 1..i + 1 + end].iter().collect::<String>(), out);
                        out.push_str("</code>");
                        i += end + 2;
                    }
                    None => {
                        out.push('`');
                        i += 1;
                    }
                }
            }
            '*' | '_' if next == Some(c) => {
                out.push_str(if bold { "</b>" } else { "<b>" });
                bold = !bold;
                i += 2;
            }
            '*' | '_'
                if italic
                    || next.is_some_and(|n| !n.is_whitespace())
                        && (c == '*' || i == 0 || !chars[i - 1].is_alphanumeric()) =>
            {
                out.push_str(if italic { "</i>" } else { "<i>" });
                italic = !italic;
                i += 1;
            }
            '!' | '[' => {
                let image = c == '!';
                let open = if image { i + 1 } else { i };
                let link = (chars.get(open) == Some(&'['))
                    .then(|| {
                        let rest = rest(open + 1);
                        let close = rest.find("](")?;
                        let end = rest[close + 2..].find(')')?;
                        Some((
                            rest[..close].to_string(),
                            rest[close + 2..close + 2 + end].to_string(),
                            open + 1 + rest[..close + 2 + end + 1].chars().count(),
                        ))
                    })
                    .flatten();
                match link {
                    Some((label, _, end)) if image => {
                        escape(&label, out);
                        i = end;
                    }
                    Some((label, url, end)) => {
                        let url = url.split_whitespace().next().unwrap_or_default();
                        out.push_str("<a href=\"");
                        escape(url, out);
                        out.push_str("\">");
                        inline(&label, out);
                        out.push_str("</a>");
                        i = end;
                    }
                    None => {
                        escape(&c.to_string(), out);
                        i += 1;
                    }
                }
            }
            c => {
                escape(&c.to_string(), out);
                i += 1;
            }
        }
    }
    if italic {
        out.push_str("</i>");
    }
    if bold {
        out.push_str("</b>");
    }
}

/// What is open while lines are read.
#[derive(Debug, Default)]
struct State {
    paragraph: bool,
    /// The kind of list open (`ul` or `ol`) and whether an item is.
    list: Option<&'static str>,
    item: bool,
    quote: bool,
    code: bool,
}

impl State {
    fn close_paragraph(&mut self, out: &mut String) {
        if self.paragraph {
            out.push_str("</p>\n");
            self.paragraph = false;
        }
        if self.item {
            out.push_str("</li>\n");
            self.item = false;
        }
    }

    fn close_blocks(&mut self, out: &mut String) {
        self.close_paragraph(out);
        if let Some(list) = self.list.take() {
            out.push_str(&format!("</{list}>\n"));
        }
        if self.quote {
            out.push_str("</blockquote>\n");
            self.quote = false;
        }
    }
}

/// Turns one line into HTML.
fn line(raw: &str, state: &mut State, out: &mut String) {
    let line = raw.trim_end();
    let trimmed = line.trim_start();
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        if state.code {
            out.push_str("</pre>\n");
            state.code = false;
        } else {
            state.close_blocks(out);
            out.push_str("<pre>");
            state.code = true;
        }
        return;
    }
    if state.code {
        escape(line, out);
        out.push('\n');
        return;
    }
    if trimmed.is_empty() {
        state.close_paragraph(out);
        return;
    }
    // A block quote: the rest of the line read on.
    let (quoted, text) = match trimmed.strip_prefix('>') {
        Some(rest) => (true, rest.trim_start()),
        None => (false, trimmed),
    };
    if quoted != state.quote {
        state.close_blocks(out);
        if quoted {
            out.push_str("<blockquote>\n");
            state.quote = true;
        }
    }
    let heading = text.chars().take_while(|&c| c == '#').count();
    if (1..=6).contains(&heading) && text[heading..].starts_with(' ') {
        state.close_blocks(out);
        let title = text[heading..].trim().trim_end_matches('#').trim();
        out.push_str(&format!("<h{heading}>"));
        inline(title, out);
        out.push_str(&format!("</h{heading}>\n"));
        return;
    }
    let rule = text
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    if rule.len() >= 3
        && ["-", "*", "_"]
            .iter()
            .any(|r| rule.chars().all(|c| r.starts_with(c)))
    {
        state.close_blocks(out);
        out.push_str("<hr/>\n");
        return;
    }
    let bullet = ["- ", "* ", "+ "].iter().find_map(|b| text.strip_prefix(b));
    let number = text
        .split_once(". ")
        .filter(|(n, _)| !n.is_empty() && n.len() <= 9 && n.chars().all(|c| c.is_ascii_digit()))
        .map(|(_, rest)| rest);
    if let Some((kind, rest)) = bullet.map(|r| ("ul", r)).or(number.map(|r| ("ol", r))) {
        state.close_paragraph(out);
        if state.list != Some(kind) {
            if let Some(list) = state.list.take() {
                out.push_str(&format!("</{list}>\n"));
            }
            out.push_str(&format!("<{kind}>\n"));
            state.list = Some(kind);
        }
        out.push_str("<li>");
        inline(rest.trim(), out);
        state.item = true;
        return;
    }
    if state.item {
        // A list item going on.
        out.push(' ');
        inline(text, out);
        return;
    }
    if state.list.is_some() && !state.item {
        state.close_blocks(out);
        if quoted {
            out.push_str("<blockquote>\n");
            state.quote = true;
        }
    }
    if state.paragraph {
        // Two spaces at the end break the line.
        out.push_str(if raw.ends_with("  ") { "<br/>" } else { " " });
    } else {
        out.push_str("<p>");
        state.paragraph = true;
    }
    inline(text, out);
}

/// Writes the HTML for the Markdown file at `path` to `to`, a few lines
/// at a time.
fn convert(storage: &dyn Storage, path: &str, to: &str) -> Result<(), DocumentError> {
    storage.write(to, b"<html><body>\n")?;
    let mut state = State::default();
    let mut out = String::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 1024];
    let mut at = 0u64;
    let flush = |out: &mut String| -> Result<(), DocumentError> {
        storage.append(to, out.as_bytes())?;
        out.clear();
        Ok(())
    };
    loop {
        let n = storage.read_at(path, at, &mut chunk)?;
        at += n as u64;
        pending.extend_from_slice(&chunk[..n]);
        while let Some(end) = pending.iter().position(|&b| b == b'\n') {
            let text: Vec<u8> = pending.drain(..=end).collect();
            let text = String::from_utf8_lossy(&text);
            line(text.trim_end_matches(['\n', '\r']), &mut state, &mut out);
        }
        if n == 0 {
            if !pending.is_empty() {
                let text = String::from_utf8_lossy(&pending).to_string();
                line(&text, &mut state, &mut out);
            }
            break;
        }
        if out.len() > 2048 {
            flush(&mut out)?;
        }
    }
    if state.code {
        out.push_str("</pre>\n");
    }
    state.close_blocks(&mut out);
    out.push_str("</body></html>\n");
    flush(&mut out)
}

impl Document for MarkdownDocument {
    fn metadata(&self) -> &Metadata {
        &self.metadata
    }

    fn sections(&self) -> u32 {
        self.html.sections()
    }

    fn section_len(&self, section: u32) -> u32 {
        self.html.section_len(section)
    }

    fn block_at(&self, storage: &dyn Storage, at: Locator) -> Result<Option<Block>, DocumentError> {
        self.html.block_at(storage, at)
    }

    fn toc_len(&self, storage: &dyn Storage) -> u32 {
        self.html.toc_len(storage)
    }

    fn toc_entry(&self, storage: &dyn Storage, index: u32) -> Option<TocEntry> {
        self.html.toc_entry(storage, index)
    }

    fn links(&self, storage: &dyn Storage, section: u32, from: u32, to: u32) -> Vec<Link> {
        self.html.links(storage, section, from, to)
    }

    fn link_target(&self, storage: &dyn Storage, href: &str) -> Option<Locator> {
        self.html.link_target(storage, href)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;
    use quireink_sdk::document::BlockKind;

    fn html(md: &str) -> String {
        let mut state = State::default();
        let mut out = String::new();
        for l in md.lines() {
            line(l, &mut state, &mut out);
        }
        state.close_blocks(&mut out);
        out
    }

    #[test]
    fn blocks_and_inline_markup() {
        assert_eq!(html("# Title #"), "<h1>Title</h1>\n");
        assert_eq!(
            html("Some **bold** and *it* or _it_,\nwith `a<b` and [a link](https://x.org \"t\")."),
            "<p>Some <b>bold</b> and <i>it</i> or <i>it</i>, with <code>a&lt;b</code> and \
             <a href=\"https://x.org\">a link</a>.</p>\n"
        );
        assert_eq!(
            html("- one\n- two\n\n1. first\n2. second"),
            "<ul>\n<li>one</li>\n<li>two</li>\n</ul>\n<ol>\n<li>first</li>\n<li>second</li>\n</ol>\n"
        );
        assert_eq!(
            html("> quoted\n> on\n\nafter"),
            "<blockquote>\n<p>quoted on</p>\n</blockquote>\n<p>after</p>\n"
        );
        assert_eq!(html("```\nlet a = 1;\n```"), "<pre>let a = 1;\n</pre>\n");
        assert_eq!(html("---"), "<hr/>\n");
        assert_eq!(html("snake_case_name"), "<p>snake_case_name</p>\n");
    }

    #[test]
    fn reads_as_a_document() {
        let md = "# Notes\n\nA *first* paragraph.\n\n## Part\n\n- item\n";
        let storage = MemoryStorage::new().with("n/notes.md", md.as_bytes().to_vec());
        let doc = MarkdownDocument::open(&storage, "n/notes.md", true).unwrap();
        let first = doc.block_at(&storage, Locator::START).unwrap().unwrap();
        assert_eq!(
            (first.kind, first.text.as_str()),
            (BlockKind::Heading(1), "Notes")
        );
        let mut texts = Vec::new();
        let mut at = Locator::START;
        while let Some(block) = doc.block_at(&storage, at).unwrap() {
            texts.push(block.text.clone());
            at = Locator::new(0, block.end);
        }
        assert_eq!(texts, ["Notes", "A first paragraph.", "Part", "• item"]);
    }
}
