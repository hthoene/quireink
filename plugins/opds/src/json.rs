//! OPDS 2 catalogs: JSON feeds (`application/opds+json`), read as they
//! stream from storage into the same [`Feed`] as OPDS 1, so a feed of any
//! size needs little memory.
//!
//! What is read: the title (`metadata.title`); `navigation` (entries that
//! lead to further feeds); `publications` (books: title, authors,
//! description, cover from `images`, files from acquisition `links`);
//! the same inside `groups`; and the feed's own `links`: `next` and
//! `search` (a URI template such as `…/search{?query}`).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use quireink_sdk::Storage;
use quireink_sdk::json::stream::{self, Event};
use quireink_sdk::web::join_url;

use crate::feed::{Book, Entry, Feed, MAX_ENTRIES, Search};

/// A link being read.
#[derive(Debug, Default)]
struct Link {
    rel: Vec<String>,
    href: String,
    kind: String,
    title: String,
    templated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Publication,
    Navigation,
}

/// Builds the feed from the parser's events.
#[derive(Debug, Default)]
struct Builder {
    base: String,
    feed: Feed,
    /// The entry being read, what it is, and how deep it starts.
    entry: Option<(Entry, Kind, usize)>,
    authors: Vec<String>,
    link: Option<Link>,
}

/// `path` without the leading `groups/[]` (entries of groups count as
/// the feed's own).
fn ungrouped(path: &[String]) -> &[String] {
    if path.len() >= 2 && path[0] == "groups" && path[1] == "[]" {
        &path[2..]
    } else {
        path
    }
}

fn is(path: &[String], parts: &[&str]) -> bool {
    path.len() == parts.len() && path.iter().zip(parts).all(|(a, b)| a == b)
}

impl Builder {
    fn event(&mut self, event: Event<'_>) {
        match event {
            Event::Open(path) => self.open(path),
            Event::Close(path) => self.close(path),
            Event::Value(path, value) => self.value(path, value),
        }
    }

    fn open(&mut self, path: &[String]) {
        if let Some((_, _, depth)) = &self.entry {
            let rel = &path[*depth..];
            if is(rel, &["links", "[]"]) {
                self.link = Some(Link::default());
            }
            return;
        }
        let local = ungrouped(path);
        if is(local, &["publications", "[]"]) || is(local, &["navigation", "[]"]) {
            let kind = if local[0] == "publications" {
                Kind::Publication
            } else {
                Kind::Navigation
            };
            self.entry = Some((Entry::default(), kind, path.len()));
            self.authors.clear();
        } else if is(path, &["links", "[]"]) {
            self.link = Some(Link::default());
        }
    }

    fn close(&mut self, path: &[String]) {
        let entry_depth = self.entry.as_ref().map(|(_, _, d)| *d);
        if let Some(depth) = entry_depth {
            let rel = &path[depth..];
            if is(rel, &["links", "[]"]) {
                if let (Some(link), Some((entry, kind, _))) = (self.link.take(), &mut self.entry) {
                    add_entry_link(&self.base, entry, *kind, link);
                }
                return;
            }
            if path.len() == depth {
                if let Some((mut entry, _, _)) = self.entry.take() {
                    entry.authors = self.authors.join(" & ");
                    if !entry.title.is_empty() && self.feed.entries.len() < MAX_ENTRIES {
                        self.feed.entries.push(entry);
                    }
                }
            }
            return;
        }
        if is(path, &["links", "[]"])
            && let Some(link) = self.link.take()
        {
            let href = self.template(&link);
            if link.rel.iter().any(|r| r == "next") && self.feed.next.is_none() {
                self.feed.next = Some(href);
            } else if link.rel.iter().any(|r| r == "search") && self.feed.search.is_none() {
                self.feed.search = Some(if link.templated {
                    Search::Template(href)
                } else {
                    Search::Description(href)
                });
            }
        }
    }

    /// A link's address, with an OPDS 2 search template (`{?query}`) in
    /// the form searches use (`{searchTerms}`).
    fn template(&self, link: &Link) -> String {
        let href = join_url(&self.base, &link.href);
        if !link.templated {
            return href;
        }
        href.replace("{?query}", "?query={searchTerms}")
            .replace("{&query}", "&query={searchTerms}")
            .replace("{query}", "{searchTerms}")
    }
}

/// A link of an entry: a file of a book, or the feed it leads to.
fn add_entry_link(base: &str, entry: &mut Entry, kind: Kind, link: Link) {
    let url = join_url(base, &link.href);
    let acquisition = link.rel.iter().any(|r| r.contains("acquisition"));
    let feed = link.kind.contains("opds") || link.kind.contains("atom");
    if kind == Kind::Publication && (acquisition || (!feed && link.rel.is_empty())) {
        entry.books.push(Book {
            mime: link.kind,
            url,
            title: link.title,
        });
    } else if feed && entry.feed.is_none() {
        entry.feed = Some(url);
    }
}

