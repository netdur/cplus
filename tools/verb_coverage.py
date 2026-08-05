#!/usr/bin/env python3
"""verb_coverage.py — is every declared verb implemented, or written down?

`views::has_body` answers at the KIND level: all 42 kinds have a body. That is
a true statement and it was reading as a stronger one, because a body can
implement three of a control's twenty verbs and still be a body.

This answers below the kind level, in two dimensions — every declared write and
command is a `P_*` bit in its module, and every declared handler is a `fn` field
— and sorts each into one of six dispositions:

  LIVE           the apply body gates on `<mod>::P_X`, so a later write lands
  HOST-RENDERED  the node has no view; its HOST re-applies (a `span` is a run
                 in its label's attributed string). Listed in the manifest.
  CREATE-ONLY    read when the view is built, never after — and listed in the
                 manifest with the reason, because an unlisted one is debt
  DERIVED        written BACK by the backend, or read by an observer
  MODIFIER       no write of its own: it changes what another write does
  DECIDED        the manifest's cannot-ledger says AppKit cannot
  NO CARRIER     AppKit can; facet declares no thing to apply it to
  ABSENT         none of the above. This is the debt.

`--check` makes it a GATE: it fails on an absent verb, a dead handler, or a
ledger row naming a verb that does not exist. That enforces the manifest's own
oldest claim — a verb neither implemented nor listed is a bug.

Four things this has to get right, and simpler versions got each of them wrong:

  ALIASES.  The backend imports `facet/box` as `box_view`, so looking for the
  literal `box::P_COLOR` finds nothing and calls a gated verb create-only.
  Imports are parsed per file and the alias resolved back to the module.

  SCOPE.  A field name is not unique. Grepping the whole backend for
  `(*p).text` matches `label`'s body and marks `span.text`, `menu.text`,
  `menu_item.text`, `swipe_item.text` and `toolbar_item.text` create-only with
  it. A read only counts for a control whose props struct THAT function casts
  to, so the backend is split into function bodies first.

  INHERITANCE.  `on_text_changed` lives on `InputViewProps` and is inherited by
  three controls, so a body that casts to the BASE implements it for all three
  — but only for the ones some body actually bridges by KIND. Without that
  second half, `MenuItemProps.on_clicked` counts for `swipe_item`, which no
  body ever reaches.

  CARRIERS.  A carrier type is stored DECOMPOSED (`shortcut` is `shortcut_key`
  plus `shortcut_modifiers`), so a backend implementing the verb never names
  the whole. Looking only for the exact name calls it absent.

  python3 tools/verb_coverage.py            # the summary
  python3 tools/verb_coverage.py --list     # every verb, by bucket
  python3 tools/verb_coverage.py --check    # the gate
"""
import glob
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# Modules that declare no control verbs: the tree itself, the seam, the tiers.
NOT_CONTROLS = {
    "props", "vocabulary", "elements", "facet", "mount", "services", "component",
    "screen", "theme", "runtime", "runtime_macos", "application", "nav",
    "test_main", "gestures", "icons", "agent",
}

IMPORT = re.compile(r'^import\s+"([^"]+)"\s+as\s+(\w+)\s*;', re.M)
FN = re.compile(r"^fn\s+\w+", re.M)
BIT_USE = re.compile(r"\b(\w+)::([PC]_[A-Z0-9_]+)\b")
STRUCT_USE = re.compile(r"\b(\w+)::(\w+Props)\b")
# `(*p).field`, and also `(*(raw as *props::XProps)).field` — the cast form,
# which a plain `\(\*\w+\)` misses and which is how the per-kind readers in
# `text_input.cplus` reach a prop without naming a local for it.
FIELD_READ = re.compile(r"\(\*[^;{}]*?\)((?:\.\w+)+)")


def facet_modules(facet_dir):
    """module -> (props struct name, {bit: field})."""
    shared = open(os.path.join(facet_dir, "props.cplus")).read()
    shared_structs = set(re.findall(r"^struct (\w+Props)", shared, re.M))
    mods = {}
    for f in sorted(glob.glob(os.path.join(facet_dir, "*.cplus"))):
        mod = os.path.basename(f)[:-6]
        if mod in NOT_CONTROLS:
            continue
        src = open(f).read()
        bits = {b: b[2:].lower() for b in re.findall(r"^const ([PC]_[A-Z0-9_]+): u64", src, re.M)}
        if not bits:
            continue
        camel = "".join(w.capitalize() for w in mod.split("_")) + "Props"
        if camel in shared_structs:
            struct = camel
        else:
            own = re.findall(r"^struct (\w+Props)", src, re.M)
            struct = own[0] if own else None
        mods[mod] = (struct, bits)
    return mods


