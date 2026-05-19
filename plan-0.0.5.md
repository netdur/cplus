# C+ — Plan

Version 0.0.4 shipped 2026-05-18 (Phase 4 MVP). See [plan-0.0.4.md](plan-0.0.4.md) for the archived 0.0.4 roadmap and resolved log; [plan-0.0.3.md](plan-0.0.3.md) covers v0.0.3, [plan-0.0.2.md](plan-0.0.2.md) v0.0.2, and [plan-0.0.1.md](plan-0.0.1.md) v0.0.1.

---

## v0.0.5 — Correctness, then ergonomics

**Strategy: correctness gaps first, then the small compiler unblockers, then features built on top.** The two double-free Drop bugs and the missing inner-T Drop on container drop are real footguns with workarounds; they're not "wait for a workload" issues. After they land, two small compiler slices (generic-method-body propagation + gen-methods) unlock the iterator ecosystem and the async method APIs. Async polish and platform parity round out the release; perf + polish close it.

Slice sizes use assistant-paced framing (S/M/L), not human-typing weeks. A "session" means one focused implementation pass with verification; a phase is "ship the phase when its exit criteria are green," not "schedule N weeks."

---

### Phase 1 — Drop correctness · size M

The Drop-machinery gaps that survived v0.0.3 and v0.0.4. Each is a real correctness hole with a known workaround; the workarounds shouldn't have to exist.

#### Slice 1A — Auto-promote non-Copy value param to `move` (or reject without it) · ✅ partial shipped 2026-05-19 (string narrow path; generic containers carry forward)

**Bug** (now closed for `Ty::String`): `fn echo(x: string) -> string { return x; }` used to double-free at runtime. The caller's `x` flowed in as a value-passed `{ptr, len, cap}` aggregate (heap pointer shared with the caller); `return x` lifted that pointer into the caller's result `t`; at scope exit, both `s` and `t` Dropped the same heap → SIGTRAP (exit 133 on darwin).

**Path 3 shipped (narrow — Ty::String only).** Took the codegen-side auto-clone route, scoped to `Ty::String` returns from non-`move` parameters:

- New `borrowed_params: HashSet<String>` on `FnState` ([cplus-core/src/codegen.rs](cplus-core/src/codegen.rs)) tracking parameter names whose binding shares heap with the caller — populated in both `gen_function` and `gen_method` for (a) pointer-passed non-Copy structs and (b) value-passed non-`move` `Ty::String` params.
- New `clone_string_aggregate` helper that extracts `ptr`/`len` from the source, mallocs + memcpies, returns a fresh `{new_ptr, len, len}` aggregate.
- In `StmtKind::Return`, when the returned expression is a bare `Ident` pointing at a `borrowed_params` entry AND return type is `Ty::String`, substitute the cloned aggregate for the raw return value before `ret`/`store-to-sret`.

**Why narrow.** `Vec[T]` / `HashMap[K,V]` / other generic heap-owning containers still need explicit `move` because element-level clone requires `T::clone` glue, which the language doesn't have yet (the inner-T drop intrinsic from Slice 1C is the closest precedent). The broader Path-3 design (generic Clone in body context) lands when that infrastructure does.

**Earlier reverted attempts (kept for context):**

- **Auto-promote** (treat non-`move` non-Copy params as if `move`): closes the `echo` bug cleanly but breaks every read-only API that takes a non-Copy value (`cow::is_owned(c: CowStr)`, `cow::len(c: CowStr)`, lots of stdlib helpers). After the first call, the caller's binding is consumed → second use is E0335.

- **Narrow E1100** (error only when a single-param function body directly returns its non-Copy parameter): catches the headline `echo` case without disturbing readers. But layering conflicts with the existing borrow checker — E0372 ("move-while-return-borrow-live") and E0384 ("ambiguous root in mixed-rooting return") fire on overlapping shapes, and since sema errors abort the pipeline before borrowck runs, E1100 masks both.

**Tests:** new e2e `echo_string_param_does_not_double_free` exercises the canonical shape. 869 lib + 351 e2e green at landing.

**Carryover:** auto-clone for `Vec[T]` / `HashMap[K,V]` / other Drop containers. Needs a `T::clone` intrinsic mirroring `__cplus_drop_in_place::[T]` from Slice 1C.

#### Slice 1B — Re-bind Drop tracking · ✅ shipped 2026-05-18

**Half already worked, half didn't.** The plain top-level form `let a: string = "x".to_string(); let b: string = a;` was already handled by Phase 3's `scan_moves_in_stmt` — when a Let's init is `Ident(n)`, n was pre-registered in `moved_bindings`, getting Runtime drop disposition, and codegen flipped the flag. No double-free in this shape.

**Real bug:** the block-tail-as-RHS form `let f: string = { let inner: string = ...; inner };`. The inner block's tail expression `inner` evaluated to a struct value, the inner scope's `pop_scope` then dropped `inner` (freeing the heap buffer), and the resulting dangling-pointer struct stored into `f`. At main's scope exit, `f` dropped → double-free on the same buffer.

**Fix:** two-side coordination.

- [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs) `scan_moves_in_block`: when a block's tail expr is `Ident(n)`, pre-insert n into `moved_bindings`. Promotes n's drop_flag to Runtime disposition.
- [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs) `gen_block_expr`: after evaluating the tail expr, if it's a non-Copy `Ident(name)`, call `mark_moved(name)` before `pop_scope` fires the scope-exit drop. The flag now flips to false; the drop is disarmed; the value flows cleanly to the caller's slot.

Recursive: nested `{ { ... inner }; outer }` works because each block-tail Ident gets pre-registered + disarmed independently.

**Tests:** new e2e `block_tail_ident_non_copy_does_not_double_free` exercises both single-level and nested block-tail rebinds. 335 e2e + 866 lib + 11 LSP, all green.

#### Slice 1C — Inner-T Drop on container Drop · ✅ shipped 2026-05-18

**Bug (closed):** `Box[T]`, `Arc[T]`, `Rc[T]`, `Mutex[T]`, `Channel[T]`, `Vec[T]`, and `HashMap[K, V]` all freed their heap storage on Drop but **didn't call `T::drop()`** on the contained value(s). With `T = string` or `T = Vec[U]`, every container leaked per-instance.

**Fix shipped in two pieces:**

