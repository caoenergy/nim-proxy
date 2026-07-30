---
type: Runbook
title: Test strategy
description: Unit, end-to-end, load, and fuzz layers — what each catches and how to run them.
tags: [testing, ci]
timestamp: 2026-07-02T00:00:00Z
---

# Test strategy

Four layers (unit, end-to-end, load, fuzz). CI (`.github/workflows/ci.yml`)
runs the unit + e2e suites plus a full gate set on every PR: `fmt` + `clippy
-D warnings` + tests (the `check` job), `coverage` (≥90%, `cargo-llvm-cov`),
an `msrv` build against Rust 1.87, `cargo-deny` (advisories + bans + licenses),
`gitleaks` secret scan, `workflow lint` (`actionlint` + `zizmor`),
`dependency review`, and a `docker build` with a container healthcheck smoke.
Three more workflows run outside PR CI: **CodeQL** SAST (`codeql.yml` — PR +
push + weekly), a weekly **cargo-deny advisories** audit (`audit.yml`), a
weekly **fuzz** smoke (`fuzz.yml`, layer 4 below), and the weekly **OpenSSF
Scorecard** scan.

## Proof routing

Use the proof that exercises the changed surface, and preserve the limits of
that proof.

- **Rust logic:** `cargo test` and `cargo clippy --all-targets -- -D warnings`.
- **Handlers or wire types:** `UPDATE_OPENAPI=1 cargo test --test openapi`,
  then verify that `openapi.json` has only the deliberate diff.
- **Embedded pages:** `node scripts/render_check.js --syntax-selftest` proves
  the syntax gate can reject its fixtures. `node scripts/render_check.js
  --syntax-only` parses the real dashboard and setup scripts without launching
  Chromium. Syntax-only mode proves parsing, not behavior. For behavior, run
  `node scripts/render_check.js`,
  `node scripts/render_check.js --escape-probe`,
  `node scripts/render_check.js --page setup`, and
  `node scripts/render_check.js --page setup --escape-probe`.
- **Number, date, and duration formatting:**
  `TZ=UTC LC_ALL=en_US.UTF-8 node scripts/formatter_fixture.js --check`.
- **Catalog and UI text:** `python3 scripts/check_i18n.py --selftest`, then
  `python3 scripts/check_i18n.py`; when English changes, also run
  `python3 scripts/gen_pseudolocale.py --check`.
- **Locale files:** `python3 scripts/locale_v1.py --selftest` and
  `python3 scripts/locale_v1.py --all`.
