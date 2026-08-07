#!/usr/bin/env python3
"""agent_* adapter parity — the access-and-control layer has ONE surface.

`agent_core::backend::Backend` is the access and control layer. Everything else
is an adapter over it: `agent_mcp` adds a socket and JSON-RPC, `agent_inapp`
adds nothing at all. An adapter that answers a different set of verbs is not an
adapter, it is a second surface with its own idea of what an agent may do.

That drift is not hypothetical and it went both ways unnoticed:

    poll_event   MCP had it, agent_inapp did not — an embedded assistant simply
                 could not observe events, and nobody decided that.
    auth         agent_inapp had a channel check MCP puts at its socket edge,
                 which in-process asks permission from itself.

Neither was a decision. Both were drift, and drift is what a guard is for.

WHAT IS CHECKED

  1. every field of the `Backend` vtable is reached by every adapter, and
  2. every adapter answers the same set of VERBS.

(1) catches a backend that grew a capability one adapter forgot. (2) catches an
adapter growing a verb of its own, which is how one of them ends up able to do
something the other cannot.

Exit 1 on any difference. Run it from the repository root.
"""

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BACKEND = ROOT / "vendor/agent_core/src/backend.cplus"

# An adapter is a file plus how its verbs are spelled in it. MCP dispatches on
# a wire string; the in-process one declares methods, because it is typed.
ADAPTERS = {
    "agent_mcp": {
        "path": ROOT / "vendor/agent_mcp/src/agent_mcp.cplus",
        "verbs": lambda src: set(re.findall(r'method == "([a-z_]+)"', src)),
    },
    "agent_inapp": {
        "path": ROOT / "vendor/agent_inapp/src/agent_inapp.cplus",
        "verbs": lambda src: set(re.findall(r"^    fn ([a-z_]+)\(", src, re.M)),
    },
}


def vtable_fields():
    """The Backend struct's fn-pointer fields — the capabilities themselves."""
    src = BACKEND.read_text()
    m = re.search(r"^struct Backend \{(.*?)^\}", src, re.S | re.M)
    if not m:
        sys.exit("adapter_parity: could not find `struct Backend` in backend.cplus")
    return set(re.findall(r"^    ([a-z_]+): fn\(", m.group(1), re.M))


def main():
    problems = []

    fields = vtable_fields()
    if not fields:
        sys.exit("adapter_parity: `struct Backend` parsed to zero fields")

    # (1) coverage: every capability reached by every adapter.
    for name, spec in ADAPTERS.items():
        src = spec["path"].read_text()
        for f in sorted(fields):
            if not re.search(rf"\bvt\.{re.escape(f)}\b", src):
                problems.append(
                    f"{name} never reaches `vt.{f}` — the backend offers it and "
                    f"this adapter cannot"
                )

    # (2) the verb sets agree.
    verbs = {n: s["verbs"](s["path"].read_text()) for n, s in ADAPTERS.items()}
    names = sorted(verbs)
    base = verbs[names[0]]
    for other in names[1:]:
        for missing in sorted(base - verbs[other]):
            problems.append(f"{other} has no `{missing}`, and {names[0]} does")
        for extra in sorted(verbs[other] - base):
            problems.append(f"{other} answers `{extra}`, and {names[0]} does not")

    if problems:
        print(f"adapter parity: {len(problems)} problem(s)\n", file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        print(
            "\nAn adapter is a transport over the backend, never a second "
            "opinion about\nwhat an agent may do. Add the verb, or take it out.",
            file=sys.stderr,
        )
        return 1

    print(
        f"adapter parity: {len(fields)} backend capabilities, "
        f"{len(base)} verbs, {len(ADAPTERS)} adapters agree"
    )
    for v in sorted(base):
        print(f"  {v}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
