#!/usr/bin/env python3
"""Generate the en-XA pseudolocale from locales/en-US.json.

    python3 scripts/gen_pseudolocale.py            # write locales/en-XA.json
    python3 scripts/gen_pseudolocale.py --check    # fail if it would differ

en-XA is not a language. It is a rendering test that answers three questions no
English screenshot can:

- **Is anything still hardcoded?** Untranslated text stays plain ASCII and
  stands out immediately against the accented text around it.
- **Does the layout survive longer strings?** Real translations of English run
  20-40% longer; German and Finnish more. Padding to 135% makes a container
  that only fits English fail here rather than after a locale ships.
- **Is anything truncated?** The brackets mark each message's boundaries, so a
  clipped string is visible as a missing `]`.

Deterministic and regenerated rather than maintained, so it is always complete
and never stale. Placeholders and inline markup pass through untouched — an
accented `{çôûñt}` would not substitute, which would make the pseudolocale test
the pseudolocale instead of the layout.
"""
import argparse
import hashlib
import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
# Both catalogs: the wizard is half the surface en-XA has to prove out, and it
# is where the placeholder-bearing messages live.
PAIRS = [
    (ROOT / "locales/en-US.json", ROOT / "locales/en-XA.json"),
    (ROOT / "locales/setup-en-US.json", ROOT / "locales/setup-en-XA.json"),
]

# Latin-1/Latin-Extended-A lookalikes: legible as English, obviously not English.
ACCENTS = str.maketrans(
    "aeiouAEIOUbcdfghjklmnpqrstvwxyzBCDFGHJKLMNPQRSTVWXYZ",
    "áéíóúÁÉÍÓÚƀçđƒǥĥĵķŀɱñƥɋŕşŧṽŵxŷẑƁÇĐƑǤĤĴĶĿMÑƤɊŔŞŦṼŴXŶẐ",
)
# Padding is Latin text, not filler glyphs, so a reviewer can tell an
# over-long string from a rendering fault.
PAD = "escamotable"
SEGMENT = re.compile(r"(\{[^{}]*\})")


def pseudo(text: str) -> str:
    """Accent the prose, leave every {placeholder} alone, pad to ~135%."""
    parts = SEGMENT.split(text)
    accented = "".join(p if SEGMENT.fullmatch(p) else p.translate(ACCENTS) for p in parts)

    # Pad on the *prose* length: padding a string that is mostly placeholders
    # would stretch it far past what any real translation would do.
    prose = sum(len(p) for p in parts if not SEGMENT.fullmatch(p))
    need = max(1, round(prose * 0.35))
    tail = (PAD * (need // len(PAD) + 1))[:need]
    return f"[{accented} {tail}]"


def build(source: pathlib.Path) -> dict:
    src = json.loads(source.read_text())
    messages = {}
    for mid, msg in src["messages"].items():
        out = {"en": pseudo(msg["en"])}
        if "desc" in msg:
            out["desc"] = msg["desc"]
        # The hash records the SOURCE text this was generated from, so en-XA
        # goes Stale through the same mechanism as a real locale.
        out["hash"] = msg["hash"]
        if "maxLen" in msg:
            out["maxLen"] = msg["maxLen"]
        messages[mid] = out
    return {"locale": "en-XA", "state": "Generated", "messages": messages}


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--check", action="store_true", help="fail if the file is out of date")
    args = ap.parse_args()

    total = 0
    for source, target in PAIRS:
        doc = build(source)
        text = json.dumps(doc, indent=2, ensure_ascii=False) + "\n"
        total += len(doc["messages"])
        rel = target.relative_to(ROOT)
        if args.check:
            if not target.exists() or target.read_text() != text:
                print(f"{rel} is missing or stale; run scripts/gen_pseudolocale.py")
                return 1
        else:
            target.write_text(text)
            print(f"wrote {len(doc['messages'])} messages to {rel}")
    if args.check:
        print(f"en-XA up to date — {total} messages across {len(PAIRS)} catalogs")
    return 0


if __name__ == "__main__":
    sys.exit(main())
