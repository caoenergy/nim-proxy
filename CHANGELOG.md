# Changelog

All notable changes to nim-proxy are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.6.6] - 2026-07-29

### Added

- **`openapi.json`** — a generated OpenAPI 3.1 description of the dashboard
  API, committed at the repo root. It covers 14 operations: the twelve
  `/api/*` routes, plus `POST /setup` and `POST /setup/validate-key`, which
  are flagged unauthenticated because they run before any user exists (and
  404 once one does). The `/v1` passthrough is deliberately out of scope —
  that contract belongs to the upstream.

  The spec is generated from the handlers with `utoipa`, so it cannot describe
  an API that no longer exists: CI regenerates it and fails on any difference.
  Regenerate locally with `UPDATE_OPENAPI=1 cargo test --test openapi`. No
  documentation UI is served — the available ones fetch JavaScript from a CDN
  that the dashboard's Content-Security-Policy forbids, and bundling would add
  about a megabyte to a `FROM scratch` image. Point an offline viewer or a
  client generator at the file instead.

- Two vocabulary checks. `locale-v1` gains **frozen**: units, HTTP status
  codes and API identifiers (`rpm`, `tok/s`, `429`, `/v1`, `NIM`, `TTFT`) must
  survive verbatim in every translation, because a machine translator will
  otherwise render `rpm` as `tr/min` and produce a locale that passes every
  other check while being wrong. The i18n lint gains a **retired-term** check
  so a standardized interface cannot drift back apart one label at a time. The
  vocabulary they enforce is now recorded in
  `knowledge/decisions/standard-vocabulary.md`.

  The retired-term check scans the **whole page**, not just catalog values.
  Scanning values alone was not enough and the gap was live: `rpm total` was
  rendering in the setup wizard's review panel, outside the catalog, with the
  check reporting clean. A label that never made it into the catalog still
  reaches the operator.

  The frozen check found a defect on its first run: the `en-XA` pseudolocale
  was accenting those tokens, rendering `NÎM` and a mangled `/v1` across nine
  messages — in the one locale that exists to prove layout. Generator fixed and
  `en-XA` regenerated.

- A render gate, `scripts/render_check.js`: it loads the dashboard against
  captured API payloads, clicks through all five tabs, hovers every chart with
  real pointer input, and fails on any uncaught page error. No new dependency —
  Node's built-in WebSocket driving the system browser. It is the only check
  that proves the page *runs*; `cargo test` asserts on served HTML text and
  `node --check` proves only that it parses.

  `--escape-probe` enforces the escape-once rule in **both** directions, at both
  text and attribute sinks. It appends `Ampersand & Quote' <b>Tag</b> DQ"` to
  every catalog value, and each character is load-bearing:

  - `&` and `'` come back as literal `&amp;` / `&#39;` when something escaped an
    already-escaped value.
  - `<b>` parses into a real DOM element when a value reached a raw-HTML text
    sink unescaped.
  - `"` closes a quoted attribute early, so an unescaped value at an
    *attribute* sink makes the parser read the remainder as attribute **names** —
    and a real attribute name can never contain `<`, `"`, `'` or `=`.

  That last one is not the obvious approach and the obvious approach does not
  work: `getAttribute()` decodes entities, so a correctly-escaped and an
  unescaped attribute value are byte-identical by the time the DOM holds them.
  Scanning attribute values for the tag fires on eight legitimate attributes.

  The first version carried no tag and walked text nodes only, making it a
  double-escape detector blind to the missing-escape direction — the XSS
  direction. Established by injection rather than by reading: of the defects
  tried, the original probe caught a double-escape at a text sink and passed
  under-escaping at both text and attribute sinks green. All of them now fail,
  and the detector requires the probe's own marker in the same element so it
  cannot mistake a legitimate `<b>` for a sink.

  The gate also asserts the status predicates agree. `IS_2XX` / `IS_ERR` are
  module-scope in `dashboard.html` specifically so it can evaluate them: the
  captured payloads contain only `200`, `429`, `504` and `disconnect`, so
  replaying fixtures can never observe the disagreement described under Fixed.

  It covers `src/dashboard.html` only. `src/setup.html` has no render coverage
  in CI — it passes when driven by hand, but nothing keeps it passing.