1. **New compiler intrinsic `__cplus_drop_in_place::[T](p: *T)`** ([cplus-core/src/sema.rs](cplus-core/src/sema.rs), [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs)). Sema accepts a 1-type-arg, 1-raw-ptr-arg call shape (must be inside `unsafe`). Codegen's `gen_drop_in_place` lowers to:
   - `Ty::String` → inline free of the `{ptr, len, cap}` aggregate's `ptr` field.
   - `Ty::Struct(id)` where the struct has an explicit `fn drop` method → `call preserve_nonecc void @<mangled-name>.drop(ptr p)`.
   - Anything else → no-op (Copy types and structs without Drop don't need teardown).

2. **Stdlib container `drop` methods walk live storage and call the intrinsic** ([vendor/stdlib/src/box.cplus](vendor/stdlib/src/box.cplus), [vec.cplus](vendor/stdlib/src/vec.cplus), [arc.cplus](vendor/stdlib/src/arc.cplus), [rc.cplus](vendor/stdlib/src/rc.cplus), [mutex.cplus](vendor/stdlib/src/mutex.cplus), [channel.cplus](vendor/stdlib/src/channel.cplus), [hash_map.cplus](vendor/stdlib/src/hash_map.cplus)):
   - **Box** drops the inner T before freeing the heap slot; `unwrap` null's `self.p` after taking ownership so the scope-exit Drop short-circuits and doesn't double-free.
   - **Vec** walks `[0, len)` invoking inner Drop per element.
   - **Arc**, **Rc** drop inner T on the last reference, before freeing the control block.
   - **Mutex** drops inner T on the last reference, after destroying the pthread mutex.
   - **Channel** drops every buffered element in `[head, tail)` before destroying the pthread primitives + freeing the element buffer.
   - **HashMap** walks every occupied slot, dropping both K and V before freeing the parallel storage arrays.

**Bug surfaced during implementation:** `Box::unwrap(move self) -> T` had relied on the v0.0.4 "drop frees only the outer slot" semantic — moving the inner T to the caller while the outer Drop ran was safe. With v0.0.5's inner-T Drop, the same path would double-free (callee's Drop fires inner Drop, caller's binding holds the same value). Fix: `unwrap` reads the inner value, nulls `self.p`, frees the outer slot manually, returns. The scope-exit Drop then sees `self.p == null` and short-circuits.

**Tests:** new e2e `phase1c_container_inner_drop_runs_without_crash` exercises Box[string], Vec[string], Arc[string], Rc[string], HashMap[str, i32] teardown paths. 336 e2e + 866 lib + 11 LSP, all green.

**Forward-pointers:**
- Tagged-enum payloads with Drop are still rejected at the type-collection pass (E0344) per the v0.0.3 design rule. When that rule relaxes, `gen_drop_in_place`'s `Ty::Enum` branch will need to walk variants and drop their payloads — the intrinsic surface stays the same.
- Other containers with consumer methods like `Box::unwrap` (e.g. an eventual `Vec::pop`, `Arc::unwrap`, `Mutex::into_inner`) need the same null-then-drop coordination pattern. Add as their slices land.

#### Slice 1D — ASan + async coroutine interaction · ✅ closed 2026-05-18 (was already fixed)

**Bug (was open in plan-0.0.4):** scalar `i32` async fns under `--asan` returned 0 instead of the expected value. Probably ASan instrumentation of the alloca-promise + CoroSplit interaction.

**Investigation outcome:** the bug was **incidentally cured by Phase 1E's promise-alloca fix** (passing `alloca <T>` to `coro.id` instead of `ptr null` so CoroSplit hoists the slot into the frame at a known offset). The original v0.0.3 form — `ptr null` + later `coro.promise()` write — was unsound under ASan because the write went into uninitialized frame slack that ASan flagged; once Phase 1E gave the promise a real alloca tied to the coroutine frame, the writes landed in a properly-tracked region. The Phase-1E note carried the caveat forward as a known-unverified limitation; this slice confirms the fix by adding an explicit ASan regression test.

**Test:** new e2e `phase1d_async_runs_clean_under_asan` builds with `--asan` and exercises scalar primitive returns (i32, i64, bool via generic instantiation), plus chained awaits between two coroutine frames. All return their declared values. 337 e2e tests, all green.

#### Phase 1 exit criteria

- [ ] `fn echo(x: string) -> string { return x; }` doesn't double-free *(deferred — Slice 1A still needs design, three paths documented)*
- [x] `let b: string = a;` doesn't double-free at scope exit *(Slice 1B — closed for plain re-bind and block-tail-rebind shapes)*
- [x] `HashMap[str, string]` doesn't leak entries on drop *(Slice 1C — inner-T Drop on every container)*
- [x] `async fn id(x: i32) -> i32 { return x; }` driven by `block_on` returns x under ASan *(Slice 1D — verified by `phase1d_async_runs_clean_under_asan`)*

---

### Phase 2 — Compiler unblockers · size S

Three small slices that together unlock the v0.0.5 feature work. None ships a user-visible artifact directly; each closes a pre-existing limitation that was worked around in v0.0.4.

#### Slice 2A — Generic-method-body propagation · ✅ shipped 2026-05-18

**Limitation (closed):** Phase 1B's `propagate_fn_instantiations` walked generic-FREE-fn bodies but skipped impl-method bodies. So `HashMap[K, V]::get` calling `result::io_err::[V](...)` never got the propagated `(io_err, [i32])` entry → codegen panicked looking up `io_err` (un-mangled). v0.0.4's HashMap worked around it by constructing `Result::Err(...)` directly.

**Fix shipped** in [cplus-core/src/monomorphize.rs](cplus-core/src/monomorphize.rs):
- `propagate_fn_instantiations` now takes `struct_instantiations` and, after the main worklist drains, walks every generic-impl-block's methods for each instantiation. For `impl Vec[T]`'s instantiation with `T = i32`, builds the subst `{T → i32}`, walks each method body's turbofish call sites via `visit_ident_calls_in_block`, substitutes, and records the resolved `(callee, concrete_args)` pair.
- Method-discovered pairs feed a secondary worklist that re-runs the existing free-fn propagation (so chained transitive calls — generic-method → generic-free-fn → another generic-free-fn — also resolve).

**Stdlib follow-through:** [vendor/stdlib/src/hash_map.cplus](vendor/stdlib/src/hash_map.cplus) reverts its v0.0.4 workaround — `return result::io_err::[V](...)` / `return result::io_ok::[V](v)` now work directly inside `HashMap[K, V]::get`. Cleaner stdlib code; the principled fix replaces the inlining hack.

**Tests:** existing `stdlib_hash_map_generic_k_v` exercises the path end-to-end. 337 e2e, all green.

#### Slice 2B — Gen-methods · ✅ shipped 2026-05-18 (machinery; stdlib follow-through pending)

**Limitation (closed):** v0.0.4 Slice 4A added `is_gen` to `Method` and the parser accepted `pub gen fn iter(self) -> T`, but sema's `check_method` didn't thread `current_fn_is_gen` and codegen's `gen_method` didn't dispatch to a coroutine path. Writing a `gen` method silently failed at sema (E1001 fires on `yield`).

**Fix shipped** across [cplus-core/src/sema.rs](cplus-core/src/sema.rs) + [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs):
- **Sema sig collection** (concrete impls + generic impl-method templates): when `m.is_gen`, wrap the declared return T → `Iterator[T]` via `wrap_in_iterator` (mirror of free-fn `gen` path).
- **Sema body checking** (`check_method`): when `m.is_gen`, set `body_return = Ty::Unit` (the body produces values via `yield`, not `return`), and push `current_fn_is_gen` + `current_gen_yield_ty` for the body's `yield` checks. Restore previous state on exit.
- **Codegen sig collection**: mirror the wrap in codegen's `TypeTable` so call-site method-lookup returns the right `Iterator[T]` aggregate.
- **Codegen method emission** (`gen_method`): when `m.is_gen`, dispatch to new `gen_gen_method`. This is a method-shaped clone of `gen_gen_function` — same `coro.id/begin/suspend/end` shape, but with receiver-prefix parameter layout and `Iterator[T]` return aggregate.

**Tests:** new e2e `phase2b_gen_method_on_struct` builds a user `struct Counter` with `pub gen fn iter(self) -> i32`, drives via `for x in c.iter()`. 338 e2e, all green.

**Stdlib follow-through (forward-pointed):** `Vec[T]::iter()` / `HashMap[K, V]::iter()` / `HashMap[K, V]::keys()` / `HashMap[K, V]::values()` / `File::lines()` all become trivial gen-methods now. Each addition needs the relevant stdlib module to `import "./iterator"` (so `Iterator[T]` is in scope for `wrap_in_iterator`), and every e2e test that stages that module to also stage `iterator.cplus` + `option.cplus`. That's ~10–15 test stagings to update — mechanical but each one's a small edit. Lands as a separate slice (call it 2B-follow-through or 3A.v2 — schedules with the iterator ecosystem in Phase 3).

#### Slice 2C — `impl` on enum types · ✅ shipped 2026-05-18 (non-generic + generic-enum impls)

**Limitation closed for non-generic enums:** v0.0.4 sema rejected `impl Foo` when `Foo` is an enum (E0325). v0.0.5 lifts that for both plain enums (`enum Tag { Yes, No }`) and tagged enums (`enum Shape { Circle(i32), Square(i32) }`).

**Fix shipped across [cplus-core/src/sema.rs](cplus-core/src/sema.rs) + [cplus-core/src/codegen.rs](cplus-core/src/codegen.rs):**

- **EnumDef + EnumInfo** got `methods: HashMap<String, MethodSig|MethodInfo>` mirroring the struct version. Construction sites pre-populate empty.
- **Sema `collect_methods`** routes enum impl targets to a new `collect_enum_impl_methods` that builds method sigs into `EnumDef::methods`. Same `gen-fn → wrap_in_iterator` wiring as structs. `drop` on enums explicitly rejected (E0338) — the §3.3 no-Drop-payload rule for tagged enums means there's nothing for a destructor to do.
- **Sema `check_methods`** dispatches to a new `check_enum_method` that mirrors `check_method` — `Self` resolves to `Ty::Enum(enum_id)`, receiver bindings take the enum's type, body sees the unwrapped return for `is_gen`.
- **Sema `check_method_call`** detects `Ty::Enum` receivers before the struct path and routes to `check_enum_method_call` (mirror of the struct method call check).
- **Codegen `TypeTable`** collects enum methods into `EnumInfo::methods` during the third pass.
- **Codegen emission dispatch** routes enum impl blocks to `gen_enum_method` (which forwards `is_gen` to `gen_gen_enum_method` — the enum-receiver mirror of `gen_gen_method`).
- **Codegen `gen_method_call`** detects enum receivers and routes to `gen_enum_method_call_inner` (mirror of the struct method-call lowering with sret-awareness, move-receiver disarm, and Unit-return handling).

**Tests:** new e2e `phase2c_enum_impl_methods_dispatch` exercises both plain (`Tag::flip` / `Tag::is_yes`) and tagged (`Shape::area`) enum methods. Updated the existing `impl_on_enum_e0325` lib test to assert the new accept behavior (renamed `impl_on_concrete_enum_accepted_phase2c`). 339 e2e + 866 lib + 11 LSP, all green.

**Generic-enum impls also shipped (follow-on, same day).** `impl Maybe[T] { fn is_some(self) -> bool { ... } }` style compiles + dispatches. Three-piece change: sema's `collect_generic_impl_methods` accepts both struct + enum generic templates (single `generic_impl_methods` table, keyed by name); `instantiate_enum_from_arg_tys` populates methods on the synthesized concrete enum (mirror of struct-side); `synthesize_generic_typed_impls` in monomorphize iterates both struct + enum instantiations to emit concrete impl items. New e2e `phase2c_generic_enum_impl_synthesis` covers Maybe[i32]/Maybe[bool] dispatching their is_some method correctly.

**Historical detail (resolved):** The Slice 2C original ship deferred this with this rationale:
1. Build a subst from impl's `target_generic_params → concrete_args` per enum instantiation.
2. Render each method's signature + body with `Self → Path(mangled enum name)` rewriting.
3. Emit one synthesized concrete `ImplBlock` per `(enum_name, args)` pair.

Mechanically a clone of the existing struct-side `synthesize_generic_typed_impls`. Lands as a small follow-up slice when motivated by stdlib (`impl Option[T]`, `impl Result[T, E]`).

#### Slice 2D — Bound-aware method dispatch + `<`-on-`T` lint · ✅ shipped 2026-05-19

**Two coupled bugs**, both surfaced by writing the canonical `fn max[T: Ord](a: T, b: T) -> T { if a.cmp(b) < 0 { return b; } return a; }`:

1. **`a.cmp(b)` on a `T: Ord` receiver** failed sema with "no method `cmp` on type `type-param`" (E0324). Generic bodies aren't fully sema-checked at definition time, but the methods on impl-generic bodies (`impl Vec[T] { fn push(self, x: T) }`) ARE checked with T as `Ty::Param`. In those bodies, calling a bound's method couldn't resolve because `check_method_call` only handled `Ty::Struct` / `Ty::Enum` receivers.
2. **`a < b` on a `T: Ord` receiver** was silently accepted at sema (Ty::Param-bodies skip body-check) and produced invalid `icmp slt %StructTy` at codegen, surfacing as a cryptic LLVM error ("icmp requires integer operands") only when the user happened to instantiate with a non-numeric type. C+ has no operator overloading (SKILL.md §2.6), so the only correct shape is `.cmp(other) <op> 0`.

**Fix shipped** in [cplus-core/src/sema.rs](cplus-core/src/sema.rs):

- **`param_bounds_stack: Vec<HashMap<String, Vec<String>>>`** — parallel to `type_params_stack`, tracks the declared interface bounds (`["Ord", "Eq"]`) for each in-scope generic param. Pushed/popped together via `push_type_params` / `pop_type_params` (so impl-method generic params + fn-level generics + interface-level generics all stack consistently).
- **`lookup_bound_method(param_name, method)`** — walks the bound stack to find the first frame declaring a bound for `param_name`, then returns the first matching method signature from the bound interfaces' method tables. Inner frames mask outer ones.
- **Method-call dispatch on `Ty::Param`** — in `check_method_call`, before falling through to E0324, check for a `Ty::Param(name)` receiver and resolve via `lookup_bound_method`. Substitutes `Self → Ty::Param(name)` in the interface method's param + return types via `subst_ty_deep` so arg-type checks match what the user wrote (`other: T` not `other: Self`).
- **`lint_generic_fn_bodies`** — narrow AST walker over generic-fn bodies that catches `<` / `<=` / `>` / `>=` on bare-Ident operands typed as generic parameters. Emits E0302 pointing at `.cmp()`. Doesn't run the full sema body-check (a false start — too many subsystems assume "generic bodies = lazy / monomorphization-time"; turning on body-check for generics cascaded through intrinsic dispatch and struct-instantiation lookups). Only catches bare-Ident-of-param operand shapes; `let x = a; if x < b { ... }` slips through but the canonical `.cmp()` idiom is what users should write.

**Tests:** new e2e `generic_max_with_ord_bound_calls_cmp_in_body` (positive — `max[T: Ord]` with `.cmp()` builds + dispatches per-instantiation) and `ordered_compare_on_generic_param_rejected_e0302` (negative — `<` on T fires E0302 with `.cmp()` + §2.6 in the message). 869 lib + 353 e2e green.

**Doc updates:** [SKILL.md](SKILL.md) §2.6 now shows both the rejected `<` shape and the canonical `.cmp()` shape side-by-side, with the explicit "no `T: Ord` desugar to `T::cmp` because that would *be* operator overloading" callout.

#### Phase 2 exit criteria

- [x] `HashMap[K, V]::get` can call `result::io_err::[V](...)` directly without inlining *(2A)*
- [x] `pub gen fn iter(self) -> T` parses, sema-checks, and codegens correctly *(2B — compiler machinery; stdlib follow-through pending)*
- [x] `impl Tag { fn flip(self) -> Tag }` on non-generic enums AND `impl Maybe[T] { ... }` on generic enums compile + dispatch *(2C + 2C follow-on)*
- [x] `fn max[T: Ord]` with `.cmp()` body compiles + dispatches; `<` on generic param rejected with helpful diagnostic *(2D)*

---

### Phase 3 — Iterator ecosystem · size M

Builds on Phase 2 to close the "you can have a generator but you can't iterate over your stdlib types" gap. The headline deliverable is `vec.iter().filter(pred).map(f).collect_to_vec()` reads and runs.

#### Slice 3A — `Vec[T]::iter()` · ✅ shipped 2026-05-18

```cplus
impl Vec[T] {
    pub gen fn iter(self) -> T {
        let mut i: usize = 0 as usize;
        while i < self.len {
            yield self.get(i);
            i = i +% (1 as usize);
        }
        return;
    }
}
```

First stdlib gen-method. Verifies Phase 2B's machinery on a concrete generic-struct instantiation (`Vec[i32]`). `for x in v.iter() { ... }` and explicit `v.iter().next()` both work.

**Test stagings updated:** vec.cplus now imports stdlib/iterator (for `wrap_in_iterator` to resolve at sig time), so every e2e test that stages vec.cplus also stages iterator.cplus + option.cplus. Twelve sites updated (~7 via `let vec_src = ...; std::fs::write(...)` pattern via replace_all; 4 for-loop name-list sites; 2 inline `let vec_src = ...; let io_src = ...; ...` sites; 1 in stdlib_net_read_fd_async).

New e2e `stdlib_vec_iter_for_in` exercises the path end-to-end. 340 e2e + 866 lib + 11 LSP, all green.

#### Slice 3B — Tuple types · ✅ partial shipped 2026-05-18 (types + projection; `HashMap::iter` follow-on)

Tuple types `(T1, T2, ...)`, tuple literals `(a, b, ...)`, and numeric field projection `pair.0` / `pair.1` ship end-to-end. Arity ≥ 2 (the parser rejects 1-element parens as grouping; `()` stays as the unit type's source spelling). Mixed element types work — `(i32, bool)`, `(string, i32)`, etc.

**Implementation:**
- **AST**: new `TypeKind::Tuple(Vec<Type>)` and `ExprKind::TupleLit(Vec<Expr>)`.
- **Parser**: `parse_primary` distinguishes tuple literal from grouping by looking for a comma after the first expression; `parse_type` adds an `LParen` branch with the same shape; numeric field access `.N` rewrites the int token to a synthetic `_N` ident matching the tuple struct's field names.
- **Sema**: `resolve_type(Tuple)` and `check_tuple_lit` both call a new `synthesize_tuple_struct` that creates a concrete `StructDef` named `__tuple_<t1>_<t2>_...` with fields `_0`, `_1`, ..., registered under `struct_instantiations` with the synthetic template name `"__Tuple"`.
- **Monomorphize**: `subst_type_ast` rewrites `TypeKind::Tuple` → `TypeKind::Path(mangled)` via the same `struct_lookup.by_names` mechanism that handles `TypeKind::Generic`. Tuple structs flow out as AST items via the existing `struct_instantiations` emission path.
- **Codegen**: `gen_tuple_lit` reconstructs the synthesized struct's name from element types (via a codegen-side `tuple_struct_name` that mirrors sema's naming), looks up the struct id, and emits the same alloca/store/load pattern as `gen_struct_lit`.

**HashMap::iter / keys / values follow-on (deferred again):** with tuple types shipped, `HashMap[K, V]::iter() -> (K, V)` and `keys()` / `values()` are *syntactically* expressible. Attempting `pub gen fn keys(self) -> K` on the generic `HashMap[K, V]` impl, however, triggers a codegen panic — even with the method body emptied. Adding the method alone (no caller) makes a downstream method's match-arm method-call see `Ty::Error` in the receiver/arg slot, then `llvm_ty(Ty::Error)` aborts. The non-generic variant (`pub gen fn slot_count(self) -> usize`) is fine; the bug is specifically in **generic-return gen-methods on generic impls**. Needs a focused investigation (likely `populate_generic_impl_methods` interaction with `wrap_in_iterator` when the inner T is `Ty::Param`). Forward-pointed.

**Limitations:**
- No tuple destructuring patterns yet (`let (a, b) = pair;`). Access via `.0` / `.1` works.
- No tuple pattern in `match` arms.
- 1-tuples not supported (no syntactic disambiguation from grouping without trailing-comma sugar — deferred).

`HashMap[K, V]::iter()` needs to yield `(K, V)` pairs — which requires tuple types as values. v0.0.4 doesn't have `(a, b)` as a type. Land as a small slice:
1. Sema: `(T1, T2)` is a synthetic struct with fields `.0`, `.1`. Parse `(a, b)` as a tuple constructor; `.0`/`.1` access as field projection.
2. Codegen: lower as `{T1, T2}` LLVM struct.
3. `HashMap::iter()` yields `(K, V)`; `for (k, v) in m.iter() { ... }` works via destructuring.

If tuple types feel too big for one slice, ship `HashMap::keys()` + `HashMap::values()` first (each yields a single primitive), then revisit `iter()` for v0.0.6.

#### Slice 3C — Iterator adapters · ✅ shipped 2026-05-18

`Iterator[T]::filter(self, pred: fn(T) -> bool)`, `::take(self, n: usize)`, and free `iterator::map[T, U](source: Iterator[T], f: fn(T) -> U)` — all `gen fn`s. `map` is a free function rather than a method because method-level generics on top of an impl's `T` aren't parseable today (the impl declares `T`, the method would need to declare an extra `U`); the free-fn form sidesteps that without losing expressiveness.

The keystone was a sema-side propagation pass — `propagate_pattern_instantiations` — that walks generic-impl-method bodies and generic free fn bodies for `match`/`if let`/`while let`/`guard let` patterns whose `PatternKind::Variant` carries explicit type-args. For each concrete struct/fn instantiation it substitutes through the outer subst and feeds the discovered `(enum_name, concrete_args)` to `instantiate_enum_from_arg_tys`. Without this, `Iterator[i32]::filter`'s synthesized body references an un-instantiated `Option[i32]` and codegen panics in `lty(Ty::Enum(EnumId(0)))`.

The struct-side analog wasn't needed: struct fields are pure `Type` nodes that already lower through the existing `struct_instantiations` path. A single non-iterative pass over the seed set is enough — pattern discoveries from one instantiation don't feed back to discover more struct instantiations (struct fields don't carry patterns).

`collect_to_vec` shipped (2026-05-18 follow-on) as **`vec::collect[T]`** — a free generic fn rather than `impl Iterator[T]::collect_to_vec` to avoid the iterator↔vec circular import. Body uses `move source` + `match source.next() { Option[T]::Some(x)/None }` to drain the iterator into a fresh `Vec[T]`. New e2e `stdlib_vec_collect_drains_iterator` covers `src.iter().filter(is_pos)` → Vec[i32].

#### Slice 3D — `File::lines()` · ✅ shipped 2026-05-18

```cplus
impl File {
    pub gen fn lines(self) -> string {
        // ... reads self.fd, yields each newline-terminated chunk.
    }
}
```

Shipped as `pub gen fn lines(self) -> string` on `impl File` in [vendor/stdlib/src/fs.cplus](vendor/stdlib/src/fs.cplus). Reads in 4 KiB chunks via libc `read(self.fd, ...)`; scans for `\n`; yields each line as an owned `string` (built via `str_from_raw_parts(...).to_string()`). Carry-over `Vec[u8]` handles lines spanning chunk boundaries; a final fragment without trailing `\n` at EOF is yielded as the last line. New e2e `stdlib_fs_file_lines_round_trip` validates the chunk-and-carry path. Side fix: added `Vec::clear` so the carry-buffer can be reset in place (drops elements, retains capacity).

#### Slice 3E — Borrow check across yield (dataflow tightening) · M

v0.0.4 ships permissive — gen fns can have `str` / `T[]` params. That's safe for the typical case (next() caller's frame outlives iteration) but unsafe for nested generators where one gen fn's yielded value borrows into another's frame. Tighten with a dataflow rule: parameters with borrow-shaped types may not be live across a yield-into-nested-gen-fn boundary.

**Forward-pointable** if no real workload surfaces the gap.

#### Phase 3 exit criteria

- [ ] `for x in v.iter() { ... }` works on `Vec[i32]`
- [x] `v.iter().filter(pred)` / `iterator::map[T,U](v.iter(), f)` / `.take(n)` works *(3C)*; `vec::collect::[T](v.iter().filter(p))` drains to Vec[T] *(3C follow-on)*
- [ ] `for (k, v) in m.iter() { ... }` works (or `for k in m.keys()` as the smaller cut)
- [x] `for line in f.lines() { ... }` works *(3D)*
- [ ] At least 3 stdlib types expose iterator-style API

---

### Phase 4 — Async polish · size M

Closes the v0.0.4 Track A wrappers and unblocks a real-world async demo.

#### Slice 4A — `sleep(ms)` via `EVFILT_TIMER` · ✅ shipped 2026-05-18

```cplus
pub async fn sleep(ms: u64) {
    unsafe { __cplus_reactor_wait_timer(ms); }
    return;
}
```

Shipped in [vendor/stdlib/src/time.cplus](vendor/stdlib/src/time.cplus). New compiler intrinsic `__cplus_reactor_wait_timer` mirrors `wait_read`/`wait_write`: registers a one-shot EVFILT_TIMER with the reactor (ident = coro-handle pointer, data = ms), then suspends self via `llvm.coro.suspend`. When the timer fires, `poll_one_event` recognizes `EVFILT_TIMER` and reads the ident back as the handle pointer, resuming the coroutine directly — no separate waiter table needed.

Side fix: `await`-of-`Future[Unit]` produced invalid `load void` IR. Codegen now short-circuits the promise read when `U == Unit`, since the unit value carries no payload. Was latent in v0.0.4 (no `async fn` returning unit was exercised end-to-end); 4A's first user (`time::sleep`) surfaced it.

epoll port adds `timerfd_create` — same intrinsic, different reactor backend (Phase 5).

#### Slice 4B — `TcpStream::read_async` / `write_async` / `accept_async` method form · ✅ shipped 2026-05-18

Shipped in [vendor/stdlib/src/net.cplus](vendor/stdlib/src/net.cplus). `TcpStream` gains `read_async`, `write_async`, `write_all_async`, `make_nonblocking`; `TcpListener` gains `accept_async` + `make_nonblocking`. All take `mut self` (pointer-passed, not consumed) so callers can chain across calls without losing the handle to Drop. The method bodies delegate to the existing free-fn `*_fd_async` implementations through `self.fd` — keeps the free-fn surface as the load-bearing path, methods are sugar.

The 1A `move`-on-value-pass blocker turned out to be a red herring: `mut self` was the right shape all along. The previous "free-fn-only" decision was based on `self` (value-pass) consuming the stream, but `mut self` is pointer-passed and doesn't.

Compiler machinery added alongside:

- **Sema** ([cplus-core/src/sema.rs](cplus-core/src/sema.rs)): `collect_methods` (struct + enum + generic-impl) now wraps `T → Future[T]` for `m.is_async` (parallel to the existing `is_gen → Iterator[T]` wrap). `check_method` + `check_enum_method` thread `current_fn_is_async` and unwrap `Future[T]` for the body's return type so `await` checks fire correctly.
- **Codegen** ([cplus-core/src/codegen.rs](cplus-core/src/codegen.rs)): `collect_sigs` mirrors the wrap. New `gen_async_method` parallels `gen_gen_method` — same method-shaped signature (receiver + params, mangled name) but with `gen_async_function`'s coroutine body (coro.id/begin/suspend/end, promise allocation, Future-aggregate return).

E2E covered by new `async_method_on_user_struct_round_trip` — exercises `mut self` async method body via pipe + reactor end-to-end.

#### Slice 4C — `File::read_async` · ✅ shipped 2026-05-18

Shipped as `pub async fn read_async(mut self, buf, count) -> isize` + `pub fn make_nonblocking(mut self) -> i32` on `impl File`. Forwarder over `net::read_fd_async(self.fd, ...)` / `net::set_nonblocking(self.fd)` — fs.cplus imports net.cplus to pull in both. New e2e `stdlib_fs_file_read_async_compiles` exercises the method-form call site (`move f` + local re-bind to dodge the E0900 mut-pointer-pass-through-await guard).

#### Slice 4D — Hand-rolled `Future` implementations · M

v0.0.4 forward-pointed this on "needs dyn-dispatch design." The pragmatic answer: **monomorphize**. `block_on::[F: Future]` synthesizes one drive loop per concrete F. No `dyn Future`, no trait objects — same model the rest of the language uses.

Land `interface Future[T] { fn poll(mut self) -> Poll[T]; }`, accept user `impl Future for MyTimer`, generate `block_on::[MyTimer]` monomorph on demand. Compiler-coroutine futures (from `async fn`) get a synthetic `impl Future for Future__T` so the same path drives both.

#### Slice 4F — Executor awaiter-notification fix · ✅ shipped 2026-05-18

Closes the multi-level-await stall surfaced during 4E. v0.0.4's `block_on` only re-resumed the *outermost* future on each loop pass, so any coroutine awaiting an inner async fn would stall when the inner suspended on fd/timer + later completed (the inner's `coro.resume` came from `poll_one_event`, never the awaiter).

