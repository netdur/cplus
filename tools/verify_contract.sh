#!/bin/sh
# The provenance proof for the facet package, runnable any time (clean or
# dirty tree). Invariant: the working-tree generated files are EXACTLY what
# the generator emits — a hand edit to a generated file is detected (and
# overwritten, with a failure) here. Also: no marker regions anywhere, and
# the dead v1 band stays dead.
set -e
cd "$(dirname "$0")/.."
G1=vendor/facet/src/contract.cplus
G2=vendor/facet_appkit/src/contract_appkit.cplus
G3=vendor/facet/docs/contract.md
TMP=$(mktemp -d)
cp "$G1" "$TMP/1"; cp "$G2" "$TMP/2"; cp "$G3" "$TMP/3"
python3 tools/maui_map.py >/dev/null
python3 tools/gen_contract.py >/dev/null
fail=0
cmp -s "$G1" "$TMP/1" || { echo "FAIL: $G1 had drifted from generator output (now regenerated)" >&2; fail=1; }
cmp -s "$G2" "$TMP/2" || { echo "FAIL: $G2 had drifted from generator output (now regenerated)" >&2; fail=1; }
cmp -s "$G3" "$TMP/3" || { echo "FAIL: $G3 had drifted from generator output (now regenerated)" >&2; fail=1; }
rm -rf "$TMP"
if grep -rn 'GEN:' vendor/facet/src/*.cplus vendor/facet_appkit/src/*.cplus >/dev/null 2>&1; then
  echo "FAIL: marker regions found" >&2; fail=1
fi
if grep -rn 'fn set_anchor_x\|fn set_width_request\|fn set_is_visible(\|SET_CHARACTER_SPACING_FN\|fn set_thumb_color(' \
    vendor/facet/src vendor/facet_appkit/src >/dev/null 2>&1; then
  echo "FAIL: dead band code found" >&2; fail=1
fi
[ "$fail" = 0 ] && echo "contract provenance clean"
exit $fail
