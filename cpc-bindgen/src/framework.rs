// cpc-bindgen framework mode — generate a whole C+ package from an Apple system
// framework. Discovers the framework's public headers from its umbrella header,
// emits one binding module per header (via the ObjC front-end), and writes the
// package skeleton the single-header mode leaves to the author: the umbrella
// module (imports + type re-exports), a `Cplus.toml` populated from the
// framework metadata, and a starter `overrides.json`.

use crate::objc::{loc_file, ObjcEmitter};
use crate::Emitter;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Generate the package. Returns a process exit code.
pub fn generate(
    name: &str,
    prefix: &str,
    overrides_path: Option<&str>,
    out_dir: Option<&str>,
) -> i32 {
    let sdk = match sdk_path() {
        Some(s) => s,
        None => {
            eprintln!("cpc-bindgen: could not resolve the SDK path (is `xcrun` available?)");
            return 1;
        }
    };
    let fw_dir = Path::new(&sdk)
        .join("System/Library/Frameworks")
        .join(format!("{name}.framework"));
    if !fw_dir.join("Headers").is_dir() {
        eprintln!(
            "cpc-bindgen: framework `{name}` not found at {}",
            fw_dir.display()
        );
        return 1;
    }
    let header_paths = discover_headers(&fw_dir, name);
    if header_paths.is_empty() {
        eprintln!("cpc-bindgen: framework `{name}` exposes no headers");
        return 1;
    }

    let overrides = match overrides_path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(e) => {
                    eprintln!("cpc-bindgen: overrides `{p}` parse failed: {e}");
                    return 1;
                }
            },
            Err(e) => {
                eprintln!("cpc-bindgen: cannot read overrides `{p}`: {e}");
                return 1;
            }
        },
        None => serde_json::Value::Null,
    };

    let pkg = name.to_lowercase();
    let out = PathBuf::from(out_dir.unwrap_or(&pkg));
    let src = out.join("src");
    if let Err(e) = std::fs::create_dir_all(&src) {
        eprintln!("cpc-bindgen: cannot create {}: {e}", src.display());
        return 1;
    }

    // One module per header. Detect Objective-C vs C per header (a header is
    // ObjC iff it defines an ObjC interface/protocol/category of its own) and
    // dispatch to the matching emitter, so C frameworks (Accelerate) bind too.
    // `any_objc` decides whether the package needs the `objc` runtime dep.
    let mut modules: Vec<(String, Vec<String>)> = Vec::new();
    let mut any_objc = false;
    for hp in &header_paths {
        let fname = hp.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let ast = match clang_ast(hp, &sdk) {
            Some(a) => a,
            None => {
                eprintln!("cpc-bindgen:   {fname}: clang failed, skipping");
                continue;
            }
        };
        let is_objc = header_is_objc(&ast, fname);
        let hp_str = hp.to_string_lossy();
        let text = if is_objc {
            any_objc = true;
            ObjcEmitter::new(&hp_str, prefix, overrides.clone()).run(&ast)
        } else {
            let mut e = Emitter::new(&hp_str);
            e.walk(&ast);
            e.finish()
        };
        let module = module_name(fname, if is_objc { prefix } else { "" });
        if modules.iter().any(|(m, _)| *m == module) {
            continue; // two headers snaked to the same module name; keep the first
        }
        let types = exported_types(&text);
        if let Err(e) = std::fs::write(src.join(format!("{module}.cplus")), &text) {
            eprintln!("cpc-bindgen:   {fname}: write failed: {e}");
            continue;
        }
        eprintln!(
            "  {fname} -> src/{module}.cplus ({}, {} types)",
            if is_objc { "objc" } else { "c" },
            types.len()
        );
        modules.push((module, types));
    }

    std::fs::write(src.join(format!("{pkg}.cplus")), build_umbrella(name, &modules)).ok();
    std::fs::write(
        out.join("Cplus.toml"),
        cplus_toml(name, &pkg, prefix, overrides_path.is_some(), &sdk_version(), modules.len(), any_objc),
    )
    .ok();
    if overrides_path.is_none() {
        let ov = out.join("overrides.json");
        if !ov.exists() {
            std::fs::write(&ov, overrides_stub(name)).ok();
        }
    }

    eprintln!(
        "cpc-bindgen: wrote package `{pkg}` ({} modules) to {}",
        modules.len(),
        out.display()
    );
    0
}

