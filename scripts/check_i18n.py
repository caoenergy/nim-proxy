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


def blank_comments(source: str) -> str:
    """Like strip_comments, but preserves line count so reported line numbers
    still match the file. A multi-line `/* */` collapsed to nothing shifts every
    line after it."""
    source = re.sub(
        r"/\*.*?\*/", lambda m: "\n" * m.group(0).count("\n"), source, flags=re.S
    )
    source = re.sub(r"<!--.*?-->", lambda m: "\n" * m.group(0).count("\n"), source, flags=re.S)
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
#
# The `=` must ABUT the quote, as it does in markup (`class="x"`). Allowing
# whitespace exempted every JS assignment and comparison — `const x = 'label'`,
# `el.textContent = 'label'`, `if (s === 'label')` — which is the most common
# way to write a string in this file, and `.textContent =` is how the page
# renders labels. Tightening this costs zero findings on either page.
ATTR_VALUE = re.compile(r"[\w-]=$")
# How far back to look for the machinery that governs a quoted string. Wide
# enough to cover `document.querySelector(` and `.getAttribute(`.
NOT_DISPLAY_WINDOW = 30
# ...but the exemption belongs to the string that machinery GOVERNS, not to its
# neighbours. `bar(y.padStart(3), 'label')` put `padStart(` inside the window
# while the string is a different argument entirely, so an argument boundary
# between the two cancels the exemption.
ARG_BOUNDARY = re.compile(r"[,;]")
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
    #
    # `^[a-z-]+:` alone was too greedy — it exempted `note: …`, `error: …`,
    # `avg: …`, which are labels. A CSS declaration list either carries a `;`
    # or its single value has no spaces (`display:flex`, `position:relative`);
    # prose after the colon has spaces and no semicolon.
    if re.match(r"^[a-z-]+\s*:", text) and (
        ";" in text or " " not in text.split(":", 1)[1].strip()
    ):
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
    helpers = (
        r"message|setMessageText|setMessageAttr|applyStatic|"
        r"t|tRaw|tHtml|rawMsg|fill"
    )
    called = set(re.findall(rf"\b({helpers})\s*\(", js))
    defined = set(
        re.findall(rf"(?:const|let|var|function)\s+({helpers})\b", js)
    )
    return [
        f"{name}: calls {fn}() but never defines it — the page will not localize"
        for fn in sorted(called - defined)
    ]


I18N_TEXT_ATTRS = {"title", "placeholder", "aria-label", "alt"}


