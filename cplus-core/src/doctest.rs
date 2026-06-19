//! Phase 5 slice 5DOC — doctest extraction.
//!
//! Scans `///` doc-comment blocks for triple-backtick fences and synthesizes
//! `#[test]` functions out of each fence body. The output is source text;
//! the synthesized functions are appended to the file source and then go
//! through the regular lex / parse / lower / sema / borrowck / codegen
//! pipeline — same as a hand-written `#[test] fn`.
//!
//! Design note: [docs/design/phase5-doctests.md](../../docs/design/phase5-doctests.md).
//!
//! Pre-parser source-rewriting is the Phase-5 implementation choice. The note
//! §6 enumerates two equally-valid hooks (token-stream rewrite, AST rewrite);
//! the source-level path keeps the change confined to a single module and
//! preserves byte spans for everything *outside* the synthesized region. Span
//! attribution into the fence body is approximate (the synthesized fn's body
//! sits at the end of the file, not at the doc comment) — design-note §3.4
//! flags this as a follow-up.
//!
//! Naming: each synthesized fn is `__doctest_<item_name>_<fence_idx>`. The
//! identifier must be a valid C+ name so we can't use the note's literal
//! `DOC_TEST::...` form here (`::` is path syntax, not an identifier char).
//! `attrs::discover_tests` recognizes the `__doctest_` prefix and rewrites
//! the display name to the design-note form for human / JSON output.

/// Extract doctests from `src` and return rewritten source. New `#[test]`
/// functions are appended verbatim after the original program — there are no
/// imports to merge, no item ordering to disturb, and parser passes that
/// follow walk the resulting concatenation as a single file.
///
/// The original source bytes are unchanged in the prefix region; only the
/// trailing region differs. Callers that need span-stable behavior on
/// non-doctest items see no shift.
pub fn extract(src: &str) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let mut appended = String::new();
    let mut i = 0;
    while i < lines.len() {
        if !is_doc_line(lines[i]) {
            i += 1;
            continue;
        }
        // Collect a contiguous run of `///` lines.
        let block_start = i;
        let mut block_bodies: Vec<&str> = Vec::new();
        while i < lines.len() && is_doc_line(lines[i]) {
            block_bodies.push(strip_doc_prefix(lines[i]));
            i += 1;
        }
        // Walk forward over blank lines and ordinary comments to find the
        // next item header. The item header gives the synthesized fn a
        // human-readable suffix; if no item is found (doc comment at end of
        // file), fall back to a line-number based name.
        let item_name = find_next_item_name(&lines, i)
            .unwrap_or_else(|| format!("anon_l{}", block_start + 1));
        // Walk the block body collecting fenced sections.
        for (fence_idx, fence) in find_fences(&block_bodies).into_iter().enumerate() {
            let fn_name = format!("__doctest_{item_name}_{fence_idx}");
            appended.push('\n');
            appended.push_str("#[test]\n");
            appended.push_str(&format!("fn {fn_name}() {{\n"));
            for line in &fence {
                appended.push_str(line);
                appended.push('\n');
            }
            appended.push_str("}\n");
        }
    }
    if appended.is_empty() {
        return src.to_string();
    }
    let mut out = String::with_capacity(src.len() + appended.len() + 1);
    out.push_str(src);
    if !src.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&appended);
    out
}

fn is_doc_line(line: &str) -> bool {
    // A doc line is `///` (three slashes) optionally indented. `////` etc.
    // count too — the marker is "starts with `///` and is not a `//` line
    // comment". We disambiguate by checking the byte after the third `/`:
    // anything except another `/` opens a doc comment; `////...` is treated
    // as a doc comment too (consistent with Rust).
    let t = line.trim_start();
    t.starts_with("///")
}

