---
type: Log
title: Knowledge base chronology
description: Append-only record of ingests, decisions, and maintenance passes.
---

# Log

## [2026-07-29] decision — message catalog and the escape-once contract

Third change of the 0.6.6 rationalization. Extracted the dashboard and setup
wizard to an embedded `en-US` catalog (159 messages) behind a `t()` runtime.
Recorded in [message-catalog-and-escaping](decisions/message-catalog-and-escaping.md).

- Removed the file's only JavaScript-context interpolation before extracting
  anything. `chipHtml` built an `onerror="..."` attribute containing a JS
  statement containing single-quoted JS literals, safe only because
  `initialsOf()` strips to `[A-Za-z0-9 ]` — a monogram helper, not a
  sanitizer. One catalog candidate reached it.
- Catalog values are plain text, escaped once at load. This is not a style
  choice: `metricRow`, `perfBlock`, `tile`, and `prow` interpolate their label
  into `innerHTML` with no `esc()`, and `kpiCards` escapes `k.label` but not
  `k.value`/`k.sub`. Escaping at load covers all of them at one point.
- Lint: two chart hover handlers declared `const t` for a cursor timestamp,
  which shadowed the new global `t()`. Renamed the locals to `at`. Nothing was
  broken yet — no `t()` call sat inside those scopes — but the next one added
  there would have failed silently, and `cargo test` cannot see it.
- Adversarial review found a user-visible bug the tests could not: `applyStatic`
  ran after the tab-restore loop, so deep-linking to any non-Overview tab showed
  that section under a topbar reading "Overview". It also found `sortTable`
  double-escaping every catalog column header, four blind spots in
  `check_i18n.py`, `locales/*.json` validated by nothing, an unenforced
  attribute allowlist, six more `t` shadows, and a dozen half-extracted
  surfaces. All fixed in-PR; each new check proven to fail on a broken input.
- Verification is `scripts/check_i18n.py` plus `node --check` on the extracted
  script bodies plus a headless-Chromium render that mutates three catalog
  values and confirms the DOM follows. The last one matters: `cargo test`
  passes on JavaScript that does not parse, and it did — an earlier pass wrote
  `${...}` into single-quoted strings and only `node --check` caught it.

## [2026-07-29] ingest — standard vocabulary across the interface

Second change of the 0.6.6 rationalization. Applied the agreed standard
ops-dashboard vocabulary to `src/dashboard.html` and `src/setup.html`:
`Harness` → `Client`, dashboard `window` → `time range`, `lane` → `key`,
`Conversation stickiness` → `Session affinity`, `Model-pressure governor` →
`Model limits`, `Where time goes` → `Latency breakdown`, `Rate-limit
pressure` → `Throttling`, `Keyed` → `API key required`, and the composite
`Shed · 401 · failed logins` row split into three.

Display strings only. Every identifier held: DOM ids, CSS classes, `data-*`
attributes, metric names, and the `sortTable` state keys — where `harness`
and `clients` both exist, so renaming that argument would have silently made
the two tables share sort state.

- Lint: the interface now says **key** while the Prometheus exposition still
  says `lane` (`nimproxy_lane_requests_total`, and the `lane` label on
  `nimproxy_lane_cooldown_total`). That divergence is deliberate — renaming a
  second series would be another breaking change, and 0.6.6 already carries
  one. `README.md`'s architecture section still says "lane" for the same
  reason: it describes the code, which has not been renamed.
- `knowledge/architecture/dashboard.md`, `ops/configure-env.md`, and the
  README dashboard-tour section were updated to name the new labels; the
  `agent harness` prose describing what OpenCode/Codex *are* was deliberately
  left alone.
- Adversarial review caught the one dangerous rename: the Access &amp; keys chip
  reads `${k.in_window} / ${k.rpm}`, which is the **rate-limit** rolling
  window, and a blanket `window` → `time range` pass had turned it into
  "in range". That is the single place the two meanings had to stay apart, and
  it is also factually wrong — reverted. Review also found five dashboard
  `window` strings the pass missed (so two empty-state vocabularies were
  visible at once), a `Per lane` heading left sitting above a `Key` table, a
  `lane N` chip beside `Slot N`, `Peak shortfall` rendering without its rpm
  unit, and a dead `sul` binding orphaned by the composite-row split.

## [2026-07-29] decision — lane cooldown naming, savings metric removed

First change of the 0.6.6 presentation-layer rationalization.

- Retired the `bench` idiom for the post-backoff lane state in favor of
  **cooldown**, which the `cooldown_until` field already used. Renamed across
  `src/`, `tests/`, `scripts/`, `knowledge/`, and the `README.md` metrics table;
  `nimproxy_lane_benched_total` → `nimproxy_lane_cooldown_total`;
  `proxy::bench` → `proxy::enter_cooldown`. Recorded in
  [lane-cooldown-naming](decisions/lane-cooldown-naming.md), including why a
  read-time history alias was rejected in favor of a bounded, documented gap.
