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

Task 10 defines the canonical successor format, `nimproxy-history/v1`; Task
11 owns publication and runtime use. Until then, the checked-in runtime still
uses its experimental history behavior and this codec does not open, read, or
write any history path.

Every canonical JSONL row begins, in this exact order, with
`format:"nimproxy-history"`, `v:1`, and `kind`. The complete field order is:

```json
{"format":"nimproxy-history","v":1,"kind":"boot","timestamp":1000,"boot_id":"boot-a","capacity":{"capacity_rpm":80,"enabled_keys":2,"key_rpms":[40,40]}}
{"format":"nimproxy-history","v":1,"kind":"sample","timestamp":1000,"boot_id":"boot-a","capacity":{"capacity_rpm":80,"enabled_keys":2,"key_rpms":[40,40]},"state":[{"kind":"counter","metric":"nimproxy_requests_total","labels":{"client":"redacted-client"},"value":1.0}]}
{"format":"nimproxy-history","v":1,"kind":"checkpoint","timestamp":1300,"boot_id":"boot-a","capacity":{"capacity_rpm":80,"enabled_keys":2,"key_rpms":[40,40]}}
```

`boot` and `checkpoint` carry no state. A `sample` has a complete state made
of `counter` or `gauge` entries, each ordered `kind`, `metric`, `labels`, and
`value`. The codec rejects non-finite values and duplicate semantic series
`(kind, metric, labels)`; it never normalizes or chooses a last writer.
Unknown non-state object fields are ignored, while any invalid state entry
rejects the whole sample. Reordering otherwise valid object fields is accepted
on decode and canonicalized by encode. Invalid UTF-8, invalid JSON, corrupt
v1 record/state, and unknown v1 record kind are distinct codec diagnostics.
An unknown `format` or `v` is explicitly `unsupported_format` or
`unsupported_version`, not corrupt-line recovery; Task 11 uses those errors to
refuse startup.

The canonical destination is `history-v1.jsonl`. The production runtime must
never read, rename, truncate, delete, or migrate experimental `history.jsonl`.
Timestamp stream ordering, startup refusal, checkpoint expansion, and recovery
semantics are intentionally outside this codec and belong to Tasks 11–12.

### Sanitized corpus evidence

On 2026-07-31, a read-only ephemeral container streamed the local
`nim-proxy_history` volume through a metadata-only analyzer: 235,966,850 bytes,
8,014 rows, one boot row and 8,013 legacy-sample rows, zero malformed rows,
and timestamps 1,783,077,758 through 1,785,479,582. The dominant cadence was
300 seconds (7,985 intervals), with 15 at 301 seconds and isolated restart or
timing gaps. The analyzer self-check first extracted a synthetic metric and
two synthetic label keys. The corpus extraction then found 45 repository metric
names and their label-key sets without printing values; its three structural
hashes were `ed3745ecb9bc2cc4` (7,503), `087debe8b773af5d` (510), and
`bebe937ef48eda06` (1). Of 8,013 payloads, 7,238 were nonempty and all 7,238
had distinct payload hashes; those nonempty payloads contributed 221,875,277
bytes. Sampling rows are idle-cadenced, but byte growth is traffic/state-driven
rather than empty-idle snapshots. Fixtures preserve only approved metric
identifiers, label keys, scalar shapes, and ordering with synthetic redacted
values.

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