def backend_functions(backend_dir):
    """Every top-level fn body, with its imports resolved.

    Yields (bits_used, structs_owned, fields_read) per function. `bits_used`
    holds (module, BIT) pairs with the alias mapped back to the facet module;
    `structs_owned` holds bare struct names the body casts to.
    """
    out = []
    for f in sorted(glob.glob(os.path.join(backend_dir, "*.cplus"))):
        src = open(f).read()
        alias = {}
        for path, name in IMPORT.findall(src):
            alias[name] = path.split("/")[-1]
        starts = [m.start() for m in FN.finditer(src)] + [len(src)]
        for i in range(len(starts) - 1):
            body = src[starts[i]:starts[i + 1]]
            bits = {(alias.get(a, a), b) for a, b in BIT_USE.findall(body)}
            structs = {s for _, s in STRUCT_USE.findall(body)}
            kinds = set(re.findall(r"\bK_[A-Z0-9_]+\b", body))
            fields = set()
            for path in FIELD_READ.findall(body):
                fields.update(path.lstrip(".").split("."))
            out.append((bits, structs, fields, kinds))
    return out


def ledger(manifest, tag):
    """`module.verb` rows the backend has DECIDED AppKit cannot do.

    Read from a fenced ```cannot-ledger block rather than from the prose around
    it. Grepping the prose was tried and is what produced the wrong number
    twice: a section that merely MENTIONS `button.corner_radius` while calling
    it a gap matched as a commitment that AppKit cannot do it.
    """
    if not os.path.exists(manifest):
        return {}
    block = re.search(r"^```" + tag + r"\n(.*?)^```", open(manifest).read(), re.M | re.S)
    if not block:
        return {}
    rows = {}
    for line in block.group(1).splitlines():
        line = line.strip()
        if not line:
            continue
        name, _, reason = line.partition(" ")
        rows[name] = reason.strip()
    return rows


def struct_bodies(facet_dir):
    out = {}
    for f in glob.glob(os.path.join(facet_dir, "*.cplus")):
        out.update(re.findall(r"^struct (\w+Props) \{(.*?)\n\}", open(f).read(), re.M | re.S))
    return out


def declared_handlers(bodies, struct, seen=()):
    """(handler name, the struct that DECLARES it) for a control, bases included.

    The declaring struct is what a backend body names when it reads the field:
    `on_text_changed` lives on `InputViewProps` and is inherited by three
    controls, so a reader that casts to `InputViewProps` is implementing it for
    all three. Checking only the owning struct calls all three unwired.
    """
    out = []
    for line in bodies.get(struct, "").splitlines():
        m = re.match(r"\s*(\w+): fn\(", line)
        if m:
            out.append((m.group(1), struct))
        m = re.match(r"\s*(\w+): (\w+Props),", line)
        if m and m.group(2) not in seen:
            out += declared_handlers(bodies, m.group(2), seen + (struct,))
    return out


def handler_buckets(facet_dir, backend_dir, decided=(), blocked=()):
    """Handlers ride the shared `C_HANDLERS` bit, so they are invisible to the
    verb count above — and 52 of 76 were never wired while it read 42/42 kinds
    done. A handler that never fires is not a smaller gap than a dead verb; it
    is the same gap, one dimension over."""
    mods = facet_modules(facet_dir)
    bodies = struct_bodies(facet_dir)
    fns = backend_functions(backend_dir)
    wired, dead, ruled_out = [], [], []
    for mod, (struct, _) in sorted(mods.items()):
        if not struct:
            continue
        kind = "K_" + mod.upper()
        # A base's handler counts for a DERIVED control only if some body
        # actually bridges that control's kind to the base struct. Without
        # this, `MenuItemProps.on_clicked` — read by the menu builders — would
        # count for `swipe_item`, which no body ever reaches: swipe is recorded
        # "AppKit cannot", and a tool that calls it done hides the deferral.
        bridged = {s for _, ss, _, ks in fns if kind in ks for s in ss}
        for name, declarer in declared_handlers(bodies, struct):
            entry = f"{mod}.{name}"
            reach = {struct} | (bridged & {declarer})
            if any((s & reach) and name in r for _, s, r, _ in fns):
                wired.append(entry)
            elif entry in decided or entry in blocked:
                ruled_out.append(entry)
            else:
                dead.append(entry)
    return wired, dead, ruled_out


def reads_field(read, field):
    """Does this body read `field`, directly or through its PARTS?

    A carrier type is stored DECOMPOSED — `shortcut` is `shortcut_key` plus
    `shortcut_modifiers`, one owned field per part (see `CARRIER_TYPES` in
    gen_contract.py) — so a backend that implements the verb never names the
    whole. Looking only for the exact name calls an implemented verb absent.
    """
    if field in read:
        return True
    return any(r.startswith(field + "_") for r in read)


def buckets(facet_dir, backend_dir, recorded=()):
    mods = facet_modules(facet_dir)
    fns = backend_functions(backend_dir)
    gated = set()
    for bits, _, _, _ in fns:
        gated |= bits
    live, create_only, absent, ruled_out = [], [], [], []
    for mod, (struct, bits) in sorted(mods.items()):
        # The same inheritance rule the handler pass uses: a prop declared on a
        # BASE (`remaining_threshold` on `ItemsViewProps`) is read through the
        # base struct, and counts for this control only where some body bridges
        # this control's KIND to that struct.
        kind = "K_" + mod.upper()
        reach = {struct} | {t for _, ss, _, ks in fns if kind in ks for t in ss}
        for bit, field in bits.items():
            entry = f"{mod}.{field}"
            if (mod, bit) in gated:
                live.append(entry)
            elif entry in recorded:
                # An explicit record beats the heuristic below. Without this,
                # `toolbar_item.is_destructive` reads as create-only because a
                # SIBLING control's body reads the same base field, and the
                # ledger row saying NSToolbarItem cannot is overridden by a
                # guess. A written-down decision is not a guess.
                ruled_out.append(entry)
            elif struct and any((s & reach) and reads_field(r, field) for _, s, r, _ in fns):
                create_only.append(entry)
            else:
                absent.append(entry)
    return live, create_only, absent, ruled_out


