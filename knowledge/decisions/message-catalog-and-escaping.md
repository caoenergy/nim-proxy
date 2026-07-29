---
type: Decision
title: Embed the message catalog and escape it once, at load
description: Extraction routes every user-visible string through t(). Catalog values are plain text escaped once on load, because four render helpers interpolate their arguments into innerHTML without escaping.
tags: [i18n, dashboard, security, xss]
timestamp: 2026-07-29T00:00:00Z
---

# Embed the message catalog and escape it once, at load

## Context

Localizing the dashboard means every user-visible string has to come from a
catalog rather than a literal. `src/dashboard.html` is one embedded file with
no build step ([dashboard](../architecture/dashboard.md)), so the catalog has
to ship inline and the runtime has to be a few lines, not a framework.

The hard part is not the lookup. It is that the dashboard builds HTML by string
concatenation into `innerHTML` — 45 sites — and the escaping posture recorded in
[input-sanitizing-and-xss](input-sanitizing-and-xss.md) assumes every *dynamic*
value passes through `esc()`. Catalog values are a new class of dynamic value,
and an inventory of the render path found four helpers that interpolate their
arguments with no escaping at all:

| Helper | Argument |
|---|---|
| `metricRow` | label |
| `perfBlock` | label |
| `tile` | label |
| `prow` | label |

`kpiCards` escapes `k.label` but not `k.value` or `k.sub`, and the delta-chip
`title=` attribute is interpolated raw. All six are safe *today* only because
every argument is a hardcoded literal. The moment they read from a catalog,
they are injection sinks — and a catalog is exactly the kind of file that later
gets machine-generated, contributed, or edited by someone who is not reading
this page.

Separately, `chipHtml` built an `onerror="…"` attribute containing a JavaScript
statement containing single-quoted JS string literals — three nested contexts,
no escaping. It was safe only because `initialsOf()` strips its input to
`[A-Za-z0-9 ]`, a helper written to build monograms, not to sanitize. One
catalog candidate (`'unknown'`, the fallback publisher name) reached it.

## Options

1. **Escape at every call site.** Wrap each `t()` result in `esc()`. Correct if
   done everywhere, and silently wrong the first time someone forgets. Six
   sinks today, more as the file grows.
2. **Fix the four helpers to escape their arguments.** Better, but it changes
   the contract of helpers that also receive already-escaped HTML fragments
   from other call sites, so it would double-escape those.
3. **Convert `innerHTML` to `textContent`.** Explicitly out of scope — that is a
   rewrite of the render path, not an extraction.
4. **Escape once, at load.** The catalog stores plain text; the runtime escapes
   every value as it is parsed. `t()` is then safe to interpolate anywhere a
   literal was safe, which is the whole file.

## Choice

**Option 4.** Catalog values are plain text. `t(id, params)` returns an escaped
string; `tRaw(id)` returns the plain one for `setAttribute`, which does not
parse entities. Placeholder substitution happens after escaping, so params keep
the existing `esc()` discipline exactly as before.

And `chipHtml`'s `onerror=` is gone before any extraction: a delegated
capture-phase `error` listener reads the fallback from `data-*` attributes.
The file now has no JavaScript-context interpolation at all, so the rule
"catalog values reach element content and quoted attributes only" is
enforceable rather than aspirational.

Three tagging mechanisms, chosen so markup structure never changes:

- `data-i18n` — replaces an element's content
- `data-i18n-attr="attr:id"` — sets an attribute
- `data-i18n-text` — replaces an element's *own first text node*, for headings
  that mix an icon, a text node, and a `.note` span. Assigning to a text node
  cannot introduce markup, so this path needs no escaping at all.

Inline emphasis inside a message (`{b}…{/b}`) and fixed literals (`{key}`,
`{endpoint}`) are placeholders expanded from a table in code, never markup
stored in a catalog value.

## Consequences

- English rendering is unchanged. `scripts/check_i18n.py` is the proof: every
  tagged element still holds exactly the text its id claims, no id is missing
  or orphaned, no hash is stale. Verified non-vacuous against three broken
  inputs.
- **`cargo test` cannot validate any of this.** The e2e suite asserts on served
  HTML text, so it passes on JavaScript that does not parse — and it did. An
  extraction pass wrote `${…}` into single-quoted strings and only
  `node --check` caught it. Treat the Rust suite as necessary, never sufficient,
  for changes to the embedded pages.
- A headless-Chromium render that mutates three catalog values and confirms the
  DOM follows is the end-to-end check. It is the only way to prove the runtime
  actually ran, since an unmutated en-US catalog renders identically whether
  substitution happened or not.
- Two chart hover handlers declared `const t` for a cursor timestamp and
  shadowed the new global. Renamed to `at`. Nothing was broken yet, but a
  `t()` call added inside those scopes later would have failed silently.
- The settings surface (~79 strings) is deliberately not extracted; it lands in
  0.6.7.