- Removed the estimated-savings metric and everything feeding it: the
  `Dollars saved` KPI, three `Saved` table columns, `money()`, the `Pricing`
  settings card and config block, `/api/settings/pricing`, and the pricing
  validation branch. Recorded in
  [no-estimated-savings-metric](decisions/no-estimated-savings-metric.md).
  `REF_PRICE_IN`/`REF_PRICE_OUT` deliberately stay in the legacy-env warning
  list. New regression test proves a 0.6.5 store carrying a `pricing` block
  still loads.
- Lint: `knowledge/architecture/key-pool.md` and `governor.md` described the
  state as "benching" while `pool.rs` already named the field `cooldown_until`
  — the pages and the code now agree. Adversarial review of the change also
  caught `README.md` and `architecture/dashboard.md` still advertising the
  deleted savings KPI and sparklines, and both breaking-change notes naming
  `/api/dashboard` where the removed fields were actually emitted by
  `/api/dashboard/now`; all corrected in-PR. Removing the savings card
  orphaned the `valColor` and `green` options on `kpiCards`/`sparkSvg` — it
  was their only caller — so the unreachable branches went with it.

## [2026-07-28] ingest — prepare v0.6.5 maintenance release

- Promoted the accumulated maintenance, dashboard-history corrections, and
  security notes from Unreleased into v0.6.5; synchronized the crate/lockfile
  version and changelog comparison links.

## [2026-07-28] lint — remove duplicate planning and prototype documents

- Removed `docs/superpowers` and the root `design/` prototype handoffs;
  durable design decisions and operational facts remain in the project
  knowledge graph instead of parallel planning archives.

## [2026-07-28] decision — reset-aware dashboard history

- Replaced browser-local lifetime and cross-boot subtraction with a
  server-side reset-aware history index, one selected analytical window,
  separately configured retention/default window, and lightweight current
  polling.

## [2026-07-28] ingest — bound capacity to observed history

- Clarified that a partial default window keeps the first retained sample's
  exact totals without inventing duration or capacity in the unavailable
  prefix; saturated retention arithmetic also keeps extreme valid values safe.

## [2026-07-28] ingest — synchronize dashboard history documentation

- Updated the dashboard, metrics-history, configuration, retention, and auth
  pages to match the typed range/current contracts, revision-bound tail,
  sample-time capacity, live Settings behavior, boot-read file policy, and
  atomic compaction boundary.

## [2026-07-28] lint — correct history sizing premise

- Corrected the disproven fixed snapshot-size estimate using the observed
  235,598,655-byte production history as workload evidence, not a replacement
  universal sizing formula.

## [2026-07-28] ingest — publish GitHub Releases with the runner CLI

Replaced `softprops/action-gh-release` with the GitHub-hosted runner's
preinstalled `gh release create`. The release job still uses the prepared tag,
prepends the container and Sigstore verification instructions to generated
notes, and uploads the same signed tarballs and SBOM assets, while removing
one third-party action from the release trust surface.

## [2026-07-28] ingest — harden workflow inputs and parameterize Compose publishing

Moved release metadata and digest values out of shell-script template
expansions and into step-scoped environment variables, preserving the release
pipeline while removing seven template-injection findings. Added a
`PUBLISH_HOST` Compose interpolation with a loopback default so intentional
LAN exposure lives in ignored `.env` deployment state rather than a tracked
`docker-compose.yml` edit.

## [2026-07-28] decision — delay routine dependency updates for seven days

Applied one seven-day Dependabot cooldown to Cargo, GitHub Actions, and Docker
version updates. The explicit observation window follows zizmor's
supply-chain recommendation without delaying security updates, which
Dependabot exempts from cooldowns.

## [2026-07-28] ingest — repair Cosign v3 release asset signing

The first v0.6.4 release attempt built and signed the multi-arch image but
failed before publishing the GitHub Release. `cosign-installer` 4.1.2 had
changed its default CLI from Cosign v2 to v3 while the release job retained
the legacy `.sig` + `.pem` `sign-blob` flags. Migrated release assets and
verification instructions to `.sigstore.json` bundles, explicitly pinned the
Cosign CLI separately from the installer, and added a real offline
sign/verify contract smoke test to the workflow-lint gate.

## [2026-07-17] ingest — prepare v0.6.4 release metadata

Promoted the accumulated deadline, security, cleanup, and dependency entries
from Unreleased into v0.6.4; bumped the crate and lockfile package version; and
repaired the stale changelog comparison links so v0.6.3 and v0.6.4 each have a
complete release range.

## [2026-07-16] lint — low-risk cleanup batch (dead async, redundant clones)

