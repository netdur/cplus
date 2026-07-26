//! Generate a package's `lib/include/` headers from its `src/`.
//!
//! A binary package ships signatures, not bodies: the implementation lives in
//! the bundled archive. This produces that header form.
//!
//! **Approach: byte-span surgery on the original source, not an AST printer.**
//! Printing an AST back to source means re-implementing the syntax of every
//! type, parameter mode, generic bound and attribute, and any gap between
//! printer and parser silently corrupts a header. Replacing each body's byte
//! range with `;` instead leaves everything else — imports, `struct` layouts,
//! `const`s, doc comments, ordinary comments — exactly as the author wrote it.
//! A header is a thing humans read, so preserving comments matters, and a
//! `struct` whose layout must match the archive byte for byte is safest copied
//! verbatim rather than round-tripped.
//!
//! ## Mixed mode
//!
//! Generics cannot cross a precompiled boundary: `Vec[T]` has no object code
//! until a consumer picks a `T`, so its body must travel with the package. A
//! module that declares any generic is therefore emitted **verbatim** — the
//! same choice C++ makes when a template lives in a header while the
//! non-template code sits in the `.a`. Concrete modules get their bodies
//! stripped.
//!
//! That is a per-module decision, which is what `plan-0.0.2.md` means by
//! "mixed source + artifact (per-module) — selectable by what the author
//! commits".

use crate::ast::{ItemKind, Program};
use crate::lexer::tokenize;
use crate::parser::parse;

/// What `generate` decided to do with a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderKind {
    /// Bodies replaced with `;` — the implementation is in the archive.
    Stripped,
    /// Copied verbatim: the module declares generics, whose bodies must be
    /// available to the consumer for monomorphization.
    VerbatimGeneric,
}

#[derive(Debug)]
pub enum HeaderError {
    Lex(String),
    Parse(String),
}

impl std::fmt::Display for HeaderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HeaderError::Lex(m) => write!(f, "lex: {m}"),
            HeaderError::Parse(m) => write!(f, "parse: {m}"),
        }
    }
}

/// Does this module declare anything generic? If so its bodies must ship.
fn declares_generics(p: &Program) -> bool {
    p.items.iter().any(|item| match &item.kind {
        ItemKind::Function(f) => !f.generic_params.is_empty(),
        ItemKind::Struct(s) => !s.generic_params.is_empty(),
        ItemKind::Enum(e) => !e.generic_params.is_empty(),
        ItemKind::Impl(b) => {
            !b.target_generic_params.is_empty()
                || b.methods.iter().any(|m| !m.generic_params.is_empty())
        }
        _ => false,
    })
}

/// Byte ranges of every function/method body in the module, as `(start, end)`
/// half-open over the source.
fn body_spans(p: &Program) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    for item in &p.items {
        match &item.kind {
            ItemKind::Function(f) => {
                // `extern fn` bodies are synthesized empty blocks with no
                // source extent; skip them, and skip anything already
                // body-less.
                if !f.is_extern && !f.is_declaration {
                    out.push((f.body.span.start as usize, f.body.span.end as usize));
                }
            }
            ItemKind::Impl(b) => {
                for m in &b.methods {
                    if !m.is_declaration {
                        out.push((m.body.span.start as usize, m.body.span.end as usize));
                    }
                }
            }
            _ => {}
        }
    }
    out.sort_unstable();
    out
}