impl Builder {
    fn value(&mut self, path: &[String], value: &str) {
        if let Some((entry, kind, depth)) = &mut self.entry {
            let rel = &path[*depth..];
            if let Some(link) = &mut self.link
                && rel.len() >= 3
                && rel[0] == "links"
            {
                match rel[2].as_str() {
                    "rel" => link.rel.push(value.to_string()),
                    "href" => link.href = value.to_string(),
                    "type" => link.kind = value.to_string(),
                    "title" => link.title = value.to_string(),
                    _ => {}
                }
                return;
            }
            match *kind {
                Kind::Navigation => match rel {
                    [k] if k == "title" => entry.title = value.to_string(),
                    [k] if k == "href" => entry.feed = Some(join_url(&self.base, value)),
                    _ => {}
                },
                Kind::Publication => {
                    let rel: Vec<&str> = rel.iter().map(String::as_str).collect();
                    match rel.as_slice() {
                        ["metadata", "title"] => entry.title = value.to_string(),
                        ["metadata", "identifier"] => entry.id = value.to_string(),
                        ["metadata", "description"] => {
                            entry.summary = plain(value);
                        }
                        ["metadata", "author"]
                        | ["metadata", "author", "[]"]
                        | ["metadata", "author", "name"]
                        | ["metadata", "author", "[]", "name"] => {
                            self.authors.push(value.to_string());
                        }
                        ["images", "[]", "href"] if entry.thumbnail.is_none() => {
                            entry.thumbnail = Some(join_url(&self.base, value));
                        }
                        _ => {}
                    }
                }
            }
            return;
        }
        if let Some(link) = &mut self.link
            && path.len() >= 3
            && path[0] == "links"
        {
            match path[2].as_str() {
                "rel" => link.rel.push(value.to_string()),
                "href" => link.href = value.to_string(),
                "type" => link.kind = value.to_string(),
                "templated" => link.templated = value == "true",
                _ => {}
            }
            return;
        }
        if is(path, &["metadata", "title"]) {
            self.feed.title = value.to_string();
        }
    }
}

/// A description as plain text: tags dropped, entities decoded,
/// whitespace collapsed.
fn plain(html: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                text.push(' ');
            }
            c if !in_tag => text.push(c),
            _ => {}
        }
    }
    crate::feed::clean(&quireink_sdk::xml::decode(&text))
}

/// Whether the file at `path` is JSON (its first character `{`).
pub(crate) fn is_json(storage: &dyn Storage, path: &str) -> bool {
    let mut head = [0u8; 64];
    let n = storage.read_at(path, 0, &mut head).unwrap_or(0);
    head[..n]
        .iter()
        .find(|b| !b.is_ascii_whitespace() && **b != 0xef && **b != 0xbb && **b != 0xbf)
        == Some(&b'{')
}

/// Reads the OPDS 2 feed in file `path`, which came from `base`.
pub(crate) fn read(storage: &dyn Storage, path: &str, base: &str) -> Feed {
    let mut builder = Builder {
        base: base.to_string(),
        ..Builder::default()
    };
    stream::read_file(storage, path, &mut |event| builder.event(event));
    builder.feed
}

#[cfg(test)]
mod tests {
    use super::*;
    use quireink_sdk::MemoryStorage;

    const FEED: &str = r#"{
  "metadata": {"title": "Koméga 📚 library", "numberOfItems": 2},
  "links": [
    {"rel": "self", "href": "/opds/v2/catalog", "type": "application/opds+json"},
    {"rel": ["next"], "href": "?page=2", "type": "application/opds+json"},
    {"rel": "search", "href": "/opds/v2/search{?query}", "type": "application/opds+json", "templated": true}
  ],
  "navigation": [
    {"href": "/opds/v2/series", "title": "Series", "type": "application/opds+json", "rel": "subsection"}
  ],
  "publications": [
    {
      "metadata": {
        "title": "Alice's Adventures",
        "identifier": "urn:uuid:1",
        "author": [{"name": "Lewis Carroll"}, "Someone \"Else\""],
        "description": "<p>Down the <b>rabbit</b> hole.</p>"
      },
      "links": [
        {"rel": "http://opds-spec.org/acquisition", "href": "/books/1/file", "type": "application/epub+zip", "title": "EPUB"}
      ],
      "images": [{"href": "/books/1/thumb", "type": "image/jpeg"}]
    }
  ],
  "groups": [
    {"metadata": {"title": "New"}, "publications": [
      {"metadata": {"title": "Grouped", "author": "A. Writer"},
       "links": [{"rel": ["http://opds-spec.org/acquisition/open-access"], "href": "https://x.org/g.epub", "type": "application/epub+zip"}]}
    ]}
  ]
}"#;

    #[test]
    fn reads_opds_2() {
        let storage = MemoryStorage::new();
        storage.write("feed.json", FEED.as_bytes()).unwrap();
        assert!(is_json(&storage, "feed.json"));
        let feed = read(
            &storage,
            "feed.json",
            "https://komga.example/opds/v2/catalog",
        );
        assert_eq!(feed.title, "Koméga 📚 library");
        assert_eq!(
            feed.next.as_deref(),
            Some("https://komga.example/opds/v2/catalog?page=2")
        );
        assert_eq!(
            feed.search,
            Some(Search::Template(String::from(
                "https://komga.example/opds/v2/search?query={searchTerms}"
            )))
        );
        assert_eq!(feed.entries.len(), 3);
        let series = &feed.entries[0];
        assert_eq!(series.title, "Series");
        assert_eq!(
            series.feed.as_deref(),
            Some("https://komga.example/opds/v2/series")
        );
        let alice = &feed.entries[1];
        assert_eq!(alice.authors, "Lewis Carroll & Someone \"Else\"");
        assert_eq!(alice.summary, "Down the rabbit hole.");
        assert_eq!(alice.books[0].url, "https://komga.example/books/1/file");
        assert_eq!(alice.books[0].extension(), Some("epub"));
        assert_eq!(
            alice.thumbnail.as_deref(),
            Some("https://komga.example/books/1/thumb")
        );
        let grouped = &feed.entries[2];
        assert_eq!(grouped.authors, "A. Writer");
        assert_eq!(grouped.books[0].url, "https://x.org/g.epub");
    }

    #[test]
    fn atom_is_no_json() {
        let storage = MemoryStorage::new();
        storage
            .write("feed.xml", b"<?xml version=\"1.0\"?><feed/>")
            .unwrap();
        assert!(!is_json(&storage, "feed.xml"));
    }
}