**Fix:**
1. Reactor grows an awaiter table — parallel `awaitee_hdls` / `awaiter_hdls` arrays in the state header (header 72→96 bytes; new offsets at 72..96). New exports `stdlib_reactor_register_awaiter_v1` + `stdlib_reactor_notify_completed_v1`. `register_awaiter` appends a `(awaitee, awaiter)` mapping; `notify_completed` linear-scans, swap-removes the matching entry, and routes the awaiter through `enqueue_pending` so `drain_pending` picks it up.
2. Codegen `gen_await_expr` emits `call void @stdlib_reactor_register_awaiter_v1(inner_hdl, %.coro.hdl)` in the `resume_bb` right before the awaiter suspends itself.
3. Codegen `gen_async_function` + `gen_async_method` emit `call void @stdlib_reactor_notify_completed_v1(%.coro.hdl)` as the first instruction of `.coro.final_suspend` — fires before the final suspend executes, so the awaiter is enqueued before this frame returns. By the time the awaiter resumes (via `drain_pending` later), `coro.done(awaitee)` reads true and the awaiter's await loop extracts cleanly.

Single-threaded executor is enough — no race between "awaitee completes" and "awaiter checks done", since neither happens concurrently.

**Knock-on cleanups:**
- `TcpStream`/`TcpListener` method wrappers reverted from non-async forwarders (4E workaround) back to real `async fn` methods.
- `write_all_fd_async` reverted from inlined-loop back to `await write_fd_async(...)` (the natural shape).
- The `gen_async_method` codegen path (4B) remains the canonical stdlib shape.