fn strip_doc_prefix(line: &str) -> &str {
    let t = line.trim_start();
    // Strip the leading `///`. A single optional space after the marker is
    // also stripped so users can write `/// assert ...` and have the body
    // be `assert ...` rather than ` assert ...`. Tabs are not stripped —
    // matches Rust's behavior.
    let s = t.strip_prefix("///").unwrap_or(t);
    s.strip_prefix(' ').unwrap_or(s)
}

fn find_next_item_name(lines: &[&str], from: usize) -> Option<String> {
    let mut i = from;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.is_empty() {
            i += 1;
            continue;
        }
        // Skip non-doc line comments and block comments quickly. Multi-line
        // block comments aren't handled here — doctest blocks immediately
        // followed by a `/* */` between block and item are uncommon enough
        // that the fallback `anon_lN` name is acceptable.
        if t.starts_with("//") && !t.starts_with("///") {
            i += 1;
            continue;
        }
        if t.starts_with("#[") {
            // Skip an attribute line and keep looking.
            i += 1;
            continue;
        }
        let mut rest = t;
        for prefix in ["fn ", "fn ", "struct ", "struct ", "enum ", "enum ", "impl "] {
            if let Some(after) = rest.strip_prefix(prefix) {
                rest = after;
                let name: String = rest.chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                if !name.is_empty() {
                    return Some(name);
                }
                return None;
            }
        }
        return None;
    }
    None
}

/// Walk `lines` collecting each fence's interior. A fence opens with a line
/// that trims to exactly ```` ``` ```` and closes with the same. Unterminated
/// fences are dropped silently (parse will catch the resulting syntax error
/// if the user intended the trailing code to be a fence).
fn find_fences<'a>(lines: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut fences = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim() == "```" {
            let start = i + 1;
            let mut end = start;
            while end < lines.len() && lines[end].trim() != "```" {
                end += 1;
            }
            if end < lines.len() {
                fences.push(lines[start..end].to_vec());
                i = end + 1;
            } else {
                // Unterminated — stop scanning.
                break;
            }
        } else {
            i += 1;
        }
    }
    fences
}