Staged a tight, YAGNI-scoped cleanup batch on a sub-branch off the
`claude/dependabot-pull-requests-2z8240` integration branch (PR into it, not
into `main`). Scope was chosen for highest signal / least risk: (1) removed a
redundant `async` on `proxy::streaming` — every `.await` lives inside its
`tokio::spawn`ed task, so the function body only spawns and returns a
`Response`; dropping `async` avoids a pointless future wrapper and the single
caller drops its `.await`; (2) removed two redundant `String` clones on the
nim-key / client-key add paths (the value was moved into the struct, not
reused); (3) `clone_from` buffer reuse when re-owning orphan keys during
superuser claim. Deliberately excluded as churn/net-negative: the 13
`redundant_closure_for_method_calls` rewrites (`|x| x.as_u64()` →
`serde_json::Value::as_u64`) which are longer and less readable, the
`clone_into` inversions on cold config-write paths, and adding a new
`[lints.clippy]` gate (not requested; nursery lints risk future CI
false-positives). Verified: fmt clean, `clippy --all-targets -D warnings`
clean, lib 84 + e2e 72 tests green.

Follow-up (same PR): CI's `fmt, clippy, tests` job went red — not on the
cleanup, but on pre-existing code. Rust stable rolled 1.94 → 1.97 on
2026-07-14, and 1.97's improved `clippy::question_mark` now flags the
`else if let Some(basic) = … else { return None }` shape in `auth::identify`
under `-D warnings`. The Dependabot PRs (#47–49) had passed this job because
they ran on 2026-07-09, before the toolchain bump; this PR was the first to
run CI afterward, so it surfaced here. Applied clippy's own `?`-operator
rewrite (behavior identical, auth tests cover it). Reproduced locally by
`rustup update stable` to 1.97.1 before and after the fix. Because #52's head
is the integration branch, merging this PR into it also clears the same
failure for #52.

## [2026-07-16] decision — opt-in absolute request deadlines

Rambler's model tournament showed a buffered request continuing inside the
proxy for 825 seconds after its client timed out. Root cause: `max_wait`,
`request_timeout`, and `stream_idle` bound individual phases, while buffered
handlers cannot reliably observe a downstream disconnect before producing a
response. Added `X-Nim-Proxy-Deadline-Ms` as an opt-in absolute clock across
admission, retries, and generation. Expiry drops the request workflow and its
RAII-owned resources; buffered callers receive `504 deadline_exceeded`, while
streams receive a best-effort terminal SSE error. Status `deadline` and
`nimproxy_deadline_exceeded_total` make the outcome independently visible.

## [2026-07-16] lint — crossbeam-epoch advisory fix (RUSTSEC-2026-0204)

`cargo-deny`'s advisories check went red on `main` — and therefore on every
open Dependabot PR (#47 bytes, #48 actions group, #49 cosign-installer) — after
RUSTSEC-2026-0204 was published against `crossbeam-epoch` < 0.9.20 (invalid
pointer dereference in the `fmt::Pointer` impl for `Atomic`/`Shared`). It is a
transitive dep via `metrics-util` → `metrics-exporter-prometheus`, not a direct
one. Bumped the `Cargo.lock` entry to 0.9.20 (the advisory's recommended fix) —
a single-package lockfile change, no `Cargo.toml` edits. Staged on the
`claude/dependabot-pull-requests-2z8240` integration branch pending a decision
on batching further fixes vs. cutting a release.

## [2026-07-05] v0.6.3 — release-asset signing + CodeQL fixture-noise triage

Maintenance release closing two loose ends from the rigor pass.

- **Release assets are signed** (`cosign sign-blob`, keyless): the `release`
  job now signs each downloadable tarball and the SBOM, attaching a `.sig` +
  `.pem` per asset (needed `id-token: write` on the job). The container
  manifest was already cosign-signed; this extends verifiability to a binary
  pulled straight from the Releases page. Feeds Scorecard's Signed-Releases
  lever. Release notes carry the `cosign verify-blob` command.
- **CodeQL hard-coded-secret triage**: the 5 Critical `rust/hardcoded-
  cryptographic-value` alerts were all false positives — test fixtures (fake
  passwords, RFC-7914 vector salts) plus one scratch buffer (`let mut salt =
  [0u8; 16]` in `hash_password`, immediately overwritten by `getrandom`, but
  the extractor doesn't model the `&mut` overwrite). Added
  `.github/codeql/codeql-config.yml` with `paths-ignore: [tests/**, fuzz/**]`
  (verified: honored for Rust under `build-mode: none`) to kill the separate
  test-crate alert and prevent future fixture noise. The 4 alerts inside
  `#[cfg(test)]` modules in scanned `src/` can't be path-excluded without
  dropping the whole file — dismissed in the code-scanning UI as "used in
  tests" / false-positive. Deliberately did NOT `query-filter` the rule
  globally: it must keep firing on a real hard-coded key in shipped code.
- Fixed two off-by-a-minute cron comments (audit.yml 06:42→06:43,
  scorecard.yml 07:27→07:28) — comments now match the actual cron minute.
- **Coverage expansion 91.4%→96.1% lines** (gate raised 80→90). Applied the
  YAGNI gates to eliminate a planned clock/injection seam: the throttle
  window-rollover branch is reachable by setting `Throttle.window_start`
  directly from an in-module test, and the password-change HashRotated/UserGone
  logic was *already* unit-tested — so no production code changed. Wave 1: pure
  in-module unit tests (auth primitives → `auth.rs` 100%; `config::validate`
  branches; `parse_role`; SSE 1 MiB guard; history load + compaction). Wave 2:
  e2e legs via the existing harness (setup double-claim/orphan-adoption/throttle,
  key-probe non-success + unreachable, client/nim-key/user validation +
  ownership, auth Basic/logout/login redirects). A second blind auditor then
  showed several "excluded" filesystem/boot paths were in fact cheaply and
  deterministically reachable with tricks already used in the harness, so those
  WERE added (round 2): `config` serde-defaults + unreadable-store (invalid
  UTF-8), `history` dir-create/file-open/write failures (dir-as-file trick),
  `lib` empty-`DATA_DIR` and the `--health` probe (subprocess exit codes), and
  the `setup` commit `invalid_config` leg. Genuinely left uncovered (documented
  residual): every handler's `role_of==None` stale-session arm and account's
  own None/commit arms — a REAL TOCTOU race (the auth middleware validates the
  session under one store-lock and releases it; each handler re-locks
  separately, and a concurrent user-deletion must land in that window), not
  deterministically triggerable through the black-box harness without a
  test-only sync hook (the pure logic — `apply_password_change`'s
  UserGone/HashRotated — is already unit-tested); `lib.rs` banner/`tracing`
  logging, `warn_legacy_env`, and the unused `GovernorSettings::default`;
  `tracing!` argument lines the test subscriber never evaluates; and the proxy
  request-flow branches (streaming/models/relay/buffered — Wave 3, out of
  scope this release).
- **PR template** rewritten into a standard, agent-legible form (typed
  sections + a checklist whose conditional groups name their trigger, so an
  agent pulling the template sees which gates apply). Requirements sourced from
  CONTRIBUTING.md + AGENTS.md.
- **Doc-consistency lint** (agent sweep) fixed post-rigor-pass drift: SECURITY
  said `cargo audit` runs in CI (it's `cargo-deny`, self-contradicting the same
  file); release.md's required-checks list and rulesets were stale (missing
  msrv/workflow-lint/dependency-review/codeql, and marked "not yet applied"
  when both `main` and `v*` rulesets are live); the test-strategy page had no
  fuzz layer and an incomplete CI description; CONTRIBUTING framed the gate set
  as "three"; README lacked a supply-chain section and called testing "three
  layers"; bug_report.yml still placeheld `0.4.0`. Test counts (69+53) and all
  internal doc links verified clean — no change needed.

## [2026-07-04] ingest — repo-rigor pass 3: fuzzing the untrusted-byte parsers

- **cargo-fuzz harnesses** for the three surfaces that parse bytes we don't
  control: `SseScan::feed` (upstream SSE arrives arbitrarily fragmented —
  fed whole AND re-fragmented at an input-derived chunk size, asserting the
  1 MiB pathological-line guard), `sanitize_label` (asserting the output
  invariants that ARE the metric-injection defense: non-empty, ≤64 chars,
  safe charset), and the `StoredConfig` JSON round-trip (operator-edited
  file: parse never panics, serialize→parse→serialize is a fixpoint).
- **Crate restructure**: src/main.rs became a 3-line shim over src/lib.rs
  (`nim_proxy::run()`) so the fuzz crate can link the internals. Modules
  stay private; the fuzz surface is `#[doc(hidden)]` wrapper fns re-exported
  as `fuzz_proxy`/`fuzz_config`. All `crate::` paths survived the move
  unchanged; 69 unit + 53 e2e tests unaffected.
- **fuzz.yml**: weekly + dispatch + PR-path-filtered smoke pass (60 s per
  target, nightly via explicit `cargo +nightly` which outranks
  rust-toolchain.toml); crash reproducers upload as artifacts on failure.
  Deliberately not a required merge check. ClusterFuzzLite deferred —
  OSS-Fuzz scaffolding is disproportionate; escalate if Scorecard doesn't
  credit in-repo cargo-fuzz or a target finds a real bug.
- Seed corpora committed under `fuzz/seeds/` (real SSE shapes incl.
  truncated mid-JSON, hostile label bytes, a full store.json), marked
  `binary` in .gitattributes so eol-normalization can't corrupt them.
  `fuzz/corpus/` is the gitignored working corpus — a local run generates
  thousands of evolved entries that must never be committed.

## [2026-07-04] gotcha — pin the PEELED commit SHA for annotated tags

Bumping CodeQL Action v3→v4 broke the Scorecard `publish_results` step:
`400 … imposter commit: 8533807f…`. Cause: the bulk `git ls-remote 'refs/tags/v4*'
| grep -v '^{}'` dropped the peeled entries, so for github/codeql-action's
**annotated** tags it returned the tag-OBJECT SHA, not the commit. GitHub
Actions dereferences a tag-object SHA silently (init/analyze ran green), but
Scorecard's imposter-commit check rejects any pin that isn't a real commit.
Fix: pin the `refs/tags/vX^{}` peeled commit SHA
(`54f647b7…` for v4.36.3). Rule going forward: always resolve pins with
`git ls-remote <repo> refs/tags/TAG 'refs/tags/TAG^{}'` and take the `^{}`
value when the two differ.

## [2026-07-04] ingest — repo-rigor pass 2: hygiene, metadata, MSRV, release-notes taxonomy

- **MSRV**: measured honestly with `cargo msrv find` → **1.87.0**, re-verified
  with `--all-targets` (dev-deps included). Declared in Cargo.toml
  `rust-version` and enforced by a CI `msrv` job that must `rm
  rust-toolchain.toml` first — the toolchain file (channel=stable) outranks
  `rustup default`, so without the rm the job would silently test stable.
- **Language stats**: GitHub listed the repo as an HTML project (design/
  prototypes ≈220 KB HTML vs ≈198 KB Rust). `.gitattributes` marks `design/**`
  linguist-documentation; `src/*.html` deliberately stays counted (shipped
  source).
- **Release notes**: `.github/release.yml` groups generated notes by PR label
  (Dependabot's default `dependencies` label buckets its bumps for free);
  `skip-changelog` opts a PR out. Labels to create in repo settings:
  `security`, `breaking-change`, `skip-changelog`.
- **Docker base digest-pinned** (`rust:1-alpine@sha256:a41f…`); Dependabot's
  docker ecosystem advances the pin. `FROM scratch` has no digest to pin.
- Also: `.editorconfig`, `rust-toolchain.toml` (stable channel — a pinned
  1.XX rejected as weekly bump chores), SUPPORT.md, Best Practices badge
  (bestpractices.dev project 13484, registered by the maintainer same day),
  README contributing/security/support section.
- **CodeQL Rust caveat (investigated, upstream)**: the maintainer spotted
  "11/11 files extracted with errors" in a green CodeQL run. Every
  diagnostic is a failed macro expansion — including std macros like
  `format!` — which is an open limitation of the Rust extractor
  (github/codeql#19966, #19982, #20659), not a local config problem: adding
  a rust-src toolchain + `cargo fetch` produced byte-identical diagnostics
  (195 suppressed, 0/11 clean) and was reverted rather than cargo-culted.
  Queries still run on all non-macro code. Watch the "extracted with
  errors" metric drop as CodeQL bundles update.

## [2026-07-04] ingest — repo-rigor pass 1: SAST, workflow lint, dep review, scheduled audit

Scorecard run #1 scored five checks at 0; the fixable ones drove this PR
(Code-Review and Maintained are structural for a 2-day-old single-maintainer
repo — accepted, time fixes them):

- **CodeQL for Rust** (`codeql.yml`): GA since CodeQL 2.23.3 with
  `build-mode: none`, so the scan needs no cargo build (~4–8 min). Fixes
  SAST=0. clippy-SARIF-to-code-scanning was rejected — Scorecard's SAST check
  doesn't recognize it.
- **Workflow lint** (`lint-workflows` in ci.yml): `actionlint` always gates
  (correctness); `zizmor` uploads all severities as SARIF and gates only on
  high so new low-noise rules can't block unrelated PRs. actionlint isn't in
  install-action's registry → pinned release binary, checksum-verified.
  zizmor immediately paid for itself: it flagged a real template-injection
  (`${{ github.event.repository.default_branch }}` inline in the release
  prepare script) — now passed via `env`. The prepare checkout's kept
  credentials carry an inline `zizmor: ignore[artipacked]` with reason.
- **Dependency review** on PRs (vulnerabilities only; `license-check: false`
  because deny.toml is the single license policy — ClearlyDefined's crate
  data is spottier and would double-gate with drift).
- **Weekly advisories run** (`audit.yml`): same cargo-deny + deny.toml as CI
  (rustsec/audit-check rejected — second ignore-list format). Failure = red
  scheduled run + GitHub's failure email.
- **Release concurrency**: global `release` group, `cancel-in-progress:
  false` — queue, never cancel a half-done signed release; also serializes a
  dispatch racing a tag push.

## [2026-07-04] ingest — actions hardening + native-runner releases (v0.6.2+)

Two follow-ups to the release automation, in the order they shipped:

- **v0.6.2 — native-runner split**: the release image build moved from one
  QEMU-emulated multi-arch buildx invocation to two parallel native jobs
  (amd64 on `ubuntu-latest`, arm64 on `ubuntu-24.04-arm`), pushed by digest
  and stitched by a `merge` job; cosign/provenance/SBOM anchor to the manifest
  digest. Measured: v0.6.1 (QEMU) 34m12s → v0.6.2 (native) **5m18s**, same
  artifact set. Buildx GHA caching + CI concurrency groups landed alongside.
- **Workflow hardening (OpenSSF baseline)**: all actions pinned to full commit
  SHAs (Dependabot's `github-actions` ecosystem keeps pins fresh);
  `step-security/harden-runner` (egress audit) opens every job;
  `persist-credentials: false` on non-pushing checkouts (the release `prepare`
  job keeps credentials — it pushes the minted tag); a weekly OpenSSF
  Scorecard workflow publishes to code scanning + a README badge. The SLSA L3
  isolated builder was considered and deferred (documented in SECURITY.md
  posture; revisit if consumers demand L3). A `v*` tag ruleset (no
  update/delete/force-push; admin bypass) was applied in repo settings —
  "Restrict creations" is deliberately unchecked because the built-in
  github-actions app cannot be added to bypass lists on personal repos and
  the dispatch path mints tags with `GITHUB_TOKEN`.

## [2026-07-04] ingest — release automation: workflow_dispatch cuts releases (v0.6.1)

Tagging by hand (`git tag` + `git push`) was the one release step that
required a local terminal with tag-push rights — and restricted sessions
(e.g. Claude Code remote, whose git proxy only allows the designated branch)
cannot do it at all, which bit the v0.6.0 cut. The Release workflow gained a
`workflow_dispatch` entry point: a new `prepare` job resolves the version
from Cargo.toml on the default branch, refuses if the tag already exists,
mints and pushes the tag itself, and the same run releases end-to-end.
Design constraint that shaped it: tags pushed with `GITHUB_TOKEN` trigger no
follow-on workflow runs (GitHub's recursion guard), so the dispatch path must
never rely on the tag-push event — hence one workflow with two triggers, and
image/release tags now derive from the resolved version, not the git ref.
Full automation (release-plz/release-please) was considered and rejected:
version choice is a scope decision and the CHANGELOG is deliberately
hand-written. Runbook: [release](ops/release.md). v0.6.1 is the maintenance
release that shipped and validated this path.

## [2026-07-04] ingest — v0.6.0 release cut: correctness fixes, wizard client key, outcome charts

The 0.6.0 release closes the loose ends found during the config-store epic
and cuts the version:

- **Streaming inflight accounting fixed**: the `max_inflight` guard now rides
  into the spawned streaming task, so live streams occupy their slot until the
  stream ends (it previously dropped at response-header time, bounding only
  buffered requests). E2e-proven with a hang-stream + shed test.
- **Disconnect noticed during blocked upstream reads** (blind-review finding):
  the streaming relay races each upstream read against `tx.closed()`, so a
  client hang-up frees its `max_inflight` slot immediately instead of at the
  `stream_idle` cutoff — and hung upstreams can't pin slots until restart
  when `stream_idle` is 0. E2e: `disconnected_stream_releases_its_inflight_slot`.
- **Password-change TOCTOU closed**: an own-password change commits only if
  the stored hash is still the one the current password was verified against
  (verify runs outside the store lock); a concurrent admin reset now wins with
  a 409 (`settings::apply_password_change`, unit-tested).
- **Wizard mints the first client key** (default on, explicit warning on
  opt-out — the maintainer's rule: let users run it any way they want, warn
  when it's unsafe): `POST /setup` takes `create_client_key`, returns the
  `npk_` secret once, and the wizard ends on a connect panel (base URL + key +
  copy). [client-auth](architecture/client-auth.md) and the
  [config-store ADR](decisions/ui-managed-config-store.md) updated.
- **Charts for the collected-but-undrawn signals**: a `stackChart` primitive +
  requests-by-outcome-over-time on Reliability; requested output cap
  (`request_max_tokens`) on Clients; tool-call volume per model on Models.
- **Coverage backfill**: governor/pricing/history/limits/account endpoint e2e,
  extended role-denial matrix, unwritable-DATA_DIR boot refusal.
- **README rewritten** as a usage-focused snapshot (logo, live-traffic
  screenshots in `docs/assets/`, boot banner; history/migration framing
  dropped). CHANGELOG promoted to 0.6.0; SECURITY.md supported versions moved
  to 0.6.x.

## [2026-07-04] ingest — UI-managed config store, multi-user, governor (v0.6.0)

App-level configuration moved out of env vars and into a store the app owns,
edited from a new dashboard Settings area and claimed by a first-run wizard.
New ADR [ui-managed-config-store](decisions/ui-managed-config-store.md); the
[auth-posture](decisions/auth-posture-and-dashboard-password.md) ADR gained a
v0.6.0 amendment.

- **Store**: `DATA_DIR/config.json`, version 1, atomic writes (tmp + fsync +
  rename + dir fsync), 0600, snapshot-cached (`RwLock<Arc<Config>>`). JSON not
  SQLite (kilobytes, read-mostly, single-writer; recovery = text edit; zero
  binary weight — revisit triggers recorded in the ADR). Corrupt/unreadable/
  `version>1` = **hard boot error**, never a silent fall-through to setup.
- **First run**: `/v1` → `503 setup_required`, browsers → `/setup`, a 3-step
  wizard (superuser [password ≥10] → ≥1 NIM key validated live → finish) does
  one atomic POST, mints a session, lands on the dashboard. Claim risk accepted
  (matches Grafana/Portainer; loud boot log; no claim token).
- **Multi-user**: roles superuser (undeletable admin — deletion guard only) /
  admin / user; per-key ownership; `GET /api/config` filtered server-side.
  Sessions carry `username || first8(sha256(password_hash))`, so password
  change/reset invalidates sessions and role/deletion apply next request.
  `INSECURE_NO_AUTH` retired → store `open|keyed` mode, `/v1`-only. Client keys
  are `npk_…` 128-bit secrets shown once, stored as SHA-256 digests. Passwords
  PBKDF2-HMAC-SHA256 600k, RFC 7914 vectors.
- **Env retired to 5 container vars** (`HOST`, `PORT`, `DATA_DIR`, `RUST_LOG`,
  `TRUST_PROXY`); legacy vars ignored with one boot warning; no seed-from-env,
  no migration. `configure-env` rewritten; `.env.example` shrunk.
- **Model-pressure governor** (new component page
  [governor](architecture/governor.md)): classifies NIM's per-model
  worker-exhaustion error apart from 429s and backs off the **model** (never
  cools down the lane); adaptive AIMD (engage at half in-flight, +1/stable-min,
  dissolve after 30 clean min) with optional pinned caps. New metrics
  `nimproxy_worker_exhausted_total` / `nimproxy_model_inflight` /
  `nimproxy_model_limit`; a Reliability "Model pressure" card appears once
  engaged.
- **Key pool**: per-key rpm (default 40, 1–10000) replacing global
  `RPM_PER_KEY`; live `rebuild` with rate-state carryover; superuser-key pool
  floor invariant. [key-pool](architecture/key-pool.md) updated.
- Docs swept: README (quickstart→wizard, 5-var table, auth/sharing/metrics),
  `deploy-docker` (volume now holds credentials), `sharing-with-friends`
  (create-a-user flow), `client-auth` rewritten, `examples/README`, CHANGELOG.
- **Lint** — flagged in the summary: the Settings admin API (PR 4) and Settings
  UI incl. `npk_` client-key generation and role-filtered `/api/config` (PR 5)
  are not yet in `src/` on this branch; docs describe the intended v0.6.0
  surface per the plan. The store, wizard, auth, and governor **are**
  implemented.

## [2026-07-03] ingest — dashboard operator-console redesign

Presentation-only redesign of `src/dashboard.html` (data layer, metrics, and
history contracts untouched); see
[dashboard-operator-console-redesign](decisions/dashboard-operator-console-redesign.md)
and the rewritten [dashboard](architecture/dashboard.md) architecture page.

- **IA collapsed from six tabs to five**: `Overview · Models · Clients ·
  Reliability · Capacity`. Compare merged into Models as a scorecard section;
  Harnesses/Proxy/Keys renamed to Clients/Reliability/Capacity.
- **Dark-only.** The light palette and `prefers-color-scheme` handling were
  deleted — a committed design choice, not an oversight.
- **New interactions on every chart and table**: line-chart hover crosshair
  with a per-series tooltip snapped to the nearest sample, and click-to-sort
  tables (sticky header, capped height, internal scroll) whose sort order and
  scroll position survive the 3s live re-render.
- **CSP extended** in `src/main.rs`: `style-src` gained
  `https://fonts.googleapis.com`, a new `font-src` allows
  `https://fonts.gstatic.com` — needed for the Space Grotesk / Spline Sans
  Mono webfonts (system-font fallback offline). Everything else in the CSP is
  unchanged; `tests/e2e.rs` now asserts `font-src https://fonts.gstatic.com`
  alongside the existing CSP checks.
- No new `innerHTML` sink bypasses `esc()` — the redesign added interaction
  state (sort index, hover index) but no new dynamic-string interpolation
  path; see the security-invariant note in
  [dashboard](architecture/dashboard.md).

## [2026-07-03] ops — v0.5.0 first public release prep

Repo went public; cutting the first tagged release (which also gives
`release.yml` its first-ever run — GHCR multi-arch image, keyless cosign,
provenance, SBOM, GitHub Release).

- **New runbook** → [ops/release](ops/release.md): tag-driven release
  procedure, post-release verification checklist, roll-forward policy, and the
  one-time repo settings (private vulnerability reporting, auto-delete head
  branches, recommended `main` ruleset).
- Version 0.5.0; CHANGELOG `[Unreleased]` promoted. `release.yml` gained a
  tag↔Cargo.toml version guard so the OCI label and boot banner can't disagree.
- SECURITY.md now points **only** at private GitHub Security Advisories (no
  maintainer email published); CODE_OF_CONDUCT reports go via the maintainer's
  GitHub profile. README gained a release badge and a published-image
  (`ghcr.io`) quick start.

## [2026-07-02] decision + ingest — Benchmarking observability (v0.4.0)

Turned the proxy into a benchmarking / agent-observability tool. The request
body is already deserialized and every SSE event already scanned, so the
agent-behavior + model-quality signal was in hand but unread.

- **New decision** → [request-shape-metrics](decisions/request-shape-metrics.md):
  capture request shape (messages, tools, sampling params, stream/JSON mode) and
  response quality (finish_reason/truncation, tool calls, reasoning tokens, mean
  TPOT) as bounded-cardinality metrics — **counts and sizes, never content**.
  Shape is labeled by *client* (harness behavior), quality by *model*. Enums
  (`finish_reason`, `tool_choice` mode, `stream`) are clamped server-side.
- **Dashboard** rebuilt from three tabs to six persona-aligned views (Overview,
  Models, Compare, Harnesses, Proxy, Keys); see
  [dashboard](architecture/dashboard.md). Added `scorecard()`/`barRows()`
  helpers and a hash-to-hue color fallback past the six categorical slots.
- **Verified** in headless Chromium against a mock driving two named harnesses
  (opencode: tool-heavy/deep; codex: plain): all six tabs populate, the
  Harnesses view distinguishes both with distinct fingerprints, zero JS errors.
  Cardinality bounding is unit- and e2e-tested.

### Pre-merge hardening pass (same PR)

Before merge: security scan (dedicated dashboard-XSS audit + a full
`/security-review` of the branch) found **zero** vulnerabilities — every new
`innerHTML` value is escaped, every new label is a bounded enum / histogram, and
no route left the admin gate. Documentation swept and confirmed current (six
views, metric table, env vars). Test coverage extended to the buffered
`relay()` quality path, an unknown-`finish_reason`→`other` clamp, JSON mode, and
non-`auto` `tool_choice` (now **29 unit + 21 e2e**). The load harness gained
tool/JSON/sampling variety and a corrected boot command (`INSECURE_NO_AUTH`);
re-run at 80×3 = 240 requests → 0 failures, 0 upstream rate violations, balanced
across all keys, with the new metric series confirmed populated.

## [2026-07-02] ingest — Dashboard reporting polish

Client-side only (no server change, security invariants untouched); surfaces
data already collected but previously under-shown. See
[dashboard](architecture/dashboard.md).

- **Generation speed (tok/s) median/p95 trend** on the Models tab — the
  `nimproxy_tokens_per_second` histogram was only ever shown as one average
  tile. Same bucket-delta quantile machinery as TTFT, filtered to
  `source="usage"` so estimates don't drag the trend down.
- **Non-success outcomes table** on the Proxy tab — ranks every recorded
  non-200 status by count with a plain-language reason and share, so the
  status detail already in `nimproxy_requests_total` is legible instead of
  lumped into one "errors/min" line.
- **Threshold-colored gauges** — capacity (blue→amber≥70%→red≥90%) and success
  rate (green→amber<99%→red<90%) so the dials signal, not just count.
- Verified in headless Chromium against the mock: both new elements render with
  live data, gauges take the amber band under induced load/errors, zero JS
  page errors.

## [2026-07-02] ingest — Security hardening (v0.3.0)

A security review of the merged proxy found a stored-XSS chain (client-supplied
`model` → unescaped dashboard `innerHTML`), unbounded metric-label cardinality,
log injection, and an open-by-default posture (unauthenticated dashboard +
optional API auth). Hardening phase (branch `claude/security-hardening-auth`):

- **Fail-closed auth** → [auth-posture-and-dashboard-password](decisions/auth-posture-and-dashboard-password.md):
  refuse to start exposed without auth; `PROXY_API_KEYS` gates the API,
  `ADMIN_PASSWORD` gates the dashboard/`/metrics`/`/api/history` via an
  HMAC-signed session cookie (Bearer/Basic for scrapers).
- **Input hardening** → [input-sanitizing-and-xss](decisions/input-sanitizing-and-xss.md):
  sanitize + cardinality-cap the `model`/`path` labels at ingest, `esc()` every
  dashboard `innerHTML` sink, add a strict CSP + anti-framing/sniffing headers.
- Constant-time secret compares, failed-login throttle, `MAX_INFLIGHT` flood
  cap, `cargo audit` in CI, compose loopback-publish by default.
- Verified: 45 tests (26 unit + 19 e2e incl. boot posture, session flow, label
  sanitizing, security headers), a real-browser XSS check (payload rendered
  inert), secure-mode load test (300/300, 0 rate violations), `cargo audit`
  clean.

## [2026-07-02] ingest — CI caught the musl proc-macro trap

First real Docker build (in CI — this environment has no daemon) failed:
global crt-static RUSTFLAGS broke proc-macro dylibs on the musl-host alpine
builder. Fixed with an explicit `--target`; details appended to
[distroless-scratch-image](decisions/distroless-scratch-image.md).

## [2026-07-02] ingest — Initial bundle

Compiled the founding conversation into the knowledge base: project purpose
(rate-limit-respecting NIM proxy for agent harnesses), all eight design
decisions to date, three validated research findings about NIM's free tier,
six architecture pages, four runbooks, and the test strategy.

Notable facts captured at ingest time:

- Load test (100 clients, strict enforcing mock) caught 7/307 boundary-jitter
  rate violations at an exact 60s window → [window-jitter-margin](decisions/window-jitter-margin.md).
- Dashboard capacity gauge honestly read 133% during a cold-start burst drain
  before smoothing to a trailing-60s average → noted in [dashboard](architecture/dashboard.md).
- The `/v1/models` schema research killed the idea of API-sourced model
  descriptions; cards enrich from the id namespace instead.
