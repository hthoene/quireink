//! OPDS 1 catalogs: Atom feeds of books and of further feeds, read as
//! they stream from storage, so a feed of any size needs little memory.
//!
//! What is kept of a feed: its title, its entries (at most
//! [`MAX_ENTRIES`]) with their titles, authors and links, the next page
//! and where to search. Entries lead to another feed (navigation) or to
//! books (acquisition links), or both.

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::Storage;
use quireink_sdk::web::{encode, join_url};
use quireink_sdk::xml::{Token, Tokenizer, decode, local_name};

/// Entries kept of one page; catalogs page longer lists with *next*.
pub(crate) const MAX_ENTRIES: usize = 100;
/// Titles and notes are cut at this many characters.
const MAX_TEXT: usize = 300;
/// Feeds are read in pieces of this size.
const CHUNK: usize = 2048;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Feed {
    pub title: String,
    pub entries: Vec<Entry>,
    /// The next page of the list.
    pub next: Option<String>,
    pub search: Option<Search>,
}

/// Where a catalog is searched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Search {
    /// An OpenSearch description, which names the address to use.
    Description(String),
    /// An address with `{searchTerms}` in it.
    Template(String),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub id: String,
    pub title: String,
    /// Its authors, separated by ` & ` (names may hold commas:
    /// `Austen, Jane`).
    pub authors: String,
    /// A short plain text about it, if it has one.
    pub summary: String,
    /// A feed it leads to.
    pub feed: Option<String>,
    /// A small picture of it (a cover), if the catalog has one.
    pub thumbnail: Option<String>,
    pub books: Vec<Book>,
}

/// A file of a book to download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Book {
    /// Its content type, e.g. `application/epub+zip`.
    pub mime: String,
    pub url: String,
    /// How the catalog names this file, e.g. `EPUB (no images)`.
    pub title: String,
}

impl Book {
    /// The file extension for its content type, if it is a known one.
    pub(crate) fn extension(&self) -> Option<&'static str> {
        let mime = self.mime.split(';').next().unwrap_or("").trim();
        Some(match mime {
            "application/epub+zip" => "epub",
            "text/plain" => "txt",
            "application/pdf" => "pdf",
            "application/x-mobipocket-ebook" => "mobi",
            "application/vnd.amazon.ebook" => "azw3",
            "application/x-fictionbook+xml" | "application/fb2+xml" | "text/fb2+xml" => "fb2",
            "application/fb2+zip" | "application/x-zip-compressed-fb2" => "fb2.zip",
            "application/x-cbz" | "application/vnd.comicbook+zip" => "cbz",
            "application/rtf" | "text/rtf" => "rtf",
            "application/vnd.oasis.opendocument.text" => "odt",
            _ => return None,
        })
    }
}

impl Entry {
    /// What to show below its title: its authors, else its summary.
    pub(crate) fn note(&self) -> &str {
        if self.authors.is_empty() {
            &self.summary
        } else {
            &self.authors
        }
    }

    /// The first of its files whose format `readable` accepts (by its
    /// extension), with that extension.
    pub(crate) fn readable_book(
        &self,
        readable: impl Fn(&str) -> bool,
    ) -> Option<(&Book, &'static str)> {
        self.books
            .iter()
            .filter_map(|book| book.extension().map(|ext| (book, ext)))
            .find(|(_, ext)| readable(ext))
    }
}

/// The text being collected, and what it is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    FeedTitle,
    Title,
    Id,
    Author,
    Content,
}

/// The parser's state between tokens.
#[derive(Debug, Default)]
struct Reader {
    base: String,
    feed: Feed,
    entry: Option<Entry>,
    in_author: bool,
    field: Option<Field>,
    text: String,
    /// The plain content or summary of the entry.
    content: String,
    authors: Vec<String>,
}

