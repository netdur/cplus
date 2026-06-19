//! v0.0.3 Phase 5 Slice 5A: atomic intrinsic spec parser.
//!
//! Atomic operations are compiler intrinsics named
//! `__cplus_atomic_<op>_<ty>_<ord>`. The name-pattern carries enough
//! structure to map directly onto an LLVM atomic instruction without
//! needing a separate `Ordering` enum in the type system. The stdlib
//! `atomic` module wraps these intrinsics in an ergonomic surface.
//!
//! Recognised forms:
//!
//! - `__cplus_atomic_load_<ty>_<ord>(p: *T) -> T`
//! - `__cplus_atomic_store_<ty>_<ord>(p: *T, v: T) -> ()`
//! - `__cplus_atomic_xchg_<ty>_<ord>(p: *T, v: T) -> T`
//! - `__cplus_atomic_cmpxchg_<ty>_<ord>(p: *T, expected: T, desired: T) -> T`
//!   (returns the previous value; compare to `expected` for success)
//! - `__cplus_atomic_fetch_{add,sub,and,or,xor}_<ty>_<ord>(p: *T, v: T) -> T`
//!
//! `<ty>` ∈ {`i8`, `i16`, `i32`, `i64`, `u8`, `u16`, `u32`, `u64`}.
//! `<ord>` ∈ {`relaxed`, `acquire`, `release`, `acqrel`, `seqcst`}.
//!
//! Ordering maps to LLVM keywords as: relaxed→monotonic, acquire→acquire,
//! release→release, acqrel→acq_rel, seqcst→seq_cst. Cmpxchg uses the
//! same ordering for both success and failure (failure-ordering can't
//! be stronger than success; using the same keyword sidesteps the
//! constraint when callers pass `relaxed`/`acquire`/`seqcst`).

use crate::sema::Ty;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AtomicOp {
    Load,
    Store,
    Xchg,
    Cmpxchg,
    FetchAdd,
    FetchSub,
    FetchAnd,
    FetchOr,
    FetchXor,
}

impl AtomicOp {
    /// The LLVM `atomicrmw` opcode (for fetch ops + xchg) or `""` for
    /// load/store/cmpxchg which use different instructions.
    pub fn rmw_opcode(self) -> &'static str {
        match self {
            AtomicOp::FetchAdd => "add",
            AtomicOp::FetchSub => "sub",
            AtomicOp::FetchAnd => "and",
            AtomicOp::FetchOr => "or",
            AtomicOp::FetchXor => "xor",
            AtomicOp::Xchg => "xchg",
            _ => "",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtomicSpec {
    pub op: AtomicOp,
    pub ty: Ty,
    pub bits: u32,
    /// LLVM ordering keyword: `monotonic`, `acquire`, `release`,
    /// `acq_rel`, or `seq_cst`.
    pub llvm_ordering: &'static str,
}

impl AtomicSpec {
    /// Number of *value* args (excluding the pointer).
    pub fn value_arg_count(&self) -> usize {
        match self.op {
            AtomicOp::Load => 0,
            AtomicOp::Cmpxchg => 2,
            _ => 1,
        }
    }

    /// Whether the intrinsic returns the operand type (vs `()`).
    pub fn returns_value(&self) -> bool {
        !matches!(self.op, AtomicOp::Store)
    }
}

/// Parse `__cplus_atomic_*` names. Returns `None` if the name isn't an
/// atomic intrinsic; returns `Some(spec)` if every component is recognised.
pub fn parse_atomic_intrinsic(name: &str) -> Option<AtomicSpec> {
    let rest = name.strip_prefix("__cplus_atomic_")?;

    // Split off the trailing ordering suffix first — orderings are
    // single tokens, types are single tokens, but op names can contain
    // underscores (`fetch_add`).
    let (head, ord) = split_off_suffix(rest, ORDERINGS)?;
    let llvm_ordering = ordering_to_llvm(ord)?;

    // Then split off the type from the remaining head.
    let (op_str, ty_str) = split_off_suffix(head, TYPES)?;
    let (ty, bits) = type_str_to_ty(ty_str)?;

    let op = match op_str {
        "load" => AtomicOp::Load,
        "store" => AtomicOp::Store,
        "xchg" => AtomicOp::Xchg,
        "cmpxchg" => AtomicOp::Cmpxchg,
        "fetch_add" => AtomicOp::FetchAdd,
        "fetch_sub" => AtomicOp::FetchSub,
        "fetch_and" => AtomicOp::FetchAnd,
        "fetch_or" => AtomicOp::FetchOr,
        "fetch_xor" => AtomicOp::FetchXor,
        _ => return None,
    };

    Some(AtomicSpec {
        op,
        ty,
        bits,
        llvm_ordering,
    })
}

/// v0.0.12 G-030 (llama.cplus G-029): parse `__cplus_atomic_fence_<ord>`
/// — a standalone memory fence with no operand and no type. Returns the
/// LLVM `fence` ordering keyword on hit. Distinct from the typed atomic
/// ops because there's no pointer or value to thread through.
///
/// Lowering note: LLVM's `fence relaxed`-equivalent (`fence monotonic`)
/// is rejected by the verifier — only acquire / release / acq_rel /
/// seq_cst are valid orderings for `fence`. We honor that here: a
/// `relaxed` argument from the stdlib wrapper is a no-op (returns
/// `Some("relaxed")` so sema accepts the call, and codegen emits no
/// instruction at all — matching what every C compiler does for
/// `atomic_thread_fence(memory_order_relaxed)`).
pub fn parse_atomic_fence(name: &str) -> Option<&'static str> {
    let rest = name.strip_prefix("__cplus_atomic_fence_")?;
    match rest {
        "relaxed" => Some("relaxed"),
        "acquire" => Some("acquire"),
        "release" => Some("release"),
        "acqrel" => Some("acqrel"),
        "seqcst" => Some("seqcst"),
        _ => None,
    }
}

const ORDERINGS: &[&str] = &["relaxed", "acquire", "release", "acqrel", "seqcst"];
const TYPES: &[&str] = &["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"];

fn split_off_suffix<'a>(s: &'a str, suffixes: &[&'static str]) -> Option<(&'a str, &'static str)> {
    for suf in suffixes {
        if let Some(head) = s.strip_suffix(suf) {
            if let Some(head) = head.strip_suffix('_') {
                // Return the canonical &'static str, not the &str slice
                // from `s` — callers want literal equality with the
                // suffix list.
                return Some((head, suf));
            }
        }
    }
    None
}

fn ordering_to_llvm(ord: &str) -> Option<&'static str> {
    Some(match ord {
        "relaxed" => "monotonic",
        "acquire" => "acquire",
        "release" => "release",
        "acqrel" => "acq_rel",
        "seqcst" => "seq_cst",
        _ => return None,
    })
}

