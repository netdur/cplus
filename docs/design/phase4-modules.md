# Phase 4 — Modules

> Status: draft (redesigned post-discussion, 2026-05-11)
> Scope: explicit path-string imports with mandatory `as` prefix, `pub` visibility, multi-file projects rooted at `Cplus.toml`. Compilation model is whole-program (all reachable `.cplus` files → one LLVM IR module) for the initial Phase 4 landing; per-module separate compilation is later polish.
> Out of scope: external dependencies / package manager (Phase 9), re-exports, finer visibility scoping (`pub(crate)` / `pub(super)`), conditional compilation (`cfg`).

## 1. Problem

C+ has been single-file through Phase 3. Real programs span many files; the language needs:
- A way to split definitions across files
- A way to import items from one file into another
- A privacy story so libraries can hide internals
- A project entry point and manifest so the compiler knows what to build

Three Phase-1 commitments shape this:
- §2.5: "Real module system. No preprocessor. One file = one module. Explicit imports. Declarations and definitions live together. No headers."
- §2.8 "won't add": "No glob imports as a default."
- §2.8c "verbosity acceptable": prefer explicit + local over short + magic.

A first design draft used filesystem-derived namespaces (`src/foo/bar.cplus` = module `foo::bar`). That model was replaced after discussion: paths in source come from explicit `import "..." as ...` declarations, not directory structure. Rationale and locked-in decisions are recorded below; the resolved-by-redesign questions are summarized in plan.md §11.

## 2. Decision — surface

### 2.1 File ↔ module mapping

**No filesystem-derived namespaces.** Directories are organizational only. `src/foo/bar.cplus` does NOT automatically define module `foo::bar`. Each `.cplus` file is its own compilation unit; how its items are referenced from elsewhere is determined entirely by the `import` declaration in the importing file.

This means:
- You can reorganize directories without breaking imports as long as the relative paths are updated.
- Two files with the same basename in different directories don't collide as modules — they collide only if a single file tries to import both under the same prefix.
- There is no "root module name" to bikeshed — the entry binary's file is just the entry binary's file.

### 2.2 `import` syntax

```cp
import "math.cplus" as math;
import "util/strings.cplus" as strings;
import "../shared/types.cplus" as types;
```

- The `as NAME` clause is **mandatory** on every import. There is no unprefixed form. Every cross-file reference therefore announces its origin at the use site.
- Imports are scoped to the file. An `import` at the top of `main.cplus` does not bleed into `math.cplus`.
- Paths are resolved **relative to the importing file's directory**, not relative to `src/` or the project root. `import "../util/x.cplus"` walks up one directory from the importing file.
- The string must end in `.cplus`. Other extensions or directory-only paths are rejected.
- Forward order doesn't matter — the resolver collects all imports across all reached files before resolving names.

There is no item-level import / no narrowing form. To use `square` from `math.cplus`, write `math::square`. One mechanism, no escape hatches. Rationale: §2.8c (verbosity acceptable) and §5.8 (locality of reasoning) — every name carries its origin.

### 2.3 Visibility — `pub` keyword

```cp
struct InternalState { ... }      // private to this file
pub struct PublicConfig { ... }   // exported

fn helper() -> i32 { ... }        // private
pub fn parse_file(path: ...) { ... }  // exported

pub enum Color { Red, Green, Blue }   // entire enum public

impl PublicConfig {
    fn private_helper(self) -> i32 { ... }    // private method
    pub fn validate(self) -> bool { ... }     // public method
}
```

- **Default is private.** Per §2.8c verbosity is acceptable — typing `pub` on each exported item is the price of safety against accidental exposure.
- `pub` applies to: `fn`, `struct`, `enum`, `impl` methods, top-level `let` constants (future).
- For `struct`: `pub` on the type makes the type-name visible; individual fields are *still private* unless marked `pub field_name: T`. "Expose the type, hide the fields" stays the default.
- For `enum`: `pub` on the enum makes the enum-name AND all variants public. Variants don't get individual `pub`. If variant-level privacy is needed, split into two enums.
- For `impl`: each method has its own `pub` setting. Methods default to private even on a `pub` type — same logic as struct fields.
- Same-file references see everything regardless of `pub`. `pub` gates cross-file access only.

