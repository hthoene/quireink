# 0007. Content-based reading positions

- Status: Accepted
- Date: 2026-10-14

## Context

The first idea was to rely on pixel-identical pagination to sync positions.
Pagination is only identical for identical screens, fonts and settings, so
positions would not transfer between devices or survive a font change.

## Decision

Reading positions are **locators** into the content, not page numbers:
spine item, node path and character offset for reflowable formats, byte
offset for plain text, page index for fixed layouts. Locators convert to and
from KOReader xpointers and percentages, so progress syncs with KOReader.
Pagination caches map locators to pages per device and settings.

## Consequences

- Positions survive font and layout changes and sync across devices.
- Layout must keep a mapping from rendered content back to locators, which
  also enables hit testing, selection and annotations.
