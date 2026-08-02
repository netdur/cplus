//! Drop unreachable function definitions from generated IR before clang sees
//! them.
//!
//! Whole-program codegen emits every function of every dependency, reachable or
//! not. Measured on iris (139k lines of source across 15 packages): 23,673
//! definitions emitted, of which 20,158 — **85%** — are unreachable once the
//! removal is iterated to a fixed point. appkit alone contributes 10,309 dead
//! definitions out of 12,730. Every one of them is `internal`.
//!
//! **The saving is modest, and smaller than the size reduction suggests.**
//! Measured end to end on iris, pruned vs `CPC_NO_PRUNE=1`:
//!
//!   - debug (`-O0`):   8.0s -> 7.3s   (~9%)
//!   - release (`-O3`): 264.9s -> 256.0s (~3%)
//!
//! The reason is that LLVM already discards this code cheaply: clang
//! dead-strips unreferenced `internal` functions itself, and does so early
//! enough in the pipeline that the dead 85% is never fully optimised. The
//! object file is 15.2 MB with or without pruning. What pruning actually buys
//! is the *parse* of ~33 MB of surplus text, which is real but small.
//!
//! Recorded because an earlier estimate of "~2x on release" was wrong: it
//! compared a standalone `clang -O2` run against a real `-O3` release build,
//! so it measured the optimisation level, not the pruning. A 4x smaller module
//! is not 4x less work — the work is in the code that survives.
//!
//! The pass is kept because it costs 0.07s, shrinks the module 4x for anything
//! that reads it (`--emit-ll-project`, debugging, any future per-module
//! caching where 85% less text is 85% less to hash and store), and makes the
//! emitted IR honestly reflect what the program contains.
//!
//! This pass is deliberately a **text post-pass over the emitted module**, not a
//! reachability walk over the AST. The AST route has to enumerate every way one
//! item can name another — direct calls, method dispatch, fn-pointer references,
//! trampolines, drop glue, static initialisers — and a single missed reference
//! kind silently deletes live code. Working on the emitted IR inverts that: a
//! definition is removed only when the literal token `@name` appears nowhere
//! else in the module, so the analysis cannot miss a reference that is present.
//! The worst failure mode is a loud undefined-symbol error at link time, never
//! wrong runtime behaviour.
//!
//! Two conservative rules keep it safe:
//!   - only `internal` definitions are ever removed — anything with external
//!     linkage may be called from outside this module;
//!   - a name referenced from outside any function body (a global initialiser,
//!     an alias, metadata) is treated as a root.
//!
//! Removal is decided by one reachability walk from the roots: a definition is
//! kept when the walk reaches it, so a function stranded by the removal of its
//! only caller is simply never reached, without a second pass.

use std::collections::{HashMap, HashSet};

/// One `define ... { ... }` block in the module.
struct Block {
    name: String,
    internal: bool,
    /// Line range `[start, end]` inclusive: `start` is the `define` line,
    /// `end` the closing `}`.
    start: usize,
    end: usize,
}

