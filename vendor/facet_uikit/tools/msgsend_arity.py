#!/usr/bin/env python3
"""Check every objc_msgSend call against its selector's arity.

A selector is a STRING. Nothing in the compiler relates `"beginAnimations:
context:"` to the shape of the `objc_msgSend` extern it is sent through, so a
call that declares one double where the method takes two objects compiles
cleanly and then reads whatever was in the register. That is not a hypothetical:
it crashed the gallery in `objc_retain` the first time a swipe was released, and
the same sweep found `URLWithString` written without its colon (a selector that
does not exist) and `dataUsingEncoding:` sent with no encoding.

The check is arity only — colons in the selector against argument slots in the
extern — because that is the half a script can know for certain. Types are not
checked: `msg_void_id` and `msg_void_i64` are indistinguishable here, and a
wrong one of those is still a real bug this will not catch.

    python3 vendor/facet_uikit/tools/msgsend_arity.py

Exits non-zero if anything mismatches. Three shapes are legitimately exempt and
are listed in EXEMPT: a struct passed by value arrives as its fields, so
`setTitleEdgeInsets:` really is four doubles on this ABI.
"""

import glob
import os
import re
import sys

# selector -> the argument count its C shape actually uses, when a struct passed
# by value makes that differ from the colon count.
EXEMPT = {
    "setTitleEdgeInsets:": 4,       # UIEdgeInsets: four doubles in v0-v3
    "setTextContainerInset:": 4,
    "setContentInset:": 4,
    "setScrollIndicatorInsets:": 4,
}

RUNTIME = "vendor/objc/src/runtime.cplus"
BACKEND = "vendor/facet_uikit/src"


def declared_arity(path_globs):
    """extern name -> argument slots after (recv, sel). Local decls win."""
    shared = {}
    text = open(RUNTIME).read()
    for m in re.finditer(r"fn (msg_[a-z0-9_]+)\(([^)]*)\)", text):
        params = [p for p in m.group(2).split(",") if p.strip()]
        shared[m.group(1)] = len(params) - 2
    local = {}
    for path in path_globs:
        src = open(path).read()
        for m in re.finditer(
            r'#\[link_name = "objc_msgSend"\]\s*\nextern fn (\w+)\(([^)]*)\)', src
        ):
            params = [p for p in m.group(2).split(",") if p.strip()]
            local[(path, m.group(1))] = len(params) - 2
    return shared, local


def main():
    root = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(
        os.path.abspath(__file__)))))
    os.chdir(root)
    files = sorted(glob.glob(os.path.join(BACKEND, "*.cplus")))
    shared, local = declared_arity(files)

    bad = set()
    for path in files:
        src = open(path).read()
        for m in re.finditer(
            r'\b(?:rt::)?(msg_[a-z0-9_]+)\s*\(([^;]{0,400}?)'
            r'rt::sel\(#str_ptr\("([A-Za-z:_]+)\\0"\)\)',
            src,
        ):
            name, between, sel = m.group(1), m.group(2), m.group(3)
            # A nested `rt::msg_id(cls, sel("alloc"))` inside the arguments of an
            # outer call: the selector belongs to the inner one, not this name.
            if "msg_" in between:
                continue
            want = EXEMPT.get(sel, sel.count(":"))
            have = local.get((path, name), shared.get(name))
            if have is None or have == want:
                continue
            bad.add((os.path.basename(path), name, sel, have, want))

    for f, name, sel, have, want in sorted(bad):
        print(f"{f:18} {name:24} {sel:44} declares {have}, selector takes {want}")
    if bad:
        print(f"\n{len(bad)} mismatch(es) — each one reads a register the callee never set")
        return 1
    print(f"{len(files)} files, no msgSend arity mismatches")
    return 0


if __name__ == "__main__":
    sys.exit(main())
