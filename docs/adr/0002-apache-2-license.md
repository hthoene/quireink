# 0002. Apache License 2.0

- Status: Accepted
- Date: 2026-10-14

## Context

The license determines who can adopt QuireInk and which libraries it may
use. Options were MIT/Apache dual licensing, Apache-2.0 alone, GPL-3.0 and
AGPL-3.0 (as KOReader uses).

## Decision

QuireInk is licensed under Apache-2.0 only. Contributions are accepted under
the same license (inbound = outbound) with a DCO sign-off.

Copyleft libraries (e.g. MuPDF under AGPL, DjVuLibre and FBInk under GPL)
are not linked into QuireInk itself. They may only be used by optional
plugins that are distributed separately under a compatible license.
License compliance of all dependencies is checked in CI with `cargo-deny`.

## Consequences

- Permissive and patent-safe for users, vendors and contributors.
- KOReader code cannot be copied; it can only be studied.
- The Linux shell implements vendor e-ink ioctls itself instead of using
  FBInk.
- PDF support uses a permissive engine (e.g. pdfium) by default.
