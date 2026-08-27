#!/usr/bin/env python3
"""Prop coverage for facet_android, measured against facet's own contract.

Any adjective about a backend ("early", "usable", "complete") is worthless
unless it is a NUMBER, and the number has to come from the contract rather than
from the backend's own opinion of itself. This walks facet's per-kind modules
for their `P_*` dirty bits and asks which of them each backend actually names.

    python3 vendor/facet_android/tools/parity.py            # from the repo root
    python3 vendor/facet_android/tools/parity.py --check    # non-zero if android regressed

A prop counts as implemented when the backend REFERENCES its bit IN CODE. That
is a proxy, and a deliberately loose one: a body could name a bit and do nothing
useful with it. It cannot go the other way, though — a prop nobody names is a
prop nobody honours — so the number is an UPPER BOUND, which is the right
direction for a claim to be wrong in.

Comments are stripped first. A backend explaining WHY it does not answer a verb
would otherwise be counted as answering it, which would make the honest thing to
write also the thing that inflates the number.

The same measurement as facet_gtk/tools/parity.py — the same file, with SELF
and the three floors changed. The columns are the same four backends so the
numbers stay comparable between copies.
"""

import os
import re
import sys
import glob

FACET = "vendor/facet/src"
SELF = "android"
BACKENDS = {
    "appkit": "vendor/facet_appkit/src",
    "uikit": "vendor/facet_uikit/src",
    "gtk": "vendor/facet_gtk/src",
    "android": "vendor/facet_android/src",
}
# The floor this backend has already reached. `--check` fails below it, so a
# refactor that quietly drops a verb is caught at the number rather than by
# someone noticing a control stopped working.
#
# These are LOW and rising, which is the opposite of gtk's — this backend was
# started on 2026-08-24 and the number is a progress bar rather than a guard
# against regression. Raise it whenever it moves up; that is the whole ritual.
FLOOR = 319
# Same gate on the READ half. Kept separate because the two surfaces fail
# differently: a missing prop is a control that ignores you, a missing handler
# is a control that never answers — and the gap between 45% and 35% here says
# this backend can be TOLD more than it can SAY.
HANDLER_FLOOR = 67
# The SHARED band, which every node has and which no per-kind number covers.
SHARED_FLOOR = 19

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


MANIFEST = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
                        "MANIFEST.md")


def decided_absent():
    """Names this backend has DECIDED it cannot answer, read out of MANIFEST §1.

    WHY THE TOOL READS PROSE. Everything above is an upper bound on what is
    answered; what it could not say is whether an unanswered verb had been
    LOOKED AT. Those are different states and only one of them is debt — GTK has
    no per-widget opaque hint and no safe area, and no amount of work changes
    that, so counting them against the package makes the number stop meaning
    anything and the next reader re-derives the same dead ends.

    §1's rule is that every row was looked for before it was written down, so a
    name appearing there is a decision with an argument attached. A name that is
    in NEITHER the code NOR §1 is the only thing that should raise an alarm.

    Deliberately literal: the backticked identifiers in §1's rows, nothing
    inferred. A row that forgets to name its verb does not get counted, which
    fails towards reporting debt that is not there rather than hiding debt that
    is.
    """
    if not os.path.isfile(MANIFEST):
        return set()
    body = open(MANIFEST).read()
    start = body.find("\n## 1.")
    end = body.find("\n## 2.", start + 1)
    if start < 0 or end < 0:
        return set()
    section = body[start:end]
    names = set()
    for tick in re.findall(r"`([^`]+)`", section):
        # `carousel.bounces` -> bounces ; `C_SAFE_AREA` -> C_SAFE_AREA
        leaf = tick.split(".")[-1].strip()
        if re.fullmatch(r"C_[A-Z0-9_]+", leaf):
            names.add(leaf)
        elif re.fullmatch(r"[a-z][a-z0-9_]*", leaf):
            names.add(leaf)
    return names


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


