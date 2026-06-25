// cpc-bindgen framework mode — generate a whole C+ package from an Apple system
// framework. Discovers the framework's public headers from its umbrella header,
// emits one binding module per header (via the ObjC front-end), and writes the
// package skeleton the single-header mode leaves to the author: the umbrella
// module (imports + type re-exports), a `Cplus.toml` populated from the
// framework metadata, and a starter `overrides.json`.

use crate::objc::ObjcEmitter;
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
    let headers_dir = fw_dir.join("Headers");
    if !headers_dir.is_dir() {
        eprintln!(
            "cpc-bindgen: framework `{name}` not found at {}",
            fw_dir.display()
        );
        return 1;
    }

    // Public headers come from the framework umbrella header's `#import`s; if it
    // has none (or is absent), fall back to every header except the umbrella.
    let umbrella = headers_dir.join(format!("{name}.h"));
    let mut headers = public_headers(&umbrella, name);
    if headers.is_empty() {
        headers = list_headers(&headers_dir, name);
    }
    if headers.is_empty() {
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

    // One module per header. `modules` keeps (module name, its exported types)
    // so the umbrella can re-export them.
    let mut modules: Vec<(String, Vec<String>)> = Vec::new();
    for h in &headers {
        let header_path = headers_dir.join(h);
        let ast = match clang_ast(&header_path, &sdk) {
            Some(a) => a,
            None => {
                eprintln!("cpc-bindgen:   {h}: clang failed, skipping");
                continue;
            }
        };
        let emitter = ObjcEmitter::new(&header_path.to_string_lossy(), prefix, overrides.clone());
        let text = emitter.run(&ast);
        let module = module_name(h, prefix);
        let types = exported_types(&text);
        if let Err(e) = std::fs::write(src.join(format!("{module}.cplus")), &text) {
            eprintln!("cpc-bindgen:   {h}: write failed: {e}");
            continue;
        }
        eprintln!("  {h} -> src/{module}.cplus ({} types)", types.len());
        modules.push((module, types));
    }

    std::fs::write(src.join(format!("{pkg}.cplus")), build_umbrella(name, &modules)).ok();
    std::fs::write(
        out.join("Cplus.toml"),
        cplus_toml(name, &pkg, prefix, overrides_path.is_some(), &sdk_version(), modules.len()),
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
) -> String {
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
         [dependencies]\nstdlib = \"*\"\nobjc   = \"*\"\n\n\
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