### 2.4 Cross-file references — namespace access through the prefix

Once a file is imported under a prefix, its items are reached with `::`:

```cp
import "math.cplus" as math;

fn main() -> i32 {
    let r: i32 = math::square(7);
    let p: math::Point = math::Point::new(3, 4);
    println(r);
    return 0;
}
```

- `prefix::Item` reaches a top-level item (`fn`, `struct`, `enum`).
- `prefix::Type::method` reaches an associated function (Phase 2C convention preserved).
- `prefix::Enum::Variant` reaches an enum variant.

`::` for type/namespace and `.` for instance access (§2.8a) is preserved exactly. The import prefix sits to the left of the leftmost `::` and is itself part of namespace-flavored access. No Dart-style `math.square(7)`.

### 2.5 Cycles

**Cycles are forbidden** (E0404). A cycle exists when, starting from any file, the import graph (edges follow `import` statements after path resolution) loops back to that file. The driver detects cycles during the import-walk phase and reports the offending chain.

Within a single file, type/function definitions cross-reference freely; mutual recursion has worked since Phase 1.

### 2.6 Compilation model

**Phase 4 minimum: whole-program compilation.** The driver walks the import graph starting from the entry binary, parses every reached `.cplus` file, builds one combined AST, and runs the existing pipeline:

1. Load `Cplus.toml`, identify entry file.
2. Parse entry; collect its `import` declarations.
3. Resolve each import path (relative-to-importing-file) → physical file path.
4. Recursively parse each reached file (cycle-checking as we go).
5. Run sema across the combined symbol table, scoping imports per file.
6. Emit one LLVM IR module containing all functions.
7. Link as before.

Dependency tracking is via this import-chain walk from the entry, not a `src/` tree walk. Files that no one imports are not compiled.

Separate compilation (per-file `.o`, LTO link) is later polish — it's a performance optimization and doesn't change semantics. The linker we invoke (clang) supports ThinLTO; when we get there, it's a flag flip.

## 3. Filesystem layout

Directories are purely organizational. A typical layout:

```
my_project/
├── Cplus.toml              # manifest (§5)
├── src/
│   ├── main.cplus          # binary entry (per manifest)
│   ├── math.cplus
│   └── util/
│       ├── strings.cplus
│       └── numbers.cplus
└── target/                 # build artifacts (cpc creates)
    └── debug/myapp
```

- The `src/` convention is a habit, not a rule — the manifest's binary entry path can point anywhere.
- A directory does not itself become a namespace. `src/util/` produces no `util::` prefix automatically; if `main.cplus` wants `strings::trim`, it writes `import "util/strings.cplus" as strings;`.
- Files outside the reachable import graph are not part of the build. `bench/`, `docs/`, etc. are ignored unless something imports them.

## 4. Diagnostics (AI recovery)

Per §5.9, every new error must be precise and locally repairable.

| Code | Meaning | Suggestion shape |
|---|---|---|
| E0401 | imported file not found at given path | "no file at `<resolved-absolute-path>`; did you mean `<closest-existing-file>`?" |
| E0402 | unknown item accessed through prefix | "module imported as `math` defines: [list of public items]" |
| E0403 | private item accessed from outside its file | "item is private; mark `pub` on its declaration in `<file>` to export" |
| E0404 | cyclic import dependency | "cycle: `a.cplus` imports `b.cplus`, `b.cplus` imports `c.cplus`, `c.cplus` imports `a.cplus`" |
| E0405 | duplicate `as` prefix in same file | "prefix `math` was already used at line N; choose a distinct name" |
| E0406 | malformed `Cplus.toml` | "missing `<field>`" or "expected `<type>`" |
| E0407 | binary entry path in manifest doesn't exist | "no file at `<resolved-path>`" |
| E0408 | `impl` for type declared in a different file | "methods must live in the same file as the type; move this `impl` to `<file>`" |
| E0409 | reserved |  |
| E0410 | reserved |  |

