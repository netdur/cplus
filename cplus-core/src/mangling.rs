//! issue-02: the one implementation of the `name__T1__T2` mangling grammar.
//!
//! A monomorphized symbol's name IS its identity: sema mangles a name, and
//! codegen finds the definition by rebuilding — or by parsing — the same
//! string. The grammar used to be written five times as a printer and twice as
//! a parser, kept in step by comments ("the shapes must match `mangle_ty`
//! exactly"). They did not stay in step, and every divergence was a lookup
//! miss: the missing `Vec[*T]` and `Vec[fn]` parser arms were each a recorded
//! miscompile, and three more divergences were still live when this module was
//! written (see the tests at the bottom).
//!
//! ## The grammar
//!
//! ```text
//! symbol  := base ("__" type)*          // `join`
//! type    := primitive                  // i8 u16 f32 bool unit str string ERR
//!          | "ptr_" type                // *T
//!          | "slice_" type              // T[]
//!          | "arr" N "_" type           // [N]T
//!          | "fn" ("_" mode? type)* ("_ret_" type)?   // mode := "take_" | "ref_"
//!          | type "x" N                 // SIMD vector
//!          | "mask" type "x" N          // SIMD mask
//!          | "Param_" ident             // an unsubstituted type parameter
//!          | nominal                    // a struct / enum name, possibly qualified
//! ```
//!
//! A unit RETURN is omitted rather than spelled (`fn_i32`, not
//! `fn_i32_ret_unit`) — a unit-returning fn pointer is the common case and the
//! shorter spelling is the one every Ty-side printer has always produced.
//!
//! ## Why this is parseable at all
//!
//! `_` and `__` are separators AND legal identifier characters, so the grammar
//! is only unambiguous because sema reserves interior `__` in user identifiers
//! (E0917). That invariant is what lets a nominal name be recovered by
//! longest-prefix match at a `_` boundary; `mangled_name_matches` and the
//! struct-prefix search in codegen both lean on it. Weaken E0917 and this
//! module stops working.
//!
//! ## Id universes stay local
//!
//! Sema `StructId`s and codegen `StructId`s are different numbering spaces and
//! must stay that way. So the GRAMMAR lives here and every NOMINAL lookup is a
//! callback the caller supplies from its own tables — `render` takes "name this
//! nominal type", `take` and `from_suffix` take "resolve this name".
//!
//! ## Direction (not implemented here)
//!
//! Parsing a name back into a `Ty` is a decode of information codegen could
//! have been handed directly. The end state is that monomorphize records
//! instance-name → argument-`Ty` side tables and codegen never demangles;
//! `TrampolineSpec::Spawn { o: Ty }` already has that shape. Until then, the
//! printer and the parser at least share one file and one test.

use crate::ast::{Type, TypeKind};
use crate::sema::Ty;

/// issue-06 step 6: the prefix the compiler reserves for its own runtime ABI.
///
/// `#reactor_get_state` reaches `__cplus_reactor_get_state`; codegen emits
/// thread trampolines and bound-method bridges under the same prefix; the
/// resolver keeps these names global rather than module-scoping them, because
/// emitted code calls them from anywhere. Three passes agreeing on one string
/// is a convention, and a convention spelled out in three places is one that
/// drifts — so it is spelled once, here, beside the rest of the naming
/// grammar the compiler owns.
///
/// A source declaration under this prefix must carry `#[runtime_abi]`
/// (E0919): it is claiming to name a symbol the compiler generates, and that
/// is a claim to make out loud rather than by spelling.
pub const RUNTIME_ABI_PREFIX: &str = "__cplus_";

/// Where a rendered type goes. `String` builds the name; `Counter` measures it
/// without materializing — the instantiation-size guard needs the length of a
/// name it is about to refuse to build.
trait Sink {
    fn put(&mut self, s: &str);
}

impl Sink for String {
    fn put(&mut self, s: &str) {
        self.push_str(s);
    }
}

#[derive(Default)]
struct Counter(usize);

impl Sink for Counter {
    fn put(&mut self, s: &str) {
        self.0 += s.len();
    }
}

/// Render one type in the mangling grammar. `nominal` names a `Ty::Struct` /
/// `Ty::Enum` from the caller's own type table.
pub fn render(ty: &Ty, nominal: &dyn Fn(&Ty) -> String) -> String {
    let mut out = String::new();
    write_ty(ty, nominal, &mut out);
    out
}

