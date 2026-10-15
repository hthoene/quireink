//! A feed (RSS 2.0, RSS 1.0 or Atom) made into a web page to read: the
//! feed's title, then each article with its title, date and text as
//! plain paragraphs (pictures, links and layout are left out, so any
//! feed reads well and needs little memory). The feed is read as it
//! streams from storage and each article written as soon as it is
//! complete, so only one article is in memory at a time.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};
use quireink_sdk::{Storage, StorageError};

/// Pieces the feed is read in.
const CHUNK: usize = 2048;
/// Headlines kept of a feed.
const HEADLINES: usize = 5;
/// Titles, dates and links are cut at this many bytes.
const MAX_FIELD: usize = 400;

/// What came of a feed.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Converted {
    /// The feed's own title.
    pub title: String,
    pub articles: usize,
    /// The first articles' titles.
    pub headlines: Vec<String>,
}

/// One article being read.
#[derive(Debug, Default)]
struct Article {
    title: String,
    date: String,
    link: String,
    /// Its text as markup (escaped HTML decoded, or inline XHTML).
    content: String,
    /// A short summary, used where there is no content.
    summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Title,
    Date,
    Link,
    Content,
    Summary,
}

/// The feed `from` as a page in `to`: at most `max_articles` articles,
/// each with at most `max_text` bytes of text.
pub(crate) fn convert(
    storage: &dyn Storage,
    from: &str,
    to: &str,
    max_articles: usize,
    max_text: usize,
) -> Result<Converted, StorageError> {
    let mut reader = Reader {
        storage,
        to,
        max_articles,
        max_text,
        converted: Converted::default(),
        article: None,
        field: None,
        depth: 0,
        started: false,
        failed: None,
        channel_title: String::new(),
    };
    let mut tokenizer = Tokenizer::new();
    let mut buf = [0u8; CHUNK];
    let mut at = 0u64;
    loop {
        let n = storage.read_at(from, at, &mut buf)?;
        if n == 0 {
            break;
        }
        at += n as u64;
        tokenizer.feed(&buf[..n], &mut |token| reader.token(token));
        if reader.failed.is_some() || reader.converted.articles >= max_articles {
            break;
        }
    }
    tokenizer.finish(&mut |token| reader.token(token));
    if let Some(e) = reader.failed {
        return Err(e);
    }
    reader.begin()?;
    storage.append(to, b"</body>\n</html>\n")?;
    Ok(reader.converted)
}

struct Reader<'a> {
    storage: &'a dyn Storage,
    to: &'a str,
    max_articles: usize,
    max_text: usize,
    converted: Converted,
    article: Option<Article>,
    field: Option<Field>,
    /// Elements open inside the field being read.
    depth: usize,
    /// The page's beginning is written.
    started: bool,
    failed: Option<StorageError>,
    channel_title: String,
}