def reads_by_struct(source, declared):
    """Which Props struct each `.on_*` read actually belongs to.

    THE BLIND SPOT THIS REPLACES, and it hid a real gap for as long as it
    existed. This used to search the WHOLE backend for `.on_opened` and answer
    yes for every struct that declares one. facet_gtk fired `on_opened` /
    `on_closed` for `date_picker` and `time_picker` and NOT for `popup` — and
    the report read 68/68, because the pickers' reads counted as the popup's.
    The verb was missing behind a number that said complete.

    Attribution is BY FUNCTION, not by the nearest annotation. Three shapes had
    to be handled and each of them produced a false gap when it was not:

      * the plain read — `let p: *props::PopupProps` then `{ (*p).on_opened }`;
      * an EMBEDDED block, read through the outer pointer as
        `{ (*p).selectable_items_view.on_selection_changed }`, where the field
        name is the answer and the annotation is not;
      * TWO annotations in one body — `swipe.cplus`'s `action_clicked` types a
        `SwipeItemProps` and then a `MenuItemProps`, and a nearest-annotation
        rule gives the swipe item's `on_invoked` to the menu item.

    So every Props type annotated anywhere in a function is in scope for every
    read in it, and a read is attributed to whichever of those actually
    DECLARES that handler — which is the same disambiguation a reader does.

    A proxy, like everything else here. It cannot invent a read that is not
    there, which is the direction that matters.
    """
    out = {}
    bodies = re.split(r"\n(?=fn |impl )", source)
    for body in bodies:
        in_scope = set(re.findall(r"\*\w+::(\w+Props)\b", body))
        for m in re.finditer(r"\.(?:(\w+)\.)?(on_\w+|observe_\w+)\b", body):
            field, handler = m.group(1), m.group(2)
            if field:
                # `selectable_items_view` -> `SelectableItemsViewProps`
                embedded = "".join(x.title() for x in field.split("_")) + "Props"
                out.setdefault(embedded, set()).add(handler)
                continue
            for cand in in_scope:
                if handler in declared.get(cand, ()):
                    out.setdefault(cand, set()).add(handler)
    return out