New e2e `phase4f_concurrent_n_sleeps_stress` — 50 concurrent `time::sleep(50ms)` futures complete in ~50ms (loose bound 40..500ms). Sequential drive would be Σ=2500ms; observed ~52ms confirms full concurrency.

#### Slice 4E — `async_fetch` recipe · ✅ shipped 2026-05-18 (single-client; 1000-task stress unblocked by 4F)

Shipped: [docs/examples/recipes/async_fetch/](docs/examples/recipes/async_fetch/) — a single-client async TCP fetcher using method-form `stream.read_async` / `write_all_async` / `make_nonblocking`. New e2e `recipe_async_fetch_runs` exercises it against a real `127.0.0.1` echo server running in a sidecar Rust thread; client reads back the byte the server echoed.

**Forward-pointed: 1000-task concurrent stress** — blocked on an executor improvement, NOT on async surface gaps. v0.0.4's `block_on` drive loop only re-resumes the *outermost* future on each iteration:

```
loop:
    if done(outer): extract
    resume(outer)
    drain_pending          # spawned futures get one-shot resume
    poll_one_event         # ONLY resumes the coroutine whose timer/fd waiter fired
```

So when an inner coroutine (e.g. `write_fd_async`) suspends, completes, and its *awaiter* (e.g. `write_all_fd_async`) is waiting on its done bit — the awaiter never gets re-resumed, because nobody schedules it. Three knock-on effects appeared during 4E:

1. **Stdlib method wrappers can't be `async fn`.** They were async in the first cut; the recipe hung whenever EAGAIN actually fired (happy path passed because the kernel rarely blocked a 1-byte write). Refactored to plain `pub fn read_async(...) -> Future[isize] { return read_fd_async(self.fd, ...); }` — synchronous forwarders that propagate the underlying free-fn's `Future` so the caller's `await` chains at ONE level. The `gen_async_method` codegen path from 4B still works (validated by `async_method_on_user_struct_round_trip`) — it's just not the recommended shape for stdlib wrappers in v0.0.5.
2. **`write_all_fd_async` inlines `write_fd_async`'s syscall/EAGAIN loop** rather than `await`ing it. Same reason — nested `await` on a free fn that internally suspends stalls when EAGAIN fires.
3. **No spawn_local-with-internal-await pattern.** `spawn_local`'d futures get one resume from `drain_pending`. Once they suspend, only their direct fd/timer waiter wakes them — never their awaiter. The async_yield_demo recipe sidesteps via `yield_now()` (which explicitly re-enqueues self in pending), but that's cooperative-only, not "fire a 1000-task fanout and wait on `block_on`."

The 1000-task stress lands when the executor learns: **on coroutine completion, schedule any registered awaiter for resume.** Concretely: each `await` site registers the surrounding fn's hdl as an "awaits this hdl" link; coroutine epilogue (in `coro.end` / final_suspend ramp) checks for and drain-enqueues a registered awaiter. ~150 lines across `codegen.rs` (await-site link, completion-callback emission) + `reactor.cplus` (awaiter table). Drops naturally out of an executor refactor and is a Phase 5 candidate.