/// Absolute paths to a framework's public leaf headers. Handles two shapes: a
/// flat framework (headers under `Headers/`, listed by the umbrella's
/// `#import <Name/...>`) and an umbrella-of-subframeworks (Accelerate), whose
/// real headers live under `Frameworks/<Sub>.framework/Headers/`.
fn discover_headers(fw_dir: &Path, name: &str) -> Vec<PathBuf> {
    let headers_dir = fw_dir.join("Headers");
    let mut paths: Vec<PathBuf> = public_headers(&headers_dir.join(format!("{name}.h")), name)
        .into_iter()
        .map(|h| headers_dir.join(h))
        .collect();

    // Umbrella-of-subframeworks: recurse one level into each sub-framework's
    // `Headers/`, taking every header except that sub-framework's own umbrella.
    let subs = fw_dir.join("Frameworks");
    if subs.is_dir() {
        for entry in std::fs::read_dir(&subs).into_iter().flatten().flatten() {
            let p = entry.path();
            if let Some(sub) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".framework")) {
                let sub_headers = p.join("Headers");
                for h in list_headers(&sub_headers, sub) {
                    paths.push(sub_headers.join(h));
                }
            }
        }
    }

    // Fallback: the framework's own headers (minus its umbrella).
    if paths.is_empty() {
        for h in list_headers(&headers_dir, name) {
            paths.push(headers_dir.join(h));
        }
    }

    paths.sort();
    paths.dedup();
    paths
}

/// A header is Objective-C iff it declares an ObjC interface / protocol /
/// category *of its own* (decls from `#import`ed system headers don't count, so
/// we match the decl's `loc` file against this header). Everything else is C.
fn header_is_objc(tu: &serde_json::Value, header_basename: &str) -> bool {
    let Some(inner) = tu.get("inner").and_then(|v| v.as_array()) else {
        return false;
    };
    for decl in inner {
        let kind = decl.get("kind").and_then(|v| v.as_str());
        if matches!(
            kind,
            Some("ObjCInterfaceDecl") | Some("ObjCProtocolDecl") | Some("ObjCCategoryDecl")
        ) {
            if let Some(f) = decl.get("loc").and_then(loc_file) {
                if Path::new(&f).file_name().and_then(|n| n.to_str()) == Some(header_basename) {
                    return true;
                }
            }
        }
    }
    false
}

fn sdk_path() -> Option<String> {
    let out = Command::new("xcrun").arg("--show-sdk-path").output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// Header filenames `#import`ed by the framework umbrella (e.g.
/// `#import <NaturalLanguage/NLTokenizer.h>` -> `NLTokenizer.h`), umbrella excluded.
fn public_headers(umbrella: &Path, name: &str) -> Vec<String> {
    let Ok(src) = std::fs::read_to_string(umbrella) else {
        return Vec::new();
    };
    let needle = format!("#import <{name}/");
    let mut hs = Vec::new();
    for line in src.lines() {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix(&needle) {
            if let Some(h) = rest.strip_suffix('>') {
                let h = h.trim();
                if h.ends_with(".h") && h != format!("{name}.h") && !hs.contains(&h.to_string()) {
                    hs.push(h.to_string());
                }
            }
        }
    }
    hs
}

fn list_headers(dir: &Path, name: &str) -> Vec<String> {
    let umbrella = format!("{name}.h");
    let mut hs: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|f| f.ends_with(".h") && *f != umbrella)
        .collect();
    hs.sort();
    hs
}

