# 0009. Documents, formats and local storage

- Status: Accepted
- Date: 2026-10-14

## Context

Reading books needs file access, a way for plugins to support file formats,
and a document model that layout engines can work with — on a device with
a few hundred kilobytes of RAM and on devices with gigabytes. ADR 0003 keeps
the kernel free of I/O.

## Decision

- **Storage is a platform service.** Shells implement the `Storage` trait
  (`quireink-platform`): random-access reads, atomic whole-file writes,
  appends, listing and removal on relative, `/`-separated paths. Local
  storage is accessed synchronously — reading kilobytes from an SD card is
  fast enough everywhere and keeps plugins simple. Slow, remote resources
  (network) will be asynchronous effects. Plugins reach storage through
  `Env::storage`, kernel and plugins never touch a file system directly.
- **Formats are contributions.** A `FormatSpec` maps file extensions to an
  opener that returns a `Document`. Later registrations win, so any format
  can be replaced.
- **Documents are read block by block.** A document has sections; each
  section is a sequence of blocks (paragraphs, headings) with normalized
  text and an offset map back to the source. `Document::block_at` returns
  the block containing a `Locator`, and every position inside a block yields
  the same block. Very long paragraphs are split into deterministic pieces.
- **Layout is a library** (`quireink-layout`), deterministic and able to
  start at any line, so readers only store page *starts*. Any reader plugin
  can use it.
- **Reader state lives in the plugin's storage namespace**
  (`.quireink/plugins/<id>/`): reading position per book and a page index
  built in the background (one 8-byte entry per page, read in place).

## Consequences

- A 5 MB plain text file reads on the ESP32-C3 profile within its heap
  budget; page turns never wait for pagination.
- Formats only implement `block_at`; layout, pagination, progress and page
  numbers come for free.
- Plugins that need asynchronous storage (e.g. network shares) will need
  the planned effect-based I/O.
