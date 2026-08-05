#!/usr/bin/env python3
"""verb_coverage.py — which declared verbs a backend actually implements.

`views::has_body` answers at the KIND level: all 42 kinds have a body. That is
a true statement and it was reading as a stronger one than it is, because a
body can implement three of a control's twenty verbs and still be a body.

This answers at the VERB level. Every declared write and command becomes a
`P_*` bit in its module, and a backend implements one of three ways:

  LIVE         the apply body gates on `<mod>::P_X`, so a later write lands
  CREATE-ONLY  the props field is read, but not gated — so it is applied when
               the view is built and a later write never reaches the screen
  ABSENT       nothing in the backend mentions either

CREATE-ONLY is the interesting bucket and the reason this exists: it looks
implemented from the outside, works in the first frame, and silently does
nothing afterwards. Some are legitimately create-only (a `span`'s text is
rendered by its LABEL's dirty bit, not its own), which is why this reports
rather than fails — the judgement is a human's, the counting is not.

  python3 tools/verb_coverage.py            # the summary
  python3 tools/verb_coverage.py --list     # every verb, by bucket
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


def buckets(facet_src, backend_src):
    backend = "".join(open(b).read() for b in sorted(glob.glob(backend_src)))
    live, create_only, absent = [], [], []
    for f in sorted(glob.glob(facet_src)):
        mod = os.path.basename(f)[:-6]
        if mod in NOT_CONTROLS:
            continue
        src = open(f).read()
        for bit in re.findall(r"^const (P_[A-Z0-9_]+): u64", src, re.M):
            field = bit[2:].lower()
            entry = f"{mod}.{field}"
            if f"{mod}::{bit}" in backend:
                live.append(entry)
            elif re.search(r"\(\*p\)\.%s\b" % re.escape(field), backend):
                create_only.append(entry)
            else:
                absent.append(entry)
    return live, create_only, absent


def main():
    live, create_only, absent = buckets(
        os.path.join(ROOT, "vendor/facet/src/*.cplus"),
        os.path.join(ROOT, "vendor/facet_appkit/src/*.cplus"),
    )
    total = len(live) + len(create_only) + len(absent)
    print(f"facet_appkit verb coverage — {total} declared prop/command bits")
    print(f"  {len(live):>4}  live         gated on the dirty bit; a later write lands")
    print(f"  {len(create_only):>4}  create-only  applied when the view is built, never after")
    print(f"  {len(absent):>4}  absent       the backend mentions neither bit nor field")
    if "--list" in sys.argv:
        for name, rows in (("CREATE-ONLY", create_only), ("ABSENT", absent), ("LIVE", live)):
            print(f"\n{name} ({len(rows)})")
            for r in rows:
                print(f"  {r}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