def lint_catalog_sinks(name: str, raw: str) -> list:
    """Return ``(check id, detail)`` for catalog values in unsafe contexts.

    This is deliberately a narrow source contract, not a JavaScript parser.
    The runtime exposes one plain ``message()`` value type, and every permitted
    call shape is explicit enough to identify mechanically. Browser probes
    separately prove the helpers' actual DOM behavior.
    """
    problems = []
    source = strip_comments(raw)

    if re.search(r'data-i18n-html=|\btHtml\s*\(', source):
        problems.append((
            "structured-message-html",
            f"{name}: structured catalog message uses an HTML-producing path",
        ))

    for match in re.finditer(
        r"\bsetMessageAttr\s*\([^,]+,\s*['\"]([^'\"]+)['\"]", source
    ):
        attr = match.group(1)
        if attr not in I18N_TEXT_ATTRS:
            problems.append((
                "forbidden-catalog-context",
                f"{name}: catalog value targets forbidden attribute {attr!r}",
            ))

    forbidden_patterns = (
        r"\.(?:href|src|onclick)\s*=\s*message\s*\(",
        r"\.style(?:\.[A-Za-z_$][\w$]*)?\s*=\s*message\s*\(",
        r"\b(?:script|scriptNode)\.textContent\s*=\s*message\s*\(",
        r"\b(?:style|styleNode)\.textContent\s*=\s*message\s*\(",
        r"\b(?:svg|svgNode)\.(?:innerHTML|textContent)\s*=\s*message\s*\(",
        r"(?:href|src|style|onclick)=[\"'][^\"']*\$\{message\s*\(",
    )
    for pattern in forbidden_patterns:
        if re.search(pattern, source):
            problems.append((
                "forbidden-catalog-context",
                f"{name}: catalog value enters URL/style/event/script/CSS/SVG context",
            ))

    if re.search(r"\.innerHTML\s*=\s*message\s*\(", source):
        problems.append((
            "sink-context",
            f"{name}: plain catalog value reaches an HTML parser without escaping",
        ))
    if re.search(r"\$\{message\s*\(", source):
        problems.append((
            "sink-context",
            f"{name}: plain catalog value is interpolated into an HTML string",
        ))

    return problems


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
        for m in QUOTED.finditer(line):
            text = m.group(1) if m.group(1) is not None else m.group(2)
            before = line[: m.start()]
            # NOT_DISPLAY applies to the string it GOVERNS, not to the whole
            # line. Matching per line meant one `.toFixed(` anywhere suppressed
            # every string beside it — which is how 'met', 'missed' and
            # 'no eligible traffic' render in English on a line CI calls clean.
            window = before[-NOT_DISPLAY_WINDOW:]
            gov = None
            for g in NOT_DISPLAY.finditer(window):
                gov = g
            # Exempt only if the machinery reaches THIS string — no argument or
            # statement boundary in between.
            if gov and not ARG_BOUNDARY.search(window[gov.end():]):
                continue
            # `class="logo cdnchip"` is an attribute value, not display text.
            if ATTR_VALUE.search(before):
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
    # strings. Comments are blanked rather than deleted so a note *about* a
    # retirement is not itself a violation AND the reported line number still
    # matches the file — collapsing multi-line /* */ blocks (this file is full
    # of them) shifted every number after the first one, and the render gate's
    # own comment says a gate that reports the wrong line is one nobody trusts.
    outside = blank_comments(
        re.sub(
            r'<script type="application/json" id="i18n-catalog">.*?</script>',
            lambda m: "\n" * m.group(0).count("\n"),
            raw,
            flags=re.S,
        )
    )
    # Whitespace-insensitive: a retired term wrapped across a line break is the
    # same term to the operator reading the rendered page. The untagged-prose
    # scan already spans newlines; this one guards a non-negotiable and must not
    # be the weaker of the two.
    flat = re.sub(r"\s+", " ", outside)
    offsets = [m.start() for m in re.finditer(r"\S+", outside)]
    for old, new in RETIRED.items():
        needle = re.sub(r"\s+", " ", old)
        for m in re.finditer(re.escape(needle), flat):
            # Map the flattened offset back to a real line number.
            word_index = flat.count(" ", 0, m.start())
            src_pos = offsets[word_index] if word_index < len(offsets) else 0
            out.append(
                f"{name}:{outside.count(chr(10), 0, src_pos) + 1}: retired term "
                f"{old!r} outside the catalog — standard vocabulary says {new!r}"
            )
    return out


# ---------------------------------------------------------------------------
# Selftest — prove the lints still bite
#
# `locale_v1.py --selftest` has done this for the validator since PR 5, with a
# negative fixture per check. This lint had no equivalent, so every time its
# holes were closed the injections that proved the fix lived in a scratch
# directory and evaporated. Three of the four holes below were opened or missed
# by a change that left `i18n OK` on screen.
#
# Fixtures are source snippets rather than files, because the unit under test is
# a scanner over page source. Each is spliced into a synthetic page and must trip
# its OWN check; the CONTROL rows must trip nothing, because a lint that flags
# CSS and class attributes gets switched off.

SELFTEST_CASES = [
    # (label, snippet, expected: "untagged" | "retired" | None)
    ("assignment-single", "const zzq = 'Some fresh label here';", "untagged"),
    ("assignment-double", 'const zzq = "Some fresh label here";', "untagged"),
    ("equality-compare", "if (x === 'Some fresh label here') {}", "untagged"),
    ("textContent-sink", "el.textContent = 'Some fresh label here';", "untagged"),
    ("innerHTML-sink", "el.innerHTML = 'Some fresh label here';", "untagged"),
    ("adjacent-arg", "bar(v.toFixed(1), 'Some fresh label here');", "untagged"),
    ("statement-boundary", "el.style.width = w; bar('Some fresh label here');", "untagged"),
    ("colon-prefixed-prose", "const z = `<div>note: some fresh label</div>`;", "untagged"),
    ("template-text-node", "const z = `<div><span>Some fresh label</span></div>`;", "untagged"),
    ("display-call-arg", "prow('Some fresh label here', 1);", "untagged"),
    ("retired-in-markup", "const z = `<div>Harness</div>`;", "retired"),
    # A real multi-word retired key, wrapped. NOT "Latency breakdown" — that is
    # the REPLACEMENT term. Using it here is how a scratch test that grepped for
    # a substring reported this working when only the prose scan had fired.
    ("retired-across-newline", "const z = `<div>Where time\n  goes</div>`;", "retired"),
    ("retired-in-comment", "/* Harness was retired; do not reintroduce */", None),
    ("css-declaration", "const z = 'display:flex;gap:20px';", None),
    ("css-single-value", "const z = 'position:relative';", None),
    ("class-attribute", 'const z = `<div class="logo cdnchip"></div>`;', None),
    ("real-machinery", "document.querySelector('#tab-models');", None),
    ("frozen-unit", "const z = 'tok/s';", None),
    ("lowercase-token", "const z = 'sticky';", None),
]

