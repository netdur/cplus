#!/usr/bin/env python3
"""Prop parity between facet_appkit and facet_uikit.

The claim in README.md is a number, so it has to be a MEASUREMENT. This walks
facet's per-kind contract modules for their `P_*` dirty bits, then asks which of
them each backend actually names — and reports every prop one backend honours
and the other does not.

    python3 vendor/facet_uikit/tools/parity.py          # from the repo root

A prop counts as implemented when the backend REFERENCES its bit. That is a
proxy, and a deliberately loose one: a body could name a bit and do nothing
useful with it. It cannot go the other way, though — a prop nobody names is a
prop nobody honours — so the number is an upper bound that catches the failure
this file exists to catch, which is a verb silently going unimplemented.

Import aliases are resolved per file, because the backends do not import a
contract module under its own name (`facet/text_field` arrives as `tf`).
"""

import os
import re
import sys
import glob

FACET = "vendor/facet/src"
BACKENDS = {"appkit": "vendor/facet_appkit/src", "uikit": "vendor/facet_uikit/src"}


def referenced(directory):
    """module basename -> set of P_* constants the backend names."""
    hits = {}
    for path in glob.glob(os.path.join(directory, "*.cplus")):
        if path.endswith("test_main.cplus"):
            continue
        text = open(path).read()
        alias = {}
        for module, name in re.findall(
            r'import\s+"(?:\.\./)?(?:facet/)?([\w/]+)"\s+as\s+(\w+)\s*;', text
        ):
            alias[name] = module.split("/")[-1]
        for name, prop in re.findall(r"\b(\w+)::(P_[A-Z0-9_]+)\b", text):
            hits.setdefault(alias.get(name, name), set()).add(prop)
    return hits


def main():
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(
        os.path.abspath(__file__)))))
    os.chdir(root)
    if not os.path.isdir(FACET):
        print("run me from the repo root", file=sys.stderr)
        return 2

    seen = {k: referenced(d) for k, d in BACKENDS.items()}
    total_ak = total_uk = 0
    gaps = []

    for path in sorted(glob.glob(os.path.join(FACET, "*.cplus"))):
        module = os.path.basename(path)[:-6]
        props = sorted(set(re.findall(
            r"^\s*const (P_[A-Z0-9_]+)\s*:", open(path).read(), re.M)))
        if not props:
            continue
        ak = {p for p in props if p in seen["appkit"].get(module, ())}
        uk = {p for p in props if p in seen["uikit"].get(module, ())}
        if not ak:
            continue
        total_ak += len(ak)
        total_uk += len(ak & uk)
        missing = sorted(ak - uk)
        extra = sorted(uk - ak)
        if missing or extra:
            gaps.append((module, len(ak), len(ak & uk), missing, extra))

    for module, n_ak, n_uk, missing, extra in sorted(gaps, key=lambda r: -len(r[3])):
        line = f"{module:16} appkit={n_ak:3} uikit={n_uk:3}"
        if missing:
            line += "  UIKIT MISSING: " + " ".join(p[2:].lower() for p in missing)
        if extra:
            line += "  APPKIT MISSING: " + " ".join(p[2:].lower() for p in extra)
        print(line)

    pct = total_uk * 100 // total_ak if total_ak else 0
    print(f"\nappkit={total_ak}  uikit={total_uk}  ({pct}%)")
    print("every remaining gap must appear in vendor/facet_uikit/MANIFEST.md §1")
    return 0


if __name__ == "__main__":
    sys.exit(main())
