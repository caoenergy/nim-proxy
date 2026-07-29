# Strings still rendering in English

Measured by replaying API payloads captured from the real binary into the page
and scanning the DOM under `en-XA`. **A scan against the page at rest reports
almost nothing** — the KPI cards, ring gauges, perf blocks and table bodies only
exist once data arrives, which is where most of these live.

Reproduce: build a copy of `src/dashboard.html` with `fetch` stubbed to replay
`/api/config`, `/api/dashboard` and `/api/dashboard/now`, swap the inline
catalog for `locales/en-XA.json`, render headless, and flag any text run with no
accented character.

## Genuine leaks

| String | Where | Why it was missed |
|---|---|---|
| `Capacity used` | Overview ring gauge | `ringGauge` label argument |
| `Success rate` | Overview ring gauge | same |
| `Time to first token` · `Generation speed` · `Inter-token latency` | Overview perf blocks | `perfBlock` label argument |
| `lower is better` · `higher is better` | Overview perf blocks | `perfBlock` note argument |
| `Active now` · `Queued` · `Error rate` | Reliability live panel | rendered by `prow`, not `metricRow` |
| `errors` | Reliability chart legend | chart series name |
| `selected time range` | KPI card subs | `windowLabel` const |
| `Default · 30d` | active range pill | overwritten at runtime by `presetLabel()` |
| `v0.6.5 · 3 keys · auth off` | sidebar footer | `verinfo`, built by concatenation |

## Fragments with interpolated counts

These are the ones PR 3 explicitly deferred — they need plural-category
messages, not string concatenation, and each is one message with placeholders:

`0 now` · `2,400 in` · `20 of 20` · `0 / 120 rpm · 3 keys` ·
`3 enabled keys` · `120 rpm available` · `4 models` · `4 tok` ·
`SLO 99.9% · met` · `0% used` · `Slot 1`

`3 enabled keys` and `N intervals with no capacity data` are the two hardcoded
English plural ternaries. They must be plural-category messages
(`zero`/`one`/`few`/`many`/`other`) rather than booleans — `cfg.lanes` can be 0,
and ar/ru/pl/cy need categories English does not have.

## Correctly NOT translated

Model ids (`Kimi K2.5`), publisher names (`DeepSeek`, `Meta`, `Moonshot AI`),
client names (`local`), monogram letters, rank markers (`#1`), and every unit or
status code on the never-translate list.