#### Phase 4 exit criteria

- [x] `sleep(100).await` actually sleeps ~100ms in `block_on` *(4A)*
- [x] `stream.read_async(buf, n).await` works (method form) *(4B)*
- [ ] `impl Future for MyTimer { ... }` compiles and runs through `block_on`
- [x] async_fetch single client compiles + round-trips (4E); 50 concurrent timer awaits complete in ~max time (4F, e2e `phase4f_concurrent_n_sleeps_stress`)

---

### Phase 5 — Platform parity · size L

The big "Linux works" lift. macOS arm64 is fully validated; Linux x86_64 has stdlib + reactor + threading code that's never been verified there.

#### Slice 5A — Linux x86_64 stdlib ABI verification · M

- Run the full test suite under Linux x86_64. Expected breakage: variadic ABI quirks (`fcntl` was already fixed for AArch64), pthread struct layout (already padded for portability in Mutex/Channel), errno location (`__errno_location()` vs `__error()`).
- Fix per-platform constants in `net::eagain_*` and add the missing extern decls.
- Verify thread spawn/join, atomics, async runtime all green.

#### Slice 5B — pthread `[link]` entry · S

Stdlib's `Cplus.toml` needs `[link] libs = ["pthread"]` for Linux. macOS gets it free via libSystem. Add the manifest field; codegen passes `-lpthread` to clang on Linux targets.

#### Slice 5C — aarch64-Linux smoke test · S

HFA optimization differs from darwin-aarch64. Run a stdlib smoke test on aarch64-Linux to catch any places where Phase 1F's recursive mangler or the thread trampoline got the wrong calling convention.

#### Slice 5D — epoll variant of reactor · M

Mirror `vendor/stdlib/src/reactor.cplus`'s kqueue implementation for Linux:
- `epoll_create1`, `epoll_ctl(EPOLL_CTL_ADD, EPOLLIN | EPOLLONESHOT)`, `epoll_wait`.
- Same `(fd, filter, hdl)` waiter table shape; same `poll_one_event` semantics.
- Compiler intrinsics (`__cplus_reactor_wait_read` etc.) stay platform-agnostic; the stdlib internals branch on `cfg(target_os = "linux")`.

#### Phase 5 exit criteria

- [ ] Full test suite passes on Linux x86_64
- [ ] Full test suite passes on Linux aarch64 (smoke at minimum)
- [ ] `async_fetch` recipe works under epoll on Linux

---

### Phase 6 — Perf + polish · size M

The leftover bucket — closing the language-polish carryovers and the raytracer codegen gap.

#### Slice 6A — Raytracer perf investigation · M

C+'s raytracer is ~30% slower than C / Rust on the same algorithm with the same RNG seed. Same machine code in the hot loop per `--emit-asm`; the gap is elsewhere. Plausible items:
- `-ffp-contract=on` (FMA fusion) defaults — C+ emits without FMA hints
- Missing `noundef` / `noalias` / `nofree` attributes on hot-path params
- Autovec heuristics — LLVM's loop vectorizer may skip without the right metadata
- Profile-guided inlining hints

Audit slice. Each plausible item gets benchmarked in isolation.

#### Slice 6B — Slice indexing `s[i]` with bounds-check · S

`let b: u8 = s[i];` parses today as a function call; rewrite as a real indexing operator on slice types. Bounds-check via `llvm.assume` so `-O2` elides the check after a prior bound proves it.

#### Slice 6C — Array → slice coercion (`arr as T[]`) · S

`let a: [u8; 16] = ...; let s: u8[] = a as u8[];` — converts a stack array to a slice (fat pointer with len from the array type). Today users construct slices via `slice_from_raw_parts` manually.

#### Slice 6D — String interpolation polish · S

Format specifiers: `"{x:5}"` / `"{x:.2}"` / `"{x:08x}"`. v0.0.4 ships bare interpolation; format specifiers are deferred.

#### Slice 6E — `while let` statement + multi-binding `guard let` patterns · S

Both inherited from v0.0.1 carry-forward. `while let Some(x) = it.next() { ... }` desugars to the existing `for x in it` shape; multi-binding `guard let Pattern1 | Pattern2 = expr` matches either alternative.

#### Slice 6F — Negative trait impls (`Rc[T]: !Send`) · M

Phase 2A in v0.0.4 shipped permissive Send/Sync. Negative impls let stdlib mark `Rc[T]: !Send`, `MutexGuard[T]: !Send`, and raw-pointer-bearing structs `!Send` unless the user opts in with `unsafe impl Send for MyType {}`. Real enforcement.

#### Slice 6G — Auto-derive attributes · M

`#[derive(Eq)]`, `#[derive(Hash)]`, `#[derive(Clone)]` on user structs. Generates the obvious field-walking impl. Lands after Phase 2C (`impl` on enums) since deriving on enums needs that machinery.

#### Slice 6H — Array repeat-count literal `[0u8; 10]` · S

Doesn't parse today. Workaround is listing elements. ~30-line parser slice.

#### Phase 6 exit criteria

- [ ] Raytracer perf within 10% of C / Rust on the same workload
- [ ] `s[i]` works with bounds-check
- [ ] `arr as T[]` works
- [ ] `"{x:5.2}"` formats correctly
- [ ] `Rc[T]: !Send` rejects cross-thread move

---

### Phase 7 — Tooling · size S

Small bucket of `cpc` UX wins. Each is independent; ship as warmup or between phases.

- **`cpc init <name>`** — scaffolder for a fresh project (Cplus.toml + src/main.cplus + .gitignore).
- **`cpc doc` project mode + HTML** — today `cpc doc` runs per-file; project mode walks Cplus.toml and emits an HTML site.
- **LSP cross-file code actions + pull diagnostics** — multi-file `WorkspaceEdit` support, on-save pull-mode diagnostics (the editor asks, not just push-when-changed).
- **ANSI-colored diagnostics** — `--color=auto/always/never` flag.
- **dsymutil integration + per-instruction `!DILocation` + `DILocalVariable`** — full debug info pipeline so `lldb` breakpoints + variable inspection work without `--release` tricks.
- **`cpc fmt` turbofish-pointer fix** — `size_of::[*u8]()` reformats to `size_of::[* u8]()` today. ~5-line fix.
- **Full `println` intrinsic removal** — v0.0.3 moved `println` to stdlib; the compiler still has a fallback intrinsic path. Remove it; stdlib's `io::println` is the only entry.

---

### Carryovers — also landing in v0.0.5

The "land when motivated" items from v0.0.4 that don't gate a phase but shouldn't be forgotten:

- **`Arc::make_mut`** (clone-on-write to mutable inner)
- **`Mutex::try_lock`** / `lock_with_timeout`
- **`channel::bounded(n)`** / `try_recv` / `recv_timeout`
- **Generic `Cow[T_view, T_owned]`** (only `CowStr` ships today)
- **`CowSlice[T]`** (the `T[]` parallel)
- **Method-level generic bounds inside generic-typed impls** (v0.0.1 carry-forward)
- **Generic struct / enum type-args in generic-fn bodies** (Phase 1B limitation — nested generic-of-generic still rough)
- **Editions support** (v0.0.1 design note)
- **HFA optimization on aarch64** (correct but suboptimal SIMD float aggregates)
- **String `s.scalars()` / `s.graphemes()` iteration**
- **Unicode `\u{HHHH}` escapes in string literals**
- **`impl Ord for i32`** (bounds on built-in primitives — newtype workaround today)
- **`cpc-bindgen`** (libclang-based — separate tool, not language; build when hand-writing bindings becomes painful)

