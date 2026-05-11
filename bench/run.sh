#!/usr/bin/env bash
# Sanity benchmark: C+ release vs clang -O2 on three micro-benchmarks.
# Reports the best of 5 runs (wall-clock seconds) to minimize timing noise.

set -euo pipefail

cd "$(dirname "$0")"

CPC=../target/release/cpc
if [[ ! -x "$CPC" ]]; then
    echo "Building cpc in release mode..."
    (cd .. && cargo build --release --bin cpc 2>&1 | tail -3)
fi

bench() {
    local name=$1
    local src_cplus=$2
    local src_c=$3

    echo "=== $name ==="

    # Compile both
    "$CPC" --release "$src_cplus" -o "out_${name}_cplus" >/dev/null
    clang -O2 "$src_c" -o "out_${name}_c"

    # Sanity-check outputs match
    local out_cplus out_c
    out_cplus=$("./out_${name}_cplus")
    out_c=$("./out_${name}_c")
    if [[ "$out_cplus" != "$out_c" ]]; then
        echo "  MISMATCH: cplus=$out_cplus  c=$out_c"
    else
        echo "  output (both):  $out_cplus"
    fi

    # Best-of-5 timing
    local best_cplus=99999 best_c=99999 t
    for i in 1 2 3 4 5; do
        t=$( { TIMEFORMAT='%R'; time "./out_${name}_cplus" >/dev/null; } 2>&1 )
        if awk -v a="$t" -v b="$best_cplus" 'BEGIN { exit !(a < b) }'; then best_cplus=$t; fi
        t=$( { TIMEFORMAT='%R'; time "./out_${name}_c" >/dev/null; } 2>&1 )
        if awk -v a="$t" -v b="$best_c" 'BEGIN { exit !(a < b) }'; then best_c=$t; fi
    done

    local ratio
    ratio=$(awk -v a="$best_cplus" -v b="$best_c" 'BEGIN { printf "%.2f", a/b }')
    printf "  cpc --release : %ss\n" "$best_cplus"
    printf "  clang -O2     : %ss\n" "$best_c"
    printf "  ratio (cpc/c) : %sx\n" "$ratio"
    echo
}

bench "sum" sum.cplus sum.c
bench "fib" fib.cplus fib.c
bench "arr" arr.cplus arr.c

# Cleanup
rm -f out_sum_cplus out_sum_c out_fib_cplus out_fib_c out_arr_cplus out_arr_c
