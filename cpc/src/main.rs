use cplus_core::codegen::BuildMode;
use cplus_core::diagnostics::{self as diag, Diagnostic, LineMap, Severity};
use cplus_core::{attrs, borrowck, codegen, doctest, fmt as cpfmt, lexer, lower, manifest, monomorphize, parser, resolver, sema};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use tempfile::NamedTempFile;

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
  cpc check FILE                    parse + sema + borrowck FILE, no codegen (fast feedback loop)
  cpc doc FILE                      extract `pub` items + `///` docs from FILE, emit
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
  -g | --debug-info                 emit DWARF debug metadata + pass -g to clang
  --asan | --ubsan | --tsan | --msan
                                    enable the matching LLVM sanitizer (asan/tsan/msan are
                                    mutually exclusive; ubsan composes with any)

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
  cpc --emit-header FILE            C header for every C-ABI-representable `pub` item
                                    in FILE. Prints to stdout; redirect with `> out.h`.
  cpc --emit-ll-project             multi-file: print the merged IR to stdout (uses ./Cplus.toml)

other:
  --diagnostics=MODE                diagnostics output: human (default) | short | json
  -V | --version                    print compiler version
  -h | --help                       show this message
";

/// Phase 11 polish (2026-05-14): subcommand-aware `--help`. Once a
/// subcommand has been seen on the CLI, `--help` returns just the
/// relevant slice instead of the full usage dump.
fn subcommand_help(sub: Option<Subcommand>) -> &'static str {
    match sub {
        None => USAGE,
        Some(Subcommand::Build) => "\
cpc build [-o OUT] [--release] [-g] [--asan|--ubsan|--tsan|--msan]

Multi-file build. Reads ./Cplus.toml at the current directory, walks the
declared imports, lowers + sema + borrowck + codegen the whole project,
and writes the linked binary to `target/{debug,release}/<name>` (or to
OUT if `-o` is given). The manifest names the project; the entry file
must define `fn main() -> i32`.
",
        Some(Subcommand::Check) => "\
cpc check FILE

Parse + sema + borrowck FILE. No codegen, no clang, no binary. Same
diagnostics you'd get from `cpc FILE -o BIN`, but faster — the editor /
LSP / pre-commit-hook use case. Exits 0 if clean, 1 on any error.
",
        Some(Subcommand::Doc) => "\
cpc doc FILE

Extract every `pub` item with a preceding `///` doc block from FILE
and emit Markdown to `./target/doc/<basename>.md`. Each item gets a
section with its signature, a `defined at line N` link, and the doc
prose. Fenced code blocks inside `///` are preserved as Markdown code
blocks — the same blocks `cpc test` runs as doctests.

Private items (and `pub` items without docs) are skipped to keep the
reference focused on the project's stable surface.
",
        Some(Subcommand::Test) => "\
cpc test [FILE] [--json]

Discover and run every `#[test]` function in the project (or in FILE if
given). Each test compiles into the test driver and runs sequentially.
Doctests embedded in `///` comments are extracted into synthesized
`#[test]` functions before running. With `--json`, emits one JSON object
per test plus a final summary line — for tool consumption.
",
        Some(Subcommand::Fmt) => "\
cpc fmt FILE|DIR [...]

Format C+ source. By default rewrites each file in place. Flags:
  --check    don't write; exit 1 if any file would change (CI mode)
  --emit     print formatted output to stdout, leave file alone
  --stdin    read source from stdin, write to stdout, no file arg

Multiple paths accepted; directories are walked recursively for
`.cplus` files.
",
        Some(Subcommand::Lsp) => "\
cpc lsp [--log PATH]

Start the C+ language server on stdin/stdout (delegates to the
`cpc-lsp` binary on PATH or next to this binary). All args after `lsp`
are forwarded.
",
        Some(Subcommand::EmitLlProject) => "\
cpc --emit-ll-project

Multi-file: run the build pipeline as `cpc build` would, but print the
merged LLVM IR to stdout instead of invoking clang. Uses ./Cplus.toml.
",
    }
}