The diagnostic JSON shape (§5.2) already supports `(span, replacement)` suggestions, so most of these can produce machine-applicable fixes.

## 5. `Cplus.toml` — minimal schema

```toml
[package]
name    = "myapp"          # project name; matches binary name by default
version = "0.1.0"          # semver
edition = "2026"           # reserved (§5.7 in plan.md); only "2026" valid

[[bin]]
name = "myapp"             # output binary name
path = "src/main.cplus"    # entry; defaults to `src/main.cplus` if omitted
```

For Phase 4 minimum, only `[package]` is required. If `[[bin]]` is omitted, the defaults are `name = package.name`, `path = "src/main.cplus"`.

Future extensions (not in Phase 4 minimum, but the schema reserves space):
- `[[bin]]` for additional binaries
- `[[lib]]` for library targets
- `[dependencies]` for external crates (Phase 9 with the package manager)
- `[features]` for conditional compilation flags
- `[build]` for custom build scripts

Parser: `toml` crate from crates.io. Validates the minimum schema; unknown fields produce warnings (not errors) so forward compatibility is easy.

## 6. New error codes summary

| Code | Meaning |
|---|---|
| E0401 | imported file not found at given path |
| E0402 | unknown item accessed through prefix |
| E0403 | private item accessed from outside its file |
| E0404 | cyclic import dependency |
| E0405 | duplicate `as` prefix in same file |
| E0406 | malformed `Cplus.toml` |
| E0407 | binary entry path in manifest doesn't exist |
| E0408 | `impl` for type declared in a different file |
| E0409 | reserved |
| E0410 | reserved |

These start at E0401 to leave a gap above E0346 (last Phase-3 error).

## 7. Sample project

`docs/examples/projects/hello_mods/` (the first multi-file sample — moves the docs/examples layout from flat `.cplus` files to small per-project directories):

```
docs/examples/projects/hello_mods/
├── Cplus.toml
└── src/
    ├── main.cplus
    └── math.cplus
```

`Cplus.toml`:
```toml
[package]
name    = "hello_mods"
version = "0.1.0"
edition = "2026"
```

`src/math.cplus`:
```cp
pub fn square(n: i32) -> i32 {
    return n * n;
}

fn private_helper() -> i32 {     // not pub — invisible outside this file
    return 42;
}
```

`src/main.cplus`:
```cp
import "math.cplus" as math;

fn main() -> i32 {
    let r: i32 = math::square(7);
    println(r);                  // 49
    return 0;
}
```

Build: `cpc build` (no args — uses `Cplus.toml` in cwd) → `target/debug/hello_mods`.
Run: `./target/debug/hello_mods` → `49`.

## 8. Implementation sketch

Slice split (status as of 2026-05-11):

1. **Slice 4A ✅ done — Manifest loading + dependency-walking driver.** Reads `Cplus.toml`, parses the entry binary's file, recursively loads every imported file, builds the combined AST. Cycle detection (E0404) landed here too. Driver gained the no-file-arg `cpc build` mode.
2. **Slice 4B ✅ done — `pub` visibility + cross-file access checks.** Parser accepts `pub` on `fn` / `struct` / `enum` / methods / struct fields. Resolver builds per-file pub_items / pub_methods, fires E0403 (Function / Struct / Enum / Method variants) at each cross-file qualification site. Same-file references ignore `pub`. Enum variants inherit the enum's `pub`.
3. **Slice 4C — Diagnostics polish + per-file source threading.** Round out the E0401–E0410 suite with suggestion text; thread per-file source through sema so cross-file errors render proper line/col; sema-side enforcement of struct-field `pub` (parsed in 4B, not yet enforced); parser polish for cross-file struct literals (`prefix::Type { ... }`).

Slices 4D+ go to `cpc fmt`, LSP foundations (each its own design note before implementation).