SINK_SELFTEST_CASES = [
    # Allowed plain-text contexts.
    ("element-text", "setMessageText(el, 'probe');", None),
    ("attr-title", "setMessageAttr(el, 'title', 'probe');", None),
    ("attr-placeholder", "setMessageAttr(el, 'placeholder', 'probe');", None),
    ("attr-aria-label", "setMessageAttr(el, 'aria-label', 'probe');", None),
    ("attr-alt", "setMessageAttr(el, 'alt', 'probe');", None),
    # Forbidden contexts are independent so a broad check cannot hide a hole.
    ("attr-href", "setMessageAttr(el, 'href', 'probe');", "forbidden-catalog-context"),
    ("attr-src", "setMessageAttr(el, 'src', 'probe');", "forbidden-catalog-context"),
    ("attr-style", "setMessageAttr(el, 'style', 'probe');", "forbidden-catalog-context"),
    ("attr-onclick", "setMessageAttr(el, 'onclick', 'probe');", "forbidden-catalog-context"),
    ("property-href", "el.href = message('probe');", "forbidden-catalog-context"),
    ("property-src", "el.src = message('probe');", "forbidden-catalog-context"),
    ("property-style", "el.style.cssText = message('probe');", "forbidden-catalog-context"),
    ("property-onclick", "el.onclick = message('probe');", "forbidden-catalog-context"),
    ("script-text", "scriptNode.textContent = message('probe');", "forbidden-catalog-context"),
    ("css-text", "styleNode.textContent = message('probe');", "forbidden-catalog-context"),
    ("raw-svg", "svgNode.innerHTML = message('probe');", "forbidden-catalog-context"),
    ("html-string", "el.innerHTML = message('probe');", "sink-context"),
    (
        "escaped-html-string",
        "el.innerHTML = '<span>' + esc(message('probe')) + '</span>';",
        None,
    ),
    (
        "structured-html",
        '<span data-i18n-html="probe"></span>',
        "structured-message-html",
    ),
]

SELFTEST_PAGE = """<!doctype html><html><body>
<script type="application/json" id="i18n-catalog">{"locale":"en-US","messages":{}}</script>
<script>
%s
</script>
</body></html>
"""


def selftest() -> int:
    """Each snippet must trip its OWN check; controls must trip nothing."""
    failures = []
    for label, snippet, want in SELFTEST_CASES:
        page = SELFTEST_PAGE % snippet
        found = set()
        if lint_untagged("selftest", page):
            found.add("untagged")
        if lint_retired_vocabulary("selftest", page):
            found.add("retired")
        if want is None:
            if found:
                failures.append(f"{label}: expected to pass, tripped {sorted(found)}")
            else:
                print(f"  ok  {label:24} passes")
        elif want not in found:
            failures.append(
                f"{label}: expected {want!r}, tripped {sorted(found) or 'nothing'}"
            )
        else:
            print(f"  ok  {label:24} trips {want}")

    for label, snippet, want in SINK_SELFTEST_CASES:
        page = SELFTEST_PAGE % snippet
        found = {check for check, _ in lint_catalog_sinks("selftest", page)}
        if want is None:
            if found:
                failures.append(f"{label}: expected to pass, tripped {sorted(found)}")
            else:
                print(f"  ok  {label:24} passes")
        elif want not in found:
            failures.append(
                f"{label}: expected {want!r}, tripped {sorted(found) or 'nothing'}"
            )
        else:
            print(f"  ok  {label:24} trips {want}")

    if failures:
        print("\nselftest FAILED:")
        for f in failures:
            print("  -", f)
        return 1
    total = len(SELFTEST_CASES) + len(SINK_SELFTEST_CASES)
    print(f"\nselftest ok — {total} cases, every check observed to fail")
    return 0


def main() -> int:
    if "--selftest" in sys.argv[1:]:
        return selftest()
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
                errors.append(f"[catalog-markup] {name}: {mid} contains raw markup")
            decoded = htmlmod.unescape(msg["en"])
            if decoded != msg["en"] and ("<" in decoded or ">" in decoded):
                errors.append(
                    f"[catalog-entity-markup] {name}: {mid} contains "
                    "entity-encoded markup"
                )

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
        errors += [
            f"[{check}] {detail}"
            for check, detail in lint_catalog_sinks(name, page)
        ]
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
