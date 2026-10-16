//! EPUB 2 and 3 support for QuireInk.
//!
//! - Metadata (title, authors, language) from the package document.
//! - Reading order from the spine; table of contents from the EPUB 3
//!   navigation document or the EPUB 2 NCX.
//! - Chapters are converted on first use, streamed from the ZIP archive
//!   through an XHTML tokenizer, into a compact block format stored in the
//!   plugin's cache. Reading afterwards needs only the block at hand, so
//!   books of any size work on small devices.
//!
//! Supported markup: paragraphs, headings, lists, block quotes, line
//! breaks, preformatted text, anchors and links, styles and images.
//!
//! HTML files (`.html`, `.htm`, `.xhtml`), e.g. saved web pages, open
//! the same way, showing only a page's reading matter (see
//! [`HtmlDocument`]), Markdown (`.md`, see [`MarkdownDocument`]),
//! FictionBooks (`.fb2`, see [`Fb2Document`]) and comic book archives
//! (`.cbz`, see [`CbzDocument`]).

#![no_std]

extern crate alloc;

mod cached;
mod cbz;
mod convert;
mod css;
mod document;
mod fb2;
mod html;
mod markdown;
mod obfuscation;
mod package;
mod woff;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec;

use quireink_sdk::document::Document;
use quireink_sdk::{
    FormatSpec, Id, Manifest, Plugin, RegisterError, Registrar, SDK_VERSION, Version,
};

pub use cbz::CbzDocument;
pub use document::EpubDocument;
pub use fb2::Fb2Document;
pub use html::HtmlDocument;
pub use markdown::MarkdownDocument;

pub const PLUGIN: Id = Id::new_static("format-epub");
pub const FORMAT: Id = Id::new_static("format-epub.epub");
/// HTML files, e.g. web pages.
pub const HTML: Id = Id::new_static("format-epub.html");
/// FictionBook 2.
pub const FB2: Id = Id::new_static("format-epub.fb2");
/// Comic book archives.
pub const CBZ: Id = Id::new_static("format-epub.cbz");
/// Markdown.
pub const MARKDOWN: Id = Id::new_static("format-epub.markdown");

/// The EPUB format plugin.
#[derive(Debug, Default)]
pub struct EpubFormat;

