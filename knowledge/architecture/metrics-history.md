---
type: Component
title: Metrics & history (src/history.rs)
description: Prometheus registry, versioned JSONL snapshots, reset-aware startup index, exact range rollups, and atomic retention compaction.
tags: [metrics, history, prometheus]
timestamp: 2026-07-02T00:00:00Z
---

# Metrics & history — `src/history.rs`

**Live metrics** use a `metrics-exporter-prometheus` registry rendered at
authenticated `GET /metrics` (series list in the README). Custom histogram
buckets cover TTFT, tokens/sec, queue wait, upstream latency, and request-shape
distributions. The registry remains the collection source; the dashboard no
longer downloads or parses its raw exposition.

## Persisted format

At startup the history component writes a v2 boot marker; the sampler appends
its first full-registry snapshot immediately, then sleeps for five minutes
between later samples. Real file size depends on the number of metric series,
labels, and histogram buckets; the original fixed-size estimate was disproven
by operation
([decision](../decisions/history-retention-days-not-size.md)).
`HISTORY_SAMPLE_SECS` is an undocumented test knob; five minutes is the
operator contract.

The reader accepts two formats:

- **v1 legacy sample**: `{"t": <unix>, "m": "<Prometheus text>"}`.
- **v2 boot marker/sample**: a boot marker records `v`, timestamp, random boot
  id, `kind:"boot"`, and capacity; each v2 sample repeats its boot id and
  contemporaneous capacity beside the exposition.

The explicit boot id makes every future process reset unambiguous. v1 files
remain readable: a sampled counter decrease, or a transition from a snapshot
with no counters at all to one with counters, is treated as a best-effort
inferred reset and counted in diagnostics. A legacy interval has no historical
capacity metadata; the dashboard reports that capacity as unavailable rather
than substituting today's configuration.

## Startup index and range rollups

History indexing is a synchronous startup task and completes before the
server listens. The parser scans and sorts the complete physical JSONL,
enforcing exposition metric-line and series bounds. It normalizes every valid
sample so a pre-retention sample can supply the boundary baseline, then indexes
only the retained points (or all points when retention is unlimited). Stale
disk rows may remain until compaction. Startup logs bytes, valid samples,
skipped records/metric lines, normalized series, inferred resets, and
duration. This is the only precomputation: page-specific dashboard models are
deliberately not cached
([reset-aware decision](../decisions/reset-aware-dashboard-history.md)).

`History::rollup(from, to, points)` returns:

- exact counter totals for the requested/effective window;
- the latest observed value per gauge series within the window;
- chart buckets capped by the requested point budget;
- contemporaneous capacity per chart bucket;
- available/effective bounds, diagnostics, and a monotonic history revision.

Totals are computed from the normalized index, not from the chart buckets, so
`points=2` and `points=288` return identical totals. Sample timestamps are the
precision boundary; partial buckets do not pretend to know intra-sample event
times. A delta belongs to a range when `from < sample_time <= to`. The HTTP
contract defaults to 288 presentation points and clamps requests to 2–1000.
When a requested range begins before the first retained sample, that sample's
exact delta remains in the total, but its chart point has zero duration and no
capacity average: the unavailable prefix is not treated as observed time.

`History::current()` renders the live registry under the same history
generation lock. It returns current typed metrics plus a tail whose totals are
the counter delta after the persisted baseline. The tail carries
`base_history_revision`; the browser accepts it only alongside the matching
range revision, so a newly persisted sample cannot be double-counted.

## Retention and durability

`history.days` in the
[config store](../decisions/ui-managed-config-store.md) defaults to 30;
`0` means unlimited. It is distinct from
`dashboard.default_window_days`, and finite retention must be at least as
long as the default dashboard window.

Changing retention in Settings validates and persists the complete config,
then immediately trims the visible in-memory index. File compaction runs
off the async executor and writes a temporary file, flushes it, atomically
renames it, and syncs the directory. It preserves the newest pre-cutoff
sample as a hidden baseline plus the relevant boot marker, which keeps the
first retained counter delta exact. A pre-replacement failure leaves the old
file untouched and the retry pending; a post-rename directory-sync failure is
reported as committed-but-pending.

At boot, expired rows are excluded from the index immediately and exposed as
compaction debt for a subsequent append. Routine append-driven compaction also
starts after more than 288 expired samples accumulate (about one day at the
five-minute interval). A history-file write failure warns and leaves the
in-memory index operating, while an unusable `DATA_DIR` or config store is
still a hard boot error
([configuration](../ops/configure-env.md)).