- Localization guards: an `en-XA` pseudolocale (generated, never hand-edited),
  a `locale-v1` validator covering completeness, placeholder parity, formatter
  syntax, raw markup, inline balance, source-hash freshness and length caps, and
  an untagged-string lint. Every check ships with a deliberately broken fixture
  proving it can fail, and all four run in CI.

  The untagged-string lint had three holes, and **25 English strings were
  shipping through them with CI green** — including a term this same release
  retired. All three are closed:

  - It scanned single-quoted strings only, so `src/setup.html`, which uses
    double quotes throughout, was effectively unscanned. Eight operator-facing
    error messages sat behind that — a ninth was extracted at the same time but
    the lint never demanded it, because it contains an underscore.
  - Nothing looked at text nodes inside template literals. `<span
    class="k">Superuser</span>` is neither a quoted string nor page markup —
    `strip_scripts()` deletes the script holding it. Sixteen labels lived there,
    including a `Latency breakdown` literal ten lines from the catalog id for
    the same words.
  - The "this is machinery, not text" filter was applied per **line**, so one
    `.toFixed(` anywhere on a line exempted every string beside it. That is how
    `no eligible traffic` rendered in English on a line CI called clean.

  A later review pass found a fourth hole that the widening itself opened — the
  "this is an attribute value" guard allowed whitespace before the `=`, which
  exempted every JS assignment and comparison (`const x = 'label'`,
  `el.textContent = 'label'`, `if (s === 'label')`). Tightening it costs zero
  findings on either page, so the loose form was pure blind spot. Closed, with
  the reviewer's eleven injections now caught and three legitimate-string
  controls still correctly ignored.

  It is still not proof that the pages contain no English, and three specific
  shapes remain invisible:

  - Lowercase single tokens, ignored deliberately because that shape is usually
    an enum or metric label value. `met` and `missed` were found by reading.
  - Prose inside a *nested* template literal, and prose in a template literal
    with no tags around it (`` `Some label ${x}` ``) — the scan looks for text
    between `>` and `<`. The two live leaks `errors 42% · 8 cooldowns` and
    `0 now` are exactly that shape.
  - An English plural written as control flow (`n === 1 ? '' : 's'`), where the
    English is the absence of a character rather than a string. Four are still
    live and named in `knowledge/decisions/plural-categories-not-ternaries.md`.

  `tests/fixtures/locales/REMAINING.md` is the measured inventory, and the
  settings surface remains out of scope until 0.6.7.

- The dashboard and setup wizard now render their text from an embedded
  `en-US` message catalog rather than hardcoded literals — the groundwork for
  localization. **225 messages (181 dashboard, 44 setup)**, covering the static
  markup of both pages, the analytics call sites, the wizard's error messages
  and the SLO and capacity notes; the settings surface follows in 0.6.7.

  Sentences with interpolated values are **one message with placeholders**, never
  concatenated fragments — `Key validation failed: {error}`, not
  `"Key validation failed: " + e` — because word order moves between languages
  and a fragment gives a translator nothing to work with.

  Counts go through `Intl.PluralRules` (ladder rung 4). Two hardcoded English
  ternaries — `enabled key${n === 1 ? '' : 's'}` and
  `interval${n === 1 ? '' : 's'}` — were English grammar sitting in the render
  path. Both are now six-category plural sets, spelled out as explicit ids so
  the orphan check can still see them; `locale-v1` requires exact id parity, so
  a category absent from the source could never be supplied by a translation,
  and ar/ru/pl/cy need categories English does not have.

  English output is unchanged, and `tests/fixtures/locales/REMAINING.md` is the
  measured inventory of what still renders in English — regenerate it from
  `node scripts/render_check.js --locale en-XA`, which prints the count.

  What proves what here is worth being precise about, because it is easy to
  overstate. `scripts/check_i18n.py` proves each **tagged element still holds
  the text its catalog id claims**, that no id is missing or orphaned, and that
  no hash is stale. It does **not** prove the rendered page is unchanged — it
  compares markup to catalog, and reworded text on both sides round-trips
  clean. The claim that the page still *renders* the same rests on
  `scripts/render_check.js`, and note that `cargo test`'s
  `assert!(html.contains("…"))` checks cannot support it either: the catalog
  ships inline in the served HTML, so those assertions match whether or not the
  render path ever uses the string.

### Changed

- Every dashboard-API response body is now a Rust type rather than a
  hand-built JSON literal, and `GET /api/config`'s role filtering is expressed
  in that type: the admin-only `server` and `users` sections are `Option`s
  that are never constructed for a `user`, instead of keys added to an
  otherwise-complete body.

  **No wire change.** The JSON is byte-for-byte what 0.6.5 served, at every
  nesting level, and the existing end-to-end suite passes unmodified. Two
  internal side effects an operator may notice: `config.json`'s `limits` block
  and `governor.overrides` are written in a different key order (the store is
  read by name, so nothing migrates), and `governor.overrides` is now
  serialized in sorted order, making repeated saves of the same configuration
  byte-identical.

- Numbers, durations, and dates in the dashboard are formatted with `Intl`,
  keyed to the interface's locale rather than the browser's. Two long-standing
  rounding bugs go with it: `999,999` rendered as `1000.0K` instead of `1M`, and
  values above a trillion rendered as `1000.0B` because there was no `T` tier.
  Durations now read `1.0 sec` rather than `1.0 s`, matching the `ms` and `min`
  forms and what `Intl` considers correct for en-US.

