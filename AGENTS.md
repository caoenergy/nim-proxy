# Agent guide for nim-proxy

## Start here

nim-proxy is a Rust proxy that makes NVIDIA NIM's free tier usable for agent
harnesses by pacing requests to the per-key rate limit, load-balancing across
keys, and keeping client connections alive while they wait.

Before non-trivial work, read the applicable file in [`docs/plans/`](docs/plans/)
completely. Then read [`knowledge/index.md`](knowledge/index.md) and search
concepts and synonyms with `rg -n -i`, supplying concrete domain terms and
known synonyms across `knowledge/` and `docs/plans/`. Read the relevant concept
pages completely. When chronology matters, search
[`knowledge/log.md`](knowledge/log.md) with concrete concept names, historical
vocabulary, and `gotcha`. Follow relative links; when evolution or provenance
matters, run `git log -- <concept-page-path>`. Read source files only after the
governing plan and memory.

## Invariants

Break any of these and the change is wrong regardless of whether it works:

1. **Fail closed.** Pre-setup the data plane is shut; post-setup every operator
   surface requires a session; a corrupt or future-version config/auth store
   refuses to boot. This durable-store startup rule applies to the config/auth
   store; history recovery follows its own
   [documented format policy](knowledge/architecture/metrics-history.md).
2. **Escape once.** The current
   [escape-at-load decision](knowledge/decisions/message-catalog-and-escaping.md)
   remains binding: catalog values are escaped at load, and no render helper
   escapes its label argument again. Only the separately approved atomic
   replacement may supersede it.
3. **Zero upstream rate violations.** Not "few". The proxy exists to never
   exceed the per-key limit.
4. **The wire format does not move.** Protected machine contracts include API
   bodies, status codes, content types, OpenAPI, config and history formats,
   metric names and label values, and stable identifiers. An intentional
   breaking change requires a decision, tests, and a release note.
5. **Data is never localized.** Model ids, client names, publisher names, and
   every `nimproxy_*` series pass through untouched. Localize repository-owned
   labels, not API values.
6. **Identifiers stay frozen** during label-only work: metric names, DOM ids,
   `data-*`, CSS classes, sort keys, and config keys do not move. Deliberate
   contract rationalization is separate work.

## Repository operations

Worktree and task-branch creation are standing repository authorization; do
not ask the owner for repeated consent. Resolve the
integration branch named by the active plan rather than hard-coding a release
name.

Create every independently reviewable work item in an isolated worktree on a
task branch based on that integration branch. Prefer native worktree support;
otherwise use Git's worktree support and verify the project-local worktree
directory is ignored before creation. Open each work-item PR against the
integration branch and keep `main` untouched.
Only the final integration PR targets `main`.

Preserve unrelated changes. Remove a worktree only after its branch is safely
integrated, and retain the branch and PR evidence required by the active plan.

## Work loop

Plans are live artifacts. Update the active plan whenever work is discovered.
**Scope deltas** are recorded there and shown to the owner before
implementation.

Every edit uses a visible **Outcome**, **Proof**, **Constraint**, and
**Ponytail Rung**, followed by **Act** and the named proof: Plan → Act → Verify.
Behavioral work uses a committed **red → green** regression check. Documentation
uses structural validation and review; layout additionally requires human
inspection. Report results, counts, and percentages only from fresh output.

Bugs outside scope use the configured GitHub bug-form fields. If GitHub is
unavailable, record the pending issue in the active plan.

Authoring may be inline or delegated by surface area. Every substantive work
item receives an independent review before commit. If no independent agent
exists, record that limitation and obtain a fresh-context or owner review;
never claim independence that did not occur.

## Memory: Query → Ingest → Lint

- **Query:** active plan → semantic index → `rg -n -i` terms and synonyms →
  complete concept pages → relevant log entries → relative links and
  `git log --` file history.
- **Ingest:** promote only reproduced, measured, authoritatively referenced,
  or approved knowledge into one concept page. Use the existing types
  `Decision`, `Research Finding`, `Component`, and `Runbook`. Update affected
  pages with the behavior they explain.
- **Index and chronology:** update `knowledge/index.md` when adding, moving, or
  retiring a concept, and append `knowledge/log.md` for every ingest. Never
  leave a durable fact only in the log.
- **Lint:** search for contradictions, stale claims, duplicate or orphan
  concepts, missing cross-links, broken local links, and log-only facts.
  Investigate a code/knowledge mismatch as a possible code defect or stale
  memory: code is evidence of current behavior, while knowledge records
  approved intent and rationale. Observed code never legitimizes an accidental
  bug, and neither side is silently rewritten to match the other.

`knowledge/log.md` is chronology, not the primary retrieval surface. The
durable schema lives in
[the OKF memory decision](knowledge/decisions/okf-query-ingest-lint.md); do not
duplicate it in this guide.

## Verification and review

Route every changing command and coverage statement to
[the test strategy](knowledge/testing/test-strategy.md). Name the exact proof
before editing and run that exact proof afterward. Report skipped checks. A
green first run requires a demonstrated failure path, and self-tests assert
which check fired. Before any push, run the relevant checks plus
`cargo fmt --check`.

## Ponytail

1. Does this need to exist?
2. Is it already in the repository?
3. Does the standard library do it?
4. Does the native platform do it?
5. Does an installed dependency do it?
6. Is it one line?
7. Only then add the minimum new machinery.

## Authority boundaries

- **Plans:** current scope, state, decisions in flight, and committed proof.
- **Code/tests:** current executable behavior and regression evidence.
- **Knowledge:** approved understanding, constraints, rationale, and runbooks.
- **GitHub:** issue, PR, and release execution state; mirror plan status without
  replacing the plan.
- **External systems:** read-only unless the owner authorizes a write.
