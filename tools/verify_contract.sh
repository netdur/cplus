#!/bin/sh
# The provenance proof for the facet package, runnable any time.
# 1. Generated files are PURE generator output: regenerating them changes
#    nothing. A hand edit to a generated file fails here.
# 2. No marker regions exist anywhere (the v1 disease cannot return).
# 3. The dead v1 band stayed dead: its slot code does not exist (a comment
#    naming the death is allowed; code is not).
# Hand-file provenance is git's: `git diff pre-maui-regen HEAD -- <file>`
# must show only region deletions and named-commit additions.
set -e
cd "$(dirname "$0")/.."
python3 tools/maui_map.py >/dev/null
python3 tools/gen_contract.py >/dev/null
if ! git diff --quiet vendor/facet/src/contract.cplus \
    vendor/facet_appkit/src/contract_appkit.cplus vendor/facet/docs/contract.md; then
  echo "FAIL: generated files differ from generator output (hand edit?)" >&2
  git checkout -- vendor/facet/src/contract.cplus vendor/facet_appkit/src/contract_appkit.cplus vendor/facet/docs/contract.md
  exit 1
fi
if grep -rn 'GEN:' vendor/facet/src/*.cplus vendor/facet_appkit/src/*.cplus >/dev/null 2>&1; then
  echo "FAIL: marker regions found" >&2
  exit 1
fi
if grep -rn 'fn set_anchor_x\|fn set_width_request\|fn set_is_visible(\|SET_CHARACTER_SPACING_FN' \
    vendor/facet/src vendor/facet_appkit/src >/dev/null 2>&1; then
  echo "FAIL: dead v1 band code found" >&2
  exit 1
fi
echo "contract provenance clean"