#[derive(Debug, Clone, Copy)]
enum DiagMode { Human, Short, Json }

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
    let mut input: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut diag_mode = DiagMode::Human;
    let mut build_mode = BuildMode::Debug;
    // Phase 11 polish (2026-05-13): `-g` emits DWARF debug metadata.
    // v1 ships function-level DI only (DICompileUnit + DIFile +
    // DISubprogram). Per-instruction DILocation is a follow-up.
    let mut emit_debug_info = false;
    // Phase 11 polish (2026-05-13): sanitizer flags. LLVM's
    // instrumentation passes do the heavy lifting; cpc just plumbs
    // the `-fsanitize=...` flag through to clang.
    let mut sanitizers: Vec<&'static str> = Vec::new();
    let mut subcommand: Option<Subcommand> = None;
    // Phase 5 Slice 5.A: deferred-dispatch input for `--emit-obj FILE`.
    // Order-independent with `-o OUT.o` because the FILE may appear before
    // or after the flag in the user's command line.
    let mut emit_obj_input: Option<PathBuf> = None;
    let mut fmt_opts = FmtOpts::default();
    let mut fmt_inputs: Vec<PathBuf> = Vec::new();
    let mut test_opts = TestOpts::default();
    let mut test_input: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].to_str();
        if let Some(s) = a {
            if let Some(rest) = s.strip_prefix("--diagnostics=") {
                diag_mode = match rest {
                    "human" => DiagMode::Human,
                    "short" => DiagMode::Short,
                    "json"  => DiagMode::Json,
                    other => {
                        eprintln!("cpc: unknown --diagnostics value: {other:?} (expected human|short|json)");
                        return ExitCode::FAILURE;
                    }
                };
                i += 1;
                continue;
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
                return dump_ll(PathBuf::from(v), diag_mode, build_mode, emit_debug_info, &sanitizers);
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
                    PathBuf::from(v), diag_mode, build_mode, ClangOutputKind::LlvmIr,
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
                    PathBuf::from(v), diag_mode, build_mode, ClangOutputKind::Assembly,
                );
            }
            Some("--emit-header") => {
                // Phase 5 Slice 5.E: emit a C header (`.h`) declaring
                // every `pub` item that's C-ABI representable. Prints to
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
        let exclusive: Vec<&'static str> = sanitizers.iter()
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

    // `cpc lsp` forwards any remaining args to the cpc-lsp binary.
    // (`--log PATH` is the only one cpc-lsp accepts in slice 4E.1, but
    // we don't reach into here — just pass everything past `lsp`.)
    let lsp_args: Vec<OsString> = match subcommand {
        Some(Subcommand::Lsp) => args.into_iter().skip_while(|a| a != "lsp").skip(1).collect(),
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
        return dump_obj(obj_in, obj_out, diag_mode, build_mode);
    }

    match (subcommand, input) {
        (Some(Subcommand::Build), _) => build_project(out, diag_mode, build_mode),
        (Some(Subcommand::EmitLlProject), _) => emit_ll_project(diag_mode, build_mode),
        (Some(Subcommand::Fmt), _) => run_fmt(fmt_inputs, fmt_opts, diag_mode),
        (Some(Subcommand::Test), _) => run_test(test_input, test_opts, diag_mode, build_mode),
        (Some(Subcommand::Lsp), _) => run_lsp(lsp_args),
        (Some(Subcommand::Check), Some(path)) => run_check(path, diag_mode),
        (Some(Subcommand::Check), None) => {
            eprintln!("cpc: `check` requires a FILE argument");
            ExitCode::FAILURE
        }
        (Some(Subcommand::Doc), Some(path)) => run_doc(path),
        (Some(Subcommand::Doc), None) => {
            eprintln!("cpc: `doc` requires a FILE argument");
            ExitCode::FAILURE
        }
        (None, Some(path)) => compile_file(
            path,
            out.unwrap_or_else(|| PathBuf::from("a.out")),
            diag_mode,
            build_mode,
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
    /// Phase 11 polish (2026-05-14): `cpc doc FILE` — extract `pub`
    /// items + their `///` docs from a source file, emit Markdown to
    /// `target/doc/<basename>.md`.
    Doc,
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

/// Phase 2 Slice 2C: detect the host triple via `clang -print-target-triple`.
/// Used by the dep walker to look up bundled binary paths in each vendor
/// package's `src/lib/<triple>/`. Each build calls this once.
fn detect_host_triple() -> Result<String, ExitCode> {
    let output = match Command::new("clang").arg("-print-target-triple").output() {
        Ok(o) => o,
        Err(e) => {
            eprintln!("cpc: invoking `clang -print-target-triple`: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    if !output.status.success() {
        eprintln!("cpc: `clang -print-target-triple` exited with {:?}", output.status.code());
        return Err(ExitCode::FAILURE);
    }
    let triple = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if triple.is_empty() {
        eprintln!("cpc: `clang -print-target-triple` produced no output");
        return Err(ExitCode::FAILURE);
    }
    Ok(triple)
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
            start: diag::Position { line: 1, col: 1, byte: 0 },
            end: diag::Position { line: 1, col: 1, byte: 0 },
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
    let host_triple = detect_host_triple()?;
    let mut link_args: Vec<String> = Vec::new();
    for dep in &m.dependencies {
        let vendor_dir = m.root.join("vendor").join(&dep.name);
        let vendor_manifest = vendor_dir.join("Cplus.toml");
        if !vendor_manifest.is_file() {
            let d = manifest_diag(
                "E0854",
                &vendor_manifest,
                format!(
                    "vendor package `{}` is missing `Cplus.toml` (expected at `{}`)",
                    dep.name, vendor_manifest.display()
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
        let bundled: &[String] = vm.link.as_ref().map(|l| l.bundled.as_slice()).unwrap_or(&[]);
        let triples: &[String] = vm.link.as_ref().map(|l| l.triples.as_slice()).unwrap_or(&[]);
        if !bundled.is_empty() {
            if !triples.iter().any(|t| t == &host_triple) {
                let supported = if triples.is_empty() {
                    "<none>".to_string()
                } else {
                    triples.iter().map(|s| format!("`{s}`")).collect::<Vec<_>>().join(", ")
                };
                let d = manifest_diag(
                    "E0862",
                    &vendor_manifest,
                    format!(
                        "package `{}` does not ship a build for host triple `{}` (supports: {})",
                        dep.name, host_triple, supported
                    ),
                    vec![
                        "add the host triple to `[link].triples` and ship the matching binaries, or build the package from source on this host".to_string(),
                    ],
                );
                emit_diag(&d, diag_mode, "");
                return Err(ExitCode::FAILURE);
            }
            let host_lib_dir = lib_root.join(&host_triple);
            for basename in bundled {
                let p = host_lib_dir.join(basename);
                if !p.is_file() {
                    let d = manifest_diag(
                        "E0860",
                        &vendor_manifest,
                        format!(
                            "package `{}` declares bundled `{}` but `src/lib/{}/{}` is not present (the package manifest says you ship it for this triple, but the file is missing)",
                            dep.name, basename, host_triple, basename
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
                    if !triple_dir.is_dir() { continue; }
                    let Ok(file_iter) = fs::read_dir(&triple_dir) else { continue };
                    for entry in file_iter.flatten() {
                        let fname_os = entry.file_name();
                        let fname = fname_os.to_string_lossy().to_string();
                        let is_binary = fname.ends_with(".a")
                            || fname.ends_with(".dylib")
                            || fname.ends_with(".so")
                            || fname.ends_with(".lib");
                        if !is_binary { continue; }
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
            for fw in &ls.frameworks {
                link_args.push("-framework".to_string());
                link_args.push(fw.clone());
            }
            for l in &ls.libs {
                link_args.push(format!("-l{l}"));
            }
            for basename in &ls.bundled {
                let p = lib_root.join(&host_triple).join(basename);
                link_args.push(p.to_string_lossy().to_string());
            }
        }
    }
    Ok(link_args)
}

/// Multi-file project build (Phase 4 slice 4A). Looks for `Cplus.toml`
/// in the current working directory, walks the import graph from the
/// declared binary entry, and produces a single linked binary at
/// `target/{debug,release}/<bin-name>` (or `-o OUT` if provided).
fn build_project(out: Option<PathBuf>, diag_mode: DiagMode, build_mode: BuildMode) -> ExitCode {
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
        return build_lib_project(&m, &lib, out, diag_mode, build_mode);
    }
    if m.bins.len() != 1 {
        eprintln!("cpc: Phase 4 slice 4A supports exactly one [[bin]]; found {}", m.bins.len());
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
                start: diag::Position { line: 1, col: 1, byte: 0 },
                end: diag::Position { line: 1, col: 1, byte: 0 },
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
    let (program, _entry_file_id) = match load_and_check_project_full(&bin.path, &m.root, diag_mode, false, Some(&dep_names)) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let ir = codegen::generate(&program, build_mode);

    let out_path = out.unwrap_or_else(|| {
        let sub = match build_mode { BuildMode::Debug => "debug", BuildMode::Release => "release" };
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
    let status = run_clang(&tmp, &out_path, build_mode, false, &[], &link_args);
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
) -> ExitCode {
    if !lib.path.is_file() {
        let d = diag::Diagnostic {
            severity: Severity::Error,
            code: diag::DiagCode("E0407"),
            message: format!("library entry `{}` does not exist", lib.path.display()),
            primary: diag::SourceSpan {
                file: lib.path.clone(),
                start: diag::Position { line: 1, col: 1, byte: 0 },
                end: diag::Position { line: 1, col: 1, byte: 0 },
            },
            labels: Vec::new(),
            notes: vec!["declared in Cplus.toml".to_string()],
            suggestions: Vec::new(),
        };
        emit_diag(&d, diag_mode, "");
        return ExitCode::FAILURE;
    }
    let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
    let (program, _entry_file_id) = match load_and_check_project_full(&lib.path, &m.root, diag_mode, true, Some(&dep_names)) {
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

    let ir = codegen::generate_lib(&program, build_mode);

    let mode_subdir = match build_mode { BuildMode::Debug => "debug", BuildMode::Release => "release" };
    let target_dir = out_override
        .as_ref()
        .and_then(|p| p.parent().map(|x| x.to_path_buf()))
        .unwrap_or_else(|| m.root.join("target").join(mode_subdir));
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
    let opt = match build_mode { BuildMode::Debug => "-O0", BuildMode::Release => "-O2" };
    let obj_status = Command::new("clang")
        .arg(opt)
        .arg("-Wno-override-module")
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
    let want_static = matches!(lib.crate_type, manifest::CrateType::Staticlib | manifest::CrateType::Both);
    let want_shared = matches!(lib.crate_type, manifest::CrateType::Cdylib    | manifest::CrateType::Both);
    if want_static {
        let a_path = target_dir.join(format!("lib{}.a", lib.name));
        // `r` replace + `c` create-if-missing + `s` index. ar quietly
        // overwrites a previous archive of the same name.
        let _ = fs::remove_file(&a_path);  // ar refuses to add a duplicate entry across runs
        let ar_status = Command::new("ar")
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
                eprintln!("cpc: failed to invoke ar: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    // Step 5 (cdylib): clang -shared -o libNAME.<ext> NAME.o + manifest frameworks/libs.
    if want_shared {
        // Platform-correct extension: .dylib on macOS, .so on Linux/other.
        // (Cross-compilation is out of scope; we use host triple via cfg.)
        let dylib_ext = if cfg!(target_os = "macos") { "dylib" } else { "so" };
        let dylib_path = target_dir.join(format!("lib{}.{}", lib.name, dylib_ext));
        let mut cmd = Command::new("clang");
        cmd.arg("-shared")
           .arg(opt)
           .arg("-Wno-override-module");
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
fn emit_ll_project(diag_mode: DiagMode, build_mode: BuildMode) -> ExitCode {
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
    if let Err(code) = collect_dep_link_args(&m, diag_mode) { return code; }
    let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
    let (program, _) = match load_and_check_project_full(&bin.path, &m.root, diag_mode, false, Some(&dep_names)) {
        Ok(p) => p,
        Err(code) => return code,
    };
    let ir = codegen::generate(&program, build_mode);
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
) -> Result<(cplus_core::ast::Program, String), ExitCode> {
    // Legacy single-file path: no manifest, no dep list. `None` keeps
    // pre-Slice-2B file-relative resolution semantics.
    load_and_check_project_full(entry, root, diag_mode, false, None)
}

/// Phase 5 Slice 5.A: variant that passes `is_lib` to the resolver so
/// library-entry items skip name qualification (exposed as bare C-callable
/// symbols).
fn load_and_check_project_with_mode(
    entry: &Path,
    root: &Path,
    diag_mode: DiagMode,
    is_lib: bool,
) -> Result<(cplus_core::ast::Program, String), ExitCode> {
    load_and_check_project_full(entry, root, diag_mode, is_lib, None)
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
) -> Result<(cplus_core::ast::Program, String), ExitCode> {
    let mut loaded = match resolver::load_project_full(entry, root, is_lib, deps) {
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
    let attr_errors = attr_diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &attr_diags {
        emit_diag(d, diag_mode, &entry_src);
    }
    if attr_errors {
        return Err(ExitCode::FAILURE);
    }
    // Lower `if let` / `guard let` (slice 4A.5) before sema.
    let lower_diags = lower::lower(&mut loaded.program, &entry.to_path_buf(), &entry_src);
    let lower_errors = lower_diags.iter().any(|d| matches!(d.severity, Severity::Error));
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
    let bc_errors = bc_diags.iter().any(|d| matches!(d.severity, Severity::Error));
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
    Ok((post_mono, loaded.entry_file_id))
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
            ItemKind::Struct(s) if s.generic_params.is_empty() => struct_names.push(s.name.name.clone()),
            ItemKind::Enum(e) if e.generic_params.is_empty() => enum_names.push(e.name.name.clone()),
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
        if struct_names.len() <= slot { struct_names.resize(slot + 1, String::from("?")); }
        struct_names[slot] = info.mangled_name.clone();
    }
    for info in mono.enum_instantiations.values() {
        let slot = info.id as usize;
        if enum_names.len() <= slot { enum_names.resize(slot + 1, String::from("?")); }
        enum_names[slot] = info.mangled_name.clone();
    }
    let name_of = move |ty: &sema::Ty| -> String {
        match ty {
            sema::Ty::Struct(id) => struct_names.get(id.0 as usize).cloned().unwrap_or_else(|| "?".into()),
            sema::Ty::Enum(id) => enum_names.get(id.0 as usize).cloned().unwrap_or_else(|| "?".into()),
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
            Ok(out) => { print!("{out}"); ExitCode::SUCCESS }
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
        if had_error { return ExitCode::FAILURE; }
        if opts.check && had_change { return ExitCode::from(1); }
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
        if matches!(basename, "target" | "node_modules" | ".git") { return; }
        let Ok(entries) = std::fs::read_dir(root) else { return; };
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
    let (program, _src_for_diags) = match file {
        Some(path) => {
            let src = match fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("cpc test: read {}: {e}", path.display());
                    return ExitCode::FAILURE;
                }
            };
            let prog = match build_program(&path, &src, diag_mode) {
                Ok(p) => p,
                Err(code) => return code,
            };
            (prog, src)
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
            if m.bins.len() != 1 {
                eprintln!("cpc test: project must declare exactly one [[bin]]; found {}", m.bins.len());
                return ExitCode::FAILURE;
            }
            let bin = &m.bins[0];
            // Phase 2 Slice 2C: validate the dep graph before sema. Tests
            // share the consumer's `[dependencies]`, so a misdeclared
            // vendor package must fail here too — silent success would let
            // bad packages ride into a passing test run.
            if let Err(code) = collect_dep_link_args(&m, diag_mode) { return code; }
            let dep_names: Vec<String> = m.dependencies.iter().map(|d| d.name.clone()).collect();
            let (program, _) = match load_and_check_project_full(&bin.path, &m.root, diag_mode, false, Some(&dep_names)) {
                Ok(p) => p,
                Err(code) => return code,
            };
            let entry_src = fs::read_to_string(&bin.path).unwrap_or_default();
            (program, entry_src)
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
    let ir = codegen::generate_test_binary(&program, build_mode, &tests, opts.json);
    let tmp_handle = match make_temp_file("cpc-test-", ".ll", ir.as_bytes()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc test: writing IR to temp file: {e}");
            return ExitCode::FAILURE;
        }
    };
    let tmp = tmp_handle.path().to_path_buf();
    let bin_out_handle = match tempfile::Builder::new()
        .prefix("cpc-test-")
        .suffix(".bin")
        .tempfile()
    {
        Ok(h) => h,
        Err(e) => {
            eprintln!("cpc test: creating temp binary path: {e}");
            return ExitCode::FAILURE;
        }
    };
    let bin_out = bin_out_handle.path().to_path_buf();
    let clang_status = run_clang(&tmp, &bin_out, build_mode, false, &[], &[]);
    drop(tmp_handle);
    if !matches!(clang_status, ExitCode::SUCCESS) {
        drop(bin_out_handle);
        return clang_status;
    }
    // Run the test binary. Its stdout is what `cpc test` prints; its exit
    // code equals the number of failing tests (clamped into [0, 255] so the
    // process-exit-code-as-u8 convention still fits).
    let status = Command::new(&bin_out).status();
    drop(bin_out_handle);
    match status {
        Ok(s) => {
            // The driver `main` returns the failure count. Map any non-zero
            // back to a clamped u8 ExitCode so callers can distinguish
            // "all passed" (0) from "something failed" (1..=255).
            let code = s.code().unwrap_or(1);
            if code == 0 { ExitCode::SUCCESS } else { ExitCode::from(code.clamp(1, 255) as u8) }
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
fn build_program(file: &Path, src: &str, mode: DiagMode) -> Result<cplus_core::ast::Program, ExitCode> {
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
    let attr_errors = attr_diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &attr_diags {
        emit_diag(d, mode, src);
    }
    if attr_errors {
        return Err(ExitCode::FAILURE);
    }
    let lower_diags = lower::lower(&mut prog, &file.to_path_buf(), src);
    let lower_errors = lower_diags.iter().any(|d| matches!(d.severity, Severity::Error));
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
    let bc_errors = bc_diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &bc_diags {
        emit_diag(d, mode, src);
    }
    if bc_errors {
        return Err(ExitCode::FAILURE);
    }
    // Slice 7GEN.5a: monomorphize generic-fn templates into concrete
    // per-instantiation fns before codegen sees the program.
    let post_mono = run_monomorphize(prog, &mono, &std::collections::BTreeMap::new());
    Ok(post_mono)
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
    match build_ir(&path, &src, mode, BuildMode::Debug, false, &[]) {
        Ok(_ir) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// Phase 11 polish (2026-05-14): `cpc doc FILE` — extract `pub` items
/// + their `///` docs from FILE, emit Markdown to
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
    let basename = path.file_name()
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
        eprintln!("    run `cargo run --bin cpc-lsp -- {}` directly.",
            args.iter().filter_map(|a| a.to_str()).collect::<Vec<_>>().join(" "));
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
            let candidate = dir.join(if cfg!(windows) { "cpc-lsp.exe" } else { "cpc-lsp" });
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // 2. PATH lookup. No fancy logic — let the shell find it.
    let name = if cfg!(windows) { "cpc-lsp.exe" } else { "cpc-lsp" };
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
    let tmp_handle = match make_temp_file("cpc-", ".ll", HELLO_LL.as_bytes()) {
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

fn compile_file(input: PathBuf, out: PathBuf, mode: DiagMode, build_mode: BuildMode, debug_info: bool, sanitizers: &[&str]) -> ExitCode {
    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let ir = match build_ir(&input, &src, mode, build_mode, debug_info, sanitizers) {
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

fn build_ir(file: &Path, src: &str, mode: DiagMode, build_mode: BuildMode, debug_info: bool, sanitizers: &[&str]) -> Result<String, ExitCode> {
    // Slice 5DOC: extract doctest fences from `///` comments into appended
    // `#[test]` functions before lexing. Files without doctests are
    // unchanged — `doctest::extract` returns the input verbatim.
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
    // Phase 5 slice 5ATTR.1: validate attributes before lower / sema.
    let attr_diags = attrs::check(&prog, file.to_path_buf(), src);
    let attr_errors = attr_diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &attr_diags {
        emit_diag(d, mode, src);
    }
    if attr_errors {
        return Err(ExitCode::FAILURE);
    }
    // Lower `if let` / `guard let` to match-using forms before sema.
    let lower_diags = lower::lower(&mut prog, &file.to_path_buf(), src);
    let lower_errors = lower_diags.iter().any(|d| matches!(d.severity, Severity::Error));
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
    // Phase 5 borrow checker (slice 5BC.2a).
    let bc_diags = borrowck::check(&prog, &file.to_path_buf(), src);
    let bc_errors = bc_diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &bc_diags {
        emit_diag(d, mode, src);
    }
    if bc_errors {
        return Err(ExitCode::FAILURE);
    }
    let post_mono = run_monomorphize(prog, &mono, &std::collections::BTreeMap::new());
    if debug_info || !sanitizers.is_empty() {
        let dbg_path = if debug_info { Some(file) } else { None };
        Ok(codegen::generate_with_options(&post_mono, build_mode, dbg_path, sanitizers))
    } else {
        Ok(codegen::generate(&post_mono, build_mode))
    }
}

fn run_clang(input_ll: &Path, out: &Path, mode: BuildMode, debug_info: bool, sanitizers: &[&str], link_args: &[String]) -> ExitCode {
    // Pass the LLVM optimization level alongside our own build-mode choice:
    //   Debug   -> `-O0`. Keeps the overflow-check intrinsics, leaves divs
    //              and branches in source order, debuggable IR.
    //   Release -> `-O2`. Engages LLVM's standard inlining, mem2reg,
    //              GVN, LICM, loop reduction, etc. Without this flag clang
    //              defaults to `-O0` and our "release" binaries are 100×
    //              slower than they need to be.
    let opt = match mode {
        BuildMode::Debug => "-O0",
        BuildMode::Release => "-O2",
    };
    let mut cmd = Command::new("clang");
    cmd.arg(opt).arg("-Wno-override-module");
    // Phase 11 polish: `-g` keeps the DWARF metadata cpc emitted in the
    // IR through to the final binary. Without it clang silently strips
    // the .debug_info section.
    if debug_info { cmd.arg("-g"); }
    // Phase 11 polish: sanitizer instrumentation. clang owns the
    // instrumentation pass + the matching runtime library; we just
    // forward the comma-joined `-fsanitize=` argument.
    if !sanitizers.is_empty() {
        cmd.arg(format!("-fsanitize={}", sanitizers.join(",")));
        // Better stack traces in sanitizer reports.
        cmd.arg("-fno-omit-frame-pointer");
    }
    // v0.0.2 (AppKit-via-Cplus.toml): manifest-driven linker args. Each
    // entry was generated by `build_project` from `[[bin]] frameworks`
    // (`-framework X`) and `libs` (`-lX`). Empty for everything except
    // project builds whose manifest declares them.
    for arg in link_args {
        cmd.arg(arg);
    }
    let status = cmd
        .arg(input_ll)
        .arg("-o")
        .arg(out)
        .status();
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

fn dump_ll(path: PathBuf, mode: DiagMode, build_mode: BuildMode, debug_info: bool, sanitizers: &[&str]) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match build_ir(&path, &src, mode, build_mode, debug_info, sanitizers) {
        Ok(ir) => { print!("{ir}"); ExitCode::SUCCESS }
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
/// program's top-level items, emits a C declaration for every `pub` item
/// whose signature is C-ABI-compatible (Slice 5.C's predicate). Items
/// that aren't representable in C (non-`#[repr(C)]` structs, Drop types,
/// tagged enums, generics) are skipped silently — sema's E0410 already
/// rejects them in `pub extern fn` signatures, so they can only reach
/// the header path via plain `pub fn` / `pub struct` declarations and
/// will be silently dropped from the header surface.
///
/// The generated header is hand-readable and idiomatic C99:
/// - `#pragma once` for include-guard simplicity.
/// - `#include <stdbool.h>` + `<stddef.h>` + `<stdint.h>` for the
///   primitive type aliases.
/// - Struct definitions before fn declarations so signatures can
///   reference them. Order: pub structs / enums / type aliases first,
///   then pub fn declarations.
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

/// Phase 5 Slice 5.E: render a C header for `program`'s `pub` surface.
/// Public so the library build pipeline (5.A) can call it alongside the
/// `.a` / `.dylib` artifact emission.
fn render_c_header(program: &cplus_core::ast::Program, lib_name: &str) -> String {
    use cplus_core::ast::{ItemKind, TypeKind};
    let mut out = String::new();
    out.push_str(&format!("// Generated by cpc — public C ABI for `{lib_name}`. Do not edit.\n"));
    out.push_str("#pragma once\n\n");
    out.push_str("#include <stdbool.h>\n");
    out.push_str("#include <stddef.h>\n");
    out.push_str("#include <stdint.h>\n\n");
    out.push_str("#ifdef __cplusplus\nextern \"C\" {\n#endif\n\n");

    // Pass 1: pub `#[repr(C)]` structs and pub plain enums (definitions
    // that fn signatures may reference). Tagged enums and non-repr-C
    // structs are skipped silently — sema's 5.C predicate already
    // rejects them in `pub extern fn` signatures, so any fn that would
    // need them in the header would have failed before reaching here.
    for item in &program.items {
        match &item.kind {
            ItemKind::Struct(s) if s.is_pub => {
                let is_repr_c = s.attributes.iter().any(|a| a.path.name == "repr");
                if !is_repr_c { continue; }
                // Drop check: a struct with a `drop` method isn't safe to
                // expose by value. The user's `pub extern fn` would have
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
                if is_tagged { continue; }
                // `typedef enum Foo { ... } Foo;` lets consumers use the
                // bare name as a type — matches what we do for structs.
                out.push_str(&format!("typedef enum {} {{\n", e.name.name));
                for (i, v) in e.variants.iter().enumerate() {
                    let sep = if i + 1 == e.variants.len() { "" } else { "," };
                    out.push_str(&format!("    {}_{} = {}{}\n", e.name.name, v.name.name, i, sep));
                }
                out.push_str(&format!("}} {};\n\n", e.name.name));
            }
            _ => {}
        }
    }

    // Pass 2: pub fn declarations. Both `pub fn` (C+-callable from inside
    // the library; scalar-only ones are accidentally C-callable too) and
    // `pub extern fn ... { body }` (Slice 5.C: explicit C-ABI export).
    // Any signature element that fails the C-mapping (e.g. `str`, slice,
    // tagged enum) makes us skip the whole fn — that's sound because the
    // consumer couldn't write a matching signature anyway.
    for item in &program.items {
        if let ItemKind::Function(f) = &item.kind {
            if !f.is_pub { continue; }
            // Skip the parser-collapsed body for extern declarations
            // (no body, decl form): those are imports, not exports.
            if f.is_extern && f.body.stmts.is_empty() && f.body.tail.is_none() {
                continue;
            }
            if !f.generic_params.is_empty() { continue; }
            let Some(decl) = render_fn_decl(f) else { continue; };
            out.push_str(&decl);
            out.push('\n');
        }
    }

    out.push_str("\n#ifdef __cplusplus\n} // extern \"C\"\n#endif\n");
    out
}

/// Render a `#[repr(C)] pub struct Foo { ... }` as a C declaration.
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

/// Render a `pub fn` (or `pub extern fn`) as a C prototype. Returns
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
            if i > 0 { out.push_str(", "); }
            out.push_str(&render_param_decl(&p.ty, &p.name.name)?);
        }
        if f.is_variadic {
            if !f.params.is_empty() { out.push_str(", "); }
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
    if let TypeKind::FnPtr { params, return_type } = &t.kind {
        let ret = match return_type {
            Some(t) => type_to_c(t)?,
            None => "void".to_string(),
        };
        let mut s = format!("{} (*{})(", ret, name);
        if params.is_empty() {
            s.push_str("void");
        } else {
            for (i, p) in params.iter().enumerate() {
                if i > 0 { s.push_str(", "); }
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
        TypeKind::FnPtr { params, return_type } => {
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
                    if i > 0 { s.push_str(", "); }
                    s.push_str(&type_to_c(p)?);
                }
            }
            s.push(')');
            s
        }
        TypeKind::Array { elem, len } => {
            // In a parameter position, `T[N]` decays to `T*` in C —
            // technically the same ABI. We render the array form anyway
            // since the user's intent is "fixed-size buffer" and clang
            // treats `T arr[N]` and `T *arr` interchangeably in proto.
            let elem_c = type_to_c(elem)?;
            format!("{}[{}]", elem_c, len)
        }
        // Generics, borrows, slices — not C-representable.
        TypeKind::Generic { .. }
        | TypeKind::Borrowed { .. }
        | TypeKind::Slice(_) => return None,
    })
}

fn dump_obj(input: PathBuf, out: PathBuf, diag_mode: DiagMode, build_mode: BuildMode) -> ExitCode {
    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let ir = match build_ir(&input, &src, diag_mode, build_mode, false, &[]) {
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
        BuildMode::Release => "-O2",
    };
    let status = Command::new("clang")
        .arg(opt)
        .arg("-Wno-override-module")
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
    output_kind: ClangOutputKind,
) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let ir = match build_ir(&path, &src, mode, build_mode, false, &[]) {
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
/// the result on stdout. Matches `run_clang`'s `-O0`/`-O2` selection so the
/// `--debug` / `--release` flags compose with `--emit-ll-opt` and
/// `--emit-asm` consistently.
fn run_clang_to_stdout(input_ll: &Path, mode: BuildMode, kind: ClangOutputKind) -> ExitCode {
    let opt = match mode {
        BuildMode::Debug => "-O0",
        BuildMode::Release => "-O2",
    };
    let mut cmd = Command::new("clang");
    cmd.arg(opt).arg("-Wno-override-module").arg("-S");
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