impl Reader {
    fn token(&mut self, token: Token<'_>) {
        match token {
            Token::Start {
                name,
                attrs,
                self_closing,
            } => {
                let name = local_name(name);
                match name {
                    "entry" => {
                        self.entry = Some(Entry::default());
                        self.authors.clear();
                        self.content.clear();
                    }
                    "author" => self.in_author = true,
                    "link" => self.link(
                        attrs.get("rel").as_deref(),
                        attrs.get("type").as_deref().unwrap_or(""),
                        attrs.get("href").as_deref().unwrap_or(""),
                        attrs.get("title").as_deref().unwrap_or(""),
                    ),
                    _ => {}
                }
                if self_closing {
                    return;
                }
                let in_entry = self.entry.is_some();
                let field = match name {
                    "title" if in_entry => Some(Field::Title),
                    "title" if !self.in_author && self.feed.title.is_empty() => {
                        Some(Field::FeedTitle)
                    }
                    "id" if in_entry && !self.in_author => Some(Field::Id),
                    "name" if in_entry && self.in_author => Some(Field::Author),
                    "content" | "summary" if in_entry => {
                        let kind = attrs.get("type");
                        matches!(kind.as_deref(), None | Some("text")).then_some(Field::Content)
                    }
                    _ => None,
                };
                if field.is_some() {
                    self.field = field;
                    self.text.clear();
                }
            }
            Token::Text(text) => {
                if self.field.is_some() && self.text.chars().count() < MAX_TEXT {
                    self.text.push_str(&decode(&text));
                }
            }
            Token::CData(text) => {
                if self.field.is_some() && self.text.chars().count() < MAX_TEXT {
                    self.text.push_str(text);
                }
            }
            Token::End { name } => self.end(local_name(name)),
        }
    }

    fn end(&mut self, name: &str) {
        let ends_field = match self.field {
            Some(Field::FeedTitle | Field::Title) => name == "title",
            Some(Field::Id) => name == "id",
            Some(Field::Author) => name == "name",
            Some(Field::Content) => name == "content" || name == "summary",
            None => false,
        };
        if ends_field {
            let text = clean(&self.text);
            match (self.field.take(), self.entry.as_mut()) {
                (Some(Field::FeedTitle), _) => self.feed.title = text,
                (Some(Field::Title), Some(entry)) => entry.title = text,
                (Some(Field::Id), Some(entry)) => entry.id = text,
                (Some(Field::Author), Some(_)) if !text.is_empty() => self.authors.push(text),
                (Some(Field::Content), Some(_)) if self.content.is_empty() => self.content = text,
                _ => {}
            }
        }
        match name {
            "author" => self.in_author = false,
            "entry" => {
                if let Some(mut entry) = self.entry.take() {
                    entry.authors = self.authors.join(" & ");
                    entry.summary = core::mem::take(&mut self.content);
                    let useful = entry.feed.is_some() || !entry.books.is_empty();
                    if useful && self.feed.entries.len() < MAX_ENTRIES {
                        self.feed.entries.push(entry);
                    }
                }
            }
            _ => {}
        }
    }

    fn link(&mut self, rel: Option<&str>, kind: &str, href: &str, title: &str) {
        if href.is_empty() || href.starts_with("data:") {
            return;
        }
        let url = join_url(&self.base, href);
        let atom = kind.contains("atom+xml");
        let Some(entry) = self.entry.as_mut() else {
            match rel {
                Some("next") if atom || kind.is_empty() => self.feed.next = Some(url),
                Some("search") if kind.contains("opensearchdescription") => {
                    self.feed.search = Some(Search::Description(url));
                }
                Some("search") if href.contains("{searchTerms}") => {
                    // `{` and `}` survive joining as they are.
                    self.feed.search = Some(Search::Template(url));
                }
                _ => {}
            }
            return;
        };
        let rel = rel.unwrap_or("");
        if rel == "http://opds-spec.org/image/thumbnail" || rel == "http://opds-spec.org/thumbnail"
        {
            entry.thumbnail = Some(url);
            return;
        }
        if rel == "http://opds-spec.org/image" || rel == "http://opds-spec.org/cover" {
            // The full picture only where there is no small one.
            entry.thumbnail.get_or_insert(url);
            return;
        }
        if rel == "http://opds-spec.org/acquisition"
            || rel == "http://opds-spec.org/acquisition/open-access"
        {
            entry.books.push(Book {
                mime: kind.to_string(),
                url,
                title: clean(title),
            });
        } else if atom
            && entry.feed.is_none()
            && (matches!(rel, "" | "subsection" | "alternate" | "start")
                || rel.starts_with("http://opds-spec.org/sort/")
                || rel.starts_with("http://opds-spec.org/featured")
                || rel.starts_with("http://opds-spec.org/recommended"))
        {
            entry.feed = Some(url);
        }
    }
}

