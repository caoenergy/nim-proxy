#!/usr/bin/env python3
"""locale-v1: validate a translated message catalog against its en-US source.

Stdlib only, like the other scripts here.

    python3 scripts/locale_v1.py locales/de-DE.json
    python3 scripts/locale_v1.py --all
    python3 scripts/locale_v1.py --selftest

Every check below has a matching deliberately-broken fixture in
`tests/fixtures/locales/`, and `--selftest` asserts each fixture fails for its
own reason while `valid.json` passes. Checks were written against those
fixtures rather than the other way round: a check nobody has watched fail is
decoration.

The checks, and why each exists:

- **completeness** — a missing id renders the raw id string to the operator,
  because the runtime's fallback is the id itself.
- **no orphans** — an id absent from en-US is either a typo or a leftover from
  a removed feature; both are silent.
- **placeholder parity** — the runtime substitutes by exact name. A renamed or
  dropped placeholder leaves `{count}` visible in the interface.
- **formatter syntax** — an unbalanced brace parses as literal text and ships
  as literal text.
- **no catalog markup** — catalog values are plain Unicode text. Raw markup
  and entity-encoded markup are rejected independently so neither can become
  executable or double-encoded when a value reaches the wrong sink.
  See knowledge/decisions/message-catalog-and-escaping.md.
- **inline structure** — removed, duplicated, or reordered `{b}`/`{/b}`
  markers describe a shape the fixed-node runtime does not accept.
- **source-hash freshness** — the hash records which en-US text a translation
  was made from. When the source changes and the hash does not, the translation
  is stale: still valid, no longer correct. This is the check that makes a
  regenerate-on-drift pipeline possible.
- **length caps** — opt-in per key, for strings in width-constrained elements.
"""
import argparse
import hashlib
import html
import json
import pathlib
import re
import sys

# One definition of the never-translate list, not two. check_i18n guards its
# main(), so importing it is free of side effects, and a drifting second copy
# would be worse than the coupling.
from check_i18n import NEVER_TRANSLATE

ROOT = pathlib.Path(__file__).resolve().parent.parent
FIXTURES = ROOT / "tests/fixtures/locales"
INLINE_OPEN = {"{b}"}
INLINE_CLOSE = {"{/b}"}
INLINE_TOKEN = re.compile(r"\{/?b\}")
PLACEHOLDER = re.compile(r"\{[^{}]*\}")


def placeholders(text: str) -> set:
    """Value placeholders only — inline markup tags are checked separately."""
    return {p for p in PLACEHOLDER.findall(text) if p not in INLINE_OPEN | INLINE_CLOSE}


def unbalanced_braces(text: str) -> bool:
    depth = 0
    for ch in text:
        if ch == "{":
            if depth:
                return True  # nested { — never valid
            depth = 1
        elif ch == "}":
            if not depth:
                return True
            depth = 0
    return depth != 0


def validate(source: dict, candidate: dict, name: str) -> list:
    """Return a list of (check, message). Empty means the locale is sound."""
    problems = []
    src, cand = source["messages"], candidate["messages"]

    for mid in sorted(set(src) - set(cand)):
        problems.append(("completeness", f"{name}: missing {mid}"))
    for mid in sorted(set(cand) - set(src)):
        problems.append(("orphan", f"{name}: {mid} is not in the source catalog"))

    for mid in sorted(set(src) & set(cand)):
        s, c = src[mid], cand[mid]
        text = c.get("en", "")

        if unbalanced_braces(text):
            problems.append(("syntax", f"{name}: {mid} has an unbalanced brace"))
        else:
            want, got = placeholders(s["en"]), placeholders(text)
            if want != got:
                miss = ", ".join(sorted(want - got)) or "none"
                extra = ", ".join(sorted(got - want)) or "none"
                problems.append((
                    "placeholders",
                    f"{name}: {mid} placeholder mismatch (missing {miss}; unexpected {extra})",
                ))

        if "<" in text or ">" in text:
            problems.append(("catalog-markup", f"{name}: {mid} contains raw markup"))
        decoded = html.unescape(text)
        if decoded != text and ("<" in decoded or ">" in decoded):
            problems.append((
                "catalog-entity-markup",
                f"{name}: {mid} contains entity-encoded markup",
            ))

        source_inline = INLINE_TOKEN.findall(s["en"])
        candidate_inline = INLINE_TOKEN.findall(text)
        if candidate_inline != source_inline:
            problems.append((
                "inline",
                f"{name}: {mid} inline structure {candidate_inline!r} "
                f"does not match source {source_inline!r}",
            ))

        if c.get("hash") != s["hash"]:
            problems.append((
                "stale",
                f"{name}: {mid} was translated from a different source text "
                f"(recorded {c.get('hash')}, source is {s['hash']})",
            ))

        cap = s.get("maxLen")
        if cap and len(text) > cap:
            problems.append(("length", f"{name}: {mid} is {len(text)} chars, cap is {cap}"))

        # Units, HTTP status codes and API identifiers carry the same meaning in
        # every language. A machine translator will happily render "rpm" as
        # "tr/min" or "429 retry" as "429 réessai", and the result validates
        # against every other check while being wrong. Only tokens the SOURCE
        # actually uses are required, so this never invents a constraint.
        for token in sorted(NEVER_TRANSLATE):
            if token in s["en"] and token not in text:
                problems.append((
                    "frozen",
                    f"{name}: {mid} dropped or translated {token!r}, "
                    f"which must survive verbatim in every locale",
                ))

    return problems


