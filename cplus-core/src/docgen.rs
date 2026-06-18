//! Phase 11 polish (2026-05-14): documentation generator.
//!
//! Walks a C+ source file looking for `pub` items immediately preceded
//! by a block of `///` doc comments. Emits Markdown — one page per
//! input file — that pairs each item's signature with its docs.
//!
//! Reuses the same surface as the doctest extractor (slice 5DOC):
//! - `///` is the doc-comment marker.
//! - Triple-backtick fenced blocks inside `///` are code examples that
//!   `cpc test` runs verbatim. The doc generator preserves them as
//!   Markdown code blocks (rendered the same way GitHub does).
//!
//! Source-based, not AST-based — same trade-off the doctest extractor
//! made. Pro: simple, no new AST surface needed. Con: signature
//! extraction is a textual line-grab, not a fully-typed rendering. For
//! v1 this is fine; a future polish slice could swap to AST + the `cpc
//! fmt` renderer for canonical signatures.

/// One documented item extracted from a source file.
#[derive(Debug, Clone, PartialEq)]
pub struct DocItem {
    /// Item kind label, e.g. `"fn"`, `"struct"`, `"enum"`, `"impl"`,
    /// `"interface"`, `"type"`. Used as a section header in the output.
    pub kind: ItemKind,
    /// Source-level name, e.g. `"max"` or `"Point"`. Used as the
    /// subsection header.
    pub name: String,
    /// The signature line(s) up to (but not including) the opening
    /// brace `{` or terminator `;`. Whitespace preserved.
    pub signature: String,
    /// The doc comment body, with the leading `///` and one optional
    /// space stripped from each line. Empty lines between doc lines
    /// are preserved (Markdown paragraph break).
    pub doc: String,
    /// 1-based line number where the item starts (after the doc
    /// comment block). Used for "defined at file:line" links in the
    /// rendered output.
    pub line: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Fn,
    Struct,
    Enum,
    Impl,
    Interface,
    TypeAlias,
}

impl ItemKind {
    pub fn label(self) -> &'static str {
        match self {
            ItemKind::Fn => "fn",
            ItemKind::Struct => "struct",
            ItemKind::Enum => "enum",
            ItemKind::Impl => "impl",
            ItemKind::Interface => "interface",
            ItemKind::TypeAlias => "type",
        }
    }
}

/// Extract every `pub` item with a preceding `///` doc block from
/// `src`. Items without docs are silently skipped — undocumented
/// internals shouldn't pollute the user-facing reference. Private
/// items are skipped regardless of whether they have docs.
///
/// `impl` blocks are a slight special case: the impl itself is
/// emitted if its target type is pub (parsed conservatively from the
/// `impl Name` line), and `pub fn`s *inside* the impl get their own
/// entries with `name = "TargetName::method_name"`.
pub fn extract(src: &str) -> Vec<DocItem> {
    let lines: Vec<&str> = src.lines().collect();
    let mut items = Vec::new();
    let mut i = 0;
    let mut current_impl_target: Option<String> = None;
    while i < lines.len() {
        // Try to find a doc block starting at line i.
        let (doc_lines, doc_start) = collect_doc_block(&lines, i);
        if doc_lines.is_empty() {
            // No doc block here; check if this is an impl-opening or
            // impl-closing line to track the current_impl_target.
            update_impl_tracker(lines[i], &mut current_impl_target);
            i += 1;
            continue;
        }
        // Advance past the doc block and any attributes.
        let after_doc = doc_start + doc_lines.len();
        let item_line = skip_attributes(&lines, after_doc);
        if item_line >= lines.len() {
            // Doc block at end of file with no item — skip.
            break;
        }
        if let Some((kind, name, signature)) =
            parse_item_header(&lines, item_line, current_impl_target.as_deref())
        {
            items.push(DocItem {
                kind,
                name,
                signature,
                doc: render_doc_body(&doc_lines),
                line: item_line + 1,
            });
        }
        // Track impl context if this line opens one (so subsequent
        // methods get qualified).
        update_impl_tracker(lines[item_line], &mut current_impl_target);
        i = item_line + 1;
    }
    items
}

/// Render extracted items to Markdown. One section per item; items
/// grouped by kind. Header includes the source file's basename.
pub fn render_markdown(file_label: &str, items: &[DocItem]) -> String {
    let mut out = String::new();
    out.push_str(&format!("# `{file_label}`\n\n"));
    if items.is_empty() {
        out.push_str("_No documented `pub` items._\n");
        return out;
    }
    // Table of contents.
    out.push_str("## Contents\n\n");
    for it in items {
        out.push_str(&format!(
            "- [`{}` {}](#{})\n",
            it.kind.label(),
            it.name,
            anchor(&it.name)
        ));
    }
    out.push('\n');
    // Per-item sections.
    for it in items {
        out.push_str(&format!("## `{} {}` <a id=\"{}\"></a>\n\n", it.kind.label(), it.name, anchor(&it.name)));
        out.push_str(&format!("Defined at line {}.\n\n", it.line));
        out.push_str("```\n");
        out.push_str(it.signature.trim_end());
        out.push_str("\n```\n\n");
        if !it.doc.is_empty() {
            out.push_str(&it.doc);
            if !it.doc.ends_with('\n') { out.push('\n'); }
            out.push('\n');
        }
    }
    out
}

