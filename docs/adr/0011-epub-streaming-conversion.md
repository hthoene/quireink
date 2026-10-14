# 0011. EPUB: streaming conversion into a block cache

- Status: Accepted
- Date: 2026-10-14

## Context

EPUB is a ZIP archive of XHTML chapters. The ESP32-C3 profile has a heap
budget of 192 KiB, and the 2-bit framebuffer alone takes 94 KiB of it. A
single chapter can be larger than the remaining memory, and inflating
deflate data needs a 32 KiB history window. Readers must jump to any
position (ADR 0007) without re-parsing a chapter from its start for every
page.

## Decision

- **Shared building blocks in the SDK.** `quireink-zip` reads the central
  directory entry by entry and inflates entries as a stream.
  `quireink-xml` is an incremental, allocation-light tokenizer. Both are
  re-exported by the SDK (`sdk::zip`, `sdk::xml`), so other ZIP- or
  XML-based format plugins (FB2, CBZ, DOCX, …) can reuse them.
- **Inflate with the smallest window that works.** The decoder state is
  about 10 KiB. Its window is a power of two that holds the whole entry,
  and at most 32 KiB. Small entries such as package documents therefore
  need only a few KiB.
- **The package is read once.** On first open, the spine (archive record
  and size per chapter), the table of contents and the metadata are
  written to the cache (`spine.bin`, `toc.dat`/`toc.idx`, `package.ini`).
  In memory an open book needs 8 bytes per archive entry (the ZIP index
  keeps name hashes, not names) and 12 bytes per chapter. The table of
  contents is read entry by entry (`Document::toc_len`, `toc_entry`,
  `toc_target`); anchors are resolved only when an entry is chosen.
- **Chapters are converted once, on first use.** The XHTML is tokenized
  while it is inflated and written to the plugin's cache
  (`.quireink/plugins/format-epub/<hash>/`):
  - `<n>.txt` holds block records (header and text);
  - `<n>.idx` holds the offset of every block, for binary search;
  - `<n>.anc` holds anchor ids and their offsets, for the table of contents;
  - `<n>.end` holds the text length. It is written last and marks a
    complete conversion.

  All files are appended in small chunks. Long paragraphs are split into
  blocks of at most 4,000 bytes.
- **Reading needs only the block at hand.** `block_at` binary-searches the
  index on storage and reads one record. Locators are offsets into
  `<n>.txt`. They stay valid as long as the cache version and the book file
  are unchanged; otherwise the cache is rebuilt (`meta.ini`).
- **Progress without converting everything.** Progress is estimated from
  the chapters' uncompressed sizes in the archive. Unread chapters are
  never converted for it.
- **Running out of memory is an error, not a crash.** Large buffers are
  allocated fallibly, and failures surface as `DocumentError::OutOfMemory`.

## Consequences

- Peak memory while converting is about 10 KiB (decoder) + up to 32 KiB
  (window) + about 8 KiB of buffers. Opening an already converted book
  needs much less.
- On the X4 profile, chapters of any size fit the budget since frames
  are rendered in bands (ADR 0012); Die Verwandlung converts with a peak
  of 115 of 192 KiB.
- The first opening of a chapter costs one pass over it plus writes to
  the SD card; later page turns are cheap.
- Blocks carry style runs and an alignment (cache version 5). The book's
  stylesheets are parsed once into a compact rule list (`styles.txt`); a
  small CSS subset (element and class selectors, alignment, weight, style,
  `display: none`) is applied while converting. Images are not part of the
  block format yet.
