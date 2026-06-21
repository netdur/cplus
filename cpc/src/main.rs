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
  cpc FILE [-o OUT]                 compile single-file FILE.cplus to a binary (default OUT: ./a.out)
  cpc build [-o OUT]                multi-file build: reads ./Cplus.toml, walks imports
  cpc check [FILE]                  parse + sema + borrowck, no codegen (fast feedback loop).
                                    With no FILE: whole-project check via Cplus.toml,
                                    enforcing any [profile.realtime] gate.
  cpc doc FILE                      extract public items + `///` docs from FILE, emit
                                    Markdown to ./target/doc/<basename>.md
  cpc test [FILE] [--json]          discover + run `#[test]` functions. Single-file mode
                                    if FILE is given; project mode (reads ./Cplus.toml)
                                    otherwise. `--json` emits one JSON object per test
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
  --release                         -O2, no overflow checks on `+ - *` (default: debug, checked)
  --debug                           -O0 with overflow traps (the default)
  --fp-contract=off|on|fast         float contraction policy; `off` keeps `a*b+c` as
                                    fmul+fadd for bit-identical-to-C output (default: on).
                                    Place before --emit-ll/--emit-asm/--emit-obj FILE.
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
                                    --emit-ll / --emit-asm, or `cpc build` of a `[lib]`
                                    staticlib. android-arm64 uses the NDK's clang
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
`value-refs FILE:LINE:COL`.
Reads ./Cplus.toml; exit code signals found / not-found.
"
        }
        Some(Subcommand::Mcp) => {
            "\
cpc mcp

Resident MCP server over the code knowledge graph: builds the graph once
from ./Cplus.toml, then answers MCP tool calls over stdio (newline-
delimited JSON-RPC 2.0) until stdin closes. Point an MCP client at
`cpc mcp` to give an agent resolved, typed C+ navigation in place of grep.
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

fn emit_diag(d: &Diagnostic, mode: DiagMode, src: &str) {
    let line = match mode {
        DiagMode::Human => d.render_human(src),
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
    // the handoff points; `build` enforces its own [lib]-vs-[[bin]] rule.
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
                    "    use --emit-obj/--emit-ll/--emit-asm, or `cpc build` with a `[lib]` staticlib"
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
        (Some(Subcommand::Fmt), _) => run_fmt(fmt_inputs, fmt_opts, diag_mode),
        (Some(Subcommand::Test), _) => run_test(test_input, test_opts, diag_mode, build_mode),
        (Some(Subcommand::Lsp), _) => run_lsp(lsp_args),
        (Some(Subcommand::Check), Some(path)) => run_check(path, diag_mode),
        (Some(Subcommand::Check), None) => run_check_project(diag_mode),
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
    EmitLlProject,
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
/// package's `src/lib/<triple>/`. Each build calls this once.
fn detect_host_triple() -> Result<String, ExitCode> {
    let output = match Command::new(clang_program())
        .arg("-print-target-triple")
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            eprintln!("cpc: invoking `clang -print-target-triple`: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    if !output.status.success() {
        eprintln!(
            "cpc: `clang -print-target-triple` exited with {:?}",
            output.status.code()
        );
        return Err(ExitCode::FAILURE);
    }
    let triple = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if triple.is_empty() {
        eprintln!("cpc: `clang -print-target-triple` produced no output");
        return Err(ExitCode::FAILURE);
    }
    Ok(triple)
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
/// own `[[bin]].frameworks`/`libs` (or `[lib]`'s equivalents) so the order
/// is: consumer-first, then each dep in declared order.
///
/// Per-dep validation:
///   - `vendor/<name>/Cplus.toml` exists (E0854) and parses cleanly.
///   - Vendor manifest's `[package].name == <name>` (E0855).
///   - For each name in `[link].bundled`:
///       host triple is in `[link].triples` (E0862),
///       `vendor/<name>/src/lib/<host-triple>/<basename>` exists (E0860).
///   - No `.a`/`.dylib`/`.so` files under any
///     `vendor/<name>/src/lib/<triple>/` that aren't in `[link].bundled`
///     (E0861). Applies even when a package declares no `[link]` table —
///     orphan binaries are a manifest bug, never a graceful-degradation
///     case.
///
/// On any failure: a structured diagnostic is emitted via `emit_diag` and
/// `Err(ExitCode::FAILURE)` is returned before codegen / linking can run.
fn collect_dep_link_args(
    m: &manifest::Manifest,
    diag_mode: DiagMode,
) -> Result<Vec<String>, ExitCode> {
    if m.dependencies.is_empty() {
        return Ok(Vec::new());
    }
    // v0.0.21 multi-backend slice 1: bundled artifacts resolve by the
    // *selected* target's stable artifact triple; only the host target
    // still asks `clang -print-target-triple`. `triple_word` keeps the
    // E0862 message precise ("host triple" vs "target triple").
    let tgt = target::active_target();
    let (link_triple, triple_word) = match tgt.artifact_triple {
        Some(t) => (t.to_string(), "target"),
        None => (detect_host_triple()?, "host"),
    };
    let mut link_args: Vec<String> = Vec::new();
    for dep in &m.dependencies {
        let mut vendor_dir = m.root.join("vendor").join(&dep.name);
        let mut vendor_manifest = vendor_dir.join("Cplus.toml");
        // Vendor-package self-test fallback: when run from inside a
        // vendor package, sibling vendor packages live at
        // `<m.root>/../<dep>/` rather than under `<m.root>/vendor/`.
        // See resolver.rs's matching fallback in `resolve_vendor_path`.
        if !vendor_manifest.is_file() {
            if let Some(parent) = m.root.parent() {
                let alt_dir = parent.join(&dep.name);
                let alt_manifest = alt_dir.join("Cplus.toml");
                if alt_manifest.is_file() {
                    vendor_dir = alt_dir;
                    vendor_manifest = alt_manifest;
                }
            }
        }
        if !vendor_manifest.is_file() {
            let d = manifest_diag(
                "E0854",
                &vendor_manifest,
                format!(
                    "vendor package `{}` is missing `Cplus.toml` (expected at `{}`)",
                    dep.name,
                    vendor_manifest.display()
                ),
                vec![format!(
                    "declared in `[dependencies]` of {}",
                    m.root.join("Cplus.toml").display()
                )],
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
        let lib_root = vendor_dir.join("src").join("lib");
        let bundled: &[String] = vm
            .link
            .as_ref()
            .map(|l| l.bundled.as_slice())
            .unwrap_or(&[]);
        let triples: &[String] = vm
            .link
            .as_ref()
            .map(|l| l.triples.as_slice())
            .unwrap_or(&[]);
        if !bundled.is_empty() {
            if !triples.iter().any(|t| t == &link_triple) {
                let supported = if triples.is_empty() {
                    "<none>".to_string()
                } else {
                    triples
                        .iter()
                        .map(|s| format!("`{s}`"))
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                let d = manifest_diag(
                    "E0862",
                    &vendor_manifest,
                    format!(
                        "package `{}` does not ship a build for {} triple `{}` (supports: {})",
                        dep.name, triple_word, link_triple, supported
                    ),
                    vec![
                        format!("add `{link_triple}` to `[link].triples` and ship the matching binaries, or build the package from source for this triple"),
                    ],
                );
                emit_diag(&d, diag_mode, "");
                return Err(ExitCode::FAILURE);
            }
            let triple_lib_dir = lib_root.join(&link_triple);
            for basename in bundled {
                let p = triple_lib_dir.join(basename);
                if !p.is_file() {
                    let d = manifest_diag(
                        "E0860",
                        &vendor_manifest,
                        format!(
                            "package `{}` declares bundled `{}` but `src/lib/{}/{}` is not present (the package manifest says you ship it for this triple, but the file is missing)",
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
        // Orphan-file check: every binary under `src/lib/<triple>/` (any
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
                        if !bundled.iter().any(|b| b == &fname) {
                            let triple_name = triple_dir
                                .file_name()
                                .and_then(|s| s.to_str())
                                .unwrap_or("?");
                            let d = manifest_diag(
                                "E0861",
                                &vendor_manifest,
                                format!(
                                    "package `{}` ships `src/lib/{}/{}` but the manifest doesn't declare it; the manifest is the single source of truth",
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
        // Bundled artifacts go in as full paths (not `-l<name>` — they're
        // not on the linker's search path).
        if let Some(ls) = &vm.link {
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
            for basename in &ls.bundled {
                let p = lib_root.join(&link_triple).join(basename);
                link_args.push(p.to_string_lossy().to_string());
            }
            // v0.0.9 Phase 8 (cpc-gaps G-001): vendor packages may also
            // declare `extra-objects` (rare — usually consumer-side).
            // Validate existence here so the diag carries the dep name.
            for obj in &ls.extra_objects {
                if !obj.is_file() {
                    return Err(emit_extra_object_missing(diag_mode, obj, &vendor_manifest));
                }
                link_args.push(obj.to_string_lossy().to_string());
            }
        }
    }
    Ok(link_args)
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
    // Phase 5 Slice 5.A: a `[lib]` manifest dispatches to the library
    // build path (object → archive / shared-library) instead of the
    // executable path. Mutual exclusion with `[[bin]]` is enforced at
    // manifest-parse time (E0408), so reaching here with `lib` set
    // means no `[[bin]]` declared.
    if let Some(lib) = m.lib.clone() {
        return build_lib_project(&m, &lib, out, diag_mode, build_mode, fp_contract);
    }
    // v0.0.21 multi-backend slice 1: an external-builder target has no app
    // link inside cpc — Xcode (or the platform's build system) owns it. A
    // `[[bin]]` build would end at exactly that link, so reject it with the
    // supported flow instead of failing inside clang.
    let tgt = target::active_target();
    if tgt.handoff == Handoff::ExternalBuilder {
        eprintln!(
            "cpc: target `{}` stops at object emission (the external builder owns the final link); `[[bin]]` projects can't be built for it",
            tgt.name
        );
        eprintln!(
            "    declare a `[lib]` staticlib instead and link the archive from the external build system"
        );
        return ExitCode::FAILURE;
    }
    if m.bins.len() != 1 {
        eprintln!(
            "cpc: Phase 4 slice 4A supports exactly one [[bin]]; found {}",
            m.bins.len()
        );
        return ExitCode::FAILURE;
    }
    let bin = &m.bins[0];
    if !bin.path.is_file() {
        // Build E0407 directly here — same structured shape so json/short/human
        // all work uniformly.
        let d = diag::Diagnostic {
            severity: Severity::Error,
            code: diag::DiagCode("E0407"),
            message: format!("binary entry `{}` does not exist", bin.path.display()),
            primary: diag::SourceSpan {
                file: bin.path.clone(),
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
    // vendor/<dep>/src/. The consumer's bin path is the entry.
    let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
    let (program, _entry_file_id, mono) = match load_and_check_project_full(
        &bin.path,
        &m.root,
        diag_mode,
        false,
        Some(&dep_names),
        m.realtime_profile.as_ref(),
    ) {
        Ok(p) => p,
        Err(code) => return code,
    };
    // v0.0.3 Phase 5 Slice 5D follow-up: forward --asan/--tsan/--ubsan/
    // --msan through codegen options + clang. Previously `cpc build`
    // silently dropped these flags (always emitted unsanitised IR and
    // linked without `-fsanitize=...`), which meant every e2e ASan
    // test was vacuously clean. The single-file path (`compile_file`)
    // already plumbed sanitizers; this matches.
    ensure_coro_end_probed();
    let ir = codegen::generate_with_mono(
        &program,
        build_mode,
        fp_contract,
        None,
        sanitizers,
        false,
        &mono,
    );

    let out_path = out.unwrap_or_else(|| {
        let sub = match build_mode {
            BuildMode::Debug => "debug",
            BuildMode::Release => "release",
        };
        m.root.join("target").join(sub).join(&bin.name)
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
    // v0.0.2 (AppKit-via-Cplus.toml): expand the manifest's `frameworks`
    // and `libs` lists into `-framework <name>` / `-l<name>` linker args.
    // `frameworks` is macOS/iOS-specific (no-op elsewhere because clang's
    // `-framework` flag is platform-gated), `-l` is cross-platform.
    let mut link_args: Vec<String> = Vec::with_capacity(bin.frameworks.len() * 2 + bin.libs.len());
    // The consumer's own `[link] search-paths` go first so `-L<dir>`
    // precedes any `-l<name>` (its own `[[bin]] libs` below, or a dep's).
    if let Some(ls) = m.link.as_ref() {
        // v0.0.20 (W0003): a `[[bin]]` package's own `[link] libs`/`frameworks`
        // are dead — the dep walk (`collect_dep_link_args`) reads a package's
        // `[link]` libs/frameworks only when it is a *dependency* of another,
        // and a `[[bin]]` package is never a dependency. Only `[link]
        // search-paths` feed the binary's own link line (the `-L`/`-rpath`
        // below). Warn rather than silently ignore; the build continues.
        if !ls.libs.is_empty() || !ls.frameworks.is_empty() {
            let mut what: Vec<&str> = Vec::new();
            if !ls.libs.is_empty() {
                what.push("libs");
            }
            if !ls.frameworks.is_empty() {
                what.push("frameworks");
            }
            let what = what.join(" / ");
            let d = diag::Diagnostic {
                severity: Severity::Warning,
                code: diag::DiagCode("W0003"),
                message: format!(
                    "`[link] {what}` on a `[[bin]]` package is ignored when building the binary"
                ),
                primary: diag::SourceSpan {
                    file: manifest_path.clone(),
                    start: diag::Position { line: 1, col: 1, byte: 0 },
                    end: diag::Position { line: 1, col: 1, byte: 0 },
                },
                labels: Vec::new(),
                notes: vec![
                    "`[link] libs`/`frameworks` are read only when this package is a dependency of another package".to_string(),
                    format!("move them to `[[bin]] {what}` to link them into this binary"),
                ],
                suggestions: Vec::new(),
            };
            emit_diag(&d, diag_mode, "");
        }
        for dir in &ls.search_paths {
            link_args.push(format!("-L{dir}"));
            link_args.push(format!("-Wl,-rpath,{dir}"));
        }
    }
    for fw in &bin.frameworks {
        link_args.push("-framework".to_string());
        link_args.push(fw.clone());
    }
    for lib in &bin.libs {
        link_args.push(format!("-l{lib}"));
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
    let status = run_clang(&tmp, &out_path, build_mode, false, sanitizers, &link_args);
    drop(tmp_handle); // explicit cleanup on the secure temp path
    status
}

/// Phase 5 Slice 5.A: library-build path. Produces `lib<name>.a` and/or
/// `lib<name>.{dylib,so}` in `target/<mode>/`. Mutually exclusive with
/// the executable build via the manifest's `[[bin]]` vs `[lib]` choice.
///
/// Pipeline (mirrors the bin path's structure):
///   1. Load + sema-check the lib root source (via `load_and_check_project`).
///   2. Reject `fn main` if defined (E0409) — libraries don't have entry points.
///   3. Emit IR; write IR to temp `.ll`; run `clang -c` → `target/<mode>/<name>.o`.
///   4. For `staticlib` / `both`: `ar rcs target/<mode>/lib<name>.a <name>.o`.
///   5. For `cdylib`   / `both`: `clang -shared <opts> -o target/<mode>/lib<name>.<ext> <name>.o`.
///   6. Manifest `frameworks` / `libs` are forwarded only at the cdylib link
///      step — they don't get into the static archive (consumers re-state them).
fn build_lib_project(
    m: &manifest::Manifest,
    lib: &manifest::LibTarget,
    out_override: Option<PathBuf>,
    diag_mode: DiagMode,
    build_mode: BuildMode,
    fp_contract: bool,
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
            "cpc: target `{}` stops at object emission (the external builder owns the final link); `crate-type = \"cdylib\"` would require one",
            tgt.name
        );
        eprintln!("    use `crate-type = \"staticlib\"` and link the archive from the external build system");
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
    let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
    let (program, _entry_file_id, mono) = match load_and_check_project_full(
        &lib.path,
        &m.root,
        diag_mode,
        true,
        Some(&dep_names),
        m.realtime_profile.as_ref(),
    ) {
        Ok(p) => p,
        Err(code) => return code,
    };

    // Phase 2 Slice 2C: dep walk runs even for library targets — a `.dylib`
    // baked from a package that itself depends on something must record
    // those link args. Static archives can't carry link metadata, but we
    // still validate the dep graph here so any contract violation surfaces
    // at lib build time rather than ambushing the consumer later.
    let dep_link_args: Vec<String> = match collect_dep_link_args(m, diag_mode) {
        Ok(v) => v,
        Err(code) => return code,
    };

    // Phase 5 Slice 5.A.4: reject `fn main` in a library target. A
    // library has no entry point; declaring one means the user probably
    // meant `[[bin]]` instead. E0409 — sema-level gate enforced here at
    // build-time because sema itself doesn't know about manifest mode.
    for item in &program.items {
        if let cplus_core::ast::ItemKind::Function(f) = &item.kind {
            if f.name.name == "main" && !f.is_extern {
                let d = diag::Diagnostic {
                    severity: Severity::Error,
                    code: diag::DiagCode("E0409"),
                    message: "library targets must not define `fn main`".to_string(),
                    primary: diag::SourceSpan {
                        file: lib.path.clone(),
                        start: diag::Position { line: 1, col: 1, byte: 0 },
                        end: diag::Position { line: 1, col: 1, byte: 0 },
                    },
                    labels: Vec::new(),
                    notes: vec![
                        "this manifest declares `[lib]`; a `fn main` would conflict with the consumer's entry point".to_string(),
                        "if you meant to build an executable, use `[[bin]]` instead of `[lib]`".to_string(),
                    ],
                    suggestions: Vec::new(),
                };
                emit_diag(&d, diag_mode, "");
                return ExitCode::FAILURE;
            }
        }
    }

    ensure_coro_end_probed();
    let ir = codegen::generate_with_mono(&program, build_mode, fp_contract, None, &[], true, &mono);

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
    let obj_status = Command::new(&clang_prog)
        .arg(opt)
        .arg("-Wno-override-module")
        .args(clang_target_args(&tgt))
        .arg("-c")
        .arg(&tmp_ll)
        .arg("-o")
        .arg(&obj_path)
        .status();
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
        // for `[lib] crate-type = "staticlib"` are silently dropped —
        // the consumer's `[[bin]]` is where they'd be respected anyway.
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
fn emit_ll_project(diag_mode: DiagMode, build_mode: BuildMode, fp_contract: bool) -> ExitCode {
    let manifest_path = PathBuf::from("Cplus.toml");
    let m = match manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            emit_diag(&e.to_diagnostic(), diag_mode, "");
            return ExitCode::FAILURE;
        }
    };
    if m.bins.len() != 1 {
        eprintln!("cpc: Phase 4 slice 4A supports exactly one [[bin]]");
        return ExitCode::FAILURE;
    }
    let bin = &m.bins[0];
    // Phase 2 Slice 2C: surface dep walk errors before codegen — the same
    // E0854/E0855/E0860-E0862 checks fire on `--emit-ll-project`, even
    // though no link step runs here. Catches manifest-is-truth violations
    // in CI loops that exercise this flag.
    if let Err(code) = collect_dep_link_args(&m, diag_mode) {
        return code;
    }
    let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
    let (program, _, mono) = match load_and_check_project_full(
        &bin.path,
        &m.root,
        diag_mode,
        false,
        Some(&dep_names),
        m.realtime_profile.as_ref(),
    ) {
        Ok(p) => p,
        Err(code) => return code,
    };
    ensure_coro_end_probed();
    let ir =
        codegen::generate_with_mono(&program, build_mode, fp_contract, None, &[], false, &mono);
    print!("{ir}");
    ExitCode::SUCCESS
}

/// Run the resolver + sema for a project. Sema diagnostics are emitted to
/// stderr; on error we return a failure ExitCode. On success returns the
/// merged Program and the entry file id.
fn load_and_check_project(
    entry: &Path,
    root: &Path,
    diag_mode: DiagMode,
) -> Result<(cplus_core::ast::Program, String, sema::MonoInfo), ExitCode> {
    // Legacy single-file path: no manifest, no dep list. `None` keeps
    // pre-Slice-2B file-relative resolution semantics.
    load_and_check_project_full(entry, root, diag_mode, false, None, None)
}

/// Phase 5 Slice 5.A: variant that passes `is_lib` to the resolver so
/// library-entry items skip name qualification (exposed as bare C-callable
/// symbols).
fn load_and_check_project_with_mode(
    entry: &Path,
    root: &Path,
    diag_mode: DiagMode,
    is_lib: bool,
) -> Result<(cplus_core::ast::Program, String, sema::MonoInfo), ExitCode> {
    load_and_check_project_full(entry, root, diag_mode, is_lib, None, None)
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
) -> Result<(cplus_core::ast::Program, String, sema::MonoInfo), ExitCode> {
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
        emit_diag(d, diag_mode, &entry_src);
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
    // lower the per-file source map (like attrs / sema) so an E0X30 / E0X36 in
    // an imported file renders against that file, not the entry file.
    let lower_diags = lower::lower_multi(
        &mut loaded.program,
        &entry.to_path_buf(),
        &entry_src,
        loaded.files.clone(),
    );
    let lower_errors = lower_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &lower_diags {
        emit_diag(d, diag_mode, &entry_src);
    }
    if lower_errors {
        return Err(ExitCode::FAILURE);
    }
    // Slice 4C: hand sema the per-file source map so cross-file
    // diagnostics render against the right file's line/column. Sema
    // routes via each item's `origin_file`.
    let (diags, mono) = sema::check_multi_with_mono(
        &loaded.program,
        entry.to_path_buf(),
        &entry_src,
        loaded.files.clone(),
    );
    let had_errors = diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &diags {
        emit_diag(d, diag_mode, &entry_src);
    }
    if had_errors {
        return Err(ExitCode::FAILURE);
    }
    // Phase 5 borrow checker (slice 5BC.2a — active diagnostics E0370).
    // Runs after sema so it inherits type-correctness assumptions.
    let bc_diags = borrowck::check(&loaded.program, &entry.to_path_buf(), &entry_src);
    let bc_errors = bc_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &bc_diags {
        emit_diag(d, diag_mode, &entry_src);
    }
    if bc_errors {
        return Err(ExitCode::FAILURE);
    }
    // Slice 7GEN.5a: monomorphization. Generic-fn templates are
    // replaced by per-instantiation concrete fns; generic call sites
    // are rewritten to mangled names. The result is a Program with no
    // generic items — codegen can consume it directly.
    let post_mono = run_monomorphize(loaded.program, &mono, &loaded.files);
    Ok((post_mono, loaded.entry_file_id, mono))
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
///                   No file arguments allowed.
///   - `--emit`:     read each file argument, write formatted to stdout.
///                   Multiple files are concatenated in order.
///   - `--check`:    read each file argument, exit 1 if formatting would
///                   change anything, 0 otherwise. Prints a unified diff
///                   per changed file to stderr.
///   - default:      rewrite each file argument in place. A directory
///                   argument recurses for `*.cplus` files.
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
) -> ExitCode {
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
            // Resolve the entry: prefer [lib] (explicit library target),
            // then [[bin]]. If neither exists on disk, fall back to
            // `src/<package-name>.cplus` — library-only vendor packages
            // commonly declare no target at all, and the manifest auto-
            // injects a phantom `[[bin]]` pointing at `src/main.cplus`
            // that doesn't exist. The fallback lets such packages still
            // discover and run their `#[test]` fns.
            let (entry_path, is_lib_pkg, fw_list, lib_list) = if let Some(lt) = m.lib.as_ref() {
                (
                    lt.path.clone(),
                    true,
                    lt.frameworks.clone(),
                    lt.libs.clone(),
                )
            } else if m.bins.len() == 1 && m.bins[0].path.is_file() {
                let b = &m.bins[0];
                (b.path.clone(), false, b.frameworks.clone(), b.libs.clone())
            } else if m.bins.len() == 1 {
                let guess = m.root.join("src").join(format!("{}.cplus", m.package.name));
                if !guess.is_file() {
                    eprintln!(
                        "cpc test: bin entry `{}` not found, and no `{}` fallback either",
                        m.bins[0].path.display(),
                        guess.display()
                    );
                    return ExitCode::FAILURE;
                }
                (guess, true, Vec::new(), Vec::new())
            } else {
                eprintln!(
                    "cpc test: project must declare at most one [[bin]]; found {}",
                    m.bins.len()
                );
                return ExitCode::FAILURE;
            };
            // Phase 2 Slice 2C: validate the dep graph before sema. Tests
            // share the consumer's `[dependencies]`, so a misdeclared
            // vendor package must fail here too — silent success would let
            // bad packages ride into a passing test run.
            if let Err(code) = collect_dep_link_args(&m, diag_mode) {
                return code;
            }
            let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
            let (program, _, mono) = match load_and_check_project_full(
                &entry_path,
                &m.root,
                diag_mode,
                is_lib_pkg,
                Some(&dep_names),
                m.realtime_profile.as_ref(),
            ) {
                Ok(p) => p,
                Err(code) => return code,
            };
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
            // pass above doesn't see it (those come from [[bin]]/[lib]
            // targets only). Splice in the package's own [link]
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
    let ir = codegen::generate_test_binary(&program, build_mode, &tests, opts.json, &mono);
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
    let clang_status = run_clang(&tmp, &bin_out, build_mode, false, &[], &link_args);
    drop(tmp_handle);
    if !matches!(clang_status, ExitCode::SUCCESS) {
        return clang_status;
    }
    // Run the test binary. Its stdout is what `cpc test` prints; its exit
    // code equals the number of failing tests (clamped into [0, 255] so the
    // process-exit-code-as-u8 convention still fits).
    let status = Command::new(&bin_out).status();
    drop(bin_path);
    match status {
        Ok(s) => {
            // The driver `main` returns the failure count. Map any non-zero
            // back to a clamped u8 ExitCode so callers can distinguish
            // "all passed" (0) from "something failed" (1..=255).
            let code = s.code().unwrap_or(1);
            if code == 0 {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(code.clamp(1, 255) as u8)
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
            let d = diag::from_lex(&e, &file.to_path_buf(), &lm, src);
            emit_diag(&d, mode, src);
            return Err(ExitCode::FAILURE);
        }
    };
    let mut prog = match parser::parse(toks) {
        Ok(p) => p,
        Err(e) => {
            let lm = LineMap::new(src);
            let d = diag::from_parse(&e, &file.to_path_buf(), &lm, src);
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
    let lower_diags = lower::lower(&mut prog, &file.to_path_buf(), src);
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
    let bc_diags = borrowck::check(&prog, &file.to_path_buf(), src);
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
    let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
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
/// [lib], then a single real [[bin]], then the `src/<package-name>.cplus`
/// fallback for library-only packages that declare no on-disk target. `ctx` is
/// the command label used in error messages.
fn resolve_project_entry(m: &manifest::Manifest, ctx: &str) -> Result<(PathBuf, bool), ExitCode> {
    if let Some(lt) = m.lib.as_ref() {
        Ok((lt.path.clone(), true))
    } else if m.bins.len() == 1 && m.bins[0].path.is_file() {
        Ok((m.bins[0].path.clone(), false))
    } else if m.bins.len() == 1 {
        let guess = m.root.join("src").join(format!("{}.cplus", m.package.name));
        if !guess.is_file() {
            eprintln!(
                "{ctx}: bin entry `{}` not found, and no `{}` fallback either",
                m.bins[0].path.display(),
                guess.display()
            );
            return Err(ExitCode::FAILURE);
        }
        Ok((guess, true))
    } else {
        eprintln!(
            "{ctx}: project must declare at most one [[bin]]; found {}",
            m.bins.len()
        );
        Err(ExitCode::FAILURE)
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
    let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
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
    let (diags, _mono) = sema::check_multi_with_mono(
        &loaded.program,
        entry_path.clone(),
        &entry_src,
        loaded.files.clone(),
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
    let manifest_path = PathBuf::from("Cplus.toml");
    let m = match manifest::load(&manifest_path) {
        Ok(m) => m,
        Err(e) => {
            emit_diag(&e.to_diagnostic(), diag_mode, "");
            return Err(ExitCode::FAILURE);
        }
    };
    let (entry_path, is_lib_pkg) = if let Some(lt) = m.lib.as_ref() {
        (lt.path.clone(), true)
    } else if m.bins.len() == 1 && m.bins[0].path.is_file() {
        (m.bins[0].path.clone(), false)
    } else if m.bins.len() == 1 {
        let guess = m.root.join("src").join(format!("{}.cplus", m.package.name));
        if !guess.is_file() {
            eprintln!(
                "cpc: bin entry `{}` not found, and no `{}` fallback either",
                m.bins[0].path.display(),
                guess.display()
            );
            return Err(ExitCode::FAILURE);
        }
        (guess, true)
    } else {
        eprintln!(
            "cpc: project must declare at most one [[bin]]; found {}",
            m.bins.len()
        );
        return Err(ExitCode::FAILURE);
    };
    let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
    match resolver::load_project_full(
        &entry_path,
        &m.root,
        is_lib_pkg,
        Some(&dep_names),
        Default::default(),
    ) {
        Ok(loaded) => Ok(loaded),
        Err(e) => {
            emit_diag(&e.to_diagnostic(), diag_mode, "");
            Err(ExitCode::FAILURE)
        }
    }
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
    let loaded = match load_project_for_graph(diag_mode) {
        Ok(l) => l,
        Err(code) => return code,
    };
    let g = cplus_core::graph::CodeGraph::build(&loaded);
    mcp::serve(&g, &loaded)
}

/// `cpc query <kind> [args...]` — answer one graph query as JSON on stdout.
/// Exit code signals found (0) vs not-found (1), per plan.graph.md §6. This
/// build ships the Phase 1 index: `def`, `members`, `symbols`. Call /
/// reference / type queries land in later phases and report so explicitly.
fn run_query(kind: Option<String>, args: Vec<String>, diag_mode: DiagMode) -> ExitCode {
    let Some(kind) = kind else {
        eprintln!("cpc query: expected a query kind (def | members | symbols)");
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
            let Some(pos) = arg0 else {
                eprintln!("cpc query type-at: expected FILE:LINE:COL");
                return ExitCode::FAILURE;
            };
            // FILE:LINE:COL — split COL and LINE off the right so the path may
            // contain no colons (the common case on unix).
            let parts: Vec<&str> = pos.rsplitn(3, ':').collect(); // [col, line, file]
            if parts.len() != 3 {
                eprintln!("cpc query type-at: expected FILE:LINE:COL (got `{pos}`)");
                return ExitCode::FAILURE;
            }
            let (Ok(col), Ok(line)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
                eprintln!("cpc query type-at: LINE and COL must be numbers");
                return ExitCode::FAILURE;
            };
            let file = parts[2];
            let Some((fid, (_, src))) = loaded
                .files
                .iter()
                .find(|(_, (path, _))| path.ends_with(file) || path.to_string_lossy() == *file)
            else {
                eprintln!("cpc query type-at: no source file matching `{file}`");
                return ExitCode::FAILURE;
            };
            let Some(byte) = cplus_core::graph::byte_offset(src, line, col) else {
                eprintln!("cpc query type-at: position {line}:{col} is out of range");
                return ExitCode::FAILURE;
            };
            return match g.type_at_json(fid, byte) {
                Some(j) => {
                    println!("{j}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!(
                        "cpc query type-at: no typed node at {file}:{line}:{col} \
                         (type-at resolves params, fields, locals, `self`, and inferred \
                         expressions — call results, field/index reads, match/if values)"
                    );
                    ExitCode::FAILURE
                }
            };
        }
        "value-refs" => {
            let Some(pos) = arg0 else {
                eprintln!("cpc query value-refs: expected FILE:LINE:COL");
                return ExitCode::FAILURE;
            };
            let parts: Vec<&str> = pos.rsplitn(3, ':').collect(); // [col, line, file]
            if parts.len() != 3 {
                eprintln!("cpc query value-refs: expected FILE:LINE:COL (got `{pos}`)");
                return ExitCode::FAILURE;
            }
            let (Ok(col), Ok(line)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>()) else {
                eprintln!("cpc query value-refs: LINE and COL must be numbers");
                return ExitCode::FAILURE;
            };
            let file = parts[2];
            let Some((fid, (_, src))) = loaded
                .files
                .iter()
                .find(|(_, (path, _))| path.ends_with(file) || path.to_string_lossy() == *file)
            else {
                eprintln!("cpc query value-refs: no source file matching `{file}`");
                return ExitCode::FAILURE;
            };
            let Some(byte) = cplus_core::graph::byte_offset(src, line, col) else {
                eprintln!("cpc query value-refs: position {line}:{col} is out of range");
                return ExitCode::FAILURE;
            };
            return match g.value_refs_json(fid, byte) {
                Some(j) => {
                    println!("{j}");
                    ExitCode::SUCCESS
                }
                None => {
                    eprintln!(
                        "cpc query value-refs: no local binding at {file}:{line}:{col} \
                         (value-refs resolves a parameter or `let`, then its classified uses)"
                    );
                    ExitCode::FAILURE
                }
            };
        }
        other => {
            eprintln!(
                "cpc query: unknown query kind `{other}` (expected: def | members | symbols | \
                 refs | callers | callees | call-hierarchy | context | type-at | value-refs)"
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
    let ir = match build_ir(
        &input,
        &src,
        mode,
        build_mode,
        fp_contract,
        debug_info,
        sanitizers,
    ) {
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
    let status = run_clang(&tmp, &out, build_mode, debug_info, sanitizers, &[]);
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
    let (mut prog, files_map) = if has_imports {
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
                    let deps: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
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
        (loaded.program, loaded.files)
    } else {
        let toks = match lexer::tokenize(src) {
            Ok(t) => t,
            Err(e) => {
                let lm = LineMap::new(src);
                let d = diag::from_lex(&e, &file.to_path_buf(), &lm, src);
                emit_diag(&d, mode, src);
                return Err(ExitCode::FAILURE);
            }
        };
        let prog = match parser::parse(toks) {
            Ok(p) => p,
            Err(e) => {
                let lm = LineMap::new(src);
                let d = diag::from_parse(&e, &file.to_path_buf(), &lm, src);
                emit_diag(&d, mode, src);
                return Err(ExitCode::FAILURE);
            }
        };
        (prog, std::collections::BTreeMap::new())
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
        emit_diag(d, mode, src);
    }
    if attr_errors {
        return Err(ExitCode::FAILURE);
    }
    // Lower `if let` / `guard let` to match-using forms before sema. GAP 3:
    // route the per-file source map through lower (like attrs / sema) so a
    // lower-pass error in an imported file renders against that file.
    let lower_diags = if files_map.is_empty() {
        lower::lower(&mut prog, &file.to_path_buf(), src)
    } else {
        lower::lower_multi(&mut prog, &file.to_path_buf(), src, files_map.clone())
    };
    let lower_errors = lower_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &lower_diags {
        emit_diag(d, mode, src);
    }
    if lower_errors {
        return Err(ExitCode::FAILURE);
    }
    let (diags, mono) =
        sema::check_multi_with_mono(&prog, file.to_path_buf(), src, files_map.clone());
    let had_errors = diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &diags {
        emit_diag(d, mode, src);
    }
    if had_errors {
        return Err(ExitCode::FAILURE);
    }
    // Phase 5 borrow checker (slice 5BC.2a).
    let bc_diags = borrowck::check(&prog, &file.to_path_buf(), src);
    let bc_errors = bc_diags
        .iter()
        .any(|d| matches!(d.severity, Severity::Error));
    for d in &bc_diags {
        emit_diag(d, mode, src);
    }
    if bc_errors {
        return Err(ExitCode::FAILURE);
    }
    let post_mono = run_monomorphize(prog, &mono, &files_map);
    let dbg_path = if debug_info { Some(file) } else { None };
    ensure_coro_end_probed();
    Ok(codegen::generate_with_mono(
        &post_mono,
        build_mode,
        fp_contract,
        dbg_path,
        sanitizers,
        false,
        &mono,
    ))
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
    //   Release -> `-O2`. Engages LLVM's standard inlining, mem2reg,
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
    // The program object goes FIRST, before any libraries. GNU `ld`
    // resolves left-to-right in a single pass and pulls a static-archive
    // member only to satisfy a reference it has ALREADY seen. So a bundled
    // `lib*.a` listed before the object that calls into it contributes
    // nothing and its symbols come up undefined (macOS's ld64 does a full
    // resolution and is order-insensitive, which is why this only bites on
    // Linux). Emit `input_ll`, then the manifest link args, then `-lm`.
    cmd.arg(input_ll);
    // v0.0.2 (AppKit-via-Cplus.toml): manifest-driven linker args. Each
    // entry was generated by `build_project` from `[[bin]] frameworks`
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
    let status = cmd.arg("-o").arg(out).status();
    match status {
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
    use cplus_core::ast::{ItemKind, TypeKind};
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
        // Generics, borrows, slices, tuples — not C-representable.
        TypeKind::Generic { .. }
        | TypeKind::Borrowed { .. }
        | TypeKind::Slice(_)
        | TypeKind::Tuple(_) => return None,
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
/// always version-matched to this `cpc`. Source of truth: `docs/SKILL.md`.
const SKILL_MD: &str = include_str!("../../docs/SKILL.md");

const SKILL_USAGE: &str = "\
cpc skill - print the C+ reference for an LLM/agent (version-matched to this cpc)

usage:
  cpc skill                 print the reference to stdout
  cpc skill --write [PATH]  write it into the project (default: ./SKILL.md)
  cpc skill --write --force overwrite an existing file
";

/// `cpc skill [--write [PATH]] [--force]`.
fn run_skill(args: &[OsString]) -> ExitCode {
    let mut write = false;
    let mut force = false;
    let mut dest: Option<PathBuf> = None;
    for a in args {
        match a.to_str() {
            Some("--write") | Some("-w") => write = true,
            Some("--force") | Some("-f") => force = true,
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

    if !write {
        print!("{SKILL_MD}");
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
    match std::fs::write(&path, SKILL_MD) {
        Ok(()) => {
            println!("wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cpc skill: could not write {}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}

// ---- `cpc pm` : the package manager, unified under cpc ----------------------

/// `cpc pm <command> ...` — dispatch to the package manager (the same
/// dispatcher as the standalone `cplus-pm` binary). Shipping it under `cpc`
/// means the one Homebrew-installed toolchain carries the package manager too.
fn run_pm(args: &[OsString]) -> ExitCode {
    let strs: Option<Vec<String>> =
        args.iter().map(|a| a.to_str().map(String::from)).collect();
    let Some(strs) = strs else {
        eprintln!("cpc pm: arguments must be valid UTF-8");
        return ExitCode::FAILURE;
    };
    match cplus_pm::cli::run(strs) {
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
  cpc init [NAME]   create a project. With NAME, scaffold into NAME/; without,
                    scaffold in the current directory (name = directory name).

writes: Cplus.toml, src/main.cplus, .gitignore, SKILL.md
";

/// `cpc init [NAME]`.
fn run_init(args: &[OsString]) -> ExitCode {
    let mut name: Option<String> = None;
    for a in args {
        match a.to_str() {
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

    let manifest_toml = format!(
        "[package]\nname    = \"{proj_name}\"\nversion = \"0.0.1\"\nedition = \"2026\"\n\n\
         [[bin]]\nname = \"{proj_name}\"\npath = \"src/main.cplus\"\n\n\
         [dependencies]\nstdlib = \"*\"\n"
    );
    let main_cplus = "import \"stdlib/io\" as io;\n\n\
         fn main() -> i32 {\n    io::println(\"hello from C+\");\n    return 0;\n}\n";
    let gitignore = "/target\n/vendor\n";

    let files: [(PathBuf, &str); 4] = [
        (manifest, &manifest_toml),
        (src.join("main.cplus"), main_cplus),
        (root.join(".gitignore"), gitignore),
        // The agent reference, so the fresh project is immediately LLM-ready.
        (root.join("SKILL.md"), SKILL_MD),
    ];
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
    println!("  cpc build            # compile and link");
    println!();
    println!("note: src/main.cplus imports `stdlib/io`; vendor the stdlib package into");
    println!("      vendor/stdlib before building — from");
    println!("      https://github.com/netdur/cplus/tree/main/vendor/stdlib");
    ExitCode::SUCCESS
}
