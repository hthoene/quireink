//! A small dictionary built here, plain and as dictzip, with synonyms.

// Tests panic on purpose when something is wrong.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use quireink_sdk::{MemoryStorage, Storage};
use quireink_stardict::{Dictionary, Error};

/// Headwords in StarDict order with their definitions; long enough that
/// the dictzip version spans several chunks.
fn entries() -> Vec<(String, String)> {
    let mut words: Vec<String> = (0..200).map(|i| format!("ord{i:03}")).collect();
    words.extend(["Hus", "hus", "hytte", "øy"].map(String::from));
    words.sort_by(|a, b| {
        a.to_ascii_lowercase()
            .cmp(&b.to_ascii_lowercase())
            .then_with(|| a.cmp(b))
    });
    words
        .into_iter()
        .map(|w| {
            let definition = format!("<p><b>{w}</b></p><ol><li>Bedeutung von {w}</li></ol>");
            (w, definition)
        })
        .collect()
}

/// Builds `.ifo`, `.idx`, `.syn` and the definitions (`.dict`, or
/// `.dict.dz` in chunks of `chunk` bytes).
fn dictionary(chunk: Option<usize>) -> MemoryStorage {
    let entries = entries();
    let mut dict = Vec::new();
    let mut idx = Vec::new();
    for (word, definition) in &entries {
        idx.extend_from_slice(word.as_bytes());
        idx.push(0);
        idx.extend_from_slice(&(dict.len() as u32).to_be_bytes());
        idx.extend_from_slice(&(definition.len() as u32).to_be_bytes());
        dict.extend_from_slice(definition.as_bytes());
    }
    let number = |w: &str| entries.iter().position(|(e, _)| e == w).unwrap() as u32;
    let mut syn = Vec::new();
    for (form, word) in [("husene", "hus"), ("huset", "hus"), ("øya", "øy")] {
        syn.extend_from_slice(form.as_bytes());
        syn.push(0);
        syn.extend_from_slice(&number(word).to_be_bytes());
    }
    let ifo = format!(
        "StarDict's dict ifo file\nversion=2.4.2\nbookname=Test\nwordcount={}\nsynwordcount=3\nidxfilesize={}\nsametypesequence=h\n",
        entries.len(),
        idx.len()
    );
    let storage = MemoryStorage::new()
        .with("d/test.ifo", ifo)
        .with("d/test.idx", idx)
        .with("d/test.syn", syn);
    match chunk {
        None => storage.with("d/test.dict", dict),
        Some(chunk) => storage.with("d/test.dict.dz", dictzip(&dict, chunk)),
    }
}

/// A dictzip file: gzip header with the `RA` chunk table, then each chunk
/// compressed on its own.
fn dictzip(data: &[u8], chunk: usize) -> Vec<u8> {
    let parts: Vec<Vec<u8>> = data
        .chunks(chunk)
        .map(|c| miniz_oxide::deflate::compress_to_vec(c, 6))
        .collect();
    let mut table = Vec::new();
    table.extend_from_slice(&1u16.to_le_bytes());
    table.extend_from_slice(&(chunk as u16).to_le_bytes());
    table.extend_from_slice(&(parts.len() as u16).to_le_bytes());
    for part in &parts {
        table.extend_from_slice(&(part.len() as u16).to_le_bytes());
    }
    let mut extra = b"RA".to_vec();
    extra.extend_from_slice(&(table.len() as u16).to_le_bytes());
    extra.extend_from_slice(&table);
    let mut file = vec![0x1f, 0x8b, 8, 4 | 8, 0, 0, 0, 0, 0, 3];
    file.extend_from_slice(&(extra.len() as u16).to_le_bytes());
    file.extend_from_slice(&extra);
    file.extend_from_slice(b"test.dict\0");
    for part in parts {
        file.extend_from_slice(&part);
    }
    file
}

fn prepare(storage: &MemoryStorage, dictionary: &Dictionary) {
    for mut builder in dictionary.builders(storage) {
        // Small steps, as in the background.
        while !builder.step(storage, 300).unwrap() {}
    }
    assert!(dictionary.builders(storage).is_empty());
}

#[test]
fn looks_up_words_and_forms() {
    for chunk in [None, Some(1000)] {
        let storage = dictionary(chunk);
        let dictionary = Dictionary::open(&storage, "d/test.ifo", "cache").unwrap();
        assert_eq!(dictionary.info.name, "Test");
        assert_eq!(
            dictionary.lookup(&storage, "hus"),
            Err(Error::NotPrepared),
            "needs its block index first"
        );
        prepare(&storage, &dictionary);

        // Both spellings of "hus", ignoring case.
        let found = dictionary.lookup(&storage, "HUS").unwrap();
        let words: Vec<&str> = found.iter().map(|d| d.word.as_str()).collect();
        assert_eq!(words, ["Hus", "hus"], "{chunk:?}");
        assert_eq!(found[1].text, "hus\n• Bedeutung von hus");

        // Inflected forms through the synonyms.
        let found = dictionary.lookup(&storage, "Husene").unwrap();
        assert_eq!(found[0].word, "hus");
        assert_eq!(dictionary.lookup(&storage, "ØYA").unwrap()[0].word, "øy");

        // Every headword, including the first and last, across chunks.
        for (word, _) in entries() {
            let found = dictionary.lookup(&storage, &word).unwrap();
            assert!(found.iter().any(|d| d.word == word), "{word} {chunk:?}");
            let expected = format!("{word}\n• Bedeutung von {word}");
            assert!(found.iter().any(|d| d.text == expected), "{word} {chunk:?}");
        }
        assert!(
            dictionary
                .lookup(&storage, "finnes-ikke")
                .unwrap()
                .is_empty()
        );
        assert!(dictionary.lookup(&storage, "a").unwrap().is_empty());
        assert!(dictionary.lookup(&storage, "zzz").unwrap().is_empty());
    }
}

#[test]
fn rebuilds_a_stale_block_index() {
    let storage = dictionary(None);
    let dictionary = Dictionary::open(&storage, "d/test.ifo", "cache").unwrap();
    prepare(&storage, &dictionary);
    // A half-written index (e.g. power lost) is not used.
    let index = quireink_sdk::read_file(&storage, "cache/idx.qbi", 1 << 16).unwrap();
    storage
        .write("cache/idx.qbi", &index[..index.len() - 4])
        .unwrap();
    assert_eq!(dictionary.builders(&storage).len(), 1);
}