impl Reader<'_> {
    fn token(&mut self, token: Token<'_>) {
        if self.failed.is_some() || self.converted.articles >= self.max_articles {
            return;
        }
        match token {
            Token::Start {
                name,
                attrs,
                self_closing,
            } => {
                let local = local_name(name);
                if self.field == Some(Field::Content) || self.field == Some(Field::Summary) {
                    // Inline XHTML: kept as markup.
                    if !self_closing {
                        self.depth += 1;
                    }
                    let tag = if self_closing {
                        format!("<{local}/>")
                    } else {
                        format!("<{local}>")
                    };
                    self.push(&tag);
                    return;
                }
                if matches!(local, "item" | "entry") {
                    self.article = Some(Article::default());
                    return;
                }
                if self.article.is_none() {
                    if local == "title" && self.channel_title.is_empty() {
                        self.field = Some(Field::Title);
                    }
                    return;
                }
                let field = match (name, local) {
                    (_, "title") => Some(Field::Title),
                    ("pubDate" | "dc:date", _) | (_, "published" | "updated" | "date") => {
                        Some(Field::Date)
                    }
                    ("content:encoded", _) => Some(Field::Content),
                    (_, "content") => Some(Field::Content),
                    (_, "description" | "summary") => Some(Field::Summary),
                    (_, "link") => {
                        // Atom: the address is an attribute.
                        let rel = attrs.get("rel");
                        if let Some(href) = attrs.get("href")
                            && rel.as_deref().is_none_or(|r| r == "alternate")
                            && let Some(article) = &mut self.article
                            && article.link.is_empty()
                        {
                            article.link = cut(&href, MAX_FIELD);
                        }
                        (!self_closing).then_some(Field::Link)
                    }
                    _ => None,
                };
                if self_closing {
                    return;
                }
                // A date or the first title: not overwritten.
                self.field = field.filter(|f| match (f, &self.article) {
                    (Field::Date, Some(a)) => a.date.is_empty(),
                    (Field::Title, Some(a)) => a.title.is_empty(),
                    (Field::Content, Some(a)) => a.content.is_empty(),
                    (Field::Summary, Some(a)) => a.summary.is_empty(),
                    (Field::Link, Some(a)) => a.link.is_empty(),
                    _ => true,
                });
                self.depth = 0;
            }
            Token::End { name } => {
                let local = local_name(name);
                if self.depth > 0 {
                    self.depth -= 1;
                    self.push(&format!("</{local}>"));
                    return;
                }
                if self.article.is_some() && matches!(local, "item" | "entry") {
                    self.field = None;
                    if let Some(article) = self.article.take() {
                        self.write(article);
                    }
                    return;
                }
                self.field = None;
            }
            Token::Text(text) => {
                if self.field.is_some() {
                    // Escaped HTML is decoded once into markup; inside
                    // inline XHTML, text stays as it is.
                    if self.depth > 0 {
                        self.push(&text);
                    } else {
                        let decoded = decode(&text);
                        self.push(&decoded);
                    }
                }
            }
            Token::CData(text) => {
                if self.field.is_some() {
                    self.push(text);
                }
            }
        }
    }

    /// Adds to the field being read (markup, made text when written).
    fn push(&mut self, text: &str) {
        let Some(field) = self.field else {
            return;
        };
        let Some(article) = &mut self.article else {
            if field == Field::Title && self.channel_title.len() < MAX_FIELD {
                self.channel_title.push_str(text);
            }
            return;
        };
        let (target, max) = match field {
            Field::Title => (&mut article.title, MAX_FIELD),
            Field::Date => (&mut article.date, MAX_FIELD),
            Field::Link => (&mut article.link, MAX_FIELD),
            Field::Content => (&mut article.content, self.max_text * 2),
            Field::Summary => (&mut article.summary, self.max_text * 2),
        };
        if target.len() < max {
            let room = max - target.len();
            target.push_str(&cut(text, room));
        }
    }

    /// The page's beginning, once the feed's title is known.
    fn begin(&mut self) -> Result<(), StorageError> {
        if self.started {
            return Ok(());
        }
        self.started = true;
        let title = one_line(&text_of(&self.channel_title));
        self.converted.title = title.clone();
        let head = format!(
            "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\"/>\n<title>{0}</title>\n</head>\n<body>\n<h1>{0}</h1>\n",
            escape(&title)
        );
        self.storage.write(self.to, head.as_bytes())
    }

    fn write(&mut self, article: Article) {
        if let Err(e) = self.begin() {
            self.failed = Some(e);
            return;
        }
        let title = one_line(&text_of(&article.title));
        let markup = if article.content.trim().is_empty() {
            &article.summary
        } else {
            &article.content
        };
        let paragraphs = paragraphs(markup, self.max_text);
        if title.is_empty() && paragraphs.is_empty() {
            return;
        }
        let mut out = String::new();
        out.push_str(&format!("<h2>{}</h2>\n", escape(&title)));
        let date = one_line(&article.date);
        if !date.is_empty() {
            out.push_str(&format!("<p><i>{}</i></p>\n", escape(&date)));
        }
        for p in &paragraphs {
            out.push_str(&format!("<p>{}</p>\n", escape(p)));
        }
        if let Err(e) = self.storage.append(self.to, out.as_bytes()) {
            self.failed = Some(e);
            return;
        }
        if self.converted.headlines.len() < HEADLINES && !title.is_empty() {
            self.converted.headlines.push(title);
        }
        self.converted.articles += 1;
    }
}

/// `text` cut to at most `max` bytes, at a character boundary.
fn cut(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// Whitespace collapsed to single spaces.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of markup, without its tags (for titles).
fn text_of(markup: &str) -> String {
    paragraphs(markup, MAX_FIELD).join(" ")
}

/// Elements that start a new paragraph.
fn is_block(name: &str) -> bool {
    matches!(
        name,
        "p" | "div"
            | "br"
            | "li"
            | "h1"
            | "h2"
            | "h3"
            | "h4"
            | "h5"
            | "h6"
            | "blockquote"
            | "tr"
            | "section"
            | "article"
            | "ul"
            | "ol"
            | "pre"
            | "figure"
            | "figcaption"
            | "hr"
            | "table"
            | "dd"
            | "dt"
    )
}

/// The paragraphs of the text of `markup`, at most `max` bytes in all.
fn paragraphs(markup: &str, max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut total = 0;
    let mut skipping = 0usize;
    let flush = |current: &mut String, out: &mut Vec<String>| {
        let line = one_line(current);
        if !line.is_empty() {
            out.push(line);
        }
        current.clear();
    };
    let mut tokenizer = Tokenizer::html();
    let mut emit = |token: Token<'_>| match token {
        Token::Start {
            name, self_closing, ..
        } => {
            let name = local_name(name).to_ascii_lowercase();
            if matches!(name.as_str(), "script" | "style") && !self_closing {
                skipping += 1;
            } else if is_block(&name) {
                flush(&mut current, &mut out);
            }
        }
        Token::End { name } => {
            let name = local_name(name).to_ascii_lowercase();
            if matches!(name.as_str(), "script" | "style") {
                skipping = skipping.saturating_sub(1);
            } else if is_block(&name) {
                flush(&mut current, &mut out);
            }
        }
        Token::Text(text) if skipping == 0 && total < max => {
            let text = decode(&text);
            total += text.len();
            current.push_str(&text);
        }
        Token::CData(text) if skipping == 0 && total < max => {
            total += text.len();
            current.push_str(text);
        }
        _ => {}
    };
    tokenizer.feed(markup.as_bytes(), &mut emit);
    tokenizer.finish(&mut emit);
    flush(&mut current, &mut out);
    out
}

