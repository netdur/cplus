/*
 * c_user.c — a C program that calls every export from mathlib.
 *
 * Each block exercises a different ABI class. Run after `make` (or the
 * commands documented in the Makefile) and check stdout matches the
 * "expected" comments. The Makefile's `make check` target runs the
 * binary and asserts the exit code.
 */

#include <stdio.h>
#include <stdint.h>
#include "../mathlib/target/release/mathlib.h"

/* Callback used by `apply` — proves fn-pointer params work both ways. */
static int32_t double_it(int32_t x) { return x * 2; }

int main(void) {
    int failures = 0;

    /* ---- Scalar args / scalar return ---- */
    int32_t s = add(20, 22);
    printf("add(20, 22) = %d (expected 42)\n", s);
    if (s != 42) failures++;

    int32_t n = negate(7);
    printf("negate(7) = %d (expected -7)\n", n);
    if (n != -7) failures++;

    /* ---- 8-byte struct (Point) ---- *
     * Before Slice 5.D this returned garbage; now it round-trips. */
    Point p = make_point(3, 4);
    printf("make_point(3, 4) = {%d, %d}\n", p.x, p.y);
    if (p.x != 3 || p.y != 4) failures++;
    int32_t sq = square(p);
    printf("square({3, 4}) = %d (expected 25)\n", sq);
    if (sq != 25) failures++;

    /* ---- 16-byte struct (Pair) — 2 registers ---- */
    Pair pr = make_pair(10, 20);
    int64_t pr_sum = sum_pair(pr);
    printf("sum_pair({10, 20}) = %lld (expected 30)\n", (long long)pr_sum);
    if (pr_sum != 30) failures++;

    /* ---- >16-byte struct (Triple) — indirect + sret ---- */
    Triple t = make_triple(100, 200, 300);
    printf("make_triple = {%lld, %lld, %lld}\n",
        (long long)t.a, (long long)t.b, (long long)t.c);
    if (t.a != 100 || t.b != 200 || t.c != 300) failures++;
    int64_t t_sum = sum_triple(t);
    printf("sum_triple = %lld (expected 600)\n", (long long)t_sum);
    if (t_sum != 600) failures++;

    /* ---- Plain enum ---- */
    int32_t ci = color_index(Color_Green);
    printf("color_index(Green) = %d (expected 1)\n", ci);
    if (ci != 1) failures++;

    /* ---- Raw pointer out-param ---- */
    int32_t slot = 0;
    fill_with(&slot, 99);
    printf("fill_with &slot, 99 -> slot = %d (expected 99)\n", slot);
    if (slot != 99) failures++;

    /* ---- Fn-pointer arg ---- */
    int32_t y = apply(double_it, 21);
    printf("apply(double_it, 21) = %d (expected 42)\n", y);
    if (y != 42) failures++;

    /* ---- Internal helper used via public wrapper ---- */
    int32_t h = use_helper(10);
    printf("use_helper(10) = %d (expected 11)\n", h);
    if (h != 11) failures++;

    printf("\n%d failure(s)\n", failures);
    return failures;
}
