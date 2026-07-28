# Reset-aware dashboard history and time semantics

**Date:** 2026-07-28  
**Issue:** [#67 — Make dashboard history reset-aware and time-window consistent](https://github.com/miztertea/nim-proxy/issues/67)  
**Status:** Draft for user review; conceptual direction approved

## Summary

nim-proxy already persists full Prometheus snapshots, but the dashboard opens
from the current process's `/metrics` registry and a browser-local sample
buffer. A container restart therefore makes the default dashboard appear
empty even when `history.jsonl` contains substantial retained activity.
Historical range calculations also subtract the first cumulative value from
the last, which is invalid when counters reset between process boots.

This change makes persisted history the analytical source for every dashboard
tab. Rust will normalize cumulative counters into reset-aware deltas, compute
exact summaries independently of chart downsampling, and return a typed
dashboard response for the selected time window. The existing dashboard
layout remains intact. Small indicators will distinguish the selected window,
current operational state, and the configured SLO.

The default dashboard window and data-retention period become separate
UI-managed Server settings. Both default to 30 days. Retention may be
unlimited, but a finite retention period may not be shorter than the default
dashboard window.

## Goals

- Show retained activity immediately after login, including activity recorded
  before the current container boot.
- Apply one selected time window consistently across Overview, Models,
  Clients, Reliability, and Capacity.
- Keep current operational values visibly distinct from historical values.
- Preserve exact totals while independently downsampling chart series.
- Normalize counter resets across explicit future boot epochs and best-effort
  legacy history inference.
- Preserve existing `history.jsonl` data without an offline migration.
- Refresh browser-visible configuration after Settings changes without a page
  reload.
- Preserve the current tabs, visual hierarchy, cards, charts, tables,
  navigation, dark palette, and responsive layout.
- Keep all telemetry privacy-safe: counts, timings, bounded labels, and
  capacity shape only; never prompts, responses, credentials, or key
  fingerprints.

## Non-goals

- Adopting Prometheus, Datadog, Grafana, SQLite, or another external storage
  or visualization service.
- Redesigning the operator console.
- Adding disk-file watching for out-of-band `config.json` edits.
- Turning SLO management into a general reliability platform.
- Establishing durable identities for individual NIM keys in telemetry.
- Changing request admission, rate limiting, retries, authentication, or
  upstream behavior.

## Root causes

### Browser-local "lifetime"

`pollLive()` fetches `/metrics`, appends the current process registry to a
browser-local ring, and labels current cumulative counters as "lifetime."
Opening a new browser tab starts a new chart buffer. Restarting the proxy
starts a new metric registry. Neither path establishes a baseline from
persisted history.

### Invalid range subtraction

Historical totals use `last - first`. Prometheus counters are monotonic only
within one process lifetime. Across a restart the current value can be lower
than the preceding value, so direct subtraction loses traffic or collapses to
zero.

### Historical capacity uses current configuration

The dashboard obtains lane count and RPM capacity from `/dash/config.json`,
not from historical snapshots. Capacity views can therefore compare old load
with a pool configuration that did not exist at the time.

### Browser configuration is fetched once

The global dashboard `cfg` object is initialized from `/dash/config.json` only
at page boot. Settings writes correctly persist and atomically swap server
runtime state, but lane count, RPMs, capacity, pricing, and auth indicators in
an already-open browser can remain stale.

## Time model

### Selected window

The top-right time controller defines one dashboard-wide analytical window.
It applies to every historical chart, total, table, comparison, and grouped
breakdown on all five analytical tabs.

The default window is:

```text
[now - dashboard.default_window_days, now]
```

Its endpoint follows now until the operator pauses it or selects a fixed
custom range. A new server with less history than the configured window uses
the data that exists; it does not treat the unavailable prefix as zero-valued
activity.

Existing presets remain. Two explicit presets are added:

- **Default · Nd** — the configured default rolling window.
- **All retained** — the earliest retained snapshot through now.

"All retained" is deliberately not called "All time": finite retention can
delete older evidence.

### Current state

Operational gauges and current configuration represent **Now**, independently
of the selected analytical window:

- active and queued requests;
- current RPM/load;
- current lane count, per-lane RPMs, and aggregate capacity;
- current lane cooldown/pressure state where exposed;
- current model governor in-flight counts and limits;
- current version, uptime, pricing, and auth mode.

### SLO

The SLO target is a small Server setting with a conventional 99.9% default.
Reliability computes availability over the selected dashboard window and
compares it with that configured target. The numerator is successful 2xx
terminal outcomes. The denominator is every terminal outcome except 4xx
responses and client disconnects; server errors, upstream failures, stalls,
and deadlines remain eligible failures. No separate SLO window or broader SLO
subsystem is introduced.

## Configuration contract

The stored configuration gains dashboard-specific settings while retaining
the existing history retention object:

```json
{
  "history": {
    "days": 30
  },
  "dashboard": {
    "default_window_days": 30,
    "slo_target_percent": 99.9
  }
}
```

Serde defaults make existing version-1 stores forward-compatible; no store
version bump or migration command is required.

Validation rules:

```text
default_window_days >= 1
history.days == 0 || history.days >= default_window_days
0 < slo_target_percent <= 100
```

Invalid Settings saves return a clear 400 response and apply nothing. Values
are never silently clamped.

Settings → Server splits the current "Pricing & history" group into:

- **Pricing**
- **History & dashboard**
  - Default dashboard window
  - Data retention (`0` = unlimited)
  - SLO target
  - Earliest retained snapshot
  - Current history file size, when persistence is enabled

`DATA_DIR` remains deployment-level environment configuration.
`HISTORY_DAYS` remains a warned-and-ignored legacy variable.

## Persisted history format

### Versioned snapshot lines

New lines extend the existing JSONL record without exposing credentials:

```json
{
  "v": 2,
  "t": 1785260925,
  "boot": "random-process-epoch",
  "capacity": {
    "enabled_lanes": 3,
    "rpms": [40, 40, 40],
    "capacity_rpm": 120
  },
  "m": "Prometheus exposition text"
}
```

- `boot` is a random, non-secret identifier generated once per process.
- `capacity` contains only operational shape. It never contains keys,
  fingerprints, owners, or credentials.
- `rpms` follows the numeric lane order used by the metrics for that snapshot.
- Missing `v`, `boot`, or `capacity` identifies a legacy line and remains
  readable.

The process writes a boot marker at startup before the first periodic sample.
This establishes an unambiguous future reset boundary even when traffic begins
before the next five-minute sample.

```json
{
  "v": 2,
  "t": 1785253424,
  "boot": "random-process-epoch",
  "kind": "boot",
  "capacity": {
    "enabled_lanes": 3,
    "rpms": [40, 40, 40],
    "capacity_rpm": 120
  }
}
```

### Legacy reset inference

For history written before version 2:

1. A decrease in a cumulative series is a reset.
2. A disappearance of all counter series followed by their reappearance is a
   reset boundary.
3. Multiple simultaneous counter decreases strengthen the boundary but are
   not required.
4. Malformed JSON and malformed exposition lines remain skippable, with
   diagnostics recorded rather than a boot failure.

Legacy histories cannot reveal a restart when a counter resets and exceeds its
old value before the next stored snapshot. The loader preserves all source
lines and applies the strongest inference the evidence allows. Version-2 boot
epochs make future calculations exact.

## In-memory rollup index

Raw JSONL remains the durable, inspectable format. On boot, `History` parses
each retained snapshot once into a compact in-memory delta index. The
dashboard no longer replays repeated raw Prometheus text in the browser.

Index construction is a synchronous startup task completed before the server
is marked ready. The service already reads the complete retained JSONL file at
startup; normalization adds one parsing pass without introducing a new
asymptotic dependency. Startup logs the source bytes, valid and skipped
snapshot counts, normalized series count, inferred legacy resets, and elapsed
indexing time.

Startup computes the generic metric index only. It does not pre-render tabs,
cards, rankings, fixed time ranges, or chart resolutions. Those inexpensive
views remain query-time rollups over the index, so Settings changes and new
samples cannot leave a page-specific cache stale.

A monotonically increasing in-process `history_revision` changes whenever a
sample is added, retention changes the query boundary, or compaction replaces
the retained source set. The browser can therefore keep a range response
without polling and transferring it again when only current gauges have
changed.

A persisted sidecar index and asynchronous partial-readiness state are
deliberately deferred. If an unlimited-retention deployment later demonstrates
unacceptable measured startup time, either can be introduced without changing
the dashboard contract.

For each labeled series:

- counters, histogram buckets, histogram counts, and histogram sums become
  non-negative deltas;
- a boot change makes the current value the first delta of the new process;
- an unexpected decrease within one boot is treated as a defensive reset and
  recorded as a diagnostic;
- gauges retain their sampled values rather than becoming deltas.

The index stores interval deltas and the capacity context needed by dashboard
queries. It does not need to retain the repeated raw exposition string for
every in-memory point. This prevents the new query engine from doubling the
already substantial memory cost of `history.jsonl`.

The protected raw `/api/history` replay route is retired with the dashboard
migration; it is an internal UI transport rather than a documented public API.
The durable JSONL file remains backward-readable by nim-proxy itself.

## Window query and rollup rules

The typed endpoint accepts:

```text
GET /api/dashboard?from=<unix>&to=<unix>&points=<n>
```

Input rules:

- `from < to`;
- requested points are clamped to a safe range;
- omitted bounds use the configured default window through now;
- an all-retained request uses the available-history boundary returned by the
  server;
- a request may extend beyond retained history, but the response reports its
  effective data boundary explicitly.

Index construction uses each preceding sample as the next sample's counter
baseline. A normalized delta represents the interval ending at its sample
timestamp. A window accumulates deltas whose timestamps satisfy
`from < sample_time <= to`; no interpolation invents sub-sample precision.
Consequently, totals are exact with respect to retained samples, while their
wall-clock boundary precision is limited by the configured snapshot interval.

Exact totals and grouped breakdowns consume all underlying deltas. Chart
series are bucketed separately to the requested point budget. Changing chart
resolution can never change totals, rankings, availability, token counts, or
cost estimates.

Histogram quantiles are computed from accumulated per-bucket deltas. Empty
traffic intervals produce zero rate points and do not erase earlier activity
in the same window.

Capacity utilization uses the capacity metadata associated with each
interval. Current lane details still represent current generic capacity
slots; the system does not claim durable identity for secret NIM keys.

## Response contracts

Historical range results and current operational state use separate
role-protected contracts. This prevents the browser from transferring and
recomputing a 30-day range on every live poll.

Metric values use a typed generic shape so every existing dashboard metric can
flow through the contracts without building a separate API for each tab:

```json
{
  "metric": "nimproxy_requests_total",
  "labels": {
    "client": "Mindmap",
    "model": "z-ai/glm-5.2",
    "path": "/v1/chat/completions",
    "status": "200"
  },
  "value": 248
}
```

The range response from
`GET /api/dashboard?from=<unix>&to=<unix>&points=<n>` is:

```json
{
  "history_revision": 42,
  "window": {
    "requested_from": 1782668925,
    "requested_to": 1785260925,
    "effective_from": 1783077758,
    "effective_to": 1785260925,
    "available_from": 1783077758,
    "available_to": 1785260925,
    "following_now": true,
    "default_window_days": 30,
    "retention_days": 30
  },
  "totals": [
    {
      "metric": "nimproxy_requests_total",
      "labels": {
        "client": "Mindmap",
        "model": "z-ai/glm-5.2",
        "path": "/v1/chat/completions",
        "status": "200"
      },
      "value": 248
    }
  ],
  "latest": [
    {
      "metric": "nimproxy_model_inflight",
      "labels": {
        "model": "z-ai/glm-5.2"
      },
      "value": 0
    }
  ],
  "points": [
    {
      "from": 1785257325,
      "to": 1785260925,
      "duration_seconds": 3600,
      "values": [
        {
          "metric": "nimproxy_requests_total",
          "labels": {
            "client": "Mindmap",
            "model": "z-ai/glm-5.2",
            "path": "/v1/chat/completions",
            "status": "200"
          },
          "value": 12
        }
      ],
      "capacity": {
        "average_rpm": 120,
        "latest_rpms": [40, 40, 40]
      }
    }
  ],
  "diagnostics": {
    "legacy_resets_inferred": 0,
    "malformed_points_skipped": 0
  }
}
```

The lightweight current-state response from `GET /api/dashboard/now` is:

```json
{
  "history_revision": 42,
  "config_revision": 7,
  "sampled_at": 1785260925,
  "available_from": 1783077758,
  "available_to": 1785260925,
  "started": 1785253424,
  "version": "0.6.4",
  "lanes": 3,
  "rpms": [40, 40, 40],
  "capacity_rpm": 120,
  "price_in": 0.5,
  "price_out": 2.0,
  "auth": true,
  "default_window_days": 30,
  "retention_days": 30,
  "slo_target_percent": 99.9,
  "tail": {
    "base_history_revision": 42,
    "from": 1785260625,
    "to": 1785260925,
    "totals": [
      {
        "metric": "nimproxy_requests_total",
        "labels": {
          "client": "Mindmap",
          "model": "z-ai/glm-5.2",
          "path": "/v1/chat/completions",
          "status": "200"
        },
        "value": 2
      }
    ]
  },
  "metrics": [
    {
      "metric": "nimproxy_active_requests",
      "labels": {},
      "value": 0
    }
  ]
}
```

`totals` contains exact accumulated counter, histogram bucket, histogram
count, and histogram sum deltas across every underlying snapshot in the
window. `latest` contains the last gauge value in the window. `points`
contains separately bucketed values for charts; counter-like values are sums
of interval deltas and gauges are the last value in the bucket. Capacity is a
time-weighted interval average plus the latest per-lane shape.

The browser derives model, client, outcome, and lane groupings from the exact
`totals` rows using the dashboard's existing grouping primitives. Rankings and
scalar cards never derive from downsampled `points`. The generic contract
mirrors the signals already rendered by the five tabs rather than inventing
new product scope.

`tail` is a server-normalized, non-cumulative delta from the last indexed
snapshot through `sampled_at`. It is recomputed from the current registry on
each poll and replaces, rather than adds to, the prior tail. The browser
overlays it only when `base_history_revision` matches its cached range and the
selected window follows now. This keeps charts and totals current between
five-minute snapshots without re-querying 30 days of indexed history or
reintroducing reset calculations in JavaScript.

On page load or a range change, the browser fetches the range and current-state
contracts in parallel. It then polls only `/api/dashboard/now`. When
`history_revision` changes, it refreshes the selected range before accepting
the new tail. When `config_revision` changes, it updates current state and, if
the selected window is the configured default, recalculates that window.
Fixed historical ranges ignore the tail and remain stable while current-state
indicators continue to refresh.

The old boot-only `/dash/config.json` dependency is removed from dashboard
state. Settings endpoints remain role-filtered and independently fetch
`/api/config` for editable configuration.

## Dashboard behavior

No layout redesign occurs.

The header separates range from follow state:

```text
● Following now    Default · 30d    Jul 3 – Jul 28
```

A fixed historical range renders:

```text
○ Fixed range      Jul 13, 00:00 – Jul 14, 00:00
```

Normal analytical widgets inherit the selected window without repetitive
badges. Only exceptions receive small scope indicators:

- **Now**
- **All retained**

The Reliability SLO card states the configured target alongside the selected
window it evaluates.

The selected window persists while switching tabs. Settings remains
configuration-driven and is not governed by the historical selector.

Empty states say **No traffic in selected window**. A server with a partial
default window shows its available activity and effective dates rather than
an empty dashboard.

## Tab semantics

| Tab | Selected window | Current state |
|---|---|---|
| Overview | requests, tokens, savings, activity, outcomes, top models and clients | active requests, queue, current RPM |
| Models | usage, latency, throughput, output quality, finish reasons, comparisons | governor limits and in-flight counts |
| Clients | requests, tokens, tool intensity, depth, sampling, streaming mix | current activity only where measured |
| Reliability | terminal outcomes, stalls, deadlines, latency, pressure events, availability against the configured target | active load and queue |
| Capacity | historical traffic, utilization, rate-limit pressure, lane activity | enabled lanes, RPM budgets, aggregate capacity |

## Retention and compaction

Changing retention updates the in-memory query boundary immediately.

When retention decreases:

1. expired in-memory rollup points are removed immediately;
2. an atomic background compaction rewrites `history.jsonl` with retained
   source lines by streaming the existing file into a temporary file;
3. persistence appends serialize behind the compaction lock so no snapshot is
   lost, while dashboard queries continue from the in-memory index;
4. failure leaves the prior durable file intact and reports a warning;
5. a later sample retries compaction.

The Settings response may report that disk compaction is pending, but retained
queries must honor the new boundary immediately. Unlimited retention skips
age pruning.

## Error handling

- Invalid time windows return 400 with a stable JSON error code.
- A valid window with no retained traffic returns an empty typed response,
  not an exception.
- Malformed legacy lines are skipped and counted in diagnostics.
- A rollup query never exposes the raw history file or secret configuration.
- Failure to persist history continues to degrade to in-memory history with a
  boot warning; config-store failures remain fatal as today.
- A current-registry parse failure does not discard historical results; the
  response reports stale/unavailable current state.
- Settings validation and atomic persistence remain all-or-nothing.

## Security and privacy

- The existing session gate protects the typed dashboard endpoint.
- Server-side role filtering for Settings remains unchanged.
- Capacity metadata excludes key material, fingerprints, ownership, and
  usernames.
- Metric label sanitization and cardinality bounds remain authoritative.
- The dashboard continues to pass every dynamic string through `esc()` before
  any `innerHTML` sink.
- No prompt or response content is introduced into history, diagnostics,
  fixtures, screenshots, or logs.
- Browser tests use synthetic names and data. Live verification records only
  bounded request metadata already captured by the product.

## Testing strategy

### Unit tests

- version-1 and version-2 JSONL parsing;
- explicit boot-boundary normalization;
- counter decrease within a boot;
- legacy disappearance/reappearance inference;
- baseline snapshot immediately before a window;
- counters that appear partway through a window;
- histogram bucket/count/sum resets and quantiles;
- gauges versus counters;
- exact totals invariant under different point budgets;
- zero-traffic intervals;
- effective and available history boundaries;
- historical capacity changes;
- finite and unlimited retention validation;
- immediate in-memory prune and atomic compaction behavior;
- current configuration refresh in typed responses.
- transient current-registry tail normalization and revision matching.

### End-to-end tests

Using the existing real-binary harness and scripted mock:

1. create traffic and persisted samples;
2. restart the proxy with the same data directory;
3. create more traffic;
4. verify one range returns the combined exact totals;
5. verify all five tab data groups use the same window;
6. change key enablement/RPM and verify current capacity refreshes without a
   browser reload;
7. verify historical capacity uses its contemporaneous configuration;
8. exercise Default, All retained, fixed, and no-traffic windows;
9. lower retention and verify query pruning plus durable compaction;
10. load an unmodified legacy history fixture.

### Browser verification

Use an isolated proxy and synthetic traffic to verify:

- first login immediately renders retained traffic;
- follow-now advances the window;
- pause freezes the endpoint;
- presets, custom dates, and All retained update every analytical tab;
- selected range survives tab switching;
- Now indicators continue refreshing;
- Settings changes update capacity and header metadata without reload;
- empty windows render honest messages;
- responsive behavior and existing chart/table interactions remain intact;
- no console errors or unsafe dynamic rendering.

Capture screenshots for the PR from synthetic data only.

### Project gates

- `cargo test`
- `cargo fmt`
- `cargo clippy --all-targets -- -D warnings`
- targeted browser verification
- security review of new parsers, JSON responses, and dashboard sinks
- rate-limit load harness only if implementation unexpectedly touches request
  admission or pacing

## Documentation and release impact

The implementation updates:

- `knowledge/architecture/metrics-history.md`
- `knowledge/architecture/dashboard.md`
- `knowledge/ops/configure-env.md`
- `knowledge/decisions/history-retention-days-not-size.md`
- a new decision page for reset-aware server rollups and time semantics
- `knowledge/index.md`, `knowledge/decisions/index.md`, and `knowledge/log.md`
- README dashboard/history and Settings sections
- CHANGELOG

This is a user-visible dashboard and configuration change and should ship in a
normal release after merge. The eventual PR includes `Closes #67`.
