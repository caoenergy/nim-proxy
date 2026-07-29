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



# ---------------------------------------------------------------------------
# Untagged-string lint
#
# Without this, English creeps back within two PRs: someone adds a column or a
# metric row, writes the label inline because that is what the surrounding code
# used to look like, and nothing complains. It covers ATTRIBUTES as well as
# text, because a hardcoded title= is just as untranslated and far easier to
# miss in review.
#
# Scoped deliberately:
#   - the settings surface is excluded; its ~79 strings land in 0.6.7
#   - chipHtml's interior is excluded. It interpolates into a URL and (before
#     this release) a JS context; routing a catalog value through it is the
#     thing we spent PR 3 removing. Flagging it would invite someone to
#     "fix" it by wrapping it in t(), which is a net loss.
#   - NEVER_TRANSLATE holds the units and codes that must survive verbatim.

NEVER_TRANSLATE = {
    "TTFT", "TPOT", "tok/s", "Tools/req", "Msgs/req", "p50 / p95", "p50", "p95",
    "requests / min", "req/min", "rpm", "JSON", "HTTP", "POST", "SLO", "NIM",
    "/v1", "%", "429", "401", "503", "504", "5xx",
    "requests/min", "nvapi-\u2026", "npk_\u2026",
}

def frozen(text: str) -> bool:
    """Units, status codes, key prefixes and URLs survive verbatim in every
    locale, so a hardcoded one is correct rather than an oversight."""
    return (
        text in NEVER_TRANSLATE
        or text.startswith(("http://", "https://"))
        or not re.search(r"[A-Za-z]{2}", text)
    )

# JS positions whose first argument is displayed to the operator.
DISPLAY_CALLS = re.compile(
    r"(?:\{\s*label:\s*|metricRow\(\s*|tile\(\s*|prow\(\s*|empty:\s*)'([^']{2,})'"
)
SETTINGS_START = re.compile(r"function renderSettings\(")

# The allowlist above only sees five call shapes, so every leak that survived
# extraction lived in a shape it does not scan: `{name:'…'}` chart series,
# array-literal taxonomy tables, object-literal reason maps, and bare prose in
# template literals. This is the documented trap "strings passed as arguments
# hide from string sweeps" — a lint that reports clean while ~40 English
# strings render is worse than no lint.
#
# So: find quoted PROSE anywhere in the scanned script, and exclude what is
# provably not display text rather than allowlisting the places prose may sit.
QUOTED = re.compile(r"'([^'\\\n]{2,})'|\"([^\"\\\n]{2,})\"")
# Text nodes inside template literals — `<span class="k">Superuser</span>`.
# Neither the quoted scan nor the markup scan can see these: there are no
# quotes around the text, and strip_scripts() deletes the script that holds
# it. Six English labels and a retired term shipped in setup.html's review
# panel through this hole.
TEMPLATE_LITERAL = re.compile(r"`(?:[^`\\]|\\.)*`", re.S)
INTERPOLATION = re.compile(r"\$\{[^{}]*(?:\{[^{}]*\}[^{}]*)*\}")
TEXT_NODE = re.compile(r">([^<>]+)<")
# Attribute values are not text nodes. The localizable ones (title, alt,
# placeholder, aria-label) have their own check; the rest are machinery.
ATTR_VALUE = re.compile(r"=\s*$")
# Contexts where a quoted string is machinery, not text for a human.
NOT_DISPLAY = re.compile(
    r"querySelector|getElementById|createElementNS|setAttribute\(|getAttribute\(|"
    r"classList|\.style|addEventListener|removeEventListener|localStorage|"
    r"nimproxy_|labels\[|dataset\.|\.dataset|JSON\.|console\.|new Intl|"
    r"\.split\(|\.join\(|\.replace\(|padStart|toFixed|encodeURI|fetch\("
)


