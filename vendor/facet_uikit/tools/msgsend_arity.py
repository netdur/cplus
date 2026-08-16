#!/usr/bin/env python3
"""The backend's static checks — the things the compiler cannot catch.

Two of them, both earned by a crash:

  1. every objc_msgSend call against its selector's arity;
  2. `mount::mount` outside the window host.

Check every objc_msgSend call against its selector's arity.

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


# ---- 2. mount vs realise -----------------------------------------------------
# `mount::mount` OPENS A WINDOW — it appends the root to the application's window
# list, and every later tick walks that list. A recycled row must not be one: the
# cell frees its node when the row scrolls away, and the window entry becomes a
# dangling pointer the next tick reads. Measured: 25 windows for a 24-row tree,
# then a segfault in `flex_layout.Node.attachment` from the run-loop observer.
#
# facet ships `realise` / `unrealise` for exactly this — the same walk, minus the
# window — and says so in a comment above them. So `mount::mount` belongs in the
# window host and nowhere else.
WINDOW_HOST = "window.cplus"


def check_mount(files):
    bad = []
    for path in files:
        if os.path.basename(path) == WINDOW_HOST:
            continue
        for i, line in enumerate(open(path), 1):
            if "mount::mount(" in line:
                bad.append((os.path.basename(path), i, line.strip()))
    for f, i, line in bad:
        print(f"{f}:{i}  mount::mount outside {WINDOW_HOST} — use mount::realise")
        print(f"    {line}")
    return len(bad)


# ---- 3. an association that does not retain ----------------------------------
# `synth::set_associated` uses OBJC_ASSOCIATION_ASSIGN — it does NOT retain. So
# storing anything that came back AUTORELEASED leaves the association pointing
# at freed memory as soon as the pool drains, which is the end of the current
# run-loop turn. The write succeeds, the read one event later does not.
#
# That is not hypothetical: a swipe stored its offset as `numberWithDouble:`,
# wrote it on every Changed event and read it once on Ended — so it crashed on
# release, every time, and never during the drag.
#
# The fix is `retain_associated`, or not storing an object at all. Both are
# fine; a factory result stored through the assigning call is not.
FACTORIES = ("numberWith", "stringWith", "arrayWith", "dictionaryWith",
             "dataWith", "colorWith", "valueWith", "nsstring(")


def check_associations(files):
    bad = []
    for path in files:
        src = open(path).read()
        # the call and its argument may wrap across lines
        for m in re.finditer(r"synth::set_associated\((.{0,240}?)\);", src, re.S):
            arg = m.group(1)
            if any(f in arg for f in FACTORIES):
                line = src[: m.start()].count("\n") + 1
                bad.append((os.path.basename(path), line, " ".join(arg.split())[:90]))
    for f, line, arg in bad:
        print(f"{f}:{line}  set_associated does not retain — this value is autoreleased")
        print(f"    {arg}")
    return len(bad)


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
    mounts = check_mount(files)
    if mounts:
        print(f"{mounts} stray mount::mount — each one leaks a window entry that "
              f"dangles when the node is freed")
        return 1
    print("no mount::mount outside the window host")
    assoc = check_associations(files)
    if assoc:
        print(f"{assoc} assigning association(s) holding an autoreleased object — "
              f"each dangles when the pool drains")
        return 1
    print("no assigning associations holding autoreleased objects")
    return 0


if __name__ == "__main__":
    sys.exit(main())
