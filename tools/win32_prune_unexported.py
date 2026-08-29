#!/usr/bin/env python3
"""Drop bindings for Win32 symbols the SDK declares but no import library exports.

Called by `tools/gen_win32.sh` on the generated modules.

WHY THIS EXISTS. `cpc-bindgen` binds every function declaration it finds, which
is the right default — a header declares a function because you can call it. A
handful of Win32 entry points break that assumption: they are declared in the
public headers but exported by NO import library in the SDK, so binding them
turns the whole package into an unlinkable archive. The failure lands on the
CONSUMER, not on the package build, because a library archive resolves nothing:

    lld-link: error: undefined symbol: GetClipboardMetadata
    >>> referenced by libwin32.a(win32.o)

Each name below was checked against every `.Lib` in the SDK's `um/x64` before
being listed, not guessed from the docs. If a future SDK starts exporting one,
delete it here and the binding comes back on the next regeneration.

NOT THE SAME AS "wrong library". `wgl*`, `AlphaBlend`, `GradientFill`,
`TransparentBlt` and `DeviceCapabilities*` were also missing at first, but they
ARE exported — by opengl32, msimg32 and winspool. Those are fixed by naming the
library in `Cplus.toml`, not by pruning, and they are deliberately absent here.
"""

import re
import sys

# Declared in the SDK headers, exported by nothing in um/x64.
UNEXPORTED = {
    # Reserved/undocumented user32 entry points.
    "EnableMouseInPointerForThread",
    "GetClipboardMetadata",
    "GetDisplayAutoRotationPreferencesByProcessId",
    "SetThreadCursorCreationScaling",
    # The flat scroll bar API: `FlatSB_GetScrollProp` is exported, this
    # pointer-taking sibling never was.
    "FlatSB_GetScrollPropPtr",
}

LINK_NAME = re.compile(r'^#\[link_name = "([A-Za-z_0-9]+)"\]\s*$')


def prune(text: str) -> tuple[str, int]:
    """Remove the `#[link_name]` + extern + wrapper trio for each denied name."""
    lines = text.splitlines(keepends=True)
    out: list[str] = []
    removed = 0
    i = 0
    n = len(lines)
    while i < n:
        m = LINK_NAME.match(lines[i])
        if not m or m.group(1) not in UNEXPORTED:
            out.append(lines[i])
            i += 1
            continue

        name = m.group(1)
        j = i + 1
        # The `extern fn` declaration, which may wrap across lines and ends at
        # the first line whose content closes with `;`.
        while j < n and not lines[j].rstrip().endswith(";"):
            j += 1
        j += 1
        # The safe wrapper, if one follows: a `fn <name>(` block, matched by
        # brace depth so a wrapped signature or a multi-line body is handled.
        if j < n and lines[j].lstrip().startswith(f"fn {name}("):
            depth = 0
            opened = False
            while j < n:
                depth += lines[j].count("{") - lines[j].count("}")
                if "{" in lines[j]:
                    opened = True
                j += 1
                if opened and depth <= 0:
                    break
        out.append(
            f"// PRUNED `{name}`: declared by the SDK headers, exported by no "
            f"import library.\n//   See tools/win32_prune_unexported.py.\n"
        )
        removed += 1
        i = j
    return "".join(out), removed


def main() -> int:
    total = 0
    for path in sys.argv[1:]:
        with open(path, encoding="utf-8") as f:
            text = f.read()
        pruned, count = prune(text)
        if count:
            with open(path, "w", encoding="utf-8") as f:
                f.write(pruned)
            print(f"  pruned {count} unexported binding(s) from {path}")
        total += count
    if total == 0:
        print("  nothing pruned — every declared symbol has an export")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