/// Remove unreachable `internal` function definitions from `ir`.
///
/// Returns the rewritten module and the number of definitions dropped.
pub fn prune_unreachable(ir: &str) -> (String, usize) {
    let lines: Vec<&str> = ir.lines().collect();
    let blocks = collect_blocks(&lines);
    if blocks.is_empty() {
        // issue-08 step 4: this pass reads codegen's own text and contracts its
        // exact shape — `define ` at column 0, a closing `}` alone at column 0.
        // A formatting change on the emitter side turns pruning into a silent
        // no-op: conservative, so nothing breaks, and nothing says so either
        // (the module just stops shrinking). A module with `define ` in it and
        // no blocks parsed means the contract broke.
        debug_assert!(
            !ir.contains("\ndefine "),
            "prune found no blocks in a module that defines functions — \
             codegen's text shape and this parser have diverged"
        );
        return (ir.to_string(), 0);
    }

    let by_name: HashMap<&str, usize> = blocks
        .iter()
        .enumerate()
        .map(|(i, b)| (b.name.as_str(), i))
        .collect();

    // Which lines belong to which block, so "outside" references are cheap to
    // identify. A line inside no block indexes to `usize::MAX`.
    let mut owner = vec![usize::MAX; lines.len()];
    for (i, b) in blocks.iter().enumerate() {
        for l in b.start..=b.end {
            owner[l] = i;
        }
    }

    // Edges: block -> names its body references. Plus the root set from every
    // reference that is not inside a function body.
    let mut edges: Vec<HashSet<usize>> = vec![HashSet::new(); blocks.len()];
    let mut roots: HashSet<usize> = HashSet::new();
    for (ln, line) in lines.iter().enumerate() {
        let holder = owner[ln];
        // The `define` line names the function itself; that is not a reference.
        let is_own_header = holder != usize::MAX && blocks[holder].start == ln;
        for name in symbol_refs(line) {
            let Some(&target) = by_name.get(name) else {
                continue;
            };
            if is_own_header && target == holder {
                continue;
            }
            if holder == usize::MAX {
                roots.insert(target);
            } else {
                edges[holder].insert(target);
            }
        }
    }

    // Anything not `internal` is externally visible and must be kept.
    for (i, b) in blocks.iter().enumerate() {
        if !b.internal {
            roots.insert(i);
        }
    }

    // Reachability from the roots.
    let mut live: HashSet<usize> = HashSet::new();
    let mut stack: Vec<usize> = roots.into_iter().collect();
    while let Some(i) = stack.pop() {
        if !live.insert(i) {
            continue;
        }
        for &next in &edges[i] {
            if !live.contains(&next) {
                stack.push(next);
            }
        }
    }

    let dropped = blocks.len() - live.len();
    if dropped == 0 {
        return (ir.to_string(), 0);
    }

    // Emit every line except those belonging to a dead block.
    let mut dead_line = vec![false; lines.len()];
    for (i, b) in blocks.iter().enumerate() {
        if !live.contains(&i) {
            for l in b.start..=b.end {
                dead_line[l] = true;
            }
        }
    }
    let mut out = String::with_capacity(ir.len());
    for (ln, line) in lines.iter().enumerate() {
        if dead_line[ln] {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    (out, dropped)
}

/// Find the `define` blocks. A block runs from a line starting with `define `
/// to the next line that is exactly `}` at column 0 — the shape codegen emits.
fn collect_blocks(lines: &[&str]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        if let Some(rest) = lines[i].strip_prefix("define ") {
            let Some(name) = first_symbol(rest) else {
                i += 1;
                continue;
            };
            // Linkage keywords sit between `define` and the return type, so an
            // `internal` before the `@` is this function's linkage.
            let head = &rest[..rest.find('@').unwrap_or(rest.len())];
            let internal = head.split_whitespace().any(|w| w == "internal");
            let mut j = i + 1;
            while j < lines.len() && lines[j] != "}" {
                j += 1;
            }
            if j >= lines.len() {
                break; // unterminated; leave the tail alone
            }
            out.push(Block {
                name: name.to_string(),
                internal,
                start: i,
                end: j,
            });
            i = j + 1;
            continue;
        }
        i += 1;
    }
    out
}

/// The first `@symbol` in `s`, if any.
fn first_symbol(s: &str) -> Option<&str> {
    let at = s.find('@')?;
    let rest = &s[at + 1..];
    let end = rest
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$'))
        .unwrap_or(rest.len());
    if end == 0 {
        None
    } else {
        Some(&rest[..end])
    }
}

/// Every `@symbol` occurring in `s`.
fn symbol_refs(s: &str) -> Vec<&str> {
    let bytes = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            let rest = &s[i + 1..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$'))
                .unwrap_or(rest.len());
            if end > 0 {
                out.push(&rest[..end]);
                i += 1 + end;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_an_internal_function_nothing_references() {
        let ir = "\
define i32 @main() {
  ret i32 0
}
define internal i32 @dead() {
  ret i32 1
}
";
        let (out, n) = prune_unreachable(ir);
        assert_eq!(n, 1);
        assert!(out.contains("@main"));
        assert!(!out.contains("@dead"), "{out}");
    }

    #[test]
    fn keeps_an_internal_function_that_is_called() {
        let ir = "\
define i32 @main() {
  %1 = call i32 @helper()
  ret i32 %1
}
define internal i32 @helper() {
  ret i32 7
}
";
        let (out, n) = prune_unreachable(ir);
        assert_eq!(n, 0);
        assert!(out.contains("@helper"));
    }

    // External linkage means a caller outside this module may exist.
    #[test]
    fn never_drops_an_externally_visible_definition() {
        let ir = "\
define void @exported_shim() {
  ret void
}
";
        let (_out, n) = prune_unreachable(ir);
        assert_eq!(n, 0);
    }

    // Deleting a function strands whatever only it called; iterate.
    #[test]
    fn iterates_to_a_fixed_point_through_a_dead_chain() {
        let ir = "\
define i32 @main() {
  ret i32 0
}
define internal i32 @a() {
  %1 = call i32 @b()
  ret i32 %1
}
define internal i32 @b() {
  %1 = call i32 @c()
  ret i32 %1
}
define internal i32 @c() {
  ret i32 3
}
";
        let (out, n) = prune_unreachable(ir);
        assert_eq!(n, 3, "whole chain should go: {out}");
        assert!(out.contains("@main"));
        for d in ["@a(", "@b(", "@c("] {
            assert!(!out.contains(d), "{d} survived: {out}");
        }
    }

    // A reference from outside any body — a global initialiser holding a
    // function pointer — is a root. Missing this would delete live code.
    #[test]
    fn a_function_referenced_only_by_a_global_is_a_root() {
        let ir = "\
@table = internal global [1 x ptr] [ptr @handler]
define i32 @main() {
  ret i32 0
}
define internal void @handler() {
  ret void
}
";
        let (out, n) = prune_unreachable(ir);
        assert_eq!(n, 0, "handler is reachable via @table: {out}");
        assert!(out.contains("@handler"));
    }

    // Recursion must not make a dead function look live.
    #[test]
    fn a_self_recursive_dead_function_is_still_dropped() {
        let ir = "\
define i32 @main() {
  ret i32 0
}
define internal i32 @loops() {
  %1 = call i32 @loops()
  ret i32 %1
}
";
        let (_out, n) = prune_unreachable(ir);
        assert_eq!(n, 1);
    }

    #[test]
    fn mutually_recursive_dead_pair_is_dropped() {
        let ir = "\
define i32 @main() {
  ret i32 0
}
define internal i32 @ping() {
  %1 = call i32 @pong()
  ret i32 %1
}
define internal i32 @pong() {
  %1 = call i32 @ping()
  ret i32 %1
}
";
        let (_out, n) = prune_unreachable(ir);
        assert_eq!(n, 2);
    }

    #[test]
    fn declares_and_metadata_survive_untouched() {
        let ir = "\
declare ptr @malloc(i64)
define i32 @main() {
  %1 = call ptr @malloc(i64 8)
  ret i32 0
}
!0 = !{i32 1}
";
        let (out, n) = prune_unreachable(ir);
        assert_eq!(n, 0);
        assert!(out.contains("declare ptr @malloc"));
        assert!(out.contains("!0 = !{i32 1}"));
    }

    #[test]
    fn empty_or_definitionless_module_is_returned_unchanged() {
        for ir in ["", "declare void @f()\n"] {
            let (out, n) = prune_unreachable(ir);
            assert_eq!(n, 0);
            assert_eq!(out, ir);
        }
    }
}

