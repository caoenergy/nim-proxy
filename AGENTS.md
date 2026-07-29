# Agent guide for nim-proxy

nim-proxy is a Rust proxy that makes NVIDIA NIM's free tier usable for agent
harnesses: it paces requests to the per-key rate limit, load-balances across
keys, and keeps client connections alive while it waits. Source lives in
`src/` (`main.rs`, `lib.rs`, `proxy.rs`, `pool.rs`, `dispatch.rs`,
`governor.rs`, `history.rs`, `config.rs`, `settings.rs`, `auth.rs`, `api.rs`,
`dashboard.html`, `setup.html`), tests in `tests/`, scripts and harnesses in
`scripts/`, translations in `locales/`.

Auth lives in `src/auth.rs` (fail-closed posture, admin-password session
cookie, constant-time compares); the API-key gate and label sanitizing are in
`src/proxy.rs`.

## Non-negotiables

Break any of these and the change is wrong regardless of whether it works:

1. **Fail closed.** Pre-setup the data plane is shut; post-setup every
   operator surface requires a session; a corrupt or future-version store
   refuses to boot.
2. **Escape once.** Catalog values are escaped at load. No render helper
   escapes its label argument again.
3. **Zero upstream rate violations.** Not "few". The proxy exists to never
   exceed the per-key limit.
4. **The wire format does not move** without a deliberate, documented
   breaking-change note.
5. **Data is never localized.** Model ids, client names, publisher names and
   every `nimproxy_*` series pass through untouched. We localize our labels,
   not the API's values.
6. **Identifiers stay frozen** during label work — metric names, DOM ids,
   `data-*`, CSS classes, sort keys, config keys.

## Where to look — knowledge router

`knowledge/` is an Open Knowledge Format bundle: one concept per file, path =
identity, so it greps well. **Read the page for the area before you touch
it** — the constraint is usually already written down, and contradicting a
decision page is the most common way work here goes wrong.

| Touching | Read first |
|---|---|
| pacing, key pool, lanes | `decisions/sliding-window-not-token-bucket.md`, `decisions/window-jitter-margin.md`, `architecture/key-pool.md` |
| queueing, dispatch order | `decisions/global-fifo-dispatcher.md`, `architecture/dispatcher.md` |
| affinity, prefix cache | `decisions/sticky-affinity-with-spillover.md`, `research/nim-kv-cache-reuse.md` |
| per-model concurrency | `architecture/governor.md` |
| streaming, retries, deadlines | `architecture/streaming-pipeline.md`, `decisions/sse-heartbeats-for-rate-waits.md`, `decisions/explicit-request-deadline.md` |
| auth, roles, sessions | `decisions/auth-posture-and-dashboard-password.md`, `architecture/client-auth.md` |
| anything reaching `innerHTML` | `decisions/input-sanitizing-and-xss.md`, `decisions/message-catalog-and-escaping.md` |
| UI text, catalog, labels | `decisions/standard-vocabulary.md`, `decisions/message-catalog-and-escaping.md` |
| locales, translation | `decisions/locale-guards.md`, `decisions/intl-formatting.md` |
| numbers, dates, durations | `decisions/intl-formatting.md` |
| a label with a count in it | `decisions/plural-categories-not-ternaries.md` |
| page behavior, checks | `decisions/render-gate.md`, `testing/test-strategy.md` |
| history, retention, rollups | `decisions/reset-aware-dashboard-history.md`, `architecture/metrics-history.md` |
| config store, settings | `decisions/ui-managed-config-store.md` |
| API responses, `openapi.json` | `decisions/typed-responses-and-generated-openapi.md` |
| releases, tags, branch protection | `ops/release.md` |
| deployment, env vars | `ops/deploy-docker.md`, `ops/configure-env.md` |

`knowledge/index.md` is the full catalog; `knowledge/log.md` is the
chronology. If no page covers what you are doing, say so in the plan — that is
a valid answer once you have looked, and a signal a new decision page is owed.

## How to run the work

**Track it.** Keep a task list and update it as a gate, not as decoration: a
task moves to done when its proof has run, not when the edit is written.

**Delegate by surface area, not by difficulty.**

- **Do it inline** — single-file edits, renames, one-line fixes, anything
  where reading the diff is the review.
- **Fan out to subagents** — work spanning many call sites or files (a sweep
  across render functions, one agent per group), and independent research or
  terminology passes where you want more than one opinion.