/// `text` safe in HTML.
fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    fn page(feed: &str) -> (Converted, String) {
        let storage = MemoryStorage::new();
        storage.write("feed.xml", feed.as_bytes()).unwrap();
        let converted = convert(&storage, "feed.xml", "out.html", 10, 10_000).unwrap();
        let data = quireink_sdk::read_file(&storage, "out.html", 1 << 20).unwrap();
        (converted, String::from_utf8(data).unwrap())
    }

    #[test]
    fn rss_with_escaped_html_and_cdata() {
        let (converted, html) = page(
            r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:content="http://purl.org/rss/1.0/modules/content/">
<channel><title>Rabbit &amp; Co</title><link>https://example.org/</link>
<item><title>First &lt;b&gt;news&lt;/b&gt;</title><pubDate>Tue, 13 Oct 2026 10:00:00 GMT</pubDate>
<description>&lt;p&gt;Down the &lt;a href="x"&gt;hole&lt;/a&gt;.&lt;/p&gt;&lt;p&gt;Second one.&lt;/p&gt;</description></item>
<item><title>Second</title><description>Short</description>
<content:encoded><![CDATA[<p>The full <em>text</em>.</p><script>alert(1)</script><p>More &amp; more.</p>]]></content:encoded></item>
</channel></rss>"#,
        );
        assert_eq!(converted.title, "Rabbit & Co");
        assert_eq!(converted.articles, 2);
        assert!(html.contains("<title>Rabbit &amp; Co</title>"), "{html}");
        assert!(html.contains("<h2>First news</h2>"), "{html}");
        assert!(
            html.contains("<p><i>Tue, 13 Oct 2026 10:00:00 GMT</i></p>"),
            "{html}"
        );
        assert!(
            html.contains("<p>Down the hole.</p>\n<p>Second one.</p>"),
            "{html}"
        );
        // The full text over the summary; scripts left out.
        assert!(
            html.contains("<p>The full text.</p>\n<p>More &amp; more.</p>"),
            "{html}"
        );
        assert!(!html.contains("alert"), "{html}");
        assert!(!html.contains("Short"), "{html}");
        assert!(html.ends_with("</body>\n</html>\n"));
    }

    #[test]
    fn atom_with_inline_xhtml() {
        let (converted, html) = page(
            r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>Atom news</title>
<entry><title type="html">One</title><updated>2026-10-14T08:00:00Z</updated>
<link rel="alternate" href="https://example.org/1"/>
<content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><p>Inline &amp; fine.</p><p>Two</p></div></content></entry>
<entry><title>Two</title><summary>Only a summary</summary></entry></feed>"#,
        );
        assert_eq!(
            (converted.title.as_str(), converted.articles),
            ("Atom news", 2)
        );
        assert!(
            html.contains("<h2>One</h2>\n<p><i>2026-10-14T08:00:00Z</i></p>"),
            "{html}"
        );
        assert!(
            html.contains("<p>Inline &amp; fine.</p>\n<p>Two</p>"),
            "{html}"
        );
        assert!(
            html.contains("<h2>Two</h2>\n<p>Only a summary</p>"),
            "{html}"
        );
    }

    #[test]
    fn articles_and_text_are_limited() {
        let items: String = (0..30)
            .map(|n| {
                format!(
                    "<item><title>N{n}</title><description>{}</description></item>",
                    "word ".repeat(500)
                )
            })
            .collect();
        let storage = MemoryStorage::new();
        let feed = format!("<rss><channel><title>Many</title>{items}</channel></rss>");
        storage.write("feed.xml", feed.as_bytes()).unwrap();
        let converted = convert(&storage, "feed.xml", "out.html", 5, 100).unwrap();
        assert_eq!(converted.articles, 5);
        let size = storage.size("out.html").unwrap();
        assert!(size < 2000, "{size}");
    }
}