These are S-each and ship between phases or as bundled polish PRs.

### Things explicitly NOT on this roadmap

Locked decisions from v0.0.2 / v0.0.3 / v0.0.4; don't reopen without a clear motivating case:

- Effect tracking + built-in contracts (rejected 2026-05-14)
- Phase 9 / TS-flavored review (rejected 2026-05-13)
- Null in safe code (locked — FFI null is `0 as *T` in `unsafe`)
- `?*T` nullable pointers (killed 2026-05-14)
- Dynamic dispatch / `dyn Interface` (Phase 7 is monomorphization-only)
- Multi-package repos (subdirectory packages)
- Package-manager sandbox / capabilities
- Windows-MSVC `pub extern fn` (needs `inalloca`, rejected v0.0.2 Slice 1H Tier-3)
- Multi-threaded async executor (v0.0.6+ territory)
- SIMD primitives (waits for an intrinsic-plumbing slice)

---

## Known compiler bugs (surfaced during use)

Tracked separately from feature carryovers — these are wrong-behaviour bugs in shipped code paths, not deliberate deferrals.

_(none open at v0.0.5 start — see plan-0.0.4.md's resolved log for v0.0.4's fixes; Phase 1 slices above absorb the carried-forward Drop bugs)_

---

## Resolved log

- **2026-05-18** — Carry-over cleanup pass: shipped `vec::collect[T]` (drains Iterator[T] → Vec[T], free fn to dodge iterator↔vec circular import) + generic-enum impl synthesis (2C follow-on: `impl Maybe[T] { fn is_some(self) -> bool }` style — sema lifts the E0325 gate, `instantiate_enum_from_arg_tys` populates methods on synthesized concrete enums mirroring the struct side, monomorphize's `synthesize_generic_typed_impls` iterates both `struct_instantiations` + `enum_instantiations`). New e2e `stdlib_vec_collect_drains_iterator` + `phase2c_generic_enum_impl_synthesis`. Investigated `HashMap::keys/values` (gen-method on generic impl yielding K/V) but blocked on a codegen interaction bug — adding `pub gen fn keys(self) -> K` to HashMap[K, V]'s impl makes a downstream method's match-arm method-call see Ty::Error during codegen, even with the method body empty. Specific to **generic-return gen-methods on generic impls**; non-generic gen-methods (`fn slot_count(self) -> usize`) work fine. Forward-pointed pending focused investigation. Tuple destructuring (`let (a, b) = pair;`) deferred — needs parser refactor to emit multiple stmts per `let` (move from `parse_let_stmt -> Stmt` to `-> Vec<Stmt>` + threading through callers). 350 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 3 Slice 3D + Phase 4 Slice 4C shipped together. `File::lines() -> Iterator[string]` reads 4 KiB chunks via libc `read`, scans for `\n`, yields each line as an owned `string` (`str_from_raw_parts(...).to_string()`); carry-over `Vec[u8]` handles chunk-spanning lines, with a new `Vec::clear` (drops elements, retains capacity) added to support per-iteration reset. `File::read_async(buf, count) -> isize` + `File::make_nonblocking()` are forwarders over the existing `net::read_fd_async` / `net::set_nonblocking`; fs.cplus imports net.cplus to pull both. New e2e `stdlib_fs_file_lines_round_trip` validates 3-line `alpha\nbeta beta\ngamma` parses to lines totaling 19 bytes; `stdlib_fs_file_read_async_compiles` covers the method-form call site (`move f` + local re-bind dodges the E0900 mut-pointer-pass-through-await guard). The existing `stdlib_fs_round_trip` test was updated to stage the new transitive imports (net + reactor + future). 348 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 3 Slice 3B partial. Tuple types (`(T1, T2, ...)`), tuple literals (`(a, b)`), and numeric field projection (`pair.0`, `pair.1`) ship end-to-end. Arity ≥ 2 (grouping `(x)` and unit `()` keep their existing meanings). Sema synthesizes a concrete `__tuple_<t1>_<t2>_...` struct per unique element-type combo with fields `_0`, `_1`, ... registered under the synthetic template `"__Tuple"` in `struct_instantiations`. Monomorphize rewrites `TypeKind::Tuple` → `TypeKind::Path(mangled)`. Codegen reconstructs the matching struct from element types and uses the same insertvalue/load shape as `gen_struct_lit`. AST + parser + sema + monomorphize + codegen changes; ~250 lines net. New e2e `phase3b_tuple_construct_projection_round_trip` covers fn-return tuples, 3-tuples, mixed element types. `HashMap::iter -> (K, V)` is now expressible; the impl itself is a follow-on slice. 346 e2e + 866 lib + 11 LSP, all green. Tuple destructuring patterns + 1-tuples deferred.

- **2026-05-18** — Phase 4 Slice 4F shipped. Executor awaiter-notification fix closes the multi-level-await stall. Reactor: new awaiter table (parallel `awaitee_hdls`/`awaiter_hdls` arrays, header 72→96 bytes), exports `stdlib_reactor_register_awaiter_v1` + `stdlib_reactor_notify_completed_v1`. Codegen: `gen_await_expr` emits `register_awaiter(inner, self)` in resume_bb before suspending; `gen_async_function` + `gen_async_method` emit `notify_completed(self)` as the first instruction of `.coro.final_suspend`. Knock-on: stdlib `TcpStream`/`TcpListener` methods reverted from 4E's non-async forwarders back to real `async fn`s; `write_all_fd_async` reverted to nested-await shape. New e2e `phase4f_concurrent_n_sleeps_stress` — 50 concurrent `time::sleep(50ms)` complete in ~52ms (Σ would be 2500ms). 345 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 4 Slice 4E partial. Shipped: async_fetch recipe + e2e `recipe_async_fetch_runs` (single-client TCP fetch via method-form async I/O against a real localhost echo server). Discovered an executor limitation while attempting the 1000-task stress: `block_on` only re-resumes the outermost future on each loop pass, so any coroutine *awaiting* an inner async fn stalls when the inner suspends and completes (nobody schedules the awaiter for resume). Two stdlib follow-ups landed in the same slice: (1) `TcpStream`/`TcpListener` async methods refactored from `async fn` to plain `pub fn ... -> Future[T]` forwarders (single-level await), (2) `write_all_fd_async` inlined `write_fd_async`'s syscall/EAGAIN loop to avoid nested `await`. The `gen_async_method` codegen path from 4B remains supported (`async_method_on_user_struct_round_trip` still passes) — it's just not the right shape for stdlib wrappers under v0.0.4's executor. 1000-task stress + general multi-level-await support forward-pointed; lands when the executor grows awaiter-of-completed-coroutine notification (~150 lines, Phase 5 candidate). 344 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 4 Slice 4B shipped. Async method form on `TcpStream`/`TcpListener` (and on any user struct/enum). `mut self` is pointer-passed so methods don't consume their receiver — the 1A `move`-on-value-pass blocker was a red herring; `mut self` was the right shape from day one. Method bodies delegate to the existing free-fn `*_fd_async` implementations through `self.fd`. Compiler-side: sema's method-sig collection (`collect_methods` for struct + enum + generic-impl) now wraps `T → Future[T]` for `m.is_async`; `check_method` / `check_enum_method` thread `current_fn_is_async` and unwrap for the body return type. Codegen's `collect_sigs` mirrors the wrap; new `gen_async_method` parallels `gen_gen_method` (method-shaped signature + `gen_async_function`'s coroutine body). New e2e `async_method_on_user_struct_round_trip`. 343 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 4 Slice 4A shipped. `time::sleep(ms)` runs an async task on the reactor's timer path. New compiler intrinsic `__cplus_reactor_wait_timer(ms: u64)` (sema + codegen, mirrors `wait_read`/`wait_write` exactly — unsafe + async-only gates, switched-resume suspend pattern). Reactor extension: 8-byte header growth for `n_timers`, new `register_timer` + stable export `stdlib_reactor_register_timer_v1`, `poll_one_event` recognizes `EVFILT_TIMER` (-7) and reads the kevent ident back as the coro-handle pointer (handle doubles as the timer ident — no waiter-table allocation needed for timers). `waiter_count` now sums fd-waiters + timer-count so `block_on` keeps driving while either is pending. New stdlib module `time.cplus`. Latent codegen bug surfaced: `await` of `Future[Unit]` produced `load void, ptr ...` (illegal LLVM); fixed by short-circuiting the promise read when U is Unit. New e2e `stdlib_time_sleep_round_trip` validates 80ms sleep returns within 70..500ms (proving suspend really blocked, not busy-looped). 342 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 3 Slice 3C shipped. Iterator adapters `filter` + `take` (gen methods on `Iterator[T]`) and free gen fn `iterator::map[T, U]`. Keystone fix: a new sema-side `propagate_pattern_instantiations` pass that walks generic-impl-method bodies and generic free fn bodies for `match`/`if let`/`while let`/`guard let` patterns, finds `PatternKind::Variant` nodes carrying explicit type-args, substitutes through the outer subst, and registers the discovered `(enum_name, concrete_args)` via `instantiate_enum_from_arg_tys`. Without this, monomorphize's existing `ExprKind::Call`-only walk missed `Option[i32]` references hidden behind variant patterns in `Iterator[i32]::filter`'s synthesized body, and codegen panicked in `lty(Ty::Enum(EnumId(0)))`. Sema-side (not mono-side) is the right home because `instantiate_enum_from_arg_tys` lives on `SemaCx` and the substitution path needs sema's struct/enum tables. New e2e `stdlib_iterator_adapters_filter_take_map`. 341 e2e + 866 lib + 11 LSP, all green. 3D (File::lines) is unblocked from the propagation side — remaining work is straight File API plumbing.