/// Produce the header form of one module.
///
/// Returns the header text and which rule was applied.
pub fn generate(src: &str) -> Result<(String, HeaderKind), HeaderError> {
    let toks = tokenize(src).map_err(|e| HeaderError::Lex(format!("{e:?}")))?;
    let program = parse(toks).map_err(|e| HeaderError::Parse(format!("{e:?}")))?;

    if declares_generics(&program) {
        return Ok((src.to_string(), HeaderKind::VerbatimGeneric));
    }

    let spans = body_spans(&program);
    if spans.is_empty() {
        return Ok((src.to_string(), HeaderKind::Stripped));
    }

    // Walk forward, copying everything outside a body and emitting `;` for
    // each body. Bodies never nest at item level, so a single pass suffices.
    let bytes = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut cursor = 0usize;
    for (start, end) in spans {
        if start < cursor || end > bytes.len() || start >= end {
            // A span that doesn't line up with the source (synthesized, or
            // overlapping) — skip it rather than corrupt the output.
            continue;
        }
        out.push_str(&src[cursor..start]);
        out.push(';');
        cursor = end;
    }
    out.push_str(&src[cursor..]);
    Ok((out, HeaderKind::Stripped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr(src: &str) -> (String, HeaderKind) {
        generate(src).expect("should generate")
    }

    #[test]
    fn strips_a_free_function_body() {
        let (out, kind) = hdr("fn answer(seed: i32) -> i32 { return seed + 22; }");
        assert_eq!(kind, HeaderKind::Stripped);
        assert_eq!(out, "fn answer(seed: i32) -> i32 ;");
    }

    #[test]
    fn the_generated_header_parses_back() {
        // The whole point: a header must be valid C+ that a consumer compiles.
        let (out, _) = hdr("fn a(x: i32) -> i32 { return x; }\nfn b() -> bool { return true; }");
        let toks = tokenize(&out).expect("header should lex");
        parse(toks).expect("header should parse");
    }

    #[test]
    fn strips_method_bodies_and_keeps_the_struct_verbatim() {
        let (out, _) = hdr(
            "struct Widget { id: i32, name: i32 }\n\
             impl Widget {\n\
             \x20   fn value(this) -> i32 { return this.id * 3; }\n\
             }",
        );
        // Layout must survive byte for byte — the archive was compiled against it.
        assert!(out.contains("struct Widget { id: i32, name: i32 }"), "{out}");
        assert!(out.contains("fn value(this) -> i32 ;"), "{out}");
        assert!(!out.contains("this.id * 3"), "body leaked: {out}");
    }

    #[test]
    fn preserves_comments_and_imports() {
        let src = "// module doc\n\
                   import \"./option\" as option;\n\
                   \n\
                   /// what it does\n\
                   fn find(h: i32) -> i32 { return h; }";
        let (out, _) = hdr(src);
        assert!(out.contains("// module doc"), "{out}");
        assert!(out.contains("import \"./option\" as option;"), "{out}");
        assert!(out.contains("/// what it does"), "{out}");
    }

    // Generics cannot cross a precompiled boundary, so their bodies must ship.
    #[test]
    fn a_generic_module_is_emitted_verbatim() {
        let src = "struct Vec[T] { n: i32 }\nimpl Vec[T] { fn count(this) -> i32 { return this.n; } }";
        let (out, kind) = hdr(src);
        assert_eq!(kind, HeaderKind::VerbatimGeneric);
        assert_eq!(out, src, "generic module must be copied unchanged");
        assert!(out.contains("return this.n;"), "generic body must survive");
    }

    #[test]
    fn a_generic_free_fn_also_forces_verbatim() {
        let (_, kind) = hdr("fn id[T](v: T) -> T { return v; }");
        assert_eq!(kind, HeaderKind::VerbatimGeneric);
    }

    #[test]
    fn extern_declarations_are_left_alone() {
        // They have no source body to strip, and no extent to splice.
        let (out, _) = hdr("extern fn malloc(n: usize) -> *u8;\nfn f() -> i32 { return 1; }");
        assert!(out.contains("extern fn malloc(n: usize) -> *u8;"), "{out}");
        assert!(out.contains("fn f() -> i32 ;"), "{out}");
    }

    #[test]
    fn an_already_stripped_header_is_idempotent() {
        let once = hdr("fn a(x: i32) -> i32 { return x; }").0;
        let twice = hdr(&once).0;
        assert_eq!(once, twice, "generating a header from a header must not drift");
    }

    #[test]
    fn a_module_with_no_functions_is_unchanged() {
        let src = "struct Point { x: i32, y: i32 }";
        assert_eq!(hdr(src).0, src);
    }
}
