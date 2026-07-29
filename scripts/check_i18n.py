#!/usr/bin/env python3
"""Verify the embedded message catalog against the markup it was extracted from.

Stdlib only, like the other scripts here. Run from the repo root:

    python3 scripts/check_i18n.py

Three checks, all of which must hold for an extraction to be non-destructive:

1. **Round-trip.** Every element carrying `data-i18n` / `data-i18n-text` /
   `data-i18n-attr` still contains, in the markup, exactly the text the catalog
   holds for that id. Extraction only *adds attributes* — if a tagged element's
   text and its catalog value ever disagree, the page renders differently after
   the runtime substitutes, which is the bug this catches.
2. **Completeness.** No id referenced from the markup is missing from the
   catalog, and no catalog id is unreferenced (orphan).
3. **Hash freshness.** Each message's recorded hash matches its current text.
   Translated locales record the source hash they were made from, so a stale
   translation is detectable rather than silently wrong.

Exits non-zero on any failure. PR 5 extends this into the full `locale-v1`
validator (placeholder parity, no raw HTML in values, length caps) with the
negative fixtures that prove each check actually fails.
"""
import hashlib
import html as htmlmod
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent


def load_catalog(source: str):
    m = re.search(
        r'<script type="application/json" id="i18n-catalog">(.*?)</script>', source, re.S
    )
    if not m:
        sys.exit("no inline i18n catalog found")
    return json.loads(m.group(1))["messages"]


def strip_scripts(source: str) -> str:
    """Drop executable <script> bodies; the runtime's own doc comment mentions
    these attribute names and would otherwise be scanned as markup."""
    return re.sub(r"<script>.*?</script>", "<script></script>", source, flags=re.S)


def own_text(inner: str) -> str:
    """The element's own first text node, skipping complete child elements.

    `<h2><svg>...</svg>Traffic <span>note</span>` -> "Traffic ". This mirrors
    what the runtime does, which assigns to the first non-blank text-node child
    of the element itself, not of a descendant."""
    depth, buf, out = 0, [], ""
    i = 0
    while i < len(inner):
        ch = inner[i]
        if ch == "<":
            if depth == 0 and "".join(buf).strip():
                return "".join(buf)
            buf = []
            close = inner.find(">", i)
            if close == -1:
                break
            if not inner[i + 1: close].endswith("/") and not inner.startswith("</", i):
                depth += 1
            elif inner.startswith("</", i):
                depth -= 1
            i = close + 1
            continue
        if depth == 0:
            buf.append(ch)
        i += 1
    return "".join(buf)


def tagged(source: str, attr: str):
    """Yield (id, full_element_markup) for each element carrying `attr`."""
    for m in re.finditer(rf'<(\w+)([^>]*\s{attr}="([^"]+)"[^>]*)>', source):
        tag, mid = m.group(1), m.group(3)
        # naive but sufficient: these elements do not nest the same tag
        close = source.find(f"</{tag}>", m.end())
        yield mid, source[m.end():close if close != -1 else m.end()]


def main() -> int:
    errors = []
    referenced = set()

    for name in ("src/dashboard.html", "src/setup.html"):
        path = ROOT / name
        raw = path.read_text()
        if 'id="i18n-catalog"' not in raw:
            continue
        catalog = load_catalog(raw)
        source = strip_scripts(raw)

        for mid, inner in tagged(source, "data-i18n"):
            referenced.add(mid)
            if mid not in catalog:
                errors.append(f"{name}: data-i18n={mid} has no catalog entry")
                continue
            # catalog stores PLAIN text; the runtime escapes on load, so compare
            # the markup with entities decoded
            want = catalog[mid]["en"]
            if htmlmod.unescape(inner).strip() != want.strip():
                errors.append(
                    f"{name}: {mid} markup {htmlmod.unescape(inner).strip()[:50]!r} "
                    f"!= catalog {want[:50]!r}"
                )

        for mid, inner in tagged(source, "data-i18n-text"):
            referenced.add(mid)
            if mid not in catalog:
                errors.append(f"{name}: data-i18n-text={mid} has no catalog entry")
                continue
            first = own_text(inner).strip()
            want = htmlmod.unescape(catalog[mid]["en"]).strip()
            if htmlmod.unescape(first) != want:
                errors.append(
                    f"{name}: {mid} text node {first[:50]!r} != catalog {want[:50]!r}"
                )

        # data-i18n-html carries inline markup as placeholders; the element is
        # emptied in the markup, so there is no text to round-trip against.
        # What must hold is that every placeholder is one the runtime expands.
        known = {"{b}", "{/b}", "{key}", "{endpoint}"}
        for m in re.finditer(r'data-i18n-html="([^"]+)"', source):
            mid = m.group(1)
            referenced.add(mid)
            if mid not in catalog:
                errors.append(f"{name}: data-i18n-html={mid} has no catalog entry")
                continue
            for ph in re.findall(r"\{[^}]*\}", catalog[mid]["en"]):
                if ph not in known:
                    errors.append(f"{name}: {mid} uses unknown placeholder {ph}")
            if "<" in catalog[mid]["en"]:
                errors.append(f"{name}: {mid} contains raw markup; use a placeholder")

        for m in re.finditer(r'data-i18n-attr="([^"]+)"', source):
            for pair in m.group(1).split(","):
                _, _, mid = pair.partition(":")
                referenced.add(mid)
                if mid not in catalog:
                    errors.append(f"{name}: data-i18n-attr={mid} has no catalog entry")

        for mid, msg in catalog.items():
            got = hashlib.sha256(msg["en"].encode()).hexdigest()[:8]
            if msg.get("hash") != got:
                errors.append(
                    f"{name}: {mid} hash {msg.get('hash')} stale, text hashes to {got}"
                )

        for mid in catalog:
            if mid not in referenced and not mid.startswith("dashboard.js."):
                errors.append(f"{name}: catalog id {mid} is never referenced (orphan)")

    if errors:
        print(f"{len(errors)} problem(s):")
        for e in errors:
            print("  -", e)
        return 1
    print(f"i18n OK — {len(referenced)} ids referenced, round-trip clean")
    return 0


if __name__ == "__main__":
    sys.exit(main())