def looks_like_prose(text: str) -> bool:
    """Human-facing text: a capitalised word, or two words with a space.

    Deliberately NOT flagged: lowercase single tokens (enum values, metric
    label values, CSS keywords), anything with an underscore or slash-prefix
    (identifiers, selectors, paths), and anything already frozen.
    """
    if frozen(text) or "${" in text or "_" in text:
        return False
    if text.startswith(("#", ".", "/", "--", "http")):
        return False
    # CSS values look like prose to a word counter: `var(--violet, #8B7BB8)`
    # has two words and starts with a letter. They are style, never text.
    if text.startswith("var(") or re.match(r"^[a-z-]+\(", text) or "#" in text:
        return False
    # A whole inline style attribute is also style: `display:flex;gap:20px`
    # reads as prose to a word counter. CSS declarations, never text.
    if re.match(r"^[a-z-]+\s*:", text):
        return False
    if text == "use strict":
        return False
    if not re.search(r"[A-Za-z]{3}", text):
        return False
    words = text.split()
    if len(words) >= 2 and re.match(r"^[A-Za-z]", text):
        return True
    return bool(re.match(r"^[A-Z][a-z]{2,}$", text))


def lint_runtime_helpers(name: str, raw: str) -> list:
    """Every i18n helper a page CALLS must be defined in that page.

    `node --check` sees only syntax and `cargo test` never parses the script,
    so an undefined helper is silent until the page runs. It bites hard:
    applyStatic aborts on the first throw, so ONE missing helper leaves the
    entire page untranslated rather than one string. That happened — setup.html
    called tRaw() while defining only rawMsg()."""
    body = re.search(r"<script>(.*?)</script>", raw, re.S)
    if not body:
        return []
    js = strip_comments(body.group(1))
    called = set(re.findall(r"\b(t|tRaw|tHtml|applyStatic|rawMsg|fill)\s*\(", js))
    defined = set(re.findall(r"(?:const|let|var|function)\s+(t|tRaw|tHtml|applyStatic|rawMsg|fill)\b", js))
    return [
        f"{name}: calls {fn}() but never defines it — the page will not localize"
        for fn in sorted(called - defined)
    ]


def lint_untagged(name: str, raw: str) -> list:
    errors = []
    body = re.search(r"<script>(.*?)</script>", raw, re.S)
    js = body.group(1) if body else ""

    # The settings surface is out of scope until 0.6.7.
    cut = SETTINGS_START.search(js)
    scanned = js[: cut.start()] if cut else js

    # PUBLISHERS maps a model namespace to its vendor's brand name. Brand
    # names are DATA — they arrive from the model id and are never translated,
    # so the prose detector must not see this table. Excluded by design.
    scanned = re.sub(r"const PUBLISHERS = \{.*?\n\};", "", scanned, flags=re.S)

    # chipHtml is excluded by design, not by oversight.
    scanned = re.sub(r"function chipHtml\(pub\) \{.*?\n\}", "", scanned, flags=re.S)

    for m in DISPLAY_CALLS.finditer(strip_comments(scanned)):
        text = m.group(1)
        if frozen(text):
            continue
        errors.append(f"{name}: untagged display string {text!r} — route it through t()")

    stripped = strip_comments(scanned)
    for line in stripped.splitlines():
        if NOT_DISPLAY.search(line):
            continue
        for m in QUOTED.finditer(line):
            text = m.group(1) if m.group(1) is not None else m.group(2)
            # `class="logo cdnchip"` is an attribute value, not display text.
            if ATTR_VALUE.search(line[: m.start()]):
                continue
            if looks_like_prose(text):
                errors.append(f"{name}: untagged prose {text!r} — route it through t()")

    # Bare prose sitting between tags inside a template literal.
    for m in TEMPLATE_LITERAL.finditer(stripped):
        # Interpolations become a separator, not a hole: `${n} validated ·`
        # must still expose "validated" rather than vanishing as `${`-bearing.
        lit = INTERPOLATION.sub("\x00", m.group(0))
        for node in TEXT_NODE.findall(lit):
            for piece in node.split("\x00"):
                piece = piece.strip()
                if piece and looks_like_prose(piece):
                    errors.append(
                        f"{name}: untagged prose {piece!r} in template markup"
                        f" — route it through t()"
                    )

    # Localizable attributes carrying prose, with no data-i18n-attr beside them.
    markup = strip_scripts(raw)
    for m in re.finditer(r"<[^>]*\s(title|placeholder|aria-label)=\"([^\"]{2,})\"[^>]*>", markup):
        tag, attr, text = m.group(0), m.group(1), m.group(2)
        if "data-i18n-attr" in tag or "${" in text:
            continue
        if frozen(text):
            continue
        errors.append(f"{name}: untagged {attr}={text!r} — add data-i18n-attr")
    return errors



