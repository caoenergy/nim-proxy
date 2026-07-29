---
type: Decision
title: Format numbers, durations, and dates with Intl, keyed to the catalog locale
description: Hand-rolled K/M/B suffixes and concatenated units are replaced by cached Intl formatters reading the catalog's locale. CSS percentages are deliberately excluded.
tags: [i18n, dashboard, formatting]
timestamp: 2026-07-29T00:00:00Z
---

# Format numbers, durations, and dates with Intl, keyed to the catalog locale

## Context

With text coming from a catalog ([message-catalog-and-escaping](message-catalog-and-escaping.md)),
the numbers beside it were still formatted by hand: a ternary chain appending
`K`/`M`/`B`, durations built by concatenating `' ms'`/`' s'`/`' min'`, and seven
date strings assembled from `toLocaleDateString`/`toLocaleTimeString` with the
locale argument left as `[]` — the browser's, not the interface's.

That last part is the real defect. A dashboard rendered in German would have
grouped thousands the American way because the operator's OS happened to be
en-US.

The hand-rolled `fmt` also had two arithmetic bugs that nobody had hit:
`999999` rendered as **`1000.0K`** rather than rolling over to `1M`, because the
`>= 1e4` branch divided by `1e3` for everything below `1e6`; and `1e12` rendered
as **`1000.0B`**, because there was no tier above `B`.

## Options

1. **Leave it.** Cheapest, but locks the dashboard to one language's number
   habits and keeps two rollover bugs.
2. **Extend the ternary chain** — add a `T` tier, fix the divisor, add
   separators by hand. Fixes the bugs, still wrong for every non-English locale,
   and grows the thing that was wrong in the first place.
3. **`Intl`, keyed to the catalog locale.**

## Choice

**Option 3.** Formatters are constructed once at module scope — `Intl`
constructors are expensive and these run inside render loops — and read
`LOCALE` from the shipped catalog rather than the browser.

Thresholds are deliberately **unchanged**. `Intl`'s own compact notation begins
at 1,000 (`1K`); this dashboard shows exact counts up to 10,000 because request
counts in the hundreds are meaningful to an operator and `1.2K` is not.

**CSS percentages are excluded, and this is the important part.** Six
`toFixed()` calls remain, every one of them inside a `style=` attribute.
`style="width:12.3%"` must stay locale-independent: a comma-decimal locale would
emit `width:12,3%`, which is invalid CSS and silently collapses the element to
zero width. Display percentages go through `Intl` with `style: 'percent'`;
layout percentages stay raw. Confusing the two is a layout bug that appears only
in some locales, which is the worst kind.

Date stamps use explicit components rather than `dateStyle: 'short'`, which
abbreviates the year to two digits — ambiguous on a dashboard that retains 30+
days of history.

## Consequences

- Eleven formatter outputs change in en-US, each recorded in
  `tests/fixtures/formatters-en-US.txt`. Two are bug fixes (`999999` → `1M`,
  `1e12` → `1T`), five drop a redundant trailing `.0`, four change the seconds
  unit from `s` to the locale-correct `sec`, and one gains thousands grouping.
- `secs()` now reads `1.0 sec` rather than `1.0 s`. Slightly longer, but it is
  what `Intl` considers correct for en-US and it matches the `ms`/`min` forms,
  which already used a space and an abbreviation.
- The fixture harness reads the formatter bodies straight out of
  `src/dashboard.html`, so it cannot drift from the code it pins, and it refuses
  to run unless `TZ=UTC`. An unpinned fixture would encode whichever machine
  last wrote it.
- Verified in a second locale: `de-DE` yields `1,2 Mio.`, `1,5 Sek.`, `50 %`
  with the non-breaking space German uses. That is the whole point of the change
  and it is now demonstrable rather than assumed.