def load(path: pathlib.Path) -> dict:
    return json.loads(path.read_text())


def selftest() -> int:
    """Each fixture must fail for its OWN reason, and valid.json must pass."""
    source = load(FIXTURES / "_source-en-US.json")
    expected = {
        "valid.json": None,
        "missing-key.json": "completeness",
        "orphan-key.json": "orphan",
        "placeholder-mismatch.json": "placeholders",
        "bad-formatter-syntax.json": "syntax",
        "raw-html.json": "catalog-markup",
        "entity-html.json": "catalog-entity-markup",
        "stale-hash.json": "stale",
        "too-long.json": "length",
        "inline-dropped.json": "inline",
        "unbalanced-inline.json": "inline",
        "frozen-token-dropped.json": "frozen",
    }
    failures = []
    for fixture, want in sorted(expected.items()):
        got = validate(source, load(FIXTURES / fixture), fixture)
        checks = {c for c, _ in got}
        if want is None:
            if got:
                failures.append(f"{fixture}: expected to pass, got {sorted(checks)}")
            else:
                print(f"  ok  {fixture:28} passes")
        elif want not in checks:
            failures.append(f"{fixture}: expected check {want!r}, got {sorted(checks) or 'nothing'}")
        else:
            print(f"  ok  {fixture:28} trips {want}")

    if failures:
        print("\nselftest FAILED:")
        for f in failures:
            print("  -", f)
        return 1
    print(f"\nselftest ok — {len(expected)} fixtures, every check observed to fail")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("locale", nargs="*", type=pathlib.Path)
    ap.add_argument("--all", action="store_true", help="validate every locales/*.json")
    ap.add_argument("--selftest", action="store_true", help="prove each check can fail")
    args = ap.parse_args()

    if args.selftest:
        return selftest()

    # Two catalogs ship — the dashboard and the wizard — and each locale must
    # be checked against its OWN source. Pairing a wizard locale with the
    # dashboard source reports every id in both as missing or orphaned.
    def source_for(path: pathlib.Path) -> pathlib.Path:
        return ROOT / ("locales/setup-en-US.json" if path.name.startswith("setup-")
                       else "locales/en-US.json")

    SOURCES = {"en-US.json", "setup-en-US.json"}
    targets = list(args.locale)
    if args.all:
        targets = [
            p for p in sorted((ROOT / "locales").glob("*.json"))
            if p.name not in SOURCES
        ]
    if not targets:
        print("no locales to validate (en-US is the source)")
        return 0

    problems = []
    for path in targets:
        src = source_for(path)
        if not src.exists():
            sys.exit(f"no {src.relative_to(ROOT)} to validate {path.name} against")
        problems += validate(load(src), load(path), path.name)

    if problems:
        print(f"{len(problems)} problem(s):")
        for check, msg in problems:
            print(f"  [{check}] {msg}")
        return 1
    print(f"locale-v1 ok — {len(targets)} locale(s) clean")
    return 0


if __name__ == "__main__":
    sys.exit(main())