/// The length `render` would produce, without building the string.
pub fn render_len(ty: &Ty, nominal: &dyn Fn(&Ty) -> String) -> usize {
    let mut out = Counter::default();
    write_ty(ty, nominal, &mut out);
    out.0
}

fn write_ty(ty: &Ty, nominal: &dyn Fn(&Ty) -> String, out: &mut dyn Sink) {
    match ty {
        Ty::I8 => out.put("i8"),
        Ty::I16 => out.put("i16"),
        Ty::I32 => out.put("i32"),
        Ty::I64 => out.put("i64"),
        Ty::U8 => out.put("u8"),
        Ty::U16 => out.put("u16"),
        Ty::U32 => out.put("u32"),
        Ty::U64 => out.put("u64"),
        Ty::Isize => out.put("isize"),
        Ty::Usize => out.put("usize"),
        Ty::F16 => out.put("f16"),
        Ty::F32 => out.put("f32"),
        Ty::F64 => out.put("f64"),
        Ty::Bool => out.put("bool"),
        Ty::Unit => out.put("unit"),
        Ty::Str => out.put("str"),
        Ty::String => out.put("string"),
        Ty::Error => out.put("ERR"),
        Ty::Slice(inner) => {
            out.put("slice_");
            write_ty(inner, nominal, out);
        }
        Ty::RawPtr(inner) => {
            out.put("ptr_");
            write_ty(inner, nominal, out);
        }
        Ty::Array(elem, n) => {
            out.put(&format!("arr{n}_"));
            write_ty(elem, nominal, out);
        }
        Ty::FnPtr {
            params,
            param_takes,
            param_refs,
            return_type,
        } => {
            out.put("fn");
            for (i, p) in params.iter().enumerate() {
                out.put("_");
                if param_takes.get(i).copied().unwrap_or(false) {
                    out.put("take_");
                } else if param_refs.get(i).copied().unwrap_or(false) {
                    out.put("ref_");
                }
                write_ty(p, nominal, out);
            }
            // A unit return is spelled by omission — see the module header.
            if !matches!(**return_type, Ty::Unit) {
                out.put("_ret_");
                write_ty(return_type, nominal, out);
            }
        }
        Ty::Simd { elem, lanes } => {
            write_ty(elem, nominal, out);
            out.put(&format!("x{lanes}"));
        }
        Ty::Mask { elem, lanes } => {
            out.put("mask");
            write_ty(elem, nominal, out);
            out.put(&format!("x{lanes}"));
        }
        Ty::Param(name) => out.put(&format!("Param_{name}")),
        // v0.0.27 distinct alias: mangles by its own (unique, resolver-
        // qualified) name, NOT the base — `Vec[UserId]` and `Vec[i64]` must
        // not collide on one mangled symbol while sema still tells them
        // apart. Layout is identical either way.
        Ty::Distinct { name, .. } => out.put(name),
        Ty::Struct(_) | Ty::Enum(_) => out.put(&nominal(ty)),
    }
}

