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
FLOOR = 353
# Same gate on the READ half. Kept separate because the two surfaces fail
# differently: a missing prop is a control that ignores you, a missing handler
# is a control that never answers.
HANDLER_FLOOR = 68
# The SHARED band, which had no floor at all — and that is how four verbs every
# node has (`C_ANIMATE`, `C_TRANSFORM`, `C_SHADOW`, `C_CLIP`) sat unanswered
# while the two numbers above read 98% and 100%. See `shared_band`.
SHARED_FLOOR = 14

# Handlers facet fires ITSELF, from `mount.cplus`'s post-walk notification
# queue (M4). A backend neither can nor should wire them, so counting them
# against one is measuring the wrong thing — facet_appkit "answers" both only
# by mentioning them in a comment.
FACET_FIRED = {"on_attach", "on_detach"}


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


def shared_band():
    """The `C_*` bits every node has, and which backends name them.

    THE BLIND SPOT THIS TOOL HAD. Everything above counts `P_*` — per-kind bits,
    declared in facet's kind modules. But facet also declares a SHARED band on
    `props.cplus`: opacity, background, corner radius, transform, shadow, clip,
    the animation verbs. Those are `C_*`, they were never matched by any pattern
    here, and a backend could ignore all of them and still print 98%.

    Measured when this was added: gtk named 0 of C_ANIMATE and C_TRANSFORM, and
    listed C_SHADOW and C_CLIP in a bit-set nothing acted on.

    Bits that are pure BOOKKEEPING are excluded by name — a blanket word, a
    sentinel, a mask — because they are not verbs a backend answers.
    """
    text = open(os.path.join(FACET, "props.cplus")).read()
    declared = [c for c in re.findall(r"^const (C_[A-Z0-9_]+)\s*:", text, re.M)
                if c not in NOT_A_VERB]
    out = {}
    for k, d in BACKENDS.items():
        if not os.path.isdir(d):
            continue
        src = strip_comments("".join(
            open(p).read() for p in glob.glob(os.path.join(d, "*.cplus"))
            if not p.endswith("test_main.cplus")))
        out[k] = sorted(c for c in declared if re.search(r"\bprops::" + c + r"\b", src))
    return declared, out


# Words, masks and sentinels rather than verbs: nothing "implements" them.
NOT_A_VERB = {
    "C_ALL_STATE",     # the blanket touch
    "C_RESTYLE",       # C_ALL_STATE minus one bit
    "C_COMMANDS",      # the verb group, as a mask
    "C_LAYOUT",        # raised by geometry writes, answered by the layout pass
}


def handlers():
    """Props struct -> its handler fields, from facet's own declarations."""
    out = {}
    for path in glob.glob(os.path.join(FACET, "*.cplus")):
        if path.endswith("test_main.cplus"):
            continue
        for m in re.finditer(r"struct (\w+Props) \{(.*?)\n\}",
                             open(path).read(), re.S):
            hs = sorted({h for h in re.findall(
                r"^\s+(on_\w+|observe_\w+): fn\(", m.group(2), re.M)
                if h not in FACET_FIRED})
            if hs:
                out[m.group(1)] = hs
    return out


def fires(source, handler):
    """Does this backend deliver `handler`?

    TWO SHAPES COUNT, and the second is the one a naive proxy misses. A backend
    usually reads the field off the props block (`{ (*p).on_click }`), but for
    the handlers facet owns the delivery of it calls the blessed API instead —
    `core::fire_focus_handler` for `on_focus`. Counting only the field access
    marks a correct implementation as missing, which pushes whoever is chasing
    the number toward touching the struct directly just to be counted.
    """
    if re.search(r"\." + handler + r"\b", source):
        return True
    stem = handler[3:] if handler.startswith("on_") else handler
    return re.search(r"fire_" + stem + r"_handler\b", source) is not None


def handler_parity():
    src = {}
    for k, d in BACKENDS.items():
        if not os.path.isdir(d):
            continue
        src[k] = strip_comments("".join(
            open(p).read() for p in glob.glob(os.path.join(d, "*.cplus"))
            if not p.endswith("test_main.cplus")))
    H = handlers()
    declared = sum(len(v) for v in H.values())
    totals = {k: sum(len([h for h in hs if fires(src[k], h)])
                     for hs in H.values()) for k in src}
    print(f"\n{declared} handlers declared across facet's Props structs\n")
    for k in ("appkit", "uikit", "gtk", "android"):
        if k not in totals:
            continue
        pct = totals[k] * 100 // declared if declared else 0
        mark = "  <-- this package" if k == "gtk" else ""
        print(f"  {k:<8} {totals[k]:>4} / {declared}   {pct:>3}%{mark}")
    gaps = [(s, [h for h in hs if not fires(src["gtk"], h)])
            for s, hs in sorted(H.items())]
    gaps = [(s, m) for s, m in gaps if m]
    if gaps:
        print("\n  gtk does not fire:")
        for s, m in gaps:
            print(f"    {s:24} {' '.join(m)}")
    return totals.get("gtk", 0)


