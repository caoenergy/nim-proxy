# Agent guide for nim-proxy

nim-proxy is a Rust proxy that makes NVIDIA NIM's free tier usable for agent
harnesses: it paces requests to the per-key rate limit, load-balances across
keys, and keeps client connections alive while it waits. Source lives in
`src/` (`main.rs`, `lib.rs`, `proxy.rs`, `pool.rs`, `dispatch.rs`,
`governor.rs`, `history.rs`, `config.rs`, `settings.rs`, `auth.rs`,
`dashboard.html`, `setup.html`), tests in `tests/`, scripts and harnesses in
`scripts/`, translations in `locales/`.

Auth lives in `src/auth.rs` (fail-closed posture, admin-password session
cookie, constant-time compares); the API-key gate and label sanitizing are in
`src/proxy.rs`. Any change touching auth, request labels, or the dashboard's
`innerHTML` must keep the security invariants — see the `decisions/` pages on
auth posture and input sanitizing before editing.

## How to work here

**Plan → Act → Verify, at every scale.** Not once per release — once per task,
and again per subtask. A "task" is any edit you are about to make.

Before editing, write down three things:

1. **Outcome** — what will be true after this change that is not true now.
2. **Proof** — the exact command or observation that will show it, *and how it
   would look if the change were wrong*. If you cannot describe the failing
   case, you do not have a check; find a different proof.
3. **Constraint** — which `knowledge/` page governs this area. Read it. The
   reasoning that limits your change is probably already recorded.

Then act. Then run the proof you named — not a different, easier one, and not
a batch of unrelated green checks at the end.

Write the check first when the change is behavioral. A test that has never
been observed to fail has not been shown to test anything: make it fail, then
make it pass. This is why `scripts/locale_v1.py --selftest` exists, and why
the locale fixtures were committed one commit before the validator.

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

Say which rung you landed on and why the ones above it did not apply. New
components are pinned at the version you got working (`=x.y.z`).

## What proves what

`cargo test` **does not validate the embedded pages.** It asserts on served
HTML *text* and never parses or executes the page JavaScript. Every page bug
this project has shipped got past a green `cargo test`. Pick the check that
actually covers what you changed:

| Change | What proves it |
|---|---|
| Rust logic | `cargo test` (unit + e2e), `cargo clippy --all-targets -- -D warnings` |
| Anything in `dashboard.html` / `setup.html` | `node --check` on each extracted `<script>` — syntax only; necessary, never sufficient |
| Page *behavior* | `node scripts/render_check.js` (+ `--escape-probe`) — renders against captured payloads, hovers every chart, fails on any uncaught page error; nothing else proves the code ran |
| Page *layout* | your eyes. Clipping is a layout property and no script here judges it |
| Number/date/duration formatting | `TZ=UTC LC_ALL=en_US.UTF-8 node scripts/formatter_fixture.js --check` (golden; extracts the real function bodies, and refuses to run unpinned) |
| Strings, catalog, any new UI text | `python3 scripts/check_i18n.py` (round-trip + untagged-string lint) |
| A locale file | `python3 scripts/locale_v1.py --all`; `--selftest` proves the validator still bites |
| Any change to English source text | `python3 scripts/gen_pseudolocale.py --check` |
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