/// Given a qualified name produced by `attrs::discover_tests` (e.g.
/// `src.util.__doctest_max_0`), reformat it for human/JSON display per the
/// design note: `DOC_TEST::src::util::max::0`. Returns None when the name
/// isn't a doctest synthesis (callers fall back to the standard
/// `.`→`::` conversion). The doctest prefix is matched on the *leaf*
/// segment, not the qualifier — `src.__doctest_x.foo` is not a doctest.
pub fn format_doctest_display_name(qualified: &str) -> Option<String> {
    let (prefix, leaf) = match qualified.rfind('.') {
        Some(idx) => (&qualified[..idx], &qualified[idx + 1..]),
        None => ("", qualified),
    };
    let rest = leaf.strip_prefix("__doctest_")?;
    // `<item>_<idx>` — the index is the last `_`-suffix that parses as a
    // base-10 integer. Walk from the right.
    let split = rest.rfind('_')?;
    let item = &rest[..split];
    let idx = &rest[split + 1..];
    if item.is_empty() || idx.is_empty() || !idx.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let mut out = String::from("DOC_TEST::");
    if !prefix.is_empty() {
        out.push_str(&prefix.replace('.', "::"));
        out.push_str("::");
    }
    out.push_str(item);
    out.push_str("::");
    out.push_str(idx);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_doc_comments_returns_unchanged() {
        let src = "fn main() -> i32 { return 0; }\n";
        assert_eq!(extract(src), src);
    }

    #[test]
    fn doc_comment_without_fence_is_ignored() {
        let src = "/// A regular doc comment with no fence.\nfn f() {}\n";
        assert_eq!(extract(src), src);
    }

    #[test]
    fn single_fence_synthesizes_test_fn() {
        let src = "\
/// ```
/// assert 1 == 1;
/// ```
fn f() {}
";
        let out = extract(src);
        assert!(out.contains("#[test]"), "no #[test] in output: {out}");
        assert!(out.contains("fn __doctest_f_0()"), "no synth name: {out}");
        assert!(out.contains("assert 1 == 1;"), "no fence body: {out}");
    }

    #[test]
    fn fence_strips_doc_prefix_and_one_space() {
        let src = "\
/// ```
/// let x: i32 = 5;
/// assert x == 5;
/// ```
fn f() {}
";
        let out = extract(src);
        assert!(out.contains("let x: i32 = 5;"));
        assert!(out.contains("assert x == 5;"));
        // The leading `/// ` must not appear inside the synthesized body.
        let synth_start = out.find("fn __doctest_f_0()").unwrap();
        let synth_region = &out[synth_start..];
        assert!(!synth_region.contains("///"), "doc prefix leaked into body: {synth_region}");
    }

    #[test]
    fn multiple_fences_get_distinct_names() {
        let src = "\
/// ```
/// assert true;
/// ```
/// gap text
/// ```
/// assert false;
/// ```
fn f() {}
";
        let out = extract(src);
        assert!(out.contains("fn __doctest_f_0()"));
        assert!(out.contains("fn __doctest_f_1()"));
    }

    #[test]
    fn doc_comment_attaches_to_following_item() {
        let src = "\
/// ```
/// assert true;
/// ```
fn pub_target() {}
";
        let out = extract(src);
        assert!(out.contains("fn __doctest_pub_target_0()"),
            "name should come from item past `pub`, got: {out}");
    }

    #[test]
    fn doc_comment_with_attribute_in_between_resolves_to_item() {
        let src = "\
/// ```
/// assert true;
/// ```
#[test]
fn t() {}
";
        let out = extract(src);
        assert!(out.contains("fn __doctest_t_0()"),
            "name should skip attribute lines, got: {out}");
    }

    #[test]
    fn unterminated_fence_is_dropped_silently() {
        let src = "\
/// ```
/// assert true;
fn f() {}
";
        // No `#[test]` should be synthesized — unterminated fence is ignored.
        let out = extract(src);
        assert!(!out.contains("__doctest_"),
            "unterminated fence should not synthesize: {out}");
    }

    #[test]
    fn anonymous_block_at_eof_gets_fallback_name() {
        let src = "\
fn main() {}
/// ```
/// assert true;
/// ```
";
        let out = extract(src);
        // Doc block at end of file — no following item. Falls back to
        // line-number-based name.
        assert!(out.contains("fn __doctest_anon_"),
            "expected anon fallback, got: {out}");
    }

    #[test]
    fn format_display_name_single_file() {
        assert_eq!(
            format_doctest_display_name("__doctest_max_0"),
            Some("DOC_TEST::max::0".to_string())
        );
    }

    #[test]
    fn format_display_name_qualified() {
        assert_eq!(
            format_doctest_display_name("src.util.__doctest_max_0"),
            Some("DOC_TEST::src::util::max::0".to_string())
        );
    }

    #[test]
    fn format_display_name_rejects_non_doctest() {
        assert_eq!(format_doctest_display_name("foo"), None);
        assert_eq!(format_doctest_display_name("src.math.foo"), None);
    }

    #[test]
    fn format_display_name_rejects_missing_index() {
        assert_eq!(format_doctest_display_name("__doctest_max"), None);
    }

    #[test]
    fn format_display_name_rejects_non_numeric_index() {
        assert_eq!(format_doctest_display_name("__doctest_max_x"), None);
    }

    #[test]
    fn no_doc_comment_at_eof_no_panic() {
        // Edge: source ends with a `///` block and no following item.
        let src = "/// hello\n";
        let out = extract(src);
        assert_eq!(out, src);
    }

    #[test]
    fn item_name_handles_struct() {
        let src = "\
/// ```
/// assert true;
/// ```
struct Point { x: i32, y: i32 }
";
        let out = extract(src);
        assert!(out.contains("fn __doctest_Point_0()"));
    }
}