def field_touches(directory):
    """Props fields the backend READS or ASSIGNS.

    THREE SHAPES ANSWER A PROP, and the bit-reference proxy only sees the first.

      * an APPLIED prop is written onto the control in an `apply_*` body, gated
        on its dirty bit -- `dirty & label::P_TEXT`. The bit is named, so the
        proxy sees it.
      * a DERIVED prop goes the other way: the backend writes it BACK into the
        props block so the application can read it (`is_scrolling`). Nothing is
        gated, so no bit is ever named.
      * a CONSULTED prop is read at event time rather than applied -- a text
        field's `return_key` decides what Enter does, inside the handler, and
        there is no apply-time write it could be gated on.

    The last two are answered by touching the FIELD, and scoring them absent
    pushed the author toward naming a bit for no reason just to be counted --
    which is the measurement changing the code rather than describing it.

    A field name is not unique across Props structs (`text` is in half of
    them), so a touch alone would credit every kind that happens to share the
    name — measured: it moved facet_android from 6 to 39 without a line of
    android changing. So the caller pairs it with EVIDENCE THAT THE BACKEND
    IMPLEMENTS THAT KIND AT ALL: a field touch credits `<module>::P_<FIELD>`
    only when the backend also names at least one other bit of that module.
    A backend that has never heard of `carousel` cannot be credited with
    `carousel.is_scrolling` because something else in it touches an
    `is_scrolling` field.
    """
    out = set()
    for path in glob.glob(os.path.join(directory, "*.cplus")):
        if path.endswith("test_main.cplus"):
            continue
        text = strip_comments(open(path).read())
        for f in re.findall(r"\.\s*([a-z_][a-z_0-9]*)\b", text):
            out.add(f)
    return out


def main():
    root = os.path.dirname(os.path.dirname(os.path.dirname(
        os.path.dirname(os.path.abspath(__file__)))))
    os.chdir(root)
    if not os.path.isdir(FACET):
        print("run me from the repo root", file=sys.stderr)
        return 2

    seen = {k: referenced(d) for k, d in BACKENDS.items() if os.path.isdir(d)}
    written = {k: field_touches(d) for k, d in BACKENDS.items() if os.path.isdir(d)}
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
            named = seen[k].get(module, set())
            # The field-touch fallback needs evidence this backend implements
            # the KIND — see `field_touches`. One named bit is that evidence.
            plausible = len(named) > 0
            got[k] = {p for p in props
                      if p in named
                      or (plausible and p[2:].lower() in written[k])}
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

    fired = handler_parity()

    declared_c, named = shared_band()
    print(f"\n{len(declared_c)} bits declared on facet's SHARED band\n")
    for k in ("appkit", "uikit", "gtk", "android"):
        if k not in named:
            continue
        pct = len(named[k]) * 100 // len(declared_c) if declared_c else 0
        mark = "  <-- this package" if k == "gtk" else ""
        print(f"  {k:<8} {len(named[k]):>4} / {len(declared_c)}   {pct:>3}%{mark}")
    missing = [c for c in declared_c if c not in named.get("gtk", [])]
    if missing:
        print("\n  gtk does not name:")
        for c in missing:
            print(f"    {c}")
    shared = len(named.get("gtk", []))

    if "--check" in sys.argv:
        got = totals.get("gtk", 0)
        bad = False
        if got < FLOOR:
            print(f"\nFAIL: gtk answers {got} props, floor is {FLOOR} — a verb was dropped.",
                  file=sys.stderr)
            bad = True
        if fired < HANDLER_FLOOR:
            print(f"FAIL: gtk fires {fired} handlers, floor is {HANDLER_FLOOR}.",
                  file=sys.stderr)
            bad = True
        if shared < SHARED_FLOOR:
            print(f"FAIL: gtk names {shared} shared-band bits, floor is {SHARED_FLOOR}.",
                  file=sys.stderr)
            bad = True
        if bad:
            return 1
        print(f"\nok: props {got} (floor {FLOOR}), handlers {fired} "
              f"(floor {HANDLER_FLOOR}), shared {shared} (floor {SHARED_FLOOR})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
