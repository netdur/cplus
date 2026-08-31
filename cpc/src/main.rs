use cplus_core::codegen::BuildMode;
use cplus_core::diagnostics::{self as diag, Diagnostic, LineMap, Severity};
use cplus_core::target::{self, Handoff, TargetSpec};
use cplus_core::{
    attrs, borrowck, codegen, doctest, fmt as cpfmt, lexer, lower, manifest, monomorphize, parser,
    resolver, sema,
};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::sync::OnceLock;
use tempfile::NamedTempFile;

mod mcp;

const HELLO_LL: &str = include_str!("hello.ll");

/// v0.0.3 Phase 2 (CWE-377 hardening): create a secure temp file with the
/// given content and a stable suffix so clang sees `cpc-<rand>.ll` (etc.)
/// rather than the predictable `cpc-<pid>.ll` shape. The returned
/// `NamedTempFile` cleans up on drop — callers don't `fs::remove_file`.
///
/// The previous shape (`env::temp_dir().join(format!("cpc-{pid}.ll"))`)
/// allowed a local attacker to pre-create the path as a symlink to a
/// victim file; running `cpc` would then overwrite the attacker's chosen
/// target with the LLVM IR.
fn make_temp_file(prefix: &str, suffix: &str, content: &[u8]) -> std::io::Result<NamedTempFile> {
    let mut handle = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile()?;
    handle.write_all(content)?;
    handle.flush()?;
    Ok(handle)
}

const USAGE: &str = "\
cpc — C+ compiler

usage:
  cpc skill                         print the C+ language reference for an LLM/agent —
                                    a dense, self-contained guide to writing C+, embedded
                                    in this binary (version-matched, no network). If you
                                    are an agent about to write or edit C+, read this first.
                                    Inside a project it also prints the SKILL.md of every
                                    dependency that ships one (`--lang-only` to suppress).
  cpc explain [CODE]                explain a diagnostic (e.g. `cpc explain E0502`): cause,
                                    fix, example. No CODE (or --list): list every code.
  cpc FILE [-o OUT]                 compile single-file FILE.cplus to a binary (default OUT: ./a.out)
  cpc build [-o OUT]                multi-file build: reads ./Cplus.toml, walks imports
  cpc check [FILE]                  fast feedback loop: parse + sema + borrowck.
                                    With no FILE: whole-project check via Cplus.toml,
                                    enforcing any [profile.realtime] gate; no codegen.
                                    With a FILE: also runs codegen and discards the IR,
                                    so a codegen-stage FAULT is caught here too. It does
                                    NOT assemble: clang never sees the IR, so invalid IR
                                    passes `check` and fails only in a real build. When
                                    the question is whether it compiles, build it.
  cpc headers                       generate `lib/include/` from `src/` for the package in
                                    the current directory: concrete modules become
                                    declarations (`fn f(...) -> T;`), modules declaring
                                    generics are copied verbatim (a generic has no object
                                    code until the consumer instantiates it)
  cpc doc FILE                      extract public items + `///` docs from FILE, emit
                                    Markdown to ./target/doc/<basename>.md
  cpc test [FILE] [--json]          discover + run `#[test]` functions. Single-file mode
                                    if FILE is given; project mode (reads ./Cplus.toml)
                                    otherwise. Honors the build flags below, so
                                    `--release` runs the suite at -O3 and `--asan`/`--ubsan`
                                    instrument the test binary.
                                    `--json` emits one JSON object per test
                                    plus a final summary line.
  cpc fmt FILE|DIR [...]            format C+ source. By default: rewrites in place.
                                    flags: --check (no write, exit non-zero on diff)
                                           --emit  (print to stdout, leave file alone)
                                           --stdin (read source from stdin, write to stdout)
  cpc lsp [--log PATH]              start the C+ language server on stdin/stdout
                                    (delegates to the `cpc-lsp` binary on PATH or
                                    next to this binary)
  cpc [-o OUT]                      with no FILE: emit the Phase-0 hello-world demo

build flags (apply to `cpc FILE` and `cpc build`):
  --release                         -O3, no overflow checks on `+ - *` (default: debug, checked)
  --debug                           -O0 with overflow traps (the default)
  --fp-contract=off|on|fast         float contraction policy; `off` keeps `a*b+c` as
                                    fmul+fadd for bit-identical-to-C output (default: on).
                                    Place before --emit-ll/--emit-asm/--emit-obj FILE.
  --warn-deps                       report warnings from dependencies too
                                    (default: only this project's own `src/`)
  --timings                         print build cost to stderr: per phase
                                    (resolve+sema+borrowck / codegen / prune / clang+link)
                                    then per package (each prebuilt dependency,
                                    the project itself, and the unaccounted rest)
  -g | --debug-info                 emit DWARF debug metadata + pass -g to clang
  --asan | --ubsan | --tsan | --msan
                                    enable the matching LLVM sanitizer (asan/tsan/msan are
                                    mutually exclusive; ubsan composes with any)
  --target NAME                     compile for a named target: host (default), ios-arm64,
                                    ios-arm64-simulator, android-arm64, esp32-xtensa,
                                    esp32c3-riscv32.
                                    External-builder targets stop at object emission — the
                                    external build system (Xcode, the Android NDK build,
                                    ESP-IDF) owns the final link. Combine with --emit-obj /
                                    --emit-ll / --emit-asm, or `cpc build` of a project (an
                                    app entry or a library becomes lib<name>.a + a C
                                    header). android-arm64 uses the NDK's clang
                                    ($ANDROID_NDK_HOME, or the SDK's newest ndk/; r28.2+);
                                    esp32-xtensa uses esp-clang ($CPC_ESP_CLANG, or
                                    ~/.espressif via `idf_tools.py install esp-clang`).
                                    esp32-xtensa is 32-bit: usize/isize/pointers are 4
                                    bytes; heap types (Text, Vec) are not yet supported
                                    there. Place before --emit-ll/--emit-asm FILE.
  --min-os VERSION                  override the OS floor baked into a versioned target
                                    triple: 13.0 for the ios targets, API 24 for
                                    android-arm64. Place after --target.

debug / introspection (single-file):
  cpc --emit-ir                     print the frozen Phase-0 LLVM IR to stdout
  cpc --tokens FILE                 lex FILE and print the token stream
  cpc --ast FILE                    lex+parse FILE and print the AST
  cpc --emit-ll FILE                lex+parse+sema+codegen FILE and print the .ll IR
  cpc --emit-ll-opt FILE            post-optimization IR (cpc → clang -S -emit-llvm
                                    at the build mode's -O level; see --release / --debug)
  cpc --emit-asm FILE               native assembly (cpc → clang -S at the build mode's -O level)
  cpc --emit-obj FILE -o OUT.o      relocatable object (cpc → clang -c). Used by the
                                    library-build pipeline; -o OUT.o is required.
  cpc --emit-header FILE            C header for every C-ABI-representable `export` item
                                    in FILE. Prints to stdout; redirect with `> out.h`.
  cpc --emit-ll-project             multi-file: print the merged IR to stdout (uses ./Cplus.toml)
  cpc build --print-link-args       print the link line the DEPENDENCIES contribute, one arg per
                                    line, and build nothing. What a cross target's consumer (Xcode,
                                    Gradle, ESP-IDF) must add beside the app's own archive.

other:
  --diagnostics=MODE                diagnostics output: human (default) | short | json
  --realtime-report[=json]          whole-project real-time contract digest (reads
                                    ./Cplus.toml + [profile.realtime]); prints the profile,
                                    functions-under-contract count, and E0901/E0906/E0907/
                                    E0908 violations grouped by contract. Exits non-zero on any.
  -V | --version                    print compiler version
  -h | --help                       show this message
";

/// Phase 11 polish (2026-05-14): subcommand-aware `--help`. Once a
/// subcommand has been seen on the CLI, `--help` returns just the
/// relevant slice instead of the full usage dump.
fn subcommand_help(sub: Option<Subcommand>) -> &'static str {
    match sub {
        None => USAGE,
        Some(Subcommand::Headers) => {
            "\
cpc headers

Generate `lib/include/` from `src/` for the package in the current directory.

The header is what a consumer compiles against when the package ships as a
binary. Each module in `src/` produces one file in `lib/include/`:

  - a module with no generics has its function and method bodies replaced by
    `;` — the implementation lives in the bundled archive named by
    `[link].bundled`;
  - a module that declares ANY generic is copied verbatim, because a generic
    has no object code until the consumer instantiates it, so its body has to
    travel with the package (the same reason a C++ template lives in a header).

Everything else — imports, `struct` layouts, `const`s, comments — is preserved
byte for byte, so the header cannot drift from the source it came from.
"
        }
        Some(Subcommand::Build) => {
            "\
cpc build [-o OUT] [--release] [-g] [--asan|--ubsan|--tsan|--msan]

Multi-file build. Reads ./Cplus.toml at the current directory, walks the
declared imports, lowers + sema + borrowck + codegen the whole project,
and writes the linked binary to `target/{debug,release}/<name>` (or to
OUT if `-o` is given). The manifest names the project; the entry file
must define `fn main() -> i32`.
"
        }
        Some(Subcommand::Check) => {
            "\
cpc check FILE

Parse + sema + borrowck FILE. No codegen, no clang, no binary. Same
diagnostics you'd get from `cpc FILE -o BIN`, but faster — the editor /
LSP / pre-commit-hook use case. Exits 0 if clean, 1 on any error.
"
        }
        Some(Subcommand::Doc) => {
            "\
cpc doc FILE

Extract every public item with a preceding `///` doc block from FILE
and emit Markdown to `./target/doc/<basename>.md`. Each item gets a
section with its signature, a `defined at line N` link, and the doc
prose. Fenced code blocks inside `///` are preserved as Markdown code
blocks — the same blocks `cpc test` runs as doctests.

Private items (and public items without docs) are skipped to keep the
reference focused on the project's stable surface.
"
        }
        Some(Subcommand::Test) => {
            "\
cpc test [FILE] [--json]

Discover and run every `#[test]` function in the project (or in FILE if
given). Each test compiles into the test driver and runs sequentially.
Doctests embedded in `///` comments are extracted into synthesized
`#[test]` functions before running. With `--json`, emits one JSON object
per test plus a final summary line — for tool consumption.
"
        }
        Some(Subcommand::Fmt) => {
            "\
cpc fmt FILE|DIR [...]

Format C+ source. By default rewrites each file in place. Flags:
  --check    don't write; exit 1 if any file would change (CI mode)
  --emit     print formatted output to stdout, leave file alone
  --stdin    read source from stdin, write to stdout, no file arg

Multiple paths accepted; directories are walked recursively for
`.cplus` files.
"
        }
        Some(Subcommand::Lsp) => {
            "\
cpc lsp [--log PATH]

Start the C+ language server on stdin/stdout (delegates to the
`cpc-lsp` binary on PATH or next to this binary). All args after `lsp`
are forwarded.
"
        }
        Some(Subcommand::EmitLlProject) => {
            "\
cpc --emit-ll-project

Multi-file: run the build pipeline as `cpc build` would, but print the
merged LLVM IR to stdout instead of invoking clang. Uses ./Cplus.toml.
"
        }
        Some(Subcommand::PrintLinkArgs) => {
            "\
cpc build --print-link-args [--target NAME] [--release]

Print the link line this project's DEPENDENCIES contribute, one argument
per line, and build nothing else. Reads ./Cplus.toml.

For a cross target (`--target ios-arm64-simulator`, android, esp32) cpc
emits ONE archive: the entry package and the generics it instantiated.
Every dependency's object code is in its own prebuilt slice beside the
package, and the external build system — Xcode, Gradle, ESP-IDF — has to
name all of them. This is that list, resolved the same way the compiler
resolves it, so nothing downstream has to re-derive it and drift.

Dependency slices are brought up to date first, so every path printed is
a file that exists and is current.

  cpc build --target ios-arm64-simulator
  cpc build --target ios-arm64-simulator --print-link-args
"
        }
        Some(Subcommand::Graph) => {
            "\
cpc graph

Build the project's code knowledge graph and print it as JSON (nodes +
edges) on stdout. Reads ./Cplus.toml. The resolved index an agent or the
LSP queries by symbol instead of by grep.
"
        }
        Some(Subcommand::Query) => {
            "\
cpc query <kind> [args...]

Answer one code-graph query as JSON. Kinds: `def SYMBOL`, `members TYPE`,
`symbols [FILE]`, `refs SYMBOL`, `callers FN`, `callees FN`,
`call-hierarchy FN [--depth N]`, `context FN`, `type-at FILE:LINE:COL`,
`value-refs FILE:LINE:COL`, `scope-at FILE:LINE:COL`,
`complete FILE:LINE:COL`.
Reads ./Cplus.toml; exit code signals found / not-found.

`complete` is the composed one: it decides whether the caret is after a
`.`, a `::`, or neither, and answers with the ranked candidates for that
question. `scope-at` is its unqualified half on its own.

Both omit `#[test]` functions. Every other kind still finds them; in
`symbols` they carry `is_test: true`.

Every kind pays the whole-project graph build (~2s). An editor asking on a
keystroke wants `cpc mcp`, which builds once and answers from memory.
"
        }
        Some(Subcommand::Mcp) => {
            "\
cpc mcp

Resident MCP server over the code knowledge graph: builds the graph once
from ./Cplus.toml, then answers MCP tool calls over stdio (newline-
delimited JSON-RPC 2.0) until stdin closes. Point an MCP client at
`cpc mcp` to give an agent resolved, typed C+ navigation in place of grep.

The graph is live, not a snapshot: `did_change` hands the server an unsaved
buffer, `did_close` drops it, `reload` rebuilds from disk, and `graph_status`
says what the server is currently holding. A rebuild that fails to parse keeps
the last good graph and reports the error rather than going blind, so an editor
can keep querying through a half-typed line.
"
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DiagMode {
    Human,
    Short,
    Json,
}

/// Whose problems this invocation reports.
///
/// `cpc build` in a project prints diagnostics about THAT project. A warning
/// about a dependency is advice for that dependency's author: the reader did
/// not write the code, usually cannot edit it (a vendored copy is replaced on
/// the next sync, a generated header on the next prebuild), and gets the same
/// text on every build forever. stdlib's refcounted `Channel` is the standing
/// example — a W0002 that is correct, deliberate, and useless to everyone
/// except stdlib. Left on, that noise teaches people to skim past warnings,
/// which is the opposite of what a warning is for.
///
/// So dependency WARNINGS are dropped unless `--warn-deps` asks for them.
/// ERRORS are never dropped: an error anywhere stops the build, and the reader
/// has to see it even when the fix is upstream.
///
/// "The project" is `<root>/src/` — not `<root>`, because `cpc pm` copies
/// dependencies to `<root>/vendor/<name>`, which is inside the root but is
/// somebody else's code.
///
/// Set ONCE, from the top-level invocation. A dependency prebuilt as part of
/// this build is still a dependency: running `cpc build` in an app should not
/// print the UI toolkit's warnings just because the toolkit happened to need
/// recompiling on the way.
mod diagpolicy {
    use std::path::{Path, PathBuf};
    use std::sync::OnceLock;

    static OWN_SRC: OnceLock<PathBuf> = OnceLock::new();
    static WARN_DEPS: OnceLock<()> = OnceLock::new();

    /// Name the project whose diagnostics are the reader's own. First call
    /// wins — see the note on prebuilt dependencies above.
    pub fn set_project_root(root: &Path) {
        let src = root.join("src");
        let src = std::fs::canonicalize(&src).unwrap_or(src);
        let _ = OWN_SRC.set(src);
    }

    pub fn warn_about_dependencies() {
        let _ = WARN_DEPS.set(());
    }

    /// Should a warning about this file be withheld from the reader?
    ///
    /// Two ways to be somebody else's problem, one flag governing both, so
    /// `--warn-deps` means exactly "show me dependency diagnostics too":
    ///
    /// 1. A GENERATED HEADER (`<pkg>/lib/include/...`). Never the reader's,
    ///    even when it belongs to the project being built — it is rewritten by
    ///    the next prebuild, and its contents were already reported against
    ///    the real `src/` path.
    /// 2. Anything outside this project's own `src/`.
    ///
    /// False when no project has been named (single-file builds,
    /// `cpc check FILE`), so those keep reporting everything.
    pub fn suppress_warning(file: &Path) -> bool {
        if WARN_DEPS.get().is_some() {
            return false;
        }
        if crate::diag::is_generated_header(file) {
            return true;
        }
        let own = match OWN_SRC.get() {
            Some(o) => o,
            None => return false,
        };
        let f = std::fs::canonicalize(file).unwrap_or_else(|_| file.to_path_buf());
        !f.starts_with(own)
    }
}

fn emit_diag(d: &Diagnostic, mode: DiagMode, src: &str) {
    let line = match mode {
        DiagMode::Human => d.render_human(src),
        DiagMode::Short => d.render_short(),
        DiagMode::Json => d.to_json(),
    };
    eprintln!("{line}");
}

/// `emit_diag` for multi-module pipelines: snippets quote the file each
/// span names (via the loader's per-file source map), not the entry file.
///
/// A WARNING about a dependency's generated header is dropped here — see
/// `diag::is_generated_header`. It is advice for that package's author, it was
/// already delivered against the real `src/` path when that package was built,
/// and it names a file the reader must not edit. Errors are never dropped: an
/// error in a header is a genuine incompatibility between this build and the
/// slice it is compiling against, and the reader has to know even though the
/// fix is upstream.
fn emit_diag_multi(
    d: &Diagnostic,
    mode: DiagMode,
    src: &str,
    files: &std::collections::BTreeMap<String, (PathBuf, String)>,
) {
    if matches!(d.severity, Severity::Warning) && diagpolicy::suppress_warning(&d.primary.file) {
        return;
    }
    let line = match mode {
        DiagMode::Human => d.render_human_multi(src, files),
        DiagMode::Short => d.render_short(),
        DiagMode::Json => d.to_json(),
    };
    eprintln!("{line}");
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();

    // Unified subcommands that own the rest of argv and don't flow through the
    // build-style flag parser below. Dispatched before it so their arguments
    // (a project name, package-manager flags, `--write`) aren't misread as
    // build flags.
    match args.first().and_then(|a| a.to_str()) {
        Some("skill") => return run_skill(&args[1..]),
        Some("explain") => return run_explain(&args[1..]),
        Some("init") => return run_init(&args[1..]),
        Some("pm") => return run_pm(&args[1..]),
        _ => {}
    }

    let mut input: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut diag_mode = DiagMode::Human;
    let mut build_mode = BuildMode::Debug;
    // B-10: floating-point contraction policy. On by default (matches
    // clang's `-ffp-contract=on`): codegen contracts source-level `a*b+c`
    // into `llvm.fmuladd` and tags float arithmetic `contract`. Set off
    // with `--fp-contract=off` for output bit-identical to a C build
    // compiled with `-ffp-contract=off`.
    let mut fp_contract = true;
    // Phase 11 polish (2026-05-13): `-g` emits DWARF debug metadata.
    // v1 ships function-level DI only (DICompileUnit + DIFile +
    // DISubprogram). Per-instruction DILocation is a follow-up.
    let mut emit_debug_info = false;
    // Phase 11 polish (2026-05-13): sanitizer flags. LLVM's
    // instrumentation passes do the heavy lifting; cpc just plumbs
    // the `-fsanitize=...` flag through to clang.
    let mut sanitizers: Vec<&'static str> = Vec::new();
    // v0.0.21 multi-backend slice 1: the compilation target. Defaults to
    // the host spec, which reproduces pre-`--target` behavior byte-for-byte.
    // Resolved (and installed as codegen's active target) at flag-parse
    // time so the inline-dispatching `--emit-*` flags see it — hence the
    // "place --target first" rule shared with --fp-contract.
    let mut target_spec: TargetSpec = target::HOST;
    let mut subcommand: Option<Subcommand> = None;
    // Phase 5 Slice 5.A: deferred-dispatch input for `--emit-obj FILE`.
    // Order-independent with `-o OUT.o` because the FILE may appear before
    // or after the flag in the user's command line.
    let mut emit_obj_input: Option<PathBuf> = None;
    let mut fmt_opts = FmtOpts::default();
    let mut fmt_inputs: Vec<PathBuf> = Vec::new();
    let mut test_opts = TestOpts::default();
    let mut test_input: Option<PathBuf> = None;
    // `cpc query <kind> [args...]` — kind is the first positional after
    // `query`, the rest are its arguments (e.g. a symbol or file id).
    let mut query_kind: Option<String> = None;
    let mut query_args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].to_str();
        if let Some(s) = a {
            if let Some(rest) = s.strip_prefix("--diagnostics=") {
                diag_mode = match rest {
                    "human" => DiagMode::Human,
                    "short" => DiagMode::Short,
                    "json" => DiagMode::Json,
                    other => {
                        eprintln!("cpc: unknown --diagnostics value: {other:?} (expected human|short|json)");
                        return ExitCode::FAILURE;
                    }
                };
                i += 1;
                continue;
            }
            // v0.0.13 (topic C tail): `--realtime-report[=json]` — a whole-project
            // summary of the real-time contract analysis (reads Cplus.toml,
            // applies [profile.realtime], runs the front-end, aggregates the
            // E0901/E0906/E0907/E0908 violations). `cpc check` already gates the
            // build; this is the machine-readable digest deferred from Phase 8.
            if s == "--realtime-report" || s == "--realtime-report=human" {
                return run_realtime_report(false);
            }
            if s == "--realtime-report=json" {
                return run_realtime_report(true);
            }
        }
        match a {
            Some("-o") => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("cpc: -o requires an argument");
                    return ExitCode::FAILURE;
                };
                out = Some(PathBuf::from(v));
                i += 2;
            }
            Some("--emit-ir") => {
                print!("{HELLO_LL}");
                return ExitCode::SUCCESS;
            }
            Some("--tokens") => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("cpc: --tokens requires a FILE argument");
                    return ExitCode::FAILURE;
                };
                return dump_tokens(PathBuf::from(v), diag_mode);
            }
            Some("--ast") => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("cpc: --ast requires a FILE argument");
                    return ExitCode::FAILURE;
                };
                return dump_ast(PathBuf::from(v), diag_mode);
            }
            Some("--emit-ll") => {
                let Some(v) = args.get(i + 1) else {
                    eprintln!("cpc: --emit-ll requires a FILE argument");
                    return ExitCode::FAILURE;
                };
                return dump_ll(
                    PathBuf::from(v),
                    diag_mode,
                    build_mode,
                    fp_contract,
                    emit_debug_info,
                    &sanitizers,
                );
            }
            Some("--emit-ll-opt") => {
                // Slice 1G: post-pass LLVM IR. Runs clang with
                // `-S -emit-llvm` at the build_mode's optimization level so
                //1B's !range / 1C's !alias.scope can be inspected after
                // inlining + InstCombine.
                let Some(v) = args.get(i + 1) else {
                    eprintln!("cpc: --emit-ll-opt requires a FILE argument");
                    return ExitCode::FAILURE;
                };
                return dump_ll_or_asm(
                    PathBuf::from(v),
                    diag_mode,
                    build_mode,
                    fp_contract,
                    ClangOutputKind::LlvmIr,
                );
            }
            Some("--emit-asm") => {
                // Slice 1G: native assembly via `clang -S` at the
                // build_mode's optimization level. Used to verify hot-loop
                // bounds-check elision and other -O2 wins.
                let Some(v) = args.get(i + 1) else {
                    eprintln!("cpc: --emit-asm requires a FILE argument");
                    return ExitCode::FAILURE;
                };
                return dump_ll_or_asm(
                    PathBuf::from(v),
                    diag_mode,
                    build_mode,
                    fp_contract,
                    ClangOutputKind::Assembly,
                );
            }
            Some("--emit-header") => {
                // Phase 5 Slice 5.E: emit a C header (`.h`) declaring
                // every `export` item that's C-ABI representable. Prints to
                // stdout; redirect with `> out.h`.
                let Some(v) = args.get(i + 1) else {
                    eprintln!("cpc: --emit-header requires a FILE argument");
                    return ExitCode::FAILURE;
                };
                return dump_header(PathBuf::from(v), None, diag_mode);
            }
            Some("--emit-obj") => {
                // Phase 5 (v0.0.2 Slice 5.A): emit a relocatable object
                // (`.o`) file. Drives `clang -c <opt>` on the IR cpc
                // emits. The library-build path uses this to feed
                // `ar` / `ld -shared`. Requires `-o OUT.o`; FILE may
                // come either before or after the flag, so we defer
                // dispatch to end-of-args.
                let Some(v) = args.get(i + 1) else {
                    eprintln!("cpc: --emit-obj requires a FILE argument");
                    return ExitCode::FAILURE;
                };
                emit_obj_input = Some(PathBuf::from(v));
                i += 2;
                continue;
            }
            Some("--emit-ll-project") => {
                subcommand = Some(Subcommand::EmitLlProject);
                i += 1;
            }
            Some("--print-link-args") => {
                subcommand = Some(Subcommand::PrintLinkArgs);
                i += 1;
            }
            Some("--timings") => {
                timings::enable();
                i += 1;
            }
            Some("--warn-deps") => {
                diagpolicy::warn_about_dependencies();
                i += 1;
            }
            Some("--release") => {
                build_mode = BuildMode::Release;
                i += 1;
            }
            Some("--debug") => {
                build_mode = BuildMode::Debug;
                i += 1;
            }
            // B-10: `--fp-contract=off|on|fast`. `off` suppresses FMA
            // contraction (`a*b+c` stays fmul+fadd, float ops drop the
            // `contract` flag) for bit-identical-to-C float output;
            // `on`/`fast` keep the default fusing behavior.
            Some(s) if s.starts_with("--fp-contract=") => {
                match &s["--fp-contract=".len()..] {
                    "off" => fp_contract = false,
                    "on" | "fast" => fp_contract = true,
                    other => {
                        eprintln!("cpc: --fp-contract expects off|on|fast, got `{other}`");
                        return ExitCode::from(2);
                    }
                }
                i += 1;
            }
            Some("-g" | "--debug-info") => {
                emit_debug_info = true;
                i += 1;
            }
            Some("--asan") => {
                sanitizers.push("address");
                i += 1;
            }
            Some("--ubsan") => {
                sanitizers.push("undefined");
                i += 1;
            }
            Some("--tsan") => {
                sanitizers.push("thread");
                i += 1;
            }
            Some("--msan") => {
                sanitizers.push("memory");
                i += 1;
            }
            // v0.0.21 multi-backend slice 1: `--target NAME` / `--target=NAME`.
            // Resolves a named target and installs it as codegen's active
            // target immediately, so the `--emit-*` flags (which dispatch
            // inline during this loop) pick it up. An unknown name is a hard
            // error listing the supported set.
            Some("--target") => {
                let Some(v) = args.get(i + 1).and_then(|v| v.to_str()) else {
                    eprintln!(
                        "cpc: --target requires a NAME argument (supported: {})",
                        target::supported_names()
                    );
                    return ExitCode::FAILURE;
                };
                let Some(spec) = TargetSpec::from_name(v) else {
                    eprintln!(
                        "cpc: unknown target `{v}` (supported: {})",
                        target::supported_names()
                    );
                    return ExitCode::FAILURE;
                };
                target_spec = spec;
                target::set_active_target(spec);
                i += 2;
            }
            // v0.0.22: `--min-os VERSION` — override the OS version baked
            // into a versioned target triple (ios 13.0 / android API 24).
            // Requires `--target` first so the version can be validated
            // against the selected target.
            Some("--min-os") => {
                let Some(v) = args.get(i + 1).and_then(|v| v.to_str()) else {
                    eprintln!("cpc: --min-os requires a VERSION argument (e.g. 15.0 for ios targets, 28 for android-arm64)");
                    return ExitCode::FAILURE;
                };
                if v.is_empty() || !v.chars().all(|c| c.is_ascii_digit() || c == '.') {
                    eprintln!("cpc: --min-os expects a dotted numeric version, got `{v}`");
                    return ExitCode::FAILURE;
                }
                if target_spec.min_os_default.is_none() {
                    eprintln!(
                        "cpc: --min-os applies to targets with a versioned triple (ios-arm64, ios-arm64-simulator, android-arm64); current target is `{}`",
                        target_spec.name
                    );
                    eprintln!("    place `--target NAME` before `--min-os VERSION`");
                    return ExitCode::FAILURE;
                }
                target::set_min_os_override(v.to_string());
                i += 2;
            }
            Some(s) if s.starts_with("--target=") => {
                let v = &s["--target=".len()..];
                let Some(spec) = TargetSpec::from_name(v) else {
                    eprintln!(
                        "cpc: unknown target `{v}` (supported: {})",
                        target::supported_names()
                    );
                    return ExitCode::FAILURE;
                };
                target_spec = spec;
                target::set_active_target(spec);
                i += 1;
            }
            Some("-h" | "--help") => {
                // Subcommand-aware: if we've already seen `cpc test`,
                // `cpc fmt`, etc., print just that subcommand's slice
                // of the usage. Falls back to the full usage when no
                // subcommand is active.
                let slice = subcommand_help(subcommand);
                print!("{slice}");
                return ExitCode::SUCCESS;
            }
            Some("-V" | "--version") => {
                println!("cpc {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            // `build` / `fmt` are positional subcommands. They must
            // appear before any positional input file.
            Some("build") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Build);
                i += 1;
            }
            Some("fmt") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Fmt);
                i += 1;
            }
            Some("test") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Test);
                i += 1;
            }
            Some("check") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Check);
                i += 1;
            }
            Some("headers") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Headers);
                i += 1;
            }
            Some("doc") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Doc);
                i += 1;
            }
            Some("lsp") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Lsp);
                i += 1;
            }
            Some("graph") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Graph);
                i += 1;
            }
            Some("query") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Query);
                i += 1;
            }
            Some("mcp") if subcommand.is_none() && input.is_none() => {
                subcommand = Some(Subcommand::Mcp);
                i += 1;
            }
            // `cpc query`-specific flag: `--depth N` for call-hierarchy.
            Some("--depth") if matches!(subcommand, Some(Subcommand::Query)) => {
                if let Some(v) = args.get(i + 1) {
                    query_args.push("--depth".to_string());
                    query_args.push(v.to_string_lossy().into_owned());
                    i += 2;
                } else {
                    eprintln!("cpc query: --depth requires a number");
                    return ExitCode::FAILURE;
                }
            }
            // `cpc test`-specific flags.
            Some("--json") if matches!(subcommand, Some(Subcommand::Test)) => {
                test_opts.json = true;
                i += 1;
            }
            // `cpc fmt`-specific flags. Only recognized after `fmt`.
            Some("--check") if matches!(subcommand, Some(Subcommand::Fmt)) => {
                fmt_opts.check = true;
                i += 1;
            }
            Some("--emit") if matches!(subcommand, Some(Subcommand::Fmt)) => {
                fmt_opts.emit = true;
                i += 1;
            }
            Some("--stdin") if matches!(subcommand, Some(Subcommand::Fmt)) => {
                fmt_opts.stdin = true;
                i += 1;
            }
            Some(s) if s.starts_with('-') => {
                eprintln!("cpc: unknown flag: {s}");
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            _ => {
                // `cpc fmt` accepts multiple positional paths; every other
                // mode takes exactly one input.
                if matches!(subcommand, Some(Subcommand::Fmt)) {
                    fmt_inputs.push(PathBuf::from(&args[i]));
                    i += 1;
                } else if matches!(subcommand, Some(Subcommand::Test)) {
                    if test_input.is_some() {
                        eprintln!("cpc test: at most one FILE argument");
                        return ExitCode::FAILURE;
                    }
                    test_input = Some(PathBuf::from(&args[i]));
                    i += 1;
                } else if matches!(subcommand, Some(Subcommand::Query)) {
                    // First positional is the query kind; the rest are args.
                    if query_kind.is_none() {
                        query_kind = Some(args[i].to_string_lossy().into_owned());
                    } else {
                        query_args.push(args[i].to_string_lossy().into_owned());
                    }
                    i += 1;
                } else {
                    if input.is_some() {
                        eprintln!("cpc: multiple input files not yet supported");
                        return ExitCode::FAILURE;
                    }
                    input = Some(PathBuf::from(&args[i]));
                    i += 1;
                }
            }
        }
    }

    // Phase 11 polish: validate sanitizer combinations. ASan/TSan/MSan
    // are mutually exclusive (they own the shadow memory or interpose
    // on the same syscalls); UBSan composes with any of them.
    {
        let exclusive: Vec<&'static str> = sanitizers
            .iter()
            .copied()
            .filter(|s| matches!(*s, "address" | "thread" | "memory"))
            .collect();
        if exclusive.len() > 1 {
            eprintln!(
                "cpc: --asan/--tsan/--msan are mutually exclusive (got: {})",
                exclusive.join(", ")
            );
            return ExitCode::FAILURE;
        }
    }

    // v0.0.21 multi-backend slice 1: external-builder targets stop at object
    // emission — cpc never runs their final link (Xcode/NDK/ESP-IDF own it).
    // Reject the host-link entry points up front with a pointer to the
    // supported flows. `--emit-obj` (checked via emit_obj_input) and the
    // `--emit-ll`/`--emit-asm` flags (already dispatched inline above) are
    // the handoff points; `build` routes by entry/library and handoff class.
    if target_spec.handoff == Handoff::ExternalBuilder && emit_obj_input.is_none() {
        match (subcommand, &input) {
            (Some(Subcommand::Test), _) => {
                eprintln!(
                    "cpc: `cpc test` does not support --target {} (test binaries link and run on the host)",
                    target_spec.name
                );
                return ExitCode::FAILURE;
            }
            (None, Some(_)) | (None, None) => {
                eprintln!(
                    "cpc: target `{}` stops at object emission (the external builder owns the final link)",
                    target_spec.name
                );
                eprintln!(
                    "    use --emit-obj/--emit-ll/--emit-asm, or `cpc build` of a project (the entry or library becomes a static archive)"
                );
                return ExitCode::FAILURE;
            }
            _ => {}
        }
    }

    // `cpc lsp` forwards any remaining args to the cpc-lsp binary.
    // (`--log PATH` is the only one cpc-lsp accepts in slice 4E.1, but
    // we don't reach into here — just pass everything past `lsp`.)
    let lsp_args: Vec<OsString> = match subcommand {
        Some(Subcommand::Lsp) => args
            .into_iter()
            .skip_while(|a| a != "lsp")
            .skip(1)
            .collect(),
        _ => Vec::new(),
    };

    // Phase 5 Slice 5.A: `--emit-obj FILE -o OUT.o` runs before any
    // subcommand dispatch. Both args must be present; both can be in any
    // order on the command line because we deferred them here.
    if let Some(obj_in) = emit_obj_input {
        let Some(obj_out) = out else {
            eprintln!("cpc: --emit-obj requires `-o OUT.o`");
            return ExitCode::FAILURE;
        };
        return dump_obj(obj_in, obj_out, diag_mode, build_mode, fp_contract);
    }

    match (subcommand, input) {
        (Some(Subcommand::Build), _) => {
            build_project(out, diag_mode, build_mode, fp_contract, &sanitizers)
        }
        (Some(Subcommand::EmitLlProject), _) => emit_ll_project(diag_mode, build_mode, fp_contract),
        (Some(Subcommand::PrintLinkArgs), _) => {
            print_link_args(diag_mode, build_mode, &sanitizers)
        }
        (Some(Subcommand::Fmt), _) => run_fmt(fmt_inputs, fmt_opts, diag_mode),
        (Some(Subcommand::Test), _) => {
            run_test(test_input, test_opts, diag_mode, build_mode, &sanitizers, fp_contract)
        }
        (Some(Subcommand::Lsp), _) => run_lsp(lsp_args),
        (Some(Subcommand::Check), Some(path)) => run_check(path, diag_mode),
        (Some(Subcommand::Check), None) => run_check_project(diag_mode),
        (Some(Subcommand::Headers), _) => run_headers(),
        (Some(Subcommand::Doc), Some(path)) => run_doc(path),
        (Some(Subcommand::Doc), None) => {
            eprintln!("cpc: `doc` requires a FILE argument");
            ExitCode::FAILURE
        }
        (Some(Subcommand::Graph), _) => run_graph(diag_mode),
        (Some(Subcommand::Query), _) => run_query(query_kind, query_args, diag_mode),
        (Some(Subcommand::Mcp), _) => run_mcp(diag_mode),
        (None, Some(path)) => compile_file(
            path,
            out.unwrap_or_else(|| PathBuf::from("a.out")),
            diag_mode,
            build_mode,
            fp_contract,
            emit_debug_info,
            &sanitizers,
        ),
        (None, None) => phase0_hello(out.unwrap_or_else(|| PathBuf::from("hello"))),
    }
}

