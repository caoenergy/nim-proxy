# Reset-Aware Dashboard History Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:subagent-driven-development` (recommended) or
> `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every analytical dashboard tab use reset-aware retained history,
with a configurable 30-day default window, separately configurable retention,
live current-state refresh, and honest time-scope indicators.

**Architecture:** Extend the existing `History` component to parse each JSONL
snapshot once at startup into generic counter deltas and gauge samples. Keep
exact range aggregation on the server, poll current state through a lightweight
endpoint, and reuse the dashboard's existing renderers by adapting typed
rollups into their current sample shape. Keep `config.json` boot-loaded and
settings-managed; do not add file watching.

**Tech Stack:** Rust 1.87, Axum 0.8, Tokio, Serde/serde_json, the existing
`metrics` and `metrics-exporter-prometheus` crates, embedded HTML/CSS/vanilla
JavaScript, the existing real-binary e2e harness, and headless Google Chrome.

**Design:** `docs/superpowers/specs/2026-07-28-dashboard-history-rollups-design.md`

**Tracking:** GitHub issue
[#67](https://github.com/miztertea/nim-proxy/issues/67); the eventual PR body
must contain `Closes #67`.

## Global Constraints

- Both `history.days` and `dashboard.default_window_days` default to `30`.
- `history.days == 0 || history.days >= dashboard.default_window_days`.
- `dashboard.default_window_days >= 1`.
- `0 < dashboard.slo_target_percent <= 100`; the default is `99.9`.
- Availability numerator: successful 2xx terminal outcomes.
- Availability denominator: all terminal outcomes except 4xx responses and
  client disconnects.
- One selected window governs Overview, Models, Clients, Reliability, and
  Capacity; Settings is never time-filtered.
- Current gauges and current configuration are labeled **Now** and remain
  independent of a fixed historical window.
- Preserve the existing tabs, cards, charts, tables, navigation, dark palette,
  responsive behavior, and `esc()` protection at every dynamic `innerHTML`
  sink.
- Preserve existing v1 JSONL lines; new boot markers and samples use v2.
- Do not store prompts, responses, credentials, key fingerprints, owners, or
  usernames in telemetry.
- Do not add a dependency, database, service, sidecar index, frontend
  framework, file watcher, generalized SLO subsystem, or page-specific
  precomputation.
- `config.json` remains read from disk only at boot. Dashboard Settings writes
  remain the sole live update path: validate, atomically persist, then swap
  runtime state.
- Rate admission, pacing, dispatch, retry, authentication, and upstream request
  behavior remain unchanged.
- Run `cargo test`, `cargo fmt --check`, and
  `cargo clippy --all-targets -- -D warnings` before opening the PR.
- Run the strict load harness only if the final diff unexpectedly touches
  request admission, pacing, `src/dispatch.rs`, or rate-state code.

## Ponytail Minimality Ladder Audit

