---
type: Decision
title: Typed API responses and a generated OpenAPI spec
description: Response bodies become derive(Serialize) structs whose declaration order is the wire order, and openapi.json is generated from those types with utoipa — spec file only, no served UI.
tags: [api, openapi, dashboard, wire-format, ci]
timestamp: 2026-07-29T00:00:00Z
---

# Typed API responses and a generated OpenAPI spec

## Context

Every response the operator surface served was built by hand with
`serde_json::json!`: ~20 sites in `src/settings.rs`, two more in `src/lib.rs`.
The shape of `/api/config` existed in exactly two places — inside a macro
invocation, and inside whatever `src/dashboard.html` happened to read — and
nowhere as a declaration. That has three costs:

- **No generator input.** There was nothing for an OpenAPI tool to read, so
  there was no machine-readable contract for the dashboard, for a third-party
  client, or for a reviewer asking "what does this return for a `user` role?".
- **Silent shape drift.** Adding a key to a `json!` body is invisible to the
  compiler. Removing one is too — the 0.6.6 pricing removal had to be chased
  through both the macro and the page.
- **Role filtering by omission.** `/api/config` built an admin body and then
  *added* `server`/`users` keys, which reads as "start open, remove things" —
  the opposite of the [fail-closed posture](auth-posture-and-dashboard-password.md)
  the rest of the code holds.

`src/config.rs` and `src/history.rs` were already typed; the API layer was the
hold-out.

The constraint that shaped everything: **the wire format must not move.**
0.6.6 already carries two breaking changes (the pricing removal and the
`nimproxy_lane_benched_total` rename). A third — an accidental one — would be
unforgivable, and ~85 settings tests plus a dashboard that parses these bodies
are the things that would break.

## Options

1. **Hand-write `openapi.json` alongside the `json!` bodies.** Rejected: two
   descriptions of one thing, and the one CI can check is not the one that
   serves requests. This is the drift the change exists to remove.
2. **Type the responses, generate the spec, and serve it through
   `utoipa-scalar` / `utoipa-redoc`.** Rejected for the *serving* half only.
   Both UIs load JavaScript from a CDN, which the dashboard's
   `script-src 'self' 'unsafe-inline'` CSP forbids; the fixes are widening the
   CSP for a docs page (weakening a control that exists to contain XSS) or
   bundling ~1 MB of assets into a `FROM scratch` image whose whole point is
   being a single static binary.
3. **Type the responses and generate a spec *file*.** Chosen.

## Choice

Option 3, in two halves.

**Wire types.** `src/api.rs` holds every response body as a
`derive(Serialize, ToSchema)` struct. Handlers construct a value instead of a
`json!` literal. `/api/config` now *builds or does not build* the admin
sections — role filtering became a `Option<ServerSettings>` that is `None`
for a `user`, so the type says what the security posture already required.

**Generated spec.** `utoipa` (pinned `=5.5.0`, project convention) renders
`openapi.json` at the repo root from `#[utoipa::path]` on the handlers and
`ToSchema` on the types. 14 operations: the **12** `/api/*` routes, plus
`POST /setup` and `POST /setup/validate-key`.

- **The two `/setup` routes are in the spec, flagged unauthenticated.** They
  are JSON endpoints that a wizard, a scripted install, or an operator
  debugging upstream reachability legitimately calls, and leaving them
  undocumented would not make them less reachable. They sit outside the
  `route_layer` because no user exists yet, so they carry an explicit empty
  `security: []` — the document-level requirement (session cookie *or* header
  credentials) applies to everything else. Both 404 the moment a superuser
  exists, so the window is exactly one claim wide.
- **Out of scope, deliberately:** the OpenAI-compatible `/v1` passthrough (that
  contract is the upstream's, not ours), the HTML page routes, the
  form-encoded `/login`/`/logout` browser flow, plain-text `/health`, and the
  Prometheus exposition at `/metrics`. The spec describes the *dashboard API*,
  not every path the router answers.
- **No served UI**, per option 2.

### Field order is the wire order

The load-bearing detail. `serde` emits struct fields in **declaration order**;
`serde_json::Map` is a `BTreeMap` unless the `preserve_order` feature is on
(it is not), so every `json!` body this change replaced emitted its keys in
**ASCII order**. A struct that declares fields in a "natural" reading order
therefore *reshapes the response*.

So every wire struct declares its fields ASCII-sorted, and three existing
types were reordered to match what they had always serialized as:
`history::MetricValue`, `history::RollupPoint`, `history::HistoryDiagnostics`,
and `config::Limits`. `_` (0x5F) sorts before every lowercase letter, so
`lane` < `last4` and `username` < `users` — not obvious, hence the guard test
`api::field_order_stays_ascii_sorted`, which serializes a populated value of
every wire type and asserts the emitted keys come out sorted.

`config::GovernorCfg::overrides` changed from `HashMap` to `BTreeMap` for the
same reason: inside `json!` it was sorted by the intermediate `Map`, and
serialized directly it would have been hash-ordered. The side benefit is that
`config.json` is now byte-deterministic — two saves of the same config used to
differ.

### Drift is checked, not trusted

`tests/openapi.rs` regenerates the spec and compares it to the committed file;
CI's `check` job additionally runs `UPDATE_OPENAPI=1 cargo test --test openapi`
followed by `git diff --exit-code -- openapi.json`, so a stale spec fails the
build rather than merely being discouraged. `spec_is_usable` asserts the
document is consumable at all: 14 operations, every one tagged with a
documented 200, every `/api/*` operation inheriting the auth requirement and
every `/setup` operation explicitly waiving it.

## Consequences

- **The wire format did not move.** Verified two ways: the ~85 settings tests
  in `tests/e2e.rs` pass **unmodified**, and a throwaway harness captured raw
  response bytes for 31 request/response pairs (both `/api/config` role views,
  both dashboard endpoints with and without traffic, every settings write, and
  every error branch) before and after — key-for-key identical at every
  nesting level.
- **`info.version` tracks `CARGO_PKG_VERSION`**, so a version bump makes
  `openapi.json` stale and CI says so. [Cutting a release](../ops/release.md)
  step 1 now includes regenerating it.
- Two new crates in the tree (`utoipa`, `utoipa-gen`); `indexmap` was already
  there. Both MIT OR Apache-2.0, so `cargo deny`'s allowlist did not move.
- The `/api/config` role filter is now expressed in the type. A future field
  that should be admin-only goes inside `ServerSettings` and inherits the
  filtering; adding it to `ConfigResponse` instead is a visible choice in a
  diff rather than an invisible one inside a macro.
- `config.json`'s `limits` block and `governor.overrides` are written in a new
  key order. Purely cosmetic — the store is read by serde, which does not care
  — and no migration runs.
- The spec is a file, not a page. Operators who want a browsable UI can point
  any offline viewer at `openapi.json`; nothing is served, so the CSP and the
  scratch image are untouched.
