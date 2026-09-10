#!/usr/bin/env python3
"""objc_ownership.py — a wrapper releases itself; releasing it again is a bug.

In the generated Apple bindings a wrapper struct with a `drop` is an OWNING
handle: `::new` is +1, `from_raw` RETAINS, `drop` RELEASES. So

    rt::release(x.raw())        # x is such a wrapper

is the SECOND release. The object goes to zero while its owner still has it,
and the next release lands on freed memory.

SEVEN of those had accumulated in `vendor/camera/src/camera_backend.cplus` and
one of them killed the process on a lens flip — see
`bugs/closed/camera-flipping-the-lens-over-releases-the-video-output.md`. The
shape is why they survived: five were on error paths nobody takes, and the two
on the main path only bite when a session is CLOSED AND RE-OPENED, because a
single close at app exit over-releases into a process that is exiting anyway.

Nothing in the compiler can see this — `.raw()` is an honest accessor and
`rt::release` is an honest call — so it is checked here, the way
`vendor/facet_uikit/tools/msgsend_arity.py` checks selector arity.

WHAT IS NOT FLAGGED, deliberately: `rt::release(session)` and friends where the
operand is a raw `*u8` with no wrapper behind it. Those are correct and common,
and they sit line-for-line beside the bugs. The tell is `.raw()` on a local
whose declared type has a releasing `drop`.

    python3 tools/objc_ownership.py            # the check
    python3 tools/objc_ownership.py --list     # every owning wrapper it knows

Exits non-zero on a finding.
"""

import glob
import os
import re
import sys

FN = re.compile(r"^(?:export )?(?:extern )?fn \w+", re.M)
IMPL = re.compile(r"impl (\w+) \{")
# `let out: av::CaptureVideoDataOutput = ...`  ->  (out, CaptureVideoDataOutput)
LOCAL = re.compile(r"\blet\s+(\w+)\s*:\s*(?:\w+::)?(\w+)\s*=")
VAR = re.compile(r"\bvar\s+(\w+)\s*:\s*(?:\w+::)?(\w+)\s*=")


def owning_wrappers(files):
    """Types whose `drop` releases — the ones an extra release breaks."""
    out = set()
    for f in files:
        src = open(f, errors="replace").read()
        for m in IMPL.finditer(src):
            j = m.end()
            nxt = src.find("\nimpl ", j)
            body = src[j : nxt if nxt > 0 else len(src)]
            if "fn drop(ref this)" not in body:
                continue
            # The drop has to actually RELEASE. A `drop` that frees a Vec is
            # not this, and flagging it would be noise.
            if "rt::release" in body or "objc_release" in body:
                out.add(m.group(1))
    return out


def findings(files, owning):
    bad = []
    for f in files:
        src = open(f, errors="replace").read()
        starts = [m.start() for m in FN.finditer(src)] + [len(src)]
        for i in range(len(starts) - 1):
            body = src[starts[i] : starts[i + 1]]
            types = {}
            for rx in (LOCAL, VAR):
                for m in rx.finditer(body):
                    types[m.group(1)] = m.group(2)
            # direct:  rt::release(x.raw())
            for m in re.finditer(r"rt::release\(\s*(\w+)\.raw\(\)\s*\)", body):
                name = m.group(1)
                ty = types.get(name)
                if ty in owning:
                    line = src[: starts[i] + m.start()].count("\n") + 1
                    bad.append((f, line, name, ty, "rt::release(%s.raw())" % name))
            # stashed: let p: *u8 = x.raw(); ... rt::release(p)
            for m in re.finditer(r"let\s+(\w+)\s*:\s*\*u8\s*=\s*\{?\s*(\w+)\.raw\(\)", body):
                alias, name = m.group(1), m.group(2)
                ty = types.get(name)
                if ty in owning and re.search(r"rt::release\(\s*%s\s*\)" % alias, body):
                    line = src[: starts[i] + m.start()].count("\n") + 1
                    bad.append((f, line, name, ty, "let %s = %s.raw(); rt::release(%s)" % (alias, name, alias)))
    return bad


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    os.chdir(root)
    files = sorted(glob.glob("vendor/*/src/*.cplus"))
    owning = owning_wrappers(files)

    if "--list" in sys.argv:
        for t in sorted(owning):
            print(t)
        print(f"\n{len(owning)} owning wrappers across {len(files)} files")
        return 0

    bad = findings(files, owning)
    for f, line, name, ty, what in bad:
        print(f"{f}:{line}  {what}")
        print(f"    `{name}` is an `{ty}`, whose `drop` already releases — this is the second one")
    if bad:
        print(f"\n{len(bad)} double release(s). Delete the explicit release; the wrapper owns it.")
        return 1
    print(f"{len(files)} files, {len(owning)} owning wrappers, no double releases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
