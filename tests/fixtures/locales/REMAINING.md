# Strings still rendering in English

Measured, not estimated. Reproduce with:

```
node scripts/render_check.js --locale en-XA
```

The gate replays API payloads captured from the real binary, clicks through all
five tabs, and reports every DOM text run with no accented character. **A scan
against the page at rest reports almost nothing** — the KPI cards, ring gauges,
perf blocks and table bodies only exist once data arrives, which is where most
of these live.

Last measured at 30 actionable runs (27 further runs are correctly untranslated:
frozen units, status codes, and data from the API). The gate prints both numbers,
so this file being stale is detectable rather than a matter of trust.

## What is left, by mechanism

**Chart series names and axis labels.** Passed as arguments to `lineChart`,
`stackChart` and `legend`, then escaped by those helpers, so they take `tRaw()`
rather than `t()`:

`requests` · `errors` · `disconnects` · `active` · `queued` · `median` ·
`20.0 sec` · `40.0 sec` · `60.0 sec`

**Composite runs built by concatenation.** Each needs to become one message with
placeholders, because word order moves:

| Run | Built by |
|---|---|
| `v0.6.5 · 3 keys · auth on` | `verinfo`, sidebar footer |
| `● Live    Default · 30d    Jul 29 – Jul 29` | the topbar row, read as one text run |
| `Default · 30d` | written into the active pill by `presetLabel()` at runtime, which is why the static markup being tagged is not enough |
| `errors 42% · 8 cooldowns` | model-card subtitle |
| `0 / 24 rpm · 3 keys` | capacity ring sub |
| `median 25 ms` · `median –` | perf-block value prefix |
| `5 · 0.17/req` | tool-intensity cell |
| `3.89 tok` | head-to-head bar label |
| `0 now` | KPI card sub |
| `3 models` · `Slot 1` | count fragments; `Slot` is the standard term, but the number makes it a message with `{n}` |

**`Proxy`** — sidebar heading, plain untagged text.

## Two things this file is not

It is **not** the settings surface. `renderSettings` and its sub-panels are out
of scope until 0.6.7 and the gate does not visit that tab at all, so none of
those strings appear above.

It is **not** a list of everything untranslated in the tree. The gate only sees
what the captured fixtures cause to render. Paths the fixtures never reach —
`content_filter` / `Filtered`, the `REASONS` rows for 400/403/404/502/503, the
`Disconnected` live badge, and every empty state — are tagged in the catalog but
unverified by any run. `knowledge/decisions/render-gate.md` records that gap.

## Correctly NOT translated

Model ids (`Kimi K2.5`), publisher names (`DeepSeek`, `Meta`, `Moonshot AI`),
client names (`local`), monogram letters, rank markers (`#1`), and every unit or
status code on the never-translate list. These come from the API; localizing
them would be manipulating data rather than labelling it.