- **2026-05-18** — Phase 3 Slice 3A shipped. `Vec[T]::iter()` is the first stdlib gen-method — uses Phase 2B's machinery on a concrete generic struct instantiation. Pure-source stdlib: `pub gen fn iter(self) -> T { while i < self.len { yield self.get(i); i +%= 1 } }`. Side work: vec.cplus now imports stdlib/iterator (for `wrap_in_iterator` at sig collection time), so every e2e test that stages vec.cplus also stages iterator.cplus + option.cplus — ~12 staging sites updated. New e2e `stdlib_vec_iter_for_in` covers `for x in v.iter()` end-to-end. 340 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 2 Slice 2C shipped (non-generic enums). `impl EnumName { fn ... }` now compiles for both plain enums (`Tag::Yes/No`) and tagged enums (`Shape::Circle(i32)/Square(i32)`). Touched `EnumDef`/`EnumInfo` (added `methods` field), sema (`collect_enum_impl_methods` + `check_enum_method` + `check_enum_method_call`), codegen (`gen_enum_method` + `gen_gen_enum_method` + `gen_enum_method_call_inner`). Drop on enums still rejected (E0338). Generic-enum impls (`impl Option[T]`) forward-pointed pending monomorphize-side `synthesize_generic_typed_impls` analog for `enum_instantiations` — straightforward clone when motivated. Existing E0325-on-enum lib test renamed to assert the new accept behavior. New e2e `phase2c_enum_impl_methods_dispatch`. 339 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 2 Slice 2B shipped. Gen-method compiler machinery: sema's `check_method` threads `current_fn_is_gen` + `current_gen_yield_ty` for the body's `yield` checks; sig collection wraps T → Iterator[T] for `m.is_gen`; codegen's `gen_method` dispatches to new `gen_gen_method` (a method-shaped clone of `gen_gen_function` with receiver-prefix params + Iterator[T] return aggregate). User structs can now declare `pub gen fn iter(self) -> T` and consume via `for x in obj.iter()`. New e2e `phase2b_gen_method_on_struct`. Stdlib follow-through (Vec/HashMap iter methods + test stagings) forward-pointed to a separate slice. 338 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 2 Slice 2A shipped. Generic-method-body propagation: `propagate_fn_instantiations` now also walks every generic impl-block's methods for each `struct_instantiation`, substituting the impl's type params through the recorded turbofish args. Method-discovered pairs feed a secondary worklist that re-runs the existing free-fn propagation for transitive chains. Stdlib follow-through: `HashMap[K, V]::get` reverts its v0.0.4 inlining workaround — `return result::io_err::[V](...)` / `return result::io_ok::[V](v)` work directly now. 337 e2e, all green.

- **2026-05-18** — Phase 1 Slice 1D closed (was already fixed). Investigation confirmed the v0.0.4-carryover bug ("scalar `i32` async fns under `--asan` return 0") was incidentally cured by Phase 1E's promise-alloca fix — once `coro.id` got a real `alloca <T>` instead of `ptr null`, the previously-OOB writes landed in a CoroSplit-tracked region that ASan no longer flags. New e2e `phase1d_async_runs_clean_under_asan` locks the fix in: scalar i32/i64/bool generic-async instantiations + a chained-await pair all return their declared values under `--asan`. 337 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 1 Slice 1C shipped. Inner-T Drop on container teardown. New `__cplus_drop_in_place::[T](p: *T)` compiler intrinsic — sema accepts shape, codegen dispatches to `<mangled>.drop(p)` for struct T (or to inline string-free for `Ty::String`, no-op otherwise). Container `drop` methods now walk live storage and call the intrinsic per element: Box's inner T, Vec's `[0, len)`, HashMap's occupied slots (both K and V), Arc/Rc/Mutex's last-ref inner T, Channel's buffered `[head, tail)`. Box::unwrap reworked to null `self.p` after extracting the inner value so the scope-exit Drop short-circuits cleanly. New e2e `phase1c_container_inner_drop_runs_without_crash`. 336 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 1 Slice 1B shipped. Block-tail `Ident(name)` of a non-Copy binding now moves the value out cleanly instead of dropping it before the caller's slot picks it up. Fixed via two-side coordination in codegen: `scan_moves_in_block` pre-marks the tail-Ident binding for Runtime drop disposition; `gen_block_expr` calls `mark_moved` after evaluating the tail and before `pop_scope`. Top-level `let b: string = a;` form was already correct (Phase 3 handled it). New e2e `block_tail_ident_non_copy_does_not_double_free` covers single-level and nested rebinds. 335 e2e + 866 lib + 11 LSP, all green.

- **2026-05-18** — Phase 1 Slice 1A attempted, reverted. Auto-promote (treat non-Copy non-`move` params as if `move` was written) breaks read-only stdlib APIs that take non-Copy values (CowStr inspectors, fs/net helpers). Narrow "error on direct return of single non-Copy param" approach conflicts with the borrow checker's E0372/E0384 layering — sema errors abort the pipeline before borrowck, so the narrow error masks the more-specific diagnostics those tests expect. Three follow-up design paths documented in plan.md Slice 1A. Headline bug (`fn echo(x: string) -> string { return x; }` double-frees) carries forward with the same `move <name>` workaround as v0.0.4.

--- 

fn max[T: Ord] with no operator overloading is suspicious. How does a < b resolve in the body? Either Ord is a magic compiler-known bound (which is overloading by another name) or there's an Ord::lt(a, b) form the tutorial isn't showing. Worth pinning down.

The mutex-guard-deadlock gap is a real soundness hole. Rust prevents two-guards-in-a-scope structurally; the parameter-marker model may make that harder to express, and the doc admits v0.0.4 doesn't yet.

Mandatory move on non-Copy value params seems redundant — non-Copy types can't be silently copied anyway, so the keyword is enforcing something the type system already knows. The "footgun" example implies the default for non-Copy params is borrow-not-move, which makes every take-ownership API carry the keyword. Defensible, but a lot of moves.

