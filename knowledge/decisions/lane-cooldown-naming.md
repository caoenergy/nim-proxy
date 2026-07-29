---
type: Decision
title: Name the post-backoff lane state "cooldown", not "bench"
description: The sports idiom "bench" had no standard translation that wasn't already load-balancing vocabulary. Renamed to cooldown across code, metrics, and prose, accepting a history discontinuity on the renamed series.
tags: [terminology, metrics, pool, breaking-change]
timestamp: 2026-07-29T00:00:00Z
---

# Name the post-backoff lane state "cooldown", not "bench"

## Context

When the upstream returns 429/5xx/connect-error, the granting lane is taken out
of rotation until a deadline. The design-phase name for that state was
**bench** — a sports idiom, borrowed before the vocabulary settled.

It was never a good fit. "Bench" has no standard equivalent in load-balancing
vocabulary, so it had to be explained rather than recognized, and every
translation of the dashboard would have had to invent a term. The field holding
the deadline was already called `cooldown_until`, so the codebase was
internally inconsistent with itself: one concept, two names, and the better name
was already load-bearing.

## Options

1. **Keep `bench`.** Zero work, but the term stays unexplainable and the
   internal inconsistency with `cooldown_until` persists.
2. **`ejection`** — Envoy's word for the same idea (outlier detection ejects a
   host). Accurate, but implies removal from the pool rather than a timed
   pause, and the lane always comes back.
3. **`circuit break`** — widely understood, but carries half-open/closed state
   machinery this does not have. Claiming it would overstate the behavior.
4. **`quarantine`** — accurate but alarming for a routine 429.
5. **`cooldown`** — a timed pause before reuse. Already the field name, already
   standard, and it is exactly what the code does.

## Choice

**Option 5.** `bench` → `cooldown` across `src/`, `tests/`, `scripts/`,
`knowledge/`, and `README.md`. The Prometheus series `nimproxy_lane_benched_total` becomes
`nimproxy_lane_cooldown_total`; every other `nimproxy_*` series is unchanged.
The internal helper `proxy::bench` becomes `proxy::enter_cooldown`.

Prose says **lane cooldown** where ambiguity with
[dependency-update-cooldown](dependency-update-cooldown.md) is possible. The two
are different domains and do not conflict.

Historical CHANGELOG entries keep the old word — they describe what shipped at
the time, and rewriting them would be dishonest.

## Consequences

- **Breaking for anything scraping the old series.** Dashboards, alerts, or
  recording rules referencing `nimproxy_lane_benched_total` must be updated.
- **Retained history shows a gap.** `history.jsonl` persists the metric name
  verbatim, so points recorded before the upgrade stay under the old key while
  the dashboard queries the new one. Lane-cooldown charts are blank for
  pre-upgrade points and return to full fidelity one retention window
  (`history_days`) after upgrading.

  This was a deliberate choice over a read-time alias in `history.rs`. The
  alias would work, but it is a permanent compatibility shim carried forever to
  smooth a one-time bounded gap in a homelab tool — precisely the kind of
  prototype-phase residue this release exists to remove. The gap is visible,
  self-healing, and documented.
- The vocabulary is now recognizable without explanation, which is what makes
  the labels machine-translatable in the same release.
