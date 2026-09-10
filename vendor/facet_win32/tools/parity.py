#!/usr/bin/env python3
"""Prop, handler and shared-band coverage for facet_win32.

    python3 vendor/facet_win32/tools/parity.py           # the report
    python3 vendor/facet_win32/tools/parity.py --check   # non-zero if win32 regressed

A SHIM, NOT A FOURTH COPY. The measurement lives once, in
`vendor/facet_gtk/tools/parity.py`, which already knows every backend —
`BACKENDS` names all five and `FOCUS` selects the column. What did not exist was
a way to REACH it as this package: the tool is found beside the package it
reports on, so anyone standing in facet_win32 had no parity script and the
contract gap here went unmeasured for the whole port.

Copying it would have been the obvious move and is the wrong one. There are
already three copies in the tree — gtk 25K, android 21K, uikit 5K — and they
have drifted: the uikit one predates the per-struct handler attribution that
found four dead handlers behind a 68/68, and the android one predates the fenced
-code-block fix that was FOUND ON THIS PACKAGE, where four argued rows read as
UNRECORDED because a ``` fence desynchronised the backtick parse. A fourth copy
inherits whichever bugs its parent had on the day it was taken, and the report
that steers this package is the last one that should be a stale fork.

So this file imports that one and asks for the win32 column. The floors live in
its `FLOORS` table beside GTK's, for the same reason: one place to raise them.
"""

import os
import sys
import importlib.util

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))
IMPL = os.path.join(ROOT, "vendor", "facet_gtk", "tools", "parity.py")


def main():
    if not os.path.isfile(IMPL):
        print(f"parity: the measurement lives in {IMPL} and is not there",
              file=sys.stderr)
        return 2
    spec = importlib.util.spec_from_file_location("facet_parity", IMPL)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    # `main()` reads argv for the backend name, so ASK AS win32 unless the caller
    # already named a column — `parity.py appkit` from here still works, and
    # comparing against another backend is half of what the report is for.
    if not any(a in mod.BACKENDS for a in sys.argv[1:]):
        sys.argv.append("win32")
    return mod.main()


if __name__ == "__main__":
    sys.exit(main())