Apply the canonical [Ponytail Minimality
Ladder](https://app.notion.com/p/39a27ada618f8100babadb321a70de9b)
before each task and again during final diff review:

1. **Does it need to exist?** Reset normalization, a retained range contract,
   and separate current-state polling are required to prevent false zeroes and
   invalid cross-restart subtraction. Page-specific caches, a TSDB, and a
   general SLO platform are not required and are excluded.
2. **Already in the codebase?** Extend `src/history.rs`, `StoredConfig`,
   `settings::commit`, the authenticated Axum router, the dashboard's
   aggregation/render helpers, `tests/e2e.rs`, and the OKF pages.
3. **Stdlib?** Use `BufRead`, `BTreeMap`, `HashMap`, `RwLock`, atomics,
   `OpenOptions`, temporary-file rename, and filesystem metadata.
4. **Native platform?** Use Axum extractors/responses, Tokio's existing
   `spawn_blocking`, and Chrome's built-in headless screenshot support.
5. **Installed dependency?** Use Serde for JSONL/contracts, `getrandom` for the
   process epoch, and the installed Prometheus handle for current exposition.
6. **One local operation?** Extend `/api/settings/history` atomically instead
   of creating a second settings endpoint; increment one config revision in
   the existing commit pipeline.
7. **Minimum new machinery:** Two authenticated dashboard endpoints, one
   generic in-memory index inside `History`, versioned records, and a small
   browser adapter. No new Rust source module is introduced.

## File Map

- `src/config.rs` — stored dashboard settings, defaults, and shared validation.
- `src/settings.rs` — atomic settings update and role-filtered configuration
  response.
- `src/history.rs` — Prometheus parsing, boot/reset normalization, compact
  in-memory index, range rollups, transient live tail, retention, and JSONL
  persistence.
- `src/lib.rs` — application revisions, Prometheus handle, dashboard response
  handlers, sampler capacity capture, and authenticated routes.
- `src/dashboard.html` — typed-response adapter, global time controller,
  lightweight current polling, scope indicators, Settings controls, and SLO
  calculation.
- `tests/support/mod.rs` — configurable store fixture fields used by real-binary
  tests.
- `tests/e2e.rs` — settings, auth, restart/reset, rollup, live-tail, capacity,
  and retention contracts.
- `README.md`, `CHANGELOG.md` — operator-facing behavior.
- `knowledge/architecture/metrics-history.md`,
  `knowledge/architecture/dashboard.md`,
  `knowledge/ops/configure-env.md`,
  `knowledge/decisions/history-retention-days-not-size.md` — existing project
  memory that changes with the implementation.
- `knowledge/decisions/reset-aware-dashboard-history.md` — one new decision
  because the server-side rollup boundary and reset policy are durable.
- `knowledge/index.md`, `knowledge/decisions/index.md`, `knowledge/log.md` —
  graph links and the required ingest entry.
- `docs/assets/` — replace dashboard screenshots with synthetic-data captures
  only when the verified UI differs materially from the current images.

---

### Task 1: Stored dashboard settings and one atomic settings write

**Files:**

- Modify: `src/config.rs` (`StoredConfig`, defaults, `validate`, unit tests)
- Modify: `src/settings.rs` (`commit`, `api_config`, `HistoryReq`, `history`)
- Modify: `src/lib.rs` (`AppState`, state construction)
- Modify: `src/dashboard.html` (preserve the existing combined save during the
  incremental settings change)
- Modify: `tests/support/mod.rs` (`StoreOpts`)
- Modify: `tests/e2e.rs` (settings contract tests)

**Interfaces:**

- Produces:
  `config::DashboardCfg { default_window_days: u64, slo_target_percent: f64 }`.
- Produces:
  `POST /api/settings/history` body
  `{days, default_window_days, slo_target_percent}`.
- Produces `AppState::config_revision: AtomicU64`, initialized to `1` and
  incremented after a successful committed settings change.
- Preserves the current `StoredConfig.version == 1` and Serde-default upgrade
  path.

- [ ] **Step 1: Add failing config default and validation tests**

  Add these focused assertions to `config::tests`:

  ```rust
  #[test]
  fn dashboard_defaults_are_backward_compatible() {
      let sc: StoredConfig = serde_json::from_str(r#"{"version":1}"#).unwrap();
      assert_eq!(sc.history.days, 30);
      assert_eq!(sc.dashboard.default_window_days, 30);
      assert_eq!(sc.dashboard.slo_target_percent, 99.9);
      validate(&sc).unwrap();
  }

  #[test]
  fn dashboard_window_must_fit_finite_retention() {
      let mut sc = StoredConfig::default();
      sc.dashboard.default_window_days = 31;
      assert_eq!(
          validate(&sc).unwrap_err(),
          "history days must be 0 or at least default_window_days"
      );
      sc.history.days = 0;
      validate(&sc).unwrap();
  }

  #[test]
  fn dashboard_and_slo_bounds_are_validated() {
      let mut sc = StoredConfig::default();
      sc.dashboard.default_window_days = 0;
      assert_eq!(
          validate(&sc).unwrap_err(),
          "default_window_days must be >= 1"
      );
      sc.dashboard.default_window_days = 30;
      for target in [0.0, -1.0, 100.1, f64::NAN] {
          sc.dashboard.slo_target_percent = target;
          assert_eq!(
              validate(&sc).unwrap_err(),
              "slo_target_percent must be a number greater than 0 and at most 100"
          );
      }
  }
  ```

- [ ] **Step 2: Run the config tests and confirm the missing field fails**

  Run:

  ```bash
  cargo test config::tests::dashboard_ -- --nocapture
  ```

  Expected: compilation fails because `StoredConfig::dashboard` and
  `DashboardCfg` do not exist.

- [ ] **Step 3: Add `DashboardCfg` with Serde defaults and exact validation**

  Extend `StoredConfig` and its defaultable types:

  ```rust
  #[derive(Serialize, Deserialize, Clone, Debug)]
  pub struct DashboardCfg {
      #[serde(default = "default_dashboard_window_days")]
      pub default_window_days: u64,
      #[serde(default = "default_slo_target_percent")]
      pub slo_target_percent: f64,
  }

  impl Default for DashboardCfg {
      fn default() -> Self {
          Self {
              default_window_days: default_dashboard_window_days(),
              slo_target_percent: default_slo_target_percent(),
          }
      }
  }

  fn default_dashboard_window_days() -> u64 {
      30
  }

  fn default_slo_target_percent() -> f64 {
      99.9
  }
  ```

  Add `#[serde(default)] pub dashboard: DashboardCfg` to `StoredConfig`.
  Add the three exact validation branches before ownership validation:

  ```rust
  if sc.dashboard.default_window_days == 0 {
      return Err("default_window_days must be >= 1".into());
  }
  if sc.history.days != 0
      && sc.history.days < sc.dashboard.default_window_days
  {
      return Err("history days must be 0 or at least default_window_days".into());
  }
  if !sc.dashboard.slo_target_percent.is_finite()
      || !(0.0 < sc.dashboard.slo_target_percent
          && sc.dashboard.slo_target_percent <= 100.0)
  {
      return Err(
          "slo_target_percent must be a number greater than 0 and at most 100".into(),
      );
  }
  ```

- [ ] **Step 4: Run config tests and confirm they pass**

  Run:

  ```bash
  cargo test config::tests::dashboard_ -- --nocapture
  ```

  Expected: all three new tests pass.

- [ ] **Step 5: Add a failing e2e test for the atomic settings body**

  Replace the existing history-only POST in
  `pricing_and_history_settings_reflect_in_api_config` with:

  ```rust
  let (status, v) = post_json(
      &proxy,
      &root,
      "/api/settings/history",
      serde_json::json!({
          "days": 45,
          "default_window_days": 30,
          "slo_target_percent": 99.5
      }),
  )
  .await;
  assert_eq!(status, 200, "{v}");

  let cfg = api_config(&proxy, &root).await;
  assert_eq!(cfg["server"]["history"]["days"], 45);
  assert_eq!(cfg["server"]["dashboard"]["default_window_days"], 30);
  assert_eq!(cfg["server"]["dashboard"]["slo_target_percent"], 99.5);
  ```

  Add a second test that posts `days: 7, default_window_days: 30`, expects
  status `400`, and verifies all three prior values remain unchanged.

- [ ] **Step 6: Run the settings test and confirm the old request shape fails**

  Run:

  ```bash
  cargo test --test e2e pricing_and_history_settings_reflect_in_api_config -- --nocapture
  ```

  Expected: failure because the endpoint does not accept or publish dashboard
  fields.

- [ ] **Step 7: Extend the existing history settings endpoint**

  Use one request so the retention/window invariant is never transiently
  invalid:

  ```rust
  #[derive(Deserialize)]
  pub struct HistoryReq {
      days: u64,
      default_window_days: u64,
      slo_target_percent: f64,
  }

  admin_section!(
      history,
      HistoryReq,
      |cand: &mut StoredConfig, req: HistoryReq| {
          cand.history.days = req.days;
          cand.dashboard.default_window_days = req.default_window_days;
          cand.dashboard.slo_target_percent = req.slo_target_percent;
      }
  );
  ```

  Publish `sc.dashboard` beside `sc.history` in the admin-only `server`
  response. Add `config_revision: AtomicU64` to `AppState`, initialize it to
  `1`, and increment it with `fetch_add(1, Ordering::SeqCst)` only after
  persistence, runtime swap, pool rebuild, and retention retune succeed.

- [ ] **Step 8: Keep the existing dashboard save compatible**

  Until Task 8 splits the cards, change the current combined save to send the
  values it just fetched from `/api/config`:

  ```javascript
  await sPost('/api/settings/history', {
    days,
    default_window_days: sv.dashboard.default_window_days,
    slo_target_percent: sv.dashboard.slo_target_percent,
  });
  ```

  Do not add new controls in this task.

- [ ] **Step 9: Run focused and full tests**

  Run:

  ```bash
  cargo test config::tests::dashboard_ -- --nocapture
  cargo test --test e2e pricing_and_history_settings_reflect_in_api_config -- --nocapture
  cargo test
  ```

  Expected: all commands exit `0`.

- [ ] **Step 10: Commit the settings contract**

  ```bash
  git add src/config.rs src/settings.rs src/lib.rs src/dashboard.html tests/support/mod.rs tests/e2e.rs
  git commit -m "feat: configure dashboard history windows"
  ```

---

### Task 2: Parse Prometheus snapshots and normalize resets inside `History`

**Files:**

- Modify: `src/history.rs` (metric types, parser, normalization, unit tests)

**Interfaces:**

- Produces:
  `MetricKey { metric: String, labels: BTreeMap<String, String> }`.
- Produces:
  `MetricValue { metric: String, labels: BTreeMap<String, String>, value: f64 }`
  with `Serialize`.
- Produces:
  `ParsedSnapshot { counters: BTreeMap<MetricKey, f64>, gauges:
  BTreeMap<MetricKey, f64> }`.
- Produces:
  `normalize(previous, current, reset) -> NormalizedMetrics`.
- Uses `# TYPE` metadata first; suffix fallback treats `_total`, `_bucket`,
  `_count`, and `_sum` as counter-like.

- [ ] **Step 1: Write parser tests using real exporter shapes**

  Add an inline fixture and exact assertions:

  ```rust
  const SNAPSHOT: &str = r#"
  # TYPE nimproxy_requests_total counter
  nimproxy_requests_total{client="Mindmap",model="z-ai/glm-5.2",status="200"} 12
  # TYPE nimproxy_active_requests gauge
  nimproxy_active_requests 2
  # TYPE nimproxy_ttft_seconds histogram
  nimproxy_ttft_seconds_bucket{model="z-ai/glm-5.2",le="0.5"} 3
  nimproxy_ttft_seconds_bucket{model="z-ai/glm-5.2",le="+Inf"} 4
  nimproxy_ttft_seconds_sum{model="z-ai/glm-5.2"} 1.25
  nimproxy_ttft_seconds_count{model="z-ai/glm-5.2"} 4
  "#;

  #[test]
  fn parses_counter_gauge_and_histogram_series() {
      let parsed = parse_exposition(SNAPSHOT);
      assert_eq!(parsed.counters.len(), 5);
      assert_eq!(parsed.gauges.len(), 1);
      assert_eq!(parsed.skipped_lines, 0);
  }
  ```

  Add cases for escaped label quotes/backslashes/newlines, `NaN` values being
  skipped, malformed lines being counted, and a `_total` line without
  `# TYPE` using the suffix fallback.

- [ ] **Step 2: Run the parser test and confirm it fails to compile**

  Run:

  ```bash
  cargo test history::tests::parses_counter_gauge_and_histogram_series -- --nocapture
  ```

  Expected: compilation fails because `parse_exposition` is undefined.

- [ ] **Step 3: Implement the smallest local parser**

  Keep the parser private to `history.rs`. Reuse the dashboard parser's
  accepted metric-name and quoted-label grammar, but return stable
  `BTreeMap` labels. Parse `# TYPE <base> <counter|gauge|histogram>` into a
  type table. Reject non-finite sample values. Bound an individual input line
  at 1 MiB and a snapshot at 100,000 parsed series; increment diagnostics
  instead of panicking when either bound is exceeded.

  Use these concrete types:

  ```rust
  #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
  struct MetricKey {
      metric: String,
      labels: BTreeMap<String, String>,
  }

  #[derive(Clone, Debug, PartialEq, Serialize)]
  pub struct MetricValue {
      pub metric: String,
      pub labels: BTreeMap<String, String>,
      pub value: f64,
  }

  #[derive(Default)]
  struct ParsedSnapshot {
      counters: BTreeMap<MetricKey, f64>,
      gauges: BTreeMap<MetricKey, f64>,
      skipped_lines: usize,
  }
  ```

  Use `split_once(char::is_whitespace)` for sample/value separation, a
  character scanner for quoted labels, and `str::parse::<f64>()` for values.
  Do not add a Prometheus parser dependency.

- [ ] **Step 4: Run all history parser tests**

  Run:

  ```bash
  cargo test history::tests::parse -- --nocapture
  cargo test history::tests::parses_ -- --nocapture
  ```

  Expected: all parser tests pass.

- [ ] **Step 5: Write failing reset-normalization tests**

  Cover explicit boot changes, defensive decreases, new series, and gauges:

  ```rust
  #[test]
  fn explicit_boot_change_counts_new_process_values() {
      let first = parse_exposition(
          "# TYPE requests_total counter\nrequests_total 100\n"
      );
      let second = parse_exposition(
          "# TYPE requests_total counter\nrequests_total 7\n"
      );
      let normalized = normalize(Some(&first), &second, true);
      assert_eq!(metric(&normalized.deltas, "requests_total"), 7.0);
  }

  #[test]
  fn one_legacy_counter_decrease_resets_the_snapshot_epoch() {
      let first = parse_exposition(
          "# TYPE a_total counter\na_total 100\n# TYPE b_total counter\nb_total 20\n"
      );
      let second = parse_exposition(
          "# TYPE a_total counter\na_total 3\n# TYPE b_total counter\nb_total 25\n"
      );
      let normalized = normalize(Some(&first), &second, false);
      assert!(normalized.inferred_reset);
      assert_eq!(metric(&normalized.deltas, "a_total"), 3.0);
      assert_eq!(metric(&normalized.deltas, "b_total"), 25.0);
  }
  ```

  Add a no-reset case (`100 → 107` produces `7`), a newly appearing counter
  case (current value becomes its delta), and a gauge case (current value is
  copied, never subtracted).

- [ ] **Step 6: Run normalization tests and confirm failure**

  Run:

  ```bash
  cargo test history::tests::explicit_boot_change_counts_new_process_values -- --nocapture
  cargo test history::tests::one_legacy_counter_decrease_resets_the_snapshot_epoch -- --nocapture
  ```

  Expected: compilation fails because `normalize` is undefined.

- [ ] **Step 7: Implement reset-aware normalization**

  Use:

  ```rust
  #[derive(Default)]
  struct NormalizedMetrics {
      deltas: BTreeMap<MetricKey, f64>,
      gauges: BTreeMap<MetricKey, f64>,
      inferred_reset: bool,
  }
  ```

  Set `inferred_reset` when `reset` is false and any counter present in both
  snapshots decreases. When `reset || inferred_reset`, every current counter
  value is its delta. Otherwise subtract the previous value when present and
  use the current value for a newly appearing series. Copy all current gauges.
  Clamp only negative floating-point noise in `[-f64::EPSILON, 0.0)` to zero;
  a real decrease triggers reset rather than silent clamping.

- [ ] **Step 8: Run the complete `history` unit suite**

  Run:

  ```bash
  cargo test history::tests -- --nocapture
  ```

  Expected: all parser, normalization, and pre-existing history tests pass.

- [ ] **Step 9: Commit parsing and normalization**

  ```bash
  git add src/history.rs
  git commit -m "feat: normalize persisted metric resets"
  ```

---

### Task 3: Versioned boot-aware records and the startup index

**Files:**

- Modify: `src/history.rs` (`History`, load, append, status, unit tests)
- Modify: `src/lib.rs` (pool/history construction order and sampler)
- Modify: `tests/e2e.rs` (v2 persistence assertions)

**Interfaces:**

- Produces serializable
  `CapacitySnapshot { enabled_lanes, rpms, capacity_rpm }`.
- Produces v2 `boot` and `sample` JSONL records; v1 `{t,m}` remains readable.
- Produces `History::load(dir, days, initial_capacity)`; `History` generates
  and owns the process boot ID.
- Produces `History::append(t, snapshot, capacity)`.
- Produces `History::revision() -> u64` and
  `History::status() -> HistoryStatus`.
- Startup index construction completes before `/health` can report ready.

- [ ] **Step 1: Add failing v1/v2 load tests**

  Write a temporary JSONL file containing:

  ```json
  {"t":10,"m":"# TYPE requests_total counter\nrequests_total 5\n"}
  {"t":20,"m":"# TYPE requests_total counter\nrequests_total 8\n"}
  {"v":2,"t":21,"boot":"boot-b","kind":"boot","capacity":{"enabled_lanes":2,"rpms":[40,30],"capacity_rpm":70}}
  {"v":2,"t":30,"boot":"boot-b","capacity":{"enabled_lanes":2,"rpms":[40,30],"capacity_rpm":70},"m":"# TYPE requests_total counter\nrequests_total 2\n"}
  ```

  Assert that three metric samples load, the boot marker is not a metric
  sample, the combined request delta is `5 + 3 + 2 == 10`, the v2 capacity is
  preserved, and the explicit `boot-b` transition is not counted as a legacy
  inferred reset.

- [ ] **Step 2: Run the new load test and confirm the old loader fails**

  Run:

  ```bash
  cargo test history::tests::load_reads_v1_and_v2_boot_epochs -- --nocapture
  ```

  Expected: failure because the old loader discards boot/capacity metadata and
  stores raw strings.

- [ ] **Step 3: Replace raw in-memory points with compact indexed points**

  Define:

  ```rust
  #[derive(Clone, Debug, Deserialize, Serialize)]
  pub struct CapacitySnapshot {
      pub enabled_lanes: usize,
      pub rpms: Vec<usize>,
      pub capacity_rpm: usize,
  }

  struct IndexedPoint {
      t: u64,
      deltas: BTreeMap<MetricKey, f64>,
      gauges: BTreeMap<MetricKey, f64>,
      capacity: Option<CapacitySnapshot>,
  }

  struct HistoryInner {
      points: Vec<IndexedPoint>,
      revision: u64,
      available_from: Option<u64>,
      available_to: Option<u64>,
      diagnostics: HistoryDiagnostics,
      last_parsed: Option<ParsedSnapshot>,
      last_sample_boot: Option<String>,
  }

  #[derive(Clone, Debug, Default, PartialEq, Serialize)]
  pub struct HistoryDiagnostics {
      pub valid_samples: usize,
      pub skipped_records: usize,
      pub skipped_metric_lines: usize,
      pub normalized_series: usize,
      pub legacy_resets_inferred: usize,
  }

  #[derive(Clone, Debug, Serialize)]
  pub struct HistoryStatus {
      pub available_from: Option<u64>,
      pub available_to: Option<u64>,
      pub file_bytes: u64,
      pub compaction_pending: bool,
  }
  ```

  Keep only parsed deltas/gauges/capacity in `HistoryInner`; do not retain
  repeated Prometheus text in the final implementation. For this incremental
  task, keep the pre-existing raw `points` field only until Task 7 replaces
  `/api/history`; delete it in the same Task 7 commit so the completed branch
  has one compact in-memory representation. Parse input lines in timestamp
  order, carrying `last_parsed` and `last_sample_boot`. Store the new process
  boot ID separately on `History`; a startup boot marker must not relabel the
  preceding process's last parsed sample. For legacy data, one counter decrease
  resets the whole snapshot epoch. A legacy all-counters-empty snapshot
  followed by counter reappearance also starts a new inferred epoch.

- [ ] **Step 4: Generate and persist one process epoch**

  Add `new_boot_id() -> String` using the existing `getrandom` crate with
  16 random bytes encoded as 32 lowercase hex characters. In
  `History::load`, finish parsing/indexing first, then append one v2 boot
  marker with the current capacity before returning.

  Serialize boot and sample lines through typed Serde structs:

  ```rust
  #[derive(Serialize)]
  struct BootRecord<'a> {
      v: u8,
      t: u64,
      boot: &'a str,
      kind: &'static str,
      capacity: &'a CapacitySnapshot,
  }
  ```

  Use `v: 2` and `kind: "boot"`. Sample records contain `v`, `t`, `boot`,
  `capacity`, and `m`.

- [ ] **Step 5: Reorder startup and capture contemporaneous capacity**

  Build `PoolHandle` before `History::load`. Derive capacity with:

  ```rust
  fn capacity_snapshot(pool: &Pool) -> history::CapacitySnapshot {
      history::CapacitySnapshot {
          enabled_lanes: pool.len(),
          rpms: pool.rpms(),
          capacity_rpm: pool.capacity_rpm(),
      }
  }
  ```

  Pass the initial snapshot to `load`. In the sampler, read one pool snapshot
  and call:

  ```rust
  hist.append(
      unix_now(),
      &prom.render(),
      capacity_snapshot(&pool.read().unwrap()),
  );
  ```

  Log source bytes, valid samples, skipped lines, normalized series, inferred
  resets, and elapsed indexing time before binding the listener.

- [ ] **Step 6: Update the e2e persistence test before changing its route**

  In `history_records_snapshots_and_survives_restart`, parse JSONL lines and
  assert:

  ```rust
  assert!(records.iter().any(|v| v["kind"] == "boot"));
  assert!(records.iter().any(|v| {
      v["v"] == 2
          && v["boot"].is_string()
          && v["capacity"]["capacity_rpm"] == 120
          && v["m"].as_str().is_some_and(|m| m.contains("nimproxy"))
  }));
  ```

- [ ] **Step 7: Run history and persistence tests**

  Run:

  ```bash
  cargo test history::tests -- --nocapture
  cargo test --test e2e history_records_snapshots_and_survives_restart -- --nocapture
  ```

  Expected: both commands exit `0`; JSONL includes one boot marker per
  process and v2 metric samples.

- [ ] **Step 8: Commit the startup index**

  ```bash
  git add src/history.rs src/lib.rs tests/e2e.rs
  git commit -m "feat: index history at server startup"
  ```

---

### Task 4: Exact range rollups and transient current tail

**Files:**

- Modify: `src/history.rs` (rollup types, query, tail, unit tests)

**Interfaces:**

- Produces `History::rollup(from, to, points) -> Rollup`.
- Produces
  `History::current(t, current_exposition) -> CurrentMetrics`, containing both
  typed current values and a reset-aware `Tail`.
- `Rollup.totals` is exact across all indexed deltas.
- `Rollup.points` is independently bucketed and never drives totals.
- A delta belongs to `(previous_sample_time, sample_time]` and is selected
  when `from < sample_time <= to`.
- Point budget clamps to `2..=1000`.
- Tail carries `base_history_revision` and replaces the prior browser tail.

- [ ] **Step 1: Write a reusable indexed-history test fixture**

  Add a private `history_with_points` helper that appends snapshots at
  `t=100, 200, 300` with request totals `10, 25, 40`, prompt tokens grouped
  across two models, histogram buckets, one gauge, and capacity changes
  `40 → 80 → 80`.

  Keep the fixture fully inline so no new fixture format or loader is created.

- [ ] **Step 2: Add failing exactness and boundary tests**

  Add:

  ```rust
  #[test]
  fn rollup_totals_do_not_change_with_point_budget() {
      let h = history_with_points();
      let coarse = h.rollup(99, 300, 2);
      let fine = h.rollup(99, 300, 1000);
      assert_eq!(coarse.totals, fine.totals);
      assert_eq!(value(&coarse.totals, "requests_total"), 40.0);
      assert!(coarse.points.len() <= 2);
  }

  #[test]
  fn rollup_uses_open_closed_sample_boundaries() {
      let h = history_with_points();
      let r = h.rollup(100, 200, 20);
      assert_eq!(value(&r.totals, "requests_total"), 15.0);
  }
  ```

  Add tests for grouped label preservation, latest gauges, histogram bucket
  totals, empty windows, effective/available boundaries, and time-weighted
  capacity.

- [ ] **Step 3: Run the rollup tests and confirm failure**

  Run:

  ```bash
  cargo test history::tests::rollup_ -- --nocapture
  ```

  Expected: compilation fails because `History::rollup` is undefined.

- [ ] **Step 4: Implement exact totals and independent bucket points**

  Define serializable values:

  ```rust
  #[derive(Clone, Debug, PartialEq, Serialize)]
  pub struct CapacityRollup {
      pub average_rpm: f64,
      pub latest_rpms: Vec<usize>,
  }

  #[derive(Clone, Debug, PartialEq, Serialize)]
  pub struct RollupPoint {
      pub from: u64,
      pub to: u64,
      pub duration_seconds: u64,
      pub values: Vec<MetricValue>,
      pub capacity: Option<CapacityRollup>,
  }

  #[derive(Clone, Debug, PartialEq, Serialize)]
  pub struct Rollup {
      pub available_from: Option<u64>,
      pub available_to: Option<u64>,
      pub effective_from: Option<u64>,
      pub effective_to: Option<u64>,
      pub totals: Vec<MetricValue>,
      pub latest: Vec<MetricValue>,
      pub points: Vec<RollupPoint>,
      pub diagnostics: HistoryDiagnostics,
      pub history_revision: u64,
  }
  ```

  Binary-search the first point with `t > from`, scan through `t <= to`, sum
  every counter-like delta into one `BTreeMap`, retain the last gauge value,
  and assign each point to:

  ```text
  bucket = min(points - 1, ((sample_time - from) * points) / (to - from))
  ```

  Sum deltas inside buckets and take the last gauge in each bucket. Compute
  capacity using overlap duration and the capacity attached to each ending
  interval. Convert maps to sorted `Vec<MetricValue>` only at the response
  boundary.

- [ ] **Step 5: Add failing tail tests**

  Verify that a current registry value of `43` after indexed value `40`
  returns a tail delta of `3`, a new process boot value of `2` returns `2`,
  gauges are current values, and `base_history_revision` matches the index.

- [ ] **Step 6: Implement `History::current`**

  Define:

  ```rust
  #[derive(Clone, Debug, Serialize)]
  pub struct Tail {
      pub base_history_revision: u64,
      pub from: Option<u64>,
      pub to: u64,
      pub totals: Vec<MetricValue>,
  }

  #[derive(Clone, Debug, Serialize)]
  pub struct CurrentMetrics {
      pub metrics: Vec<MetricValue>,
      pub tail: Tail,
  }
  ```

  Parse the current exposition, normalize it against the current boot's last
  indexed parsed snapshot, and return both all current typed values and the
  resulting counter deltas. If `last_sample_boot` differs from the process
  boot ID, treat current counters as the first values of the new process. Do
  not mutate the index and do not accumulate successive polls.

- [ ] **Step 7: Run the complete rollup/tail test set**

  Run:

  ```bash
  cargo test history::tests::rollup_ -- --nocapture
  cargo test history::tests::tail_ -- --nocapture
  cargo test history::tests -- --nocapture
  ```

  Expected: all commands exit `0`.

- [ ] **Step 8: Commit query semantics**

  ```bash
  git add src/history.rs
  git commit -m "feat: roll up exact dashboard windows"
  ```

---

### Task 5: Authenticated dashboard range and current-state contracts

**Files:**

- Modify: `src/lib.rs` (`AppState`, handlers, routes)
- Modify: `src/settings.rs` (`api_config` history status)
- Modify: `tests/e2e.rs` (endpoint and authorization tests)

**Interfaces:**

- Produces authenticated
  `GET /api/dashboard?from=<unix>&to=<unix>&points=<n>`.
- Produces authenticated `GET /api/dashboard/now`.
- Coexists temporarily with `GET /api/history` and
  `GET /dash/config.json`; Task 7 removes them together with their final
  browser callers.
- `AppState` stores the existing `PrometheusHandle` so both `/metrics` and the
  current-state handler render the same registry.
- Error body for invalid bounds uses code `invalid_time_window`.

- [ ] **Step 1: Add failing authenticated range-contract e2e tests**

  After generating traffic and waiting for samples, request:

  ```rust
  let response = client()
      .get(proxy.url("/api/dashboard?from=1&to=4102444800&points=24"))
      .header("cookie", &cookie)
      .send()
      .await
      .unwrap();
  assert_eq!(response.status(), 200);
  let body: serde_json::Value = response.json().await.unwrap();
  assert!(body["history_revision"].as_u64().is_some());
  assert!(body["window"]["available_from"].as_u64().is_some());
  assert!(body["totals"].as_array().is_some());
  assert!(body["latest"].as_array().is_some());
  assert!(body["points"].as_array().is_some());
  ```

  Add assertions that `from >= to` returns `400` with
  `error.code == "invalid_time_window"`, omitted bounds use the configured
  default window, and no cookie receives the existing login redirect/401
  posture.

- [ ] **Step 2: Add a failing current-state contract test**

  Request `/api/dashboard/now` and assert exact fields:

  ```rust
  assert_eq!(body["lanes"], 3);
  assert_eq!(body["rpms"], serde_json::json!([40, 40, 40]));
  assert_eq!(body["capacity_rpm"], 120);
  assert_eq!(body["default_window_days"], 30);
  assert_eq!(body["retention_days"], 30);
  assert_eq!(body["slo_target_percent"], 99.9);
  assert!(body["history_revision"].as_u64().is_some());
  assert!(body["config_revision"].as_u64().is_some());
  assert!(body["tail"]["totals"].as_array().is_some());
  assert!(body["metrics"].as_array().is_some());
  ```

- [ ] **Step 3: Run endpoint tests and confirm 404 failures**

  Run:

  ```bash
  cargo test --test e2e dashboard_range_contract -- --nocapture
  cargo test --test e2e dashboard_now_contract -- --nocapture
  ```

  Expected: both fail because the routes do not exist.

- [ ] **Step 4: Store the Prometheus handle and assemble responses**

  Add `prometheus: metrics_exporter_prometheus::PrometheusHandle` to
  `AppState`. Replace the closure-captured `/metrics` handle with an Axum
  handler that calls `state.prometheus.render()`.

  Add `DashboardQuery` with optional `from`, `to`, and `points`. The range
  handler:

  1. snapshots `StoredConfig`;
  2. defaults to `[now - default_window_days * 86400, now]`;
  3. rejects `from >= to`;
  4. clamps `points` to `2..=1000`;
  5. calls `history.rollup`;
  6. adds requested/effective/available bounds and configured window/retention.

  The current handler snapshots config and pool once, parses current
  exposition through `History::current`, and returns version, uptime, prices,
  auth mode, current capacity, configured history/dashboard values, both
  revisions, history bounds, `CurrentMetrics.metrics`, and
  `CurrentMetrics.tail`.

- [ ] **Step 5: Add authenticated routes**

  In the protected router use:

  ```rust
  .route("/api/dashboard", get(api_dashboard))
  .route("/api/dashboard/now", get(api_dashboard_now))
  .route("/metrics", get(metrics_text))
  ```

  Keep `/api/history` and `/dash/config.json` for the still-unmigrated
  dashboard in this intermediate commit. Do not alter the session middleware
  or `/metrics` authentication.

- [ ] **Step 6: Publish retention status to admin Settings**

  Extend admin `server.history` with read-only values from
  `History::status()`:

  ```json
  {
    "days": 30,
    "available_from": 1783077758,
    "file_bytes": 246415360,
    "compaction_pending": false
  }
  ```

  Keep non-admin responses unchanged; no filesystem path is exposed.

- [ ] **Step 7: Run endpoint, auth, and full tests**

  Run:

  ```bash
  cargo test --test e2e dashboard_range_contract -- --nocapture
  cargo test --test e2e dashboard_now_contract -- --nocapture
  cargo test --test e2e operator_surface_always_requires_auth -- --nocapture
  cargo test
  ```

  Expected: all commands exit `0`.

- [ ] **Step 8: Commit the HTTP contracts**

  ```bash
  git add src/lib.rs src/settings.rs tests/e2e.rs
  git commit -m "feat: serve typed dashboard history"
  ```

---

### Task 6: Immediate retention pruning and atomic background compaction

**Files:**

- Modify: `src/history.rs` (retention lifecycle and tests)
- Modify: `src/settings.rs` (`commit` side effect)
- Modify: `tests/e2e.rs` (retention behavior)

**Interfaces:**

- Replaces `set_days` with
  `History::reconfigure_retention(self: &Arc<Self>, days, now)`.
- Queries honor the new cutoff before the settings response returns.
- File compaction runs through the existing Tokio runtime with
  `spawn_blocking`.
- Appends and compaction serialize through one filesystem mutex.
- A compacted file retains the last pre-cutoff metric snapshot as a hidden
  reset baseline plus the latest pre-cutoff boot marker and every record at or
  after the cutoff.

- [ ] **Step 1: Write failing immediate-prune tests**

  Build a history with points older/newer than a one-day cutoff, call
  `reconfigure_retention(1, 200_000)`, and immediately assert that
  `rollup(0, u64::MAX, 100)` excludes expired deltas and
  `status().available_from` is the first retained timestamp.

- [ ] **Step 2: Write failing durable-compaction tests**

  Use a temp JSONL file with:

  - one v1 sample before cutoff;
  - one v2 sample before cutoff;
  - one boot marker immediately before cutoff;
  - two samples after cutoff.

  Await compaction completion with this bounded pattern:

  ```rust
  let deadline = std::time::Instant::now() + Duration::from_secs(5);
  while h.status().compaction_pending {
      assert!(
          std::time::Instant::now() < deadline,
          "history compaction did not finish"
      );
      tokio::time::sleep(Duration::from_millis(20)).await;
  }
  ```

  Assert the file contains the last pre-cutoff sample, the relevant boot
  marker, and both retained samples, but not older superseded lines. Reload the
  compacted file and assert retained totals are unchanged.

- [ ] **Step 3: Run retention tests and confirm failure**

  Run:

  ```bash
  cargo test history::tests::retention_ -- --nocapture
  cargo test history::tests::compaction_ -- --nocapture
  ```

  Expected: failures because pruning still waits for append and rewriting is
  neither background nor atomic.

- [ ] **Step 4: Implement immediate in-memory pruning**

  Under the history write lock:

  1. store the new `days`;
  2. compute `cutoff = now.saturating_sub(days * 86400)` when finite;
  3. remove indexed points with `t < cutoff`;
  4. update available bounds;
  5. increment `history_revision` when the visible set changes.

  For `days == 0`, skip pruning and compaction.

- [ ] **Step 5: Implement serialized atomic compaction**

  Add one filesystem `Mutex<()>` and `AtomicBool` flags for pending/running.
  In `spawn_blocking`:

  1. lock filesystem IO;
  2. stream `history.jsonl` with `BufRead`;
  3. retain only the hidden baseline/boot records and in-cutoff records;
  4. remove a stale `history.jsonl.tmp`, then recreate it with `create_new`,
     owner-only `0600`, and `sync_all`;
  5. rename over `history.jsonl`;
  6. fsync the containing directory on Unix;
  7. increment `history_revision` and clear pending only on success.

  `append` takes the same filesystem lock. If pending remains true after a
  failed compaction, the next append schedules one retry. Never replace the
  durable file after a write or fsync failure.

- [ ] **Step 6: Invoke retention through the existing commit pipeline**

  After successful config persistence/runtime swap/pool rebuild call:

  ```rust
  state.history.clone().reconfigure_retention(
      candidate.history.days,
      crate::unix_now(),
  );
  ```

  Keep the whole candidate atomic: validation or config-store failure performs
  no prune.

- [ ] **Step 7: Add an e2e test for query and disk behavior**

  Pre-populate history, lower retention through the authenticated settings
  endpoint, assert `/api/dashboard` immediately excludes expired activity,
  wait for `compaction_pending == false` in `/api/config`, restart with the
  same data directory, and assert the same totals/bounds.

- [ ] **Step 8: Run retention and full tests**

  Run:

  ```bash
  cargo test history::tests::retention_ -- --nocapture
  cargo test history::tests::compaction_ -- --nocapture
  cargo test --test e2e retention_change_prunes_queries_and_disk -- --nocapture
  cargo test
  ```

  Expected: all commands exit `0`.

- [ ] **Step 9: Commit retention behavior**

  ```bash
  git add src/history.rs src/settings.rs tests/e2e.rs
  git commit -m "feat: compact retained history atomically"
  ```

---

### Task 7: Reuse the dashboard renderers with a typed rollup adapter

**Files:**

- Modify: `src/history.rs` (remove temporary raw compatibility storage)
- Modify: `src/lib.rs` (remove retired handlers/routes)
- Modify: `src/dashboard.html` (state, adapter, polling, time controls)
- Modify: `tests/e2e.rs` (served-dashboard contract assertions)

**Interfaces:**

- Replaces browser parsing of `/metrics` and `/api/history` with
  `/api/dashboard` and `/api/dashboard/now`.
- Keeps the existing `samples: [{t, rows}]` renderer input by converting
  server deltas into synthetic cumulative samples.
- Maintains `nowRows` separately from selected-window samples.
- Relative presets and **All retained** follow now; custom range and pause are
  fixed.
- The tail overlay is accepted only when its
  `base_history_revision == range.history_revision`.

- [ ] **Step 1: Add failing static dashboard contract assertions**

  Extend `dashboard_and_config_are_served_to_authenticated_users` to assert the
  HTML contains `/api/dashboard/now`, `data-range="default"`, and
  `data-range="all-retained"`, and does not contain `fetch('/metrics')`,
  `/api/history?`, or `/dash/config.json`.

- [ ] **Step 2: Run the dashboard contract test and confirm failure**

  Run:

  ```bash
  cargo test --test e2e dashboard_and_config_are_served_to_authenticated_users -- --nocapture
  ```

  Expected: assertion failure against the old data-source strings.

- [ ] **Step 3: Add the typed-row adapter without changing render layout**

  Keep `parseProm` only if `/metrics` formatting helpers still use it; remove
  it when no caller remains. Add:

  ```javascript
  const rowKey = r => r.metric + '\0' +
    Object.entries(r.labels || {}).sort(([a], [b]) => a.localeCompare(b))
      .map(([k, v]) => `${k}=${v}`).join('\0');

  const asRow = r => ({
    name: r.metric,
    labels: r.labels || {},
    value: +r.value,
  });
  ```

  Implement `rangeSamples(range, tail)`:

  1. create a zero counter baseline at `range.window.effective_from`;
  2. identify gauge keys from `range.latest`;
  3. walk `range.points` in time order;
  4. add counter deltas to a cumulative map;
  5. replace gauge values with each bucket's last value;
  6. overwrite the final cumulative counter map from `range.totals`, making
     scalar cards/rankings consume the explicit exact contract rather than
     chart buckets;
  7. append one synthetic tail sample by adding `tail.totals`;
  8. return the existing `{t: milliseconds, rows}` shape.

  Never accumulate one tail poll onto another; rebuild from the cached range
  plus the newest tail.

- [ ] **Step 4: Separate current rows from selected-window rows**

  Add:

  ```javascript
  let rangeData = null;
  let nowData = null;
  let nowRows = [];
  let mode = {
    kind: 'following',
    preset: 'default',
    paused: false,
    from: 0,
    to: 0,
  };
  ```

  In `render`, use the selected samples for totals, trends, historical
  histograms, rankings, and heatmaps. Use `nowRows` for active requests, queue
  depth, current governor limits/in-flight gauges, and other **Now** widgets.
  Replace the old `mode.kind === "live"` lifetime branches: every counter
  helper now computes selected-window values from the zero baseline.

- [ ] **Step 5: Implement range and lightweight current fetches**

  Add:

  ```javascript
  async function fetchRange(from, to) {
    const url = `/api/dashboard?from=${from}&to=${to}&points=288`;
    const response = await fetch(url);
    if (!response.ok) throw new Error(`range ${response.status}`);
    rangeData = await response.json();
  }

  async function pollNow() {
    const response = await fetch('/api/dashboard/now');
    if (!response.ok) throw new Error(`now ${response.status}`);
    const next = await response.json();
    const historyChanged =
      rangeData && next.history_revision !== rangeData.history_revision;
    nowData = next;
    nowRows = next.metrics.map(asRow);
    if (historyChanged) await refreshSelectedRange();
    rebuildSamples();
    render();
  }
  ```

  If `config_revision` changes, update version, capacity, pricing, auth,
  default window, retention, and SLO from `nowData`. If the active preset is
  `default`, recompute its bounds before the next range fetch.

- [ ] **Step 6: Replace the top-right controller semantics**

  Replace the old **Live** range pill with **Default · Nd**, retain `1h`,
  `6h`, `24h`, `7d`, and `30d`, and add **All retained** before Custom.

  Relative/default/all-retained selections set `kind: "following"` and use
  `nowData.sampled_at` as `to`. Custom applies `kind: "fixed"`. Clicking the
  sidebar live control while following freezes `from`/`to` at the last sample;
  clicking it again resumes the same preset.

  Render exact scope text:

  ```text
  ● Following now    Default · 30d    Jul 3 – Jul 28
  ○ Fixed range      Jul 13, 00:00 – Jul 14, 00:00
  ```

  For partial history, display the effective dates supplied by the server.
  Empty windows say `No traffic in selected window`.

- [ ] **Step 7: Preserve tab and Settings behavior**

  Keep one selected window while switching analytical tabs. Continue hiding
  the range controller on Settings. Current-state polling continues while a
  fixed range is displayed, but it updates only **Now** widgets and the
  sidebar/header metadata.

- [ ] **Step 8: Remove the retired browser transports**

  Remove `/api/history`, `/dash/config.json`, `api_history`, and
  `dash_config` from `src/lib.rs`. Delete the temporary raw `points` field and
  its compatibility range method from `History`; only the normalized index
  remains in memory. Replace the old unauthenticated `/api/history == 401`
  assertion in `operator_surface_always_requires_auth` with `401` assertions
  for both new dashboard endpoints. Add authenticated requests asserting each
  retired route returns `404`.

- [ ] **Step 9: Run the static contract and full tests**

  Run:

  ```bash
  cargo test --test e2e dashboard_and_config_are_served_to_authenticated_users -- --nocapture
  cargo test
  ```

  Expected: all commands exit `0`.

- [ ] **Step 10: Commit the dashboard data flow**

  ```bash
  git add src/history.rs src/lib.rs src/dashboard.html tests/e2e.rs
  git commit -m "feat: apply one history window across dashboards"
  ```

---

### Task 8: Settings presentation, SLO semantics, and historical capacity

**Files:**

- Modify: `src/dashboard.html` (Server settings, availability, scope labels,
  capacity calculations)
- Modify: `tests/e2e.rs` (HTML contract and live config refresh)

**Interfaces:**

- Settings → Server has separate **Pricing** and **History & dashboard** cards.
- History save sends all three fields in one `/api/settings/history` request.
- SLO uses `nowData.slo_target_percent / 100`.
- Current capacity comes from `nowData`; historical utilization comes from
  each range point's contemporaneous capacity.

- [ ] **Step 1: Add failing Settings markup contract assertions**

  Assert the served HTML contains `History &amp; dashboard`,
  `sv-default-days`, `sv-retention-days`, `sv-slo`, and
  `/api/settings/history`, while `Pricing &amp; history` and
  `const SLO = 0.999` are absent.

- [ ] **Step 2: Run the markup contract test and confirm failure**

  Run:

  ```bash
  cargo test --test e2e dashboard_history_settings_markup -- --nocapture
  ```

  Expected: failure against the old combined card and hardcoded SLO.

- [ ] **Step 3: Split Pricing from History & dashboard**

  Keep the existing Pricing card/save handler for `ref_price_in` and
  `ref_price_out`. Add a second card containing:

  - default dashboard window, minimum `1`, in days;
  - retention, minimum `0`, in days, with `0 = unlimited`;
  - SLO target, greater than `0` and at most `100`, step `0.1`;
  - earliest retained snapshot formatted in local time;
  - current history file size formatted with `Intl.NumberFormat`;
  - a small `compaction pending` note only when true.

  The History save handler validates finite numeric values and sends:

  ```javascript
  await sPost('/api/settings/history', {
    days: retention,
    default_window_days: defaultDays,
    slo_target_percent: slo,
  });
  ```

  Show the server's exact 400 message on invariant failures.

- [ ] **Step 4: Replace hardcoded availability**

  Compute:

  ```javascript
  const success = [...statusG.entries()]
    .filter(([status]) => /^2\d\d$/.test(status))
    .reduce((n, [, value]) => n + value, 0);
  const eligible = [...statusG.entries()]
    .filter(([status]) => status !== 'disconnect' && !/^4\d\d$/.test(status))
    .reduce((n, [, value]) => n + value, 0);
  const slo = (nowData?.slo_target_percent ?? 99.9) / 100;
  const availability = eligible ? success / eligible : NaN;
  ```

  Use `availability` only for the SLO hero and error-budget calculation.
  Keep the full outcome taxonomy, including 4xx and disconnects, in diagnostic
  charts/tables.

- [ ] **Step 5: Use historical and current capacity in their proper scopes**

  Build a capacity series from `rangeData.points[].capacity.average_rpm`.
  Historical utilization, average, peak comparisons, and provisioning use
  contemporaneous bucket capacity. Active load, enabled lane count, RPM
  budgets, and aggregate capacity use `nowData` and display a small **Now**
  indicator. Legacy intervals without capacity metadata render capacity as
  unavailable; never substitute today's configuration for an unknown
  historical interval.

  Do not label numeric lane slots as durable key identities.

- [ ] **Step 6: Add an e2e config-refresh contract**

  Fetch `/api/dashboard/now`, change one lane RPM through the existing
  authenticated NIM-key settings endpoint, fetch `/api/dashboard/now` again,
  and assert:

  ```rust
  assert!(
      after["config_revision"].as_u64().unwrap()
          > before["config_revision"].as_u64().unwrap()
  );
  assert_ne!(after["capacity_rpm"], before["capacity_rpm"]);
  ```

  Also assert `history_revision` does not change solely because current
  configuration changes.

- [ ] **Step 7: Run settings, SLO, and refresh tests**

  Run:

  ```bash
  cargo test --test e2e dashboard_history_settings_markup -- --nocapture
  cargo test --test e2e dashboard_now_refreshes_after_settings_change -- --nocapture
  cargo test
  ```

  Expected: all commands exit `0`.

- [ ] **Step 8: Commit the remaining UI semantics**

  ```bash
  git add src/dashboard.html tests/e2e.rs
  git commit -m "feat: expose dashboard history settings"
  ```

---

### Task 9: Restart-aware end-to-end acceptance

**Files:**

- Modify: `tests/support/mod.rs` (fixture helpers only when reused by two tests)
- Modify: `tests/e2e.rs` (acceptance coverage)

**Interfaces:**

- Proves one range combines traffic from multiple process epochs.
- Proves exact totals are independent of `points`.
- Proves future v2 epochs are exact and legacy v1 decreases are inferred.
- Proves current tail is not double-counted when a periodic sample advances
  `history_revision`.

- [ ] **Step 1: Add a failing two-boot traffic test**

  Using `HISTORY_SAMPLE_SECS=1`:

  1. send two successful chat requests;
  2. wait for a persisted sample;
  3. restart with the same data directory;
  4. send three successful chat requests;
  5. wait for another sample;
  6. query the full retained range with `points=2` and `points=1000`;
  7. select the `nimproxy_requests_total` rows for
     `/v1/chat/completions,status=200`;
  8. assert both responses total exactly `5`.

- [ ] **Step 2: Add a failing live-tail rollover test**

  Query a range after one persisted request, send another request before the
  next sample, and assert `/api/dashboard/now.tail` reports exactly one.
  After the sample lands, assert `history_revision` advances, the refreshed
  range reports both requests, and the new tail reports zero.

- [ ] **Step 3: Add a legacy reset fixture test**

  Write three v1 JSONL lines with request counters `10`, `15`, and `4`.
  Start the real proxy on that data directory and assert the retained range
  reports `10 + 5 + 4 == 19` and
  `diagnostics.legacy_resets_inferred == 1`.

- [ ] **Step 4: Add a historical-capacity test**

  Persist traffic at 120 RPM capacity, change one lane to 20 RPM, persist more
  traffic at 100 RPM capacity, and assert the range points contain both
  capacity values while `/api/dashboard/now.capacity_rpm == 100`.

- [ ] **Step 5: Run each acceptance test independently**

  Run:

  ```bash
  cargo test --test e2e dashboard_history_combines_process_epochs -- --nocapture
  cargo test --test e2e dashboard_tail_rolls_into_persisted_history_once -- --nocapture
  cargo test --test e2e legacy_history_infers_counter_reset -- --nocapture
  cargo test --test e2e historical_capacity_uses_snapshot_configuration -- --nocapture
  ```

  Expected: all four commands exit `0`.

- [ ] **Step 6: Run the complete Rust test suite**

  Run:

  ```bash
  cargo test
  ```

  Expected: unit and e2e tests finish with zero failures.

- [ ] **Step 7: Commit acceptance coverage**

  ```bash
  git add tests/support/mod.rs tests/e2e.rs
  git commit -m "test: cover dashboard history across restarts"
  ```

---

### Task 10: Real-NIM acceptance and synthetic browser verification

**Files:**

- Modify: `docs/assets/dashboard-overview.png` only if the verified header
  state materially changes the current reference
- Modify: other `docs/assets/dashboard-*.png` only when the corresponding
  verified surface materially changes

**Interfaces:**

- Sends a bounded real request batch through the deployed nim-proxy using the
  neighboring Rambler environment, then uses a disposable copy of retained
  history to exercise the feature build.
- Uses a separate disposable synthetic data directory for screenshots.
- Loads live credentials from the environment without printing, copying,
  logging, or staging them.
- Captures no real client names, prompts, or responses in screenshots/assets.
- Adds no browser automation dependency or repository test framework.

- [ ] **Step 1: Send bounded real NIM traffic through the deployed proxy**

  In a shell with tracing disabled, load the existing Rambler proxy client
  configuration without printing values:

  ```bash
  set +x
  set -a
  source /home/tchawes/rambler/.env
  set +a
  ```

  Send eight buffered and four streaming requests with a tiny synthetic prompt
  and output redirected:

  ```bash
  for request_number in $(seq 1 8); do
    curl --silent --show-error --fail \
      --header "authorization: Bearer ${JEVANCLIEF_NIM_API_KEY}" \
      --header 'content-type: application/json' \
      --data '{"model":"z-ai/glm-5.2","messages":[{"role":"user","content":"Reply with OK."}],"max_tokens":8,"stream":false}' \
      "${JEVANCLIEF_INFERENCE_BASE_URL%/}/chat/completions" \
      >/dev/null
  done
  for request_number in $(seq 1 4); do
    curl --silent --show-error --fail \
      --header "authorization: Bearer ${JEVANCLIEF_NIM_API_KEY}" \
      --header 'content-type: application/json' \
      --data '{"model":"z-ai/glm-5.2","messages":[{"role":"user","content":"Reply with OK."}],"max_tokens":8,"stream":true}' \
      "${JEVANCLIEF_INFERENCE_BASE_URL%/}/chat/completions" \
      >/dev/null
  done
  ```

  Every curl must exit `0`. Do not enable `set -x`, echo the variables, place
  them on a command line as literal values, or save response bodies.

- [ ] **Step 2: Exercise startup indexing against a copy of real retained data**

  Create a disposable directory and copy telemetry only—never the production
  `config.json`:

  ```bash
  real_history_test_data=$(mktemp -d)
  docker cp \
    nim-proxy-nim-proxy-1:/data/history.jsonl \
    "$real_history_test_data/history.jsonl"
  ```

  Start a local mock on port `19998` and the feature build on port `18081`:

  ```bash
  python3 scripts/mock_nim.py --enforce --rpm 40 --port 19998
  ```

  ```bash
  PORT=18081 DATA_DIR="$real_history_test_data" HISTORY_SAMPLE_SECS=300 \
    cargo run
  ```

  Complete setup with synthetic credentials and fetch two point budgets:

  ```bash
  curl --fail-with-body \
    --cookie-jar /tmp/nim-proxy-real-history-cookie \
    --header 'content-type: application/json' \
    --data '{"username":"historyop","password":"history-test-1","base_url":"http://127.0.0.1:19998","nim_keys":[{"key":"synthetic-history-k1"}]}' \
    http://127.0.0.1:18081/setup
  curl --fail-with-body \
    --cookie /tmp/nim-proxy-real-history-cookie \
    'http://127.0.0.1:18081/api/dashboard?from=1&to=4102444800&points=288' \
    --output /tmp/nim-proxy-range-288.json
  curl --fail-with-body \
    --cookie /tmp/nim-proxy-real-history-cookie \
    'http://127.0.0.1:18081/api/dashboard?from=1&to=4102444800&points=2' \
    --output /tmp/nim-proxy-range-2.json
  jq -e '[.totals[] | select(.metric == "nimproxy_requests_total") | .value] | add > 0' \
    /tmp/nim-proxy-range-288.json
  diff \
    <(jq -S '.totals' /tmp/nim-proxy-range-288.json) \
    <(jq -S '.totals' /tmp/nim-proxy-range-2.json)
  ```

  Startup logs must report indexed bytes/samples/resets/duration; `jq` and
  `diff` must exit `0`. Do not open this real-history copy in the browser or
  capture it in an asset.

- [ ] **Step 3: Start an isolated synthetic mock and proxy**

  In terminal one:

  ```bash
  python3 scripts/mock_nim.py --enforce --rpm 40 --port 19999
  ```

  In terminal two:

  ```bash
  dashboard_test_data=$(mktemp -d)
  PORT=18080 DATA_DIR="$dashboard_test_data" HISTORY_SAMPLE_SECS=1 \
    cargo run
  ```

  In terminal three, create only synthetic credentials and retain the session
  cookie:

  ```bash
  curl --fail-with-body \
    --cookie-jar /tmp/nim-proxy-dashboard-cookie \
    --header 'content-type: application/json' \
    --data '{"username":"dashop","password":"dashboard-test-1","base_url":"http://127.0.0.1:19999","nim_keys":[{"key":"synthetic-k1"},{"key":"synthetic-k2"},{"key":"synthetic-k3"}]}' \
    http://127.0.0.1:18080/setup
  curl --fail-with-body \
    --cookie /tmp/nim-proxy-dashboard-cookie \
    --header 'content-type: application/json' \
    --data '{"mode":"open"}' \
    http://127.0.0.1:18080/api/settings/clients
  ```

- [ ] **Step 4: Generate bounded synthetic traffic**

  Run:

  ```bash
  python3 scripts/loadtest.py \
    --proxy http://127.0.0.1:18080 \
    --mock http://127.0.0.1:19999 \
    --clients 12 \
    --requests 2 \
    --timeout 120
  curl --output /dev/null --write-out '%{http_code}\n' \
    --header 'content-type: application/json' \
    --data '{"model":"synthetic/model","messages":"invalid"}' \
    http://127.0.0.1:18080/v1/chat/completions
  ```

  The load harness must report zero upstream rate violations. The invalid
  local request supplies a synthetic client-error outcome; restart/5xx/stall
  paths are covered by Task 9's scripted Rust mock rather than adding another
  browser-only traffic mechanism. Confirm no real NIM endpoint or credential
  appears in either process environment.

- [ ] **Step 5: Verify first-login retained history**

  Restart the isolated proxy against the same data directory, log in with
  Google Chrome, and confirm Overview immediately shows the retained request
  total instead of zero. Confirm the header says **Following now** and
  **Default · 30d** with the effective available dates.

- [ ] **Step 6: Verify every analytical tab shares the window**

  Select `1h`, `All retained`, and one fixed custom range. On Overview,
  Models, Clients, Reliability, and Capacity confirm totals/rankings change
  together and the range survives tab switches. Confirm Settings hides the
  controller.

- [ ] **Step 7: Verify current-state exceptions**

  Pause on a fixed historical range, send traffic, enable/disable a synthetic
  lane, and change its RPM. Confirm fixed totals remain stable while **Now**
  active load, capacity, lane count, and header metadata refresh without a
  page reload.

- [ ] **Step 8: Verify SLO and empty states**

  Confirm 4xx/disconnect outcomes appear in diagnostics but do not reduce the
  SLO availability denominator. Confirm a future custom range displays
  `No traffic in selected window`, not `0 requests lifetime`.

- [ ] **Step 9: Verify responsive and security behavior**

  Inspect desktop and narrow viewport layouts, browser console errors, tooltip
  behavior, table sorting, custom dates, pause/resume, and offline logo/font
  fallbacks. Send a synthetic model label containing escaped punctuation and
  confirm it renders as text, never markup.

- [ ] **Step 10: Capture synthetic screenshots**

  Use `/usr/bin/google-chrome --headless --screenshot` or the available browser
  UI to capture Overview, Models, Clients, Reliability, Capacity, and Server
  Settings. Replace tracked assets only where the screenshots document the new
  time/scope behavior. Inspect every image before staging it.

- [ ] **Step 11: Commit changed reference assets**

  If screenshots changed:

  ```bash
  git add docs/assets
  git commit -m "docs: refresh dashboard reference images"
  ```

  If no tracked asset needs replacement, record browser evidence in the PR
  body and create no commit for this task.

- [ ] **Step 12: Remove disposable credentials and telemetry copies**

  Stop the mock/proxy processes, then run:

  ```bash
  rm -f \
    /tmp/nim-proxy-dashboard-cookie \
    /tmp/nim-proxy-real-history-cookie \
    /tmp/nim-proxy-range-288.json \
    /tmp/nim-proxy-range-2.json
  for disposable_dir in "$real_history_test_data" "$dashboard_test_data"; do
    resolved_dir=$(realpath "$disposable_dir")
    case "$resolved_dir" in
      /tmp/tmp.*) rm -rf -- "$resolved_dir" ;;
      *) echo "refusing unexpected cleanup target: $resolved_dir" >&2; exit 1 ;;
    esac
  done
  unset JEVANCLIEF_NIM_API_KEY JEVANCLIEF_INFERENCE_BASE_URL
  ```

  Confirm neither disposable directory exists. Production history and the
  running production container remain unchanged except for the twelve
  intentionally generated requests.

---

### Task 11: Repository memory and operator documentation

**Files:**

- Modify: `README.md`
- Modify: `CHANGELOG.md`
- Modify: `knowledge/architecture/metrics-history.md`
- Modify: `knowledge/architecture/dashboard.md`
- Modify: `knowledge/ops/configure-env.md`
- Modify: `knowledge/decisions/history-retention-days-not-size.md`
- Create: `knowledge/decisions/reset-aware-dashboard-history.md`
- Modify: `knowledge/index.md`
- Modify: `knowledge/decisions/index.md`
- Modify: `knowledge/log.md`

**Interfaces:**

- Documents the code that actually shipped; Git/OKF remains implementation
  authority and IdeaOS remains project meaning/continuity authority.
- Corrects the false 4 KB / 35 MB sizing claim rather than replacing it with
  another unmeasured estimate.
- Records one durable ADR for reset-aware rollups and time semantics.

- [ ] **Step 1: Update README dashboard and limitations text**

  Replace browser-local lifetime/range descriptions with:

  - persisted history drives every analytical tab;
  - default window and retention are separate Server settings;
  - default is 30 days for each, retention `0` is unlimited;
  - **All retained** means the current retained boundary;
  - current operational values are labeled **Now**;
  - custom/fixed ranges do not stop current-state refresh;
  - config files remain boot-read; Settings changes are live.

- [ ] **Step 2: Update the two architecture pages**

  In `metrics-history.md`, document v1/v2 JSONL, boot epochs, synchronous
  startup indexing, reset inference, exact totals, chart bucketing, tail
  normalization, retention baseline preservation, and atomic compaction.

  In `dashboard.md`, document the two typed endpoints, revision invalidation,
  cumulative-sample adapter, one global selected window, current-state
  exceptions, configured SLO, and removal of `/api/history` /
  `/dash/config.json`.

- [ ] **Step 3: Correct retention and environment guidance**

  In `history-retention-days-not-size.md`, explicitly state that the original
  4 KB snapshot and 35 MB/month estimate was disproven by real operation:
  label cardinality and histogram series can make files substantially larger.
  Keep the decision to use time-based retention, but ground it in operator
  intent rather than a size estimate.

  In `configure-env.md`, keep `DATA_DIR` deployment-level, keep
  `HISTORY_DAYS` warned-and-ignored, and state that out-of-band edits to
  `config.json` require restart.

- [ ] **Step 4: Write the reset-aware rollup ADR**

  Use the repository's required frontmatter and Context → Options → Choice →
  Consequences shape. Cover these rejected options:

  1. frontend-only reset repair;
  2. external TSDB/database;
  3. persisted sidecar or async partial readiness;
  4. precomputing page-specific dashboards.

  Record the chosen generic startup index, typed range/current contracts,
  explicit future boot epochs, best-effort legacy inference, and sample-time
  boundary precision.

- [ ] **Step 5: Link and log the knowledge ingest**

  Add the ADR to `knowledge/index.md` and
  `knowledge/decisions/index.md`. Append:

  ```markdown
  ## [2026-07-28] decision — reset-aware dashboard history

  - Replaced browser-local lifetime and cross-boot subtraction with a
    server-side reset-aware history index, one selected analytical window,
    separately configured retention/default window, and lightweight current
    polling.
  ```

  Preserve existing chronology; append rather than rewriting prior entries.

- [ ] **Step 6: Add the release note**

  In `CHANGELOG.md` under the active unreleased section, describe persisted
  post-restart dashboards, global time filtering, separate retention/default
  settings, **All retained**, configured SLO, and corrected capacity history.

- [ ] **Step 7: Verify knowledge links and stale claims**

  Run:

  ```bash
  rg -n "4 KB|35 MB|browser-side ring|hardcoded 99.9|last . first|/api/history|dash/config" README.md knowledge
  ```

  Inspect every match and retain only historical context that is clearly
  labeled as superseded. Check all new relative Markdown links manually with
  `test -e` against their resolved repository paths.

- [ ] **Step 8: Commit documentation and knowledge**

  ```bash
  git add README.md CHANGELOG.md knowledge
  git commit -m "docs: explain reset-aware dashboard history"
  ```

---

### Task 12: Final verification, Ponytail diff audit, and draft PR

**Files:**

- Review: every file changed from `2c48e84`
- Modify: only files required to correct verification failures

**Interfaces:**

- Produces one clean branch whose draft PR closes issue #67 when merged.
- Produces fresh command evidence for tests, formatting, linting, browser
  behavior, security, and scope.

- [ ] **Step 1: Run formatting and inspect formatter changes**

  Run:

  ```bash
  cargo fmt --check
  ```

  If it fails, run `cargo fmt`, inspect the formatting-only diff, and rerun
  `cargo fmt --check`.

- [ ] **Step 2: Run the complete compiler/lint/test gates**

  Run:

  ```bash
  cargo test
  cargo clippy --all-targets -- -D warnings
  git diff --check 2c48e84...HEAD
  ```

  Expected: every command exits `0` with zero test failures and zero warnings.

- [ ] **Step 3: Run targeted release/security checks**

  Run:

  ```bash
  python3 scripts/test_release_contract.py
  rg -n "nvapi-|npk_|password|secret_sha256|prompt|response" \
    docs/assets knowledge README.md CHANGELOG.md
  ```

  Inspect matches so documentation terms are allowed but no credential,
  captured prompt, response body, or real private client label is present.

- [ ] **Step 4: Apply the Ponytail ladder to the final diff**

  Run:

  ```bash
  git diff --stat 2c48e84...HEAD
  git diff --name-status 2c48e84...HEAD
  git diff 2c48e84...HEAD -- Cargo.toml Cargo.lock
  ```

  Verify:

  - no dependency changed;
  - no new service/database/sidecar/framework appeared;
  - no duplicate parser, settings pipeline, auth gate, or renderer was added;
  - every new type/endpoint/state field maps to an acceptance criterion;
  - page-specific precomputation and generalized SLO machinery remain absent;
  - the raw-history transport removal has no remaining caller;
  - no unrelated refactor is mixed into the branch.

  Remove or collapse any addition that fails this audit, then rerun Steps 1–3.

- [ ] **Step 5: Review the security invariants**

  Inspect every changed `innerHTML` assignment and confirm dynamic
  model/client/status/range text passes through `esc()` or `textContent`.
  Confirm `/api/dashboard` and `/api/dashboard/now` remain behind
  `require_session`, role filtering still protects Server settings, and
  capacity/history responses contain no key identity or filesystem path.

- [ ] **Step 6: Review acceptance criteria against issue #67**

  Use `gh issue view 67` and map each issue checkbox to fresh test or browser
  evidence. Add missing evidence before publishing; do not edit the issue to
  hide a gap.

- [ ] **Step 7: Commit verification fixes**

  If verification changed files:

  ```bash
  git add -A
  git commit -m "fix: satisfy dashboard history verification"
  ```

  If no file changed, create no empty commit.

- [ ] **Step 8: Push and open a draft PR**

  First use `apply_patch` to create
  `/tmp/nim-proxy-dashboard-history-pr.md` with this exact body after the
  verification steps have passed:

  ```markdown
  Closes #67

  ## Summary

  - build one reset-aware generic history index at startup and preserve
    backward reading of v1 JSONL
  - apply one retained time window across all analytical tabs, with separate
    default-window, retention, and SLO settings
  - split retained range queries from lightweight current polling and refresh
    live tails/configuration by revision
  - calculate historical capacity from contemporaneous snapshot metadata

  ## Verification

  - `cargo test`
  - `cargo fmt --check`
  - `cargo clippy --all-targets -- -D warnings`
  - `python3 scripts/test_release_contract.py`
  - synthetic Chrome verification across Overview, Models, Clients,
    Reliability, Capacity, and Server Settings
  - bounded real NIM requests through the deployed proxy plus startup/query
    verification against a disposable copy of retained production telemetry
  - bounded enforcing-mock traffic with zero upstream rate violations

  ## Scope and minimality

  Request admission, pacing, dispatch, retries, authentication, and upstream
  behavior are unchanged. The implementation reuses the existing History,
  config commit, authenticated router, dashboard renderer, e2e harness, Rust
  stdlib, Axum/Tokio/Serde, and installed metrics stack. It adds no dependency,
  database, service, sidecar index, frontend framework, generalized SLO
  subsystem, or page-specific dashboard cache.
  ```

  Run:

  ```bash
  git push -u origin feat/dashboard-history-rollups
  gh pr create --draft \
    --base main \
    --head feat/dashboard-history-rollups \
    --title "Make dashboard history reset-aware" \
    --body-file /tmp/nim-proxy-dashboard-history-pr.md
  ```

  Attach the synthetic screenshots to the draft PR after creation when they
  materially aid review. The body already records the exact commands and
  Ponytail audit; edit it only to add a concrete failure/limitation discovered
  during verification.

- [ ] **Step 9: Inspect the published PR**

  Run:

  ```bash
  gh pr view --json number,title,isDraft,baseRefName,headRefName,body,url
  gh pr checks --watch
  ```

  Confirm the base/head branches, draft state, issue-closing phrase, changed
  file set, and CI results. Keep the worktree until review/merge work is
  complete.
