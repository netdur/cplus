#!/usr/bin/env bash
# Regenerate the raw Win32 FFI modules in `vendor/win32/src/` from the Windows
# SDK headers, via `cpc-bindgen --cpackage`.
#
# Run from the repo root, on Windows, with clang on PATH:
#
#     bash tools/gen_win32.sh
#
# ---- WHAT IT WRITES, AND WHAT IT LEAVES ALONE -------------------------------
#
# WRITES (generated; do not hand-edit — the next run overwrites them):
#     vendor/win32/src/winuser.cplus       user32   — windows, messages, input
#     vendor/win32/src/wingdi.cplus        gdi32    — drawing, fonts, bitmaps
#     vendor/win32/src/commctrl.cplus      comctl32 — the common controls
#     vendor/win32/src/libloaderapi.cplus  kernel32 — GetModuleHandle, LoadLibrary
#
# `Cplus.toml` names more libraries than the four the GUI needs, because the
# headers bind more than the GUI: `wingdi.h` declares the whole `wgl*` family
# (opengl32), the alpha-blend trio (msimg32) and `DeviceCapabilities` (winspool).
# Those are real exports in the wrong library, not missing ones — see the prune
# script for the difference.
#
# LEAVES ALONE (hand-written, and the reason the package is usable):
#     core / window / controls / menu / dialogs / graphics — the curated facade
#     win32.cplus       — the umbrella; it imports the generated modules so a
#                         broken regeneration fails the build, and so their
#                         `#[test]`s are discoverable
#     Cplus.toml        — carries prose and a link list `--cpackage` would flatten
#
# Those hand-written modules ARE this package's `appkit_ext.cplus`: anything
# added by hand goes in one of them, never in a generated file.
#
# ---- WHY THE SHIM HEADERS -----------------------------------------------------
#
# bindgen keeps declarations whose source file BASENAME matches the header it
# was given, and no Win32 header compiles standalone — `winuser.h` alone is a
# wall of errors, because `windows.h` must come first. So each target gets a
# one-line shim OF THE SAME NAME that includes `windows.h`: the shim compiles,
# and the real header's declarations match the basename filter and are kept.
#
# ---- WHY `commdlg` IS NOT IN THE LIST -----------------------------------------
#
# The same filter misses it. `windows.h` pulls `commdlg.h` in through a chain
# that leaves clang reporting `GetOpenFileNameA`'s `loc.file` as `ole2.h`, so a
# `commdlg.h` shim generates an empty module. That surface is small and the
# hand-written `dialogs.cplus` already covers it, so it stays hand-written
# rather than the filter being fought.
#
# ---- WHY wingdi GETS A SUPPLEMENT ---------------------------------------------
#
# bindgen will not emit a record inside a `#pragma pack(N)` region, because
# clang's JSON carries the pack ATTRIBUTE with no VALUE and guessing offsets in
# an FFI struct corrupts silently. Five gdi records are affected and are named
# by generated code, so they cannot live in a sibling module (C+ modules are
# namespaced). `tools/win32_packed_structs.cplus` holds them, verified against
# the SDK with `offsetof`, and is appended here.

set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
bindgen="$repo/target/debug/cpc-bindgen.exe"
[ -x "$bindgen" ] || bindgen="$repo/target/release/cpc-bindgen.exe"
if [ ! -x "$bindgen" ]; then
    echo "gen_win32: build it first — cargo build -p cpc-bindgen" >&2
    exit 1
fi

shims="$(mktemp -d)"
staging="$(mktemp -d)"
trap 'rm -rf "$shims" "$staging"' EXIT

for h in winuser wingdi commctrl libloaderapi; do
    printf '#include <windows.h>\n' > "$shims/$h.h"
done
# The common controls are not in `windows.h`; they need their own include, and
# the angle brackets keep it resolving to the SDK rather than back to this shim.
printf '#include <windows.h>\n#include <commctrl.h>\n' > "$shims/commctrl.h"

"$bindgen" --cpackage --out "$staging" \
    --header "$shims/winuser.h" \
    --header "$shims/wingdi.h" \
    --header "$shims/commctrl.h" \
    --header "$shims/libloaderapi.h" \
    --clib user32 --clib gdi32 --clib comctl32 --clib comdlg32

dest="$repo/vendor/win32/src"
for m in winuser wingdi commctrl libloaderapi; do
    if [ ! -s "$staging/src/$m.cplus" ]; then
        echo "gen_win32: $m generated empty — check the shim and the basename filter" >&2
        exit 1
    fi
    cp "$staging/src/$m.cplus" "$dest/$m.cplus"
done

# A few entry points are declared by the SDK headers and exported by NO import
# library; binding them makes the archive unlinkable for every consumer. Drop
# them (the list, and the check behind it, are in the script).
python tools/win32_prune_unexported.py     "$dest/winuser.cplus" "$dest/wingdi.cplus" "$dest/commctrl.cplus" "$dest/libloaderapi.cplus"

# The five pack-region records, appended into the module that names them.
{
    printf '\n'
    cat "$repo/tools/win32_packed_structs.cplus"
} >> "$dest/wingdi.cplus"

echo "gen_win32: wrote winuser/wingdi/commctrl/libloaderapi into $dest"
echo "gen_win32: hand-written modules and Cplus.toml untouched"
