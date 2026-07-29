---
type: Decision
title: Counted labels use CLDR plural categories, not an English ternary
description: >-
  `n === 1 ? '' : 's'` is English grammar compiled into the render path. Counted
  messages carry all six CLDR categories as explicit catalog ids, selected by
  Intl.PluralRules.
tags: [i18n, dashboard, locale, intl]
timestamp: 2026-07-29T00:00:00Z
---

# Counted labels use CLDR plural categories

## Context

Two labels pluralized themselves inline:

```js
`${cfg.lanes} enabled key${cfg.lanes === 1 ? '' : 's'}`
`${unknownCapacity} interval${unknownCapacity === 1 ? '' : 's'} with no capacity data`
```

Both survived the extraction pass and the untagged-string lint, because neither
contains a quoted prose string — the English is the *absence* of a character in
one branch of a ternary. They are the same class as
[the leaks that hid as arguments](message-catalog-and-escaping.md): grammar in
the render path is invisible to a scan that looks for text.

A ternary encodes two claims that are only true of English: that there are
exactly two forms, and that the boundary sits at 1. Arabic has six categories,
Russian and Polish three, Welsh six, Japanese one. `cfg.lanes` can also be 0,
which English happens to render as the plural and several languages do not.

## Options

1. **Keep the ternary, translate the two strings.** Cheapest, and wrong in a way
   that cannot be fixed by translators: they receive a singular and a plural slot
   and no way to add the four categories their language needs.
2. **A pluralization library.** Rejected at ladder rung 4 — the platform already
   ships this. `Intl.PluralRules` is CLDR data, in the browser, no dependency.
3. **`Intl.PluralRules` with all six categories in the source catalog.**

## Choice

Option 3. `Intl.PluralRules(LOCALE).select(n)` returns the category; a `switch`
maps it to a catalog id.

Two details that are not obvious and are the reason this page exists:

**All six categories live in the English source catalog**, even though English
uses two and the other four repeat the `other` text verbatim. `locale-v1`
enforces exact id parity between a translation and the source, so a category
absent from the source can never be *added* by a translation — the Arabic file
would be rejected for having ids English lacks. The redundancy is what makes the
languages that need those forms reachable at all. Each redundant form carries a
`desc` telling the translator so, because a translator seeing six identical
English strings will otherwise assume a mistake.

**The ids are spelled out, not built from the category.** `t(prefix + '.' +
select(n))` is shorter and breaks the orphan check: `check_i18n.py` finds
references by matching `t('literal')`, so a computed id makes all six look
unreferenced, and the check would then demand their deletion. Writing
`case 'few': return t('…enabled_keys.few', p)` keeps the check working.

## Consequences

- Six catalog ids per counted message. Two sets so far (`enabled_keys`,
  `history.no_data`), 12 of the 181 dashboard messages.
- A third counted label should reuse the `KEY_PLURALS` instance rather than
  constructing another `Intl.PluralRules`; the formatter-caching reasoning in
  [intl-formatting](intl-formatting.md) applies unchanged.
- English output is byte-identical to the ternaries, so this is invisible in the
  shipped locale — which means the pseudolocale run is the only check that
  observes it working. `node scripts/render_check.js --locale en-XA` shows the
  counted runs accented.
- The untagged-string lint still cannot see a ternary of this shape. If a third
  one is written, nothing will complain. Recorded here rather than solved,
  because a lint for "English grammar expressed as control flow" is not a lint
  anyone knows how to write.
