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
string; `tRaw(id)` returns the plain one for `setAttribute` and `textContent`,
neither of which parses entities. Placeholder values are escaped as they are
substituted — substitution happens *after* the message is escaped, so an
unescaped param would land raw in exactly the sinks this decision exists to
protect.

The corollary is that **helpers must not escape their label argument**.
`sortTable` did, which double-encoded every catalog-supplied column header —
invisible in en-US, where no header contains `&` or `'`, and wrong the first
time a locale or a copy edit introduces one. Worse, one id fed both an escaping
sink (`sortTable`) and a non-escaping one (`metricRow`), so a single message had
two escaping regimes. `sortTable` no longer escapes; column labels come from a
literal or the catalog, never from request data.

And `chipHtml`'s `onerror=` is gone before any extraction: a delegated
capture-phase `error` listener reads the fallback from `data-*` attributes.
The file now has no JavaScript-context interpolation at all.

That alone did not make the rule "catalog values reach element content and
quoted attributes only" *enforced* — `applyStatic` called `setAttribute` with
whatever attribute name the markup supplied, so a markup edit could still route
a catalog value into `onclick=` or `style=`, and the CSP permits inline
handlers. It is enforced now: the runtime accepts only `title`, `placeholder`,
`aria-label`, and `alt`, and `check_i18n.py` rejects any other target.

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
- Eight bindings named `t` shadowed the new global — two cursor timestamps,
  four callback parameters, an error-rate denominator inside `renderModels`
  (which is dense with `t()` calls), and one in the custom-range handler. All
  renamed. None broke at the time, but each was a silent failure waiting for
  the next `t()` call added in that scope. Renaming one of them *did* break
  `applyRange` mid-edit — the old name was still referenced two lines down, so
  `isFinite(t)` tested the function and the Apply button stopped working. It
  was caught by re-reading, not by any test.
- Ordering matters: `applyStatic` must run **before** the tab-restore loop.
  Running it after meant deep-linking to `#models` displayed the Models section
  under a topbar reading "Overview", because the loop sets `#pagetitle` and the
  catalog pass then overwrote it. `cargo test` and `check_i18n.py` both passed
  on that; only a headless render at `#models` showed it.
- **Which helper escapes is not uniform, and the call site has to know.** The
  Context inventory above is no longer current: `kpiCards` stopped escaping
  `k.label` in `77421e2`, so it now matches `metricRow`/`perfBlock`/`tile`/
  `prow`/`stat` and takes `t()`. The *opposite* contract holds for `ringGauge`
  (escapes `label` in both the `aria-label` and `.rlabel`), `legend` and both
  charts' hover tooltips (escape `s.name`), `barList`/`leaderList` (escape
  `name`/`label`, and must keep escaping — those carry model ids), and any site
  that wraps the value in `esc()` itself: the reliability error segbar's
  `title=` and the non-success outcomes table. All of those take `tRaw()`, as
  do `textContent` and `setAttribute`. Reading the sink is the only way to
  choose, and `--escape-probe` is the only check that tells a wrong choice
  apart from a right one — it fails on entity text in the DOM.
- Message ids are always spelled out at the call site, never built by
  concatenation. `tRaw('dashboard.nav.tab.' + tab)` is invisible to a static
  linter, which is the one thing that stops English creeping back. The status
  taxonomy is the case that most invites the shortcut: `REASONS` is keyed by
  HTTP status, so `t('dashboard.common.status.' + s)` would work and would be
  unlintable. Every entry is written out instead.
- The settings surface (~79 strings) is deliberately not extracted; it lands in
  0.6.7.
