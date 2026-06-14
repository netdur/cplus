#!/usr/bin/env python3
"""Generate the diagnostic docs from the single-source catalog docs/errors.toml.

Outputs (both carry a "generated — do not edit" header):
  - docs/ERRORS.md                          (maintainer reference, in this repo)
  - the cplus-lang.dev /docs/error-codes page (the public manual)

The same catalog is intended to back a future `cpc explain <CODE>`, so the CLI,
ERRORS.md, and the website cannot drift. Edit docs/errors.toml, then run:

    python3 docs/gen_errors.py
    python3 docs/gen_errors.py --site-md /path/to/error-codes.md   # override site path
"""
import argparse, sys, tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_ERRORS  = HERE / "errors.toml"
DEFAULT_ERRORSMD = HERE / "ERRORS.md"
DEFAULT_SITEMD = Path("/Users/adel/Workspace/cplus.dev/resources/content/docs/0.0.22/error-codes.md")

# Display order of categories (entries within a category sort by id).
CATEGORY_ORDER = [
    "Lexical", "Parser", "Names, types, and items", "Control flow and matching",
    "Ownership and borrowing", "Modules, paths, and visibility", "Generics and bounds",
    "Unsafe, FFI, and intrinsics", "Compile-time builtins", "Real-time contracts",
    "Attributes", "const / static / char", "Targets and packages", "Warnings",
]

FRONTMATTER = """---
title:       "Error codes"
slug:        "error-codes"
section:     "Reference"
order:       2
description: "The complete index of C+ compiler diagnostics — every E and W code, with what it means, a minimal example that triggers it, and the fix. Generated from the compiler's diagnostic catalog so the manual cannot drift from the compiler."
---
"""

INTRO = """# Error codes

Every C+ diagnostic carries a numbered code, a source span, and often a machine-applicable suggestion. `cpc --diagnostics=json` emits the same information in a machine-readable shape for editors and agents. Codes prefixed with **W** are non-fatal warnings; the build continues. The normative ranges and what each phase owns are fixed in [§20 of the language specification](/docs/spec).

This is the complete index — **{total} codes**. Each entry gives the meaning, a minimal example that triggers it, and the typical fix. **{checked}** of the examples are reproduced directly by `cpc check`; the rest need a multi-file project, a `--target`, or a build-time file, and say so in the example.
"""

def load(path):
    return tomllib.load(open(path, "rb"))["code"]

def grouped(cat):
    known = set(CATEGORY_ORDER)
    for c in cat:
        if c["category"] not in known:
            print(f"WARN unknown category {c['category']!r} ({c['id']})", file=sys.stderr)
    order = {c: i for i, c in enumerate(CATEGORY_ORDER)}
    cats = sorted({c["category"] for c in cat}, key=lambda c: order.get(c, 99))
    return [(cat_name, sorted((c for c in cat if c["category"] == cat_name),
                              key=lambda c: c["id"])) for cat_name in cats]

def entry_md(c, *, maintainer):
    fence = c.get("lang", "cplus")
    out = [f'### {c["id"]} · {c["title"]}', "", c["cause"], "",
           f'```{fence}', c["example"].rstrip("\n"), "```", ""]
    if c.get("note"):
        out += [f'*{c["note"]}*', ""]
    out += [f'**Fix.** {c["fix"]}', ""]
    if maintainer:
        bits = [f'repro: {c.get("repro","?")}']
        if c.get("emit_site"): bits.append(c["emit_site"])
        if c.get("test"):      bits.append(f'test {c["test"]}')
        out += [f'<sub>{" · ".join(bits)}</sub>', ""]
    return "\n".join(out)

def render(cat, *, frontmatter, maintainer, header):
    total = len(cat)
    checked = sum(1 for c in cat if c.get("repro") == "checked")
    parts = []
    if frontmatter:
        parts.append(FRONTMATTER)
    parts.append(header)
    parts.append(INTRO.format(total=total, checked=checked))
    for name, items in grouped(cat):
        parts.append(f"## {name}\n")
        parts += [entry_md(c, maintainer=maintainer) for c in items]
    return "\n".join(parts).rstrip() + "\n"

SITE_HEADER = ("<!-- GENERATED FILE — do not edit by hand. Source: docs/errors.toml in "
               "the cplus compiler repo (github.com/netdur/cplus); regenerate with "
               "`python3 docs/gen_errors.py`. -->\n")
ERRORSMD_HEADER = ("<!-- GENERATED from docs/errors.toml by docs/gen_errors.py — do not "
                   "edit by hand. This is the maintainer reference; the public copy is the "
                   "cplus-lang.dev /docs/error-codes page. -->\n")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--errors", type=Path, default=DEFAULT_ERRORS)
    ap.add_argument("--errors-md", type=Path, default=DEFAULT_ERRORSMD)
    ap.add_argument("--site-md", type=Path, default=DEFAULT_SITEMD)
    a = ap.parse_args()
    cat = load(a.errors)

    a.errors_md.write_text(render(cat, frontmatter=False, maintainer=True,
                                  header=ERRORSMD_HEADER))
    print(f"wrote {a.errors_md} ({len(cat)} codes)")
    if a.site_md.parent.is_dir():
        a.site_md.write_text(render(cat, frontmatter=True, maintainer=False,
                                    header=SITE_HEADER))
        print(f"wrote {a.site_md} ({len(cat)} codes)")
    else:
        print(f"skip site page: {a.site_md.parent} not found", file=sys.stderr)

if __name__ == "__main__":
    main()
