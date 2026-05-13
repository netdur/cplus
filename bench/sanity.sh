#!/usr/bin/env bash
# Extended sanity benchmark: cpc --release vs clang -O2.
# Reports build time, binary size, run time, peak RSS.
# Best-of-N to minimize noise (build: best of 3, run: best of 5).

set -euo pipefail
cd "$(dirname "$0")"

CPC=../target/release/cpc
if [[ ! -x "$CPC" ]]; then
    echo "Building cpc in release mode..."
    (cd .. && cargo build --release --bin cpc 2>&1 | tail -3)
fi

# best-of-N for wall-clock seconds (float)
time_min() {
    local n=$1; shift
    local best=99999 t
    for ((i=0; i<n; i++)); do
        t=$( { TIMEFORMAT='%R'; time "$@" >/dev/null 2>&1; } 2>&1 )
        if awk -v a="$t" -v b="$best" 'BEGIN { exit !(a+0 < b+0) }'; then best=$t; fi
    done
    printf "%s" "$best"
}

# best-of-N for peak RSS in bytes (integer)
rss_min() {
    local n=$1; shift
    local best=999999999 r
    for ((i=0; i<n; i++)); do
        /usr/bin/time -l "$@" >/dev/null 2>/tmp/_cpc_time_out
        r=$(awk '/maximum resident set size/ { print $1; exit }' /tmp/_cpc_time_out)
        if [ "$r" -lt "$best" ]; then best=$r; fi
    done
    printf "%s" "$best"
}

human_bytes() {
    awk -v b="$1" 'BEGIN {
        if (b < 1024)        printf "%d B", b;
        else if (b < 1048576) printf "%.1f KB", b/1024;
        else                  printf "%.2f MB", b/1048576;
    }'
}

bench() {
    local name=$1 src_cplus=$2 src_c=$3
    echo "=== $name ==="

    local out_cplus="out_${name}_cplus"
    local out_c="out_${name}_c"

    # one-shot build for output check + size
    "$CPC" --release "$src_cplus" -o "$out_cplus" >/dev/null
    clang -O2 "$src_c" -o "$out_c"
    local o_cplus o_c
    o_cplus=$("./$out_cplus")
    o_c=$("./$out_c")
    if [[ "$o_cplus" != "$o_c" ]]; then
        echo "  !! MISMATCH cplus=$o_cplus  c=$o_c"
    fi

    # build time, best of 3
    local bt_cplus bt_c
    bt_cplus=$(time_min 3 "$CPC" --release "$src_cplus" -o "$out_cplus")
    bt_c=$(time_min 3 clang -O2 "$src_c" -o "$out_c")

    # binary size
    local sz_cplus sz_c
    sz_cplus=$(wc -c < "$out_cplus" | tr -d ' ')
    sz_c=$(wc -c < "$out_c" | tr -d ' ')

    # run time, best of 5
    local rt_cplus rt_c
    rt_cplus=$(time_min 5 "./$out_cplus")
    rt_c=$(time_min 5 "./$out_c")

    # peak RSS, best of 5
    local rss_cplus rss_c
    rss_cplus=$(rss_min 5 "./$out_cplus")
    rss_c=$(rss_min 5 "./$out_c")

    # ratios (cpc/c)
    local r_bt r_rt r_sz r_rss
    r_bt=$(awk -v a="$bt_cplus" -v b="$bt_c" 'BEGIN { printf "%.2f", a/b }')
    r_rt=$(awk -v a="$rt_cplus" -v b="$rt_c" 'BEGIN { printf "%.2f", a/b }')
    r_sz=$(awk -v a="$sz_cplus" -v b="$sz_c" 'BEGIN { printf "%.2f", a/b }')
    r_rss=$(awk -v a="$rss_cplus" -v b="$rss_c" 'BEGIN { printf "%.2f", a/b }')

    printf "  %-13s  build=%6ss  size=%10s  run=%6ss  rss=%10s\n" \
        "cpc --release" "$bt_cplus" "$(human_bytes "$sz_cplus")" "$rt_cplus" "$(human_bytes "$rss_cplus")"
    printf "  %-13s  build=%6ss  size=%10s  run=%6ss  rss=%10s\n" \
        "clang -O2"     "$bt_c"     "$(human_bytes "$sz_c")"     "$rt_c"     "$(human_bytes "$rss_c")"
    printf "  %-13s        %5sx               %4sx        %5sx          %4sx\n" \
        "ratio (cpc/c)" "$r_bt" "$r_sz" "$r_rt" "$r_rss"
    echo
}

bench "sum" sum.cplus sum.c
bench "fib" fib.cplus fib.c
bench "arr" arr.cplus arr.c

rm -f out_sum_cplus out_sum_c out_fib_cplus out_fib_c out_arr_cplus out_arr_c