- **Always use a reviewer subagent** for a diff you wrote and are about to
  merge. Give it the constraining decision pages as context. You cannot
  adversarially review your own work in the same context that produced it.

**Bugs outside the current scope become issues, not detours.** File them with
`.github/ISSUE_TEMPLATE/bug_report.yml` — `blank_issues_enabled` is `false`,
so a hand-rolled issue is a process bypass. Include the file:line, the
reproduction, and say plainly if it was found by inspection rather than
observed. Fix in-scope defects as you go; never silently leave one behind.

## How to work here

**Every task gets a visible Plan → Act → Verify.** A task is any edit you are
about to make — not a release, not a PR. Sub-tasks get their own loop.

**The plan is output, not thought.** Before the first tool call that changes a
file, your reply must contain these four lines. If they are not in the reply,
you have not planned:

- **Outcome** — what will be true after this change that is not true now.
- **Proof** — the exact command, *and what its output looks like if the change
  is wrong*. No describable failing case means it is not a check; find a
  different proof.
- **Constraint** — the `knowledge/` page governing this area, read **before**
  editing, and what in it limits this change. "Nothing governs this" is a
  valid answer only after looking.
- **Rung** — which rung of the ladder below you landed on, and why the ones
  above it did not apply. **Every change**, not only new files.

Then act. Then run the proof you named — that one, not an easier one, and not
a batch of unrelated green checks at the end.

**Numbers are measured, never predicted.** Never write a count, percentage, or
test total you did not just read from command output. If you expect a number
to change, run the command and quote what it said.

**A green first run is a claim, not a result.** If a check passes the first
time, say how you know it could have failed. A check nobody has watched fail
is decoration — see the traps below for four that passed while the page was
broken.

Write the check first when the change is behavioral. A test that has never
been observed to fail has not been shown to test anything: make it fail, then
make it pass. This is why `scripts/locale_v1.py --selftest` and
`scripts/check_i18n.py --selftest` exist, and why the locale fixtures were
committed one commit before the validator.

**A proof you ran by hand is not a proof — it is a rehearsal.** If no committed
check covers the thing you are about to change, *building that check is the
first deliverable*, before the change. Not after, and not "noted as a gap."

This is the rule most often broken here, and it is broken by doing extra work,
not less. `src/setup.html` was verified by a hand-built browser harness **three
separate times** — the harness thrown away each time and "setup.html has no
render coverage" written into the CHANGELOG, a knowledge page, and a PR body as
a known gap. Three throwaway proofs cost more than one committed check and
leave nothing behind. `check_i18n.py` had no `--selftest` while its lint shipped
three blind spots, so each round of injections that proved a fix evaporated with
the scratch directory.

Two tells that you are doing it:

- You are about to write "no check covers this" — that sentence is a work item,
  not a disclosure.
- Your Proof line names a command you had to construct in a scratch directory.
  Ask whether it belongs in `scripts/` instead. If it would catch a regression
  tomorrow, it does.

Scratch injection is still the right way to prove a *committed* check bites —
you cannot commit a broken page. Prove it by hand, then make the assertion
permanent. And assert on **which** check fired, not on a substring of the
output: a scratch test that grepped for `Latency` reported the retired-term scan
working when only the prose scan had fired, because `Latency breakdown` is the
replacement term, not a retired one.

**Report what happened.** If a check was skipped, say it was skipped. A green
summary that omits the unrun check is worse than no summary.

## Before adding anything new

Run every proposed file, script, dependency, abstraction, or workflow down
this ladder and stop at the first rung that answers:

1. Does this need to exist? → no: skip it.
2. Already in this codebase? → reuse it, don't rewrite.
3. Stdlib does it? → use it.
4. Native platform feature? → use it. (`Intl`, `Intl.PluralRules`,
   `<dialog>`, CSS — the pages have no build step and no framework.)
5. Installed dependency? → use it.
6. One line? → one line.
7. Only then: the minimum that works.

The rung goes in the plan above, for every change — not only when adding a
file. New components are pinned at the version you got working (`=x.y.z`).

## What proves what

`cargo test` **does not validate the embedded pages.** It asserts on served
HTML *text* and never parses or executes the page JavaScript. Every page bug
this project has shipped got past a green `cargo test`. Pick the check that
actually covers what you changed:

| Change | What proves it |
|---|---|
| Rust logic | `cargo test` (unit + e2e), `cargo clippy --all-targets -- -D warnings` |
| Anything in `dashboard.html` / `setup.html` | `node --check` on each extracted `<script>` — syntax only; necessary, never sufficient |
| Page *behavior* | `node scripts/render_check.js` (+ `--escape-probe`), and `--page setup` for the wizard — renders against captured payloads, hovers every chart, drives the wizard, fails on any uncaught page error; nothing else proves the code ran |
| Page *layout* | your eyes. Clipping is a layout property and no script here judges it |
| Number/date/duration formatting | `TZ=UTC LC_ALL=en_US.UTF-8 node scripts/formatter_fixture.js --check` (golden; extracts the real function bodies, and refuses to run unpinned) |
| Strings, catalog, any new UI text | `python3 scripts/check_i18n.py` (round-trip + untagged-string lint); `--selftest` proves the lint still bites |
| A locale file | `python3 scripts/locale_v1.py --all`; `--selftest` proves the validator still bites |
| Any change to English source text | `python3 scripts/gen_pseudolocale.py --check` |
| Handlers or wire types | `UPDATE_OPENAPI=1 cargo test --test openapi`, then commit `openapi.json` |
| Pacing, key pool, dispatcher, affinity | `scripts/mock_nim.py --enforce` + `scripts/loadtest.py` — **zero** upstream violations, not "few" |
| Anything at all, before pushing | `cargo fmt` |

CI enforces all of the above. Green CI on a page change means the syntax
parsed, not that the page works.

## Traps this repo has actually sprung

Real bugs that reached a branch here. Pattern-match against them before
claiming a sweep is done:

- **Blanket renames hit unrelated identifiers.** A `window` → `time range`
  label sweep silently renamed the rate-limit *window* counter. Never
  search-and-replace a word that is both a label and a domain term; enumerate
  the sites and decide each one.
- **Measuring the page at rest measures nothing.** KPI cards, ring gauges,
  perf blocks and table bodies do not exist in the DOM until data arrives. Any
  claim about rendered output has to be measured with API payloads replayed
  into the page.
- **Escape once.** Catalog values are escaped at load
  (`decisions/message-catalog-and-escaping.md`). A second escape at the render
  site produced `&amp;lt;` in table headers; `t()` escapes, `tRaw()` does not,
  and four render helpers do not escape their arguments.
- **Strings passed as arguments hide from string sweeps.** The leaks that
  survived extraction were all label/note arguments to `ringGauge`,
  `perfBlock` and `prow`, not text sitting in markup.
- **Ordering.** A static-apply pass that runs after the tab-restore loop
  applies to markup that has already been replaced.
- **`${...}` inside single quotes** is not a template literal, and
  **`const t`** shadows the translation function. Both parse fine.

## The knowledge base (`knowledge/`)

`knowledge/` is an Open Knowledge Format bundle (the LLM-wiki pattern): the
project's compiled memory — why decisions were made, validated research about
NIM, how components work, and operational runbooks. **Read
`knowledge/index.md` before making non-trivial changes**, and read the
decision page for the area you are about to touch before you touch it. The
constraint is usually already written down, and contradicting your own
decision page is the most common way work here goes wrong.

Schema:

- One concept per file, kebab-case filename, path = identity.
- YAML frontmatter: `type` (required — one of `Decision`, `Research Finding`,
  `Component`, `Runbook`), plus `title`, `description`, `tags`, `timestamp`,
  and `resource` (URL) where applicable.
- Markdown body with relative links to other pages — keep the graph connected.
- Decision pages follow a lightweight ADR shape: Context → Options →
  Choice → Consequences.

Maintenance workflow (you, the agent, are the maintainer):

1. **Ingest**: when a merged change alters behavior described in the wiki,
   update the affected pages in the same PR — don't let code and knowledge
   diverge.
2. **New decisions** get a new page under `decisions/` and a line in
   `knowledge/index.md`.
3. **Log**: append a dated entry to `knowledge/log.md` for every ingest
   (`## [YYYY-MM-DD] ingest|decision|lint — summary`).
4. **Lint**: if you spot a contradiction between a page and the code, flag it
   in your summary and fix the page — the code is the source of truth for
   *what*, the wiki for *why*.