def main():
    facet = os.path.join(ROOT, "vendor/facet/src")
    backend = os.path.join(ROOT, "vendor/facet_appkit/src")
    manifest = os.path.join(ROOT, "vendor/facet_appkit/MANIFEST.md")
    decided = ledger(manifest, "cannot-ledger")
    hosted = ledger(manifest, "host-rendered")
    no_carrier = ledger(manifest, "no-carrier")
    by_design = ledger(manifest, "create-only")
    derived = ledger(manifest, "derived")
    modifiers = ledger(manifest, "modifier")
    recorded = dict(decided)
    recorded.update(no_carrier)
    live, create_only, absent, ruled_out = buckets(facet, backend, recorded)
    blocked = [e for e in ruled_out if e in no_carrier]
    ruled_out = [e for e in ruled_out if e not in no_carrier]
    total = len(live) + len(create_only) + len(absent) + len(ruled_out) + len(blocked)
    print(f"facet_appkit verb coverage — {total} declared prop/command bits")
    print(f"  {len(live):>4}  live         gated on the dirty bit; a later write lands")
    by_host = [e for e in create_only if e in hosted]
    create_only = [e for e in create_only if e not in hosted]
    # A create-only verb nobody wrote down is DEBT, not a decision — the same
    # rule the cannot ledger follows, applied to the bucket the handoff said to
    # start with because it looks implemented from the outside.
    by_derivation = [e for e in create_only if e in derived]
    create_only = [e for e in create_only if e not in derived]
    # A MODIFIER has no write of its own: it changes what another write does.
    # `carousel.animates_scroll` decides whether `position` jumps or slides, so
    # gating it would mean re-scrolling to the current page when the flag
    # changed — a visible bug in the name of a tidy bucket.
    by_modification = [e for e in create_only if e in modifiers]
    create_only = [e for e in create_only if e not in modifiers]
    undocumented = [e for e in create_only if e not in by_design]
    create_only = [e for e in create_only if e in by_design]
    absent = absent + undocumented
    print(f"  {len(by_host):>4}  host-rendered  no view of its own; its host re-applies")
    print(f"  {len(by_derivation):>4}  derived      written BACK, or read by an observer")
    print(f"  {len(by_modification):>4}  modifier     no write of its own; it changes another's")
    print(f"  {len(create_only):>4}  create-only  by design, and the manifest says why")
    print(f"  {len(ruled_out):>4}  decided      the manifest's ledger says AppKit cannot")
    print(f"  {len(blocked):>4}  no carrier   AppKit can; facet declares no thing to apply it to")
    print(f"  {len(absent):>4}  absent       neither implemented nor decided — the debt")
    wired, dead, h_ruled = handler_buckets(facet, backend, decided, no_carrier)
    print(f"\nfacet_appkit handler coverage — {len(wired) + len(dead) + len(h_ruled)} declared handlers")
    print(f"  {len(wired):>4}  wired        the backend reads the field and calls it")
    print(f"  {len(h_ruled):>4}  decided      the manifest records why it does not fire")
    print(f"  {len(dead):>4}  never fire   neither wired nor decided — the debt")
    stale = [n for n in list(decided) + list(hosted) + list(no_carrier) + list(by_design)
             + list(derived) + list(modifiers)
             if n not in set(live + create_only + by_host + by_derivation + by_modification
                             + blocked + absent + ruled_out + wired + dead + h_ruled)]
    if stale:
        print(f"\nLEDGER NAMES {len(stale)} VERBS THAT DO NOT EXIST: {', '.join(sorted(stale))}")
    # `--check` makes this a GATE rather than a report. The manifest has always
    # claimed that a verb neither implemented nor listed is a bug; this is the
    # line that enforces it, and a ledger row naming nothing real fails too —
    # a stale row reads as a commitment and is not one.
    if "--check" in sys.argv:
        if absent or dead or stale:
            print("\nFAIL: every declared verb must be implemented or recorded.")
            return 1
        print("\nOK: every declared verb and handler is implemented or recorded.")
    if "--list" in sys.argv:
        for name, rows in (("CREATE-ONLY", create_only), ("HOST-RENDERED", by_host), ("DERIVED", by_derivation), ("MODIFIER", by_modification), ("NO CARRIER", blocked), ("ABSENT", absent),
                           ("NEVER FIRE", dead), ("DECIDED", ruled_out + h_ruled),
                           ("LIVE", live), ("WIRED", wired)):
            print(f"\n{name} ({len(rows)})")
            for r in rows:
                print(f"  {r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