- **Pacing, pool, dispatch, and affinity:** use the enforcing mock and load
  harness; one upstream violation is failure. Follow the setup prerequisites
  in the [load section](#3-load--scriptsloadtestpy-vs-scriptsmock_nimpy---enforce).
- **Layout:** use mechanical overflow probes where available plus explicit
  human review under rendered data and supported widths. Behavior passing does
  not prove fit.
- **Before push:** `cargo fmt --check`.

CI runs the automated checks configured in [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml);
it does not perform human layout review or the strict load scenario. `cargo test`
does not execute embedded-page JavaScript. `render_check.js` proves only its
covered fixtures and interactions; it is not evidence that every page path,
locale, or layout is covered. A relevant missing reusable proof is a work
item. A scratch reproduction may demonstrate a problem but does not become the
regression gate.

PR CI runs both agent-guide modes: `python3 scripts/check_agent_guide.py
--selftest` proves each contract check can reject its fixture, and `python3
scripts/check_agent_guide.py` rejects a missing stable contract or unresolved
repository-local guide link.

## 1. Unit — `cargo test` (in `src/`)

Pool semantics (window spread, least-loaded, sticky/spill flags, penalize,
release), dispatcher ordering and deadline fail-fast, SSE scanning, history
retention/downsampling. Fast, deterministic, no I/O.

## 2. End-to-end — `tests/e2e.rs` + `tests/support/mod.rs`

Each test launches the **real binary** (`CARGO_BIN_EXE_nim-proxy`) against an
in-process mock NIM whose next responses are scripted per test
(`Behavior::{RateLimited, ServerError, BadRequest, BadRequestIfInjected,
Hang, Ok}`). Boot uses a **pre-written `config.json` in a tempdir `DATA_DIR`**
(`start_proxy_with`, cleaned on drop) or drives the `/setup` wizard
(`start_proxy_fresh` + `complete_setup`); `expect_refuses_to_start` covers a
corrupt store, `version>1`, and an unwritable `DATA_DIR`. Covers: the setup
posture (`/v1`→503, `/`→302 `/setup`) and wizard happy path, open vs keyed
`/v1`, multi-user login / session cookie / scraper Bearer, role and ownership
denials, the config-store round-trip and live pool rebuilds mid-run, per-model
worker-exhaustion governing, 429 ride-out with key failover, Retry-After
timing, verbatim error relay, fail-fast 504, pacing enforcement, conversation
affinity (pin + spread), models cache single-hit, usage injection incl.
rejection fallback and kill switch, stalled-stream cutoff, metrics accuracy
(exact token counts), history persistence across restart, SIGTERM, and
dashboard/config routes.

### The wire-format guards

Two tests exist purely so the JSON contract cannot move by accident (see
[typed-responses-and-generated-openapi](../decisions/typed-responses-and-generated-openapi.md)):

- `api::field_order_stays_ascii_sorted` (unit) serializes a populated value of
  every response type and asserts the keys come out ASCII-sorted. Declaration
  order *is* the wire order, and the pre-0.6.6 `json!` bodies were sorted by
  `serde_json`'s `BTreeMap` — so a "tidier" field reorder is a wire change,
  and this is what says so.
- `tests/openapi.rs` regenerates `openapi.json` and fails on any difference
  from the committed file. Regenerate with
  `UPDATE_OPENAPI=1 cargo test --test openapi`; CI's `check` job runs that and
  then `git diff --exit-code -- openapi.json`. `spec_is_usable` additionally
  asserts the document is consumable — 14 operations, each tagged with a
  documented 200, `/api/*` inheriting the auth requirement and `/setup`
  explicitly waiving it.
- `control_plane_rejections_are_typed` sends raw requests through the real
  binary for malformed JSON, JSON media-type failures, body-size rejection,
  invalid dashboard query, unknown/method-mismatched `/api/*`, and post-claim
  setup POSTs. It asserts status, `application/json`, exact `ApiError` bytes,
  and unchanged `config.json` bytes. Run it with `cargo test --test e2e
  control_plane_rejections_are_typed -- --exact`.
- `unknown_control_plane_paths_are_gated_before_fallback` proves that the
  control-plane fallback remains inside the setup/auth gate: a fresh install
  returns `503 setup_required`, an anonymous configured install returns 401,
  and an authenticated caller receives typed `404 not_found`.
- `closed_setup_posts_win_before_body_rejections` proves both setup POSTs
  answer `409 setup_complete` before malformed, missing/wrong-media, or
  oversized bodies are parsed. Its oversized request sends only an over-limit
  `Content-Length` with `Expect: 100-continue`, proving the route answers
  before buffering 64 MiB. `setup_double_claim_is_rejected_with_409` also
  checks the race loser emits that exact envelope.
- `open_setup_posts_keep_typed_extractor_rejections` covers both manual setup
  extractors while setup is still open: malformed JSON, missing/wrong media
  types, and the bounded body limit retain the exact `ApiError` bytes and do
  not create a config store.

## 3. Load — `scripts/loadtest.py` vs `scripts/mock_nim.py --enforce`

The enforcing mock plays a *strict* NIM: true per-key sliding window,
counting every violation. `--worker-slots N` adds NIM's orthogonal per-model
worker-concurrency cap (emitting the real exhaustion error) so the
[governor](../architecture/governor.md) is exercised; `loadtest.py` reports
worker exhaustions + peak per-model concurrency. 100 concurrent clients, mixed
streaming/buffered, multiple models and client tokens. **Exit is non-zero on a
single client-visible failure or a single upstream rate violation.**

```sh
python3 scripts/mock_nim.py --enforce --rpm 40 --worker-slots 32 --port 9999 &
cargo run --release &     # boots into first-run setup (no app-level env vars)
# complete the wizard at /setup — base URL http://127.0.0.1:9999, add the mock's
# keys, set the API mode to open (or mint a client key for --proxy-keys)
python3 scripts/loadtest.py --clients 100 --requests 3
```

This layer earned its keep on day one: it caught ~2% boundary-jitter
violations that unit and e2e tests structurally cannot see, leading to
[window-jitter-margin](../decisions/window-jitter-margin.md); it now also gates
the governor's convergence and zero-violation invariant across live pool
rebuilds.

## 4. Fuzz — `cargo +nightly fuzz run <target>` (in `fuzz/`)

libFuzzer/cargo-fuzz harnesses over the three surfaces that parse bytes we
don't control, asserting *never panics* plus each surface's invariant:
`sse_scan` (upstream SSE arrives arbitrarily fragmented — fed whole and
re-fragmented, asserting the 1 MiB pathological-line guard), `sanitize_label`
(the metric-injection defense: output is non-empty, ≤64 chars, safe charset),
and `config_roundtrip` (operator-edited `config.json`: parse never panics,
serialize→parse→serialize is a fixpoint). `fuzz.yml` smoke-fuzzes each target
60s weekly, on demand, and on PRs touching `src/proxy.rs`, `src/config.rs`, or
`fuzz/**`; it is deliberately **not** a required merge check. Seed corpora live
in `fuzz/seeds/` (real SSE shapes, hostile label bytes, a full store); the
evolved working corpus in `fuzz/corpus/` is gitignored. Run one locally:

```sh
cargo +nightly fuzz run sse_scan -- -max_total_time=60
```

Dashboard changes get two more checks.

**Automated — `node scripts/render_check.js`.** Renders the page against the
captured payloads in `tests/fixtures/api/`, walks all five tabs, hovers every
chart with real pointer input, and fails on any uncaught page error.
`--escape-probe` additionally fails on a render helper escaping a catalog value
that was already escaped at load. This is the only gate that proves the page
*runs*: `cargo test` asserts on served HTML text and `node --check` proves only
that it parses. See [render-gate](../decisions/render-gate.md).

**Human — screenshots, still.** Real-browser screenshots under live traffic
(the UI is dark-only since the operator-console redesign), inspected by eye —
as superuser/admin/user, confirming each role sees the right Settings sections.
Clipping is a layout property and no script judges it; the `.bval` wrap on the
Models tab at 900px was found by eye and clips in English already.