### 8.1 AST / sema changes

- New `ImportDecl { path: String, as_name: Symbol, span: Span }` AST node, parsed at file top alongside items.
- New `FileUnit` in sema: file path, imports (`as_name → file_id` after resolution), items, items-marked-pub-set.
- `Item` gains a `pub: bool` flag.
- `Struct.fields` gains a per-field `pub: bool` (default false).
- Name resolution becomes layered per file: same-file items first, then the import-prefix table, then primitives.
- Cross-file references through `prefix::Item` resolve by looking up `prefix` in the importing file's import table → target file → item lookup → public-bit check.

### 8.2 Codegen changes

Modest. Functions and types get a file-derived mangled prefix to avoid collisions across files. Since there's no namespace tree to walk, we mangle by a stable file identifier — initial implementation: canonicalized-relative-path with `/` → `.` and stripping the `.cplus` suffix.

- `math.cplus`'s `square` → `@math.square`
- `util/strings.cplus`'s `trim_len` → `@util.strings.trim_len`
- `parser/lexer.cplus`'s `Token` → `%parser.lexer.Token`

The entry binary's `main` stays un-prefixed (`@main`) so the linker entry point works without special-casing.

The dot separator already matches the `Type.method` convention from slice 2C. C+ identifiers don't allow dots, so there's no collision with user names.

### 8.3 Driver changes

- New `cpc build` mode (no file arg): reads `Cplus.toml`, builds the project. Output to `target/debug/` or `target/release/`.
- Existing `cpc FILE [-o OUT]` keeps working for single-file scripts (Phase 1/2/3 samples).
- New `cpc --emit-ll` for multi-file: concatenated IR from all reached files.

### 8.4 Test strategy

- E2E tests point at `docs/examples/projects/<name>/` rather than single files when exercising multi-file behavior.
- The single-file samples from Phases 1–3 stay; they continue to be compiled with `cpc FILE` for backwards compat.
- A new e2e test asserts that a multi-file project builds and runs.

## 9. Interactions

### 9.1 Phase 1 / 2 / 3 features

All preserve their behavior within a file. Cross-file references go through the import prefix; the existing path machinery (Phase 2A two-segment paths) extends to handle `prefix::Item` and `prefix::Type::method`.

### 9.2 `cpc fmt`

The formatter (separate design note) handles `import` declarations: one per line at the file top, sorted lexicographically by quoted path. Doesn't block this note.

### 9.3 LSP

Cross-file go-to-definition + workspace symbol search become real once multi-file projects exist. The compiler-as-library `cplus-core` already separates parse / resolve / type-check phases; the LSP just calls them. Resolving a `prefix::Item` reference walks the importing file's import table → target file → item — straightforward to wire up.

### 9.4 `extern fn` interop

Unchanged. `extern fn` declarations live in whatever file the user writes them in. The `pub` rule applies — a non-public `extern fn` is only callable from inside its declaring file.

### 9.5 `impl` blocks

**Locked-in rule: an `impl T` block must live in the same file as the type `T` is declared.** This is Rust's "orphan rule" simplified — without traits, there's no reason to allow remote `impl`s, and the rule is easier to remember + easier to check. If a user writes `impl ForeignType { ... }` for a type imported from another file, sema emits E0408 pointing at the original declaration site.

When traits land (Phase 7), this rule relaxes to "the trait OR the type must be local."

## 10. Open questions

- [ ] What happens when two `import` declarations in the same file resolve to the same physical file (e.g., via different relative paths or symlinks)? Options: collapse silently, warn, or require a single canonical form. Lean: warn — silent collapse hides redundancy, hard-error is too strict.
- [ ] `Cplus.toml` schema additions for libraries (`[[lib]]`) — defer to when libraries become real.
- [ ] Shadowing a prefix with a local name in an inner scope. Lean: reject (E0405-adjacent — prefix is a file-scope binding). Confirm at implementation.
- [ ] Self-import: a file imports itself, directly or via a one-hop cycle. Lean: treat as E0404. Confirm.
