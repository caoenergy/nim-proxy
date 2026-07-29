---
type: Decision
title: Guard localization with a pseudolocale, a validator, and an untagged-string lint
description: Three language-independent guards, each with a negative fixture proving it can fail, because translation correctness cannot be reviewed by reading English.
tags: [i18n, testing, ci]
timestamp: 2026-07-29T00:00:00Z
---

# Guard localization with a pseudolocale, a validator, and an untagged-string lint

## Context

With the catalog in place ([message-catalog-and-escaping](message-catalog-and-escaping.md)),
the failure modes stop being things a reviewer can see. A missing key renders the
raw id. A renamed placeholder leaves `{count}` on screen. A translation made from
an older English string stays valid and quietly says the wrong thing. And a
hardcoded label added next month looks exactly like the code around it.

None of that is visible in an English screenshot, and none of it is reachable by
`cargo test`, which asserts on served HTML text and never parses the JavaScript.

## Options

1. **Review translations by eye.** Requires a fluent speaker per locale per
   change. Does not scale past one language and catches none of the structural
   faults, which are the common ones.
2. **Trust the generation pipeline.** The pipeline is exactly what needs
   checking.
3. **Language-independent guards.** Nothing here needs a human to read Chinese.

## Choice

**Option 3**, three guards:

**`en-XA` pseudolocale.** Accents every letter, brackets each message, pads to
135%. Untranslated text stays plain ASCII and stands out; a container that only
fits English fails here rather than after a locale ships; a clipped string shows
as a missing `]`. Generated, never maintained, so it is always complete and
never stale. Placeholders pass through untouched — an accented `{çôûñt}` would
not substitute, which would make the pseudolocale test itself instead of the
layout.

**`locale-v1` validator.** Completeness, orphans, placeholder parity, formatter
syntax, no raw markup, inline balance, source-hash freshness, opt-in length
caps. The hash is what makes a regenerate-on-drift pipeline possible: it records
which English text a translation was made from, so "still valid, no longer
correct" is detectable.

**Untagged-string lint.** Fails on a display literal that bypasses `t()`,
covering attributes as well as text. Without it, English creeps back within two
PRs, because a hardcoded label looks exactly like the code beside it.

**Every check has a negative fixture, and the fixtures came first.** They are
committed in a separate commit before the validator exists, and `--selftest`
asserts each one fails for its own reason. A check nobody has watched fail is
decoration: nothing establishes it can.

## Consequences

- The guards immediately found three real defects that had survived PR 3's
  review and its own linter:
  - The **runtime-churn strings** were never extracted — `Live`, `Absolute`,
    `Disconnected` on the dashboard, and `Validating…`, `Copied`,
    `Select & copy` plus the document `<title>` in the wizard. PR 3's inventory
    predicted this exact gap; the pseudolocale is what made it visible.
  - `locale-v1 --all` was pairing the **wizard's** locale against the
    **dashboard's** source catalog, reporting every id in both as missing or
    orphaned. The validator caught a bug in its own runner.
  - `setup.html` called `tRaw()` while defining only `rawMsg()`. Because
    `applyStatic` aborts on the first throw, one missing helper left the entire
    page untranslated rather than one string — and neither `node --check` nor
    `cargo test` can see it. A dedicated lint now catches that class.
- The untagged-string lint is deliberately scoped: the settings surface is
  excluded until 0.6.7, and `chipHtml`'s interior is excluded permanently.
  Flagging `chipHtml` would invite someone to "fix" it by routing a catalog
  value through the URL and script contexts PR 3 spent its effort removing.
- `en-XA` proves layout mechanically but not **clipping**, which needs eyes.
  That review stays with Thomas, as the plan always had it.