/// Whitespace runs as single spaces, at most [`MAX_TEXT`] characters.
fn clean(text: &str) -> String {
    let mut out = String::new();
    for word in text.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    match out.char_indices().nth(MAX_TEXT) {
        Some((cut, _)) => {
            out.truncate(cut);
            out.push('…');
            out
        }
        None => out,
    }
}

/// Reads the feed in file `path`, which came from `base` (links are
/// resolved against it).
pub(crate) fn read(storage: &dyn Storage, path: &str, base: &str) -> Feed {
    let mut reader = Reader {
        base: base.to_string(),
        ..Reader::default()
    };
    let mut tokenizer = Tokenizer::new();
    let mut buf = [0u8; CHUNK];
    let mut offset = 0;
    while let Ok(n) = storage.read_at(path, offset, &mut buf) {
        if n == 0 {
            break;
        }
        tokenizer.feed(&buf[..n], &mut |token| reader.token(token));
        offset += n as u64;
    }
    tokenizer.finish(&mut |token| reader.token(token));
    reader.feed
}

/// The address an OpenSearch description in file `path` (from `base`)
/// gives for searches in OPDS, with `{searchTerms}` in it.
pub(crate) fn search_template(storage: &dyn Storage, path: &str, base: &str) -> Option<String> {
    let data = quireink_sdk::read_file(storage, path, 64 * 1024).ok()?;
    let mut template = None;
    Tokenizer::parse(&data, &mut |token| {
        if let Token::Start { name, attrs, .. } = token
            && local_name(name) == "Url"
            && template.is_none()
            && attrs.get("type").is_some_and(|t| t.contains("atom+xml"))
            && let Some(url) = attrs.get("template")
        {
            template = Some(join_url(base, &url));
        }
    });
    template
}