/// Render an AST `Type` in the same grammar. Used by monomorphize, which keys
/// its instantiation lookups off substituted AST nodes rather than `Ty`s (it
/// has no access to sema's id table — see the module header).
///
/// Two shapes cannot round-trip to `render`, by construction rather than by
/// drift:
///
///   - a bare `Path("T")` left over from an unsubstituted type parameter
///     renders as `T`, where the `Ty` side renders `Param_T`. The AST cannot
///     tell that name from a struct called `T`. A lookup keyed on the `Ty`
///     spelling therefore misses — which is correct: an instantiation with an
///     unsubstituted parameter is not a concrete instantiation.
///   - `TypeKind::Tuple` renders structurally (`tuple2_i32_i32`) because the
///     synthesized tuple struct's name (`__tuple_i32_i32`) is only known once
///     the instantiation exists. Post-substitution a tuple is a `Path` to that
///     struct, so this arm is a fallback.
pub fn render_ast(t: &Type) -> String {
    match &t.kind {
        // v0.0.12 G-026: `()` source-spelled unit type. The `Ty` side renders
        // `Ty::Unit` as "unit"; the AST side has to match so a lookup keyed on
        // one form hits an entry built from the other.
        TypeKind::Path(name) if name == "()" => "unit".to_string(),
        TypeKind::Path(name) => name.clone(),
        TypeKind::Array { elem, len, .. } => format!("arr{}_{}", len, render_ast(elem)),
        TypeKind::Borrowed { inner, .. } => render_ast(inner),
        TypeKind::RawPtr(inner) => format!("ptr_{}", render_ast(inner)),
        TypeKind::Slice(inner) => format!("slice_{}", render_ast(inner)),
        TypeKind::FnPtr {
            params,
            param_takes,
            param_refs,
            return_type,
        } => {
            let mut s = String::from("fn");
            for (i, p) in params.iter().enumerate() {
                s.push('_');
                if param_takes.get(i).copied().unwrap_or(false) {
                    s.push_str("take_");
                } else if param_refs.get(i).copied().unwrap_or(false) {
                    s.push_str("ref_");
                }
                s.push_str(&render_ast(p));
            }
            // The `Ty` printer omits a unit return; this one used to spell it,
            // so a user-written `fn(i32) -> ()` rendered `fn_i32_ret_unit` and
            // missed every key built from the `Ty` side.
            match return_type {
                Some(rt) if !is_ast_unit(rt) => {
                    s.push_str("_ret_");
                    s.push_str(&render_ast(rt));
                }
                _ => {}
            }
            s
        }
        TypeKind::Generic { name, args } => {
            // After `subst_type_ast` recursion this should be unreachable
            // (the Generic→Path rewrite consumes Generic nodes). If it shows
            // up, render best-effort so an unresolved key falls through to the
            // unchanged Generic branch in `subst_type_ast`.
            join(
                name,
                &args.iter().map(render_ast).collect::<Vec<_>>(),
            )
        }
        TypeKind::Tuple(elems) => {
            let mut s = format!("tuple{}", elems.len());
            for e in elems {
                s.push('_');
                s.push_str(&render_ast(e));
            }
            s
        }
    }
}

fn is_ast_unit(t: &Type) -> bool {
    matches!(&t.kind, TypeKind::Path(n) if n == "()" || n == "unit")
}

/// Compose a mangled symbol: `base__arg__arg`. The separator is `__`, which
/// E0917 keeps out of user identifiers.
pub fn join(base: &str, args: &[String]) -> String {
    let mut s = String::with_capacity(base.len() + args.iter().map(|a| a.len() + 2).sum::<usize>());
    s.push_str(base);
    for a in args {
        s.push_str("__");
        s.push_str(a);
    }
    s
}

/// The length `join` would produce.
pub fn join_len(base: &str, arg_lens: impl Iterator<Item = usize>) -> usize {
    base.len() + arg_lens.map(|n| n + 2).sum::<usize>()
}

/// Does `rest` end a mangled type token?
fn boundary(rest: &str) -> bool {
    rest.is_empty() || rest.starts_with('_')
}

/// The primitive keywords, longest-first where one is a prefix of another
/// (`string` before `str`).
const PRIMITIVES: &[(&str, Ty)] = &[
    ("isize", Ty::Isize),
    ("usize", Ty::Usize),
    ("i8", Ty::I8),
    ("i16", Ty::I16),
    ("i32", Ty::I32),
    ("i64", Ty::I64),
    ("u8", Ty::U8),
    ("u16", Ty::U16),
    ("u32", Ty::U32),
    ("u64", Ty::U64),
    ("f16", Ty::F16),
    ("f32", Ty::F32),
    ("f64", Ty::F64),
    ("bool", Ty::Bool),
    ("unit", Ty::Unit),
    ("string", Ty::String),
    ("str", Ty::Str),
    ("ERR", Ty::Error),
];

