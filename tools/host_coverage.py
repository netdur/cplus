#!/usr/bin/env python3
"""host_coverage.py — does each backend fill facet's RUNTIME-tier seams?

THE HOLE THIS CLOSES. `verb_coverage.py` measures the CONTROL tier and says so:
its `NOT_CONTROLS` set names `screen`, `application`, `runtime` and `nav`
precisely so they are excluded. `map_emission.py` checks the other direction —
whether facet DECLARES the runtime tier at all. Between them nothing ever asked
whether a BACKEND implements what facet declared, and the answer turned out to
be interesting: two of the five backends fill no window seam whatsoever, so
every `Window` cursor verb on them is inert, and nothing said so.

A runtime seam is a struct of function pointers facet keeps in a static and a
backend fills at install. `window::WindowHost` is the one that exists today.
Unlike the renderer, an unfilled slot is NOT a compile error — the struct is
zero-initialised, so a backend that never assigns a field ships a null and the
facade silently falls back or answers nothing.

WHAT COUNTS AS DECIDED. An empty slot is legitimate: facet_gtk leaves the four
window observers empty because GTK's `notify::is-active` and the GdkSurface
resize property are PROCESS-WIDE, not scoped to a window the caller names, and
filling a per-window slot with a process-wide observation is exactly the defect
the window cursor exists to remove. That reasoning lived in a source comment no
tool could read. It is a ```host-ledger fenced block in the backend's MANIFEST
now, so the gate can tell a decision from a gap — the same shape and the same
rule `verb_coverage.py` already uses for ```cannot-ledger.

  python3 tools/host_coverage.py            # the table
  python3 tools/host_coverage.py --list     # every slot, by backend
  python3 tools/host_coverage.py --check    # the gate

OPTIONAL SLOTS ARE READ OUT OF FACET, not listed here. A field whose doc
comment says a backend may leave it null — `request_close` is the only one
today — is not debt for anybody, and hardcoding that set here would go stale
the moment facet grew another.
"""
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# One entry per runtime seam: where facet declares it, and the function each
# backend fills it in. The installer name is what makes a backend that never
# installs distinguishable from one that installs an empty struct.
SEAMS = {
    "WindowHost": {
        "declared_in": "vendor/facet/src/window.cplus",
        "installer": "install_host",
    },
}

BACKENDS = {
    "appkit":  "vendor/facet_appkit",
    "uikit":   "vendor/facet_uikit",
    "gtk":     "vendor/facet_gtk",
    "android": "vendor/facet_android",
    "win32":   "vendor/facet_win32",
}


def read(path):
    try:
        with open(os.path.join(ROOT, path), "r", encoding="utf-8") as f:
            return f.read()
    except OSError:
        return ""


def declared_slots(path, name):
    """`(fields, optional)` — every `fn`-typed field of the struct, and the
    subset whose own doc comment says a backend may leave it null.

    Read from the comment rather than a hardcoded list: the exemption belongs
    to facet, which declares the contract, not to this tool.
    """
    src = read(path)
    m = re.search(r"^struct %s \{(.*?)^\}" % re.escape(name), src, re.S | re.M)
    if not m:
        sys.exit("host_coverage: could not find `struct %s` in %s" % (name, path))
    body = m.group(1)
    fields, optional, pending = [], set(), []
    for line in body.split("\n"):
        stripped = line.strip()
        if stripped.startswith("//"):
            pending.append(stripped)
            continue
        fm = re.match(r"([a-z_]+):\s*fn\b", stripped)
        if fm:
            f = fm.group(1)
            fields.append(f)
            # "leaves it null", "may be null", "leaves this null" — the phrasing
            # facet uses when a slot is genuinely optional.
            if any(re.search(r"null", c, re.I) for c in pending):
                optional.add(f)
        if stripped:
            pending = [] if not stripped.startswith("//") else pending
    return fields, optional


def filled_slots(pkg, installer):
    """Which fields the backend assigns, and whether it installs at all.

    The assignment is `h.<field> = ...` inside the function whose value is
    handed to the installer. Scoped to the package's `src/`, and the installer
    call is what separates "fills nothing" from "never installs".
    """
    src = ""
    d = os.path.join(ROOT, pkg, "src")
    if os.path.isdir(d):
        for name in sorted(os.listdir(d)):
            if name.endswith(".cplus"):
                src += read(os.path.join(pkg, "src", name)) + "\n"
    installs = re.search(r"\b%s\s*\(" % re.escape(installer), src) is not None
    return set(re.findall(r"^\s+[a-z]\.([a-z_]+)\s*=", src, re.M)), installs, src


def ledger(pkg, tag):
    """Rows of a ```<tag> fenced block in the backend's MANIFEST.

    Same shape `verb_coverage.py` reads its cannot-ledger from: one
    `slot — reason` per line, so a decision is machine-readable and a stale row
    is catchable.
    """
    src = read(os.path.join(pkg, "MANIFEST.md"))
    rows = {}
    for block in re.findall(r"```%s\n(.*?)```" % re.escape(tag), src, re.S):
        for line in block.split("\n"):
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            parts = re.split(r"\s+[-—]\s+", line, maxsplit=1)
            if len(parts) == 2:
                rows[parts[0].strip()] = parts[1].strip()
    return rows


def main():
    flags = sys.argv[1:]
    rc = 0
    table = []
    for seam, spec in SEAMS.items():
        fields, optional = declared_slots(spec["declared_in"], seam)
        required = [f for f in fields if f not in optional]
        print("%s — %d slots (%d required, %d optional by facet's own note)"
              % (seam, len(fields), len(required), len(optional)))
        if optional:
            print("  optional: %s" % ", ".join(sorted(optional)))
        print()
        for name, pkg in BACKENDS.items():
            filled, installs, _src = filled_slots(pkg, spec["installer"])
            decided = ledger(pkg, "host-ledger")
            if not installs:
                # Worse than an empty slot: nothing is installed, so EVERY verb
                # is inert and the facade cannot even fall back.
                print("  %-8s NOT INSTALLED — every %s verb is inert" % (name, seam))
                table.append((name, 0, len(required), len(required), 0))
                rc = 1
                continue
            have = [f for f in required if f in filled]
            missing = [f for f in required if f not in filled]
            argued = [f for f in missing if f in decided]
            debt = [f for f in missing if f not in decided]
            # A ledger row naming a slot the backend DOES fill is stale, and a
            # stale row reads as a commitment it is not.
            stale = [f for f in decided if f in filled or f not in fields]
            print("  %-8s %2d/%-2d filled   %2d decided   %2d debt%s"
                  % (name, len(have), len(required), len(argued), len(debt),
                     "   %d STALE" % len(stale) if stale else ""))
            if "--list" in flags:
                for f in missing:
                    why = decided.get(f)
                    print("      %-20s %s" % (f, why if why else "** not recorded **"))
                for f in stale:
                    print("      %-20s ** ledger row is stale **" % f)
            table.append((name, len(have), len(required), len(debt), len(stale)))
            if debt or stale:
                rc = 1
        print()

    if "--check" in flags:
        bad = [n for (n, _h, _r, d, s) in table if d or s]
        if bad:
            print("FAIL (%s): every runtime seam slot must be filled or recorded."
                  % ", ".join(sorted(set(bad))))
            return 1
        print("OK: every runtime seam slot is filled or recorded.")
        return 0
    return rc if "--check" in flags else 0


if __name__ == "__main__":
    sys.exit(main())
