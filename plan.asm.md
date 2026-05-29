# C+ Inline Assembly Plan

This document records the codebase findings and the implementation roadmap for **inline assembly** in the C+ compiler (`cpc`). It is the result of an architecture pass over the lexer → parser → sema → codegen pipeline (as of v0.0.12), scoping what it would take to add a Rust-/GCC-style `asm!` to the language.

---

## 1. Terminology: two unrelated things called "assembly"

| Meaning | Status | Mechanism |
| :--- | :--- | :--- |
| **Assembly *output*** (`cpc → .s`) | ✅ **Already shipped** | `--emit-asm` routes LLVM IR through `clang -S` ([cpc/src/main.rs:283](file:///Users/adel/Workspace/C+/cpc/src/main.rs#L283)). Build system also accepts hand-written `.s` compiled out-of-band ([cplus-core/src/manifest.rs:88](file:///Users/adel/Workspace/C+/cplus-core/src/manifest.rs#L88)). |
| **Inline assembly in C+ source** (an `asm!`) | ❌ **Does not exist** | No `call ... asm sideeffect` anywhere in codegen. This document scopes adding it. |

Everything below concerns the second row.

---

## 2. Architectural Findings

The compiler is a **text LLVM-IR emitter** ([cplus-core/src/codegen.rs:1](file:///Users/adel/Workspace/C+/cplus-core/src/codegen.rs#L1)): C+ → LLVM IR text → `clang` → object/assembly. Because LLVM natively supports inline asm (`call <ty> asm sideeffect "<template>", "<constraints>"(<ins>)`), the backend mechanics are essentially free — LLVM performs register allocation and constraint solving. The cost is in the **language surface** and **validation**, not the IR.

### 2.1 The plumbing already exists

C+ has a unified compiler-intrinsic surface: `#name::[T](args) -> RetTy`. Key consequence: **inline asm needs zero lexer/parser changes for the basic form** — `#asm(...)` already parses.

* **Parser** lowers *any* `#name(...)` into a generic `ExprKind::Intrinsic { name, type_args, args, ret_ty }` ([cplus-core/src/parser.rs:2443](file:///Users/adel/Workspace/C+/cplus-core/src/parser.rs#L2443)).
* **Sema** dispatches by name in a single `match` ([cplus-core/src/sema.rs:4166](file:///Users/adel/Workspace/C+/cplus-core/src/sema.rs#L4166)) → one `check_intrinsic_*` fn per intrinsic.
* **Codegen** dispatches by name in a single `match` ([cplus-core/src/codegen.rs:8428](file:///Users/adel/Workspace/C+/cplus-core/src/codegen.rs#L8428)) → one `gen_intrinsic_*` fn per intrinsic.

### 2.2 Direct precedent: `#cpu_relax()`

The closest existing intrinsic is `#cpu_relax()` ([cplus-core/src/codegen.rs:8586](file:///Users/adel/Workspace/C+/cplus-core/src/codegen.rs#L8586)): per-arch lowering via `cfg!(target_arch)` (aarch64/x86_64 branches), unsafe-aware, fully tested. An inline-asm intrinsic follows the identical shape with a richer payload.

### 2.3 `unsafe` gating is reusable as-is

Inline asm must require an `unsafe` block. The machinery already exists: `unsafe_depth` + `E0801` ([cplus-core/src/sema.rs:4283](file:///Users/adel/Workspace/C+/cplus-core/src/sema.rs#L4283)). The `check_intrinsic_asm` fn reuses it verbatim.

---

## 3. Implementation Tiers

### Tier 0 — Assembly output
✅ Done (`--emit-asm`). No work.

### Tier 1 — Bare template asm, no operands
**Surface:** `#asm("dmb ish")` — template string only.
**Emit:** `call void asm sideeffect "<template>", ""()`.
**Touch points:**
* `check_intrinsic_asm` in sema — string-literal-only validation, unsafe-gate.
* `gen_intrinsic_asm` in codegen — emit the `call ... asm sideeffect`.
* Tests (unit + e2e + negative).

**Estimate: ~1 day, ~150–250 LOC + tests.** Useful for fences / barriers / hints. Cannot read or write C+ values.

### Tier 2 — Real inline asm with operands & clobbers  *(the actual feature)*
This is where the cost lives, and it is a **design** cost more than a coding cost. The current `#name(expr, expr)` arg shape cannot express an operand model.

**Recommended surface** (consistent with C+'s "one way to do a thing" / explicit principles — avoid GCC's positional `%0` soup):
```
unsafe {
    #asm("add {out}, {a}, {b}",
         out = "=r"(dst),
         a   = "r"(x),
         b   = "r"(y),
         clobbers = ["memory"])
}
```

**Touch points:**
* **Parser** ([cplus-core/src/parser.rs](file:///Users/adel/Workspace/C+/cplus-core/src/parser.rs)) — operand specifiers (`name = "constraint"(expr)`) are not plain expressions, so a small extension to the intrinsic-arg path is required. *This is the one place the feature is not purely additive on the generic intrinsic mechanism.*
* **Sema** ([cplus-core/src/sema.rs](file:///Users/adel/Workspace/C+/cplus-core/src/sema.rs)) — validate template placeholders against declared operands; map C+ operand types (ints / ptrs / floats only) to LLVM constraint codes; reject unsupported types; thread output operands through as **places** (lvalues, like assignment).
* **Codegen** ([cplus-core/src/codegen.rs](file:///Users/adel/Workspace/C+/cplus-core/src/codegen.rs)) — build the constraint string (`"=r,r,r,~{memory}"`), order operands, emit `%0 = call <ty> asm sideeffect "...", "..."(<ins>)`, then store results back into output places. Multiple outputs → anonymous-struct return that gets destructured.
* **Borrowck** ([cplus-core/src/borrowck.rs](file:///Users/adel/Workspace/C+/cplus-core/src/borrowck.rs)) — outputs initialize/mutate places; inputs are reads. Needs handling so it does not false-positive.

**Estimate: ~1.5–2.5 weeks, ~800–1,400 LOC** across parser/sema/codegen/borrowck, plus the mandated full test matrix (unit per stage + e2e in [cpc/tests/e2e.rs](file:///Users/adel/Workspace/C+/cpc/tests/e2e.rs) + negative/diagnostic tests).

### Tier 3 — `#[naked]` functions / module-level global asm  *(optional)*
Separate feature: item-level attribute handling + a module-scope asm emit. **~2–4 days** if wanted.

---

## 4. Risks & Decisions

* **Operand syntax is the real fork.** It touches the language surface, so it needs an explicit design decision (consistent with the "no several ways to do the same thing" principle) **before** any Tier-2 code is written.
* **Portability semantics.** Inline asm is inherently arch-specific; the language has so far kept arch behind intrinsics (`cpu_relax`). Decide a policy: require `cfg(target_arch)` guards, or document that `#asm` is non-portable by construction.
* **Constraint-string correctness** is the classic footgun (a missing clobber → silent miscompile). Warrants an adversarial test pass.

---

## 5. Recommendation

1. **Ship Tier 1 now** (~1 day) — immediately useful for barriers/hints and validates the end-to-end path through the intrinsic dispatch.
2. **Design Tier 2's operand syntax explicitly** (short design doc / review) before building it.
3. Total for a production-grade inline-asm feature: **~2–3 weeks** including the project's full test discipline.