/// Consume exactly ONE mangled type from the FRONT of `s`, returning the
/// parsed `Ty` and the unconsumed remainder. Tokenizing, so it can walk a
/// fn-pointer's `_`-separated parameter list.
///
/// `nominal` resolves a struct/enum name at the front of the remaining input
/// from the caller's table, returning the type and how many bytes it consumed.
pub fn take<'a>(
    s: &'a str,
    nominal: &dyn Fn(&str) -> Option<(Ty, usize)>,
) -> Option<(Ty, &'a str)> {
    for (kw, ty) in PRIMITIVES {
        if let Some(r) = s.strip_prefix(kw).filter(|r| boundary(r)) {
            return Some((ty.clone(), r));
        }
    }
    if let Some(rest) = s.strip_prefix("ptr_") {
        let (inner, r) = take(rest, nominal)?;
        return Some((Ty::RawPtr(Box::new(inner)), r));
    }
    if let Some(rest) = s.strip_prefix("slice_") {
        let (inner, r) = take(rest, nominal)?;
        return Some((Ty::Slice(Box::new(inner)), r));
    }
    if let Some(rest) = s.strip_prefix("arr") {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            if let Some(elem_s) = rest[digits.len()..].strip_prefix('_') {
                if let Ok(n) = digits.parse::<u32>() {
                    let (elem, r) = take(elem_s, nominal)?;
                    return Some((Ty::Array(Box::new(elem), n), r));
                }
            }
        }
    }
    // fn-pointer: `fn` then `_[take_|ref_]<param>` repeated, then an optional
    // `_ret_<ret>`. Only commit when `fn` ends at a boundary (a struct named
    // `fnord` falls through to the nominal match below).
    if let Some(after_fn) = s.strip_prefix("fn") {
        if boundary(after_fn) {
            let mut params: Vec<Ty> = Vec::new();
            let mut param_takes: Vec<bool> = Vec::new();
            let mut param_refs: Vec<bool> = Vec::new();
            let mut return_type = Ty::Unit;
            let mut rest = after_fn;
            loop {
                let Some(after_us) = rest.strip_prefix('_') else {
                    break;
                };
                if let Some(ret_s) = after_us.strip_prefix("ret_") {
                    let (rty, r) = take(ret_s, nominal)?;
                    return_type = rty;
                    rest = r;
                    break;
                }
                let (is_take, is_ref, tys) = if let Some(r) = after_us.strip_prefix("take_") {
                    (true, false, r)
                } else if let Some(r) = after_us.strip_prefix("ref_") {
                    (false, true, r)
                } else {
                    (false, false, after_us)
                };
                let (pty, r) = take(tys, nominal)?;
                params.push(pty);
                param_takes.push(is_take);
                param_refs.push(is_ref);
                rest = r;
            }
            return Some((
                Ty::FnPtr {
                    params,
                    param_takes,
                    param_refs,
                    return_type: Box::new(return_type),
                },
                rest,
            ));
        }
    }
    if let Some(rest) = s.strip_prefix("Param_") {
        let name: String = rest.chars().take_while(|&c| c != '_').collect();
        let consumed = name.len();
        return Some((Ty::Param(name), &rest[consumed..]));
    }
    // A nominal name is matched BEFORE the vector rule, because the two forms
    // genuinely collide: `i8x2` is a legal struct name, and if the user
    // declared that struct, that is what the name means. The prefix rules
    // above still win over a nominal name (a struct called `ptr` does not
    // capture `ptr_i32`), which is the behavior every earlier parser had.
    if let Some((ty, len)) = nominal(s) {
        return Some((ty, &s[len..]));
    }
    if let Some(rest) = s.strip_prefix("mask") {
        if let Some((elem, lanes, r)) = take_vector(rest) {
            return Some((
                Ty::Mask {
                    elem: Box::new(elem),
                    lanes,
                },
                r,
            ));
        }
    }
    if let Some((elem, lanes, r)) = take_vector(s) {
        return Some((
            Ty::Simd {
                elem: Box::new(elem),
                lanes,
            },
            r,
        ));
    }
    None
}

/// `<elem>x<lanes>` at the front of `s`, where `elem` is a primitive keyword.
/// Only primitives can be lanes, so this never has to recurse.
fn take_vector(s: &str) -> Option<(Ty, u32, &str)> {
    for (kw, ty) in PRIMITIVES {
        let Some(rest) = s.strip_prefix(kw) else {
            continue;
        };
        let Some(after_x) = rest.strip_prefix('x') else {
            continue;
        };
        let digits: String = after_x.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            continue;
        }
        let r = &after_x[digits.len()..];
        if !boundary(r) {
            continue;
        }
        let lanes = digits.parse::<u32>().ok()?;
        return Some((ty.clone(), lanes, r));
    }
    None
}