- Dashboard and setup-wizard labels now use standard ops-dashboard vocabulary
  throughout. `Harness`/`Harnesses` become **Client**/**Clients**; the
  dashboard's `window` becomes **time range** (the rate-limit rolling window
  keeps "window"); `lane` becomes **key** in the interface, since a lane is one
  NIM credential and Settings already said "keys". `Conversation stickiness` →
  **Session affinity**, `Model-pressure governor` → **Model limits**,
  `Where time goes` → **Latency breakdown**, `Rate-limit pressure` →
  **Throttling**, `Historical provisioning` → **Capacity history**, `Keyed` →
  **API key required**. The composite `Shed · 401 · failed logins` row splits
  into **Dropped**, **Unauthorized**, and **Failed logins**.

  Display text only — no metric, route, CSS class, `data-*` attribute, or DOM
  id changed. Metric labels keep `lane` (`nimproxy_lane_requests_total`), so
  the interface says "key" while the exposition still says "lane"; renaming
  the series would be a second breaking change and was not taken.

- **Breaking:** the lane state entered after an upstream 429/5xx is now called
  **cooldown** rather than "bench", and the Prometheus series
  `nimproxy_lane_benched_total` is renamed to `nimproxy_lane_cooldown_total`.
  Every other `nimproxy_*` series is unchanged. Update any dashboards, alerts,
  or recording rules that referenced the old name.

  Retained history stores the metric name verbatim, so lane-cooldown charts
  show a gap for points recorded before the upgrade and return to full fidelity
  one retention window (`history.days`) later. This is a deliberate clean break
  — no compatibility alias is carried.

### Removed

- **Breaking:** pricing configuration and the estimated-savings metric. The
  `Dollars saved` KPI, the `Saved` columns in the Models, Clients, and
  Reliability tables, the Pricing settings card, the `pricing` config block
  (`ref_price_in` / `ref_price_out`), and the `POST /api/settings/pricing`
  route are gone; `/api/config` no longer returns `server.pricing` and
  `/api/dashboard/now` no longer returns `price_in` / `price_out`.

  An honest figure needs a published per-model rate for each model in the pool;
  applying one reference rate to every model measured nothing. Existing config
  stores containing a `pricing` block still load — the orphan key is ignored,
  and no migration runs. `REF_PRICE_IN` / `REF_PRICE_OUT` remain in the
  legacy-env warning list so an upgrader who still sets them is told they do
  nothing.

### Fixed

- Hovering any time-series chart no longer breaks the dashboard. A
  module-scope date helper introduced in this release collided with two
  pre-existing local bindings of the same name, so every chart threw on hover.
  Because the chart re-applies the last hover position on each live re-render
  and the poll loop treats any error as a lost connection, resting the cursor
  over a chart made a perfectly healthy proxy display a red **Disconnected**
  badge, stop its uptime clock, and leave most of the tab frozen at stale
  values. Only ever present on unreleased 0.6.6 builds.

- KPI card labels no longer double-escape. Catalog values are escaped once when
  the catalog loads, and the KPI helper escaped its label a second time. In
  English no KPI label contains an escapable character, so this was invisible —
  it would first have appeared as `&#39;` and `&amp;` in the interface of any
  translated build.

- **Reliability panels no longer contradict each other about what succeeded.**
  Seven sites (eight occurrences) decided "this request succeeded" by comparing
  the status label to the literal `'200'`, and the label is whatever the upstream
  returned, passed through verbatim. A single `204` was simultaneously counted as Success in the
  stacked outcome chart, as an error in the "Outcomes per minute" chart directly
  above it, as an `HTTP 204` row in the table directly below, and excluded from
  the taxonomy entirely — so the Error rate percentage and the segbar rendered
  inside the *same card* disagreed. There is now one `IS_2XX` / `IS_ERR` pair at
  module scope and the render gate asserts on it.

- **The setup wizard's `data-i18n-attr` handler had no attribute allowlist.**
  The dashboard refuses anything outside `title`, `placeholder`, `aria-label`
  and `alt`; the wizard called `setAttribute()` with whatever attribute name the
  markup supplied. A markup edit adding `data-i18n-attr="onclick:…"` would have
  routed a catalog value into an inline event handler, which the
  Content-Security-Policy permits. Not reachable in the shipped markup, and the
  static check caught non-allowlisted targets — but both
  `knowledge/decisions/message-catalog-and-escaping.md` and a comment in
  `check_i18n.py` asserted the runtime enforced this, and it did not.

- `rpm total` was still rendering in the setup wizard's review panel, a term
  this release retired, because the retired-term check only read catalog values.

## [0.6.5] - 2026-07-28

### Fixed

- Persisted dashboard traffic now appears immediately after login and remains
  truthful across process/container restarts. Startup indexing normalizes
  explicit v2 process epochs and legacy v1 counter resets; chart point limits
  no longer change reported totals.
- The existing dashboard time controls now apply one selected window across
  Overview, Models, Clients, Reliability, and Capacity. The default follows
  the retained 30-day window, fixed/paused ranges stay fixed, and **All
  retained** reaches the earliest available sample.
- Historical Capacity views now use the configuration recorded with each
  sample instead of comparing past traffic with today's pool. Unavailable
  pre-history time is no longer treated as observed capacity.

### Security

- Moved release metadata and image digests from inline shell-template
  expansions into step-scoped environment variables, and added a seven-day
  observation window for routine Cargo, GitHub Actions, and Docker dependency
  updates. Dependabot security updates remain immediate.
- Replaced the third-party GitHub Release publishing action with the
  GitHub-hosted runner's preinstalled `gh` CLI, preserving generated notes,
  verification instructions, and signed asset uploads while reducing the
  workflow's external action surface.

### Changed

- Docker Compose's host-side publish address is now configurable with
  `PUBLISH_HOST` in `.env` while retaining `127.0.0.1` as the safe default.
- Replaced browser parsing of raw `/metrics`, `/api/history`, and
  `/dash/config.json` data with authenticated typed range/current dashboard
  contracts, revision-aware live tails, and server-side exact rollups.
- Default dashboard window, data retention, and availability target are
  separate Server settings rather than hardcoded display assumptions. Window
  and retention default to 30 days; retention `0` is unlimited and finite
  retention cannot be shorter than the default view.
- Current lane/load values use the live **Now** snapshot while selected-window
  values stay historical.
- History retention now trims the in-memory index immediately and compacts the
  JSONL file atomically in the background while preserving the boundary
  baseline and boot marker needed for exact retained totals. The old
  fixed-size estimate was removed after a real 7,316-sample history measured
  235,598,655 bytes; size remains workload-dependent.

## [0.6.4] - 2026-07-17

### Added

- Added the opt-in `X-Nim-Proxy-Deadline-Ms` request header: an absolute
  wall-clock deadline enforced across queueing, worker admission, retries, and
  generation. Buffered expiry returns `504 deadline_exceeded`; streaming
  expiry emits the same error inside the committed SSE response. Expiry drops
  upstream work and all request-owned permits, and is exposed as request status
  `deadline` plus `nimproxy_deadline_exceeded_total`.

### Security

- Bumped `crossbeam-epoch` to 0.9.20 in `Cargo.lock` to resolve
  [RUSTSEC-2026-0204](https://rustsec.org/advisories/RUSTSEC-2026-0204)
  (invalid pointer dereference in the `fmt::Pointer` impl for `Atomic`/`Shared`).
  It reaches us transitively via `metrics-util` →
  `metrics-exporter-prometheus`. Lockfile-only change; no dependency versions
  in `Cargo.toml` changed. Clears the `cargo-deny` advisories failure that was
  red on `main` and every open Dependabot PR.

### Changed

- Refreshed runtime and supply-chain dependencies: Tokio 1.53.1, bytes 1.12.1,
  serde 1.0.229, serde_json 1.0.151, futures-util 0.3.33, tokio-stream 0.1.19,
  the pinned Rust builder image and toolchain action, pinned GitHub Actions,
  and `sigstore/cosign-installer` 4.1.2.
- Migrated downloadable-asset signing to Cosign v3 Sigstore bundles, pinned
  the Cosign CLI independently of its installer action, and added a real
  sign/verify contract smoke test to CI.
- Internal cleanup (no behavior change): dropped a redundant `async` on the
  streaming handler (all `.await`s live inside its spawned task, so the
  function itself never awaited — this avoids wrapping it in a needless
  future), removed two redundant `String` clones on the key-add paths, and
  reused the destination buffer via `clone_from` when re-owning keys during
  superuser claim. `cargo clippy --all-targets -- -D warnings`, `cargo fmt`,
  and the full test suite (lib + e2e) stay green.
- Rewrote the `Basic`-auth credential branch in `auth::identify` with the `?`
  operator (behavior identical). Rust stable rolled to 1.97 on 2026-07-14 and
  its improved `clippy::question_mark` lint flagged the old
  `else if let … else { return None }` shape, breaking the `-D warnings` CI
  job on code untouched by any open PR. Covered by the existing auth tests.

## [0.6.3] - 2026-07-05

Supply-chain and static-analysis release — no proxy behavior changes.

### Documentation

- Enriched the PR template into a standard, agent-legible form (Summary / Type
  of change / Related issues / What & why / How it was tested / Breaking
  changes, plus a checklist grouped by concern with each conditional section
  labeled by its trigger).
- Documentation-consistency pass across README, CONTRIBUTING, SECURITY, the
  test-strategy and release runbooks, and the issue templates: recorded the
  full current CI gate set (coverage, MSRV, workflow lint, dependency review,
  CodeQL) and the applied `main`/`v*` rulesets, added the fuzzing test layer
  and signed-release-asset notes, and corrected a stale `cargo audit` reference
  (it is `cargo-deny`) and an old version placeholder.

### Testing

- **Coverage expansion** (91.4% → 96.1% lines): new unit tests for the auth
  primitives (base64/unhex/session-shape/cookie-Secure/throttle-rollover —
  `auth.rs` is now 100%), `config::validate` rejection branches, `parse_role`
  (superuser is never assignable), the SSE 1 MiB guard, and history load +
  daily-compaction; plus e2e tests for setup double-claim, orphan client-key
  adoption, throttled/failed key probes, client/nim-key/user validation and
  ownership legs, and the auth handler surface (HTTP Basic scrape creds, login
  redirects, logout). The CI coverage gate is raised from 80% to 90%.

### Added

- **Release assets are signed** (`cosign sign-blob`, keyless via OIDC): the
  downloadable per-arch tarballs and the SBOM now ship with a detached
  signature (`.sig`) and the signing certificate (`.pem`), so a binary pulled
  from the Releases page is verifiable with `cosign verify-blob` — previously
  only the container image was signed. The release notes carry the exact
  verification command.

- **Fuzz testing** (`fuzz/` + a weekly smoke-fuzz workflow): cargo-fuzz
  targets for the three untrusted-byte parsers — the upstream SSE scanner
  (arbitrary fragmentation, buffer-bound invariant), the Prometheus-label
  sanitizer (charset/length/non-empty invariants), and the config-store
  JSON round-trip (parse never panics; save→load is a fixpoint). The crate
  is now a thin binary over a library so the fuzz harnesses can link the
  internals; no public API is added (`#[doc(hidden)]` wrappers only).

- **Repo hygiene & metadata**: `.editorconfig`, `.gitattributes` (LF
  normalization + language-stats fix so the repo reads as Rust, not HTML),
  `rust-toolchain.toml` (stable + rustfmt/clippy for contributors),
  `SUPPORT.md`, and a release-notes template (`.github/release.yml`) that
  groups generated notes by PR label. Cargo.toml now declares
  `rust-version = "1.87"` (measured with `cargo msrv find`) plus
  keywords/categories/homepage, and a new CI `msrv` job builds with exactly
  that toolchain. The Docker build base is digest-pinned. README gains the
  OpenSSF Best Practices badge and a contributing/security/support section.

- **CodeQL static analysis** for the Rust source on every PR, push to main,
  and a weekly re-scan (`build-mode: none` — no cargo build needed).
- **Workflow lint job in CI**: `actionlint` (correctness, always gates) and
  `zizmor` (Actions security lint; every severity is uploaded to code
  scanning, high-severity findings fail the build).
- **Dependency review on PRs**: introducing a crate with a known
  vulnerability now fails the PR (licenses stay `cargo-deny`'s job).
- **Weekly advisories audit** (`audit.yml`): the lockfile is checked against
  the RUSTSEC database on a schedule, so a new advisory surfaces within a
  week instead of at the next push.

### Changed

- Upgraded the CodeQL Action from v3 to v4 (both `codeql.yml` and the
  Scorecard SARIF upload), clearing the Node 20 deprecation and the
  December-2026 v3 sunset warnings.

- **CodeQL scope**: a config file (`.github/codeql/codeql-config.yml`) now
  excludes the `tests/**` and `fuzz/**` trees, so the hard-coded-secret
  queries fire on the operator-facing source but not on intentional test
  fixtures (throwaway passwords, RFC-vector salts). The handful of fixture
  alerts inside `#[cfg(test)]` modules in scanned source are dismissed as
  "used in tests".

- The release workflow now runs under a global concurrency group (one release
  at a time, queued rather than cancelled), and the `prepare` script takes
  workflow-context values via `env` instead of inline template expansion.

- **Workflow hardening to the OpenSSF-recommended baseline**: every GitHub
  Actions step is pinned to a full commit SHA (Dependabot keeps the pins
  fresh); all CI/release jobs start with `step-security/harden-runner` egress
  monitoring (audit mode); checkouts that don't push drop their credentials
  (`persist-credentials: false`); and a weekly OpenSSF Scorecard workflow
  publishes the repo's supply-chain score to code scanning and the README
  badge.

## [0.6.2] - 2026-07-04

CI/release infrastructure release — no proxy behavior changes.

### Changed

- **Release images build on native runners in parallel**: amd64 on
  `ubuntu-latest` and arm64 on `ubuntu-24.04-arm`, each pushed by digest and
  stitched into one multi-arch manifest; the cosign signature, provenance
  attestation, and SBOM now target the manifest digest. This removes the
  QEMU-emulated arm64 Rust compile that made releases take ~30 minutes.
  Buildx layer caching added to the release and CI image builds.
- CI runs superseded by a newer push to the same ref are cancelled
  (concurrency groups; main is never cancelled), and the CI image smoke test
  no longer sets legacy env vars retired in 0.6.0.

## [0.6.1] - 2026-07-04

Maintenance release — no proxy behavior changes; it exists to ship and
validate the new release automation.

### Changed

- **Releases can be cut from the Actions UI** (`workflow_dispatch` on the
  Release workflow): a new `prepare` job resolves the version from Cargo.toml
  on the default branch, refuses if that tag already exists, mints and pushes
  the `v*` tag itself, and the same run carries the release end-to-end — no
  local `git tag`/`git push` needed. The tag-push path still works and keeps
  its tag-must-match-Cargo.toml guard; image tags and the GitHub Release tag
  now come from the resolved version rather than the triggering git ref.

## [0.6.0] - 2026-07-04

> **Breaking (v0.6.0):** app-level configuration moved from env vars into a
> UI-managed store. `NIM_API_KEYS`, `PROXY_API_KEYS`, `ADMIN_PASSWORD`,
> `INSECURE_NO_AUTH`, `NIM_BASE_URL`, `RPM_PER_KEY`, `MAX_WAIT_SECS`,
> `HEARTBEAT_SECS`, `MODELS_TTL_SECS`, `STREAM_IDLE_SECS`,
> `REQUEST_TIMEOUT_SECS`, `STRICT_PASSTHROUGH`, `REF_PRICE_IN`/`REF_PRICE_OUT`,
> `HISTORY_DAYS`, and `MAX_INFLIGHT` are **ignored** (a one-line boot warning
> lists any still set). Configure everything in the dashboard on first run. The
> dashboard is now multi-user (username + password), and `INSECURE_NO_AUTH` is
> replaced by an `open|keyed` API-access mode that affects only `/v1`. There is
> no migration (there were no deployments to migrate).

### Added

- **UI-managed config store + first-run setup wizard**: app-level config lives
  in `DATA_DIR/config.json` (version 1, atomic writes, 0600), edited from a new
  dashboard **Settings** area (sub-nav: Access & keys · Server · Users ·
  Account) and claimed by a 3-step wizard (create superuser → add ≥1 NIM key,
  validated live against the upstream → finish, logged in). A corrupt/unreadable
  or future-version store is a hard boot error, never a silent fall-through to
  setup. JSON not SQLite — see
  `knowledge/decisions/ui-managed-config-store.md`.
- **Multi-user with roles & per-key ownership**: `superuser` (an admin that can
  never be deleted), `admin` (server settings + user management), `user` (own
  account, own client keys, own NIM keys). Dashboards are identical for every
  role; `GET /api/config` is filtered server-side so hidden sections are absent
  from the payload, not CSS-hidden. Sessions carry the username plus a fragment
  of the password hash, so a password change/reset invalidates that user's
  sessions instantly and role changes/deletion apply on the next request.
  Passwords are PBKDF2-HMAC-SHA256 (600k iterations, RFC 7914 vectors). See the
  v0.6.0 amendment in
  `knowledge/decisions/auth-posture-and-dashboard-password.md`.
- **Per-key rpm and live key management**: each NIM key has its own rpm
  (default 40, range 1–10000), an owner, and an enable/disable toggle; the pool
  rebuilds live on any change with rate-state carryover (kept keys keep their
  in-window counts; disabled keys re-enable warm). The superuser always owns ≥1
  enabled key (the pool floor). Client API keys are server-generated 128-bit
  secrets with an `npk_` prefix, shown exactly once and stored only as SHA-256
  digests (+ last-4 for display).
- **Model-pressure governor**: classifies NIM's per-model worker-concurrency
  exhaustion (`Worker local total request limit reached`) apart from plain 429s
  and backs off the **model** (never benches the lane, since key failover can't
  help). Adaptive and zero-config (engages at half observed in-flight, +1 per
  stable minute, dissolves after 30 clean minutes) with optional per-model
  pinned caps in Settings. New metrics `nimproxy_worker_exhausted_total{model}`,
  `nimproxy_model_inflight{model}`, `nimproxy_model_limit{model}` (0 =
  ungoverned), and a Reliability **Model pressure** card that appears only once
  the governor has engaged. See `knowledge/architecture/governor.md`;
  `mock_nim.py` gained `--worker-slots N` and `loadtest.py` reports worker
  exhaustions + peak per-model concurrency.
- **Redesigned dashboard**: a dark, NVIDIA-green "operator console" — left
  sidebar nav (collapses to an icon rail below 860px), top bar with range
  pills, Space Grotesk + Spline Sans Mono webfonts. Five persona-aligned tabs
  (`Overview · Models · Clients · Reliability · Capacity`), richer KPI cards
  with trend delta chips and sparklines, ring gauges, and a Reliability hero
  (availability vs a 99.9% SLO, a "where time goes" latency breakdown) and a
  Capacity hero (saturation bar, keys-for-peak provisioning chip). Every line
  chart now has a hover crosshair with a per-series tooltip, and every table
  is click-to-sort with a sticky header and internal scroll — sort order and
  scroll position both survive the 3s live refresh. See
  `knowledge/decisions/dashboard-operator-console-redesign.md`.
- **The wizard mints your first client key**: setup ends on a connect panel
  with the client base URL and a once-only `npk_` secret, so a fresh
  keyed-mode proxy serves `/v1` with no Settings detour. On by default;
  opting out shows an explicit warning (keyed with zero keys rejects every
  `/v1` call until a key exists).
- **New dashboard charts** for signals that were collected but never drawn:
  requests-by-outcome over time (Reliability), requested output budget per
  harness from `nimproxy_request_max_tokens` (Clients), and tool-call volume
  per model from `nimproxy_tool_calls_total` (Models).

### Fixed

- **Streaming requests now count against `max_inflight` for their whole
  lifetime.** The in-flight guard previously dropped when the response headers
  were returned, so the cap only bounded buffered requests — a flood of live
  streams could exceed it unbounded.
- **A client disconnect during a blocked upstream read is noticed
  immediately.** The streaming relay now races each upstream read against the
  client channel closing, so a hang-up frees the request's `max_inflight`
  slot at disconnect time instead of at the `stream_idle` cutoff — and a hung
  upstream can no longer pin a slot until restart when `stream_idle` is 0.
- **Own-password change guards against a concurrent admin reset.** The change
  commits only if the stored hash is still the one the current password was
  verified against; a reset landing in the verify window now wins with a 409
  instead of being silently overwritten by the stale change.

### Changed

- **Env shrinks to 5 container-level vars** (`HOST`, `PORT`, `DATA_DIR`,
  `RUST_LOG`, `TRUST_PROXY`); `DATA_DIR` must be writable (it now holds the
  credential store as well as history) and an unwritable dir is a hard boot
  error. `.env.example`, README, and the runbooks are rewritten to match; the
  quickstart is now `docker compose up` → open the dashboard → complete the
  wizard.
- **Dashboard auth is now user-based.** Login takes a username and password;
  the single `ADMIN_PASSWORD` gate is gone. Prometheus scrapers authenticate as
  `Authorization: Bearer <username>:<password>` (or HTTP Basic). Volume backups
  now contain credentials (`config.json`, 0600) — treat them as secrets.
- `docker compose up` now runs the published `ghcr.io/miztertea/nim-proxy:latest`
  image instead of building from source; source builds move to an explicit dev
  override (`docker-compose.dev.yml`, tagged `nim-proxy:dev`). README,
  CONTRIBUTING, and the deploy runbook updated to match.
- **CSP** now allows the dashboard's webfonts: `style-src` gained
  `https://fonts.googleapis.com`, and a new `font-src` allows
  `https://fonts.gstatic.com`. Falls back to system fonts if the CDN is
  unreachable.

### Removed

- **All app-level env vars** (see the breaking note above) — they're ignored,
  with a one-line boot warning listing any still set. No seed-from-env, no
  migration.
- **`INSECURE_NO_AUTH`.** Replaced by the store's `open|keyed` API-access mode,
  which governs only `/v1`; every dashboard/observability surface always
  requires a logged-in session post-setup.
- **Light mode.** The dashboard is dark-only now; the light palette and
  `prefers-color-scheme` handling were deleted as a committed design choice.
- **The Compare tab** — its head-to-head scorecard and generation-speed bar
  race are now a section of the Models tab.
- **The heatmap's table-view toggle** — not part of the redesign; the heatmap
  keeps its per-cell hover tooltips.

## [0.5.0] - 2026-07-03

First public release: the repository is now public, and this tag publishes the
first signed multi-arch container image to GHCR with SBOM and build provenance.

### Fixed

- **Unauthenticated panic in the login handler.** A percent-escape followed by a
  multibyte UTF-8 character (e.g. `password=%€`) in the `POST /login` body sliced a
  `&str` on a non-char boundary and panicked. Percent-decoding is now byte-safe.
- **No timeout on non-streaming upstream reads.** A buffered request whose upstream
  sent headers then stalled the body could hang forever, pinning an in-flight slot.
  Non-streaming requests now honor `REQUEST_TIMEOUT_SECS` (default 300s) and surface a
  `502` on a stalled/failed body read. Streaming still uses `STREAM_IDLE_SECS`.
- **`RPM_PER_KEY=0` wedged the dispatcher** (out-of-bounds index in the pacer). Now
  rejected at startup.
- Login throttle window uses saturating subtraction (robust to clock adjustments).

### Added

- `REQUEST_TIMEOUT_SECS` config (default 300).

### Changed

- Regression tests for all of the above; coverage raised to ~90%.

### Performance

- Build with `opt-level = 3` (was `"z"`): the release profile optimized for size,
  throttling the JSON-parse and SSE-scan hot paths. Binary grows ~3.5→4.6 MB.
- Drop a deep clone of the whole request body on the streaming injection path
  (move it instead); use `Bytes::from_static` for the SSE control frames.
- Routine `cargo update` (`rustc-hash` patch).

### Dependencies

- Bump `metrics-exporter-prometheus` 0.17 → 0.18 and refresh CI/release action
  versions, including the Node 24 runtime wave (gitleaks-action v3, the docker/*
  build actions, download-artifact v8).
- Hold the auth crypto/RNG stack (`hmac` 0.12, `sha2` 0.10, `getrandom` 0.2) on
  the proven-stable line — the proposed 0.13/0.11/0.3 majors are breaking with no
  security fix; Dependabot is configured to only take patches for these.

## [0.4.0] - 2026-07-02

The proxy becomes a **benchmarking and agent-observability tool**: because it
sits in the request path for every harness and model, it can now report *how*
each agent behaves and *how well* each model responds — all from counts and
sizes, never message content.

### Added

- **Request-shape & response-quality metrics**, captured from the request path
  that was already deserialized and scanned: conversation depth, tools offered,
  sampling temperature, `max_tokens`, stream-vs-buffered and JSON-mode mix
  (labeled by client/harness), plus finish-reason/truncation, tool calls,
  reasoning ("thinking") tokens, and mean TPOT (labeled by model). Everything is
  bounded-cardinality with server-clamped enums — counts and sizes only, never
  content. See `knowledge/decisions/request-shape-metrics.md`.
- **Six persona-aligned dashboard views** (Overview, Models, Compare, Harnesses,
  Proxy, Keys), rebuilt from the previous three tabs, each ordered
  at-a-glance → trends → detail, in light and dark mode. Adds a head-to-head
  model scorecard, per-harness fingerprints, and a hash-to-hue color fallback
  past the six categorical slots.
- Generation-speed (tok/s) median/p95 trend, a ranked non-success-outcome
  breakdown, and threshold-colored capacity/success-rate gauges.
- Example [`examples/opencode.json`](examples/opencode.json) config tuned for
  GLM-5.2 (context, compaction, sampling), with rationale in
  `examples/README.md`.

### Changed

- Test coverage extended to the buffered `relay()` quality path, an
  unknown-`finish_reason` → `other` clamp, JSON mode, and non-`auto`
  `tool_choice` — now **29 unit + 21 e2e** tests.
- Load harness gained tool/JSON/sampling variety and a corrected boot command
  (`INSECURE_NO_AUTH`); re-run clean at 240 requests, 0 failures, 0 upstream
  rate violations, balanced across all keys.

### Security

- Pre-merge hardening pass: a dedicated dashboard-XSS audit plus a full security
  review of the branch found **zero** vulnerabilities — every new `innerHTML`
  value is escaped, every new label is a bounded enum/histogram, and no route
  left the admin gate.

## [0.3.0] - 2026-07-02

Security-hardening release. A review of the merged proxy found a stored-XSS
chain, unbounded metric-label cardinality, log injection, and an open-by-default
posture. All fixed.

### Added

- **Fail-closed auth.** The proxy refuses to start on a network-reachable port
  without auth. Secure mode requires `PROXY_API_KEYS` (gates `/v1/*`, any key
  works, constant-time compare) and `ADMIN_PASSWORD` (gates the dashboard,
  `/metrics`, and `/api/history` via an HMAC-signed, HttpOnly, SameSite=Strict
  session cookie; Bearer/Basic for scrapers). Open mode is an explicit
  `INSECURE_NO_AUTH=true` opt-in. See
  `knowledge/decisions/auth-posture-and-dashboard-password.md`.
- Failed-login throttle, a rejected-API-key delay, and a `MAX_INFLIGHT`
  flood-shedding cap.
- `cargo audit` in CI.

### Security

- **Input sanitizing.** Client-controlled `model`/`path` labels are sanitized to
  a conservative charset, length-capped, and cardinality-bounded at ingest —
  killing the exposition/log-injection and cardinality-blowup vectors. The
  dashboard `esc()`-escapes every dynamic `innerHTML` sink, and all responses
  carry a strict `Content-Security-Policy` plus anti-framing/anti-sniffing
  headers. See `knowledge/decisions/input-sanitizing-and-xss.md`.
- Compose now publishes `127.0.0.1:8000:8000` (loopback) by default, so a bare
  `docker compose up` can't accidentally expose an open instance.
- Verified with a real-browser XSS check (payload rendered inert), a secure-mode
  load test (300/300, 0 rate violations), and a clean `cargo audit`.

## [0.2.0] - 2026-07-02

Observability and hardening enrichments on top of the core proxy.

### Added

- **Prometheus `/metrics`** exposition and optional client access keys
  (`PROXY_API_KEYS`) for per-client attribution.
- **Built-in dashboard** — a single embedded HTML file (no Grafana, no config) —
  plus an ASCII boot banner, structured startup detail, one-line-per-request
  access logs (TTY-detected ANSI color), and a self-probe healthcheck
  (`nim-proxy --health`) that works `FROM scratch`.
- **Metrics history**: a ~4 KB snapshot every 5 minutes, retained `HISTORY_DAYS`
  days, powering time-range reports (1h/6h/24h/7d/30d + custom) that survive
  restart.
- Model cards with id-namespace enrichment (the `/v1/models` schema research
  killed the idea of API-sourced descriptions).

### Changed

- Proxy hardened and given a full test suite (unit + e2e against a scripted mock
  NIM) and a load harness (`scripts/mock_nim.py --enforce` + `scripts/loadtest.py`).
- The `knowledge/` Open Knowledge Format bundle was compiled: design decisions,
  validated NIM research, architecture notes, and runbooks.

### Fixed

- Docker build on the musl-host Alpine builder: pass an explicit `--target` so
  global `crt-static` RUSTFLAGS skip proc-macro dylibs.

## [0.1.0] - 2026-07-01

Initial rate-limit-aware proxy.

### Added

- OpenAI-compatible pass-through to NVIDIA NIM with **per-key sliding-window
  rate limiting** (40 requests per rolling 60 s, matching NIM's limiter) and
  multi-key load balancing.
- **Global FIFO dispatcher** — one queue for all clients, slots granted strictly
  in arrival order, abandoned-waiter slots returned — for fair multi-client
  allocation.
- **Conversation affinity with least-loaded spillover**: each conversation pins
  to one key to keep the server-side prefix cache warm, spilling to the
  least-loaded ready lane when its lane is full.
- **Distroless image**: a static musl binary shipped `FROM scratch` (~3.5 MB,
  TLS roots compiled in), running non-root with hardened compose defaults.

[Unreleased]: https://github.com/miztertea/nim-proxy/compare/v0.6.6...HEAD
[0.6.6]: https://github.com/miztertea/nim-proxy/compare/v0.6.5...v0.6.6
[0.6.5]: https://github.com/miztertea/nim-proxy/compare/v0.6.4...v0.6.5
[0.6.4]: https://github.com/miztertea/nim-proxy/compare/v0.6.3...v0.6.4
[0.6.3]: https://github.com/miztertea/nim-proxy/compare/v0.6.2...v0.6.3
[0.6.2]: https://github.com/miztertea/nim-proxy/compare/v0.6.1...v0.6.2
[0.6.1]: https://github.com/miztertea/nim-proxy/compare/v0.6.0...v0.6.1
[0.6.0]: https://github.com/miztertea/nim-proxy/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/miztertea/nim-proxy/releases/tag/v0.5.0
[0.4.0]: https://github.com/miztertea/nim-proxy/releases/tag/v0.4.0
[0.3.0]: https://github.com/miztertea/nim-proxy/releases/tag/v0.3.0
[0.2.0]: https://github.com/miztertea/nim-proxy/releases/tag/v0.2.0
[0.1.0]: https://github.com/miztertea/nim-proxy/releases/tag/v0.1.0
