//! EPUB 2 and 3 support for QuireInk.
//!
//! - Metadata (title, authors, language) from the package document.
//! - Reading order from the spine; table of contents from the EPUB 3
//!   navigation document or the EPUB 2 NCX.
//! - Chapters are converted on first use — streamed from the ZIP archive
//!   through an XHTML tokenizer — into a compact block format stored in the
//!   plugin's cache. Reading afterwards needs only the block at hand, so
//!   books of any size work on small devices.
//!
//! Supported markup: paragraphs, headings, lists, block quotes, line
//! breaks, preformatted text and anchors. Styles (CSS, bold, italic) and
//! images are not rendered yet.

#![no_std]

extern crate alloc;

mod convert;
mod css;
mod document;
mod obfuscation;
mod package;

use alloc::borrow::Cow;
use alloc::boxed::Box;
use alloc::vec;

use quireink_sdk::document::Document;
use quireink_sdk::{
    FormatSpec, Id, Manifest, Plugin, RegisterError, Registrar, SDK_VERSION, Version,
};

pub use document::EpubDocument;

pub const PLUGIN: Id = Id::new_static("format-epub");
pub const FORMAT: Id = Id::new_static("format-epub.epub");

/// The EPUB format plugin.
#[derive(Debug, Default)]
pub struct EpubFormat;

impl Plugin for EpubFormat {
    fn manifest(&self) -> Manifest {
        Manifest {
            id: PLUGIN,
            name: Cow::Borrowed("EPUB"),
            description: Cow::Borrowed(
                "Opens EPUB books with their styles, table of contents and images.",
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
                EpubDocument::open_with(storage, path, options.publisher_styles)
                    .map(|d| Box::new(d) as Box<dyn Document>)
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
</ol></nav></body></html>"#;

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
    fn reads_metadata_chapters_and_toc() {
        let ch2 = String::from("<html><body><p>Zweites.</p></body></html>");
        let storage = epub(ch2);
        let doc = EpubDocument::open(&storage, "books/test.epub").unwrap();
        let metadata = &doc.metadata().clone();
        assert_eq!(metadata.title.as_deref(), Some("Die Verwandlung"));
        assert_eq!(metadata.authors, ["Franz Kafka"]);
        assert_eq!(metadata.language.as_deref(), Some("de"));
        assert_eq!(doc.sections(), 2);

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