fn type_str_to_ty(s: &str) -> Option<(Ty, u32)> {
    Some(match s {
        "i8" => (Ty::I8, 8),
        "i16" => (Ty::I16, 16),
        "i32" => (Ty::I32, 32),
        "i64" => (Ty::I64, 64),
        "u8" => (Ty::U8, 8),
        "u16" => (Ty::U16, 16),
        "u32" => (Ty::U32, 32),
        "u64" => (Ty::U64, 64),
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_load() {
        let s = parse_atomic_intrinsic("__cplus_atomic_load_i32_seqcst").unwrap();
        assert_eq!(s.op, AtomicOp::Load);
        assert_eq!(s.ty, Ty::I32);
        assert_eq!(s.bits, 32);
        assert_eq!(s.llvm_ordering, "seq_cst");
    }

    #[test]
    fn parses_store() {
        let s = parse_atomic_intrinsic("__cplus_atomic_store_i64_release").unwrap();
        assert_eq!(s.op, AtomicOp::Store);
        assert_eq!(s.ty, Ty::I64);
        assert_eq!(s.llvm_ordering, "release");
    }

    #[test]
    fn parses_fetch_add() {
        let s = parse_atomic_intrinsic("__cplus_atomic_fetch_add_u64_relaxed").unwrap();
        assert_eq!(s.op, AtomicOp::FetchAdd);
        assert_eq!(s.ty, Ty::U64);
        assert_eq!(s.llvm_ordering, "monotonic");
    }

    #[test]
    fn parses_fetch_xor_acqrel() {
        let s = parse_atomic_intrinsic("__cplus_atomic_fetch_xor_i8_acqrel").unwrap();
        assert_eq!(s.op, AtomicOp::FetchXor);
        assert_eq!(s.ty, Ty::I8);
        assert_eq!(s.llvm_ordering, "acq_rel");
    }

    #[test]
    fn parses_cmpxchg() {
        let s = parse_atomic_intrinsic("__cplus_atomic_cmpxchg_i32_acquire").unwrap();
        assert_eq!(s.op, AtomicOp::Cmpxchg);
        assert_eq!(s.value_arg_count(), 2);
    }

    #[test]
    fn parses_xchg() {
        let s = parse_atomic_intrinsic("__cplus_atomic_xchg_u32_seqcst").unwrap();
        assert_eq!(s.op, AtomicOp::Xchg);
        assert_eq!(s.value_arg_count(), 1);
    }

    #[test]
    fn rejects_non_atomic() {
        assert!(parse_atomic_intrinsic("println").is_none());
        assert!(parse_atomic_intrinsic("__cplus_atomic_").is_none());
        assert!(parse_atomic_intrinsic("__cplus_atomic_load_i32").is_none()); // no ordering
        assert!(parse_atomic_intrinsic("__cplus_atomic_load_i128_seqcst").is_none()); // bad ty
        assert!(parse_atomic_intrinsic("__cplus_atomic_load_i32_strong").is_none()); // bad ord
        assert!(parse_atomic_intrinsic("__cplus_atomic_bogus_i32_seqcst").is_none());
    }

    #[test]
    fn returns_value_flag_excludes_store() {
        let load = parse_atomic_intrinsic("__cplus_atomic_load_i32_seqcst").unwrap();
        let store = parse_atomic_intrinsic("__cplus_atomic_store_i32_seqcst").unwrap();
        assert!(load.returns_value());
        assert!(!store.returns_value());
    }
}
