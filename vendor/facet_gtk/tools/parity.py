#!/usr/bin/env python3
"""Prop coverage for facet_gtk, measured against facet's own contract.

Any adjective about a backend ("early", "usable", "complete") is worthless
unless it is a NUMBER, and the number has to come from the contract rather than
from the backend's own opinion of itself. This walks facet's per-kind modules
for their `P_*` dirty bits and asks which of them each backend actually names.

    python3 vendor/facet_gtk/tools/parity.py            # from the repo root
    python3 vendor/facet_gtk/tools/parity.py --check    # non-zero if gtk regressed

A prop counts as implemented when the backend REFERENCES its bit IN CODE. That
is a proxy, and a deliberately loose one: a body could name a bit and do nothing
useful with it. It cannot go the other way, though — a prop nobody names is a
prop nobody honours — so the number is an UPPER BOUND, which is the right
direction for a claim to be wrong in.

Comments are stripped first. A backend explaining WHY it does not answer a verb
would otherwise be counted as answering it, which would make the honest thing to
write also the thing that inflates the number.

The same measurement as facet_uikit/tools/parity.py, widened to every backend
so the columns are comparable.
"""

import os
import re
import sys
import glob

FACET = "vendor/facet/src"
BACKENDS = {
    "appkit": "vendor/facet_appkit/src",
    "uikit": "vendor/facet_uikit/src",
    "gtk": "vendor/facet_gtk/src",
    "android": "vendor/facet_android/src",
}
# The floor this backend has already reached. `--check` fails below it, so a
# refactor that quietly drops a verb is caught at the number rather than by
# someone noticing a control stopped working.
FLOOR = 347


def strip_comments(text):
    """Line comments out, so PROSE does not count as an implementation.

    The regex below matches `module::P_BIT` anywhere, and a comment saying "NOT
    `menu_item::P_IS_DESTRUCTIVE`, which this backend does not answer" named the
    bit as loudly as an implementation would. A measurement that a sentence can
    move is not a measurement — and the sentence that moved it was one written
    to be honest about a gap, which is the worst possible thing to punish.
    """
    return "\n".join(line.split("//", 1)[0] for line in text.splitlines())


def referenced(directory):
    """module basename -> set of P_* constants the backend names."""
    hits = {}
    for path in glob.glob(os.path.join(directory, "*.cplus")):
        if path.endswith("test_main.cplus"):
            continue
        text = strip_comments(open(path).read())
        alias = {}
        for module, name in re.findall(
            r'import\s+"(?:\.\./)?(?:facet/)?([\w/]+)"\s+as\s+(\w+)\s*;', text
        ):
            alias[name] = module.split("/")[-1]
        for name, prop in re.findall(r"\b(\w+)::(P_[A-Z0-9_]+)\b", text):
            hits.setdefault(alias.get(name, name), set()).add(prop)
    return hits


def main():
    root = os.path.dirname(os.path.dirname(os.path.dirname(
        os.path.dirname(os.path.abspath(__file__)))))
    os.chdir(root)
    if not os.path.isdir(FACET):
        print("run me from the repo root", file=sys.stderr)
        return 2

    seen = {k: referenced(d) for k, d in BACKENDS.items() if os.path.isdir(d)}
    totals = {k: 0 for k in seen}
    declared = 0
    rows = []

    for path in sorted(glob.glob(os.path.join(FACET, "*.cplus"))):
        module = os.path.basename(path)[:-6]
        props = sorted(set(re.findall(
            r"^\s*const (P_[A-Z0-9_]+)\s*:", open(path).read(), re.M)))
        if not props:
            continue
        declared += len(props)
        got = {}
        for k in seen:
            got[k] = {p for p in props if p in seen[k].get(module, ())}
            totals[k] += len(got[k])
        if got.get("gtk"):
            rows.append((module, len(props), len(got["gtk"]),
                         sorted(p[2:].lower() for p in set(props) - got["gtk"])))

    print("facet_gtk — per kind, where anything is answered at all:\n")
    for module, n, g, missing in sorted(rows, key=lambda r: -r[2]):
        line = f"  {module:14} {g:>2}/{n:<2}"
        if missing:
            line += "   not yet: " + " ".join(missing[:8])
            if len(missing) > 8:
                line += f" (+{len(missing) - 8})"
        print(line)

    print(f"\n{declared} prop bits declared across facet's kind modules\n")
    for k in ("appkit", "uikit", "gtk", "android"):
        if k not in totals:
            continue
        pct = totals[k] * 100 // declared if declared else 0
        mark = "  <-- this package" if k == "gtk" else ""
        print(f"  {k:<8} {totals[k]:>4} / {declared}   {pct:>3}%{mark}")

    if "--check" in sys.argv:
        got = totals.get("gtk", 0)
        if got < FLOOR:
            print(f"\nFAIL: gtk covers {got}, floor is {FLOOR} — a verb was dropped.",
                  file=sys.stderr)
            return 1
        print(f"\nok: gtk covers {got} (floor {FLOOR})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