def fires(source, handler, struct=None, scoped=None):
    """Does this backend deliver `handler` FOR `struct`?

    TWO SHAPES COUNT, and the second is the one a naive proxy misses. A backend
    usually reads the field off the props block (`{ (*p).on_click }`), but for
    the handlers facet owns the delivery of it calls the blessed API instead —
    `core::fire_focus_handler` for `on_focus`. Counting only the field access
    marks a correct implementation as missing, which pushes whoever is chasing
    the number toward touching the struct directly just to be counted.

    The blessed-API shape stays GLOBAL because facet owns that delivery for
    every kind at once; only the field read is attributed. See `reads_by_struct`.
    """
    if struct is not None and scoped is not None:
        if handler in scoped.get(struct, ()):
            return True
    elif re.search(r"\." + handler + r"\b", source):
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
    scoped = {k: reads_by_struct(src[k], H) for k in src}
    totals = {k: sum(len([h for h in hs if fires(src[k], h, s_, scoped[k])])
                     for s_, hs in H.items()) for k in src}
    print(f"\n{declared} handlers declared across facet's Props structs\n")
    for k in ("appkit", "uikit", "gtk", "android"):
        if k not in totals:
            continue
        pct = totals[k] * 100 // declared if declared else 0
        mark = "  <-- this package" if k == SELF else ""
        print(f"  {k:<8} {totals[k]:>4} / {declared}   {pct:>3}%{mark}")
    gaps = [(s, [h for h in hs if not fires(src[SELF], h, s, scoped[SELF])])
            for s, hs in sorted(H.items())]
    gaps = [(s, m) for s, m in gaps if m]
    if gaps:
        print(f"\n  {SELF} does not fire:")
        for s, m in gaps:
            print(f"    {s:24} {' '.join(m)}")
    return totals.get(SELF, 0)


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

    ATTRIBUTED PER STRUCT, for the reason the handler measure had to be: a
    global set of field names credits a kind for a touch that belonged to a
    different one, and that is exactly how four dead handlers sat behind a
    68/68. The mitigation this used to rely on — "the backend names at least one
    other bit of that module" — bounds the damage to kinds the backend has heard
    of and does nothing within them.

    So the scope rule is the same one `reads_by_struct` uses: every Props type
    annotated anywhere in a function is in scope for the reads in it, plus the
    embedded-block shape where the field qualifier names the struct outright.
    """
    per = {}
    for path in glob.glob(os.path.join(directory, "*.cplus")):
        if path.endswith("test_main.cplus"):
            continue
        text = strip_comments(open(path).read())
        for body in re.split(r"\n(?=fn |impl )", text):
            in_scope = set(re.findall(r"\*\w+::(\w+Props)\b", body))
            for m in re.finditer(r"\.\s*(?:(\w+)\.)?([a-z_][a-z_0-9]*)\b", body):
                qualifier, field = m.group(1), m.group(2)
                if qualifier:
                    embedded = "".join(x.title() for x in qualifier.split("_")) + "Props"
                    per.setdefault(embedded, set()).add(field)
                    # The qualifier is itself a field of whatever is in scope.
                    for st in in_scope:
                        per.setdefault(st, set()).add(qualifier)
                    continue
                for st in in_scope:
                    per.setdefault(st, set()).add(field)
    return per


def struct_for(module):
    """`text_field` -> `TextFieldProps`, facet's own naming convention."""
    return "".join(x.title() for x in module.split("_")) + "Props"


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
            fields = written[k].get(struct_for(module), set())
            got[k] = {p for p in props
                      if p in named
                      or (plausible and p[2:].lower() in fields)}
            totals[k] += len(got[k])
        if got.get(SELF):
            rows.append((module, len(props), len(got[SELF]),
                         sorted(p[2:].lower() for p in set(props) - got[SELF])))

    absent = decided_absent()
    print(f"facet_{SELF} — per kind, where anything is answered at all:\n")
    unrecorded = []
    for module, n, g, missing in sorted(rows, key=lambda r: -r[2]):
        open_debt = [m for m in missing if m not in absent]
        unrecorded += [f"{module}.{m}" for m in open_debt]
        line = f"  {module:14} {g:>2}/{n:<2}"
        if open_debt:
            line += "   not yet: " + " ".join(open_debt[:8])
            if len(open_debt) > 8:
                line += f" (+{len(open_debt) - 8})"
        elif missing:
            # Named in MANIFEST §1: looked at, argued, and closed.
            line += "   decided absent: " + " ".join(missing[:6])
        print(line)

    print(f"\n{declared} prop bits declared across facet's kind modules\n")
    for k in ("appkit", "uikit", "gtk", "android"):
        if k not in totals:
            continue
        pct = totals[k] * 100 // declared if declared else 0
        mark = "  <-- this package" if k == SELF else ""
        print(f"  {k:<8} {totals[k]:>4} / {declared}   {pct:>3}%{mark}")

    fired = handler_parity()

    declared_c, named = shared_band()
    print(f"\n{len(declared_c)} bits declared on facet's SHARED band\n")
    for k in ("appkit", "uikit", "gtk", "android"):
        if k not in named:
            continue
        pct = len(named[k]) * 100 // len(declared_c) if declared_c else 0
        mark = "  <-- this package" if k == SELF else ""
        print(f"  {k:<8} {len(named[k]):>4} / {len(declared_c)}   {pct:>3}%{mark}")
    missing = [c for c in declared_c if c not in named.get(SELF, [])]
    if missing:
        print(f"\n  {SELF} does not name:")
        for c in missing:
            mark = "  (decided absent — MANIFEST §1)" if c in absent else "  <-- UNRECORDED"
            print(f"    {c}{mark}")
    unrecorded += [c for c in missing if c not in absent]

    # THE ONE LINE THAT IS ACTIONABLE. Everything else on this report is a
    # measurement; this is the list of verbs that are neither answered nor
    # argued about anywhere, which is the only state that needs a decision.
    print()
    if unrecorded:
        print(f"{len(unrecorded)} unanswered and UNRECORDED — decide each, "
              f"then build it or write it into MANIFEST §1:")
        for u in unrecorded:
            print(f"    {u}")
    else:
        print("Nothing unanswered is unrecorded: every gap is either built "
              "or argued in MANIFEST §1.")
    shared = len(named.get(SELF, []))

    if "--check" in sys.argv:
        got = totals.get(SELF, 0)
        bad = False
        if got < FLOOR:
            print(f"\nFAIL: {SELF} answers {got} props, floor is {FLOOR} — a verb was dropped.",
                  file=sys.stderr)
            bad = True
        if fired < HANDLER_FLOOR:
            print(f"FAIL: {SELF} fires {fired} handlers, floor is {HANDLER_FLOOR}.",
                  file=sys.stderr)
            bad = True
        if shared < SHARED_FLOOR:
            print(f"FAIL: {SELF} names {shared} shared-band bits, floor is {SHARED_FLOOR}.",
                  file=sys.stderr)
            bad = True
        if bad:
            return 1
        print(f"\nok: props {got} (floor {FLOOR}), handlers {fired} "
              f"(floor {HANDLER_FLOOR}), shared {shared} (floor {SHARED_FLOOR})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