// ---- internal helpers ----

fn collect_doc_block(lines: &[&str], from: usize) -> (Vec<String>, usize) {
    let mut out = Vec::new();
    let mut i = from;
    while i < lines.len() && lines[i].trim_start().starts_with("///") {
        out.push(strip_doc_prefix(lines[i]).to_string());
        i += 1;
    }
    (out, from)
}

fn strip_doc_prefix(line: &str) -> &str {
    let t = line.trim_start();
    let s = t.strip_prefix("///").unwrap_or(t);
    s.strip_prefix(' ').unwrap_or(s)
}

fn skip_attributes(lines: &[&str], from: usize) -> usize {
    let mut i = from;
    while i < lines.len() {
        let t = lines[i].trim_start();
        if t.is_empty() || t.starts_with("#[") || (t.starts_with("//") && !t.starts_with("///")) {
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// Parse the signature line(s) starting at `start`. Returns
/// `(kind, display_name, signature_text)`. Signature spans until the
/// first `{` (block-bodied items) or `;` (extern fn / type alias)
/// encountered, walking subsequent lines for multi-line signatures.
fn parse_item_header(
    lines: &[&str],
    start: usize,
    current_impl_target: Option<&str>,
) -> Option<(ItemKind, String, String)> {
    let head = lines[start].trim_start();
    // pub modifier required for top-level items; impl-method `pub fn`
    // also recognized via `current_impl_target.is_some()` AND the
    // prefix check.
    let is_inside_impl = current_impl_target.is_some();
    // Try each known item kind. Order matters: `pub fn` before `fn`,
    // `pub struct` before `struct`, etc. — but we also accept the
    // un-`pub` form when inside an impl that is itself pub-targeted
    // (the surrounding impl's pub-ness governs the inner methods).
    let kinds: &[(&str, ItemKind)] = &[
        ("pub fn ",        ItemKind::Fn),
        ("pub extern fn ", ItemKind::Fn),
        ("pub struct ",    ItemKind::Struct),
        ("pub enum ",      ItemKind::Enum),
        ("pub interface ", ItemKind::Interface),
        ("pub type ",      ItemKind::TypeAlias),
        ("pub impl ",      ItemKind::Impl),
    ];
    let mut matched: Option<(&str, ItemKind, bool /* pub */)> = None;
    for (prefix, kind) in kinds {
        if head.starts_with(prefix) {
            matched = Some((prefix.trim_start_matches("pub ").trim(), *kind, true));
            break;
        }
    }
    if matched.is_none() {
        // Plain `impl Type { ... }` is also documentable (matches Rust).
        if head.starts_with("impl ") {
            matched = Some(("impl", ItemKind::Impl, true));
        } else if is_inside_impl {
            // Inside a pub-targeted impl: accept `pub fn name` only.
            for (prefix, kind) in &[("pub fn ", ItemKind::Fn)] {
                if head.starts_with(prefix) {
                    matched = Some((prefix.trim_start_matches("pub ").trim(), *kind, true));
                    break;
                }
            }
        }
    }
    let (_keyword, kind, _is_pub) = matched?;
    // Extract the bare name: skip the leading `pub `/`pub extern `, the
    // item keyword, then read an identifier.
    let after_kw = head
        .trim_start_matches("pub ")
        .trim_start_matches("extern ")
        .splitn(2, ' ')
        .nth(1)
        .unwrap_or("");
    let raw_name: String = after_kw.chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if raw_name.is_empty() { return None; }
    let display_name = if let (ItemKind::Fn, Some(target)) = (kind, current_impl_target) {
        format!("{target}::{raw_name}")
    } else {
        raw_name.clone()
    };
    // Signature: from `start` until the line containing the first `{`
    // or `;` outside a string literal. We accept the textual form —
    // a `{` inside a default-value initializer would terminate early
    // but C+ has no default values, so it's fine.
    let mut sig = String::new();
    for j in start..lines.len() {
        let l = lines[j];
        if !sig.is_empty() { sig.push('\n'); }
        // Strip to the first `{` or `;` if present on this line.
        if let Some(stop) = l.bytes().position(|b| b == b'{' || b == b';') {
            sig.push_str(&l[..stop]);
            return Some((kind, display_name, sig.trim_end().to_string()));
        }
        sig.push_str(l);
    }
    Some((kind, display_name, sig.trim_end().to_string()))
}

fn update_impl_tracker(line: &str, current: &mut Option<String>) {
    let t = line.trim_start();
    if let Some(after) = t.strip_prefix("impl ").or_else(|| t.strip_prefix("pub impl ")) {
        let name: String = after.chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '[' || *c == ']' || *c == ',' || *c == ' ')
            .collect::<String>()
            .trim()
            .to_string();
        // Type-first: `impl TYPE: INTERFACE` → use TYPE (the implementing type)
        // to qualify the method name. v0.0.24 de-Rust: the connector is `:`, and
        // the `take_while` above stops at it, so `name` is already just the type
        // for `:`-form code (the `" for "` split below is vestigial back-compat,
        // a no-op now). Strip any generic params (`impl Vec[T]: Iterator` → `Vec`).
        let type_part = match name.find(" for ") {
            Some(idx) => name[..idx].trim(),
            None => name.trim(),
        };
        let target = type_part.split('[').next().unwrap_or(type_part).trim().to_string();
        if !target.is_empty() {
            *current = Some(target);
        }
    }
    // A closing `}` at column 0 marks the end of the impl block.
    if line == "}" {
        *current = None;
    }
}

fn render_doc_body(lines: &[String]) -> String {
    let mut out = String::new();
    for l in lines {
        out.push_str(l);
        out.push('\n');
    }
    out
}

/// Anchor-safe slug for in-document `#section` links. Lowercases,
/// strips most punctuation, collapses runs of `-`.
fn anchor(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    while out.ends_with('-') { out.pop(); }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_docs_returns_empty() {
        assert!(extract("fn main() -> i32 { return 0; }").is_empty());
    }

    #[test]
    fn ignores_non_pub_items() {
        let src = "\
/// Internal helper.
fn private_helper() -> i32 { return 0; }

/// Exported.
pub fn public_thing() -> i32 { return 1; }
";
        let items = extract(src);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "public_thing");
    }

    #[test]
    fn extracts_pub_fn_with_signature_and_body() {
        let src = "\
/// Returns the sum.
///
/// # Example
///
/// ```
/// assert add(1, 2) == 3;
/// ```
pub fn add(a: i32, b: i32) -> i32 {
    return a +% b;
}
";
        let items = extract(src);
        assert_eq!(items.len(), 1);
        let it = &items[0];
        assert_eq!(it.kind, ItemKind::Fn);
        assert_eq!(it.name, "add");
        assert!(it.signature.contains("pub fn add(a: i32, b: i32) -> i32"));
        assert!(it.doc.contains("Returns the sum."));
        assert!(it.doc.contains("```"));
    }

    #[test]
    fn extracts_pub_struct_and_enum_and_type() {
        let src = "\
/// A point in 2D space.
pub struct Point { pub x: i32, pub y: i32 }

/// RGB color.
pub enum Color { Red, Green, Blue }

/// Byte count alias.
pub type Bytes = usize;
";
        let items = extract(src);
        let kinds: Vec<ItemKind> = items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, vec![ItemKind::Struct, ItemKind::Enum, ItemKind::TypeAlias]);
    }

    #[test]
    fn impl_block_methods_get_qualified_names() {
        let src = "\
pub struct Point { x: i32 }

impl Point {
    /// Construct a new point.
    pub fn new(x: i32) -> Point { return Point { x: x }; }

    /// Read the x coordinate.
    pub fn x(this) -> i32 { return this.x; }
}
";
        let items = extract(src);
        let names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        assert!(names.contains(&"Point::new"));
        assert!(names.contains(&"Point::x"));
    }

    #[test]
    fn impl_interface_for_target_uses_implementing_type_name() {
        let src = "\
pub interface Display { fn show(this) -> i32 }

impl Counter: Display {
    /// Show the counter's value.
    pub fn show(this) -> i32 { return this.value; }
}
";
        let items = extract(src);
        let show = items.iter().find(|i| i.name.ends_with("show")).expect("show method");
        assert_eq!(show.name, "Counter::show");
    }

    #[test]
    fn markdown_render_includes_toc_and_sections() {
        let src = "\
/// The first.
pub fn alpha() -> i32 { return 1; }

/// The second.
pub fn beta() -> i32 { return 2; }
";
        let items = extract(src);
        let md = render_markdown("util.cplus", &items);
        assert!(md.starts_with("# `util.cplus`"));
        assert!(md.contains("## Contents"));
        assert!(md.contains("- [`fn` alpha](#alpha)"));
        assert!(md.contains("## `fn alpha`"));
        assert!(md.contains("The first."));
    }

    #[test]
    fn empty_input_renders_with_no_items_note() {
        let md = render_markdown("empty.cplus", &[]);
        assert!(md.contains("_No documented `pub` items._"));
    }

    #[test]
    fn anchor_slug_is_url_safe() {
        assert_eq!(anchor("Point::new"), "point-new");
        assert_eq!(anchor("Vec[T, A]"), "vec-t-a");
        assert_eq!(anchor("simple"), "simple");
    }

    #[test]
    fn skips_attributes_between_doc_block_and_item() {
        let src = "\
/// Tagged with an attribute.
#[inline]
pub fn tagged() -> i32 { return 0; }
";
        let items = extract(src);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].name, "tagged");
    }
}