impl Plugin for EpubFormat {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("EPUB"),
            description: Cow::Borrowed(
                "Opens EPUB books with their styles, table of contents and images, and web pages \
                 (HTML).",
            ),
            version: Version::new(0, 1, 0),
            sdk: SDK_VERSION,
        }
    }

    fn register(&self, r: &mut dyn Registrar) -> Result<(), RegisterError> {
        r.add_format(FormatSpec {
            id: FORMAT,
            title: Cow::Borrowed("format-epub.title"),
            extensions: vec![Cow::Borrowed("epub")],
            open: Box::new(|storage, path, options| {
                EpubDocument::open_options(storage, path, options)
                    .map(|d| Box::new(d) as Box<dyn Document>)
            }),
        })?;
        r.add_format(FormatSpec {
            id: HTML,
            title: Cow::Borrowed("format-epub.html"),
            extensions: vec![
                Cow::Borrowed("html"),
                Cow::Borrowed("htm"),
                Cow::Borrowed("xhtml"),
            ],
            open: Box::new(|storage, path, options| {
                HtmlDocument::open(storage, path, options.publisher_styles)
                    .map(|d| Box::new(d) as Box<dyn Document>)
            }),
        })?;
        r.add_format(FormatSpec {
            id: FB2,
            title: Cow::Borrowed("format-epub.fb2"),
            extensions: vec![Cow::Borrowed("fb2"), Cow::Borrowed("fb2.zip")],
            open: Box::new(|storage, path, options| {
                Fb2Document::open(storage, path, options.publisher_styles)
                    .map(|d| Box::new(d) as Box<dyn Document>)
            }),
        })?;
        r.add_format(FormatSpec {
            id: MARKDOWN,
            title: Cow::Borrowed("format-epub.markdown"),
            extensions: vec![Cow::Borrowed("md"), Cow::Borrowed("markdown")],
            open: Box::new(|storage, path, options| {
                MarkdownDocument::open(storage, path, options.publisher_styles)
                    .map(|d| Box::new(d) as Box<dyn Document>)
            }),
        })?;
        r.add_format(FormatSpec {
            id: CBZ,
            title: Cow::Borrowed("format-epub.cbz"),
            extensions: vec![Cow::Borrowed("cbz")],
            open: Box::new(|storage, path, _| {
                CbzDocument::open(storage, path).map(|d| Box::new(d) as Box<dyn Document>)
            }),
        })?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/en.ini")))?;
        r.add_translations(Cow::Borrowed(include_str!("../i18n/de.ini")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::String;
    use alloc::vec::Vec;
    use quireink_i18n::{Catalog, check};
    use quireink_sdk::MemoryStorage;
    use quireink_sdk::document::{Align, BlockKind, Locator, TextStyle};
    use quireink_sdk::zip::testing::ZipBuilder;

    #[test]
    fn translations_are_complete() {
        let en = Catalog::parse(include_str!("../i18n/en.ini")).unwrap();
        let de = Catalog::parse(include_str!("../i18n/de.ini")).unwrap();
        let issues = check::compare(&en, &de);
        assert!(issues.is_empty(), "{issues:?}");
    }

    const CONTAINER: &str = r#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"#;

    const OPF: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Die   Verwandlung</dc:title>
    <dc:creator>Franz Kafka</dc:creator>
    <dc:language>de</dc:language>
    <dc:subject>Novella</dc:subject>
    <dc:subject>Classics</dc:subject>
    <dc:description>&lt;p&gt;Gregor wakes up &amp;amp; finds&lt;/p&gt;&lt;p&gt;himself changed.&lt;/p&gt;</dc:description>
  </metadata>
  <manifest>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="c1" href="text/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="text/ch%202.xhtml" media-type="application/xhtml+xml"/>
    <item id="css" href="style.css" media-type="text/css"/>
  </manifest>
  <spine><itemref idref="c1"/><itemref idref="c2"/></spine>
</package>"#;

    const NAV: &str = r#"<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">
<body><nav epub:type="toc"><ol>
  <li><a href="text/ch1.xhtml">Erstes Kapitel</a>
    <ol><li><a href="text/ch1.xhtml#later">Später</a></li></ol></li>
  <li><a href="text/ch%202.xhtml">Zweites   Kapitel</a></li>
</ol></nav>
<nav epub:type="page-list"><ol><li><a href="text/ch1.xhtml#later">7</a></li></ol></nav>
</body></html>"#;

    const CH1: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Ignored</title>
<style>p { color: red }</style></head>
<body>
  <h1>I.</h1>
  <p>Als Gregor Samsa eines Morgens aus <em>unruhigen</em>
     Tr&#228;umen erwachte &#8212; fand er sich&nbsp;verwandelt.</p>
  <p>Zeile eins<br/>Zeile zwei</p>
  <ul><li>Punkt A</li><li>Punkt B</li></ul>
  <p id="later">Später.</p>
</body></html>"#;

    fn epub(ch2: String) -> MemoryStorage {
        let zip = ZipBuilder::new()
            .stored("mimetype", "application/epub+zip")
            .deflated("META-INF/container.xml", CONTAINER)
            .deflated("OEBPS/content.opf", OPF)
            .deflated("OEBPS/nav.xhtml", NAV)
            .deflated("OEBPS/text/ch1.xhtml", CH1)
            .deflated("OEBPS/text/ch 2.xhtml", ch2)
            .stored(
                "OEBPS/images/dot.png",
                &include_bytes!("../../../crates/quireink-image/tests/data/gray.png")[..],
            )
            .deflated(
                "OEBPS/style.css",
                "/* book styles */ .c { text-align: center } .hide { display: none }",
            )
            .build();
        MemoryStorage::new().with("books/test.epub", zip)
    }

    #[test]
    fn positions_as_koreader_writes_them() {
        let ch2 = String::from(
            "<html><body><div><p>Eins</p><p>Zwei <b>fett</b></p></div>\
             <div><h2>Titel</h2><p>Drei</p></div></body></html>",
        );
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let blocks: Vec<u32> = {
            let mut at = Locator::new(1, 0);
            let mut starts = Vec::new();
            while let Some(block) = doc.block_at(&storage, at).unwrap() {
                starts.push(block.start);
                at = Locator::new(1, block.end);
            }
            starts
        };
        let xp = |offset| doc.xpointer(&storage, Locator::new(1, offset)).unwrap();
        assert_eq!(xp(blocks[0]), "/body/DocFragment[2]/body/div[1]/p[1]");
        assert_eq!(xp(blocks[1] + 2), "/body/DocFragment[2]/body/div[1]/p[2]");
        assert_eq!(xp(blocks[3]), "/body/DocFragment[2]/body/div[2]/p[1]");
        let at = |x: &str| {
            doc.locate_xpointer(&storage, x)
                .map(|l| (l.section, l.offset))
        };
        // KOReader leaves out [1] and points into text.
        assert_eq!(
            at("/body/DocFragment[2]/body/div/p[2]/text().3"),
            Some((1, blocks[1]))
        );
        assert_eq!(at("/body/DocFragment[2]/body/div[2]"), Some((1, blocks[2])));
        // Deeper than blocks: the block containing it.
        assert_eq!(
            at("/body/DocFragment[2]/body/div[1]/p[2]/b/text().1"),
            Some((1, blocks[1]))
        );
        assert_eq!(at("/body/DocFragment[9]/body/p"), None);
        assert_eq!(at("nonsense"), None);
    }

    #[test]
    fn embedded_fonts_are_listed_used_and_revealed() {
        let opf = OPF
            .replace(
                "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\">",
                "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"uid\">",
            )
            .replace(
                "<dc:language>de</dc:language>",
                "<dc:language>de</dc:language><dc:identifier id=\"uid\">urn:isbn:123</dc:identifier>",
            );
        let font: Vec<u8> = (0..1500).map(|i| (i % 253) as u8).collect();
        let mut hidden = font.clone();
        obfuscation::reveal(&mut hidden, obfuscation::Method::Idpf, "urn:isbn:123");
        let encryption = r#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container"
            xmlns:enc="http://www.w3.org/2001/04/xmlenc#"><enc:EncryptedData>
            <enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/>
            <enc:CipherData><enc:CipherReference URI="OEBPS/fonts/Fancy.ttf"/></enc:CipherData>
            </enc:EncryptedData></encryption>"#;
        let zip = ZipBuilder::new()
            .stored("mimetype", "application/epub+zip")
            .deflated("META-INF/container.xml", CONTAINER)
            .deflated("META-INF/encryption.xml", encryption)
            .deflated("OEBPS/content.opf", opf.as_str())
            .deflated("OEBPS/nav.xhtml", NAV)
            .deflated("OEBPS/text/ch1.xhtml", CH1)
            .deflated(
                "OEBPS/text/ch 2.xhtml",
                "<html><body><p class=\"fancy\">Schmuck <b>fett</b></p><p>Normal</p></body></html>",
            )
            .stored("OEBPS/fonts/Fancy.ttf", &hidden[..])
            .deflated(
                "OEBPS/style.css",
                "@font-face { font-family: Fancy; src: url(fonts/Fancy.ttf) } .fancy { font-family: Fancy }",
            )
            .build();
        let storage = MemoryStorage::new().with("books/test.epub", zip);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let fonts = doc.fonts(&storage);
        assert_eq!(fonts.len(), 1);
        assert_eq!(
            (fonts[0].family, fonts[0].bold, fonts[0].italic),
            (1, false, false)
        );
        assert_eq!(doc.font_data(&storage, 0).unwrap(), font);
        let fancy = doc.block_at(&storage, Locator::new(1, 0)).unwrap().unwrap();
        assert_eq!(fancy.style_at(0).family(), 1);
        // Bold inside keeps the family.
        let bold = fancy.text.find("fett").unwrap();
        assert_eq!(fancy.style_at(bold), TextStyle::BOLD.with_family(1));
        let plain = doc
            .block_at(&storage, Locator::new(1, fancy.end))
            .unwrap()
            .unwrap();
        assert_eq!(plain.style_at(0).family(), 0);
        // Without the book's styles there are no embedded fonts either.
        let plain_doc = EpubDocument::open_with(&storage, "books/test.epub", false).unwrap();
        assert!(plain_doc.fonts(&storage).is_empty());
    }

    #[test]
    fn woff_fonts_come_out_as_their_font() {
        let woff = include_bytes!("../tests/data/atkinson-subset.woff");
        let zip = ZipBuilder::new()
            .stored("mimetype", "application/epub+zip")
            .deflated("META-INF/container.xml", CONTAINER)
            .deflated("OEBPS/content.opf", OPF)
            .deflated("OEBPS/nav.xhtml", NAV)
            .deflated("OEBPS/text/ch1.xhtml", CH1)
            .deflated(
                "OEBPS/text/ch 2.xhtml",
                "<html><body><p class=\"web\">Web</p></body></html>",
            )
            .stored("OEBPS/fonts/Web.woff", &woff[..])
            .deflated(
                "OEBPS/style.css",
                "@font-face { font-family: Web; src: url(fonts/Web.woff) format('woff') } \
                 .web { font-family: Web }",
            )
            .build();
        let storage = MemoryStorage::new().with("books/web.epub", zip);
        let doc = EpubDocument::open(&storage, "books/web.epub").unwrap();
        assert_eq!(doc.fonts(&storage).len(), 1);
        let font = doc.font_data(&storage, 0).unwrap();
        // A TrueType font again, as the reader's fonts read it.
        assert_eq!(&font[..4], &[0, 1, 0, 0]);
        let ttf = include_bytes!("../tests/data/atkinson-subset.ttf");
        assert!(font.len() >= ttf.len() - 64 && font.len() <= ttf.len() + 64);
    }

    fn blocks(
        storage: &MemoryStorage,
        doc: &EpubDocument,
        section: u32,
    ) -> Vec<(BlockKind, String)> {
        let mut at = Locator::new(section, 0);
        let mut out = Vec::new();
        while let Some(block) = doc.block_at(storage, at).unwrap() {
            assert!(block.end > at.offset || at.offset < block.start);
            at = Locator::new(section, block.end);
            out.push((block.kind, block.text));
        }
        out
    }

    #[test]
    fn links_lead_within_and_out_of_the_book() {
        let ch2 = String::from(
            "<html><body><p>Siehe <a href=\"ch1.xhtml#later\">später<sup>1</sup></a> \
             und <a href=\"#end\">unten</a>.</p>\
             <p>Mehr <a href=\"https://example.org/x?a=1&amp;b=2\">im Netz</a>.</p>\
             <p id=\"end\">Ende <a>ohne Ziel</a>.</p></body></html>",
        );
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let links = doc.links(&storage, 1, 0, u32::MAX);
        let hrefs: Vec<&str> = links.iter().map(|l| l.href.as_str()).collect();
        assert_eq!(
            hrefs,
            [
                "OEBPS/text/ch1.xhtml#later",
                "OEBPS/text/ch 2.xhtml#end",
                "https://example.org/x?a=1&b=2"
            ]
        );
        // The ranges cover the link texts.
        let first = doc.block_at(&storage, Locator::new(1, 0)).unwrap().unwrap();
        let text_of = |link: &quireink_sdk::document::Link| {
            let from = first.text_index(link.start);
            let to = first.text_index(link.end);
            String::from(&first.text[from..to])
        };
        assert_eq!(text_of(&links[0]), "später1");
        assert_eq!(text_of(&links[1]), "unten");
        // Only those overlapping a range.
        let tail = doc.links(&storage, 1, links[1].start, u32::MAX);
        assert_eq!(tail.len(), 2);
        // Into another chapter, at the anchor.
        let later = doc.link_target(&storage, &links[0].href).unwrap();
        assert_eq!(later.section, 0);
        let block = doc.block_at(&storage, later).unwrap().unwrap();
        assert_eq!(block.text, "Später.");
        let end = doc.link_target(&storage, &links[1].href).unwrap();
        let block = doc.block_at(&storage, end).unwrap().unwrap();
        assert_eq!(block.text, "Ende ohne Ziel.");
        assert_eq!(doc.link_target(&storage, &links[2].href), None);
    }

    #[test]
    fn reads_metadata_chapters_and_toc() {
        let ch2 = String::from("<html><body><p>Zweites.</p></body></html>");
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let metadata = &doc.metadata().clone();
        assert_eq!(metadata.title.as_deref(), Some("Die Verwandlung"));
        assert_eq!(metadata.authors, ["Franz Kafka"]);
        assert_eq!(metadata.language.as_deref(), Some("de"));
        assert_eq!(metadata.subjects, ["Novella", "Classics"]);
        // The printed edition's pages: from the anchor on, also in the
        // chapters after it.
        let later = doc.toc_target(&storage, 1).unwrap();
        assert_eq!(doc.page_label(&storage, Locator::new(0, 0)), None);
        assert_eq!(doc.page_label(&storage, later).as_deref(), Some("7"));
        assert_eq!(
            doc.page_label(&storage, Locator::new(1, 0)).as_deref(),
            Some("7")
        );
        assert_eq!(doc.sections(), 2);
        assert_eq!(
            metadata.description.as_deref(),
            Some("Gregor wakes up & finds himself changed.")
        );
        // Also when read from the cache.
        let again = EpubDocument::open(&storage, "books/test.epub").unwrap();
        assert_eq!(again.metadata().subjects, ["Novella", "Classics"]);
        assert_eq!(again.metadata().description, metadata.description);

        let ch1 = blocks(&storage, &doc, 0);
        let texts: Vec<&str> = ch1.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            texts,
            [
                "I.",
                "Als Gregor Samsa eines Morgens aus unruhigen Träumen erwachte — fand er sich\u{a0}verwandelt.",
                "Zeile eins\nZeile zwei",
                "• Punkt A",
                "• Punkt B",
                "Später."
            ]
        );
        assert_eq!(ch1[0].0, BlockKind::Heading(1));
        assert_eq!(blocks(&storage, &doc, 1)[0].1, "Zweites.");

        assert_toc(&storage, &doc);

        // A second open reads the package from the cache.
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        assert_eq!(doc.metadata(), metadata);
        assert_toc(&storage, &doc);
    }

    fn assert_toc(storage: &MemoryStorage, doc: &EpubDocument) {
        let titles: Vec<(String, u8)> = (0..doc.toc_len(storage))
            .map(|i| doc.toc_entry(storage, i).unwrap())
            .map(|e| (e.title, e.level))
            .collect();
        assert_eq!(
            titles,
            [
                (String::from("Erstes Kapitel"), 0),
                (String::from("Später"), 1),
                (String::from("Zweites Kapitel"), 0)
            ]
        );
        assert_eq!(doc.toc_target(storage, 2), Some(Locator::new(1, 0)));
        // The anchor points at the block with id="later".
        let later = doc.toc_target(storage, 1).unwrap();
        assert_eq!(later.section, 0);
        let block = doc.block_at(storage, later).unwrap().unwrap();
        assert_eq!(block.text, "Später.");
        assert!(doc.progress(Locator::new(1, 0)) > 0.5);
    }

    #[test]
    fn keeps_styles_and_alignment() {
        let ch2 = String::from(
            "<html><body><center><p>Mitte <b>fett <i>beides</i></b>              <span style=\"font-style: italic\">kursiv</span></p></center>             <p>Normal <em>betont <span style=\"font-style:normal\">aufrecht</span></em></p>             <p align=\"right\">Rechts</p></body></html>",
        );
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let block = doc.block_at(&storage, Locator::new(1, 0)).unwrap().unwrap();
        assert_eq!(block.text, "Mitte fett beides kursiv");
        assert_eq!(block.align, Align::Center);
        let styled: Vec<(&str, TextStyle)> = block
            .runs
            .iter()
            .map(|r| (&block.text[r.start as usize..r.end as usize], r.style))
            .collect();
        assert_eq!(
            styled,
            [
                ("fett ", TextStyle::BOLD),
                ("beides", TextStyle::BOLD_ITALIC),
                ("kursiv", TextStyle::ITALIC)
            ]
        );
        let second = doc
            .block_at(&storage, Locator::new(1, block.end))
            .unwrap()
            .unwrap();
        assert_eq!(second.align, Align::Auto);
        assert_eq!(second.style_at(7), TextStyle::ITALIC);
        assert_eq!(second.style_at(second.text.len() - 1), TextStyle::REGULAR);
        let third = doc
            .block_at(&storage, Locator::new(1, second.end))
            .unwrap()
            .unwrap();
        assert_eq!((third.text.as_str(), third.align), ("Rechts", Align::End));
    }

    #[test]
    fn decodes_images_into_the_cache() {
        let ch2 = String::from(
            "<html><body><p>Vorher</p><svg><image xlink:href=\"../images/dot.png\"/></svg>             <img src=\"../images/missing.png\" alt=\"Ein  Bild\"/><p>Nachher</p></body></html>",
        );
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let mut at = Locator::new(1, 0);
        let mut found = Vec::new();
        while let Some(block) = doc.block_at(&storage, at).unwrap() {
            at = Locator::new(1, block.end);
            let size = block.image.as_ref().map(|i| (i.width, i.height));
            found.push((block.kind, block.text.clone(), size));
        }
        assert_eq!(
            found,
            [
                (BlockKind::Paragraph, String::from("Vorher"), None),
                (BlockKind::Image, String::new(), Some((37, 23))),
                // A missing image becomes its alternative text.
                (BlockKind::Paragraph, String::from("Ein Bild"), None),
                (BlockKind::Paragraph, String::from("Nachher"), None),
            ]
        );
    }

    #[test]
    fn applies_stylesheets() {
        let ch2 = String::from(
            "<html><head><style>p.b { font-weight: bold }</style></head><body>             <p class=\"c\">Mitte</p><p class=\"x hide\">weg <b>auch weg</b></p>             <p class=\"b\">Fett</p><p>Normal</p></body></html>",
        );
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let mut at = Locator::new(1, 0);
        let mut found = Vec::new();
        while let Some(block) = doc.block_at(&storage, at).unwrap() {
            at = Locator::new(1, block.end);
            found.push((block.text.clone(), block.align, block.style_at(0)));
        }
        assert_eq!(
            found,
            [
                (String::from("Mitte"), Align::Center, TextStyle::REGULAR),
                (String::from("Fett"), Align::Auto, TextStyle::BOLD),
                (String::from("Normal"), Align::Auto, TextStyle::REGULAR),
            ]
        );
    }

    #[test]
    fn keeps_the_books_indents_and_spaces() {
        let ch2 = String::from(
            "<html><head><style>.head { padding-bottom: 1em }              .first { text-indent: 0; margin-top: 1.2em } p { margin: 0 }              div.box { text-indent: 2em; margin-bottom: 3em }</style></head><body>             <p class=\"head\">Title</p><p class=\"first\">One</p><p>Two</p>             <div class=\"box\"><p>Three</p></div><p>Four</p></body></html>",
        );
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let mut at = Locator::new(1, 0);
        let mut found = Vec::new();
        while let Some(block) = doc.block_at(&storage, at).unwrap() {
            at = Locator::new(1, block.end);
            found.push((block.text.clone(), block.indent, block.space));
        }
        let row = |t: &str, i, s| (String::from(t), i, s);
        assert_eq!(
            found,
            [
                row("Title", None, Some(0)),
                // The heading's padding plus the larger of the margins.
                row("One", Some(0), Some(220)),
                row("Two", None, Some(0)),
                // Indents are inherited.
                row("Three", Some(200), Some(0)),
                row("Four", None, Some(300)),
            ]
        );
        let plain = EpubDocument::open_with(&storage, "books/test.epub", false).unwrap();
        let block = plain
            .block_at(&storage, Locator::new(1, 0))
            .unwrap()
            .unwrap();
        assert_eq!((block.indent, block.space), (None, None));
    }

    #[test]
    fn keeps_the_books_text_sizes() {
        let ch2 = String::from(
            "<html><head><style>.title { font-size: 1.3em } .note { font-size: 80% } \
             div.small { font-size: small } h2 { font-size: 150% }</style></head><body>\
             <p class=\"title\">Title</p><p>Body</p><div class=\"small\">\
             <p class=\"note\">Note</p></div><h2>Part</h2></body></html>",
        );
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let mut at = Locator::new(1, 0);
        let mut found = Vec::new();
        while let Some(block) = doc.block_at(&storage, at).unwrap() {
            at = Locator::new(1, block.end);
            found.push((block.text.clone(), block.size));
        }
        let row = |t: &str, s| (String::from(t), s);
        assert_eq!(
            found,
            [
                row("Title", Some(130)),
                row("Body", None),
                // 80 % of `small` (89 %).
                row("Note", Some(71)),
                row("Part", Some(150)),
            ]
        );
    }

    #[test]
    fn ignores_stylesheets_without_publisher_styles() {
        let ch2 = String::from(
            "<html><head><style>p.b { font-weight: bold }</style></head><body>\
             <p class=\"c\">Mitte</p><p class=\"b\">Fett?</p>\
             <p style=\"font-style: italic\">Kursiv?</p><p><b>Fett</b></p></body></html>",
        );
        let storage = epub(ch2);
        // With styles first, so both caches exist side by side.
        let styled = EpubDocument::open(&storage, "books/test.epub").unwrap();
        assert_eq!(
            styled
                .block_at(&storage, Locator::new(1, 0))
                .unwrap()
                .unwrap()
                .align,
            Align::Center
        );
        let doc = EpubDocument::open_with(&storage, "books/test.epub", false).unwrap();
        let mut at = Locator::new(1, 0);
        let mut found = Vec::new();
        while let Some(block) = doc.block_at(&storage, at).unwrap() {
            at = Locator::new(1, block.end);
            found.push((block.text.clone(), block.align, block.style_at(0)));
        }
        assert_eq!(
            found,
            [
                (String::from("Mitte"), Align::Auto, TextStyle::REGULAR),
                (String::from("Fett?"), Align::Auto, TextStyle::REGULAR),
                (String::from("Kursiv?"), Align::Auto, TextStyle::REGULAR),
                // Markup stays.
                (String::from("Fett"), Align::Auto, TextStyle::BOLD),
            ]
        );
    }

    #[test]
    fn own_stylesheets_apply_after_the_books() {
        let ch2 = String::from(
            "<html><head><style>p.c { text-align: center }</style></head><body>\
             <p class=\"c\">Mitte</p></body></html>",
        );
        let storage = epub(ch2);
        use quireink_sdk::Storage;
        let align = |storage: &MemoryStorage| {
            let doc = EpubDocument::open(storage, "books/test.epub").unwrap();
            doc.block_at(storage, Locator::new(1, 0))
                .unwrap()
                .unwrap()
                .align
        };
        assert_eq!(align(&storage), Align::Center);
        // For all books of the folder, then this one: the last wins.
        storage
            .write("books/styles.css", b"p.c { text-align: left }")
            .unwrap();
        assert_eq!(align(&storage), Align::Start);
        storage
            .write("books/test.css", b"p.c { text-align: right }")
            .unwrap();
        assert_eq!(align(&storage), Align::End);
        // Without them, the book's own again.
        storage.remove("books/styles.css").unwrap();
        storage.remove("books/test.css").unwrap();
        assert_eq!(align(&storage), Align::Center);
    }

    #[test]
    fn plugins_stylesheets_apply_before_ones_own() {
        let ch2 = String::from(
            "<html><head><style>p.c { text-align: center }</style></head><body>\
             <p class=\"c\">Mitte</p></body></html>",
        );
        let storage = epub(ch2);
        use quireink_sdk::{OpenOptions, Storage};
        let align = |storage: &MemoryStorage, options: &OpenOptions| {
            let doc = EpubDocument::open_options(storage, "books/test.epub", options).unwrap();
            doc.block_at(storage, Locator::new(1, 0))
                .unwrap()
                .unwrap()
                .align
        };
        let themed = OpenOptions::default().css(String::from("p.c { text-align: right }"));
        assert_eq!(align(&storage, &themed), Align::End);
        // One's own stylesheet comes after the plugins' and wins.
        storage
            .write("books/styles.css", b"p.c { text-align: left }")
            .unwrap();
        assert_eq!(align(&storage, &themed), Align::Start);
        storage.remove("books/styles.css").unwrap();
        // Without the publisher's styles, none of them.
        let plain = themed.clone().publisher_styles(false);
        assert_eq!(align(&storage, &plain), Align::Auto);
        assert_eq!(align(&storage, &OpenOptions::default()), Align::Center);
    }

    #[test]
    fn splits_long_paragraphs_and_reuses_the_cache() {
        let words: Vec<String> = (0..3000).map(|i| alloc::format!("w{i}")).collect();
        let ch2 = alloc::format!("<html><body><p>{}</p></body></html>", words.join(" "));
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let ch2 = blocks(&storage, &doc, 1);
        assert!(ch2.len() > 3);
        let joined: Vec<&str> = ch2.iter().flat_map(|(_, t)| t.split_whitespace()).collect();
        assert_eq!(joined, words.iter().map(String::as_str).collect::<Vec<_>>());

        // A second open uses the cached conversion.
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        assert_eq!(blocks(&storage, &doc, 1).len(), ch2.len());
    }
}