/// Decode a WHOLE mangled type (the suffix of an instantiated name), rather
/// than one token from the front. `exact` resolves a name that equals the
/// whole remaining string; `fallback` is the caller's looser match (a
/// qualified tail, say) and is consulted only after every grammar rule and the
/// exact lookup have failed. Returns `Ty::Error` when nothing matches.
///
/// The two lookups are separate parameters because their ORDER relative to the
/// grammar matters: an exact nominal match must beat the SIMD rule, or a user
/// struct named `i8x2` decodes as a two-lane vector.
pub fn from_suffix(
    suffix: &str,
    nominal: &dyn Fn(&str) -> Option<(Ty, usize)>,
    fallback: &dyn Fn(&str) -> Option<Ty>,
) -> Ty {
    // The whole string naming something the caller knows exactly.
    let exact = |s: &str| nominal(s).and_then(|(t, l)| (l == s.len()).then_some(t));
    // Fn-pointer suffixes are `_`-tokenized, so the whole-string rules below
    // cannot decode them; parse them structurally.
    if suffix == "fn" || suffix.starts_with("fn_") {
        if let Some((ty, rest)) = take(suffix, nominal) {
            if rest.is_empty() {
                return ty;
            }
        }
    }
    for (kw, ty) in PRIMITIVES {
        if suffix == *kw {
            return ty.clone();
        }
    }
    if let Some(inner) = suffix.strip_prefix("ptr_") {
        let inner_ty = from_suffix(inner, nominal, fallback);
        if inner_ty == Ty::Error {
            return Ty::Error;
        }
        return Ty::RawPtr(Box::new(inner_ty));
    }
    if let Some(inner) = suffix.strip_prefix("slice_") {
        let inner_ty = from_suffix(inner, nominal, fallback);
        if inner_ty == Ty::Error {
            return Ty::Error;
        }
        return Ty::Slice(Box::new(inner_ty));
    }
    if let Some(rest) = suffix.strip_prefix("arr") {
        if let Some(idx) = rest.find('_') {
            if let Ok(n) = rest[..idx].parse::<u32>() {
                let elem_ty = from_suffix(&rest[idx + 1..], nominal, fallback);
                if elem_ty == Ty::Error {
                    return Ty::Error;
                }
                return Ty::Array(Box::new(elem_ty), n);
            }
        }
    }
    if let Some(inner) = suffix.strip_prefix("Param_") {
        return Ty::Param(inner.to_string());
    }
    // A name the caller knows EXACTLY wins over the vector rule: `i8x2` is a
    // legal struct name, and a struct by that name is what the user wrote.
    if let Some(ty) = exact(suffix) {
        return ty;
    }
    if let Some(rest) = suffix.strip_prefix("mask") {
        if let Some((elem, lanes, r)) = take_vector(rest) {
            if r.is_empty() {
                return Ty::Mask {
                    elem: Box::new(elem),
                    lanes,
                };
            }
        }
    }
    if let Some((elem, lanes, r)) = take_vector(suffix) {
        if r.is_empty() {
            return Ty::Simd {
                elem: Box::new(elem),
                lanes,
            };
        }
    }
    fallback(suffix).unwrap_or(Ty::Error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema::{EnumId, StructId};

    /// A stand-in type table: two nominal types, one struct and one enum, plus
    /// a struct whose NAME collides with the vector grammar.
    fn nominal_name(ty: &Ty) -> String {
        match ty {
            Ty::Struct(id) if id.0 == 0 => "Point".to_string(),
            Ty::Struct(id) if id.0 == 1 => "i8x2".to_string(),
            Ty::Struct(id) => format!("S{}", id.0),
            Ty::Enum(id) if id.0 == 0 => "pkg.mod.Color".to_string(),
            Ty::Enum(id) => format!("E{}", id.0),
            _ => unreachable!("nominal called on a non-nominal type"),
        }
    }

    fn nominal_take(s: &str) -> Option<(Ty, usize)> {
        let names: [(&str, Ty); 3] = [
            ("Point", Ty::Struct(StructId(0))),
            ("i8x2", Ty::Struct(StructId(1))),
            ("pkg.mod.Color", Ty::Enum(EnumId(0))),
        ];
        let mut best: Option<(Ty, usize)> = None;
        for (n, t) in names {
            if let Some(rest) = s.strip_prefix(n) {
                if boundary(rest) && best.as_ref().map_or(true, |(_, l)| n.len() > *l) {
                    best = Some((t, n.len()));
                }
            }
        }
        best
    }

    fn corpus() -> Vec<Ty> {
        let leaves = vec![
            Ty::I8,
            Ty::I16,
            Ty::I32,
            Ty::I64,
            Ty::U8,
            Ty::U16,
            Ty::U32,
            Ty::U64,
            Ty::Isize,
            Ty::Usize,
            Ty::F16,
            Ty::F32,
            Ty::F64,
            Ty::Bool,
            Ty::Unit,
            Ty::Str,
            Ty::String,
            Ty::Error,
            Ty::Struct(StructId(0)),
            Ty::Struct(StructId(1)),
            Ty::Enum(EnumId(0)),
            Ty::Param("T".to_string()),
        ];
        let mut out = leaves.clone();
        // `ERR` is excluded from COMPOSITE positions on purpose: `Ty::Error`
        // doubles as the whole-string parser's "did not parse" answer, so
        // `ptr_ERR` decodes as a failure rather than as a pointer-to-error.
        // No real program mangles one — an error type never reaches an
        // instantiation key.
        for l in leaves.iter().filter(|l| **l != Ty::Error) {
            out.push(Ty::RawPtr(Box::new(l.clone())));
            out.push(Ty::Slice(Box::new(l.clone())));
            out.push(Ty::Array(Box::new(l.clone()), 7));
            out.push(Ty::FnPtr {
                params: vec![l.clone()],
                param_takes: vec![true],
                param_refs: vec![false],
                return_type: Box::new(Ty::Unit),
            });
            out.push(Ty::FnPtr {
                params: vec![l.clone(), Ty::I32],
                param_takes: vec![false, false],
                param_refs: vec![true, false],
                return_type: Box::new(l.clone()),
            });
            out.push(Ty::RawPtr(Box::new(Ty::Slice(Box::new(l.clone())))));
        }
        for elem in [Ty::I8, Ty::U16, Ty::F32, Ty::F64] {
            for lanes in [2u32, 4, 16] {
                out.push(Ty::Simd {
                    elem: Box::new(elem.clone()),
                    lanes,
                });
                out.push(Ty::Mask {
                    elem: Box::new(elem.clone()),
                    lanes,
                });
            }
        }
        out.push(Ty::FnPtr {
            params: vec![],
            param_takes: vec![],
            param_refs: vec![],
            return_type: Box::new(Ty::Unit),
        });
        out
    }

    /// The property the whole module exists for: what one side prints, the
    /// other side reads back as the same type, and the length predictor agrees
    /// with the printer.
    #[test]
    fn every_type_round_trips_through_the_grammar() {
        for ty in corpus() {
            let s = render(&ty, &nominal_name);
            // The one ambiguity the grammar cannot resolve: a user may declare
            // a struct named exactly like a vector spelling (`i8x2`). Both
            // parsers answer "the declared type", so a structural type whose
            // rendering a nominal name shadows does not round-trip — by
            // decision, not by drift. `a_struct_named_like_a_vector_wins_over_
            // the_vector_rule` pins which way it goes.
            let shadowed = matches!(nominal_take(&s), Some((_, l)) if l == s.len())
                && !matches!(ty, Ty::Struct(_) | Ty::Enum(_));
            if shadowed {
                continue;
            }
            assert_eq!(
                render_len(&ty, &nominal_name),
                s.len(),
                "render_len disagrees for {ty:?} ({s})"
            );
            let parsed = take(&s, &nominal_take);
            let Some((back, rest)) = parsed else {
                panic!("take failed for {ty:?} (rendered `{s}`)");
            };
            assert!(rest.is_empty(), "take left `{rest}` for {ty:?} (`{s}`)");
            assert_eq!(back, ty, "round-trip changed the type (`{s}`)");
            let whole = from_suffix(&s, &nominal_take, &|_| None);
            assert_eq!(whole, ty, "from_suffix disagrees with take (`{s}`)");
        }
    }

    /// Divergence 1 (issue-02): the AST printer spelled a unit return, the Ty
    /// printer omits it. A user-written `fn(i32) -> ()` rendered
    /// `fn_i32_ret_unit`, which matches no key built from the Ty side.
    #[test]
    fn a_unit_return_is_spelled_by_omission_on_both_sides() {
        let ty = Ty::FnPtr {
            params: vec![Ty::I32],
            param_takes: vec![false],
            param_refs: vec![false],
            return_type: Box::new(Ty::Unit),
        };
        assert_eq!(render(&ty, &nominal_name), "fn_i32");
        let ast = Type {
            kind: TypeKind::FnPtr {
                params: vec![Type {
                    kind: TypeKind::Path("i32".into()),
                    span: crate::lexer::Span::new(0, 0),
                }],
                param_takes: vec![false],
                param_refs: vec![false],
                return_type: Some(Box::new(Type {
                    kind: TypeKind::Path("()".into()),
                    span: crate::lexer::Span::new(0, 0),
                })),
            },
            span: crate::lexer::Span::new(0, 0),
        };
        assert_eq!(render_ast(&ast), "fn_i32");
    }

    /// Divergence 3: `f16` is emitted by every printer; the whole-string
    /// parser had no arm for it.
    #[test]
    fn f16_decodes() {
        assert_eq!(from_suffix("f16", &|_| None, &|_| None), Ty::F16);
        assert_eq!(
            from_suffix("ptr_f16", &|_| None, &|_| None),
            Ty::RawPtr(Box::new(Ty::F16))
        );
        assert_eq!(
            from_suffix("maskf16x8", &|_| None, &|_| None),
            Ty::Mask {
                elem: Box::new(Ty::F16),
                lanes: 8
            }
        );
    }

    /// Divergence 4: the tokenizing parser had no vector arms at all, so a
    /// SIMD element inside a fn-pointer parameter list did not parse.
    #[test]
    fn vectors_decode_in_a_parameter_list() {
        let ty = Ty::FnPtr {
            params: vec![
                Ty::Simd {
                    elem: Box::new(Ty::F32),
                    lanes: 4,
                },
                Ty::Mask {
                    elem: Box::new(Ty::F32),
                    lanes: 4,
                },
            ],
            param_takes: vec![false, false],
            param_refs: vec![false, false],
            return_type: Box::new(Ty::Unit),
        };
        let s = render(&ty, &nominal_name);
        assert_eq!(s, "fn_f32x4_maskf32x4");
        assert_eq!(take(&s, &nominal_take), Some((ty, "")));
    }

    /// Divergence 5: the vector rule ran before the exact nominal lookup, so a
    /// user struct named `i8x2` decoded as a two-lane vector.
    #[test]
    fn a_struct_named_like_a_vector_wins_over_the_vector_rule() {
        assert_eq!(
            from_suffix("i8x2", &nominal_take, &|_| None),
            Ty::Struct(StructId(1))
        );
        assert_eq!(take("i8x2", &nominal_take), Some((Ty::Struct(StructId(1)), "")));
        // ... and a real vector still decodes, through both parsers.
        assert_eq!(
            from_suffix("i8x4", &nominal_take, &|_| None),
            Ty::Simd {
                elem: Box::new(Ty::I8),
                lanes: 4
            }
        );
        assert_eq!(
            take("i8x4", &nominal_take),
            Some((
                Ty::Simd {
                    elem: Box::new(Ty::I8),
                    lanes: 4
                },
                ""
            ))
        );
    }

    #[test]
    fn join_composes_and_measures_the_same_name() {
        let args = vec!["i32".to_string(), "Point".to_string()];
        assert_eq!(join("Pair", &args), "Pair__i32__Point");
        assert_eq!(
            join_len("Pair", args.iter().map(|a| a.len())),
            "Pair__i32__Point".len()
        );
    }

    /// A qualified nominal name contains `.` and no interior `__`; the E0917
    /// reservation is what makes the longest-prefix match unambiguous.
    #[test]
    fn qualified_nominal_names_round_trip() {
        let ty = Ty::Slice(Box::new(Ty::Enum(EnumId(0))));
        let s = render(&ty, &nominal_name);
        assert_eq!(s, "slice_pkg.mod.Color");
        assert_eq!(take(&s, &nominal_take), Some((ty, "")));
    }
}
