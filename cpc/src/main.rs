use cplus_core::codegen::BuildMode;
use cplus_core::diagnostics::{self as diag, Diagnostic, LineMap, Severity};
use cplus_core::{attrs, borrowck, codegen, doctest, fmt as cpfmt, lexer, lower, manifest, monomorphize, parser, resolver, sema};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const HELLO_LL: &str = include_str!("hello.ll");

const USAGE: &str = "\
cpc — C+ compiler

usage:
  cpc FILE [-o OUT]                 compile single-file FILE.cplus to a binary (default OUT: ./a.out)
  cpc build [-o OUT]                multi-file build: reads ./Cplus.toml, walks imports
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
  cpc --release [...]               release mode: no overflow checks on `+ - *` (default: debug, checked)
  cpc --emit-ir                     print the frozen Phase-0 LLVM IR to stdout
  cpc --tokens FILE                 lex FILE and print the token stream (debug)
  cpc --ast FILE                    lex+parse FILE and print the AST (debug)
  cpc --emit-ll FILE                lex+parse+sema+codegen FILE and print the .ll IR
  cpc --emit-ll-project             multi-file: print the merged IR to stdout (uses ./Cplus.toml)
  cpc --diagnostics=MODE [...]      diagnostics output: human (default) | short | json
  cpc -h | --help                   show this message
";

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
    let mut subcommand: Option<Subcommand> = None;
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
                return dump_ll(PathBuf::from(v), diag_mode, build_mode, emit_debug_info);
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
            Some("-h" | "--help") => {
                print!("{USAGE}");
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

    // `cpc lsp` forwards any remaining args to the cpc-lsp binary.
    // (`--log PATH` is the only one cpc-lsp accepts in slice 4E.1, but
    // we don't reach into here — just pass everything past `lsp`.)
    let lsp_args: Vec<OsString> = match subcommand {
        Some(Subcommand::Lsp) => args.into_iter().skip_while(|a| a != "lsp").skip(1).collect(),
        _ => Vec::new(),
    };

    match (subcommand, input) {
        (Some(Subcommand::Build), _) => build_project(out, diag_mode, build_mode),
        (Some(Subcommand::EmitLlProject), _) => emit_ll_project(diag_mode, build_mode),
        (Some(Subcommand::Fmt), _) => run_fmt(fmt_inputs, fmt_opts, diag_mode),
        (Some(Subcommand::Test), _) => run_test(test_input, test_opts, diag_mode, build_mode),
        (Some(Subcommand::Lsp), _) => run_lsp(lsp_args),
        (None, Some(path)) => compile_file(
            path,
            out.unwrap_or_else(|| PathBuf::from("a.out")),
            diag_mode,
            build_mode,
            emit_debug_info,
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

    let (program, _entry_file_id) = match load_and_check_project(&bin.path, &m.root, diag_mode) {
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
    let tmp = env::temp_dir().join(format!("cpc-{}.ll", std::process::id()));
    if let Err(e) = fs::write(&tmp, &ir) {
        eprintln!("cpc: writing IR to {}: {e}", tmp.display());
        return ExitCode::FAILURE;
    }
    let status = run_clang(&tmp, &out_path, build_mode, false);
    let _ = fs::remove_file(&tmp);
    status
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
    let (program, _) = match load_and_check_project(&bin.path, &m.root, diag_mode) {
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
    let mut loaded = match resolver::load_project(entry, root) {
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
            let (program, _) = match load_and_check_project(&bin.path, &m.root, diag_mode) {
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
    let tmp = env::temp_dir().join(format!("cpc-test-{}.ll", std::process::id()));
    if let Err(e) = fs::write(&tmp, &ir) {
        eprintln!("cpc test: writing IR to {}: {e}", tmp.display());
        return ExitCode::FAILURE;
    }
    let bin_out = env::temp_dir().join(format!("cpc-test-{}.bin", std::process::id()));
    let clang_status = run_clang(&tmp, &bin_out, build_mode, false);
    let _ = fs::remove_file(&tmp);
    if !matches!(clang_status, ExitCode::SUCCESS) {
        let _ = fs::remove_file(&bin_out);
        return clang_status;
    }
    // Run the test binary. Its stdout is what `cpc test` prints; its exit
    // code equals the number of failing tests (clamped into [0, 255] so the
    // process-exit-code-as-u8 convention still fits).
    let status = Command::new(&bin_out).status();
    let _ = fs::remove_file(&bin_out);
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
    let tmp = env::temp_dir().join(format!("cpc-{}.ll", std::process::id()));
    if let Err(e) = fs::write(&tmp, HELLO_LL) {
        eprintln!("cpc: writing IR to {}: {e}", tmp.display());
        return ExitCode::FAILURE;
    }
    let status = run_clang(&tmp, &out, BuildMode::Debug, false);
    let _ = fs::remove_file(&tmp);
    status
}

fn compile_file(input: PathBuf, out: PathBuf, mode: DiagMode, build_mode: BuildMode, debug_info: bool) -> ExitCode {
    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let ir = match build_ir(&input, &src, mode, build_mode, debug_info) {
        Ok(ir) => ir,
        Err(code) => return code,
    };
    let tmp = env::temp_dir().join(format!("cpc-{}.ll", std::process::id()));
    if let Err(e) = fs::write(&tmp, &ir) {
        eprintln!("cpc: writing IR to {}: {e}", tmp.display());
        return ExitCode::FAILURE;
    }
    let status = run_clang(&tmp, &out, build_mode, debug_info);
    let _ = fs::remove_file(&tmp);
    status
}

fn build_ir(file: &Path, src: &str, mode: DiagMode, build_mode: BuildMode, debug_info: bool) -> Result<String, ExitCode> {
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
    if debug_info {
        Ok(codegen::generate_with_debug(&post_mono, build_mode, file))
    } else {
        Ok(codegen::generate(&post_mono, build_mode))
    }
}

fn run_clang(input_ll: &Path, out: &Path, mode: BuildMode, debug_info: bool) -> ExitCode {
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

fn dump_ll(path: PathBuf, mode: DiagMode, build_mode: BuildMode, debug_info: bool) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match build_ir(&path, &src, mode, build_mode, debug_info) {
        Ok(ir) => { print!("{ir}"); ExitCode::SUCCESS }
        Err(code) => code,
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
