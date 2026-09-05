#!/usr/bin/env python3
"""Generate the diagnostic docs from the single-source catalog docs/lang/errors.toml.

Outputs (both carry a "generated — do not edit" header):
  - docs/lang/errors.md                     (maintainer reference, in this repo)
  - the cplus-lang.dev /docs/error-codes page (the public manual)

The same catalog is intended to back a future `cpc explain <CODE>`, so the CLI,
errors.md, and the website cannot drift. Edit docs/lang/errors.toml, then run:

    python3 docs/lang/gen_errors.py
    python3 docs/lang/gen_errors.py --site-md /path/to/error-codes.md   # override site path

The site page is SKIPPED when the newest version folder names an already-released
version (a `v<version>` git tag exists): between releases that folder is frozen
history, and writing the in-progress catalog there would publish unreleased
content as part of a shipped version. `--site-md` overrides the skip.
"""
import argparse, subprocess, sys, tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
DEFAULT_ERRORS  = HERE / "errors.toml"
DEFAULT_ERRORSMD = HERE / "errors.md"
SITE_DOCS = Path("/Users/adel/Workspace/cplus.dev/resources/content/docs")


def newest_site_version(root=SITE_DOCS):
    """The highest-numbered version folder under the site's docs/, or None.

    The site keeps every released manual version as a frozen archive and serves
    only the newest one from the clean /docs URLs, so the newest folder is the
    one a regeneration should land in. This used to be a hardcoded path, which
    silently rewrote a long-frozen archive once the manual moved on; resolve it
    instead, and pass --site-md to target a specific version deliberately.
    """
    if not root.is_dir():
        return None
    found = []
    for p in root.iterdir():
        if not p.is_dir():
            continue
        try:
            found.append((tuple(int(n) for n in p.name.split(".")), p))
        except ValueError:
            continue  # not a version folder
    return max(found)[1] if found else None

def is_released(version, repo=HERE.parent.parent):
    """True when `v<version>` is a git tag in the compiler repo.

    Site version folders are created at RELEASE, so between two releases the
    newest folder is the one that already shipped and its page is frozen
    history. Writing the in-progress catalog there republishes unreleased
    syntax as though it were part of a tagged version — which happened on
    2026-09-05, putting v0.0.28 content into the released 0.0.27 page.

    `newest_site_version` cannot see this on its own: the compiler still
    reports the shipped version during the next cycle, so "newest folder" and
    "current version" agree right up to the moment they are both wrong.
    """
    try:
        out = subprocess.run(["git", "-C", str(repo), "tag", "--list", f"v{version}"],
                             capture_output=True, text=True, timeout=10)
    except (OSError, subprocess.SubprocessError):
        return False  # no git, no claim — write as before
    return out.stdout.strip() != ""

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
    out = [f'### {c["id"]} · {c["title"]}', "", c["cause"], ""]
    # `example` is optional. A code that is not currently reachable has nothing
    # honest to show — E1003 (named args on an indirect call) is shadowed by
    # E1002 today, so it carries no example. Crashing on that made the whole
    # file un-regenerable, which silently froze it as codes were added.
    if c.get("example"):
        out += [f'```{fence}', c["example"].rstrip("\n"), "```", ""]
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

SITE_HEADER = ("<!-- GENERATED FILE — do not edit by hand. Source: docs/lang/errors.toml in "
               "the cplus compiler repo (github.com/netdur/cplus); regenerate with "
               "`python3 docs/lang/gen_errors.py`. -->\n")
ERRORSMD_HEADER = ("<!-- GENERATED from docs/lang/errors.toml by docs/lang/gen_errors.py — do not "
                   "edit by hand. This is the maintainer reference; the public copy is the "
                   "cplus-lang.dev /docs/error-codes page. -->\n")

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--errors", type=Path, default=DEFAULT_ERRORS)
    ap.add_argument("--errors-md", type=Path, default=DEFAULT_ERRORSMD)
    ap.add_argument("--site-md", type=Path, default=None,
                    help="site page to write; defaults to the newest version folder under "
                         f"{SITE_DOCS}")
    a = ap.parse_args()
    cat = load(a.errors)

    a.errors_md.write_text(render(cat, frontmatter=False, maintainer=True,
                                  header=ERRORSMD_HEADER))
    print(f"wrote {a.errors_md} ({len(cat)} codes)")

    site_md = a.site_md
    explained = False
    if site_md is None:
        newest = newest_site_version()
        if newest is None:
            print(f"skip site page: no version folder under {SITE_DOCS}", file=sys.stderr)
            explained = True
        elif is_released(newest.name):
            # Frozen history — refuse it. An explicit --site-md is a deliberate
            # choice and is always honoured, released version included.
            print(f"skip site page: {newest.name} is a RELEASED version and its page is "
                  f"frozen; regenerating would publish unreleased content as part of it.\n"
                  f"  create the next version's folder under {SITE_DOCS}, or pass "
                  f"--site-md to target one deliberately.", file=sys.stderr)
            explained = True
        else:
            site_md = newest / "error-codes.md"
    if site_md is None:
        if not explained:
            print(f"skip site page: no version folder under {SITE_DOCS}", file=sys.stderr)
    elif site_md.parent.is_dir():
        site_md.write_text(render(cat, frontmatter=True, maintainer=False,
                                  header=SITE_HEADER))
        print(f"wrote {site_md} ({len(cat)} codes)")
    else:
        print(f"skip site page: {site_md.parent} not found", file=sys.stderr)

if __name__ == "__main__":
    main()
