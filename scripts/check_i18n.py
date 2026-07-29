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


def strip_comments(source: str) -> str:
    """Drop JS comments so an id merely *mentioned* in prose does not count as
    a reference and mask an orphan."""
    source = re.sub(r"/\*.*?\*/", "", source, flags=re.S)
    return re.sub(r"(?<![:'\"])//[^\n]*", "", source)


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

        # No value anywhere may carry markup or an HTML entity. Entities would
        # double-encode (values are plain text, escaped once at load); markup
        # would render literally through the escaping paths and inject through
        # any that is added later.
        for mid, msg in catalog.items():
            if "<" in msg["en"] or ">" in msg["en"]:
                errors.append(f"{name}: {mid} contains raw markup")
            if re.search(r"&(?:[a-zA-Z]+|#\d+);", msg["en"]):
                errors.append(f"{name}: {mid} contains an HTML entity; store plain text")

        # Only text-bearing attributes are localizable; the runtime enforces
        # the same set, so a mismatch here is a bug in one of the two.
        localizable = {"title", "placeholder", "aria-label", "alt"}
        for m in re.finditer(r'(<[^>]*\sdata-i18n-attr="([^"]+)"[^>]*>)', source):
            tag, spec = m.group(1), m.group(2)
            for pair in spec.split(","):
                attr, _, mid = pair.partition(":")
                referenced.add(mid)
                if attr not in localizable:
                    errors.append(f"{name}: {mid} targets non-localizable attribute {attr!r}")
                if mid not in catalog:
                    errors.append(f"{name}: data-i18n-attr={mid} has no catalog entry")
                    continue
                # round-trip: the attribute still present in the markup must be
                # exactly what the catalog will substitute
                cur = re.search(rf'\s{re.escape(attr)}="([^"]*)"', tag)
                if cur and htmlmod.unescape(cur.group(1)) != catalog[mid]["en"]:
                    errors.append(
                        f"{name}: {mid} attribute {attr}={cur.group(1)[:40]!r} "
                        f"!= catalog {catalog[mid]['en'][:40]!r}"
                    )

        for mid, msg in catalog.items():
            got = hashlib.sha256(msg["en"].encode()).hexdigest()[:8]
            if msg.get("hash") != got:
                errors.append(
                    f"{name}: {mid} hash {msg.get('hash')} stale, text hashes to {got}"
                )

        # ids used from JavaScript: t('id') / tRaw('id') / tHtml('id').
        # These must exist — an unknown id renders the raw id string to the
        # operator rather than failing loudly.
        for m in re.finditer(r"\bt(?:Raw|Html)?\(\s*'([a-z0-9_.]+)'", strip_comments(raw)):
            mid = m.group(1)
            referenced.add(mid)
            if mid not in catalog:
                errors.append(f"{name}: t('{mid}') has no catalog entry")

        for mid in catalog:
            if mid not in referenced:
                errors.append(f"{name}: catalog id {mid} is never referenced (orphan)")

    # The standalone locale files are what translators and the PR 6 pipeline
    # edit; the inline block is what ships. They must not drift.
    for page, standalone in (
        ("src/dashboard.html", "locales/en-US.json"),
        ("src/setup.html", "locales/setup-en-US.json"),
    ):
        inline = load_catalog((ROOT / page).read_text())
        disk = json.loads((ROOT / standalone).read_text())["messages"]
        if inline != disk:
            only_inline = set(inline) - set(disk)
            only_disk = set(disk) - set(inline)
            for mid in sorted(only_inline):
                errors.append(f"{standalone}: missing {mid} (present inline in {page})")
            for mid in sorted(only_disk):
                errors.append(f"{standalone}: has {mid}, absent from {page}")
            for mid in sorted(set(inline) & set(disk)):
                if inline[mid] != disk[mid]:
                    errors.append(f"{standalone}: {mid} differs from the inline catalog")

    if errors:
        print(f"{len(errors)} problem(s):")
        for e in errors:
            print("  -", e)
        return 1
    print(f"i18n OK — {len(referenced)} ids referenced, round-trip clean")
    return 0


if __name__ == "__main__":
    sys.exit(main())