#[derive(Debug, Clone, Copy)]
enum Subcommand {
    Build,
    /// `cpc headers` — generate `lib/include/` from `src/` for the package in
    /// the current directory. Concrete modules get their bodies stripped to
    /// declarations; modules declaring generics are copied verbatim, because a
    /// generic has no object code until a consumer instantiates it.
    Headers,
    EmitLlProject,
    /// `cpc build --print-link-args` — print the link line this project's
    /// DEPENDENCIES contribute, one argument per line, and build nothing.
    ///
    /// A cross target hands its consumer one archive: the entry package and
    /// the generics it instantiated. Every dependency's object code is in its
    /// own prebuilt slice beside the package, and the external build system —
    /// Xcode, Gradle, ESP-IDF — has to name all of them or the link fails with
    /// hundreds of undefined symbols that look like a bug in whichever package
    /// happens to be named first. cpc already resolves that list to link a HOST
    /// build; for a cross target it stopped at the archive and exposed the list
    /// nowhere, so every consumer re-derived it. iris did, with `find -L` over
    /// two roots, which is a resolution this tool owns.
    PrintLinkArgs,
    Fmt,
    Test,
    Lsp,
    /// Phase 11 polish (2026-05-14): `cpc check FILE` — parse + sema +
    /// borrowck on a single file, no codegen. Promised in SKILL.md as
    /// the "fast feedback loop" command but never wired until now.
    Check,
    /// Phase 11 polish (2026-05-14): `cpc doc FILE` — extract public
    /// (non-`_`-private) items + their `///` docs from a source file,
    /// emit Markdown to `target/doc/<basename>.md`.
    Doc,
    /// `cpc graph` — build the code knowledge graph for the project and
    /// print it as JSON (nodes + edges). See `plan.graph.md`.
    Graph,
    /// `cpc query <kind> [args...]` — answer one graph query (`def`,
    /// `members`, `symbols`, …) as JSON.
    Query,
    /// `cpc mcp` — resident MCP server over the code graph (stdio JSON-RPC).
    Mcp,
}

#[derive(Debug, Default, Clone, Copy)]
struct FmtOpts {
    check: bool,
    emit: bool,
    stdin: bool,
}

#[derive(Debug, Default, Clone, Copy)]
struct TestOpts {
    json: bool,
}

/// The clang executable cpc shells out to for assembling and linking.
///
/// cpc emits the `preserve_nonecc` calling convention on drop glue, which
/// LLVM only understands from **version 19**. Distros routinely ship an
/// older `clang` as the default with newer `clang-NN` installed alongside
/// (e.g. Ubuntu 24.04: `clang` is 18, `clang-19` is a separate package that
/// does NOT take over the `clang` name). So rather than hardcode `clang`,
/// resolve the program once per process:
///   1. `$CPC_CLANG` if set — an explicit user/operator override, trusted
///      verbatim (lets packagers or CI point at any toolchain).
///   2. bare `clang` if it already reports LLVM >= 19 — honors the user's
///      PATH / `update-alternatives` choice.
///   3. `clang-21`, `clang-20`, `clang-19` in descending order — the
///      side-by-side versioned binaries.
///
/// If nothing qualifies, fall back to bare `clang` so the existing failure
/// path (clang rejecting the IR) still surfaces a clear compiler error.
fn clang_program() -> &'static str {
    static RESOLVED: OnceLock<String> = OnceLock::new();
    RESOLVED
        .get_or_init(|| {
            if let Ok(p) = env::var("CPC_CLANG") {
                if !p.is_empty() {
                    return p;
                }
            }
            if clang_major("clang").is_some_and(|m| m >= 19) {
                return "clang".to_string();
            }
            for cand in ["clang-21", "clang-20", "clang-19"] {
                if clang_major(cand).is_some_and(|m| m >= 19) {
                    return cand.to_string();
                }
            }
            "clang".to_string()
        })
        .as_str()
}

/// v0.0.21 multi-backend rung 2: the clang that consumes IR for the given
/// target. Host-toolchain targets (including iOS — Apple/mainline clang
/// emits `arm64-apple-ios` objects) use the existing `clang_program()`
/// resolution; the Android target resolves the NDK's clang, which carries
/// the Android sysroot. `Err` is a ready-to-print message (callers add the
/// `cpc: ` prefix).
fn clang_program_for(t: &TargetSpec) -> Result<String, String> {
    match t.toolchain {
        target::ToolchainKind::HostClang => Ok(clang_program().to_string()),
        target::ToolchainKind::AndroidNdk => ndk_clang().clone(),
        target::ToolchainKind::EspClang => esp_clang().clone(),
        // wasm32 emits its artifact in-process (no clang) and is browser-only;
        // it is not a `--target` the native driver resolves, so this is
        // unreachable in practice — fail loudly rather than call a wrong clang.
        target::ToolchainKind::Internal => Err(
            "the wasm32 target is built by the browser playground, not the native cpc driver"
                .to_string(),
        ),
    }
}

/// Resolve Espressif's esp-clang (the LLVM fork with the Xtensa backend),
/// cached per process. Order:
///   1. `$CPC_ESP_CLANG` — an explicit clang path, trusted verbatim.
///   2. `$IDF_TOOLS_PATH` — ESP-IDF's tools root override. Set-but-wrong is
///      an error naming the variable, never a fallback.
///   3. `~/.espressif` — the default `idf_tools.py` install root.
///
/// Inside the root: `tools/esp-clang/<newest-version>/esp-clang/bin/clang`,
/// which must report LLVM >= 19 (cpc's IR floor; esp-clang 20.1.1+ in
/// practice).
fn esp_clang() -> &'static Result<String, String> {
    static RESOLVED: OnceLock<Result<String, String>> = OnceLock::new();
    RESOLVED.get_or_init(|| {
        if let Ok(p) = env::var("CPC_ESP_CLANG") {
            if !p.is_empty() {
                return Ok(p);
            }
        }
        let tools_root: PathBuf = match env::var("IDF_TOOLS_PATH") {
            Ok(v) if !v.is_empty() => {
                let p = PathBuf::from(&v);
                if !p.is_dir() {
                    return Err(format!(
                        "$IDF_TOOLS_PATH is set to `{v}`, which is not a directory; point it at the ESP-IDF tools root (the `.espressif` directory)"
                    ));
                }
                p
            }
            _ => {
                let Some(home) = env::var_os("HOME").or_else(|| env::var_os("USERPROFILE"))
                else {
                    return Err("cannot locate the ESP-IDF tools root (no $HOME)".to_string());
                };
                PathBuf::from(home).join(".espressif")
            }
        };
        let esp_clang_dir = tools_root.join("tools").join("esp-clang");
        let Some(version_dir) = newest_version_dir(&esp_clang_dir) else {
            return Err(
                "esp-clang was not found; install it with ESP-IDF's `python3 tools/idf_tools.py install esp-clang`, or set $CPC_ESP_CLANG to its clang binary".to_string(),
            );
        };
        let clang_name = if cfg!(windows) { "clang.exe" } else { "clang" };
        let clang = version_dir.join("esp-clang").join("bin").join(clang_name);
        if !clang.is_file() {
            return Err(format!(
                "esp-clang install at `{}` has no clang at `{}`",
                version_dir.display(),
                clang.display()
            ));
        }
        let clang_str = clang.to_string_lossy().to_string();
        match clang_major(&clang_str) {
            Some(m) if m >= 19 => Ok(clang_str),
            Some(m) => Err(format!(
                "the esp-clang at `{}` reports clang {m}, but cpc emits IR for LLVM 19+; update with `idf_tools.py install esp-clang`",
                version_dir.display()
            )),
            None => Err(format!(
                "could not run `{clang_str} --version` to verify esp-clang"
            )),
        }
    })
}

/// The newest version directory under `dir`, comparing every numeric run in
/// the name (handles `esp-20.1.1_20250829`-style names and plain dotted
/// versions alike). `None` when the directory is missing or has no entries
/// with a numeric component.
fn newest_version_dir(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let mut nums: Vec<u64> = Vec::new();
        let mut cur = String::new();
        for c in name.chars() {
            if c.is_ascii_digit() {
                cur.push(c);
            } else if !cur.is_empty() {
                nums.push(cur.parse().unwrap_or(0));
                cur.clear();
            }
        }
        if !cur.is_empty() {
            nums.push(cur.parse().unwrap_or(0));
        }
        if nums.is_empty() {
            continue;
        }
        if best.as_ref().map_or(true, |(b, _)| nums > *b) {
            best = Some((nums, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Resolve the Android NDK's clang, cached per process. Order:
///   1. `$CPC_NDK_CLANG` — an explicit clang path, trusted verbatim
///      (mirrors `$CPC_CLANG`).
///   2. `$ANDROID_NDK_HOME` / `$ANDROID_NDK_ROOT` / `$ANDROID_NDK_LATEST_HOME`
///      — an NDK root. Set-but-wrong is an error naming the variable, never
///      a silent fallback.
///   3. The Android SDK's default `ndk/` directory for the host OS
///      (`~/Library/Android/sdk/ndk`, `~/Android/Sdk/ndk`,
///      `%LOCALAPPDATA%\Android\Sdk\ndk`), newest installed version.
///
/// The resolved clang must report LLVM >= 19: cpc emits `preserve_nonecc`,
/// which older LLVM rejects — that means NDK r28.2+ (r27 ships clang 18).
fn ndk_clang() -> &'static Result<String, String> {
    static RESOLVED: OnceLock<Result<String, String>> = OnceLock::new();
    RESOLVED.get_or_init(|| {
        if let Ok(p) = env::var("CPC_NDK_CLANG") {
            if !p.is_empty() {
                return Ok(p);
            }
        }
        let mut root: Option<PathBuf> = None;
        for var in ["ANDROID_NDK_HOME", "ANDROID_NDK_ROOT", "ANDROID_NDK_LATEST_HOME"] {
            if let Ok(v) = env::var(var) {
                if !v.is_empty() {
                    let p = PathBuf::from(&v);
                    if !p.is_dir() {
                        return Err(format!(
                            "${var} is set to `{v}`, which is not a directory; point it at an Android NDK root (r28.2+)"
                        ));
                    }
                    root = Some(p);
                    break;
                }
            }
        }
        let root = match root {
            Some(r) => r,
            None => match newest_default_ndk() {
                Some(r) => r,
                None => {
                    return Err(
                        "the Android NDK was not found; set $ANDROID_NDK_HOME to an NDK root (r28.2+), or $CPC_NDK_CLANG to its clang binary".to_string(),
                    );
                }
            },
        };
        let host_tag = if cfg!(target_os = "macos") {
            "darwin-x86_64" // also the arm64-mac tag: NDK ships universal binaries here
        } else if cfg!(windows) {
            "windows-x86_64"
        } else {
            "linux-x86_64"
        };
        let clang_name = if cfg!(windows) { "clang.exe" } else { "clang" };
        let clang = root
            .join("toolchains")
            .join("llvm")
            .join("prebuilt")
            .join(host_tag)
            .join("bin")
            .join(clang_name);
        if !clang.is_file() {
            return Err(format!(
                "NDK at `{}` has no clang at `{}`; expected an NDK r28.2+ install",
                root.display(),
                clang.display()
            ));
        }
        let clang_str = clang.to_string_lossy().to_string();
        match clang_major(&clang_str) {
            Some(m) if m >= 19 => Ok(clang_str),
            Some(m) => Err(format!(
                "the NDK at `{}` ships clang {m}, but cpc emits IR for LLVM 19+; install NDK r28.2 or newer (or point $ANDROID_NDK_HOME at one)",
                root.display()
            )),
            None => Err(format!(
                "could not run `{clang_str} --version` to verify the NDK clang"
            )),
        }
    })
}

/// The archiver for a target's staticlib. `$CPC_AR` overrides everything.
/// External toolchains use the `llvm-ar` sitting next to their resolved
/// clang (it understands the target's object format); host targets keep the
/// historical `ar` / `llvm-ar`-on-Windows choice.
fn ar_program_for(t: &TargetSpec, clang_prog: &str) -> String {
    if let Ok(p) = env::var("CPC_AR") {
        if !p.is_empty() {
            return p;
        }
    }
    if t.toolchain != target::ToolchainKind::HostClang {
        let name = if cfg!(windows) {
            "llvm-ar.exe"
        } else {
            "llvm-ar"
        };
        let sibling = Path::new(clang_prog).with_file_name(name);
        if sibling.is_file() {
            return sibling.to_string_lossy().to_string();
        }
    }
    if cfg!(windows) { "llvm-ar" } else { "ar" }.to_string()
}

/// The newest NDK version directory under the host's default Android SDK
/// location, or `None` when none is installed. Version directories are
/// dotted-numeric (`28.2.13676358`); non-numeric entries are ignored.
fn newest_default_ndk() -> Option<PathBuf> {
    let ndk_dir: PathBuf = if cfg!(target_os = "macos") {
        PathBuf::from(env::var_os("HOME")?).join("Library/Android/sdk/ndk")
    } else if cfg!(windows) {
        PathBuf::from(env::var_os("LOCALAPPDATA")?)
            .join("Android")
            .join("Sdk")
            .join("ndk")
    } else {
        PathBuf::from(env::var_os("HOME")?).join("Android/Sdk/ndk")
    };
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for entry in fs::read_dir(&ndk_dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Ok(parts) = name
            .split('.')
            .map(|s| s.parse::<u64>())
            .collect::<Result<Vec<u64>, _>>()
        else {
            continue;
        };
        if parts.is_empty() {
            continue;
        }
        if best.as_ref().map_or(true, |(b, _)| parts > *b) {
            best = Some((parts, path));
        }
    }
    best.map(|(_, p)| p)
}

/// Major LLVM version reported by `<prog> --version`, or `None` if the
/// program can't be run or its output can't be parsed. The first line looks
/// like `Ubuntu clang version 19.1.1` or `clang version 19.1.1`.
fn clang_major(prog: &str) -> Option<u32> {
    let out = Command::new(prog).arg("--version").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let after = text.split("clang version ").nth(1)?;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}

/// Probe whether the resolved clang's `llvm.coro.end` intrinsic returns `void`
/// (LLVM ~22+) or `i1` (older LLVM, and Apple clang 21). The two forms are
/// mutually incompatible — each version's verifier rejects the other with
/// "Intrinsic has incorrect return type!" — and the correct one depends on the
/// *target toolchain*, not the host `cpc` was built on. (Apple-clang version
/// numbers don't map to LLVM versions, so a capability probe is more reliable
/// than parsing `--version`.)
///
/// We compile a tiny IR that *calls* the `void` form: if the verifier rejects
/// the signature, the toolchain wants `i1`. Any other outcome (it links, or it
/// fails later for an unrelated reason like an unlowered intrinsic) means the
/// `void` signature was accepted. Cached for the process; defaults to `void`
/// if clang can't be run.
fn coro_end_returns_void() -> bool {
    static CACHE: OnceLock<bool> = OnceLock::new();
    *CACHE.get_or_init(|| {
        let dir = env::temp_dir();
        let pid = std::process::id();
        let probe = dir.join(format!("cpc_coro_probe_{pid}.ll"));
        let obj = dir.join(format!("cpc_coro_probe_{pid}.o"));
        let ir = "define void @__cpc_coro_probe() {\n\
                  \x20 call void @llvm.coro.end(ptr null, i1 false, token none)\n\
                  \x20 ret void\n\
                  }\n\
                  declare void @llvm.coro.end(ptr, i1, token)\n";
        if std::fs::write(&probe, ir).is_err() {
            return true;
        }
        // v0.0.21 rung 2: probe the toolchain that will consume this
        // process's IR — the active target's clang (e.g. NDK clang) when it
        // resolves, else the host clang (pure IR-emission paths must work
        // without the external toolchain installed).
        let prog = clang_program_for(&target::active_target())
            .unwrap_or_else(|_| clang_program().to_string());
        let output = Command::new(&prog)
            .arg("-x")
            .arg("ir")
            .arg(&probe)
            .arg("-c")
            .arg("-o")
            .arg(&obj)
            .output();
        let _ = std::fs::remove_file(&probe);
        let _ = std::fs::remove_file(&obj);
        match output {
            Ok(o) => !String::from_utf8_lossy(&o.stderr).contains("incorrect return type"),
            Err(_) => true,
        }
    })
}

/// Install the probed `llvm.coro.end` form into codegen. Idempotent and cheap
/// (the probe is cached); call before any `codegen::generate*`.
fn ensure_coro_end_probed() {
    cplus_core::codegen::set_coro_end_returns_void(coro_end_returns_void());
}

/// Phase 2 Slice 2C: detect the host triple via `clang -print-target-triple`.
/// Used by the dep walker to look up bundled binary paths in each vendor
/// package's `lib/<triple>/`. Each build calls this once.
///
/// MEMOISED, because "once" was the intent and not the behaviour. The dep
/// walkers that need it (`ensure_prebuilt_deps`, `collect_dep_link_args`) are
/// RECURSIVE over the dependency graph, so the probe ran once per edge
/// traversal: measured on `examples/facet_gallery`, whose GTK closure is
/// seventeen packages, `clang -print-target-triple` was executed 1037 TIMES in
/// one build — against a single `clang -cc1` that did the actual compiling.
///
/// Each of those spawns also searched `PATH` and missed nine times before
/// finding the binary, which is why a warm build showed 9.2s of SYSTEM time
/// and 15.6s of "unattributed" wall clock the timing table could not place.
///
/// The answer cannot change inside one process, so it is computed once.
fn detect_host_triple() -> Result<String, ExitCode> {
    static HOST_TRIPLE: OnceLock<Option<String>> = OnceLock::new();
    match HOST_TRIPLE.get_or_init(detect_host_triple_uncached) {
        Some(t) => Ok(t.clone()),
        // The probe already printed why; the exit code is the caller's.
        None => Err(ExitCode::FAILURE),
    }
}

fn detect_host_triple_uncached() -> Option<String> {
    let output = match Command::new(clang_program())
        .arg("-print-target-triple")
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("cpc: invoking `clang -print-target-triple`: {e}");
            return None;
        }
    };
    if !output.status.success() {
        eprintln!(
            "cpc: `clang -print-target-triple` exited with {:?}",
            output.status.code()
        );
        return None;
    }
    let triple = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if triple.is_empty() {
        eprintln!("cpc: `clang -print-target-triple` produced no output");
        return None;
    }
    Some(triple)
}

/// v0.0.21 multi-backend slice 1: clang arguments pinning an explicit
/// `--target`: `-target <triple>`, plus `-isysroot <path>` when the target
/// names an Apple SDK and `xcrun` can resolve it. Empty for the host spec,
/// so every `--target`-less command line stays exactly what it was.
///
/// The `-isysroot` is best-effort by design: object emission from IR reads
/// nothing out of the SDK (no headers, no libraries — the external builder
/// links against the SDK later), so a host without `xcrun` (e.g. Linux CI
/// cross-emitting iOS objects with mainline clang) simply omits the flag.
fn clang_target_args(t: &TargetSpec) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();
    if t.triple.is_none() {
        return args;
    }
    // `--min-os`-aware: the spliced triple when an override is installed.
    let triple = target::active_triple().expect("non-host target has a triple");
    args.push("-target".to_string());
    args.push(triple);
    for extra in t.extra_clang_args {
        args.push((*extra).to_string());
    }
    if let Some(sdk) = t.apple_sdk {
        if let Some(path) = xcrun_sdk_path(sdk) {
            args.push("-isysroot".to_string());
            args.push(path);
        }
    }
    args
}

/// `xcrun --sdk <name> --show-sdk-path`, or `None` when xcrun is missing,
/// errors, or prints nothing (non-Apple host, SDK not installed).
fn xcrun_sdk_path(sdk: &str) -> Option<String> {
    let out = Command::new("xcrun")
        .arg("--sdk")
        .arg(sdk)
        .arg("--show-sdk-path")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        return None;
    }
    Some(path)
}

/// Phase 2 Slice 2C: build a `Diagnostic` anchored at a manifest file.
/// Manifest-level driver errors (E0854/E0855/E0860/E0861/E0862) don't
/// have meaningful byte spans yet; the primary location is the file at
/// position 1:1.
fn manifest_diag(
    code: &'static str,
    path: &Path,
    message: String,
    notes: Vec<String>,
) -> Diagnostic {
    Diagnostic {
        severity: Severity::Error,
        code: diag::DiagCode(code),
        message,
        primary: diag::SourceSpan {
            file: path.to_path_buf(),
            start: diag::Position {
                line: 1,
                col: 1,
                byte: 0,
            },
            end: diag::Position {
                line: 1,
                col: 1,
                byte: 0,
            },
        },
        labels: Vec::new(),
        notes,
        suggestions: Vec::new(),
    }
}

/// Phase 2 Slice 2C: walk the consumer's `[dependencies]`, validate each
/// vendor package against the manifest-is-truth contract, and accumulate
/// linker arguments. The build driver appends these after the consumer's
/// own `[link]` frameworks/libs so the order
/// is: consumer-first, then each dep in declared order.
///
/// Per-dep validation:
///   - `vendor/<name>/Cplus.toml` exists (E0854) and parses cleanly.
///   - Vendor manifest's `[package].name == <name>` (E0855).
///   - For each name in `[link].bundled`:
///     host triple is in `[link].triples` (E0862),
///     `vendor/<name>/lib/<host-triple>/<basename>` exists (E0860).
///   - No `.a`/`.dylib`/`.so` files under any
///     `vendor/<name>/lib/<triple>/` that aren't in `[link].bundled`
///     (E0861). Applies even when a package declares no `[link]` table —
///     orphan binaries are a manifest bug, never a graceful-degradation
///     case.
///
/// On any failure: a structured diagnostic is emitted via `emit_diag` and
/// `Err(ExitCode::FAILURE)` is returned before codegen / linking can run.
/// Platform-scoped dependencies: the dep names to thread into the resolver
/// for the ACTIVE platform (`[dependencies]` entries plus the matching
/// `[<platform>.dependencies]` section). As a side effect, installs the
/// filtered-out deps into `target::set_platform_gated_deps` so an import of
/// an off-platform package gets the targeted E0866 instead of E0852.
fn active_dep_names(m: &manifest::Manifest) -> Vec<String> {
    // Lives in core now: `cpc lsp` resolves projects too, and resolving one
    // without its dependency names is what made every vendored import in an
    // editor report E0401.
    manifest::active_dep_names(m)
}

fn collect_dep_link_args(
    m: &manifest::Manifest,
    diag_mode: DiagMode,
) -> Result<Vec<String>, ExitCode> {
    if m.dependencies.is_empty() {
        return Ok(Vec::new());
    }
    // v0.0.21 multi-backend slice 1: binary slices resolve by the *selected*
    // target's stable artifact triple; only the host target still asks
    // `clang -print-target-triple`.
    //
    // The host triple is normalised to its stable, version-less form before it
    // names a directory: `clang -print-target-triple` reports the running
    // system (`arm64-apple-darwin25.5.0`), so using it raw would make a slice
    // stop being found after an OS upgrade. Cross targets already carry a
    // fixed canonical `artifact_triple`.
    let link_triple = active_link_triple()?;
    let mut link_args: Vec<String> = Vec::new();
    // (package, its declared deps, its archives) — ordered after the walk.
    let mut archives: Vec<(String, Vec<String>, Vec<String>)> = Vec::new();
    let platform = target::active_platform();
    for dep in &m.dependencies {
        // A dep scoped to another platform contributes nothing to this
        // build: no presence check (its vendor/ dir may legitimately be
        // absent here) and no `[link]` splice (its frameworks/libs would
        // be wrong for this toolchain — `-framework AppKit` on Linux).
        if !dep.active_on(platform) {
            continue;
        }
        let vendor_dir = vendor_dir_for(m, &dep.name)
            .unwrap_or_else(|| m.root.join("vendor").join(&dep.name));
        let vendor_manifest = vendor_dir.join("Cplus.toml");
        if !vendor_manifest.is_file() {
            let d = manifest_diag(
                "E0854",
                &vendor_manifest,
                format!(
                    "vendor package `{}` is missing `Cplus.toml` (expected at `{}`)",
                    dep.name,
                    vendor_manifest.display()
                ),
                vec![
                    format!(
                        "declared in `[dependencies]` of {}",
                        m.root.join("Cplus.toml").display()
                    ),
                    "run `cpc pm install` to fetch dependencies into the store".to_string(),
                ],
            );
            emit_diag(&d, diag_mode, "");
            return Err(ExitCode::FAILURE);
        }
        let vm = match manifest::load(&vendor_manifest) {
            Ok(v) => v,
            Err(e) => {
                emit_diag(&e.to_diagnostic(), diag_mode, "");
                return Err(ExitCode::FAILURE);
            }
        };
        if vm.package.name != dep.name {
            let d = manifest_diag(
                "E0855",
                &vendor_manifest,
                format!(
                    "package `Cplus.toml` declares name `{}` but lives in `vendor/{}/`",
                    vm.package.name, dep.name
                ),
                vec![
                    "a vendor package's `[package].name` must match its directory name".to_string(),
                ],
            );
            emit_diag(&d, diag_mode, "");
            return Err(ExitCode::FAILURE);
        }
        // AAR-shaped layout: binaries live at the package root under `lib/`,
        // not inside `src/`. `src/` is importable C+ source; a prebuilt archive
        // is not source and does not belong there.
        let lib_root = vendor_dir.join("lib");
        let bundled: &[String] = vm
            .link
            .as_ref()
            .map(|l| l.bundled.as_slice())
            .unwrap_or(&[]);
        // `[build] dev = true`: the package is being worked on. No binaries of
        // any origin participate — not its own prebuilt slice, not an
        // author-shipped one — and the resolver compiles it from `src/`. Said
        // out loud on every build: a manifest knob that changes what gets
        // compiled must not be able to sit there forgotten.
        if vm.build.dev {
            eprintln!(
                "cpc: `{}` is in dev mode (`[build] dev = true`) — compiling it from source",
                dep.name
            );
            splice_plain_link_args(&mut link_args, &vm, diag_mode, &vendor_manifest)?;
            continue;
        }
        // The triple is derived, never declared: whatever we are building for
        // names the directory. Its absence is not an error — it means this
        // package ships nothing for this target, so it compiles from `src/`
        // exactly like a source-only package. That is what makes an ignored
        // (or not-yet-built) slice a non-event on a fresh clone.
        let triple_lib_dir = lib_root.join(&link_triple);
        if !bundled.is_empty() && triple_lib_dir.is_dir() {
            // Inside a slice that IS present, the manifest is truth.
            for basename in bundled {
                let p = triple_lib_dir.join(basename);
                if !p.is_file() {
                    let d = manifest_diag(
                        "E0860",
                        &vendor_manifest,
                        format!(
                            "package `{}` declares bundled `{}` but `lib/{}/{}` is not present (the package manifest says you ship it for this triple, but the file is missing)",
                            dep.name, basename, link_triple, basename
                        ),
                        vec![
                            format!("expected at `{}`", p.display()),
                            format!("either add the file or remove `{}` from `[link].bundled`", basename),
                        ],
                    );
                    emit_diag(&d, diag_mode, "");
                    return Err(ExitCode::FAILURE);
                }
            }
        }
        // Orphan-file check: every binary under `lib/<triple>/` (any
        // triple, not just the host's) must be declared in `[link].bundled`.
        // Applies even when bundled is empty — a source-only package with a
        // stray `.a` is a manifest bug.
        if lib_root.is_dir() {
            if let Ok(triple_iter) = fs::read_dir(&lib_root) {
                for triple_entry in triple_iter.flatten() {
                    let triple_dir = triple_entry.path();
                    if !triple_dir.is_dir() {
                        continue;
                    }
                    let Ok(file_iter) = fs::read_dir(&triple_dir) else {
                        continue;
                    };
                    for entry in file_iter.flatten() {
                        let fname_os = entry.file_name();
                        let fname = fname_os.to_string_lossy().to_string();
                        let is_binary = fname.ends_with(".a")
                            || fname.ends_with(".dylib")
                            || fname.ends_with(".so")
                            || fname.ends_with(".lib");
                        if !is_binary {
                            continue;
                        }
                        // The archive `prebuild` produces is expected, not an
                        // orphan: the compiler put it there, and the package
                        // deliberately doesn't declare it (declaring it would
                        // make a not-yet-built slice an E0860 on a fresh clone).
                        if vm.build.prebuild && fname == prebuilt_archive_name(&dep.name) {
                            continue;
                        }
                        if !bundled.iter().any(|b| b == &fname) {
                            let triple_name = triple_dir
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("?");
                            let d = manifest_diag(
                                "E0861",
                                &vendor_manifest,
                                format!(
                                    "package `{}` ships `lib/{}/{}` but the manifest doesn't declare it; the manifest is the single source of truth",
                                    dep.name, triple_name, fname
                                ),
                                vec![format!(
                                    "either add `{}` to `[link].bundled` or delete the file", fname
                                )],
                            );
                            emit_diag(&d, diag_mode, "");
                            return Err(ExitCode::FAILURE);
                        }
                    }
                }
            }
        }
        // Splice this dep's validated link contributions into the line.
        splice_plain_link_args(&mut link_args, &vm, diag_mode, &vendor_manifest)?;
        // ARCHIVES ARE NOT PUSHED HERE. They are collected and ordered after
        // the loop — see `order_archives_dependents_first` for why the order
        // this loop walks in is the wrong one.
        let mut pkg_archives: Vec<String> = Vec::new();
        // The prebuilt slice, if `ensure_prebuilt_deps` produced one. Checked
        // for existence rather than assumed: `cpc check` never builds a cache,
        // and a dep whose package failed to prebuild has already aborted.
        if vm.build.prebuild {
            let archive = triple_lib_dir.join(prebuilt_archive_name(&dep.name));
            if archive.is_file() {
                pkg_archives.push(archive.to_string_lossy().to_string());
            }
        }
        // Bundled artifacts go in as full paths (not `-l<name>` — they're
        // not on the linker's search path). Skipped entirely when the slice
        // directory is absent: that case already resolved to source. They
        // follow the package's own slice, which is what references them.
        if triple_lib_dir.is_dir() {
            if let Some(ls) = &vm.link {
                for basename in &ls.bundled {
                    pkg_archives.push(triple_lib_dir.join(basename).to_string_lossy().to_string());
                }
            }
        }
        if !pkg_archives.is_empty() {
            let dep_names: Vec<String> =
                vm.dependencies.iter().map(|d| d.name.clone()).collect();
            archives.push((dep.name.clone(), dep_names, pkg_archives));
        }
    }
    link_args.extend(order_archives_dependents_first(archives));
    Ok(link_args)
}

/// Order package archives so a DEPENDENT always precedes its DEPENDENCY, and
/// flatten them onto the link line.
///
/// GNU `ld` resolves left-to-right in a single pass and pulls a static-archive
/// member only to satisfy a reference it has ALREADY seen — the same rule that
/// puts the program object first in `run_clang`, applied between the archives.
/// The dep walker visits `m.dependencies`, which parses into a `BTreeMap` and so
/// iterates LEXICOGRAPHICALLY (manifest.rs, "BTreeMap order so any failure is
/// deterministic" — good for diagnostics, meaningless for a link line). For the
/// GTK stack alphabetical is close to the worst possible order: `gdk`, `gio`,
/// `glib`, `gobject` and `gobject_gir` all sort before `gtk4` and are all
/// things `gtk4` calls, so each was scanned and discarded before `gtk4.o` was
/// pulled in and referenced it — 1585 undefined C+ symbols from a link whose
/// archives were all present and correct.
///
/// macOS's `ld64` resolves globally and is order-insensitive, which is why this
/// never appeared on the development host.
///
/// Ties break alphabetically so a given dependency set always produces the same
/// line. A cycle cannot be ordered at all; the packages caught in one are
/// appended alphabetically, which links no worse than before and leaves the
/// linker to report it.
fn order_archives_dependents_first(
    archives: Vec<(String, Vec<String>, Vec<String>)>,
) -> Vec<String> {
    use std::collections::{BTreeMap, BTreeSet};
    let present: BTreeSet<&str> = archives.iter().map(|(n, _, _)| n.as_str()).collect();
    // Edge dependent -> dependency; in-degree counts DEPENDENTS, so the nodes
    // nobody depends on come out first.
    let mut indeg: BTreeMap<&str, usize> = present.iter().map(|n| (*n, 0)).collect();
    for (_, deps, _) in &archives {
        for d in deps {
            if let Some(slot) = indeg.get_mut(d.as_str()) {
                *slot += 1;
            }
        }
    }
    let mut ready: BTreeSet<&str> = indeg
        .iter()
        .filter(|(_, &v)| v == 0)
        .map(|(k, _)| *k)
        .collect();
    let mut emitted: Vec<&str> = Vec::new();
    while let Some(name) = ready.iter().next().copied() {
        ready.remove(name);
        emitted.push(name);
        if let Some((_, deps, _)) = archives.iter().find(|(n, _, _)| n == name) {
            for d in deps {
                if let Some(slot) = indeg.get_mut(d.as_str()) {
                    *slot -= 1;
                    if *slot == 0 {
                        ready.insert(d.as_str());
                    }
                }
            }
        }
    }
    // Anything left is in a cycle — append it alphabetically rather than drop it.
    for n in &present {
        if !emitted.contains(n) {
            emitted.push(n);
        }
    }
    let mut out = Vec::new();
    for name in emitted {
        if let Some((_, _, paths)) = archives.iter().find(|(n, _, _)| n == name) {
            out.extend(paths.iter().cloned());
        }
    }
    out
}

/// Where a dependency's package directory lives, or `None` if it isn't there.
///
/// Normally `<root>/vendor/<name>/`. The fallback covers the vendor-package
/// self-test case: run from inside a vendor package, sibling packages live at
/// `<root>/../<name>/` rather than under a `vendor/` of their own. Mirrors
/// `resolver.rs`'s fallback in `resolve_vendor_path` — the two must agree, or
/// the link line and the import resolution point at different copies.
fn vendor_dir_for(m: &manifest::Manifest, dep_name: &str) -> Option<PathBuf> {
    let primary = m.root.join("vendor").join(dep_name);
    if primary.join("Cplus.toml").is_file() {
        return Some(primary);
    }
    if let Some(parent) = m.root.parent() {
        let alt = parent.join(dep_name);
        if alt.join("Cplus.toml").is_file() {
            return Some(alt);
        }
    }
    // The per-user store (D16), last: a globally-installed package at
    // `~/.cplus/<tier>/vendor/<name>` — the same fallback order import
    // resolution uses, so the linked package is the resolved one.
    let store = cplus_core::resolver::store_vendor_dir()?.join(dep_name);
    store.join("Cplus.toml").is_file().then_some(store)
}

/// Where each active dependency's sources live: name -> canonical `src/`.
/// `--timings` uses it to tell a dependency compiled from source apart from
/// the project's own code — see `timings::inlined_from`, which cannot answer
/// that from a path shape. Resolution mirrors `vendor_dir_for`, so a sibling
/// checkout and a store package are found the same way the build finds them.
fn dep_source_dirs(m: &manifest::Manifest) -> Vec<(String, PathBuf)> {
    active_dep_names(m)
        .into_iter()
        .filter_map(|name| {
            let src = vendor_dir_for(m, &name)?.join("src");
            let src = fs::canonicalize(&src).unwrap_or(src);
            Some((name, src))
        })
        .collect()
}