/// The address of a search for `terms` with `template`: the terms in
/// place of `{searchTerms}`, optional parameters (`{startPage?}`) left
/// out.
pub(crate) fn search_url(template: &str, terms: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            break;
        };
        let name = &rest[start + 1..start + end];
        if name == "searchTerms" {
            out.push_str(&encode(terms));
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    const NAVIGATION: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom" xmlns:opensearch="http://a9.com/-/spec/opensearch/1.1/">
<id>http://www.gutenberg.org/ebooks.opds/</id>
<title>Project  Gutenberg</title>
<author><name>Project Gutenberg</name></author>
<link rel="search" type="application/opensearchdescription+xml" href="https://www.gutenberg.org/catalog/osd-books.xml"/>
<link rel="next" type="application/atom+xml;profile=opds-catalog" href="/ebooks/search.opds/?sort_order=downloads&amp;start_index=26"/>
<entry>
<id>https://www.gutenberg.org/ebooks/1342.opds</id>
<title>Pride and Prejudice</title>
<content type="text">Jane Austen</content>
<link type="application/atom+xml;profile=opds-catalog" rel="subsection" href="/ebooks/1342.opds"/>
<link type="image/png" rel="http://opds-spec.org/image/thumbnail" href="data:image/png;base64,iVBORw0KGgo"/>
</entry>
<entry><title>Nothing to open</title><id>x</id></entry>
</feed>"#;

    const BOOKS: &str = r#"<feed xmlns="http://www.w3.org/2005/Atom">
<title>Pride and Prejudice by Jane Austen</title>
<entry>
<title>Pride and Prejudice</title>
<content type="xhtml"><div xmlns="http://www.w3.org/1999/xhtml"><p>Long text</p></div></content>
<id>urn:gutenberg:1342:2</id>
<author><name>Austen, Jane</name></author>
<link type="application/epub+zip" rel="http://opds-spec.org/acquisition" title="EPUB (no images)" href="https://www.gutenberg.org/ebooks/1342.epub.noimages"/>
<link type="application/x-mobipocket-ebook" rel="http://opds-spec.org/acquisition" href="https://www.gutenberg.org/ebooks/1342.kindle.noimages"/>
<link type="application/atom+xml;profile=opds-catalog" rel="related" href="/ebooks/author/68.opds"/>
</entry>
<entry>
<title><![CDATA[Fish & Chips]]></title>
<id>urn:b</id>
<link type="application/x-mobipocket-ebook" rel="http://opds-spec.org/acquisition" href="b.mobi"/>
<link type="application/pdf" rel="http://opds-spec.org/acquisition/buy" href="b.pdf"/>
</entry>
</feed>"#;

    fn parse(text: &str, base: &str) -> Feed {
        let storage = MemoryStorage::new();
        storage.write("feed.xml", text.as_bytes()).unwrap();
        read(&storage, "feed.xml", base)
    }

    #[test]
    fn navigation_feeds() {
        let feed = parse(NAVIGATION, "https://www.gutenberg.org/ebooks.opds/");
        assert_eq!(feed.title, "Project Gutenberg");
        assert_eq!(
            feed.next.as_deref(),
            Some(
                "https://www.gutenberg.org/ebooks/search.opds/?sort_order=downloads&start_index=26"
            )
        );
        assert_eq!(
            feed.search,
            Some(Search::Description(String::from(
                "https://www.gutenberg.org/catalog/osd-books.xml"
            )))
        );
        // Entries leading nowhere are left out.
        assert_eq!(feed.entries.len(), 1);
        let entry = &feed.entries[0];
        assert_eq!(entry.title, "Pride and Prejudice");
        assert_eq!(entry.summary, "Jane Austen");
        assert_eq!(entry.authors, "");
        assert_eq!(
            entry.feed.as_deref(),
            Some("https://www.gutenberg.org/ebooks/1342.opds")
        );
        assert!(entry.books.is_empty());
    }

    #[test]
    fn acquisition_feeds() {
        let feed = parse(BOOKS, "https://www.gutenberg.org/ebooks/1342.opds");
        assert_eq!(feed.entries.len(), 2);
        let entry = &feed.entries[0];
        assert_eq!(entry.id, "urn:gutenberg:1342:2");
        assert_eq!(entry.authors, "Austen, Jane");
        assert_eq!(entry.summary, "", "only plain text");
        assert_eq!(entry.feed, None, "related feeds are not where it leads");
        assert_eq!(entry.books.len(), 2);
        let (book, ext) = entry.readable_book(|ext| ext == "epub").unwrap();
        assert_eq!(ext, "epub");
        assert_eq!(
            book.url,
            "https://www.gutenberg.org/ebooks/1342.epub.noimages"
        );
        assert_eq!(book.title, "EPUB (no images)");
        let other = &feed.entries[1];
        assert_eq!(other.title, "Fish & Chips");
        assert_eq!(other.books.len(), 1, "books to buy are left out");
        assert_eq!(
            other.books[0].url,
            "https://www.gutenberg.org/ebooks/b.mobi"
        );
        assert!(other.readable_book(|ext| ext == "epub").is_none());
    }

    #[test]
    fn searches() {
        let storage = MemoryStorage::new();
        let description = r#"<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/">
<Url type="text/html" template="http://www.gutenberg.org/ebooks/search/?query={searchTerms}"/>
<Url type="application/atom+xml" template="http://m.gutenberg.org/ebooks/search.opds/?query={searchTerms}"/>
</OpenSearchDescription>"#;
        storage.write("osd.xml", description.as_bytes()).unwrap();
        let template = search_template(&storage, "osd.xml", "https://www.gutenberg.org/").unwrap();
        assert_eq!(
            template,
            "http://m.gutenberg.org/ebooks/search.opds/?query={searchTerms}"
        );
        assert_eq!(
            search_url(&template, "Jane Austen"),
            "http://m.gutenberg.org/ebooks/search.opds/?query=Jane+Austen"
        );
        assert_eq!(
            search_url("https://a.org/s?q={searchTerms}&p={startPage?}", "x"),
            "https://a.org/s?q=x&p="
        );
    }

    #[test]
    fn long_lists_are_cut() {
        let mut text = String::from("<feed><title>Many</title>");
        for n in 0..MAX_ENTRIES + 5 {
            text.push_str(&alloc::format!(
                "<entry><title>{n}</title><link rel=\"subsection\" type=\"application/atom+xml\" href=\"/{n}\"/></entry>"
            ));
        }
        text.push_str("</feed>");
        let feed = parse(&text, "https://a.org/");
        assert_eq!(feed.entries.len(), MAX_ENTRIES);
        assert_eq!(feed.entries[3].feed.as_deref(), Some("https://a.org/3"));
    }
}