# Terms retired by knowledge/decisions/standard-vocabulary.md. Reintroducing one
# is how a standardized interface drifts back apart: nothing else in the tree
# notices, and the next translation pass bakes the drift into eight languages.
#
# Multi-word and distinctive only, on purpose. Single ambiguous words are NOT
# listed: "window" is still correct for the rate-limit rolling window ("0 / 40
# in window"), "lane" is still correct in metric labels, "Open" and "bench"
# have unrelated legitimate senses. Banning a word that is both a retired label
# and a live domain term is the exact mistake that renamed a rate-limit counter
# during the label sweep.
RETIRED = {
    "Harness": "Client",
    "Harnesses": "Clients",
    "Conversation stickiness": "Session affinity",
    "Model-pressure governor": "Model limits",
    "Where time goes": "Latency breakdown",
    "Rate-limit pressure": "Throttling",
    "Historical provisioning": "Capacity history",
    "Dollars saved": "(removed — no honest per-model rate)",
    "All retained": "All time",
    "Earliest retained snapshot": "Oldest data point",
    "History file": "Data file",
    "exhaustions/min": "Capacity errors/min",
    "Lane slot": "Slot",
    "Avg reply": "Avg response",
    "Tool-offering": "Requests with tools",
    "Tool-using requests": "Requests using tools",
    "No reasoning-token usage seen": "No reasoning tokens",
    "selected window": "selected time range",
    "Default dashboard window": "Default time range",
    "fixed range": "Absolute",
    "following now": "Live",
    "rpm free": "Available",
    "rpm total": "Total",
    "Now rpm": "Current rate",
    "Slots in use": "Enabled keys",
    "Model pressure": "Model limits",
    "governor engaged": "(dropped — 'governor' is implementation vocabulary)",
}


def lint_retired_vocabulary(name: str, raw: str) -> list:
    """No shipped text may reintroduce a retired term.

    Scanning only catalog values was not enough: the whole point of the
    retirement is that operators stop seeing the old word, and a label that
    never made it into the catalog still renders. `rpm total` shipped in
    setup.html's review panel through exactly that hole, with CI green.
    """
    out = []
    catalog = load_catalog(raw)
    for mid, msg in sorted(catalog.items()):
        text = msg["en"] if isinstance(msg, dict) else msg
        for old, new in RETIRED.items():
            if old in text:
                out.append(
                    f"{name}: {mid} uses retired term {old!r} — "
                    f"standard vocabulary says {new!r}"
                )
    # Everything outside the catalog block: markup, template literals, quoted
    # strings. Comments are stripped so a note *about* a retirement is not
    # itself a violation.
    outside = strip_comments(
        re.sub(
            r'<script type="application/json" id="i18n-catalog">.*?</script>',
            "",
            raw,
            flags=re.S,
        )
    )
    for lineno, line in enumerate(outside.splitlines(), 1):
        for old, new in RETIRED.items():
            if old in line:
                out.append(
                    f"{name}:{lineno}: retired term {old!r} outside the catalog — "
                    f"standard vocabulary says {new!r}"
                )
    return out


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
        # both quote styles: dashboard.html uses ', setup.html uses "
        for m in re.finditer(r"""\bt(?:Raw|Html)?\(\s*['"]([a-z0-9_.]+)['"]""", strip_comments(raw)):
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

    for name in ("src/dashboard.html", "src/setup.html"):
        page = (ROOT / name).read_text()
        errors += lint_runtime_helpers(name, page)
        errors += lint_untagged(name, page)
        if 'id="i18n-catalog"' in page:
            errors += lint_retired_vocabulary(name, page)

    if errors:
        print(f"{len(errors)} problem(s):")
        for e in errors:
            print("  -", e)
        return 1
    print(f"i18n OK — {len(referenced)} ids referenced, round-trip clean")
    return 0


if __name__ == "__main__":
    sys.exit(main())