/// Bring every `[build] prebuild = true` dependency's slice up to date, before
/// anything resolves an import.
///
/// Ordering is the whole reason this is a separate pass: the resolver decides
/// header-vs-source per module, and the link line names the archive, so both
/// need the slice to already exist. `cpc check` deliberately does NOT call
/// this — checking should not spend a clang invocation on a dependency, and
/// resolution falls back to `src/` per module when a header is absent.
///
/// `cpc build --print-link-args` — the link line the DEPENDENCIES contribute,
/// one argument per line, on stdout. Builds nothing else.
///
/// THE ARTIFACT WAS ALWAYS FINE AND THE RECIPE WAS INCOMPLETE. A cross target
/// (`--target ios-arm64-simulator` and friends) emits ONE archive — the entry
/// package plus the generics it instantiated — and every dependency's object
/// code lives in its own slice at `<vendor>/<pkg>/lib/<triple>/lib<pkg>.a`.
/// Linking the app's archive alone leaves hundreds of undefined symbols, named
/// after whichever package resolves first, which reads as a bug in that package
/// and is not one. `prebuild` becoming the default (2026-08-16) moved a
/// dependency's object code OUT of the consuming app's archive and so broke
/// every hand-written iOS link recipe at once.
///
/// cpc has always computed this list — `collect_dep_link_args` is the same walk
/// a host build links with. For a cross target it stopped at the archive and
/// exposed the list nowhere, so every external build system re-derived a
/// resolution this tool owns (project `vendor/`, then a sibling, then
/// `~/.cplus/<tier>/vendor`, then `lib/<link_triple>`). iris re-derived it with
/// `find -L` over two roots and said in its own comment that it would drift the
/// moment the layout changed.
///
/// THE SLICES ARE BROUGHT UP TO DATE FIRST. Printing a path to an archive that
/// is not there yet would just move the failure into the consumer's build, and
/// a stale one is worse than that — see `prebuild_fingerprint`.
///
/// ONE ARGUMENT PER LINE because that is what a build system can consume
/// (`$(cpc build --print-link-args)`, `xargs`, an Xcode script phase) without
/// anyone having to guess a quoting rule. Nothing is printed to stdout but the
/// arguments; diagnostics go to stderr as usual.
fn print_link_args(diag_mode: DiagMode, build_mode: BuildMode, sanitizers: &[&str]) -> ExitCode {
    let manifest_path = PathBuf::from("Cplus.toml");
    let m = match manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            emit_diag(&e.to_diagnostic(), diag_mode, "");
            return ExitCode::FAILURE;
        }
    };
    if let Err(code) = ensure_prebuilt_deps(&m, build_mode, diag_mode, sanitizers, &mut Vec::new())
    {
        return code;
    }
    match collect_dep_link_args(&m, diag_mode) {
        Ok(args) => {
            for a in &args {
                println!("{a}");
            }
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// A dep that is `dev = true`, or that ships author-built binaries, is skipped:
/// the first is being worked on, the second already has its answer.
fn ensure_prebuilt_deps(
    m: &manifest::Manifest,
    build_mode: BuildMode,
    diag_mode: DiagMode,
    sanitizers: &[&str],
    stack: &mut Vec<String>,
) -> Result<(), ExitCode> {
    if m.dependencies.is_empty() {
        return Ok(());
    }
    let link_triple = active_link_triple()?;
    let platform = target::active_platform();
    for dep in &m.dependencies {
        if !dep.active_on(platform) {
            continue;
        }
        let Some(vendor_dir) = vendor_dir_for(m, &dep.name) else {
            continue; // absence is the dep walk's error to report, not ours
        };
        let vendor_manifest = vendor_dir.join("Cplus.toml");
        let Ok(vm) = manifest::load(&vendor_manifest) else {
            continue; // ditto — `collect_dep_link_args` produces the diagnostic
        };
        if !vm.build.prebuild || vm.build.dev {
            continue;
        }
        // A package that ships author-built binaries already has its answer:
        // prebuilding it would compile a slice under the SAME archive name
        // and overwrite the author's shipped library (its C symbols with
        // it). Under opt-in prebuild this gate was implicit — a bundled
        // package never set `prebuild = true` — with prebuild the default
        // (2026-08-16) it must be said out loud.
        if vm.link.as_ref().is_some_and(|l| !l.bundled.is_empty()) {
            continue;
        }
        if let Err(e) = ensure_one_prebuilt(
            &vm,
            &vendor_dir,
            &link_triple,
            build_mode,
            diag_mode,
            sanitizers,
            stack,
        ) {
            eprintln!("cpc: prebuilding `{}`: {e}", dep.name);
            return Err(ExitCode::FAILURE);
        }
    }
    Ok(())
}

/// Prebuild one package — its own prebuild dependencies FIRST, then its
/// slice. The order is the correctness condition: a slice compiled before
/// its deps' headers exist resolves them from `src/` and swallows their
/// `export extern` definitions, and every consumer linking two such slices
/// dies on the duplicate symbols. `stack` carries the chain for cycle
/// detection — a manifest cycle must be an error, not a hang.
fn ensure_one_prebuilt(
    vm: &manifest::Manifest,
    vendor_dir: &Path,
    link_triple: &str,
    build_mode: BuildMode,
    diag_mode: DiagMode,
    sanitizers: &[&str],
    stack: &mut Vec<String>,
) -> Result<(), String> {
    let name = vm.package.name.clone();
    if stack.contains(&name) {
        return Err(format!(
            "dependency cycle in prebuild: {} -> {name}; mutually-dependent packages cannot be compiled standalone — set `[build] prebuild = false` (or `dev = true`) on one side of the cycle",
            stack.join(" -> ")
        ));
    }
    stack.push(name.clone());
    let deps_first = ensure_prebuilt_deps(vm, build_mode, diag_mode, sanitizers, stack)
        .map_err(|_| format!("a dependency of `{}` failed to prebuild", vm.package.name));
    // The stopwatch starts AFTER the dep walk on purpose: each package times
    // its own slice, so the rows sum instead of nesting.
    let t0 = timings::mark();
    let result = deps_first.and_then(|()| {
        ensure_one_slice(vm, vendor_dir, link_triple, build_mode, diag_mode, sanitizers)
    });
    stack.pop();
    match result {
        Ok(built) => {
            timings::package(&name, t0, built);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Compile one package into `lib/<triple>/<name>.a` if the recorded
/// fingerprint doesn't match the current inputs, and generate its headers
/// alongside. A match is the fast path: nothing runs.
///
/// `Ok(true)` means it compiled, `Ok(false)` that the slice on disk was
/// reused — `--timings` reports the two differently, and a zero-cost row is
/// only readable if it says which one it was.
fn ensure_one_slice(
    vm: &manifest::Manifest,
    vendor_dir: &Path,
    link_triple: &str,
    build_mode: BuildMode,
    diag_mode: DiagMode,
    sanitizers: &[&str],
) -> Result<bool, String> {
    let slice_dir = vendor_dir.join("lib").join(link_triple);
    let archive = slice_dir.join(prebuilt_archive_name(&vm.package.name));
    let stamp = slice_dir.join(format!("{}.fingerprint", vm.package.name));
    let want = prebuild_fingerprint(vendor_dir, link_triple, build_mode, sanitizers)?;
    // What an archive in this directory must BE. The fingerprint is computed
    // from inputs and is structurally blind to the artifact's own bytes, so it
    // cannot notice a slice built for the wrong platform — see `artifact`.
    let want_tag = cplus_core::artifact::expected_tag(&target::active_target());
    let mut wrong_slice: Option<String> = None;
    if archive.is_file() {
        if let Ok(have) = fs::read_to_string(&stamp) {
            if have.trim() == want {
                match (&want_tag, cplus_core::artifact::tag_of_file(&archive)) {
                    // Positively for another target: the fingerprint says
                    // "current" and it is lying. Fall through and rebuild.
                    (Some(w), Some(got)) if &got != w => wrong_slice = Some(got),
                    // Agreed, or could not tell. "Could not tell" must reuse:
                    // rebuilding on it would rebuild every time, forever.
                    _ => return Ok(false),
                }
            }
        }
    }
    if let Some(got) = &wrong_slice {
        eprintln!(
            "cpc: `{}`: the slice in lib/{} is {}, not {} — rebuilding",
            vm.package.name,
            link_triple,
            got,
            want_tag.as_deref().unwrap_or("?"),
        );
    }
    eprintln!(
        "cpc: prebuilding `{}` for {} ({}{})",
        vm.package.name,
        link_triple,
        match build_mode {
            BuildMode::Release => "release",
            _ => "debug",
        },
        // Name the sanitizers in the line that says work is happening: this
        // is the one place a user sees that flipping `--tsan` invalidated
        // every slice, and why the build they expected to be cached is not.
        if sanitizers.is_empty() {
            String::new()
        } else {
            format!(", -fsanitize={}", sanitizers.join(","))
        }
    );
    // Headers first: an archive whose declarations are stale is worse than no
    // archive at all, and this pass is the only thing that keeps them in step.
    generate_headers_for(vendor_dir)?;
    let mut lib = vm
        .lib
        .clone()
        .ok_or_else(|| "`prebuild = true` did not yield a library target".to_string())?;
    if lib.synthesized {
        lib.path = write_package_entry(vendor_dir, &vm.package.name)?;
    }
    // Build into the package's own `target/`, then copy the archive across.
    // Pointing the library pipeline straight at the slice directory also
    // deposits its `.o` and its C header there — half a megabyte of build
    // litter inside what is meant to be the shipped surface.
    let code = build_lib_project(vm, &lib, None, diag_mode, build_mode, true, false, sanitizers);
    timings::report_titled(&format!("prebuild {}", vm.package.name));
    if code != ExitCode::SUCCESS {
        // Leave nothing half-built: a stale archive with a fresh fingerprint
        // would be linked forever after.
        let _ = fs::remove_file(&stamp);
        return Err("build failed".to_string());
    }
    // Read the archive back from the directory `build_lib_project` actually
    // wrote it to. An explicit target gets its own artifact tree —
    // `target/<target-name>/<mode>/` — precisely so a host build and a cross
    // build of one package never overwrite each other; only the host target
    // uses the bare `target/<mode>/`.
    //
    // Reading the host path unconditionally has two failure modes, and the
    // quiet one is the dangerous half: with no host build present the copy
    // fails with ENOENT (`cpc build --target ios-arm64-simulator` of the iOS
    // gallery, where `uikit` had never been built for the host), and WITH one
    // present it copies an `arm64-apple-darwin` archive into the
    // `arm64-apple-ios-simulator` slice slot and stamps it with a valid
    // fingerprint — so the wrong-architecture archive is then reused until
    // something invalidates it.
    let built_mode_dir = match build_mode {
        BuildMode::Release => "release",
        _ => "debug",
    };
    let built_tgt = target::active_target();
    let built = if built_tgt.is_host() {
        vm.root.join("target").join(built_mode_dir)
    } else {
        vm.root
            .join("target")
            .join(built_tgt.name)
            .join(built_mode_dir)
    }
    .join(prebuilt_archive_name(&vm.package.name));
    fs::create_dir_all(&slice_dir).map_err(|e| format!("creating {}: {e}", slice_dir.display()))?;
    fs::copy(&built, &archive)
        .map_err(|e| format!("copying {} to {}: {e}", built.display(), archive.display()))?;
    // Verify what actually landed before stamping it current. A fingerprint is
    // a promise that the archive beside it is usable, and writing one over an
    // artifact built for another platform is how twelve wrong slices went
    // unnoticed until the linker complained three packages away.
    //
    // This is an error rather than a rebuild ON PURPOSE. Reaching here means
    // the pipeline was asked to build for one target and produced another,
    // which no amount of retrying fixes — and a retry would loop, because the
    // reuse check above would reject the result again next time. Stop, with
    // both tags named.
    if let (Some(w), Some(got)) = (&want_tag, cplus_core::artifact::tag_of_file(&archive)) {
        if &got != w {
            let _ = fs::remove_file(&stamp);
            let _ = fs::remove_file(&archive);
            return Err(format!(
                "built for {w} but the archive is {got}\n    built: {}\n    slice: {}",
                built.display(),
                archive.display()
            ));
        }
    }
    fs::write(&stamp, &want).map_err(|e| format!("writing {}: {e}", stamp.display()))?;
    Ok(true)
}

/// Write the entry a `prebuild` package is actually compiled from: one that
/// imports every module in `src/`.
///
/// A package's public surface is its `src/` directory — `cpc headers` emits a
/// declaration file per module there — but a build starts from one entry and
/// only ever reaches that entry's import tree. Where the two disagree the
/// consumer loses: it compiles against a header for a module the archive
/// doesn't contain, and the link fails on symbols nothing defines. `appkit` is
/// the live example, with `appkit_ext.cplus` sitting alongside `appkit.cplus`
/// and imported by neither.
///
/// The file lands in `target/` (already ignored, never scanned by `cpc
/// headers`, which reads `src/` only) and imports by relative path, so it
/// stays inside the package boundary the resolver enforces.
fn write_package_entry(vendor_dir: &Path, pkg: &str) -> Result<PathBuf, String> {
    let src_dir = vendor_dir.join("src");
    let mut modules: Vec<String> = fs::read_dir(&src_dir)
        .map_err(|e| format!("reading {}: {e}", src_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("cplus"))
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    modules.sort();
    // Platform variants: an app build resolves ONE file per module — the
    // resolver's `platform_override` swaps `reactor` for `reactor_linux` on
    // a Linux target — and the archive must contain exactly the same set. A
    // foreign platform's variant compiled in lands its exported C symbols
    // beside the active file's and the merged module fails on the
    // redefinition (stdlib's reactor_{linux,windows} vs reactor was the
    // live case). So: import base names only and let the resolver pick the
    // active variant; keep a suffixed module only when it has no base file
    // (nothing else would reach it) and its platform is the active one.
    // The TEST ROOT is not part of the library. `src/test_main.cplus` is a
    // package's test entry by cpc's own convention (`Manifest::test_entry`),
    // and `cpc test` compiles it directly — so importing it here put every
    // `#[test]` function into the shipped archive (124 of them in `inspector`,
    // 54 in `terminal`) and, worse, made the archive's LINK REQUIREMENTS a
    // function of what the TESTS import. That is how it broke: inspector's
    // suite imports `./appkit` to exercise the macOS overlay, so an iOS slice
    // built from this entry referenced the appkit package — which is a
    // `[macos.dependencies]` and is not linked for iOS — and a facet app on the
    // simulator failed to link on 59 symbols named after a package it never
    // asked for. A consumer cannot import another package's test root, so
    // nothing is lost by leaving it out.
    modules.retain(|m| m != "test_main");

    // WHICH PLATFORM VARIANT IS ACTIVE IS THE RESOLVER'S QUESTION, and asking it
    // here is the fix for a bug where this sweep answered it itself: it compared
    // against a single active suffix, while the resolver walks an ORDERED list
    // whose Android entry falls back to `_linux`. A module existing only as
    // `foo_linux.cplus` was therefore reachable from an app build on Android and
    // missing from a library archive built for it, silently.
    let stems: Vec<String> = modules.clone();
    let platform = target::active_platform();
    modules.retain(|m| cplus_core::resolver::is_active_module(m, &stems, platform));

    let mut text = String::from(
        "// Generated by cpc. The entry a prebuilt package is compiled from:\n         // it imports every module in src/, so the archive covers the whole\n         // package and matches the declarations in lib/include/.\n",
    );
    for (i, m) in modules.iter().enumerate() {
        text.push_str(&format!("import \"../src/{m}\" as m{i};\n"));
    }
    let dir = vendor_dir.join("target");
    fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let path = dir.join(format!("cpc-prebuild-{pkg}.cplus"));
    fs::write(&path, &text).map_err(|e| format!("writing {}: {e}", path.display()))?;
    Ok(path)
}

/// What the cached archive was built from. Any change here rebuilds it:
/// package source, the triple, debug-vs-release, the compiler itself — and the
/// source of every package this one is compiled AGAINST.
///
/// Build mode is inside the fingerprint rather than splitting the cache into
/// per-mode directories, so a package has exactly one slice layout —
/// alternating `cpc build` and `cpc build --release` rebuilds rather than
/// silently linking the other mode's code, which is the bug this replaces.
///
/// THE DEPENDENCY HALF IS LOAD-BEARING, and it was missing until 2026-08-21. A
/// slice carries its own copy of every generic it instantiated — six copies of
/// `Box[flex::Node].drop` across six archives in one iris binary — and each
/// copy holds the field offsets that were current when THAT package was
/// compiled. Hashing a package's own source alone leaves every slice built
/// against an older `flex::Node` "current"; archive linking then takes the
/// first definition that resolves a symbol and ignores the rest, with no
/// duplicate-symbol error, so ONE stale copy serves the whole program. iris
/// built its nodes with the current layout and dropped them with the previous
/// one — every offset from `_rules` up short by 0x18 — and freed a word
/// AppKit's mouse-tracking stack had left behind. It ran a fortnight first:
/// every field the two layouts disagree about is a `Vec` header, and a stale
/// word only aborts when it happens to be non-zero. Filed and resolved as iris
/// `components_done/prebuilt-slice-outlives-its-layout.txt`.
fn prebuild_fingerprint(
    vendor_dir: &Path,
    link_triple: &str,
    build_mode: BuildMode,
    sanitizers: &[&str],
) -> Result<String, String> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let mut h = DefaultHasher::new();
    env!("CARGO_PKG_VERSION").hash(&mut h);
    link_triple.hash(&mut h);
    matches!(build_mode, BuildMode::Release).hash(&mut h);
    // THE SANITIZER SET IS PART OF WHAT A SLICE IS. Without it a `--tsan`
    // build asks for a slice, the fingerprint answers "current", and the
    // UNINSTRUMENTED archive from the last ordinary build is linked — so
    // every race inside `stdlib`, `facet` or any vendored package is
    // invisible to the tool whose entire job is finding them, and invisible
    // in the direction that reads as clean. (Codegen was never the problem:
    // it threads `sanitizer_attrs` correctly. Only this key was blind.)
    //
    // The slice SLOT stays `lib/<triple>/<name>.a`, so an instrumented and an
    // ordinary slice share it and flipping `--tsan` on or off rebuilds every
    // prebuilt dependency. That is deliberate: `build_mode` has exactly this
    // shape and resolves it the same way, and a suffixed slice directory
    // would be a second layout convention for `collect_dep_link_args`,
    // `print_link_args`, the artifact-tag check and every external build
    // system that consumes the printed link line to learn. A sanitizer build
    // is a deliberate, occasional act; paying a rebuild for it is the cheaper
    // side of that trade.
    //
    // Sorted so `--asan --ubsan` and `--ubsan --asan` are one slice.
    let mut sans: Vec<&str> = sanitizers.to_vec();
    sans.sort_unstable();
    sans.hash(&mut h);
    package_input_digest(vendor_dir, &mut Vec::new())?.hash(&mut h);
    Ok(format!("{:016x}", h.finish()))
}

/// One package's inputs folded into a single number: its own `src/`, then the
/// same answer for every dependency active on this platform.
///
/// Memoised for the life of the process. `stdlib` sits under twenty packages
/// and the vendor tree is twenty-odd megabytes of source, so the plain
/// recursion reads it once per consumer. Nothing writes `src/` during a build
/// — `generate_headers_for` writes `lib/include/`, `write_package_entry`
/// writes `target/` — which is what makes the cache sound.
///
/// A CYCLE IS BROKEN, NOT REJECTED. A manifest cycle is legal as long as one
/// side sets `prebuild = false`, which `ensure_one_prebuilt` says in its own
/// error, and a fingerprint has no standing to refuse what the build accepts.
/// Re-entering a package contributes its name and stops. Nothing is cached
/// after a break, because a digest taken across one depends on where the walk
/// entered the cycle and caching it would hand that answer to a different
/// entry.
fn package_input_digest(dir: &Path, stack: &mut Vec<PathBuf>) -> Result<u64, String> {
    use std::collections::hash_map::DefaultHasher;
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    static MEMO: OnceLock<Mutex<HashMap<PathBuf, u64>>> = OnceLock::new();
    static BROKE_A_CYCLE: AtomicBool = AtomicBool::new(false);
    let memo = MEMO.get_or_init(|| Mutex::new(HashMap::new()));

    // Canonical, because one package is reachable under several spellings:
    // `vendor/` carries a symlink loop (`agent_uikit/vendor -> ../../vendor`)
    // and `vendor_dir_for`'s sibling fallback reaches the same directory from
    // inside a vendor package. Two paths to one package have to be one key, or
    // the cycle guard never fires and the cache never hits.
    let key = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    if let Some(hit) = memo.lock().ok().and_then(|m| m.get(&key).copied()) {
        return Ok(hit);
    }
    if stack.contains(&key) {
        BROKE_A_CYCLE.store(true, Ordering::Relaxed);
        let mut h = DefaultHasher::new();
        "cycle".hash(&mut h);
        key.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .hash(&mut h);
        return Ok(h.finish());
    }

    let mut h = DefaultHasher::new();
    let src_dir = dir.join("src");
    // An absent `src/` contributes nothing rather than failing: a package can
    // ship only author-built binaries. Unreadable is still an error — that is
    // a fingerprint the build must not stamp over an archive.
    let mut files: Vec<PathBuf> = match fs::read_dir(&src_dir) {
        Ok(rd) => rd
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("cplus"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("reading {}: {e}", src_dir.display())),
    };
    files.sort();
    for f in &files {
        f.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .hash(&mut h);
        fs::read(f)
            .map_err(|e| format!("reading {}: {e}", f.display()))?
            .hash(&mut h);
    }

    // Dependencies, by name so a reordered `[dependencies]` table is not a
    // rebuild. A dep whose directory is missing or whose manifest will not
    // parse still contributes its NAME — declaring one has to invalidate the
    // slice even before it resolves — and the dep walk reports the absence,
    // which is not this function's job. Inactive platforms are skipped, the
    // same filter `ensure_prebuilt_deps` links by.
    stack.push(key.clone());
    let walked = (|| -> Result<Vec<(String, u64)>, String> {
        let Ok(m) = manifest::load(&dir.join("Cplus.toml")) else {
            return Ok(Vec::new());
        };
        let platform = target::active_platform();
        let mut names: Vec<String> = m
            .dependencies
            .iter()
            .filter(|d| d.active_on(platform))
            .map(|d| d.name.clone())
            .collect();
        names.sort();
        let mut out = Vec::new();
        for name in names {
            let digest = match vendor_dir_for(&m, &name) {
                Some(p) => package_input_digest(&p, stack)?,
                None => 0,
            };
            out.push((name, digest));
        }
        Ok(out)
    })();
    stack.pop();
    walked?.hash(&mut h);

    let digest = h.finish();
    if !BROKE_A_CYCLE.load(Ordering::Relaxed) {
        if let Ok(mut m) = memo.lock() {
            m.insert(key, digest);
        }
    }
    Ok(digest)
}

/// The triple that names a binary slice's directory for the build in progress.
/// Derived, never declared — see `LinkSpec::bundled`.
fn active_link_triple() -> Result<String, ExitCode> {
    match target::active_target().artifact_triple {
        Some(t) => Ok(t.to_string()),
        None => Ok(target::normalize_triple(&detect_host_triple()?)),
    }
}

/// The basename of the archive `[build] prebuild = true` produces for a
/// package. One place, because the dep walk writes it, the orphan check
/// exempts it, and the link line names it. The `lib` prefix is what the
/// library pipeline emits (`ar rcs lib<name>.a`), not a second convention.
fn prebuilt_archive_name(pkg: &str) -> String {
    format!("lib{pkg}.a")
}

/// A dependency's `[link]` contributions that are independent of binary
/// slices: search paths, frameworks, system libs, prebuilt objects. Shared by
/// the normal path and the `dev = true` path, which takes everything here and
/// nothing binary.
fn splice_plain_link_args(
    link_args: &mut Vec<String>,
    vm: &manifest::Manifest,
    diag_mode: DiagMode,
    vendor_manifest: &Path,
) -> Result<(), ExitCode> {
    let Some(ls) = &vm.link else {
        return Ok(());
    };
    // `-L<dir>` must precede the `-l<name>` it resolves; emit search
    // paths first. `-rpath` bakes the same dir into the binary so the
    // loader finds the .so at runtime (no LD_LIBRARY_PATH needed).
    for dir in &ls.search_paths {
        link_args.push(format!("-L{dir}"));
        link_args.push(format!("-Wl,-rpath,{dir}"));
    }
    for fw in &ls.frameworks {
        link_args.push("-framework".to_string());
        link_args.push(fw.clone());
    }
    for l in &ls.libs {
        link_args.push(format!("-l{l}"));
    }
    // v0.0.9 Phase 8 (cpc-gaps G-001): vendor packages may also
    // declare `extra-objects` (rare — usually consumer-side).
    // Validate existence here so the diag carries the dep name.
    for obj in &ls.extra_objects {
        if !obj.is_file() {
            return Err(emit_extra_object_missing(diag_mode, obj, vendor_manifest));
        }
        link_args.push(obj.to_string_lossy().to_string());
    }
    Ok(())
}

/// v0.0.9 Phase 8 (cpc-gaps G-001): produce E0864 ("[link]
/// extra-objects entry not found") as a structured diagnostic.
/// Used both by the dep-walker and by the consumer's own link path.
/// `declared_in` is the manifest that listed the missing file —
/// helps the user find the offending entry quickly.
fn emit_extra_object_missing(diag_mode: DiagMode, obj: &Path, declared_in: &Path) -> ExitCode {
    let d = diag::Diagnostic {
        severity: Severity::Error,
        code: diag::DiagCode("E0864"),
        message: format!("[link] extra-objects entry `{}` not found", obj.display()),
        primary: diag::SourceSpan {
            file: declared_in.to_path_buf(),
            start: diag::Position {
                line: 1,
                col: 1,
                byte: 0,
            },
            end: diag::Position {
                line: 1,
                col: 1,
                byte: 0,
            },
        },
        labels: Vec::new(),
        notes: vec![
            "produce the object out-of-band (e.g. `clang -c foo.s -o foo.o`) before `cpc build`"
                .to_string(),
        ],
        suggestions: Vec::new(),
    };
    emit_diag(&d, diag_mode, "");
    ExitCode::FAILURE
}

/// Multi-file project build (Phase 4 slice 4A). Looks for `Cplus.toml`
/// in the current working directory, walks the import graph from the
/// declared binary entry, and produces a single linked binary at
/// `target/{debug,release}/<bin-name>` (or `-o OUT` if provided).
/// W0005: list every `.cplus` under the package's own `src/` that the build
/// never loaded. An unimported file is invisible — it compiles never and
/// warns never, and an agent READS it as if it described the live API:
/// unreachable code is false evidence, proven the blunt way when a call to
/// an undefined function appended to one still built exit-0. Platform-suffixed
/// siblings (`runtime_linux.cplus` beside a loaded `runtime.cplus`) are the
/// resolver's own convention for "reachable on another target", so they are
/// exempt; everything else unloaded is dead on every target. The scan stays
/// inside `src/` — a vendored dependency legitimately ships more modules than
/// one consumer imports.
fn warn_orphan_sources(loaded: &[PathBuf], root: &Path, diag_mode: DiagMode) {
    let mut loaded_set: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();
    for p in loaded {
        if let Ok(c) = p.canonicalize() {
            loaded_set.insert(c);
        }
    }
    let mut orphans: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root.join("src")];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("cplus") {
                continue;
            }
            let Ok(c) = p.canonicalize() else { continue };
            if loaded_set.contains(&c) {
                continue;
            }
            if let Some(stem) = p.file_stem().and_then(|x| x.to_str()) {
                if target::PLATFORMS
                    .iter()
                    .any(|plat| stem.ends_with(&format!("_{plat}")))
                {
                    continue;
                }
            }
            orphans.push(p);
        }
    }
    orphans.sort();
    for p in orphans {
        let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
        let d = diag::Diagnostic {
            severity: Severity::Warning,
            code: diag::DiagCode("W0005"),
            message: format!(
                "`{}` is not reachable from the entry — it never compiles, and nothing it says is checked",
                rel.display()
            ),
            primary: diag::SourceSpan {
                file: p.clone(),
                start: diag::Position { line: 1, col: 1, byte: 0 },
                end: diag::Position { line: 1, col: 1, byte: 0 },
            },
            labels: Vec::new(),
            notes: vec![
                "an import line from a reachable module is what compiles a file; import it or delete it".to_string(),
            ],
            suggestions: Vec::new(),
        };
        emit_diag(&d, diag_mode, "");
    }
}

fn build_project(
    out: Option<PathBuf>,
    diag_mode: DiagMode,
    build_mode: BuildMode,
    fp_contract: bool,
    sanitizers: &[&str],
) -> ExitCode {
    let manifest_path = PathBuf::from("Cplus.toml");
    let m = match manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            emit_diag(&e.to_diagnostic(), diag_mode, "");
            return ExitCode::FAILURE;
        }
    };
    // Whose warnings this build reports. Set before the prebuild pass below,
    // so a dependency compiled on the way stays a dependency.
    diagpolicy::set_project_root(&m.root);
    // Dependencies that ask to be compiled once (`[build] prebuild = true`)
    // are brought up to date here, BEFORE the entry file is resolved: the
    // resolver reads `lib/include/` for them and the link line names the
    // archive, so both need the slice to already be on disk.
    if let Err(code) = ensure_prebuilt_deps(&m, build_mode, diag_mode, sanitizers, &mut Vec::new())
    {
        return code;
    }
    // A `[library]` (or prebuild-synthesized) target dispatches to the
    // library build path (object → archive / shared-library) instead of the
    // executable path. Mutual exclusion with app entries is enforced at
    // manifest-parse time (E0408).
    if let Some(mut lib) = m.lib.clone() {
        // A synthesized target compiles the whole package, the same way the
        // prebuild pass does — `cpc build` inside the package and a consumer's
        // prebuild of it must not produce different archives. An explicit
        // `[library] entry` keeps its declared entry and its bare C-ABI names.
        let c_abi_entry = !lib.synthesized;
        if lib.synthesized {
            match write_package_entry(&m.root, &m.package.name) {
                Ok(p) => lib.path = p,
                Err(e) => {
                    eprintln!("cpc: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        // `--timings` was silently a no-op on the library path, which is where
        // a `prebuild` compile spends its time — the one build whose cost you
        // most want to see, since the cache exists to avoid paying it again.
        let code = build_lib_project(
            &m,
            &lib,
            out,
            diag_mode,
            build_mode,
            fp_contract,
            c_abi_entry,
            sanitizers,
        );
        timings::report_project(&m.package.name);
        return code;
    }
    // WHAT A BUILD PRODUCES IS THE TARGET'S FACT, not the manifest's. The
    // manifest names the entry per platform; the target's handoff class
    // picks the pipeline: a self-linked platform (macos/linux/windows) gets
    // an executable, an external-builder platform (ios/android/esp32) gets
    // `lib<name>.a` + a C header and Xcode / Gradle / ESP-IDF owns the link.
    let tgt = target::active_target();
    let platform = target::active_platform();
    let entry: PathBuf = match m.entry_for(platform) {
        Some(e) => e,
        // An app that names entries, none of them for this platform: a hard,
        // specific error — never a silent fall-through to library mode.
        None if m.is_app() => {
            let declared = m.entry_platforms();
            let d = diag::Diagnostic {
                severity: Severity::Error,
                code: diag::DiagCode("E0413"),
                message: format!(
                    "`{}` declares no entry for platform `{platform}`",
                    m.package.name
                ),
                primary: diag::SourceSpan {
                    file: manifest_path.clone(),
                    start: diag::Position { line: 1, col: 1, byte: 0 },
                    end: diag::Position { line: 1, col: 1, byte: 0 },
                },
                labels: Vec::new(),
                notes: vec![
                    format!("entries are declared for: {}", declared.join(", ")),
                    format!("add `[{platform}] entry = \"src/...\"` (or a package-level `entry`) to build for this platform"),
                ],
                suggestions: Vec::new(),
            };
            emit_diag(&d, diag_mode, "");
            return ExitCode::FAILURE;
        }
        // No entry anywhere: a library package. `cpc build` archives the
        // whole src/ tree, exactly as a consumer's prebuild pass would.
        None => {
            let mut lib = manifest::LibTarget {
                name: m.package.name.clone(),
                path: PathBuf::new(),
                crate_type: manifest::CrateType::Staticlib,
                synthesized: true,
                frameworks: Vec::new(),
                libs: Vec::new(),
            };
            match write_package_entry(&m.root, &m.package.name) {
                Ok(p) => lib.path = p,
                Err(e) => {
                    eprintln!("cpc: {e}");
                    return ExitCode::FAILURE;
                }
            }
            let code = build_lib_project(
                &m,
                &lib,
                out,
                diag_mode,
                build_mode,
                fp_contract,
                false,
                sanitizers,
            );
            timings::report_project(&m.package.name);
            return code;
        }
    };
    if tgt.handoff == Handoff::ExternalBuilder {
        // The entry's import tree becomes the archive. Qualified names
        // (c_abi_entry = false): nothing consumes the archive's internal
        // names — the external shell calls the entry's `export extern fn`s,
        // which are unmangled by definition and declared in the generated
        // header. The E0409 scan inside rejects a stray `fn main`.
        let lib = manifest::LibTarget {
            name: m.package.name.clone(),
            path: entry,
            crate_type: manifest::CrateType::Staticlib,
            synthesized: false,
            frameworks: Vec::new(),
            libs: Vec::new(),
        };
        let code =
            build_lib_project(&m, &lib, out, diag_mode, build_mode, fp_contract, false, sanitizers);
        timings::report_project(&m.package.name);
        return code;
    }
    if !entry.is_file() {
        // Build E0407 directly here — same structured shape so json/short/human
        // all work uniformly.
        let d = diag::Diagnostic {
            severity: Severity::Error,
            code: diag::DiagCode("E0407"),
            message: format!("app entry `{}` does not exist", entry.display()),
            primary: diag::SourceSpan {
                file: entry.clone(),
                start: diag::Position {
                    line: 1,
                    col: 1,
                    byte: 0,
                },
                end: diag::Position {
                    line: 1,
                    col: 1,
                    byte: 0,
                },
            },
            labels: Vec::new(),
            notes: vec![format!("declared in {}", manifest_path.display())],
            suggestions: Vec::new(),
        };
        emit_diag(&d, diag_mode, "");
        return ExitCode::FAILURE;
    }

    // Phase 2 Slice 2B: thread the manifest's [dependencies] names into
    // the resolver so vendor imports (`utils/math`) resolve under
    // vendor/<dep>/src/. The app entry is the resolver's entry.
    let dep_names: Vec<String> = active_dep_names(&m);
    let (program, _entry_file_id, mono, loaded_paths) = match timings::phase("resolve+sema+borrowck", || {
        load_and_check_project_full(
            &entry,
            &m.root,
            diag_mode,
            false,
            Some(&dep_names),
            m.realtime_profile.as_ref(),
        )
    }) {
        Ok(p) => p,
        Err(code) => return code,
    };
    // E0414: a self-linked platform's entry must define `fn main` — without
    // it the only symptom used to be clang's `undefined symbol: _main`,
    // which names neither the file nor the rule.
    if !program.items.iter().any(|item| {
        matches!(&item.kind, cplus_core::ast::ItemKind::Function(f)
            if f.name.name == "main" && !f.is_extern)
    }) {
        let d = diag::Diagnostic {
            severity: Severity::Error,
            code: diag::DiagCode("E0414"),
            message: format!(
                "entry `{}` defines no `fn main` (platform `{platform}` links an executable)",
                entry.display()
            ),
            primary: diag::SourceSpan {
                file: entry.clone(),
                start: diag::Position { line: 1, col: 1, byte: 0 },
                end: diag::Position { line: 1, col: 1, byte: 0 },
            },
            labels: Vec::new(),
            notes: vec![
                "self-linked platforms (macos, linux, windows) enter through `fn main() -> i32`".to_string(),
                "an `export extern fn` entry is for external-builder platforms (ios, android), where the platform shell calls it".to_string(),
            ],
            suggestions: Vec::new(),
        };
        emit_diag(&d, diag_mode, "");
        return ExitCode::FAILURE;
    }
    // v0.0.3 Phase 5 Slice 5D follow-up: forward --asan/--tsan/--ubsan/
    // --msan through codegen options + clang. Previously `cpc build`
    // silently dropped these flags (always emitted unsanitised IR and
    // linked without `-fsanitize=...`), which meant every e2e ASan
    // test was vacuously clean. The single-file path (`compile_file`)
    // already plumbed sanitizers; this matches.
    warn_orphan_sources(&loaded_paths, &m.root, diag_mode);
    timings::inlined_from(&loaded_paths, &dep_source_dirs(&m));
    ensure_coro_end_probed();
    let ir = timings::phase("codegen", || {
        codegen::generate_with_mono(&program, build_mode, fp_contract, None, sanitizers, false, &mono)
    });
    let ir = timings::phase("prune", || prune_ir(ir));

    let out_path = out.unwrap_or_else(|| {
        let sub = match build_mode {
            BuildMode::Debug => "debug",
            BuildMode::Release => "release",
        };
        m.root.join("target").join(sub).join(&m.package.name)
    });
    if let Some(parent) = out_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("cpc: creating {}: {e}", parent.display());
            return ExitCode::FAILURE;
        }
    }
    let tmp_handle = match make_temp_file("cpc-", ".ll", ir.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc: writing IR to temp file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp = tmp_handle.path().to_path_buf();
    // The app's own link surface is its `[link]` table: `frameworks`
    // expand to `-framework <name>` (macOS/iOS-specific — clang gates the
    // flag), `libs` to `-l<name>` (cross-platform), and `search-paths` go
    // first so `-L<dir>` precedes any `-l<name>` (its own, or a dep's).
    let mut link_args: Vec<String> = Vec::new();
    if let Some(ls) = m.link.as_ref() {
        for dir in &ls.search_paths {
            link_args.push(format!("-L{dir}"));
            link_args.push(format!("-Wl,-rpath,{dir}"));
        }
        for fw in &ls.frameworks {
            link_args.push("-framework".to_string());
            link_args.push(fw.clone());
        }
        for lib in &ls.libs {
            link_args.push(format!("-l{lib}"));
        }
    }
    // Phase 2 Slice 2C: walk dependencies, validate each vendor package's
    // manifest-is-truth contract, and append their `[link]` contributions
    // after the consumer's own. Errors abort the build before clang runs.
    match collect_dep_link_args(&m, diag_mode) {
        Ok(mut extra) => link_args.append(&mut extra),
        Err(code) => return code,
    }
    // v0.0.9 Phase 8 (cpc-gaps G-001): the consumer's own
    // `[link] extra-objects = [...]` — prebuilt `.o` files appended
    // to the link line. Validated against the filesystem at link time
    // so a missing file surfaces as E0864 rather than a clang error.
    // Appended after dep `[link]` contributions so a consumer's `.o`
    // that depends on a vendor lib's symbol resolves correctly.
    if let Some(ls) = m.link.as_ref() {
        for obj in &ls.extra_objects {
            if !obj.is_file() {
                return emit_extra_object_missing(diag_mode, obj, &manifest_path);
            }
            link_args.push(obj.to_string_lossy().to_string());
        }
    }
    let status = timings::phase("clang + link", || {
        run_clang(&tmp, &out_path, build_mode, false, sanitizers, &link_args)
    });
    timings::report_project(&m.package.name);
    drop(tmp_handle); // explicit cleanup on the secure temp path
    if status == ExitCode::SUCCESS {
        // One line on success. The only signal used to be the exit code,
        // which is easy to lose — piping through `tail` reports tail's
        // status, and that produced a confidently wrong "the build is
        // green" at least once. The count is the loaded-file count: the
        // modules this binary was actually built from.
        println!("ok: {} modules -> {}", loaded_paths.len(), out_path.display());
    }
    status
}

/// Phase 5 Slice 5.A: library-build path. Produces `lib<name>.a` and/or
/// `lib<name>.{dylib,so}` in `target/<mode>/`. Reached three ways: a
/// `[library]` target, an entry-less library package, or an app entry
/// built for an external-builder platform.
///
/// Pipeline (mirrors the bin path's structure):
///   1. Load + sema-check the lib root source (via `load_and_check_project_full`).
///   2. Reject `fn main` if defined (E0409) — libraries don't have entry points.
///   3. Emit IR; write IR to temp `.ll`; run `clang -c` → `target/<mode>/<name>.o`.
///   4. For `staticlib` / `both`: `ar rcs target/<mode>/lib<name>.a <name>.o`.
///   5. For `cdylib`   / `both`: `clang -shared <opts> -o target/<mode>/lib<name>.<ext> <name>.o`.
///   6. Manifest `frameworks` / `libs` are forwarded only at the cdylib link
///      step — they don't get into the static archive (consumers re-state them).
/// `c_abi_entry` decides how the entry file's top-level names are spelled, and
/// the two consumers of an archive want opposite answers.
///
/// A declared `[library] entry` exists to be called from C: the entry file's names are
/// the public ABI, so they stay bare and match the generated `.h`. A `[build]
/// prebuild = true` package exists to be linked by another C+ project, which
/// addresses every module the same way — `<package>.src.<module>.<item>` — so
/// its entry must be qualified like any other module. Bare names there produce
/// an archive defining `_answer` against a consumer calling
/// `_mathy.src.mathy.answer`, and the only symptom is an undefined symbol at
/// the consumer's link.
fn build_lib_project(
    m: &manifest::Manifest,
    lib: &manifest::LibTarget,
    out_override: Option<PathBuf>,
    diag_mode: DiagMode,
    build_mode: BuildMode,
    fp_contract: bool,
    c_abi_entry: bool,
    sanitizers: &[&str],
) -> ExitCode {
    if !lib.path.is_file() {
        let d = diag::Diagnostic {
            severity: Severity::Error,
            code: diag::DiagCode("E0407"),
            message: format!("library entry `{}` does not exist", lib.path.display()),
            primary: diag::SourceSpan {
                file: lib.path.clone(),
                start: diag::Position {
                    line: 1,
                    col: 1,
                    byte: 0,
                },
                end: diag::Position {
                    line: 1,
                    col: 1,
                    byte: 0,
                },
            },
            labels: Vec::new(),
            notes: vec!["declared in Cplus.toml".to_string()],
            suggestions: Vec::new(),
        };
        emit_diag(&d, diag_mode, "");
        return ExitCode::FAILURE;
    }
    // v0.0.21 multi-backend slice 1: for an external-builder target the
    // library pipeline is the handoff point — object + static archive only.
    // A cdylib is a *linked* product, and cpc never runs a final link for
    // these targets, so `crate-type = "cdylib"` (or "both") is rejected
    // before any work happens.
    let tgt = target::active_target();
    if tgt.handoff == Handoff::ExternalBuilder
        && matches!(
            lib.crate_type,
            manifest::CrateType::Cdylib | manifest::CrateType::Both
        )
    {
        eprintln!(
            "cpc: target `{}` stops at object emission (the external builder owns the final link); `[library] kind = \"cdylib\"` would require one",
            tgt.name
        );
        eprintln!("    use `kind = \"staticlib\"` and link the archive from the external build system");
        return ExitCode::FAILURE;
    }
    // v0.0.21 rung 2: resolve the target's toolchain before any front-end
    // work, so a missing NDK fails in milliseconds with the setup hint
    // rather than after a full sema + codegen pass.
    let clang_prog = match clang_program_for(&tgt) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("cpc: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let dep_names: Vec<String> = active_dep_names(&m);
    let (program, _entry_file_id, mono, loaded_paths) = match timings::phase("resolve+sema+borrowck", || {
        load_and_check_project_full(
            &lib.path,
            &m.root,
            diag_mode,
            c_abi_entry,
            Some(&dep_names),
            m.realtime_profile.as_ref(),
        )
    }) {
        Ok(p) => p,
        Err(code) => return code,
    };
    timings::inlined_from(&loaded_paths, &dep_source_dirs(&m));

    // Phase 2 Slice 2C: dep walk runs even for library targets — a `.dylib`
    // baked from a package that itself depends on something must record
    // those link args. Static archives can't carry link metadata, but we
    // still validate the dep graph here so any contract violation surfaces
    // at lib build time rather than ambushing the consumer later.
    let dep_link_args: Vec<String> = match collect_dep_link_args(m, diag_mode) {
        Ok(v) => v,
        Err(code) => return code,
    };

    // Reject `fn main` in anything built as a library archive: a `[library]`
    // target, an entry-less library package, or an app entry on an
    // external-builder platform. In all three the archive has no process
    // entry — the consumer (or the platform shell, through an
    // `export extern fn`) owns it. E0409 — enforced here at build time
    // because sema itself doesn't know about manifest mode.
    for item in &program.items {
        if let cplus_core::ast::ItemKind::Function(f) = &item.kind {
            if f.name.name == "main" && !f.is_extern {
                let d = diag::Diagnostic {
                    severity: Severity::Error,
                    code: diag::DiagCode("E0409"),
                    message: "this build produces a library archive — `fn main` has no caller here".to_string(),
                    primary: diag::SourceSpan {
                        file: lib.path.clone(),
                        start: diag::Position { line: 1, col: 1, byte: 0 },
                        end: diag::Position { line: 1, col: 1, byte: 0 },
                    },
                    labels: Vec::new(),
                    notes: vec![
                        "a `[library]` package (and any library archive) leaves the entry point to its consumer".to_string(),
                        "an external-builder platform (ios, android) enters through an `export extern fn` the platform shell calls".to_string(),
                    ],
                    suggestions: Vec::new(),
                };
                emit_diag(&d, diag_mode, "");
                return ExitCode::FAILURE;
            }
        }
    }

    ensure_coro_end_probed();
    // The sanitizer list reaches codegen here. It used to be a hardcoded
    // `&[]`, which made every archive this pipeline produces — every prebuilt
    // slice, every iOS/Android handoff library — uninstrumented no matter what
    // the user asked for.
    let ir = timings::phase("codegen", || {
        codegen::generate_with_mono(
            &program,
            build_mode,
            fp_contract,
            None,
            sanitizers,
            true,
            &mono,
        )
    });
    let ir = timings::phase("prune", || prune_ir(ir));

    let mode_subdir = match build_mode {
        BuildMode::Debug => "debug",
        BuildMode::Release => "release",
    };
    // v0.0.21: explicit targets get their own artifact tree —
    // `target/<target-name>/<mode>/` (the cargo convention) — so a host
    // build and an iOS build of the same package never overwrite each
    // other. The host target keeps `target/<mode>/` byte-for-byte.
    let target_dir = out_override
        .as_ref()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| {
            if tgt.is_host() {
                m.root.join("target").join(mode_subdir)
            } else {
                m.root.join("target").join(tgt.name).join(mode_subdir)
            }
        });
    if let Err(e) = fs::create_dir_all(&target_dir) {
        eprintln!("cpc: creating {}: {e}", target_dir.display());
        return ExitCode::FAILURE;
    }

    // Phase 5 Slice 5.E: emit `target/<mode>/<libname>.h` alongside the
    // build artifacts so consumers can `#include` the generated C
    // declarations without a separate `cpc --emit-header` step.
    let header = render_c_header(&program, &lib.name);
    let header_path = target_dir.join(format!("{}.h", lib.name));
    if let Err(e) = fs::write(&header_path, &header) {
        eprintln!("cpc: writing header to {}: {e}", header_path.display());
        return ExitCode::FAILURE;
    }

    // Step 3: IR → temp .ll → clang -c → <name>.o.
    let tmp_ll_handle = match make_temp_file("cpc-lib-", ".ll", ir.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc: writing IR to temp file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp_ll = tmp_ll_handle.path().to_path_buf();
    let obj_path = target_dir.join(format!("{}.o", lib.name));
    let opt = match build_mode {
        BuildMode::Debug => "-O0",
        BuildMode::Release => "-O3",
    };
    let obj_status = timings::phase("clang -c", || {
        let mut cmd = Command::new(&clang_prog);
        cmd.arg(opt)
            .arg("-Wno-override-module")
            .args(clang_target_args(&tgt));
        // Same forwarding `run_clang` does for an executable: clang owns the
        // instrumentation pass, we name the set. Omitting it here left the
        // object's own code uninstrumented even when the IR carried the
        // function attributes.
        if !sanitizers.is_empty() {
            cmd.arg(format!("-fsanitize={}", sanitizers.join(",")));
            cmd.arg("-fno-omit-frame-pointer");
        }
        cmd.arg("-c").arg(&tmp_ll).arg("-o").arg(&obj_path).status()
    });
    drop(tmp_ll_handle);
    match obj_status {
        Ok(s) if s.success() => {}
        Ok(s) => {
            eprintln!("cpc: clang -c exited with {s}");
            return ExitCode::from(s.code().unwrap_or(1).clamp(1, 255) as u8);
        }
        Err(e) => {
            eprintln!("cpc: failed to invoke clang: {e}");
            return ExitCode::FAILURE;
        }
    }

    // Step 4 (staticlib): ar rcs libNAME.a NAME.o.
    let want_static = matches!(
        lib.crate_type,
        manifest::CrateType::Staticlib | manifest::CrateType::Both
    );
    let want_shared = matches!(
        lib.crate_type,
        manifest::CrateType::Cdylib | manifest::CrateType::Both
    );
    if want_static {
        let a_path = target_dir.join(format!("lib{}.a", lib.name));
        // `r` replace + `c` create-if-missing + `s` index. ar quietly
        // overwrites a previous archive of the same name.
        let _ = fs::remove_file(&a_path); // ar refuses to add a duplicate entry across runs
                                          // Windows/MSVC has no `ar`; LLVM ships `llvm-ar`, which speaks the
                                          // same `rcs` interface. `$CPC_AR` overrides for either host.
                                          // v0.0.21 rung 2: an external toolchain archives with its own
                                          // llvm-ar — macOS's BSD ar can't index ELF members (ranlib skips
                                          // them), leaving an archive the NDK's lld resolves no symbols from.
        let ar_prog = ar_program_for(&tgt, &clang_prog);
        let ar_status = Command::new(&ar_prog)
            .arg("rcs")
            .arg(&a_path)
            .arg(&obj_path)
            .status();
        match ar_status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("cpc: ar exited with {s}");
                return ExitCode::from(s.code().unwrap_or(1).clamp(1, 255) as u8);
            }
            Err(e) => {
                eprintln!("cpc: failed to invoke {ar_prog}: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Step 5 (cdylib): clang -shared -o libNAME.<ext> NAME.o + manifest frameworks/libs.
    if want_shared {
        // Platform-correct extension: .dylib on macOS, .so on Linux/other.
        // (Cross-compilation is out of scope; we use host triple via cfg.)
        let dylib_ext = if cfg!(target_os = "macos") {
            "dylib"
        } else {
            "so"
        };
        let dylib_path = target_dir.join(format!("lib{}.{}", lib.name, dylib_ext));
        let mut cmd = Command::new(clang_program());
        cmd.arg("-shared").arg(opt).arg("-Wno-override-module");
        // A cdylib IS linked here, so it needs the runtime as well as the
        // instrumentation — a `-fsanitize=` on the link line is what pulls in
        // libclang_rt.
        if !sanitizers.is_empty() {
            cmd.arg(format!("-fsanitize={}", sanitizers.join(",")));
            cmd.arg("-fno-omit-frame-pointer");
        }
        for fw in &lib.frameworks {
            cmd.arg("-framework").arg(fw);
        }
        for ll in &lib.libs {
            cmd.arg(format!("-l{ll}"));
        }
        // Phase 2 Slice 2C: forward each transitive dep's link args to the
        // .dylib link line. (Static archives don't carry these — consumers
        // re-walk the graph.)
        for arg in &dep_link_args {
            cmd.arg(arg);
        }
        // v0.0.9 Phase 8 (cpc-gaps G-001): the consumer's own
        // `[link] extra-objects = [...]` bakes into the .dylib so the
        // downstream consumer doesn't have to re-state them. Static
        // archives don't carry link metadata at all, so extra-objects
        // for a staticlib are silently dropped — the consumer's own
        // `[link]` is where they'd be respected anyway.
        if let Some(ls) = m.link.as_ref() {
            for obj in &ls.extra_objects {
                if !obj.is_file() {
                    return emit_extra_object_missing(diag_mode, obj, &m.root.join("Cplus.toml"));
                }
                cmd.arg(obj);
            }
        }
        let dylib_status = cmd.arg(&obj_path).arg("-o").arg(&dylib_path).status();
        match dylib_status {
            Ok(s) if s.success() => {}
            Ok(s) => {
                eprintln!("cpc: clang -shared exited with {s}");
                return ExitCode::from(s.code().unwrap_or(1).clamp(1, 255) as u8);
            }
            Err(e) => {
                eprintln!("cpc: failed to invoke clang -shared: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    ExitCode::SUCCESS
}

/// `--emit-ll-project`: project build, but emit IR to stdout instead of
/// linking. Mirrors the single-file `--emit-ll FILE` flag. Mostly useful
/// for testing.
///
/// A library-shaped project emits the IR the LIBRARY pipeline would compile, not a
/// bin-shaped approximation of it. The two differ in linkage, calling
/// convention and entry-name spelling (`is_lib` / `c_abi_entry`), and emitting
/// bin flags for a lib manifest makes this flag useless for exactly the bug
/// that only appears in an archive.
fn emit_ll_project(diag_mode: DiagMode, build_mode: BuildMode, fp_contract: bool) -> ExitCode {
    let manifest_path = PathBuf::from("Cplus.toml");
    let m = match manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            emit_diag(&e.to_diagnostic(), diag_mode, "");
            return ExitCode::FAILURE;
        }
    };
    // App entries and `[library]` are mutually exclusive in one manifest
    // (E0408), so at most one arm matches. The IR mirrors what `cpc build`
    // would compile FOR THE ACTIVE TARGET: an app entry on an
    // external-builder platform emits library-shaped IR, same as the build.
    let platform = target::active_platform();
    let (entry, is_lib, c_abi_entry) = match (m.entry_for(platform), m.lib.as_ref()) {
        (Some(e), _) => {
            let external = target::active_target().handoff == Handoff::ExternalBuilder;
            (e, external, false)
        }
        // A declared `[library] entry` spells its entry file's names bare —
        // they are the public C ABI. A synthesized one (no `entry`, or a
        // `[build] prebuild` package) is addressed like any other module, so
        // its entry stays qualified. `build_lib_project`'s caller makes the
        // same choice.
        (None, Some(lib)) => (lib.path.clone(), true, !lib.synthesized),
        (None, None) => {
            eprintln!(
                "cpc: --emit-ll-project needs an entry for platform `{platform}` or a `[library]` target"
            );
            return ExitCode::FAILURE;
        }
    };
    // Phase 2 Slice 2C: surface dep walk errors before codegen — the same
    // E0854/E0855/E0860-E0862 checks fire on `--emit-ll-project`, even
    // though no link step runs here. Catches manifest-is-truth violations
    // in CI loops that exercise this flag.
    if let Err(code) = collect_dep_link_args(&m, diag_mode) {
        return code;
    }
    let dep_names: Vec<String> = active_dep_names(&m);
    let (program, _, mono, _loaded_paths) = match load_and_check_project_full(
        &entry,
        &m.root,
        diag_mode,
        c_abi_entry,
        Some(&dep_names),
        m.realtime_profile.as_ref(),
    ) {
        Ok(p) => p,
        Err(code) => return code,
    };
    ensure_coro_end_probed();
    let ir = prune_ir(codegen::generate_with_mono(
        &program, build_mode, fp_contract, None, &[], is_lib, &mono,
    ));
    print!("{ir}");
    ExitCode::SUCCESS
}

/// v0.0.12 realtime Phase 8: synthesize `[profile.realtime]` contract
/// attributes onto every function defined in the entry package. A function is
/// "local" iff its origin file's canonical path lives under the project root
/// but not under `root/vendor` (dependency packages — including symlinked ones
/// that resolve outside the tree — are exempt). Injection is idempotent: an
/// attribute already present (or a `#[realtime]` that bundles it) is left
/// alone, so no E0357 duplicate fires.
fn apply_realtime_profile(
    program: &mut cplus_core::ast::Program,
    files: &std::collections::BTreeMap<String, (PathBuf, String)>,
    root: &Path,
    profile: &cplus_core::manifest::RealtimeProfile,
) {
    use cplus_core::ast::{AttrArg, Attribute, Ident, ItemKind};
    use cplus_core::lexer::Span;

    let canon_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let vendor_dir = canon_root.join("vendor");
    let local: std::collections::HashSet<String> = files
        .iter()
        .filter(|(_, (p, _))| {
            let cp = fs::canonicalize(p).unwrap_or_else(|_| p.clone());
            cp.starts_with(&canon_root) && !cp.starts_with(&vendor_dir)
        })
        .map(|(id, _)| id.clone())
        .collect();

    fn inject(
        attrs: &mut Vec<Attribute>,
        span: Span,
        profile: &cplus_core::manifest::RealtimeProfile,
    ) {
        let has = |n: &str, a: &[Attribute]| a.iter().any(|x| x.path.name == n);
        let bare = |name: &str| Attribute {
            path: Ident {
                name: name.to_string(),
                span,
            },
            args: Vec::new(),
            span,
        };
        if profile.deny_alloc && !has("no_alloc", attrs) && !has("realtime", attrs) {
            attrs.push(bare("no_alloc"));
        }
        if profile.deny_block && !has("no_block", attrs) && !has("realtime", attrs) {
            attrs.push(bare("no_block"));
        }
        if let Some(n) = profile.stack_limit {
            if !has("max_stack", attrs) {
                attrs.push(Attribute {
                    path: Ident {
                        name: "max_stack".to_string(),
                        span,
                    },
                    args: vec![AttrArg::Int(n as i64, span)],
                    span,
                });
            }
        }
    }

    let is_local = |o: &Option<String>| o.as_ref().map(|f| local.contains(f)).unwrap_or(false);

    for item in &mut program.items {
        let origin_local = is_local(&item.origin_file);
        match &mut item.kind {
            ItemKind::Function(f) if origin_local && !f.is_extern => {
                let span = f.name.span;
                inject(&mut f.attributes, span, profile);
            }
            ItemKind::Impl(b) if origin_local => {
                for m in &mut b.methods {
                    let span = m.name.span;
                    inject(&mut m.attributes, span, profile);
                }
            }
            _ => {}
        }
    }
}

/// Phase 2 Slice 2B: variant that passes the consumer's declared
/// `[dependencies]` to the resolver so vendor-mode imports work.
/// `deps = Some(...)` enables strict vendor mode (every bare import
/// must be a declared dep); `None` is legacy single-file mode.
fn load_and_check_project_full(
    entry: &Path,
    root: &Path,
    diag_mode: DiagMode,
    is_lib: bool,
    deps: Option<&[String]>,
    rt_profile: Option<&cplus_core::manifest::RealtimeProfile>,
) -> Result<(cplus_core::ast::Program, String, sema::MonoInfo, Vec<PathBuf>), ExitCode> {
    let mut loaded =
        match resolver::load_project_full(entry, root, is_lib, deps, Default::default()) {
            Ok(l) => l,
            Err(failure) => {
                // Slice 4C tail: render the resolver error as a structured
                // Diagnostic so json/short/human all work the same way as
                // sema diagnostics. Source for the primary span is looked
                // up from the failure's per-file map.
                let d = failure.to_diagnostic();
                let src = failure.primary_source().unwrap_or("");
                emit_diag(&d, diag_mode, src);
                return Err(ExitCode::FAILURE);
            }
        };
    // For sema, pass the entry file's source so diagnostics' line/col
    // mapping comes from there. Cross-file spans currently print without
    // a line map (slice 4A limitation; full per-file source threading is a
    // 4B/4C polish item).
    let entry_src = match fs::read_to_string(entry) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", entry.display());
            return Err(ExitCode::FAILURE);
        }
    };
    // Phase 5 slice 5ATTR.1: validate attributes before lower / sema.
    // Mirrors sema's check_multi entry — per-file source map drives
    // cross-file diagnostic rendering.
    let attr_diags = attrs::check_multi(
        &loaded.program,
        entry.to_path_buf(),
        &entry_src,
        loaded.files.clone(),
    );
    let attr_errors = attr_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &attr_diags {
        emit_diag_multi(d, diag_mode, &entry_src, &loaded.files);
    }
    if attr_errors {
        return Err(ExitCode::FAILURE);
    }
    // v0.0.12 realtime Phase 8: if a `[profile.realtime]` is active,
    // synthesize the contract attributes onto every function defined in
    // *this* package (dependencies are exempt). Runs after attribute
    // validation (the synthesized attrs are valid by construction) and
    // before sema, so the existing no_alloc/no_block/max_stack passes do the
    // enforcement with no special-casing.
    if let Some(profile) = rt_profile {
        apply_realtime_profile(&mut loaded.program, &loaded.files, root, profile);
    }
    // Lower `if let` / `guard let` (slice 4A.5) before sema. GAP 3: hand
    // lower the per-file source map (like attrs / sema) so an E0911 / E0912 in
    // an imported file renders against that file, not the entry file.
    let lower_diags = lower::lower_multi(
        &mut loaded.program,
        entry,
        &entry_src,
        loaded.files.clone(),
    );
    let lower_errors = lower_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &lower_diags {
        emit_diag_multi(d, diag_mode, &entry_src, &loaded.files);
    }
    if lower_errors {
        return Err(ExitCode::FAILURE);
    }
    // Slice 4C: hand sema the per-file source map so cross-file
    // diagnostics render against the right file's line/column. Sema
    // routes via each item's `origin_file`.
    let (diags, mono) = sema::check_multi_with_mono_imports(
        &loaded.program,
        entry.to_path_buf(),
        &entry_src,
        loaded.files.clone(),
        &loaded.imports,
    );
    let had_errors = diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &diags {
        emit_diag_multi(d, diag_mode, &entry_src, &loaded.files);
    }
    if had_errors {
        return Err(ExitCode::FAILURE);
    }
    // Phase 5 borrow checker (slice 5BC.2a — active diagnostics E0370).
    // Runs after sema so it inherits type-correctness assumptions. Routed
    // through the per-file source map (like sema) so a borrow error in an
    // imported module names that module's file, not the entry file.
    let bc_diags =
        borrowck::check_multi(&loaded.program, entry, &entry_src, &loaded.files);
    let bc_errors = bc_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &bc_diags {
        emit_diag_multi(d, diag_mode, &entry_src, &loaded.files);
    }
    if bc_errors {
        return Err(ExitCode::FAILURE);
    }
    // Guard against a self-growing generic (`fn rec[T]() { rec::[*T](); }`)
    // whose instantiation set never converges — monomorphization would hang.
    // Runs the same fixed-point propagation mono uses, capped; a breach is
    // E0910 at the offending template. Must precede monomorphization (which
    // would otherwise loop before any diagnostic could be produced).
    let inst_diags = monomorphize::check_instantiation_bounds(
        &loaded.program,
        &mono,
        entry,
        &entry_src,
        &loaded.files,
    );
    let inst_errors = inst_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &inst_diags {
        emit_diag_multi(d, diag_mode, &entry_src, &loaded.files);
    }
    if inst_errors {
        return Err(ExitCode::FAILURE);
    }
    // Slice 7GEN.5a: monomorphization. Generic-fn templates are
    // replaced by per-instantiation concrete fns; generic call sites
    // are rewritten to mangled names. The result is a Program with no
    // generic items — codegen can consume it directly.
    let post_mono = run_monomorphize(loaded.program, &mono, &loaded.files);
    // The files the build actually compiled — `cpc build`'s success line
    // counts them, and the orphan warning (W0005) subtracts them from what
    // sits under `src/`. Collected here because the per-file map is consumed
    // with `loaded`.
    let loaded_paths: Vec<PathBuf> = loaded.files.values().map(|(p, _)| p.clone()).collect();
    Ok((post_mono, loaded.entry_file_id, mono, loaded_paths))
}

/// Slice 7GEN.5a wrapper: builds the type-name lookup closure from
/// the loaded project's source map and calls
/// `monomorphize::monomorphize`. Sema does not yet maintain a
/// post-pipeline `Ty -> name` map directly; we rebuild it by
/// re-running the relevant collection passes against the program's
/// structs / enums. For 7GEN.5a we only need to render primitives
/// (the dominant case) so a minimal table suffices; struct / enum
/// instantiations land in 7GEN.5b.
fn run_monomorphize(
    program: cplus_core::ast::Program,
    mono: &sema::MonoInfo,
    _files: &std::collections::BTreeMap<String, (PathBuf, String)>,
) -> cplus_core::ast::Program {
    use cplus_core::ast::ItemKind;
    // Build a small struct/enum name table for the type-name
    // closure. Order matches sema's `collect_type_names` so IDs
    // resolve correctly.
    let mut struct_names: Vec<String> = Vec::new();
    let mut enum_names: Vec<String> = Vec::new();
    for item in &program.items {
        match &item.kind {
            ItemKind::Struct(s) if s.generic_params.is_empty() => {
                struct_names.push(s.name.name.clone())
            }
            ItemKind::Enum(e) if e.generic_params.is_empty() => {
                enum_names.push(e.name.name.clone())
            }
            _ => {}
        }
    }
    // 7GEN.5c carry-forward (2026-05-13): generic instantiations live
    // past the non-generic portion of sema's tables. Slot each one at
    // its actual id so `name_of(Ty::Struct(id))` returns the mangled
    // name (was returning "?" — which broke nested-generic lookups in
    // monomorphize like `Pair[Box[T], i32]`).
    for info in mono.struct_instantiations.values() {
        let slot = info.id as usize;
        if struct_names.len() <= slot {
            struct_names.resize(slot + 1, String::from("?"));
        }
        struct_names[slot] = info.mangled_name.clone();
    }
    for info in mono.enum_instantiations.values() {
        let slot = info.id as usize;
        if enum_names.len() <= slot {
            enum_names.resize(slot + 1, String::from("?"));
        }
        enum_names[slot] = info.mangled_name.clone();
    }
    let name_of = move |ty: &sema::Ty| -> String {
        match ty {
            sema::Ty::Struct(id) => struct_names
                .get(id.0 as usize)
                .cloned()
                .unwrap_or_else(|| "?".into()),
            sema::Ty::Enum(id) => enum_names
                .get(id.0 as usize)
                .cloned()
                .unwrap_or_else(|| "?".into()),
            other => other.name().to_string(),
        }
    };
    monomorphize::monomorphize(program, mono, &name_of)
}

/// `cpc fmt` subcommand entry. Slice 4D.
///
/// Modes (mutually exclusive at the semantic level — flags are merely
/// hints; the resolved behavior is picked from these):
///
///   - `--stdin`:    read source from stdin, write formatted to stdout.
///     No file arguments allowed.
///   - `--emit`:     read each file argument, write formatted to stdout.
///     Multiple files are concatenated in order.
///   - `--check`:    read each file argument, exit 1 if formatting would
///     change anything, 0 otherwise. Prints a unified diff
///     per changed file to stderr.
///   - default:      rewrite each file argument in place. A directory
///     argument recurses for `*.cplus` files.
///
/// Lex errors surface as structured `Diagnostic`s via `--diagnostics=...`.
fn run_fmt(paths: Vec<PathBuf>, opts: FmtOpts, diag_mode: DiagMode) -> ExitCode {
    if opts.stdin {
        if !paths.is_empty() {
            eprintln!("cpc fmt: `--stdin` does not accept file arguments");
            return ExitCode::FAILURE;
        }
        use std::io::Read;
        let mut src = String::new();
        if let Err(e) = std::io::stdin().read_to_string(&mut src) {
            eprintln!("cpc fmt: reading stdin: {e}");
            return ExitCode::FAILURE;
        }
        match cpfmt::format_source(&src) {
            Ok(out) => {
                print!("{out}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                let d = e.to_diagnostic(Path::new("<stdin>"), &src);
                emit_diag(&d, diag_mode, &src);
                ExitCode::FAILURE
            }
        }
    } else {
        if paths.is_empty() {
            eprintln!("cpc fmt: needs a file or directory argument (or `--stdin`)");
            return ExitCode::FAILURE;
        }
        let mut files: Vec<PathBuf> = Vec::new();
        for p in &paths {
            collect_cplus_files(p, &mut files);
        }
        if files.is_empty() {
            eprintln!("cpc fmt: no `.cplus` files found");
            return ExitCode::FAILURE;
        }
        let mut had_change = false;
        let mut had_error = false;
        for file in &files {
            let src = match fs::read_to_string(file) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cpc fmt: read {}: {e}", file.display());
                    had_error = true;
                    continue;
                }
            };
            let formatted = match cpfmt::format_source(&src) {
                Ok(s) => s,
                Err(e) => {
                    let d = e.to_diagnostic(file, &src);
                    emit_diag(&d, diag_mode, &src);
                    had_error = true;
                    continue;
                }
            };
            if opts.emit {
                print!("{formatted}");
            } else if opts.check {
                if formatted != src {
                    had_change = true;
                    eprintln!("--- {} (original)", file.display());
                    eprintln!("+++ {} (formatted)", file.display());
                    write_unified_diff(&src, &formatted);
                }
            } else {
                // In-place rewrite, but only when the file actually
                // changes. Avoids touching mtime on already-formatted
                // files (saves rebuild churn in watch-mode IDEs).
                if formatted != src {
                    if let Err(e) = fs::write(file, &formatted) {
                        eprintln!("cpc fmt: write {}: {e}", file.display());
                        had_error = true;
                    }
                }
            }
        }
        if had_error {
            return ExitCode::FAILURE;
        }
        if opts.check && had_change {
            return ExitCode::from(1);
        }
        ExitCode::SUCCESS
    }
}

fn collect_cplus_files(root: &Path, out: &mut Vec<PathBuf>) {
    if root.is_file() {
        if root.extension().and_then(|s| s.to_str()) == Some("cplus") {
            out.push(root.to_path_buf());
        }
        return;
    }
    if root.is_dir() {
        // Hardcoded skip list to match the design note §5.4: don't
        // descend into build / VCS / vendored directories.
        let basename = root.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if matches!(basename, "target" | "node_modules" | ".git") {
            return;
        }
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        // Deterministic order so `--check` output is stable across runs.
        let mut sorted: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        sorted.sort();
        for p in sorted {
            collect_cplus_files(&p, out);
        }
    }
}

/// Minimal unified-diff emitter. Per-line equality only — good enough
/// for `cpc fmt --check`, where the typical diff is small whitespace
/// changes. Not LCS-optimal but the input is at most ~hundreds of lines.
fn write_unified_diff(before: &str, after: &str) {
    let a: Vec<&str> = before.lines().collect();
    let b: Vec<&str> = after.lines().collect();
    let n = a.len().max(b.len());
    for i in 0..n {
        match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) if x == y => eprintln!(" {x}"),
            (Some(x), Some(y)) => {
                eprintln!("-{x}");
                eprintln!("+{y}");
            }
            (Some(x), None) => eprintln!("-{x}"),
            (None, Some(y)) => eprintln!("+{y}"),
            (None, None) => {}
        }
    }
}

/// `cpc test` subcommand (Phase 5 slice 5ATTR.4).
///
/// Modes:
///   - With a FILE argument: single-file test build. Lex/parse/lower/sema/
///     borrowck the file, run attribute validation, discover `#[test]`
///     functions, codegen a test-driver binary, link, run, exit with the
///     binary's exit code (which equals the count of failed tests).
///   - With no FILE: project mode. Reads `./Cplus.toml`, walks imports as
///     `cpc build` does, then everything else mirrors the single-file path.
///
/// `--json` switches the runner's per-test and summary lines to one JSON
/// object per line — same `--diagnostics=json` style; readable by agents.
fn run_test(
    file: Option<PathBuf>,
    opts: TestOpts,
    diag_mode: DiagMode,
    build_mode: BuildMode,
    sanitizers: &[&str],
    // Both of these used to stop at the door: codegen for the test driver
    // hardcoded `&[]` sanitizers and `true` fp_contract, so `cpc test --asan`
    // linked the ASan runtime against uninstrumented IR and
    // `cpc test --fp-contract=off` was ignored.
    // bugs/cpc-test-asan-does-not-instrument.md
    fp_contract: bool,
) -> ExitCode {
    // Named for the package table: `cpc test` reaches here from a manifest
    // (a package suite) or from a bare file, and only the first has a name.
    let mut project_name: Option<String> = None;
    let (program, _src_for_diags, mono, link_args) = match file {
        Some(path) => {
            let src = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cpc test: read {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            let (prog, mono) = match build_program(&path, &src, diag_mode) {
                Ok(p) => p,
                Err(code) => return code,
            };
            (prog, src, mono, Vec::new())
        }
        None => {
            let manifest_path = PathBuf::from("Cplus.toml");
            let m = match manifest::load(&manifest_path) {
                Ok(m) => m,
                Err(e) => {
                    emit_diag(&e.to_diagnostic(), diag_mode, "");
                    return ExitCode::FAILURE;
                }
            };
            // Same ordering rule as `build_project`: a prebuilt dependency's
            // slice must exist before anything resolves an import against it.
            if let Err(code) =
                ensure_prebuilt_deps(&m, build_mode, diag_mode, sanitizers, &mut Vec::new())
            {
                return code;
            }
            // Resolve the test entry: `src/test_main.cplus` first (a
            // dedicated test root means it, even when the package is also an
            // app), then the app entry for this platform, then the
            // `[library]` target, then the `src/<package-name>.cplus` root
            // module — library-only vendor packages commonly declare no
            // target at all, and the fallback lets them still discover and
            // run their `#[test]` fns.
            let (entry_path, is_lib_pkg) =
                match m.test_entry(cplus_core::target::active_platform()) {
                    Some(pair) => pair,
                    None => {
                        eprintln!(
                            "cpc test: no test entry — expected `src/test_main.cplus`, an app entry, a `[library]` target, or `src/{}.cplus`",
                            m.package.name
                        );
                        return ExitCode::FAILURE;
                    }
                };
            // A `[library]`'s own frameworks/libs join the test link line.
            let (fw_list, lib_list) = match m.lib.as_ref() {
                Some(lt) => (lt.frameworks.clone(), lt.libs.clone()),
                None => (Vec::new(), Vec::new()),
            };
            // Phase 2 Slice 2C: validate the dep graph before sema. Tests
            // share the consumer's `[dependencies]`, so a misdeclared
            // vendor package must fail here too — silent success would let
            // bad packages ride into a passing test run.
            if let Err(code) = collect_dep_link_args(&m, diag_mode) {
                return code;
            }
            let dep_names: Vec<String> = active_dep_names(&m);
            let (program, _, mono, loaded_paths) =
                match timings::phase("resolve+sema+borrowck", || {
                    load_and_check_project_full(
                        &entry_path,
                        &m.root,
                        diag_mode,
                        is_lib_pkg,
                        Some(&dep_names),
                        m.realtime_profile.as_ref(),
                    )
                }) {
                    Ok(p) => p,
                    Err(code) => return code,
                };
            timings::inlined_from(&loaded_paths, &dep_source_dirs(&m));
            project_name = Some(m.package.name.clone());
            // G-029: tests must link the same frameworks/libs as a real
            // `cpc build` would — consumer's manifest first, then each
            // dependency's `[link]` contribution. Without this, vendor
            // packages that depend on system frameworks (e.g. metal →
            // Metal/Foundation) can't run their unit tests because
            // selectors resolve to symbols clang never linked.
            let mut la: Vec<String> = Vec::with_capacity(fw_list.len() * 2 + lib_list.len());
            for fw in &fw_list {
                la.push("-framework".to_string());
                la.push(fw.clone());
            }
            for lib in &lib_list {
                la.push(format!("-l{lib}"));
            }
            match collect_dep_link_args(&m, diag_mode) {
                Ok(mut extra) => la.append(&mut extra),
                Err(code) => return code,
            }
            // Vendor-package self-test: when the package under test
            // declares its own `[link]` table (e.g. metal → Metal,
            // Foundation, objc), the consumer-style fw_list/lib_list
            // pass above doesn't see it (those come from a `[library]`
            // target only). Splice in the package's own [link]
            // contributions so tests resolve against the same symbols
            // a real consumer would.
            if let Some(ls) = m.link.as_ref() {
                for fw in &ls.frameworks {
                    la.push("-framework".to_string());
                    la.push(fw.clone());
                }
                for lib in &ls.libs {
                    la.push(format!("-l{lib}"));
                }
                for obj in &ls.extra_objects {
                    if !obj.is_file() {
                        return emit_extra_object_missing(diag_mode, obj, &manifest_path);
                    }
                    la.push(obj.to_string_lossy().to_string());
                }
            }
            let entry_src = fs::read_to_string(&entry_path).unwrap_or_default();
            (program, entry_src, mono, la)
        }
    };
    let tests = attrs::discover_tests(&program);
    if tests.is_empty() {
        if opts.json {
            println!("{{\"passed\":0,\"failed\":0}}");
        } else {
            println!("\ntest result: ok. 0 passed; 0 failed");
        }
        return ExitCode::SUCCESS;
    }
    ensure_coro_end_probed();
    let ir = timings::phase("codegen", || {
        codegen::generate_test_binary(
            &program, build_mode, &tests, opts.json, &mono, fp_contract, sanitizers,
        )
    });
    let ir = timings::phase("prune", || prune_ir(ir));
    // Debug hook: `CPC_TEST_IR=path` writes the generated test-driver IR out.
    // The driver module is otherwise unreachable — it exists only inside this
    // function and its temp file is deleted — so a crash in it (a release-mode
    // trap, say) has nothing to inspect without this.
    if let Ok(dump) = std::env::var("CPC_TEST_IR") {
        if let Err(e) = fs::write(&dump, &ir) {
            eprintln!("cpc test: CPC_TEST_IR write {dump}: {e}");
        }
    }
    let tmp_handle = match make_temp_file("cpc-test-", ".ll", ir.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc test: writing IR to temp file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp = tmp_handle.path().to_path_buf();
    // `into_temp_path()` keeps the unique path (and delete-on-drop) but
    // CLOSES the writable file descriptor. On Linux, exec'ing a file that
    // any process still holds open for writing fails with ETXTBSY ("Text
    // file busy"); macOS does not enforce this. clang reopens this path to
    // write the executable, then we exec it — so we must not be holding a
    // writable handle to it across the exec below.
    let bin_path = match tempfile::Builder::new()
        .prefix("cpc-test-")
        .suffix(".bin")
        .tempfile()
    {
        Ok(h) => h.into_temp_path(),
        Err(e) => {
            eprintln!("cpc test: creating temp binary path: {e}");
            return ExitCode::FAILURE;
        }
    };
    let bin_out = bin_path.to_path_buf();
    // Sanitizers reach the test binary too. They used to be dropped on the
    // floor here (a hardcoded empty slice), so `cpc test --asan` silently ran an
    // uninstrumented binary — the one place where catching UB matters most,
    // since tests are where UB is reachable on demand.
    let clang_status = timings::phase("clang + link", || {
        run_clang(&tmp, &bin_out, build_mode, false, sanitizers, &link_args)
    });
    drop(tmp_handle);
    if !matches!(clang_status, ExitCode::SUCCESS) {
        return clang_status;
    }
    // Before the binary runs: this is what BUILDING the suite cost, and
    // printing it after would bury it under the test output.
    timings::report_project(project_name.as_deref().unwrap_or("this build"));
    // Run the test binary. Its stdout is what `cpc test` prints; its exit
    // code equals the number of failing tests (clamped into [0, 255] so the
    // process-exit-code-as-u8 convention still fits).
    let mut run = Command::new(&bin_out);
    // Stack-use-after-return is instrumented by clang but gated OFF at runtime
    // (`-fsanitize-address-use-after-return=runtime` is the default), so an
    // ASan run does not look for it unless asked. That is the class where a
    // handler bound to a LOCAL outlives its frame — the one E0365 exists to
    // reject — so a sanitizer sweep that cannot see it is missing the failure
    // this project has actually shipped.
    //
    // Measured across all ten packages after the instrumentation fix: no cost
    // outside noise (13.05s -> 13.01s on the slowest suite) and nothing new
    // found. Free coverage, so it is on.
    //
    // A caller's own ASAN_OPTIONS wins — this is a default, not a policy.
    if !sanitizers.is_empty() && std::env::var_os("ASAN_OPTIONS").is_none() {
        run.env("ASAN_OPTIONS", "detect_stack_use_after_return=1");
    }
    let status = run.status();
    drop(bin_path);
    match status {
        Ok(s) => {
            // The driver `main` returns the failure count. Map any non-zero
            // back to a clamped u8 ExitCode so callers can distinguish
            // "all passed" (0) from "something failed" (1..=255).
            if let Some(code) = s.code() {
                if code == 0 {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::from(code.clamp(1, 255) as u8)
                }
            } else {
                // No exit code means the process was killed by a signal, which
                // is NOT a failing test — it is a crash, usually before the
                // driver printed anything. `unwrap_or(1)` used to collapse that
                // into a bare "1", indistinguishable from "one test failed" and
                // carrying no output at all, which is exactly how a crashing
                // release test binary looked like an unsupported flag.
                #[cfg(unix)]
                {
                    use std::os::unix::process::ExitStatusExt;
                    if let Some(sig) = s.signal() {
                        eprintln!(
                            "cpc test: the test binary was killed by signal {sig} before it finished"
                        );
                        eprintln!(
                            "    no test output means it died during discovery or in the first test"
                        );
                    }
                }
                #[cfg(not(unix))]
                eprintln!("cpc test: the test binary terminated abnormally");
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("cpc test: failed to invoke test binary: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Single-file program build (lex/parse/lower/sema/borrowck) returning the
/// AST `Program` rather than emitting IR. The `build_ir` path inlines the
/// final codegen step; for `cpc test` we want the same pipeline minus codegen
/// because codegen here is `generate_test_binary` instead of `generate`.
fn build_program(
    file: &Path,
    src: &str,
    mode: DiagMode,
) -> Result<(cplus_core::ast::Program, sema::MonoInfo), ExitCode> {
    let extracted = doctest::extract(src);
    let src = extracted.as_str();
    let toks = match lexer::tokenize(src) {
        Ok(t) => t,
        Err(e) => {
            let lm = LineMap::new(src);
            let d = diag::from_lex(&e, file, &lm, src);
            emit_diag(&d, mode, src);
            return Err(ExitCode::FAILURE);
        }
    };
    let mut prog = match parser::parse(toks) {
        Ok(p) => p,
        Err(e) => {
            let lm = LineMap::new(src);
            let d = diag::from_parse(&e, file, &lm, src);
            emit_diag(&d, mode, src);
            return Err(ExitCode::FAILURE);
        }
    };
    let attr_diags = attrs::check(&prog, file.to_path_buf(), src);
    let attr_errors = attr_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &attr_diags {
        emit_diag(d, mode, src);
    }
    if attr_errors {
        return Err(ExitCode::FAILURE);
    }
    let lower_diags = lower::lower(&mut prog, file, src);
    let lower_errors = lower_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &lower_diags {
        emit_diag(d, mode, src);
    }
    if lower_errors {
        return Err(ExitCode::FAILURE);
    }
    let (diags, mono) = sema::check_multi_with_mono(
        &prog,
        file.to_path_buf(),
        src,
        std::collections::BTreeMap::new(),
    );
    let had_errors = diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &diags {
        emit_diag(d, mode, src);
    }
    if had_errors {
        return Err(ExitCode::FAILURE);
    }
    let bc_diags = borrowck::check(&prog, file, src);
    let bc_errors = bc_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &bc_diags {
        emit_diag(d, mode, src);
    }
    if bc_errors {
        return Err(ExitCode::FAILURE);
    }
    // Slice 7GEN.5a: monomorphize generic-fn templates into concrete
    // per-instantiation fns before codegen sees the program.
    let post_mono = run_monomorphize(prog, &mono, &std::collections::BTreeMap::new());
    Ok((post_mono, mono))
}

/// `cpc lsp` — find and exec the `cpc-lsp` binary, forwarding the rest
/// of argv. Looks in the same directory as `cpc` first (handles the
/// in-tree `cargo run` and `cargo install` cases where both binaries
/// live side by side), then falls back to PATH. Slice 4E.1.
/// Phase 11 polish (2026-05-14): `cpc check FILE` — parse + sema +
/// borrowck, no codegen. The advertised "fast feedback loop" command:
/// runs the same diagnostic pipeline as a full compile but stops short
/// of LLVM emission, so it's significantly faster on large files. Exit
/// code matches diagnostics: 0 if clean, 1 if any error emitted.
/// v0.0.12 realtime Phase 8: `cpc check` with no FILE — project-mode
/// verification. Loads `./Cplus.toml`, resolves the entry like `cpc test`,
/// runs the full front-end (incl. any `[profile.realtime]` enforcement)
/// through sema/borrowck, and stops before codegen. The fast CI gate for a
/// whole package: exit 0 iff clean. Diagnostics honor `--json`.
/// `cpc headers` — regenerate `lib/include/` from `src/`.
///
/// The header is what a consumer compiles against when the package ships as a
/// binary. Generating it from source (rather than maintaining it by hand) is
/// what keeps the two from drifting: a signature can only be wrong here if it
/// is wrong in `src/` too.
fn run_headers() -> ExitCode {
    let manifest_path = PathBuf::from("Cplus.toml");
    let m = match manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            emit_diag(&e.to_diagnostic(), DiagMode::Human, "");
            return ExitCode::FAILURE;
        }
    };
    match generate_headers_for(&m.root) {
        Ok(r) => {
            println!(
                "cpc: wrote {} headers to {} ({} declarations, {} verbatim [generic])",
                r.total,
                m.root.join("lib").join("include").display(),
                r.stripped,
                r.verbatim
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cpc: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Outcome of a header-generation pass over one package.
struct HeaderRun {
    total: usize,
    stripped: usize,
    verbatim: usize,
}

/// Generate `lib/include/` from `src/` for the package rooted at `root`.
///
/// Split out of `run_headers` because `prebuild` needs the same pass: an
/// archive without headers is unusable, so the cache builds both together and
/// they cannot drift apart.
fn generate_headers_for(root: &Path) -> Result<HeaderRun, String> {
    let src_dir = root.join("src");
    if !src_dir.is_dir() {
        return Err(format!("no `src/` directory in {}", root.display()));
    }
    let include_dir = root.join("lib").join("include");
    fs::create_dir_all(&include_dir)
        .map_err(|e| format!("creating {}: {e}", include_dir.display()))?;

    let mut entries: Vec<PathBuf> = fs::read_dir(&src_dir)
        .map_err(|e| format!("reading {}: {e}", src_dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("cplus"))
        .collect();
    entries.sort();

    let (mut stripped, mut verbatim) = (0usize, 0usize);
    for path in &entries {
        let src =
            fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
        let (text, kind) = cplus_core::header::generate(&src)
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let Some(name) = path.file_name() else {
            continue;
        };
        let out = include_dir.join(name);
        fs::write(&out, &text).map_err(|e| format!("writing {}: {e}", out.display()))?;
        match kind {
            cplus_core::header::HeaderKind::Stripped => stripped += 1,
            cplus_core::header::HeaderKind::VerbatimGeneric => verbatim += 1,
        }
    }
    Ok(HeaderRun {
        total: entries.len(),
        stripped,
        verbatim,
    })
}

fn run_check_project(diag_mode: DiagMode) -> ExitCode {
    let manifest_path = PathBuf::from("Cplus.toml");
    let m = match manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            emit_diag(&e.to_diagnostic(), diag_mode, "");
            return ExitCode::FAILURE;
        }
    };
    let (entry_path, is_lib_pkg) = match resolve_project_entry(&m, "cpc check") {
        Ok(v) => v,
        Err(code) => return code,
    };
    if let Err(code) = collect_dep_link_args(&m, diag_mode) {
        return code;
    }
    let dep_names: Vec<String> = active_dep_names(&m);
    match load_and_check_project_full(
        &entry_path,
        &m.root,
        diag_mode,
        is_lib_pkg,
        Some(&dep_names),
        m.realtime_profile.as_ref(),
    ) {
        Ok(_) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// Shared whole-project entry resolution for `cpc check` / `--realtime-report`:
/// the manifest's source-entry ladder (app entry for the active platform,
/// `[library]`, `src/test_main.cplus`, then the `src/<package-name>.cplus`
/// root-module fallback for library-only packages). `ctx` is the command
/// label used in error messages.
fn resolve_project_entry(m: &manifest::Manifest, ctx: &str) -> Result<(PathBuf, bool), ExitCode> {
    match m.resolve_source_entry(cplus_core::target::active_platform()) {
        Some(pair) => Ok(pair),
        None => {
            eprintln!(
                "{ctx}: no source entry — expected an app entry, a `[library]` target, `src/test_main.cplus`, or `src/{}.cplus`",
                m.package.name
            );
            Err(ExitCode::FAILURE)
        }
    }
}

/// v0.0.13 (topic C tail): `--realtime-report[=json]`. Runs the whole-project
/// front-end (reads `Cplus.toml`, applies `[profile.realtime]`, lowers, sema-
/// checks) and prints a digest of the real-time contract analysis: which
/// functions carry a contract, and every E0901 (`#[no_alloc]`) / E0907
/// (`#[no_block]`) / E0906 (`#[bounded_recursion]`) / E0908 (`#[max_stack]`)
/// violation, grouped by contract. `cpc check` already *gates* the build; this
/// is the machine-readable summary view deferred from real-time Phase 8.
///
/// Exits non-zero when any contract violation (or other front-end error) is
/// present, so CI can use it as a gate that also produces an artifact.
fn run_realtime_report(json: bool) -> ExitCode {
    use cplus_core::ast::{Attribute, ItemKind};

    let manifest_path = PathBuf::from("Cplus.toml");
    let m = match manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            emit_diag(&e.to_diagnostic(), DiagMode::Human, "");
            return ExitCode::FAILURE;
        }
    };
    let (entry_path, is_lib_pkg) = match resolve_project_entry(&m, "cpc --realtime-report") {
        Ok(v) => v,
        Err(code) => return code,
    };
    let dep_names: Vec<String> = active_dep_names(&m);
    let mut loaded = match resolver::load_project_full(
        &entry_path,
        &m.root,
        is_lib_pkg,
        Some(&dep_names),
        Default::default(),
    ) {
        Ok(l) => l,
        Err(failure) => {
            emit_diag(
                &failure.to_diagnostic(),
                DiagMode::Human,
                failure.primary_source().unwrap_or(""),
            );
            return ExitCode::FAILURE;
        }
    };
    let entry_src = fs::read_to_string(&entry_path).unwrap_or_default();

    // Attributes must validate before we can trust the contract markers.
    let attr_diags = attrs::check_multi(
        &loaded.program,
        entry_path.clone(),
        &entry_src,
        loaded.files.clone(),
    );
    if attr_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error))
    {
        for d in &attr_diags {
            emit_diag(d, DiagMode::Human, &entry_src);
        }
        return ExitCode::FAILURE;
    }
    // Synthesize the profile contracts onto local functions, exactly as the
    // real build does, so the report reflects the project's actual gate.
    if let Some(profile) = m.realtime_profile.as_ref() {
        apply_realtime_profile(&mut loaded.program, &loaded.files, &m.root, profile);
    }
    let lower_diags = lower::lower_multi(
        &mut loaded.program,
        &entry_path,
        &entry_src,
        loaded.files.clone(),
    );
    if lower_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error))
    {
        for d in &lower_diags {
            emit_diag(d, DiagMode::Human, &entry_src);
        }
        return ExitCode::FAILURE;
    }
    // Run sema and KEEP the diagnostics (don't early-return on errors — the
    // whole point is to surface the contract violations).
    let (diags, _mono) = sema::check_multi_with_mono_imports(
        &loaded.program,
        entry_path.clone(),
        &entry_src,
        loaded.files.clone(),
        &loaded.imports,
    );

    // Map a real-time diagnostic code to its contract name.
    fn contract_of(code: &str) -> Option<&'static str> {
        match code {
            "E0901" => Some("no_alloc"),
            "E0907" => Some("no_block"),
            "E0906" => Some("bounded_recursion"),
            "E0908" => Some("max_stack"),
            _ => None,
        }
    }
    let violations: Vec<_> = diags
        .iter()
        .filter(|d| contract_of(d.code.0).is_some())
        .collect();
    let other_errors = diags
        .iter()
        .filter(|d| matches!(d.severity, Severity::Error) && contract_of(d.code.0).is_none())
        .count();
    let count = |c: &str| {
        violations
            .iter()
            .filter(|d| contract_of(d.code.0) == Some(c))
            .count()
    };

    // Count functions/methods carrying at least one real-time contract.
    fn has_rt(attrs: &[Attribute]) -> bool {
        attrs.iter().any(|a| {
            matches!(
                a.path.name.as_str(),
                "no_alloc" | "no_block" | "bounded_recursion" | "realtime" | "max_stack"
            )
        })
    }
    let mut covered = 0usize;
    for item in &loaded.program.items {
        match &item.kind {
            ItemKind::Function(f) if has_rt(&f.attributes) => covered += 1,
            ItemKind::Impl(b) => {
                for mth in &b.methods {
                    if has_rt(&mth.attributes) {
                        covered += 1;
                    }
                }
            }
            _ => {}
        }
    }

    if json {
        let viol_json: Vec<serde_json::Value> = violations
            .iter()
            .map(|d| {
                serde_json::json!({
                    "code": d.code.0,
                    "contract": contract_of(d.code.0).unwrap(),
                    "message": d.message,
                    "file": d.primary.file.display().to_string(),
                    "line": d.primary.start.line,
                    "col": d.primary.start.col,
                })
            })
            .collect();
        let profile_json = m.realtime_profile.as_ref().map(|p| {
            serde_json::json!({
                "deny_alloc": p.deny_alloc,
                "deny_block": p.deny_block,
                "deny_unknown_extern": p.deny_unknown_extern,
                "stack_limit": p.stack_limit,
            })
        });
        let report = serde_json::json!({
            "kind": "realtime-report",
            "profile": profile_json,
            "functions_under_contract": covered,
            "summary": {
                "no_alloc": count("no_alloc"),
                "no_block": count("no_block"),
                "bounded_recursion": count("bounded_recursion"),
                "max_stack": count("max_stack"),
                "total": violations.len(),
            },
            "other_errors": other_errors,
            "violations": viol_json,
            "clean": violations.is_empty() && other_errors == 0,
        });
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    } else {
        println!("real-time report — {}", m.package.name);
        match m.realtime_profile.as_ref() {
            Some(p) => println!(
                "  profile: deny_alloc={} deny_block={} deny_unknown_extern={} stack_limit={}",
                p.deny_alloc,
                p.deny_block,
                p.deny_unknown_extern,
                p.stack_limit
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "none".to_string())
            ),
            None => println!("  profile: (none — per-function contracts only)"),
        }
        println!("  functions under contract: {covered}");
        println!(
            "  violations: {} (no_alloc={}, no_block={}, bounded_recursion={}, max_stack={})",
            violations.len(),
            count("no_alloc"),
            count("no_block"),
            count("bounded_recursion"),
            count("max_stack")
        );
        for d in &violations {
            println!(
                "    [{}] {} {}:{}:{}: {}",
                contract_of(d.code.0).unwrap(),
                d.code.0,
                d.primary.file.display(),
                d.primary.start.line,
                d.primary.start.col,
                d.message
            );
        }
        if violations.is_empty() && other_errors == 0 {
            println!("  clean");
        }
        if other_errors > 0 {
            println!(
                "  note: {other_errors} other front-end error(s) — run `cpc check` for details"
            );
        }
    }

    if violations.is_empty() && other_errors == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Load + resolve the current project (mirrors `cpc check`'s entry
/// resolution), returning the resolved program for graph construction. On any
/// failure it renders a diagnostic and returns the exit code to bubble up.
fn load_project_for_graph(diag_mode: DiagMode) -> Result<resolver::LoadedProject, ExitCode> {
    match load_project_for_graph_with(&Default::default()) {
        Ok(loaded) => Ok(loaded),
        Err(GraphLoadError::Diagnostic(d)) => {
            emit_diag(&d, diag_mode, "");
            Err(ExitCode::FAILURE)
        }
        Err(GraphLoadError::Message(msg)) => {
            eprintln!("cpc: {msg}");
            Err(ExitCode::FAILURE)
        }
    }
}

/// Why a graph load failed, kept structured rather than printed.
///
/// `cpc query` prints it and exits; `cpc mcp` has to *survive* it — a resident
/// server reloading a buffer the user is halfway through typing will fail to
/// parse constantly, and the right answer there is to keep the last good graph
/// and hand the client the reason, not to go dark.
enum GraphLoadError {
    // Boxed: a `Diagnostic` carries its spans and notes, and this is the cold
    // path of a `Result` the hot path returns a whole project through.
    Diagnostic(Box<Diagnostic>),
    Message(String),
}

impl GraphLoadError {
    /// One line, for a JSON-RPC reply. `render_short` is the diagnostic form
    /// that already fits on one.
    fn to_line(&self) -> String {
        match self {
            GraphLoadError::Diagnostic(d) => d.render_short(),
            GraphLoadError::Message(m) => m.clone(),
        }
    }
}

/// The reloadable half of `load_project_for_graph`: re-reads `Cplus.toml` and
/// re-resolves from disk, with `overlays` (canonical path → unsaved buffer)
/// standing in for the files an editor has dirty.
///
/// Re-reading the manifest each time is deliberate — a dependency added while
/// the server is resident should take effect on the next reload.
fn load_project_for_graph_with(
    overlays: &std::collections::BTreeMap<PathBuf, String>,
) -> Result<resolver::LoadedProject, GraphLoadError> {
    let manifest_path = PathBuf::from("Cplus.toml");
    let m = manifest::load(&manifest_path)
        .map_err(|e| GraphLoadError::Diagnostic(Box::new(e.to_diagnostic())))?;
    let (entry_path, is_lib_pkg) = match m
        .resolve_source_entry(cplus_core::target::active_platform())
    {
        Some(pair) => pair,
        None => {
            return Err(GraphLoadError::Message(format!(
                    "no source entry — expected an app entry, a `[library]` target, `src/test_main.cplus`, or `src/{}.cplus`",
                    m.package.name
                )));
        }
    };
    let dep_names: Vec<String> = active_dep_names(&m);
    resolver::load_project_full(
        &entry_path,
        &m.root,
        is_lib_pkg,
        Some(&dep_names),
        overlays.clone(),
    )
    .map_err(|e| GraphLoadError::Diagnostic(Box::new(e.to_diagnostic())))
}

/// `cpc graph` — build the project's code knowledge graph and print it as JSON
/// (nodes + edges) on stdout.
fn run_graph(diag_mode: DiagMode) -> ExitCode {
    let loaded = match load_project_for_graph(diag_mode) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let g = cplus_core::graph::CodeGraph::build(&loaded);
    println!("{}", g.to_json());
    ExitCode::SUCCESS
}

/// `cpc mcp` — build the project's code graph once, then serve it over MCP
/// (stdio JSON-RPC) until stdin closes. Resident: the graph stays warm for the
/// whole session.
fn run_mcp(diag_mode: DiagMode) -> ExitCode {
    let t = std::time::Instant::now();
    let loaded = match load_project_for_graph(diag_mode) {
        Ok(l) => l,
        Err(code) => return code,
    };
    mcp::serve(loaded, t.elapsed().as_millis(), |overlays| {
        load_project_for_graph_with(overlays).map_err(|e| e.to_line())
    })
}

/// `FILE:LINE:COL` → the file id, that file's source, and the byte offset.
///
/// Split from the right so the path may itself contain colons, and resolved
/// through the same `find_file` the resident server uses, so a file id
/// (`src.services.cpc`) works here too. Prints its own diagnosis and hands back
/// the exit code, because every caller does the same thing with a bad position.
fn parse_position<'a>(
    kind: &str,
    pos: Option<&str>,
    loaded: &'a resolver::LoadedProject,
) -> Result<(String, &'a str, u32), ExitCode> {
    let Some(pos) = pos else {
        eprintln!("cpc query {kind}: expected FILE:LINE:COL");
        return Err(ExitCode::FAILURE);
    };
    let parts: Vec<&str> = pos.rsplitn(3, ':').collect(); // [col, line, file]
    if parts.len() != 3 {
        eprintln!("cpc query {kind}: expected FILE:LINE:COL (got `{pos}`)");
        return Err(ExitCode::FAILURE);
    }
    let (Ok(col), Ok(line)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
        eprintln!("cpc query {kind}: LINE and COL must be numbers");
        return Err(ExitCode::FAILURE);
    };
    let file = parts[2];
    let Some((fid, (_, src))) = cplus_core::session::find_file(loaded, file) else {
        eprintln!("cpc query {kind}: no source file matching `{file}`");
        return Err(ExitCode::FAILURE);
    };
    let Some(byte) = cplus_core::graph::byte_offset(src, line, col) else {
        eprintln!("cpc query {kind}: position {line}:{col} is out of range");
        return Err(ExitCode::FAILURE);
    };
    Ok((fid.clone(), src.as_str(), byte))
}

/// `cpc query <kind> [args...]` — answer one graph query as JSON on stdout.
/// Exit code signals found (0) vs not-found (1), per plan.graph.md §6. This
/// build ships the Phase 1 index: `def`, `members`, `symbols`. Call /
/// reference / type queries land in later phases and report so explicitly.
fn run_query(kind: Option<String>, args: Vec<String>, diag_mode: DiagMode) -> ExitCode {
    let Some(kind) = kind else {
        eprintln!(
            "cpc query: expected a query kind (def | members | symbols | refs | callers | \
             callees | call-hierarchy | context | type-at | value-refs | scope-at | complete)"
        );
        return ExitCode::FAILURE;
    };
    let loaded = match load_project_for_graph(diag_mode) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let g = cplus_core::graph::CodeGraph::build(&loaded);
    let arg0 = args.first().map(|s| s.as_str());
    let result = match kind.as_str() {
        "def" => {
            let Some(sym) = arg0 else {
                eprintln!("cpc query def: expected a SYMBOL");
                return ExitCode::FAILURE;
            };
            g.def(sym)
        }
        "members" => {
            let Some(ty) = arg0 else {
                eprintln!("cpc query members: expected a TYPE");
                return ExitCode::FAILURE;
            };
            g.members(ty)
        }
        "symbols" => g.symbols(arg0),
        "callers" | "callees" => {
            let Some(sym) = arg0 else {
                eprintln!("cpc query {kind}: expected a FN");
                return ExitCode::FAILURE;
            };
            let out = if kind == "callers" {
                g.callers_json(sym)
            } else {
                g.callees_json(sym)
            };
            return match out {
                Some(j) => {
                    println!("{j}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("cpc query {kind}: `{sym}` is not a known function or method");
                    ExitCode::FAILURE
                }
            };
        }
        "call-hierarchy" => {
            let Some(sym) = arg0 else {
                eprintln!("cpc query call-hierarchy: expected a FN");
                return ExitCode::FAILURE;
            };
            // `--depth N` (default 3) is appended to args by the CLI parser.
            let mut depth: u32 = 3;
            let mut it = args.iter();
            while let Some(a) = it.next() {
                if a == "--depth" {
                    if let Some(v) = it.next() {
                        depth = v.parse().unwrap_or(3);
                    }
                }
            }
            return match g.call_hierarchy_json(sym, depth) {
                Some(j) => {
                    println!("{j}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!(
                        "cpc query call-hierarchy: `{sym}` is not a known function or method"
                    );
                    ExitCode::FAILURE
                }
            };
        }
        "refs" => {
            let Some(sym) = arg0 else {
                eprintln!("cpc query refs: expected a SYMBOL");
                return ExitCode::FAILURE;
            };
            return match g.refs_json(sym) {
                Some(j) => {
                    println!("{j}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("cpc query refs: `{sym}` is not a known symbol");
                    ExitCode::FAILURE
                }
            };
        }
        "context" => {
            let Some(sym) = arg0 else {
                eprintln!("cpc query context: expected a FN");
                return ExitCode::FAILURE;
            };
            return match g.context_json(sym) {
                Some(j) => {
                    println!("{j}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!("cpc query context: `{sym}` is not a known function or method");
                    ExitCode::FAILURE
                }
            };
        }
        "type-at" => {
            let (fid, _src, byte) = match parse_position("type-at", arg0, &loaded) {
                Ok(p) => p,
                Err(code) => return code,
            };
            return match g.type_at_json(&fid, byte) {
                Some(j) => {
                    println!("{j}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!(
                        "cpc query type-at: no typed node at {} \
                         (type-at resolves params, fields, locals, `self`, and inferred \
                         expressions — call results, field/index reads, match/if values)",
                        arg0.unwrap_or("")
                    );
                    ExitCode::FAILURE
                }
            };
        }
        "value-refs" => {
            let (fid, _src, byte) = match parse_position("value-refs", arg0, &loaded) {
                Ok(p) => p,
                Err(code) => return code,
            };
            return match g.value_refs_json(&fid, byte) {
                Some(j) => {
                    println!("{j}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!(
                        "cpc query value-refs: no local binding at {} \
                         (value-refs resolves a parameter or `let`, then its classified uses)",
                        arg0.unwrap_or("")
                    );
                    ExitCode::FAILURE
                }
            };
        }
        "scope-at" => {
            let (fid, _src, byte) = match parse_position("scope-at", arg0, &loaded) {
                Ok(p) => p,
                Err(code) => return code,
            };
            println!("{}", g.scope_at_json(&fid, byte));
            return ExitCode::SUCCESS;
        }
        "complete" => {
            // The composed verb: one question, one answer. `scope-at`,
            // `type-at` and `members` stay as the primitives underneath, but a
            // caller completing at a caret should not have to decide which of
            // the three this caret is asking — that decision is C+'s rules, not
            // the caller's policy.
            let (fid, src, byte) = match parse_position("complete", arg0, &loaded) {
                Ok(p) => p,
                Err(code) => return code,
            };
            println!("{}", g.complete_at_json(&fid, src, byte));
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!(
                "cpc query: unknown query kind `{other}` (expected: def | members | symbols | \
                 refs | callers | callees | call-hierarchy | context | type-at | value-refs | \
                 scope-at | complete)"
            );
            return ExitCode::FAILURE;
        }
    };
    println!("{}", cplus_core::graph::CodeGraph::nodes_to_json(&result));
    if result.is_empty() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn run_check(path: PathBuf, mode: DiagMode) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    // Reuse the same build pipeline up through borrowck. `build_ir`
    // already handles lex/parse/attrs/lower/sema/borrowck and returns
    // either the IR string (which we discard) or an ExitCode on any
    // error. No need to invoke clang. `debug_info=false`, no sanitizers
    // — `check` is purely diagnostic.
    match build_ir(&path, &src, mode, BuildMode::Debug, true, false, &[]) {
        Ok(_ir) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// Phase 11 polish (2026-05-14): `cpc doc FILE` — extract public
/// (non-`_`-private) items + their `///` docs from FILE, emit Markdown to
/// `target/doc/<basename>.md`. Output directory is created if needed.
/// Prints the destination path to stdout so users + scripts can find
/// the result.
fn run_doc(path: PathBuf) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let basename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("source.cplus");
    let items = cplus_core::docgen::extract(&src);
    let md = cplus_core::docgen::render_markdown(basename, &items);
    let out_dir = PathBuf::from("target/doc");
    if let Err(e) = fs::create_dir_all(&out_dir) {
        eprintln!("cpc: mkdir {}: {e}", out_dir.display());
        return ExitCode::FAILURE;
    }
    let out_name = basename.strip_suffix(".cplus").unwrap_or(basename);
    let out_path = out_dir.join(format!("{out_name}.md"));
    if let Err(e) = fs::write(&out_path, &md) {
        eprintln!("cpc: write {}: {e}", out_path.display());
        return ExitCode::FAILURE;
    }
    println!("{}", out_path.display());
    ExitCode::SUCCESS
}

fn run_lsp(args: Vec<OsString>) -> ExitCode {
    let cpc_lsp = find_cpc_lsp();
    let Some(bin) = cpc_lsp else {
        eprintln!("cpc: `cpc-lsp` binary not found. Looked next to `cpc` and on PATH.");
        eprintln!("    Install via `cargo install --path cpc-lsp` from the C+ repo, or");
        eprintln!(
            "    run `cargo run --bin cpc-lsp -- {}` directly.",
            args.iter()
                .filter_map(|a| a.to_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
        return ExitCode::FAILURE;
    };
    // The LSP runs in foreground over stdio; spawn-and-wait is correct
    // (NOT `exec` — we want to keep this process alive in case the
    // child crashes so we can print a clean error).
    let status = Command::new(bin).args(&args).status();
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => ExitCode::from(s.code().unwrap_or(1).clamp(1, 255) as u8),
        Err(e) => {
            eprintln!("cpc: failed to invoke cpc-lsp: {e}");
            ExitCode::FAILURE
        }
    }
}

fn find_cpc_lsp() -> Option<PathBuf> {
    // 1. Same directory as the cpc binary (catches `target/{debug,release}/`
    //    and `cargo install --path` layouts).
    if let Ok(exe) = env::current_exe() {
        if let Some(dir) = exe.parent() {
            let candidate = dir.join(if cfg!(windows) {
                "cpc-lsp.exe"
            } else {
                "cpc-lsp"
            });
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // 2. PATH lookup. No fancy logic — let the shell find it.
    let name = if cfg!(windows) {
        "cpc-lsp.exe"
    } else {
        "cpc-lsp"
    };
    if let Ok(path) = env::var("PATH") {
        for d in env::split_paths(&path) {
            let candidate = d.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn phase0_hello(out: PathBuf) -> ExitCode {
    // The frozen hello.ll is platform-neutral; on Windows append the binary-
    // mode constructor so the demo prints LF, not "\r\n" (matching the real
    // codegen path). `windows_binary_mode_ctor_ir()` is empty off Windows.
    let hello_ir = format!(
        "{HELLO_LL}{}",
        cplus_core::codegen::windows_binary_mode_ctor_ir()
    );
    let tmp_handle = match make_temp_file("cpc-", ".ll", hello_ir.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc: writing IR to temp file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp = tmp_handle.path().to_path_buf();
    let status = run_clang(&tmp, &out, BuildMode::Debug, false, &[], &[]);
    drop(tmp_handle);
    status
}

fn compile_file(
    input: PathBuf,
    out: PathBuf,
    mode: DiagMode,
    build_mode: BuildMode,
    fp_contract: bool,
    debug_info: bool,
    sanitizers: &[&str],
) -> ExitCode {
    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let ir = match timings::phase("front end + codegen", || {
        build_ir(
            &input,
            &src,
            mode,
            build_mode,
            fp_contract,
            debug_info,
            sanitizers,
        )
    }) {
        Ok(ir) => ir,
        Err(code) => return code,
    };
    let tmp_handle = match make_temp_file("cpc-", ".ll", ir.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc: writing IR to temp file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp = tmp_handle.path().to_path_buf();
    let status = timings::phase("clang + link", || {
        run_clang(&tmp, &out, build_mode, debug_info, sanitizers, &[])
    });
    timings::report_project(&input.file_stem().unwrap_or_default().to_string_lossy());
    drop(tmp_handle);
    status
}

/// v0.0.12 G-029 (llama.cplus G-028): walk up from `start` looking for
/// `Cplus.toml`. Returns the manifest path on the first hit, or `None`
/// if we walk all the way to the filesystem root without finding one.
/// Used by the single-file driver paths (`build_ir` for `cpc FILE`,
/// `cpc check`, `cpc --emit-obj`, `cpc --emit-ll`) so they pick up the
/// project's `[dependencies]` when the file lives under a real project
/// — closing the per-file-CMake-invocation gap that blocked llama.cplus
/// from importing `stdlib/atomic` through `cpc --emit-obj`.
fn find_manifest_upward(start: &Path) -> Option<PathBuf> {
    let start = if start.as_os_str().is_empty() {
        Path::new(".")
    } else {
        start
    };
    let abs = start.canonicalize().unwrap_or_else(|_| start.to_path_buf());
    let mut cur: &Path = &abs;
    loop {
        let candidate = cur.join("Cplus.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        match cur.parent() {
            Some(p) if p != cur => cur = p,
            _ => return None,
        }
    }
}

fn build_ir(
    file: &Path,
    src: &str,
    mode: DiagMode,
    build_mode: BuildMode,
    fp_contract: bool,
    debug_info: bool,
    sanitizers: &[&str],
) -> Result<String, ExitCode> {
    // Slice 5DOC: extract doctest fences from `///` comments into appended
    // `#[test]` functions before lexing. Files without doctests are
    // unchanged — `doctest::extract` returns the input verbatim.
    let extracted = doctest::extract(src);
    let src = extracted.as_str();
    // v0.0.9 Phase 7 (cpc-gaps G-011): the single-file path used to call
    // `parser::parse` directly, which meant `import "./foo" as foo;`
    // statements were parsed but never followed. The fix routes through
    // the resolver in project mode with an empty `deps` set — `./` and
    // `../` paths resolve relative to the entry file's directory; bare
    // paths like `"stdlib/io"` fail with E0853 (no Cplus.toml, no
    // declared dependency).
    //
    // The detection logic: if the source has no `import` statements at
    // all, skip the loader entirely and use the legacy direct-parse
    // path. That keeps the single-file fast path (which dominates the
    // sample-program e2e suite) unchanged.
    let has_imports = src.contains("\nimport ") || src.starts_with("import ");
    let (mut prog, files_map, import_edges) = if has_imports {
        // v0.0.12 G-029 (llama.cplus G-028): walk up from FILE's parent
        // looking for `Cplus.toml`. If found, use that directory as the
        // manifest root and pull `[dependencies]` from it so vendor
        // imports (`import "stdlib/atomic"`) resolve the same way they
        // would under `cpc build`. Previously this path hard-coded an
        // empty deps list, which made `cpc --emit-obj src/main.cplus`
        // (the CMake `add_custom_command` shape) fail with E0852 even
        // when the file lived under a project with `stdlib = "*"` in
        // its manifest. Single-file mode without a reachable manifest
        // keeps the old behavior — no deps, only `./` paths resolve.
        let start_dir = file.parent().unwrap_or(Path::new(".")).to_path_buf();
        let manifest_hit = find_manifest_upward(&start_dir);
        let (manifest_root, dep_names): (PathBuf, Vec<String>) = match manifest_hit {
            Some(manifest_path) => match manifest::load(&manifest_path) {
                Ok(m) => {
                    let deps: Vec<String> = active_dep_names(&m);
                    (m.root, deps)
                }
                Err(e) => {
                    emit_diag(&e.to_diagnostic(), mode, "");
                    return Err(ExitCode::FAILURE);
                }
            },
            None => (start_dir, Vec::new()),
        };
        let loaded = match resolver::load_project_full(
            file,
            &manifest_root,
            false,
            Some(&dep_names),
            Default::default(),
        ) {
            Ok(l) => l,
            Err(failure) => {
                let d = failure.to_diagnostic();
                let src_for_diag = failure.primary_source().unwrap_or(src);
                emit_diag(&d, mode, src_for_diag);
                return Err(ExitCode::FAILURE);
            }
        };
        (loaded.program, loaded.files, loaded.imports)
    } else {
        let toks = match lexer::tokenize(src) {
            Ok(t) => t,
            Err(e) => {
                let lm = LineMap::new(src);
                let d = diag::from_lex(&e, file, &lm, src);
                emit_diag(&d, mode, src);
                return Err(ExitCode::FAILURE);
            }
        };
        let prog = match parser::parse(toks) {
            Ok(p) => p,
            Err(e) => {
                let lm = LineMap::new(src);
                let d = diag::from_parse(&e, file, &lm, src);
                emit_diag(&d, mode, src);
                return Err(ExitCode::FAILURE);
            }
        };
        (
            prog,
            std::collections::BTreeMap::new(),
            std::collections::BTreeMap::new(),
        )
    };
    // Phase 5 slice 5ATTR.1: validate attributes before lower / sema.
    let attr_diags = if files_map.is_empty() {
        attrs::check(&prog, file.to_path_buf(), src)
    } else {
        attrs::check_multi(&prog, file.to_path_buf(), src, files_map.clone())
    };
    let attr_errors = attr_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &attr_diags {
        emit_diag_multi(d, mode, src, &files_map);
    }
    if attr_errors {
        return Err(ExitCode::FAILURE);
    }
    // Lower `if let` / `guard let` to match-using forms before sema. GAP 3:
    // route the per-file source map through lower (like attrs / sema) so a
    // lower-pass error in an imported file renders against that file.
    let lower_diags = if files_map.is_empty() {
        lower::lower(&mut prog, file, src)
    } else {
        lower::lower_multi(&mut prog, file, src, files_map.clone())
    };
    let lower_errors = lower_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &lower_diags {
        emit_diag_multi(d, mode, src, &files_map);
    }
    if lower_errors {
        return Err(ExitCode::FAILURE);
    }
    let (diags, mono) = sema::check_multi_with_mono_imports(
        &prog,
        file.to_path_buf(),
        src,
        files_map.clone(),
        &import_edges,
    );
    let had_errors = diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &diags {
        emit_diag_multi(d, mode, src, &files_map);
    }
    if had_errors {
        return Err(ExitCode::FAILURE);
    }
    // Phase 5 borrow checker (slice 5BC.2a). Same per-file routing as sema.
    let bc_diags = borrowck::check_multi(&prog, file, src, &files_map);
    let bc_errors = bc_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &bc_diags {
        emit_diag_multi(d, mode, src, &files_map);
    }
    if bc_errors {
        return Err(ExitCode::FAILURE);
    }
    // Self-growing generic guard (E0910): a `fn rec[T]() { rec::[*T](); }`
    // expands without bound and would hang monomorphization. Detect + report
    // before mono runs. Both `cpc check` (via `run_check` → `build_ir`) and
    // `cpc build` reach codegen through this function, so guarding here covers
    // the fast-feedback and the emit paths alike.
    let inst_diags =
        monomorphize::check_instantiation_bounds(&prog, &mono, file, src, &files_map);
    let inst_errors = inst_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &inst_diags {
        emit_diag_multi(d, mode, src, &files_map);
    }
    if inst_errors {
        return Err(ExitCode::FAILURE);
    }
    let post_mono = run_monomorphize(prog, &mono, &files_map);
    let dbg_path = if debug_info { Some(file) } else { None };
    ensure_coro_end_probed();
    Ok(prune_ir(codegen::generate_with_mono(
        &post_mono,
        build_mode,
        fp_contract,
        dbg_path,
        sanitizers,
        false,
        &mono,
    )))
}

/// Phase timing for `--timings`. Off unless the flag is passed, so the
/// instrumentation costs nothing in a normal build.
///
/// Exists because the phase split had to be reverse-engineered by hand — three
/// separate runs plus arithmetic — to learn that clang is 73% of a debug build
/// and 99% of a release one. That should be one flag, not a research project.
///
/// Two tables come out of it. The phase table answers "where does a compile
/// go"; the package table answers "which package am I paying for". An app on
/// the facet stack prebuilds a dozen dependencies before it compiles a line of
/// its own, and the only way to see that split used to be reading the
/// interleaved prebuild tables and adding them up by hand.
mod timings {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static PHASES: Mutex<Vec<(&'static str, Duration)>> = Mutex::new(Vec::new());
    /// One row per package this build paid for, in the order they finished —
    /// which is dependency order, since a package prebuilds after its deps.
    static PACKAGES: Mutex<Vec<Row>> = Mutex::new(Vec::new());
    /// Dependencies compiled from source INSIDE the current compile
    /// (`prebuild = false`): name -> modules loaded out of its `src/`.
    static INLINED: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());
    /// When the flag was seen. The rows are a breakdown of this, and what they
    /// do NOT account for is worth seeing too.
    static START: Mutex<Option<Instant>> = Mutex::new(None);

    struct Row {
        name: String,
        cost: Cost,
    }

    enum Cost {
        /// Compiled now into `lib/<triple>/<name>.a`.
        Built(Duration),
        /// Its fingerprint matched — the archive on disk was reused.
        Cached(Duration),
        /// Compiled from source as part of the project's own compile.
        Inlined(usize),
        /// The project itself: everything the phase table measured.
        Project(Duration),
    }

    impl Row {
        fn secs(&self) -> f64 {
            match self.cost {
                Cost::Built(d) | Cost::Cached(d) | Cost::Project(d) => d.as_secs_f64(),
                Cost::Inlined(_) => 0.0,
            }
        }
    }

    pub fn enable() {
        ENABLED.store(true, Ordering::Relaxed);
        if let Ok(mut s) = START.lock() {
            *s = Some(Instant::now());
        }
    }

    fn on() -> bool {
        ENABLED.load(Ordering::Relaxed)
    }

    /// Run `f`, recording how long it took under `name`.
    pub fn phase<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
        if !on() {
            return f();
        }
        let start = Instant::now();
        let out = f();
        if let Ok(mut p) = PHASES.lock() {
            p.push((name, start.elapsed()));
        }
        out
    }

    /// Start a stopwatch, or `None` when timing is off — so a caller can time
    /// a package without branching on the flag itself.
    pub fn mark() -> Option<Instant> {
        on().then(Instant::now)
    }

    /// Record what one dependency cost, measured from `started`. `built` is
    /// false when the fingerprint matched and nothing ran: a zero row that
    /// SAYS "up to date" is the evidence the prebuild cache is working, and
    /// dropping it would make a warm build look like it had no dependencies.
    ///
    /// Folded by name. The prebuild walk has no visited set — it re-enters a
    /// package once per edge naming it, so `stdlib` alone arrives fifteen
    /// times in a facet app — and a row per visit buries the table it is
    /// supposed to be. The row keeps the first visit's position (deepest
    /// dependency first) and sums; one real compile among the visits makes
    /// the whole row `prebuilt`.
    pub fn package(name: &str, started: Option<Instant>, built: bool) {
        let Some(t0) = started else { return };
        let d = t0.elapsed();
        let Ok(mut p) = PACKAGES.lock() else { return };
        if let Some(row) = p.iter_mut().find(|r| r.name == name) {
            let sum = Duration::from_secs_f64(row.secs()) + d;
            let was_built = matches!(row.cost, Cost::Built(_));
            row.cost = if built || was_built {
                Cost::Built(sum)
            } else {
                Cost::Cached(sum)
            };
            return;
        }
        p.push(Row {
            name: name.to_string(),
            cost: if built { Cost::Built(d) } else { Cost::Cached(d) },
        });
    }

    /// Note the dependencies whose SOURCE this compile pulled in. A package
    /// with `prebuild = false` is recompiled from scratch inside every
    /// consumer, so it has no cost of its own to report — its share sits
    /// inside the project's row, and there is no honest way to split it out:
    /// resolve, sema, borrowck and codegen all run over one merged program,
    /// not per package. Naming it with its module count is what can be said
    /// truthfully, and saying nothing would read as "this build had no such
    /// dependency".
    ///
    /// `deps` is name -> canonical `src/` directory, from the manifest. The
    /// path SHAPE cannot answer this on its own: a vendor package built from
    /// inside itself sits at `.../vendor/<name>/src/` exactly like a
    /// dependency would, and a dependency can equally resolve to a sibling
    /// directory or the per-user store, where no `vendor/` segment appears at
    /// all. A prebuilt dependency contributes only `lib/include/`
    /// declarations, never `src/`, so it cannot land here twice.
    ///
    /// Called by every compile; the last call wins, which is the root
    /// project's — prebuilds all run before it.
    pub fn inlined_from(loaded: &[PathBuf], deps: &[(String, PathBuf)]) {
        if !on() {
            return;
        }
        let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
        for p in loaded {
            if let Some((name, _)) = deps.iter().find(|(_, src)| p.starts_with(src)) {
                *counts.entry(name.as_str()).or_insert(0) += 1;
            }
        }
        if let Ok(mut i) = INLINED.lock() {
            *i = counts
                .into_iter()
                .map(|(n, c)| (n.to_string(), c))
                .collect();
        }
    }

    /// `report`, with a heading and a reset. A `prebuild` compile runs nested
    /// inside a consumer's build, and without clearing, its phases land in the
    /// consumer's table as a second set of identically-named rows whose
    /// percentages are computed over both builds at once.
    ///
    /// Returns the measured total, which is the compile's own cost — the
    /// package roll-up uses it for the project's row.
    pub fn report_titled(title: &str) -> f64 {
        if !on() {
            return 0.0;
        }
        let Ok(mut p) = PHASES.lock() else { return 0.0 };
        if p.is_empty() {
            return 0.0;
        }
        let total: f64 = p.iter().map(|(_, d)| d.as_secs_f64()).sum();
        if title.is_empty() {
            eprintln!("cpc timings:");
        } else {
            eprintln!("cpc timings ({title}):");
        }
        for (name, d) in p.iter() {
            let secs = d.as_secs_f64();
            let pct = if total > 0.0 { 100.0 * secs / total } else { 0.0 };
            eprintln!("  {name:<26} {secs:>7.2}s  {pct:>4.0}%");
        }
        eprintln!("  {:<26} {total:>7.2}s", "measured total");
        p.clear();
        total
    }

    /// The phase table for the build that just finished, then the per-package
    /// roll-up: every dependency compiled or reused, the project's own cost,
    /// and the wall clock none of it accounted for.
    pub fn report_project(project: &str) {
        let own = report_titled("");
        if !on() {
            return;
        }
        let inlined = INLINED
            .lock()
            .map(|mut i| std::mem::take(&mut *i))
            .unwrap_or_default();
        let Ok(mut rows) = PACKAGES.lock() else {
            return;
        };
        // Nothing but the project itself: the phase table already said it all.
        if rows.is_empty() && inlined.is_empty() {
            return;
        }
        for (name, modules) in inlined {
            rows.push(Row {
                name,
                cost: Cost::Inlined(modules),
            });
        }
        rows.push(Row {
            name: project.to_string(),
            cost: Cost::Project(Duration::from_secs_f64(own)),
        });
        let accounted: f64 = rows.iter().map(Row::secs).sum();
        let elapsed = START
            .lock()
            .ok()
            .and_then(|s| *s)
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(accounted);
        let total = elapsed.max(accounted);
        eprintln!("cpc timings by package:");
        for r in rows.iter() {
            let pct = if total > 0.0 {
                100.0 * r.secs() / total
            } else {
                0.0
            };
            match &r.cost {
                Cost::Built(d) => eprintln!(
                    "  {:<26} {:>7.2}s  {pct:>4.0}%  prebuilt",
                    r.name,
                    d.as_secs_f64()
                ),
                Cost::Cached(d) => eprintln!(
                    "  {:<26} {:>7.2}s  {pct:>4.0}%  up to date",
                    r.name,
                    d.as_secs_f64()
                ),
                Cost::Project(d) => eprintln!(
                    "  {:<26} {:>7.2}s  {pct:>4.0}%  this project",
                    r.name,
                    d.as_secs_f64()
                ),
                Cost::Inlined(n) => eprintln!(
                    "  {:<26} {:>8}  {:>5}  compiled inside `{project}` ({n} modules)",
                    r.name, "—", ""
                ),
            }
        }
        // Manifest loads, the dep-link walk, header generation outside a
        // prebuild, archive copies. Small, but it is the difference between a
        // table that adds up and one that quietly doesn't.
        let other = total - accounted;
        if other > 0.005 {
            eprintln!(
                "  {:<26} {other:>7.2}s  {:>4.0}%  manifests, dep walk, glue",
                "(unattributed)",
                100.0 * other / total
            );
        }
        eprintln!("  {:<26} {total:>7.2}s", "wall clock");
        rows.clear();
    }
}

/// Drop unreachable `internal` definitions before handing the module to clang.
///
/// Whole-program codegen emits every function of every dependency; on iris 69%
/// of them are referenced nowhere, and at `-O0` clang codegens all of them into
/// the object file rather than discarding them. Pruning first is strictly less
/// work for clang and cannot change behaviour — see `cplus_core::prune`, which
/// only removes definitions whose symbol appears nowhere else in the module.
///
/// `CPC_NO_PRUNE=1` disables it, for bisecting a suspected miscompile.
fn prune_ir(ir: String) -> String {
    if std::env::var_os("CPC_NO_PRUNE").is_some() {
        return ir;
    }
    let (pruned, dropped) = cplus_core::prune::prune_unreachable(&ir);
    if dropped > 0 && std::env::var_os("CPC_VERBOSE").is_some() {
        eprintln!(
            "cpc: pruned {dropped} unreachable definitions ({:.1} MB -> {:.1} MB)",
            ir.len() as f64 / 1e6,
            pruned.len() as f64 / 1e6
        );
    }
    pruned
}

fn run_clang(
    input_ll: &Path,
    out: &Path,
    mode: BuildMode,
    debug_info: bool,
    sanitizers: &[&str],
    link_args: &[String],
) -> ExitCode {
    // Pass the LLVM optimization level alongside our own build-mode choice:
    //   Debug   -> `-O0`. Keeps the overflow-check intrinsics, leaves divs
    //              and branches in source order, debuggable IR.
    //   Release -> `-O3` (see the v0.0.5 note below). Engages LLVM's
    //              standard inlining, mem2reg,
    //              GVN, LICM, loop reduction, etc. Without this flag clang
    //              defaults to `-O0` and our "release" binaries are 100×
    //              slower than they need to be.
    //
    //              v0.0.5: bumped to -O3 (was -O2). Across the bench-cplus
    //              suite, -O3 is faster on raytracer (FP-heavy), faster on
    //              hashmap (integer-heavy), tied on JSON tokenizer; binary
    //              sizes within ±0.1%. The win is mostly LLVM's more
    //              aggressive inliner threshold + loop unrolling. Defaults
    //              for production languages (Rust --release, etc.) are
    //              equivalent. The cost is marginal extra compile time.
    let opt = match mode {
        BuildMode::Debug => "-O0",
        BuildMode::Release => "-O3",
    };
    let mut cmd = Command::new(clang_program());
    cmd.arg(opt).arg("-Wno-override-module");
    // f16 lowering on x86_64 emits libcalls to the half-precision conversion
    // builtins (`__extendhfsf2`, `__truncsfhf2`). On Linux/macOS these live in
    // the default runtime clang links; on windows-msvc clang links the MSVC
    // runtime, which lacks them, so the link fails with "undefined symbol:
    // __extendhfsf2". `-rtlib=compiler-rt` pulls in clang's builtins archive
    // (just the helpers — the C runtime stays MSVC's) to resolve them.
    if cfg!(windows) {
        cmd.arg("-rtlib=compiler-rt");
    }
    // Phase 11 polish: `-g` keeps the DWARF metadata cpc emitted in the
    // IR through to the final binary. Without it clang silently strips
    // the .debug_info section.
    if debug_info {
        cmd.arg("-g");
    }
    // Phase 11 polish: sanitizer instrumentation. clang owns the
    // instrumentation pass + the matching runtime library; we just
    // forward the comma-joined `-fsanitize=` argument.
    if !sanitizers.is_empty() {
        cmd.arg(format!("-fsanitize={}", sanitizers.join(",")));
        // Better stack traces in sanitizer reports.
        cmd.arg("-fno-omit-frame-pointer");
    }
    // Debug escape hatch: `CPC_CLANG_EXTRA="-mllvm -foo"` appends
    // whitespace-separated flags to the clang invocation. There is otherwise no
    // way to bisect a backend pass from outside the compiler, because the IR cpc
    // feeds clang is written to a temp file and deleted, and `--emit-ll-project`
    // generates with an EMPTY sanitizer list — so hand-compiling its output
    // silently produces an UNINSTRUMENTED binary and looks like a clean run.
    if let Ok(extra) = env::var("CPC_CLANG_EXTRA") {
        for a in extra.split_whitespace() {
            cmd.arg(a);
        }
    }
    // The program object goes FIRST, before any libraries. GNU `ld`
    // resolves left-to-right in a single pass and pulls a static-archive
    // member only to satisfy a reference it has ALREADY seen. So a bundled
    // `lib*.a` listed before the object that calls into it contributes
    // nothing and its symbols come up undefined (macOS's ld64 does a full
    // resolution and is order-insensitive, which is why this only bites on
    // Linux). Emit `input_ll`, then the manifest link args, then `-lm`.
    cmd.arg(input_ll);
    // v0.0.2 (AppKit-via-Cplus.toml): manifest-driven linker args. Each
    // entry was generated by `build_project` from `[link] frameworks`
    // (`-framework X`), `libs` (`-lX`), and bundled `[link]` archives.
    // Empty for everything except project builds whose manifest declares
    // them.
    for arg in link_args {
        cmd.arg(arg);
    }
    // On Linux, libm is a separate library: math symbols like `fma`,
    // `fmaf`, `sqrt` (emitted by SIMD/float lowering) are NOT resolved
    // unless we pass `-lm`. macOS rolls libm into libSystem, which clang
    // links by default, so this flag is unnecessary — and harmless — there.
    // Windows (MSVC) has no `m.lib` at all — the math functions live in the
    // UCRT, which clang links by default; passing `-lm` makes lld-link fail
    // with "could not open 'm.lib'". So scope this to non-macOS *Unix*.
    // Last on the line so it satisfies math refs from the object and any
    // bundled archive ahead of it.
    if cfg!(all(unix, not(target_os = "macos"))) {
        cmd.arg("-lm");
    }
    // On Windows the async reactor (reactor_windows.cplus) and the socket
    // stack (netsys_windows.cplus / net.cplus) call into Winsock — WSAPoll,
    // WSAStartup, recv/send/closesocket/ioctlsocket. ws2_32 is not auto-
    // linked by the MSVC driver, so request it here. Harmless (an import
    // table entry) for programs that don't touch sockets.
    if cfg!(windows) {
        cmd.arg("-lws2_32");
    }
    cmd.arg("-o").arg(out);
    // Under `-g`, clang DISCARDS the whole module's debug info — with only a
    // warning — if any inlinable call lacks a `!dbg` location. A `-g` build
    // that shipped without symbols therefore looked like a successful build
    // (reports/bug-09: the DWARF post-pass didn't recognize `musttail call`,
    // so every self-recursive fn silently cost the binary its debug info).
    // Capture stderr in this mode only and make that warning fatal, so the
    // next gap in the matcher fails loudly instead of degrading in silence.
    // Non-`-g` builds keep clang's stderr streaming live.
    if !debug_info {
        return match cmd.status() {
            Ok(s) if s.success() => ExitCode::SUCCESS,
            Ok(s) => {
                eprintln!("cpc: clang exited with {s}");
                ExitCode::from(s.code().unwrap_or(1).clamp(1, 255) as u8)
            }
            Err(e) => {
                eprintln!("cpc: failed to invoke clang: {e}");
                ExitCode::FAILURE
            }
        };
    }
    match cmd.output() {
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr);
            eprint!("{stderr}");
            if !o.status.success() {
                eprintln!("cpc: clang exited with {}", o.status);
                return ExitCode::from(o.status.code().unwrap_or(1).clamp(1, 255) as u8);
            }
            if stderr.contains("ignoring invalid debug info") {
                eprintln!(
                    "cpc: clang rejected the debug metadata, so `-g` produced a binary with \
                     NO debug info. This is a compiler bug — please report it."
                );
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cpc: failed to invoke clang: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dump_tokens(path: PathBuf, mode: DiagMode) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match lexer::tokenize(&src) {
        Ok(toks) => {
            for t in &toks {
                println!("{:>4}..{:<4}  {:?}", t.span.start, t.span.end, t.kind);
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            let lm = LineMap::new(&src);
            let d = diag::from_lex(&e, &path, &lm, &src);
            emit_diag(&d, mode, &src);
            ExitCode::FAILURE
        }
    }
}

fn dump_ll(
    path: PathBuf,
    mode: DiagMode,
    build_mode: BuildMode,
    fp_contract: bool,
    debug_info: bool,
    sanitizers: &[&str],
) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match build_ir(
        &path,
        &src,
        mode,
        build_mode,
        fp_contract,
        debug_info,
        sanitizers,
    ) {
        Ok(ir) => {
            print!("{ir}");
            ExitCode::SUCCESS
        }
        Err(code) => code,
    }
}

/// Slice 1G: post-optimization IR or assembly inspection.
///
/// `--emit-ll-opt FILE` and `--emit-asm FILE` route through here. They
/// generate the same pre-LLVM IR as `--emit-ll` but feed it to clang with
/// `-S -emit-llvm` (post-pass IR) or `-S` (assembly), at the optimization
/// level matching `--debug` (`-O0`) or `--release` (`-O2`). The slice exists
/// because slices 1B/1C cannot be validated without seeing what `-O2`
/// actually does with the metadata — `--emit-ll` shows only what cpc
/// emitted, not what LLVM keeps after inlining and InstCombine.
///
/// `output_kind` is "ll" for post-pass LLVM IR or "asm" for native assembly.
/// Phase 5 Slice 5.A: produce a relocatable object file (`.o`).
///
/// Builds the IR for `input` (skipping the `@main` injection if the
/// upstream sema marked this a library — see `build_ir_with_options`),
/// writes it to a temp `.ll`, runs `clang -c -O<level>` to produce the
/// object, and writes the result to `out`. Used both by the explicit
/// `cpc --emit-obj` flag and as the first step inside the `cpc build`
/// library pipeline (5.A.3 below).
/// Phase 5 Slice 5.E: emit a C header for a `.cplus` source. Walks the
/// program's top-level items, emits a C declaration for every `export` item
/// whose signature is C-ABI-compatible (Slice 5.C's predicate). Items
/// that aren't representable in C (non-`#[repr(C)]` structs, Drop types,
/// tagged enums, generics) are skipped silently — sema's E0410 already
/// rejects them in `export extern fn` signatures, so they can only reach
/// the header path via plain `export fn` / `export struct` declarations and
/// will be silently dropped from the header surface.
///
/// The generated header is hand-readable and idiomatic C99:
/// - `#pragma once` for include-guard simplicity.
/// - `#include <stdbool.h>` + `<stddef.h>` + `<stdint.h>` for the
///   primitive type aliases.
/// - Struct definitions before fn declarations so signatures can
///   reference them. Order: exported structs / enums / type aliases first,
///   then exported fn declarations.
///
/// `lib_name` shapes the include-guard fallback when `#pragma once`
/// isn't honored by the consumer toolchain (very rare today).
fn dump_header(input: PathBuf, lib_name: Option<&str>, diag_mode: DiagMode) -> ExitCode {
    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    // Reuse build_ir's front-end gauntlet, but only for sema validation —
    // we don't actually need the IR. If sema fails, the error message is
    // already emitted; abort the header build with the same exit.
    let toks = match cplus_core::lexer::tokenize(&src) {
        Ok(t) => t,
        Err(e) => {
            let lm = diag::LineMap::new(&src);
            let d = diag::from_lex(&e, &input.to_path_buf(), &lm, &src);
            emit_diag(&d, diag_mode, &src);
            return ExitCode::FAILURE;
        }
    };
    let prog = match cplus_core::parser::parse(toks) {
        Ok(p) => p,
        Err(e) => {
            let lm = diag::LineMap::new(&src);
            let d = diag::from_parse(&e, &input.to_path_buf(), &lm, &src);
            emit_diag(&d, diag_mode, &src);
            return ExitCode::FAILURE;
        }
    };
    let header = render_c_header(&prog, lib_name.unwrap_or("cplus_lib"));
    print!("{header}");
    ExitCode::SUCCESS
}

/// Phase 5 Slice 5.E: render a C header for `program`'s `export` surface.
/// Public so the library build pipeline (5.A) can call it alongside the
/// `.a` / `.dylib` artifact emission.
fn render_c_header(program: &cplus_core::ast::Program, lib_name: &str) -> String {
    use cplus_core::ast::ItemKind;
    let mut out = String::new();
    out.push_str(&format!(
        "// Generated by cpc — public C ABI for `{lib_name}`. Do not edit.\n"
    ));
    out.push_str("#pragma once\n\n");
    out.push_str("#include <stdbool.h>\n");
    out.push_str("#include <stddef.h>\n");
    out.push_str("#include <stdint.h>\n\n");
    out.push_str("#ifdef __cplusplus\nextern \"C\" {\n#endif\n\n");

    // Pass 1: exported `#[repr(C)]` structs and exported plain enums
    // (definitions that fn signatures may reference). Tagged enums and
    // non-repr-C structs are skipped silently — sema's 5.C predicate
    // already rejects them in `export extern fn` signatures, so any fn
    // that would need them in the header would have failed before
    // reaching here.
    for item in &program.items {
        match &item.kind {
            ItemKind::Struct(s) if s.is_pub => {
                let is_repr_c = s.attributes.iter().any(|a| a.path.name == "repr");
                if !is_repr_c {
                    continue;
                }
                // Drop check: a struct with a `drop` method isn't safe to
                // expose by value. The user's `export extern fn` would have
                // failed sema (5.C) if they tried; here we just skip.
                // We can't easily check drop without sema state, so emit
                // the struct definition and rely on consumers not to use
                // it across a value boundary if it had Drop (5.C catches
                // it at the actual use site).
                if let Some(decl) = render_struct_decl(s) {
                    out.push_str(&decl);
                    out.push('\n');
                }
            }
            ItemKind::Enum(e) if e.is_pub => {
                let is_tagged = e.variants.iter().any(|v| !v.payload.is_empty());
                if is_tagged {
                    continue;
                }
                // `typedef enum Foo { ... } Foo;` lets consumers use the
                // bare name as a type — matches what we do for structs.
                out.push_str(&format!("typedef enum {} {{\n", e.name.name));
                for (i, v) in e.variants.iter().enumerate() {
                    let sep = if i + 1 == e.variants.len() { "" } else { "," };
                    out.push_str(&format!(
                        "    {}_{} = {}{}\n",
                        e.name.name, v.name.name, i, sep
                    ));
                }
                out.push_str(&format!("}} {};\n\n", e.name.name));
            }
            _ => {}
        }
    }

    // Pass 2: exported fn declarations. Both `export fn` (C+-callable from
    // inside the library; scalar-only ones are accidentally C-callable too)
    // and `export extern fn ... { body }` (Slice 5.C: explicit C-ABI export).
    // Any signature element that fails the C-mapping (e.g. `str`, slice,
    // tagged enum) makes us skip the whole fn — that's sound because the
    // consumer couldn't write a matching signature anyway.
    for item in &program.items {
        if let ItemKind::Function(f) = &item.kind {
            if !f.is_pub {
                continue;
            }
            // Skip the parser-collapsed body for extern declarations
            // (no body, decl form): those are imports, not exports.
            if f.is_extern && f.body.stmts.is_empty() && f.body.tail.is_none() {
                continue;
            }
            if !f.generic_params.is_empty() {
                continue;
            }
            let Some(decl) = render_fn_decl(f) else {
                continue;
            };
            out.push_str(&decl);
            out.push('\n');
        }
    }

    out.push_str("\n#ifdef __cplusplus\n} // extern \"C\"\n#endif\n");
    out
}

/// Render a `#[repr(C)] export struct Foo { ... }` as a C declaration.
/// Returns None if any field's type isn't C-representable.
fn render_struct_decl(s: &cplus_core::ast::StructDecl) -> Option<String> {
    let mut out = format!("typedef struct {} {{\n", s.name.name);
    for f in &s.fields {
        let c_ty = type_to_c(&f.ty)?;
        out.push_str(&format!("    {} {};\n", c_ty, f.name.name));
    }
    out.push_str(&format!("}} {};\n", s.name.name));
    Some(out)
}

/// Render an `export fn` (or `export extern fn`) as a C prototype. Returns
/// None when any param or return type isn't C-representable.
fn render_fn_decl(f: &cplus_core::ast::Function) -> Option<String> {
    let ret = match &f.return_type {
        Some(t) => type_to_c(t)?,
        None => "void".to_string(),
    };
    let mut out = format!("{} {}(", ret, f.name.name);
    if f.params.is_empty() && !f.is_variadic {
        out.push_str("void");
    } else {
        for (i, p) in f.params.iter().enumerate() {
            if i > 0 {
                out.push_str(", ");
            }
            out.push_str(&render_param_decl(&p.ty, &p.name.name)?);
        }
        if f.is_variadic {
            if !f.params.is_empty() {
                out.push_str(", ");
            }
            out.push_str("...");
        }
    }
    out.push_str(");\n");
    Some(out)
}

/// Render a single C parameter declarator `<type> <name>` with the C
/// quirk that function-pointer params embed the name *inside* the
/// declarator: `R (*name)(args)` instead of `R (*)(args) name`.
fn render_param_decl(t: &cplus_core::ast::Type, name: &str) -> Option<String> {
    use cplus_core::ast::TypeKind;
    if let TypeKind::FnPtr {
        params,
        return_type,
        ..
    } = &t.kind
    {
        let ret = match return_type {
            Some(t) => type_to_c(t)?,
            None => "void".to_string(),
        };
        let mut s = format!("{} (*{})(", ret, name);
        if params.is_empty() {
            s.push_str("void");
        } else {
            for (i, p) in params.iter().enumerate() {
                if i > 0 {
                    s.push_str(", ");
                }
                s.push_str(&type_to_c(p)?);
            }
        }
        s.push(')');
        return Some(s);
    }
    let c_ty = type_to_c(t)?;
    Some(format!("{} {}", c_ty, name))
}

/// Map a C+ surface `Type` to the C type that has the same ABI. Returns
/// None if the C+ type has no clean C counterpart (sema's 5.C predicate
/// would already reject these in extern signatures; the header emitter
/// uses None to mean "skip this declaration").
fn type_to_c(t: &cplus_core::ast::Type) -> Option<String> {
    use cplus_core::ast::TypeKind;
    Some(match &t.kind {
        TypeKind::Path(name) => match name.as_str() {
            "i8" => "int8_t".to_string(),
            "i16" => "int16_t".to_string(),
            "i32" => "int32_t".to_string(),
            "i64" => "int64_t".to_string(),
            "u8" => "uint8_t".to_string(),
            "u16" => "uint16_t".to_string(),
            "u32" => "uint32_t".to_string(),
            "u64" => "uint64_t".to_string(),
            "isize" => "intptr_t".to_string(),
            "usize" => "size_t".to_string(),
            "f32" => "float".to_string(),
            "f64" => "double".to_string(),
            "bool" => "bool".to_string(),
            // Non-C surface types — don't appear in valid exports.
            "str" | "string" => return None,
            // Anything else: assume it's a user-defined `#[repr(C)]`
            // struct or plain enum. Bare name. If it's actually a
            // non-C type (tagged enum, etc.), the consumer's compile
            // will fail — which is the right signal.
            other => other.to_string(),
        },
        TypeKind::RawPtr(inner) => {
            // `*u8` → `uint8_t *`. For nested fn pointers fall through to
            // the FnPtr arm; for everything else, append a star.
            let inner_c = type_to_c(inner)?;
            format!("{} *", inner_c)
        }
        TypeKind::FnPtr {
            params,
            return_type,
            ..
        } => {
            let ret = match return_type {
                Some(t) => type_to_c(t)?,
                None => "void".to_string(),
            };
            let mut s = String::from(ret.as_str());
            s.push_str(" (*)(");
            if params.is_empty() {
                s.push_str("void");
            } else {
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        s.push_str(", ");
                    }
                    s.push_str(&type_to_c(p)?);
                }
            }
            s.push(')');
            s
        }
        TypeKind::Array { elem, len, .. } => {
            // In a parameter position, `T[N]` decays to `T*` in C —
            // technically the same ABI. We render the array form anyway
            // since the user's intent is "fixed-size buffer" and clang
            // treats `T arr[N]` and `T *arr` interchangeably in proto.
            let elem_c = type_to_c(elem)?;
            format!("{}[{}]", elem_c, len)
        }
        // Generics, slices, tuples — not C-representable.
        TypeKind::Generic { .. } | TypeKind::Slice(_) | TypeKind::Tuple(_) => return None,
    })
}

fn dump_obj(
    input: PathBuf,
    out: PathBuf,
    diag_mode: DiagMode,
    build_mode: BuildMode,
    fp_contract: bool,
) -> ExitCode {
    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let ir = match build_ir(&input, &src, diag_mode, build_mode, fp_contract, false, &[]) {
        Ok(ir) => ir,
        Err(code) => return code,
    };
    let tmp_handle = match make_temp_file("cpc-obj-", ".ll", ir.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc: writing IR to temp file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp = tmp_handle.path().to_path_buf();
    if let Some(parent) = out.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(parent) {
                eprintln!("cpc: creating {}: {e}", parent.display());
                drop(tmp_handle);
                return ExitCode::FAILURE;
            }
        }
    }
    let opt = match build_mode {
        BuildMode::Debug => "-O0",
        BuildMode::Release => "-O3",
    };
    let tgt = target::active_target();
    let prog = match clang_program_for(&tgt) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("cpc: {msg}");
            drop(tmp_handle);
            return ExitCode::FAILURE;
        }
    };
    let status = Command::new(&prog)
        .arg(opt)
        .arg("-Wno-override-module")
        .args(clang_target_args(&tgt))
        .arg("-c")
        .arg(&tmp)
        .arg("-o")
        .arg(&out)
        .status();
    drop(tmp_handle);
    match status {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => {
            eprintln!("cpc: clang -c exited with {s}");
            ExitCode::from(s.code().unwrap_or(1).clamp(1, 255) as u8)
        }
        Err(e) => {
            eprintln!("cpc: failed to invoke clang: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dump_ll_or_asm(
    path: PathBuf,
    mode: DiagMode,
    build_mode: BuildMode,
    fp_contract: bool,
    output_kind: ClangOutputKind,
) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let ir = match build_ir(&path, &src, mode, build_mode, fp_contract, false, &[]) {
        Ok(ir) => ir,
        Err(code) => return code,
    };
    let tmp_handle = match make_temp_file("cpc-emit-", ".ll", ir.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc: writing IR to temp file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp = tmp_handle.path().to_path_buf();
    let code = run_clang_to_stdout(&tmp, build_mode, output_kind);
    drop(tmp_handle);
    code
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClangOutputKind {
    /// `clang -S -emit-llvm` → post-pass LLVM IR text.
    LlvmIr,
    /// `clang -S`           → native assembly.
    Assembly,
}

/// Invoke clang to transform IR through the optimization pipeline and print
/// the result on stdout. Matches `run_clang`'s `-O0`/`-O3` selection so the
/// `--debug` / `--release` flags compose with `--emit-ll-opt` and
/// `--emit-asm` consistently.
fn run_clang_to_stdout(input_ll: &Path, mode: BuildMode, kind: ClangOutputKind) -> ExitCode {
    let opt = match mode {
        BuildMode::Debug => "-O0",
        BuildMode::Release => "-O3",
    };
    let tgt = target::active_target();
    let prog = match clang_program_for(&tgt) {
        Ok(p) => p,
        Err(msg) => {
            eprintln!("cpc: {msg}");
            return ExitCode::FAILURE;
        }
    };
    let mut cmd = Command::new(&prog);
    cmd.arg(opt).arg("-Wno-override-module").arg("-S");
    cmd.args(clang_target_args(&tgt));
    if matches!(kind, ClangOutputKind::LlvmIr) {
        cmd.arg("-emit-llvm");
    }
    cmd.arg(input_ll).arg("-o").arg("-");
    match cmd.status() {
        Ok(s) if s.success() => ExitCode::SUCCESS,
        Ok(s) => {
            eprintln!("cpc: clang exited with {s}");
            ExitCode::from(s.code().unwrap_or(1).clamp(1, 255) as u8)
        }
        Err(e) => {
            eprintln!("cpc: failed to invoke clang: {e}");
            ExitCode::FAILURE
        }
    }
}

fn dump_ast(path: PathBuf, mode: DiagMode) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let toks = match lexer::tokenize(&src) {
        Ok(t) => t,
        Err(e) => {
            let lm = LineMap::new(&src);
            let d = diag::from_lex(&e, &path, &lm, &src);
            emit_diag(&d, mode, &src);
            return ExitCode::FAILURE;
        }
    };
    match parser::parse(toks) {
        Ok(prog) => {
            println!("{prog:#?}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            let lm = LineMap::new(&src);
            let d = diag::from_parse(&e, &path, &lm, &src);
            emit_diag(&d, mode, &src);
            ExitCode::FAILURE
        }
    }
}

// ---- `cpc skill` : the embedded agent/LLM reference --------------------------

/// The C+ agent reference, bundled into the binary at build time so `cpc skill`
/// works from any install (brew / cargo / source) with no network, and is
/// always version-matched to this `cpc`. Source of truth: `docs/lang/skill.md`.
const SKILL_MD: &str = include_str!("../../docs/lang/skill.md");

const SKILL_USAGE: &str = "\
cpc skill - print the C+ reference for an LLM/agent (version-matched to this cpc)

usage:
  cpc skill                 print the reference to stdout
  cpc skill --write [PATH]  write it into the project (default: ./SKILL.md)
  cpc skill --write --force overwrite an existing file
  cpc skill --lang-only     the language reference alone, no package skills

Run inside a project, the language reference is followed by the SKILL.md of
every dependency that ships one — so a project that depends on `facet` gets
facet's guidance without anyone wiring it up.
";

/// A dependency's own agent reference, if it ships one.
///
/// The language reference tells an agent how to write C+; it cannot tell it how
/// to use a package correctly, and a package's misuse usually COMPILES — which
/// is exactly the class of mistake no diagnostic will catch. So a package may
/// ship `SKILL.md` in its root, and `cpc skill` appends it for every dependency
/// the manifest declares. Resolution is `vendor_dir_for`, the same path the
/// linker and the import resolver use, so the skill read is the package built.
///
/// Platform-scoped deps are included regardless of the active platform: an
/// agent writing the iOS half of a project needs `facet_uikit`'s rules while
/// building for macOS.
fn package_skills() -> Vec<(String, String)> {
    let cwd = match std::env::current_dir() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let Some(manifest_path) = find_manifest_upward(&cwd) else {
        return Vec::new();
    };
    let Ok(m) = manifest::load(&manifest_path) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for dep in &m.dependencies {
        if seen.iter().any(|n| n == &dep.name) {
            continue;
        }
        seen.push(dep.name.clone());
        let Some(dir) = vendor_dir_for(&m, &dep.name) else {
            continue;
        };
        if let Ok(text) = std::fs::read_to_string(dir.join("SKILL.md")) {
            out.push((dep.name.clone(), text));
        }
    }
    out
}

/// The language reference plus every dependency skill, in manifest order.
fn full_skill(lang_only: bool) -> String {
    let mut s = String::from(SKILL_MD);
    if lang_only {
        return s;
    }
    for (name, text) in package_skills() {
        if !s.ends_with('\n') {
            s.push('\n');
        }
        s.push_str(&format!(
            "\n---\n\n<!-- package skill: {name} (from vendor/{name}/SKILL.md) -->\n\n"
        ));
        s.push_str(&text);
    }
    s
}

/// `cpc skill [--write [PATH]] [--force]`.
fn run_skill(args: &[OsString]) -> ExitCode {
    let mut write = false;
    let mut force = false;
    let mut lang_only = false;
    let mut dest: Option<PathBuf> = None;
    for a in args {
        match a.to_str() {
            Some("--write") | Some("-w") => write = true,
            Some("--force") | Some("-f") => force = true,
            Some("--lang-only") => lang_only = true,
            Some("-h") | Some("--help") => {
                print!("{SKILL_USAGE}");
                return ExitCode::SUCCESS;
            }
            Some(p) if write && dest.is_none() && !p.starts_with('-') => {
                dest = Some(PathBuf::from(p));
            }
            other => {
                eprintln!("cpc skill: unexpected argument `{}`", other.unwrap_or("<non-utf8>"));
                eprint!("{SKILL_USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }

    let text = full_skill(lang_only);

    if !write {
        print!("{text}");
        return ExitCode::SUCCESS;
    }

    let path = dest.unwrap_or_else(|| PathBuf::from("SKILL.md"));
    if path.exists() && !force {
        eprintln!(
            "cpc skill: {} already exists (use --force to overwrite)",
            path.display()
        );
        return ExitCode::FAILURE;
    }
    match std::fs::write(&path, &text) {
        Ok(()) => {
            let extra = package_skills();
            if lang_only || extra.is_empty() {
                println!("wrote {}", path.display());
            } else {
                let names: Vec<&str> = extra.iter().map(|(n, _)| n.as_str()).collect();
                println!("wrote {} (language + {})", path.display(), names.join(", "));
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cpc skill: could not write {}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}

// ---- `cpc explain` : self-serve diagnostic reference for an LLM/agent --------

/// The C+ diagnostic catalog, embedded at build time so `cpc explain <CODE>`
/// works from any install with no network. Same source of truth that generates
/// `docs/lang/errors.md` and the cplus-lang.dev /docs/error-codes page, so the CLI,
/// the docs, and the compiler never drift. An agent that hits a diagnostic runs
/// `cpc explain E0502` for its cause, fix, and an example instead of guessing.
const ERRORS_TOML: &str = include_str!("../../docs/lang/errors.toml");

const EXPLAIN_USAGE: &str = "\
cpc explain [CODE] - explain a C+ diagnostic (e.g. `cpc explain E0502`)

  cpc explain E0502     cause, fix, and an example for E0502
  cpc explain --list    every diagnostic code with its title
  cpc explain           (no code) same as --list

The full docs at https://cplus-lang.dev/docs are LLM-readable as raw markdown:
append `.md` to any page (e.g. .../docs/control-flow -> .../docs/control-flow.md).
";

/// Normalize a user-typed code to the canonical `E####` form: `e502`, `502`,
/// and `E0502` all resolve to `E0502`. Non-numeric input is just upper-cased.
fn normalize_code(s: &str) -> String {
    let t = s.trim();
    let digits = t.strip_prefix(['E', 'e']).unwrap_or(t);
    match digits.parse::<u32>() {
        Ok(n) => format!("E{n:04}"),
        Err(_) => t.to_uppercase(),
    }
}

fn format_code(e: &toml::Value) -> String {
    let s = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("").trim();
    let mut out = String::new();
    out.push_str(&format!("{} — {}\n", s("id"), s("title")));
    out.push_str(&format!("  {} · {}\n\n", s("category"), s("severity")));
    if !s("cause").is_empty() {
        out.push_str(&format!("Cause\n  {}\n\n", s("cause")));
    }
    if !s("fix").is_empty() {
        out.push_str(&format!("Fix\n  {}\n\n", s("fix")));
    }
    if !s("example").is_empty() {
        out.push_str("Example\n");
        for line in s("example").lines() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    if !s("emit_site").is_empty() {
        out.push_str(&format!("Verified against the compiler at {}\n", s("emit_site")));
    }
    out.push_str(
        "More: https://cplus-lang.dev/docs/error-codes.md \
         (append .md to any cplus-lang.dev/docs page for the markdown an agent can read)\n",
    );
    out
}

fn explain_list(codes: &[toml::Value]) -> ExitCode {
    let mut rows: Vec<(&str, &str, &str)> = codes
        .iter()
        .map(|e| {
            let g = |k: &str| e.get(k).and_then(|v| v.as_str()).unwrap_or("");
            (g("id"), g("title"), g("category"))
        })
        .collect();
    rows.sort_by(|a, b| a.0.cmp(b.0));
    println!(
        "{} C+ diagnostics — `cpc explain <CODE>` for cause + fix + example:\n",
        rows.len()
    );
    for (id, title, cat) in rows {
        println!("  {id}  {title}  [{cat}]");
    }
    println!("\nThe docs at https://cplus-lang.dev/docs are LLM-readable as markdown — append .md to any page.");
    ExitCode::SUCCESS
}

/// `cpc explain <CODE>` — print a diagnostic's cause, fix, and example from the
/// embedded catalog, so an agent that hits an error can read what it means
/// (and how to fix it) without leaving the shell or guessing.
fn run_explain(args: &[OsString]) -> ExitCode {
    let mut query: Option<String> = None;
    let mut list = false;
    for a in args {
        match a.to_str() {
            Some("-h") | Some("--help") => {
                print!("{EXPLAIN_USAGE}");
                return ExitCode::SUCCESS;
            }
            Some("--list") | Some("-l") => list = true,
            Some(c) if !c.starts_with('-') && query.is_none() => query = Some(c.to_string()),
            other => {
                eprintln!(
                    "cpc explain: unexpected argument `{}`",
                    other.unwrap_or("<non-utf8>")
                );
                eprint!("{EXPLAIN_USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }

    let catalog: toml::Value = match toml::from_str(ERRORS_TOML) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("cpc explain: internal error parsing the embedded diagnostic catalog: {e}");
            return ExitCode::FAILURE;
        }
    };
    let codes = catalog
        .get("code")
        .and_then(|c| c.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);

    if list || query.is_none() {
        return explain_list(codes);
    }

    let want = normalize_code(&query.unwrap());
    match codes.iter().find(|e| {
        e.get("id")
            .and_then(|v| v.as_str())
            .is_some_and(|id| id.eq_ignore_ascii_case(&want))
    }) {
        Some(entry) => {
            print!("{}", format_code(entry));
            ExitCode::SUCCESS
        }
        None => {
            eprintln!("cpc explain: no diagnostic code `{want}`.");
            eprintln!(
                "Run `cpc explain --list` to see all {} codes, or read",
                codes.len()
            );
            eprintln!("https://cplus-lang.dev/docs/error-codes.md");
            ExitCode::FAILURE
        }
    }
}

// ---- `cpc pm` : the package manager, unified under cpc ----------------------

/// The toolchain monorepo: where bare `*` dependencies come from (D15). The
/// `cplus-pm` crate deliberately has no default org; `cpc` is what supplies
/// the toolchain identity, because `cpc` is the toolchain.
const TOOLCHAIN_REPO: &str = "github.com/netdur/cplus";

/// `cpc pm <command> ...` — dispatch to the package manager (the same
/// dispatcher as the standalone `cplus-pm` binary). Shipping it under `cpc`
/// means the one Homebrew-installed toolchain carries the package manager
/// too — and, running inside the toolchain, it passes the toolchain's own
/// identity: that is what resolves bare `stdlib = "*"` deps and names the
/// store tier, version-locking official packages to this compiler.
fn run_pm(args: &[OsString]) -> ExitCode {
    let strs: Option<Vec<String>> =
        args.iter().map(|a| a.to_str().map(String::from)).collect();
    let Some(strs) = strs else {
        eprintln!("cpc pm: arguments must be valid UTF-8");
        return ExitCode::FAILURE;
    };
    let toolchain = cplus_pm::store::ToolchainContext {
        repo: TOOLCHAIN_REPO.to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        package_root: "vendor".to_string(),
    };
    match cplus_pm::cli::run_with_toolchain(strs, Some(toolchain)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

// ---- `cpc init` : scaffold a new project ------------------------------------

const INIT_USAGE: &str = "\
cpc init - scaffold a new C+ project

usage:
  cpc init [--kind K] [--platform P]... [NAME]
                    create a project. With NAME, scaffold into NAME/; without,
                    scaffold in the current directory (name = directory name).

                    With no --platform: the zero-config HOST app —
                    src/main.cplus is the default entry, and what a build
                    produces is the target platform's fact.

                    With --platform (repeatable, same vocabulary as
                    `cpc pm add`): a deliberately SCOPED app — each platform
                    gets an `[<platform>] entry` section, and building for a
                    platform you did not name is an error (E0413), never a
                    guess. iOS/Android entries are scaffolded in the
                    external-builder shape (`export extern fn <name>_main`,
                    the symbol Xcode's/Gradle's own main calls); everything
                    else gets a normal `fn main`.

  --kind cli        a program with a `fn main` that prints. The default for a
                    project that names no platform, and for desktop platforms.

  --kind gui        a facet APP: src/app.cplus (one screen, shared by every
                    platform named), one entry per platform, and the backend's
                    full dependency closure in the manifest — no hand edits
                    between `init` and a window on screen.

                    WITHOUT --kind, the PLATFORM decides. --platform ios is
                    gui and cannot be anything else: iOS has no console and no
                    window a `fn main` could open, so a printing entry is a
                    black rectangle on a phone. Everything else defaults to
                    cli, because a desktop platform can legitimately be either
                    and only you know which — which is exactly the question
                    --kind exists to answer. `--kind cli --platform ios` is
                    refused rather than obeyed.

                    Backends: macOS gets facet_appkit, iOS facet_uikit,
                    Android facet_android. A gui project naming a platform
                    with no facet backend scaffolds the shared app and says
                    which entry you will have to finish yourself.

writes:  Cplus.toml, src/main*.cplus, .gitignore, SKILL.md,
         AGENTS.md, .mcp.json
    gui: + src/app.cplus
    ios: + ios/main.m, ios/Info.plist
android: + android/AndroidManifest.xml
";

/// What kind of program this project is. Not a boolean: C+ targets printing
/// programs, facet apps and (one day) firmware, and those are different
/// templates with different entries — a `--gui` flag could only ever name two
/// of the three.
#[derive(Clone, Copy, PartialEq, Eq)]
enum InitKind {
    Cli,
    Gui,
}

/// `cpc init [--kind K] [--platform P]... [NAME]`.
fn run_init(args: &[OsString]) -> ExitCode {
    let mut name: Option<String> = None;
    let mut platforms: Vec<String> = Vec::new();
    let mut want_platform = false;
    let mut want_kind = false;
    let mut kind: Option<InitKind> = None;
    for a in args {
        match a.to_str() {
            _ if want_kind => {
                kind = match a.to_str() {
                    Some("cli") => Some(InitKind::Cli),
                    Some("gui") => Some(InitKind::Gui),
                    other => {
                        eprintln!(
                            "cpc init: unknown kind `{}`; one of: cli, gui",
                            other.unwrap_or("<non-utf8>")
                        );
                        return ExitCode::FAILURE;
                    }
                };
                want_kind = false;
            }
            _ if want_platform => {
                let p = match a.to_str() {
                    Some(p) => p,
                    None => {
                        eprintln!("cpc init: --platform value must be valid UTF-8");
                        return ExitCode::FAILURE;
                    }
                };
                if !cplus_core::target::PLATFORMS.contains(&p) {
                    eprintln!(
                        "cpc init: unknown platform `{p}`; one of: {}",
                        cplus_core::target::PLATFORMS.join(", ")
                    );
                    return ExitCode::FAILURE;
                }
                if !platforms.iter().any(|q| q == p) {
                    platforms.push(p.to_string());
                }
                want_platform = false;
            }
            Some("--platform") => want_platform = true,
            Some("--kind") | Some("--template") => want_kind = true,
            Some("-h") | Some("--help") => {
                print!("{INIT_USAGE}");
                return ExitCode::SUCCESS;
            }
            Some(s) if name.is_none() && !s.starts_with('-') => name = Some(s.to_string()),
            other => {
                eprintln!("cpc init: unexpected argument `{}`", other.unwrap_or("<non-utf8>"));
                eprint!("{INIT_USAGE}");
                return ExitCode::FAILURE;
            }
        }
    }

    if want_platform {
        eprintln!("cpc init: --platform requires a value");
        return ExitCode::FAILURE;
    }
    if want_kind {
        eprintln!("cpc init: --kind requires a value (cli or gui)");
        return ExitCode::FAILURE;
    }

    // Where to scaffold. An existing project takes precedence over every other
    // check, so re-running `cpc init` in a project reports the real reason.
    let root = match &name {
        Some(n) => PathBuf::from(n),
        None => PathBuf::from("."),
    };
    let manifest = root.join("Cplus.toml");
    if manifest.exists() {
        eprintln!(
            "cpc init: {} already exists — refusing to overwrite an existing project",
            manifest.display()
        );
        return ExitCode::FAILURE;
    }

    // The package name is the final path component, so `cpc init path/to/demo`
    // scaffolds into that directory but names the package `demo`. With no
    // argument, it's the current directory's name.
    let proj_name = root
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .or_else(|| {
            env::current_dir()
                .ok()
                .and_then(|d| d.file_name().map(|f| f.to_string_lossy().into_owned()))
        })
        .unwrap_or_else(|| "app".to_string());
    if proj_name.is_empty()
        || !proj_name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        eprintln!("cpc init: project name `{proj_name}` must be alphanumeric (plus `_` or `-`)");
        if name.is_none() {
            eprintln!("    (derived from the directory name; pass an explicit name: `cpc init NAME`)");
        }
        return ExitCode::FAILURE;
    }

    let src = root.join("src");
    if let Err(e) = std::fs::create_dir_all(&src) {
        eprintln!("cpc init: could not create {}: {e}", src.display());
        return ExitCode::FAILURE;
    }

    // D15: a bare `*` is the toolchain's own package at the toolchain's
    // version — `cpc pm install` supplies the context, and the store tier
    // version-locks stdlib to this compiler by construction. Third-party
    // deps use the pinned tree-URL form instead.
    //
    // No --platform, no target section: `src/main.cplus` is the default
    // entry, and what a build produces is the target platform's fact. With
    // --platform, each named platform gets an explicit `[<p>] entry` — the
    // app is scoped deliberately, and a platform left out is E0413, never a
    // guess. The first-named platform owns src/main.cplus; the rest get
    // src/main_<p>.cplus, because entry SHAPES differ (an iOS/Android entry
    // is an `export extern fn` the platform's own main calls — those
    // targets produce a staticlib, and a library has no entry the system
    // knows to call).
    // Which platform owns the plain `src/main.cplus`. Normally the first named,
    // but NOT iOS when a self-linked platform is also named: `cpc build` for
    // that platform would then report the iOS entry as unreachable (W0005),
    // because the warning's exemption keys on the `main_<platform>.cplus`
    // convention rather than on "is declared as some platform's entry". A
    // fresh scaffold must not warn on its first build.
    let main_owner: Option<&String> = platforms
        .iter()
        .find(|p| p.as_str() != "ios")
        .or_else(|| platforms.first());
    let entry_file = |p: &str| -> String {
        if Some(p) == main_owner.map(String::as_str) {
            "main.cplus".to_string()
        } else {
            format!("main_{p}.cplus")
        }
    };

    let mut sections = String::new();
    for p in platforms.iter() {
        sections.push_str(&format!("[{p}]\nentry = \"src/{}\"\n\n", entry_file(p)));
    }
    let ios = platforms.iter().any(|p| p == "ios");

    // Which template. iOS has no console and no window a `fn main` could open:
    // an iOS target builds a STATICLIB that an app bundle links, and the first
    // thing the shell's `main.m` calls has to put a UI on screen or the app is
    // a black rectangle. So iOS IMPLIES gui and cannot be told otherwise.
    //
    // Every other platform can legitimately be either, and nothing in a
    // platform name says which — so the default is the printing program (the
    // smaller, more surprising-if-wrong answer) and `--kind gui` is how you say
    // otherwise. Before this flag existed a desktop-only facet project fell
    // through to the hello-world and took three rounds of manifest edits to
    // reach a window, which is the gap this closes.
    let gui = match kind {
        Some(InitKind::Gui) => {
            if platforms.is_empty() {
                // The backend closure is written into a `[<platform>.dependencies]`
                // section, so a facet app has to know which platform it is for.
                // A `[dependencies] facet_appkit` would be a lie the moment
                // anyone built the same manifest for anything else.
                eprintln!(
                    "cpc init: --kind gui needs at least one --platform — a facet app's backend\n\
                     (facet_appkit, facet_uikit) is named per platform in the manifest, and\n\
                     there is no platform-agnostic way to write it. Try:\n\
                     \n    cpc init --kind gui --platform macos"
                );
                return ExitCode::FAILURE;
            }
            true
        }
        Some(InitKind::Cli) => {
            if ios {
                eprintln!(
                    "cpc init: --kind cli cannot be combined with --platform ios — iOS has no\n\
                     console and no window a `fn main` could open, so a printing entry is a\n\
                     black rectangle on a phone. Drop --kind, or drop --platform ios."
                );
                return ExitCode::FAILURE;
            }
            false
        }
        None => ios,
    };

    // The platforms a facet backend is scaffolded for. A gui project naming
    // anything else still gets `src/app.cplus` and an entry that calls it —
    // the app is portable, the BACKEND is what is missing — and is told so
    // rather than being handed a manifest that fails to resolve at build time
    // with no hint about why.
    let backed: Vec<&String> = platforms
        .iter()
        .filter(|p| matches!(p.as_str(), "macos" | "ios" | "android"))
        .collect();
    if gui {
        let unbacked: Vec<&str> = platforms
            .iter()
            .map(String::as_str)
            .filter(|p| !matches!(*p, "macos" | "ios" | "android"))
            .collect();
        if !unbacked.is_empty() {
            eprintln!(
                "cpc init: no facet backend is scaffolded for {} — src/app.cplus and the entry\n\
                 are written, but you will have to name a backend for {} in Cplus.toml yourself.",
                unbacked.join(", "),
                if unbacked.len() == 1 { "it" } else { "them" }
            );
        }
    }

    // The backend and its transitive closure, per platform. The resolver
    // validates every import in the build against ONE flat set taken from THIS
    // manifest — it does not read a dependency's own — so facet_appkit's and
    // facet_uikit's deps are named here too. This is the minimum that links:
    // `webkit` is not optional, because each backend's `web.cplus` imports it
    // unconditionally.
    let backend_deps = |p: &str| -> String {
        match p {
            "ios" => "\n# facet's iOS backend and everything it imports. The resolver checks\n\
                      # every import against this one flat set, so the closure is named here.\n\
                      [ios.dependencies]\n\
                      facet_uikit = \"*\"\n\
                      uikit       = \"*\"\n\
                      objc        = \"*\"\n\
                      quartzcore  = \"*\"\n\
                      webkit      = \"*\"\n\n\
                      # What the agent surface in src/app.cplus links, named here for\n\
                      # the same flat-set reason as the backend's own closure.\n\
                      inspector   = \"*\"\n\
                      facet_agent = \"*\"\n\
                      agent_uikit = \"*\"\n\
                      agent_core  = \"*\"\n\
                      agent_inapp = \"*\"\n\
                      agent_mcp   = \"*\"\n\
                      json        = \"*\"\n"
                .to_string(),
            "macos" => "\n# facet's AppKit backend and its closure.\n\
                        [macos.dependencies]\n\
                        facet_appkit = \"*\"\n\
                        appkit       = \"*\"\n\
                        objc         = \"*\"\n\
                        quartzcore   = \"*\"\n\
                        webkit       = \"*\"\n\n\
                        # What the agent surface in src/app.cplus links. Named here for\n\
                        # the same reason the backend's closure is: the resolver checks every\n\
                        # import against this one flat set. Delete these with that line if\n\
                        # you would rather the binary could not be inspected.\n\
                        inspector    = \"*\"\n\
                        facet_agent  = \"*\"\n\
                        agent_appkit = \"*\"\n\
                        agent_core   = \"*\"\n\
                        agent_inapp  = \"*\"\n\
                        agent_mcp    = \"*\"\n\
                        json         = \"*\"\n"
                .to_string(),
            // The third full closure, the same shape as its two neighbours.
            //
            // `agent_android` is the Android sibling of agent_appkit and
            // agent_uikit and walks facet's OWN node tree rather than a native
            // hierarchy — see its manifest. `events` is NOT here despite what
            // examples/facet_gallery_android declares; that is the gallery's own
            // import, not the backend's.
            "android" => "\n# facet's Android backend and its closure. The JVM half — the Activity\n\
                          # the manifest names — ships inside facet_android as a precompiled\n\
                          # dex, so this app has no Java of its own; the packaging step merges it.\n\
                          [android.dependencies]\n\
                          facet_android = \"*\"\n\
                          android_view  = \"*\"\n\
                          jni           = \"*\"\n\n\
                          # What the agent surface in src/app.cplus links, named here\n\
                          # for the same flat-set reason as the backend's own closure. It\n\
                          # listens on a loopback PORT here rather than a socket: an app's\n\
                          # files live under /data/data/<pkg>, which your machine cannot\n\
                          # reach. The port is derived from the pid, so a launcher that\n\
                          # spawned the app can work it out; `adb forward tcp:P tcp:P` is\n\
                          # the hop.\n\
                          inspector     = \"*\"\n\
                          facet_agent   = \"*\"\n\
                          agent_android = \"*\"\n\
                          agent_core    = \"*\"\n\
                          agent_inapp   = \"*\"\n\
                          agent_mcp     = \"*\"\n\
                          json          = \"*\"\n"
                .to_string(),
            _ => String::new(),
        }
    };

    let manifest_toml = if gui {
        let closures: String = backed.iter().map(|p| backend_deps(p)).collect();
        format!(
            "[package]\nname    = \"{proj_name}\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             {sections}[dependencies]\n\
             stdlib        = \"*\"\n\
             facet         = \"*\"\n\
             facet_runtime = \"*\"\n\
             flex_layout   = \"*\"\n{closures}"
        )
    } else {
        format!(
            "[package]\nname    = \"{proj_name}\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
             {sections}[dependencies]\nstdlib = \"*\"\n"
        )
    };

    let desktop_main = "import \"stdlib/io\" as io;\n\n\
         fn main() -> i32 {\n    io::println(\"hello from C+\");\n    return 0;\n}\n";
    // The external-builder shape: a stable, unmangled C symbol in the
    // generated header, which is how an app bundle's own `main` reaches C+
    // code (see examples/facet_gallery_ios and examples/DEPLOYING.md).
    let sym = proj_name.replace('-', "_");
    let external_main = |p: &str| {
        format!(
            "// {proj_name} — the entry point the {p} app shell calls.\n\
             //\n\
             // `export extern fn` gives the symbol a stable, unmangled C name and puts\n\
             // it in the generated header. A `fn main` would not do: this target\n\
             // produces a STATICLIB, and a library has no entry the system knows to\n\
             // call. See examples/DEPLOYING.md for the app-shell recipe.\n\n\
             import \"stdlib/io\" as io;\n\n\
             export extern fn {sym}_main() -> i32 {{\n    io::println(\"hello from C+\");\n    return 0;\n}}\n"
        )
    };
    // ---- the facet app scaffold (`--kind gui`, implied by --platform ios) ---
    //
    // Three files, and the split is the point: `app.cplus` is the app and is
    // shared by every platform, while each entry is only the door its platform
    // knows how to open. iOS enters through an `export extern fn` the bundle's
    // `main.m` calls; a self-linked platform enters through `fn main`. Neither
    // installs a backend — `facet_runtime` picks its own per-platform file and
    // does that itself.
    let facet_app = format!(
        "// {proj_name} — the app: one screen, and the runtime that hosts it.\n\
         //\n\
         // facet is RETAINED and NOT reactive: `build` runs once, at mount, and\n\
         // returns a live tree. To change what is on screen, find the node by key\n\
         // and set the property — never rebuild. `cpc skill` prints facet's own\n\
         // reference (this project depends on facet, so it is included).\n\n\
         import \"flex_layout/flex_layout\" as flex;\n\
         import \"facet/facet\" as core;\n\
         import \"facet/elements\" as ui;\n\
         import \"facet/component\" as component;\n\
         import \"facet/label\" as label;\n\
         import \"facet/screen\" as screen;\n\
         import \"facet/vocabulary\" as vocab;\n\
         import \"facet_runtime/runtime\" as runtime;\n\
         import \"facet_agent/agent\" as agent;\n\

         import \"./agent_consent\" as agent_consent;\n\
         import \"stdlib/option\" as option;\n\
         import \"stdlib/vec\" as vec;\n\n\
         struct Home {{\n    taps: i64,\n}}\n\n\
         impl Home {{\n\
         \x20   fn new() -> Home {{ return Home {{ taps: 0 }}; }}\n\n\
         \x20   // A handler is a bound METHOD — `on_click: this.on_tap` wires it, and\n\
         \x20   // the compiler fills the context slot. Never hand-roll `#addr_of(this)`.\n\
         \x20   fn on_tap(ref this, sender: *u8) {{\n\
         \x20       this.taps = this.taps + 1;\n\
         \x20       this.show_taps();\n\
         \x20   }}\n\n\
         \x20   // The live tree, reached through a TYPED cursor. A label is not a\n\
         \x20   // button: the wrong kind of `find` answers None and does nothing.\n\
         \x20   fn show_taps(this) {{\n\
         \x20       let n: i64 = this.taps;\n\
         \x20       if let option::Option::Some(l) = label::find(\"taps\") {{\n\
         \x20           let _l: label::Label = l.set_text(\"tapped ${{n}}\");\n\
         \x20       }}\n\
         \x20   }}\n\
         }}\n\n\
         impl Home: component::Component {{\n\
         \x20   fn build(ref this) -> core::Node {{\n\
         \x20       return @ui {{\n\
         \x20           column {{\n\
         \x20               label(\"Hello from C+\", key: \"hello\",\n\
         \x20                     font_size: 28.0,\n\
         \x20                     font_weight: vocab::FontWeight::Bold)\n\
         \x20               label(\"tapped 0\", key: \"taps\")\n\
         \x20               button(\"Tap me\", key: \"tap\", on_click: this.on_tap)\n\
         \x20           }}\n\
         \x20               .grow(1.0)\n\
         \x20               .gap(12.0)\n\
         \x20               .padding(24.0)\n\
         \x20               .align(flex::Align::Center)\n\
         \x20               .justify(flex::Justify::Center)\n\
         \x20       }};\n\
         \x20   }}\n\
         }}\n\n\
         impl Home: component::Lifecycle {{\n\
         \x20   fn on_attach(ref this) {{ }}\n\
         \x20   fn on_detach(ref this) {{ }}\n\
         }}\n\n\
         impl Home: screen::Screen {{\n\
         \x20   fn chrome(this) -> screen::Chrome {{\n\
         \x20       // Width and height describe nothing on a phone — the screen IS the\n\
         \x20       // window — but they are the same facade on both platforms and the\n\
         \x20       // iOS backend drops them.\n\
         \x20       return screen::Chrome::new(title: \"{proj_name}\",\n\
         \x20                                  width: 390.0, height: 844.0);\n\
         \x20   }}\n\
         \x20   fn menu_items(this) -> vec::Vec[screen::MenuItem] {{\n\
         \x20       return vec::new::[screen::MenuItem]();\n\
         \x20   }}\n\
         }}\n\n\
         // Every entry — macOS, iOS and Android alike — comes through here.\n\
         //\n\
         // `run_screen` is the tier ALL THREE backends implement, and it reads\n\
         // `chrome()` above for the title and size (a phone honours the part of\n\
         // a Chrome a phone has). There is a larger tier — `runtime::App`, with\n\
         // `app.screen(\"name\", factory)` and named routes — and it is where to\n\
         // go when this app grows a second screen. It is NOT the default here\n\
         // because facet's Android facade does not implement `App::run` yet: it\n\
         // warns on `adb logcat -s facet` and returns InvalidInput, so an app\n\
         // built on it launches to a blank Activity.\n\
         fn run() -> i32 {{\n\
         \x20   // DRIVEABLE BY AN AGENT, and by the IDE that launched it. Two\n\
         \x20   // lines, each saying what it does:\n\
         \x20   //\n\
         \x20   //   enable()      fills the serving seam (without it, nothing serves)\n\
         \x20   //   agent_mcp(id) names THIS APP; the platform derives where it\n\
         \x20   //                 listens — `/tmp/mcp-{proj_name}-<pid>.socket` on a\n\
         \x20   //                 desktop, a loopback port on a phone. A launcher\n\
         \x20   //                 knows the pid it spawned, so it can work the\n\
         \x20   //                 address out without being told.\n\
         \x20   //\n\
         \x20   // That is the whole opt-in. All 25 verbs come with it — the eleven\n\
         \x20   // that drive the app as a person would, and the fourteen that\n\
         \x20   // inspect it as a developer would, seeing unexposed nodes and\n\
         \x20   // writing properties that are not user affordances. There is no\n\
         \x20   // third line: the serving facade installs the tree walker, so\n\
         \x20   // being served and being inspectable are one decision.\n\
         \x20   //\n\
         \x20   // Delete both (and the agent packages from Cplus.toml) if you\n\
         \x20   // would rather this binary could not be driven.\n\
         \x20   //\n\
         \x20   // ANYTHING THAT CONNECTS IS ADMITTED. To ask the user first, add\n\
         \x20   // `agent_consent::install();` above — see src/agent_consent.cplus.\n\
         \x20   agent::enable();\n\
         \x20   runtime::agent_mcp(\"{proj_name}\");\n\
         \x20   runtime::run_screen(Home::new());\n\
         \x20   return 0;\n\
         }}\n"
    );

    let facet_ios_entry = format!(
        "// {proj_name} — the entry point the iOS app shell calls.\n\
         //\n\
         // `export extern fn` gives the symbol a stable, unmangled C name and puts\n\
         // it in the generated header. A `fn main` would not do: this target\n\
         // produces a STATICLIB, and a library has no entry the system knows to\n\
         // call. It does not return — `UIApplicationMain` owns the process from\n\
         // here — so the value below is unreachable in a running app.\n\n\
         import \"./app\" as app;\n\n\
         export extern fn {sym}_main() -> i32 {{\n\
         \x20   // The agent surface is armed in src/app.cplus, the one file every\n\
         \x20   // platform builds — there is nothing platform-shaped about it. On\n\
         \x20   // this platform it listens on a loopback PORT rather than a socket\n\
         \x20   // path, because a Unix socket sits inside the app sandbox where the\n\
         \x20   // launcher cannot reach it; the port is derived from the pid, which\n\
         \x20   // `simctl launch` and `devicectl` both report.\n\
         \x20   return app::run();\n}}\n"
    );

    // Android's entry is the iOS shape — `export extern fn {sym}_main` — and
    // NOT the desktop one, which is the bug this replaced: an android entry
    // written as `fn main` is rejected outright (E0409, "this build produces a
    // library archive — `fn main` has no caller here"), so a scaffolded
    // project did not compile once.
    //
    // Nothing here is Android-shaped, and that is the design: facet_android's
    // Activity owns the JNI surface, finds this function by NAME through the
    // `cplus.facet.main` meta-data in android/AndroidManifest.xml, and dlsym's
    // it out of the .so. The app writes no Java and no JNI.
    //
    // It arms NOTHING of its own, like both its neighbours: the agent surface
    // is set up in src/app.cplus, which every platform builds. That used to be
    // three per-platform `serve_if_asked` calls reading three differently-spelled
    // channels; the address is derived from the pid now, so there is no channel
    // and no per-platform line.
    //
    // It DOES return, unlike iOS. `onCreate` called in, takes a View back and
    // returns to the looper that was already running — so the Android facade's
    // `App::run` builds and STORES the tree rather than entering a loop, and
    // the 0 below is reached on every launch.
    let facet_android_entry = format!(
        "// {proj_name} — the entry point facet_android's Activity calls.\n\
         //\n\
         // `export extern fn` gives the symbol a stable, unmangled C name in the\n\
         // .so. A `fn main` would not do: this target produces a STATICLIB, and a\n\
         // library has no entry the system knows to call.\n\
         //\n\
         // The Activity finds this function BY NAME — `cplus.facet.main` in\n\
         // android/AndroidManifest.xml — and dlsym's it, so nothing above this\n\
         // line is Android-shaped and src/app.cplus is the same file every\n\
         // platform builds. Rename the function and you must rename it there too.\n\n\
         import \"./app\" as app;\n\n\
         export extern fn {sym}_main() -> i32 {{\n\
         \x20   // The agent surface is armed in src/app.cplus, the one file every\n\
         \x20   // platform builds. It listens on a loopback port derived from this\n\
         \x20   // process\'s pid, so nothing has to be passed in — an Activity has no\n\
         \x20   // environment for a launcher to set, which is what the old system\n\
         \x20   // property was working around:\n\
         \x20   //\n\
         \x20   //     adb shell am start -n <pkg>/cplus.facet.FacetActivity\n\
         \x20   //     pid=$(adb shell pidof <pkg>); adb forward tcp:$((9000+pid%1000)) tcp:$((9000+pid%1000))\n\
         \x20   //\
         \x20   // This RETURNS, unlike the iOS entry: onCreate is calling in and\n\
         \x20   // needs its View back, so `App::run` builds the tree and stores it\n\
         \x20   // rather than entering a loop it would never leave.\n\
         \x20   return app::run();\n}}\n"
    );

    // Every desktop entry is now the same three lines: the agent surface is
    // armed in src/app.cplus, not here. It used to be macOS-only and in the
    // entry, because the inspector was an AppKit package reached through
    // `serve_if_asked` reading FACET_INSPECT — a second way to start the same
    // server, which silently decided whether eleven of the socket's twenty-one
    // verbs existed. See plan.md.
    let facet_desktop_entry = |p: &str| -> String {
        if p == "macos" {
            format!(
                "// {proj_name} — the desktop entry. The same `app::run` the iOS shell\n\
                 // calls; `facet_runtime` selects its own per-platform backend, so there\n\
                 // is nothing to install here.\n\n\
                 import \"./app\" as app;\n\n\
                 fn main() -> i32 {{\n\
                 \x20   // The agent surface is armed in src/app.cplus, the one file\n\
                 \x20   // every platform builds.\n\
                 \x20   return app::run();\n}}\n"
            )
        } else {
            format!(
                "// {proj_name} — the desktop entry. The same `app::run` the iOS shell\n\
                 // calls; `facet_runtime` selects its own per-platform backend, so there\n\
                 // is nothing to install here.\n\n\
                 import \"./app\" as app;\n\n\
                 fn main() -> i32 {{\n    return app::run();\n}}\n"
            )
        }
    };

    // The bundle's `main` hands the process to C+ and never gets it back:
    // `UIApplicationMain` owns it from there. There is no AppDelegate.m and no
    // storyboard — facet_uikit synthesizes `FacetUIKitAppDelegate` at runtime
    // and builds the UIWindow in didFinishLaunchingWithOptions, which is why
    // the plist below names neither.
    let ios_main_m = format!(
        "// The app bundle's entry point: `main` hands the process to C+, and\n\
         // {sym}_main never comes back — it calls UIApplicationMain, which owns\n\
         // the process from there. facet_uikit synthesizes both its delegate class\n\
         // and its scene delegate at runtime, so Info.plist NAMES the scene (every\n\
         // windowing API on iPadOS hangs off one) and must not name a storyboard\n\
         // (UIKit would wait for a nib facet has not got, and the screen stays\n\
         // black).\n\n\
         #import \"{proj_name}.h\"   // generated by cpc into target/<triple>/debug/\n\n\
         int main(int argc, char *argv[]) {{\n    return {sym}_main();\n}}\n"
    );

    // THE BUNDLE ID IS NOT THE PACKAGE NAME. Apple builds an App ID *name* out
    // of the identifier — `dev.cplus.test_app` becomes "XC dev cplus test_app"
    // — and rejects the result if it holds anything but alphanumerics, spaces,
    // hyphens and periods. A C+ package name is full of underscores, so the
    // obvious substitution mints nothing:
    //
    //     error: An attribute in the provided entity has invalid value:
    //            The attribute 'name' is invalid: 'XC dev cplus test_app'
    //     error: No profiles for 'dev.cplus.test_app' were found
    //
    // Measured against a real iPad. The failure arrives at signing time, weeks
    // after `init`, and reads as a provisioning problem rather than as a name
    // this file chose — which is why the rule lives here rather than in advice.
    // iris's own scaffold reduces the same way, so a project made by either
    // route gets the same id.
    let app_id: String = {
        let cleaned: String = proj_name.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        if cleaned.is_empty() { "app".to_string() } else { cleaned }
    };

    // A bundle display name has to start somewhere; the package name with its
    // first letter raised reads better on a home screen than `myapp`.
    let display = {
        let mut c = proj_name.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => proj_name.clone(),
        }
    };
    let ios_plist = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \x20   <key>CFBundleDisplayName</key>\n\
         \x20   <string>{display}</string>\n\
         \x20   <key>CFBundleExecutable</key>\n\
         \x20   <string>{display}</string>\n\
         \x20   <key>CFBundleIdentifier</key>\n\
         \x20   <string>dev.cplus.{app_id}</string>\n\
         \x20   <key>CFBundleName</key>\n\
         \x20   <string>{display}</string>\n\
         \x20   <key>CFBundlePackageType</key>\n\
         \x20   <string>APPL</string>\n\
         \x20   <key>CFBundleShortVersionString</key>\n\
         \x20   <string>1.0</string>\n\
         \x20   <key>CFBundleVersion</key>\n\
         \x20   <string>1</string>\n\
         \x20   <key>LSRequiresIPhoneOS</key>\n\
         \x20   <true/>\n\
         \x20   <key>MinimumOSVersion</key>\n\
         \x20   <string>14.0</string>\n\
         \x20   <!-- A SCENE, because every windowing API on iPadOS hangs off one:\n\
         \x20        the control style, the geometry callback, the size\n\
         \x20        restrictions. An app with no manifest is not answering them\n\
         \x20        badly, it is never asked. facet_uikit synthesizes\n\
         \x20        `FacetUIKitSceneDelegate` before UIApplicationMain and ADOPTS\n\
         \x20        the window the app delegate already built, so naming it here\n\
         \x20        changes nothing about how the app starts.\n\
         \x20        No UISceneStoryboardFile: a storyboard makes UIKit wait for a\n\
         \x20        nib facet does not have, and the screen stays black. That is\n\
         \x20        the key that was correctly avoided; this one is not it. -->\n\
         \x20   <key>UIApplicationSceneManifest</key>\n\
         \x20   <dict>\n\
         \x20       <key>UIApplicationSupportsMultipleScenes</key>\n\
         \x20       <false/>\n\
         \x20       <key>UISceneConfigurations</key>\n\
         \x20       <dict>\n\
         \x20           <key>UIWindowSceneSessionRoleApplication</key>\n\
         \x20           <array>\n\
         \x20               <dict>\n\
         \x20                   <key>UISceneConfigurationName</key>\n\
         \x20                   <string>Default Configuration</string>\n\
         \x20                   <key>UISceneDelegateClassName</key>\n\
         \x20                   <string>FacetUIKitSceneDelegate</string>\n\
         \x20               </dict>\n\
         \x20           </array>\n\
         \x20       </dict>\n\
         \x20   </dict>\n\
         \x20   <key>UIDeviceFamily</key>\n\
         \x20   <array>\n\
         \x20       <integer>1</integer>\n\
         \x20       <integer>2</integer>\n\
         \x20   </array>\n\
         \x20   <key>UILaunchScreen</key>\n\
         \x20   <dict/>\n\
         \x20   <!-- ALL FOUR, and this is not a preference. iPadOS grants an app a\n\
         \x20        real, resizable window only if it supports every orientation;\n\
         \x20        with three it hands out a FULL-SCREEN-SIZED canvas and scales\n\
         \x20        that into whatever window the user drags. Measured on an iPad\n\
         \x20        Pro 11: with three orientations the app was told 834x1194 while\n\
         \x20        its window was 504x722 — a 0.60 scale, which reads as \"the text\n\
         \x20        is too big\" and hides every layout bug behind it. With four it\n\
         \x20        is told 507x727, the size it actually has. -->\n\
         \x20   <key>UISupportedInterfaceOrientations</key>\n\
         \x20   <array>\n\
         \x20       <string>UIInterfaceOrientationPortrait</string>\n\
         \x20       <string>UIInterfaceOrientationPortraitUpsideDown</string>\n\
         \x20       <string>UIInterfaceOrientationLandscapeLeft</string>\n\
         \x20       <string>UIInterfaceOrientationLandscapeRight</string>\n\
         \x20   </array>\n\
         \x20   <key>UISupportedInterfaceOrientations~ipad</key>\n\
         \x20   <array>\n\
         \x20       <string>UIInterfaceOrientationPortrait</string>\n\
         \x20       <string>UIInterfaceOrientationPortraitUpsideDown</string>\n\
         \x20       <string>UIInterfaceOrientationLandscapeLeft</string>\n\
         \x20       <string>UIInterfaceOrientationLandscapeRight</string>\n\
         \x20   </array>\n\
         </dict>\n\
         </plist>\n"
    );

    // ---- the Android side --------------------------------------------------
    //
    // THE MANIFEST IS NOT BOILERPLATE HERE, which is why it is scaffolded at
    // all. `aapt2 link` takes it as a required input — there is no default and
    // nothing downstream can synthesize one — and three of its lines are facet
    // wiring an app cannot derive from anything it owns:
    //
    //   - the Activity is `cplus.facet.FacetActivity`, which lives in
    //     facet_android and ships as a precompiled dex. Guess `MainActivity`
    //     and the app dies at launch with ClassNotFoundException.
    //   - `cplus.facet.lib` is the .so basename, which must agree with what
    //     the packaging step passes to `-o`. Neither name is derivable from the
    //     other, so whoever packages the APK reads this value rather than
    //     guessing it — iris does exactly that (services/android_deploy.cplus).
    //   - `cplus.facet.main` is the entry SYMBOL, dlsym'd by name. It is the
    //     `export extern fn {sym}_main` in src/main_android.cplus.
    //
    // `configChanges` is the fourth: without it Android destroys and recreates
    // the Activity on every rotation, which tears down the mounted facet tree.
    //
    // `package=` is deprecated under AGP (it moved to `namespace` in Gradle),
    // but this is the no-Gradle path and `aapt2` still requires it.
    //
    // minSdk/targetSdk are deliberately NOT a `<uses-sdk>` element here: they
    // are aapt2 flags at package time, so there is one source of truth for them
    // rather than two that can disagree.
    //
    // A Java package may not begin with a digit, and `app_id` is the project
    // name with everything but alphanumerics stripped — so `9lives` would mint
    // `cplus.9lives`, which aapt2 rejects.
    let android_pkg = format!(
        "cplus.{}",
        if app_id.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            format!("app{app_id}")
        } else {
            app_id.clone()
        }
    );
    let android_manifest = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n\
         <manifest xmlns:android=\"http://schemas.android.com/apk/res/android\"\n\
         \x20   package=\"{android_pkg}\">\n\
         \x20   <!-- FOR THE INSPECTOR, and for nothing else this project does.\n\
         \x20        the agent surface in src/app.cplus binds a listening\n\
         \x20        socket on LOOPBACK, and Android gates socket() on the app's\n\
         \x20        membership of the inet group — which this permission is what\n\
         \x20        grants. Without it the bind fails with EACCES, the accept loop\n\
         \x20        ends the instant it starts, and an IDE that forwarded the\n\
         \x20        port connects to nothing while the app runs perfectly.\n\
         \x20        Measured, on an emulator, before this line existed.\n\n\
         \x20        Delete it with the three lines in src/app.cplus if you would\n\
         \x20        rather the app could not be inspected: they belong together. -->\n\
         \x20   <uses-permission android:name=\"android.permission.INTERNET\" />\n\
         \x20   <application android:label=\"{display}\"\n\
         \x20                android:theme=\"@android:style/Theme.DeviceDefault.DayNight\">\n\
         \x20       <!-- THE ACTIVITY IS FACET'S, not this app's. It ships inside\n\
         \x20            facet_android as a precompiled dex and is merged into\n\
         \x20            classes.dex when the APK is packaged, so this app has no\n\
         \x20            Java of its own.\n\n\
         \x20            configChanges is not a preference: without it the system\n\
         \x20            destroys and recreates the Activity on rotation, and the\n\
         \x20            mounted facet tree goes with it. -->\n\
         \x20       <activity android:name=\"cplus.facet.FacetActivity\"\n\
         \x20                 android:exported=\"true\"\n\
         \x20                 android:configChanges=\"orientation|screenSize|keyboardHidden\">\n\
         \x20           <!-- Which .so to load, the way NativeActivity takes\n\
         \x20                android.app.lib_name — the Activity is generic across\n\
         \x20                apps and cannot know the name. It must match the `-o`\n\
         \x20                `-o` of whatever links the .so. -->\n\
         \x20           <meta-data android:name=\"cplus.facet.lib\" android:value=\"{sym}\" />\n\
         \x20           <!-- The app's entry, found BY NAME: facet_android's Activity\n\
         \x20                dlsym's it, which is what lets src/app.cplus be the same\n\
         \x20                source every platform builds. It is the\n\
         \x20                `export extern fn` in src/main_android.cplus. -->\n\
         \x20           <meta-data android:name=\"cplus.facet.main\" android:value=\"{sym}_main\" />\n\
         \x20           <intent-filter>\n\
         \x20               <action android:name=\"android.intent.action.MAIN\" />\n\
         \x20               <category android:name=\"android.intent.category.LAUNCHER\" />\n\
         \x20           </intent-filter>\n\
         \x20       </activity>\n\
         \x20   </application>\n\
         </manifest>\n"
    );

    let entry_body = |p: &str| -> String {
        if matches!(p, "ios" | "android") {
            external_main(p)
        } else {
            desktop_main.to_string()
        }
    };

    let android = platforms.iter().any(|p| p == "android");
    // `android/out` is where the APK is assembled — the .so, a dex, an
    // intermediate unsigned .apk and a signed one. Ignored for the same reason
    // /target is; it is build output, whoever produced it.
    let gitignore = if gui && android {
        "/target\n/vendor\n/android/out\n"
    } else {
        "/target\n/vendor\n"
    };

    // ---- what the AGENT is handed ------------------------------------------
    //
    // A new language has no training corpus, so an agent writes C+ from
    // whatever it already knows and re-derives the language from build failures
    // every session. The answer is that THE COMPILER IS THE CORPUS — `cpc
    // skill` prints the language reference and every dependency's,
    // version-matched and offline; `cpc explain` turns a code into a cause and
    // a worked example; `cpc query` and `cpc mcp` answer "where is X / who
    // calls X / what is the type here" from the resolved graph rather than from
    // grep. All four existed and nothing pointed an agent at any of them.
    //
    // AGENTS.md, not CLAUDE.md: it is the cross-agent filename, and Codex reads
    // it too. Claude Code auto-loads it; SKILL.md it does not.
    //
    // POINT AT THE COMMAND, NOT THE FILE. A checked-in SKILL.md drifts from the
    // compiler that scaffolded it — measured at 21KB of missing language
    // surface on one repo, including a builtin. `cpc skill` cannot drift,
    // because it IS the compiler answering.
    //
    // This is cpc's section because WHICH SUBCOMMANDS EXIST is a fact about the
    // binary, and a pointer file naming one the toolchain dropped is worse than
    // no pointer file. An IDE appends its own section below; see the marker.
    let agents_md = format!(
        "# {proj_name}\n\n\
         This is a C+ project. C+ is a young language, so **do not write it from\n\
         memory** — the toolchain answers every question about it, offline and\n\
         version-matched to this project.\n\n\
         ## Before you write any C+\n\n\
         Run `cpc skill`. It prints the language reference, and inside a project\n\
         it also prints the reference of every dependency that ships one — facet\n\
         contributes several hundred lines about its retained, non-reactive model\n\
         and the mistakes that compile anyway. Read it rather than a checked-in\n\
         copy: a file drifts from the compiler, this cannot.\n\n\
         `cpc skill --lang-only` is the language alone, if that is all you need.\n\n\
         ## When the compiler says no\n\n\
         Run `cpc explain <CODE>` before you guess. Every diagnostic code has a\n\
         cause, a fix and a worked example behind it — `cpc explain E0613` is\n\
         faster and more reliable than inferring from the message.\n\n\
         ## Navigating this code\n\n\
         **Do not grep for definitions.** C+ has no dynamic dispatch, so every\n\
         call to a named function resolves and the graph's answer is COMPLETE —\n\
         which grep's never is:\n\n\
         ```\n\
         cpc query definition <symbol>     where is it\n\
         cpc query references <symbol>     everywhere it is used\n\
         cpc query callers <symbol>        who calls it\n\
         cpc query symbols <file>          the outline of a file\n\
         cpc query scope-at <file:line:col> what you can type right there\n\
         cpc query complete <file:line:col> ...and what fits after a `.` or `::`\n\
         ```\n\n\
         The same graph is available as MCP tools — see `.mcp.json`, which points\n\
         at `cpc mcp`. Prefer either over reading files to find things. Each\n\
         `cpc query` rebuilds the whole graph (~seconds on a large project) and\n\
         throws it away; the MCP server builds once and answers in microseconds,\n\
         so use it for anything more than a single lookup.\n\n\
         ## Building\n\n\
         ```\n\
         cpc build          compile and link\n\
         cpc test           run the tests\n\
         cpc fmt            canonical formatting\n\
         ```\n\n\
         ## Driving the running app\n\n\
         This app is an ACI: while it runs it serves MCP, and you can read its\n\
         UI and act on it. `src/app.cplus` is where that is turned on.\n\n\
         **Find it.** The address is derived from the app id and its pid, so a\n\
         running instance writes `/tmp/mcp-{proj_name}-<pid>.json` saying where\n\
         it landed:\n\n\
         ```\n\
         cat /tmp/mcp-{proj_name}-*.json\n\
         ```\n\n\
         A pid whose process is gone is a leftover — check with `kill -0 <pid>`.\n\
         If you launched the app yourself you already know the pid, so you can\n\
         skip the file: the port is `9000 + pid % 1000`.\n\n\
         **Talk to it.** Plain JSON-RPC over POST, no bridge:\n\n\
         ```\n\
         curl -s -X POST http://127.0.0.1:<port>/ \\\n\
         \x20    -d '{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"describe_ui\"}}'\n\
         ```\n\n\
         `tools/list` names every verb, and the startup line says how many\n\
         there are. The core eleven are `describe_ui`, `click`, `set_text`,\n\
         `hit_test`, `set_caret`, `read_text`, `read_runs`, `invoke_menu`,\n\
         `scroll_to`, `poll_event` and `activity`. Fourteen more see the whole\n\
         tree rather than just what is exposed, and can write any property on\n\
         it: `describe_tree`, `inspect`, `set`, `set_many`, `reset`, `nudge`,\n\
         `insert`, `remove`, `reparent`, `undo`, `highlight`,\n\
         `clear_highlight`, `vocabulary` and `journal`. They come with the\n\
         surface — there is nothing extra for the app to call. Ask\n\
         `vocabulary` what a property takes before you `set` one.\n\n\
         `activity` is the record of what has been DONE through the surface —\n\
         useful when a person is supervising you, and when you want to check\n\
         what you already tried.\n\n\
         **This is how you test a UI change.** `describe_ui` answers a flat node\n\
         list and each `id` is the `key:` written in the code. Click, describe\n\
         again, and read the change — that is evidence, in a way \"it should work\n\
         now\" is not.\n\n\
         Two things worth knowing before you are confused by them:\n\n\
         - **You have no hands.** There is no drag, pinch or swipe verb and\n\
         \x20 there will not be one. An affordance only a gesture can reach is a\n\
         \x20 bug in the app — it is unreachable for anyone driving by voice too.\n\
         \x20 Fix the click path; do not look for a gesture verb.\n\
         - **You may be refused once.** If the app wired `agent_consent`, your\n\
         \x20 first request is refused while a dialog asks the user. The error\n\
         \x20 says whether to retry — `consent pending` means come back,\n\
         \x20 `consent denied` means the user said no.\n\n\
         <!-- Sections below this line are written by your IDE and are rewritten\n\
              when it opens the project. Edit above the line, not below it. -->\n"
    );

    // The code graph as MCP, for an agent the user runs in their own terminal
    // with no IDE in the picture. cpc's rather than an IDE's for that reason:
    // making it the IDE's would withhold a C+ feature from C+ users.
    //
    // THE ABSOLUTE PATH, not a bare `cpc`. An MCP client is launched by whatever
    // spawned the agent, and a GUI-launched one inherits an environment where a
    // bare name does not resolve — the same trap that made an IDE's whole code
    // intelligence layer silently answer nothing.
    let cpc_path = env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| "cpc".to_string());
    let mcp_json = format!(
        "{{\n  \"mcpServers\": {{\n    \"cplus\": {{\n      \"command\": \"{cpc_path}\",\n      \"args\": [\"mcp\"]\n    }}\n  }}\n}}\n"
    );


    // Consent, as a file the developer OWNS rather than a paragraph in a
    // comment. Generated unwired: the surface admits by default so an agent can
    // drive a fresh project immediately, and turning this on is one call. The
    // file itself carries no explanation — `facet_agent/consent` is where the
    // reasoning lives, and a generated file that lectures is a generated file
    // people delete.
    let facet_consent = format!(
        "// Ask before an agent may drive this app.\n\
         //\n\
         // Wire it in src/app.cplus, before `agent::enable()`:\n\
         //\n\
         //     agent_consent::install();\n\n\
         import \"facet_runtime/runtime\" as runtime;\n\
         import \"facet_agent/agent\" as agent;\n\
         import \"facet_agent/consent\" as consent;\n\
         import \"facet/services\" as services;\n\
         import \"stdlib/text\" as text;\n\n\
         fn answered(index: i32, ctx: *u8) {{\n\
         \x20   if index == 0 {{ consent::allow_pending(); return; }}\n\
         \x20   consent::deny_pending();\n\
         }}\n\n\
         fn show(ctx: *u8) {{\n\
         \x20   let message: text::Text = \"${{consent::pending()}} wants to read this app and press its buttons.\";\n\
         \x20   runtime::alert(\"Allow agent access?\", message.view(), \"Allow\",\n\
         \x20                  secondary: \"Deny\", on_answer: answered);\n\
         }}\n\n\
         fn ask(client: str, ctx: *u8) {{\n\
         \x20   // `ask` runs on the serve thread. A dialog built there is an\n\
         \x20   // NSWindow off the main thread, which aborts the process.\n\
         \x20   if !services::has_main_hop() {{ consent::cancel_pending(); return; }}\n\
         \x20   services::run_on_main(show, 0 as *u8);\n\
         }}\n\n\
         fn install() {{\n\
         \x20   consent::on_ask(ask);\n\
         \x20   agent::set_policy(consent::gate);\n\
         }}\n"
    );

    let mut files: Vec<(PathBuf, String)> = vec![(manifest, manifest_toml)];
    if gui {
        // The shared app, then one door per platform.
        files.push((src.join("app.cplus"), facet_app));
        files.push((src.join("agent_consent.cplus"), facet_consent.clone()));
        for p in platforms.iter() {
            // EVERY EXTERNAL-BUILDER PLATFORM, not just iOS. Testing `p ==
            // "ios"` here is what handed android the desktop `fn main`, which
            // its own build rejects (E0409) — so naming --platform ios flipped
            // an android entry that was correct on its own from right to wrong.
            let body = match p.as_str() {
                "ios" => facet_ios_entry.clone(),
                "android" => facet_android_entry.clone(),
                _ => facet_desktop_entry(p),
            };
            files.push((src.join(entry_file(p)), body));
        }
        if android {
            // The Gradle-free Android side: the manifest `aapt2` requires, and
            // the pipeline that turns the archive cpc builds into an APK. Two
            // files, one template, because the .so name in one has to match the
            // `cplus.facet.lib` meta-data in the other.
            let android_dir = root.join("android");
            if let Err(e) = std::fs::create_dir_all(&android_dir) {
                eprintln!("cpc init: could not create {}: {e}", android_dir.display());
                return ExitCode::FAILURE;
            }
            files.push((android_dir.join("AndroidManifest.xml"), android_manifest.clone()));
        }
        if ios {
            // The Xcode side. `main.m` is the whole of the Objective-C in a
            // facet app; facet_uikit synthesizes its own delegate at runtime,
            // and the plist below names the scene it synthesizes with it.
            let ios_dir = root.join("ios");
            if let Err(e) = std::fs::create_dir_all(&ios_dir) {
                eprintln!("cpc init: could not create {}: {e}", ios_dir.display());
                return ExitCode::FAILURE;
            }
            files.push((ios_dir.join("main.m"), ios_main_m.clone()));
            files.push((ios_dir.join("Info.plist"), ios_plist.clone()));
        }
    } else if platforms.is_empty() {
        files.push((src.join("main.cplus"), desktop_main.to_string()));
    } else {
        for p in platforms.iter() {
            files.push((src.join(entry_file(p)), entry_body(p)));
        }
    }
    files.push((root.join(".gitignore"), gitignore.to_string()));
    // The agent reference, so the fresh project is immediately LLM-ready.
    files.push((root.join("SKILL.md"), SKILL_MD.to_string()));
    files.push((root.join("AGENTS.md"), agents_md));
    files.push((root.join(".mcp.json"), mcp_json));
    for (path, content) in files {
        if let Err(e) = std::fs::write(&path, content) {
            eprintln!("cpc init: could not write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }

    // `cpc init`, `cpc init .`, and `cpc init ./` all scaffold the current
    // directory in place (no `cd` to suggest); a name/path scaffolds into it.
    let in_place = matches!(name.as_deref(), None | Some(".") | Some("./"));
    if in_place {
        println!("created C+ project `{proj_name}` in the current directory");
    } else {
        println!("created C+ project `{proj_name}` in {}/", name.as_deref().unwrap());
    }
    println!("next:");
    if !in_place {
        println!("  cd {}", name.as_deref().unwrap());
    }
    println!("  cpc pm install       # fetch dependencies into the store");
    if ios {
        println!("  cpc build --target ios-arm64-simulator   # -> lib{proj_name}.a + {proj_name}.h");
        println!();
        println!("ios/ holds the Xcode side: main.m (calls {sym}_main) and Info.plist.");
        println!("Point an Xcode app target at them, link the .a, and add the header's");
        println!("directory to the header search path — examples/DEPLOYING.md has the recipe.");
        println!("The app is src/app.cplus; `cpc skill` prints facet's reference with it.");
    }
    if android {
        if gui {
            println!("  cpc build --target android-arm64         # -> lib{proj_name}.a + {proj_name}.h");
            println!();
            println!("android/AndroidManifest.xml is the Android side: it names facet's own");
            println!("Activity (which ships inside facet_android as a dex, so this app writes");
            println!("no Java), the .so to load, and {sym}_main as `cplus.facet.main`.");
            println!("Linking that archive into an .so and packaging it into an APK is the");
            println!("platform builder's half, the same way ios/ is Xcode's — iris does it from");
            println!("Run, and examples/facet_gallery_android/build.sh is the recipe by hand.");
        } else {
            println!("  cpc build --target android-arm64         # -> lib{proj_name}.a + {proj_name}.h");
        }
    }
    if platforms.is_empty()
        || platforms
            .iter()
            .any(|p| !matches!(p.as_str(), "ios" | "android"))
    {
        println!("  cpc build            # compile and link the desktop entry");
    }
    println!();
    println!(
        "note: Cplus.toml pins stdlib to this toolchain (v{}); `cpc pm install`",
        env!("CARGO_PKG_VERSION")
    );
    println!("      clones it from git into vendor/stdlib.");
    ExitCode::SUCCESS
}