fn clang_ast(header: &Path, sdk: &str) -> Option<serde_json::Value> {
    let out = Command::new("clang")
        .args(["-Xclang", "-ast-dump=json", "-fsyntax-only", "-x", "objective-c", "-isysroot"])
        .arg(sdk)
        .arg(header)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    serde_json::from_slice(&out.stdout).ok()
}

/// `NLTokenizer.h` with prefix `NL` -> `tokenizer`; `NLLanguageRecognizer.h` ->
/// `language_recognizer`.
fn module_name(header: &str, prefix: &str) -> String {
    let base = header.strip_suffix(".h").unwrap_or(header);
    let base = base.strip_prefix(prefix).unwrap_or(base);
    snake(base)
}

fn snake(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_uppercase() {
            let prev_lower = i > 0 && chars[i - 1].is_lowercase();
            let acronym_end =
                i > 0 && chars[i - 1].is_uppercase() && i + 1 < chars.len() && chars[i + 1].is_lowercase();
            if i > 0 && (prev_lower || acronym_end) {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// Top-level `struct` / `enum` names a generated module exposes.
fn exported_types(module_src: &str) -> Vec<String> {
    let mut ts = Vec::new();
    for line in module_src.lines() {
        for kw in ["struct ", "enum "] {
            if let Some(rest) = line.strip_prefix(kw) {
                let name: String = rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() && !ts.contains(&name) {
                    ts.push(name);
                }
            }
        }
    }
    ts
}

fn build_umbrella(name: &str, modules: &[(String, Vec<String>)]) -> String {
    let mut s = format!(
        "// {} — C+ binding for Apple's {} framework.\n// Auto-generated by cpc-bindgen (--framework). DO NOT EDIT.\n\n",
        name.to_lowercase(),
        name
    );
    for (m, _) in modules {
        s.push_str(&format!("import \"./{m}\" as {m};\n"));
    }
    s.push('\n');
    let mut seen: Vec<String> = Vec::new();
    for (m, types) in modules {
        for t in types {
            if seen.contains(t) {
                continue; // first module to export a name wins
            }
            seen.push(t.clone());
            s.push_str(&format!("type {t} = {m}::{t};\n"));
        }
    }
    s
}

fn cplus_toml(
    name: &str,
    pkg: &str,
    prefix: &str,
    overrides_used: bool,
    sdk_version: &str,
    n_headers: usize,
    any_objc: bool,
) -> String {
    // A pure-C framework (Accelerate) needs no `objc` runtime dependency.
    let deps = if any_objc {
        "stdlib = \"*\"\nobjc   = \"*\""
    } else {
        "stdlib = \"*\""
    };
    // Reproduce line: the exact command that regenerates this package.
    let mut repro = format!("cpc-bindgen --framework {name}");
    if !prefix.is_empty() {
        repro.push_str(&format!(" --prefix {prefix}"));
    }
    if overrides_used {
        repro.push_str(" --overrides overrides.json");
    }
    format!(
        "# Auto-generated by cpc-bindgen --framework. Regenerate; do not hand-edit src/.\n\
         #\n\
         # framework = \"{name}\"\n\
         # sdk       = \"{sdk_version}\"\n\
         # generator = \"cpc-bindgen {ver}\"\n\
         # headers   = {n_headers}\n\
         # reproduce = \"{repro}\"\n\
         \n\
         [package]\nname    = \"{pkg}\"\nversion = \"0.0.0\"\nedition = \"2026\"\n\n\
         [dependencies]\n{deps}\n\n\
         [link]\nframeworks = [\"{name}\", \"Foundation\"]\nlibs       = [\"objc\"]\n",
        ver = env!("CARGO_PKG_VERSION"),
    )
}

fn sdk_version() -> String {
    Command::new("xcrun")
        .args(["--show-sdk-version"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn overrides_stub(name: &str) -> String {
    format!(
        "{{\n  \"_comment\": \"cpc-bindgen naming overrides for {name}. Add `types` / `methods` / `skip` entries to refine the mechanical snake_case names.\",\n  \"methods\": {{}}\n}}\n"
    )
}
