use cplus_core::codegen::BuildMode;
use cplus_core::diagnostics::{self as diag, Diagnostic, LineMap, Severity};
use cplus_core::{codegen, lexer, parser, sema};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const HELLO_LL: &str = include_str!("hello.ll");

const USAGE: &str = "\
cpc — C+ compiler

usage:
  cpc FILE [-o OUT]                 compile FILE.cplus to a binary (default OUT: ./a.out)
  cpc [-o OUT]                      with no FILE: emit the Phase-0 hello-world demo
  cpc --release [...]               release mode: no overflow checks on `+ - *` (default: debug, checked)
  cpc --emit-ir                     print the frozen Phase-0 LLVM IR to stdout
  cpc --tokens FILE                 lex FILE and print the token stream (debug)
  cpc --ast FILE                    lex+parse FILE and print the AST (debug)
  cpc --emit-ll FILE                lex+parse+sema+codegen FILE and print the .ll IR
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
                return dump_ll(PathBuf::from(v), diag_mode, build_mode);
            }
            Some("--release") => {
                build_mode = BuildMode::Release;
                i += 1;
            }
            Some("--debug") => {
                build_mode = BuildMode::Debug;
                i += 1;
            }
            Some("-h" | "--help") => {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some(s) if s.starts_with('-') => {
                eprintln!("cpc: unknown flag: {s}");
                eprintln!("{USAGE}");
                return ExitCode::FAILURE;
            }
            _ => {
                if input.is_some() {
                    eprintln!("cpc: multiple input files not yet supported");
                    return ExitCode::FAILURE;
                }
                input = Some(PathBuf::from(&args[i]));
                i += 1;
            }
        }
    }

    match input {
        Some(path) => compile_file(
            path,
            out.unwrap_or_else(|| PathBuf::from("a.out")),
            diag_mode,
            build_mode,
        ),
        None => phase0_hello(out.unwrap_or_else(|| PathBuf::from("hello"))),
    }
}

fn phase0_hello(out: PathBuf) -> ExitCode {
    let tmp = env::temp_dir().join(format!("cpc-{}.ll", std::process::id()));
    if let Err(e) = fs::write(&tmp, HELLO_LL) {
        eprintln!("cpc: writing IR to {}: {e}", tmp.display());
        return ExitCode::FAILURE;
    }
    let status = run_clang(&tmp, &out);
    let _ = fs::remove_file(&tmp);
    status
}

fn compile_file(input: PathBuf, out: PathBuf, mode: DiagMode, build_mode: BuildMode) -> ExitCode {
    let src = match fs::read_to_string(&input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let ir = match build_ir(&input, &src, mode, build_mode) {
        Ok(ir) => ir,
        Err(code) => return code,
    };
    let tmp = env::temp_dir().join(format!("cpc-{}.ll", std::process::id()));
    if let Err(e) = fs::write(&tmp, &ir) {
        eprintln!("cpc: writing IR to {}: {e}", tmp.display());
        return ExitCode::FAILURE;
    }
    let status = run_clang(&tmp, &out);
    let _ = fs::remove_file(&tmp);
    status
}

fn build_ir(file: &Path, src: &str, mode: DiagMode, build_mode: BuildMode) -> Result<String, ExitCode> {
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
    let diags = sema::check(&prog, file.to_path_buf(), src);
    let had_errors = diags.iter().any(|d| matches!(d.severity, Severity::Error));
    for d in &diags {
        emit_diag(d, mode, src);
    }
    if had_errors {
        return Err(ExitCode::FAILURE);
    }
    Ok(codegen::generate(&prog, build_mode))
}

fn run_clang(input_ll: &Path, out: &Path) -> ExitCode {
    let status = Command::new("clang")
        .arg("-Wno-override-module")
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

fn dump_ll(path: PathBuf, mode: DiagMode, build_mode: BuildMode) -> ExitCode {
    let src = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cpc: read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    match build_ir(&path, &src, mode, build_mode) {
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
