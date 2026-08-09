//! Differential verification of `#[repr(C, packed)]` and `#[bits(N)]`
//! against clang (v0.0.28).
//!
//! A bitfield's layout is not something a language gets to decide. The
//! allocation unit, the straddling rule, the alignment a `u32:3` still
//! contributes, and the bit order inside the byte are the platform C
//! compiler's, and the only way to know cpc agrees is to ask it — which is
//! what this test does, on the same declarations, twice.
//!
//! Each shape is written once in C and once in C+, and both programs print
//! the same three things:
//!
//!   1. `sizeof` and `_Alignof`,
//!   2. the struct's BYTES after zeroing and writing one pattern per field —
//!      which pins every field's offset, bit position and width at once, in
//!      the only currency both compilers agree to speak,
//!   3. each field read back, so a signed bitfield's sign extension is
//!      compared too and not just the bits it occupies.
//!
//! Byte dumps rather than `offsetof` because a bitfield has no offset to ask
//! for: writing `1` into `a:3` and dumping the storage is the only portable
//! question that has an answer on both sides.
//!
//! Needs clang; skips (does not fail) without it, like `wasm_differential`.

use std::process::Command;

fn clang_available() -> bool {
    Command::new("clang")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// One shape, declared for both compilers plus the field writes to exercise.
struct Shape {
    name: &'static str,
    /// C declaration body, e.g. `unsigned a:3; unsigned b:5; unsigned char c;`
    c_decl: &'static str,
    /// C+ declaration, e.g. `#[repr(C)] struct A { #[bits(3)] a: u32, ... }`
    cplus_decl: &'static str,
    /// `(field, value)` pairs written in order. Values are chosen to overflow
    /// some widths on purpose — C truncates a bitfield store to its width and
    /// so must cpc.
    writes: &'static [(&'static str, i64)],
    /// Fields to read back, with `true` when the field is signed (so the C
    /// side prints it as a signed int and sign extension is compared).
    reads: &'static [(&'static str, bool)],
}

const SHAPES: &[Shape] = &[
    // Two bitfields sharing one 4-byte unit, then a plain byte after it.
    Shape {
        name: "A",
        c_decl: "unsigned a:3; unsigned b:5; unsigned char c;",
        cplus_decl: "#[repr(C)] struct A { #[bits(3)] a: u32, #[bits(5)] b: u32, c: u8 }",
        writes: &[("a", 5), ("b", 21), ("c", 200)],
        reads: &[("a", false), ("b", false), ("c", false)],
    },
    // The straddle rule: `b` does not fit in the unit `a` started, so it
    // begins a new one.
    Shape {
        name: "B",
        c_decl: "unsigned a:3; unsigned b:30;",
        cplus_decl: "#[repr(C)] struct B { #[bits(3)] a: u32, #[bits(30)] b: u32 }",
        writes: &[("a", 7), ("b", 1000000007)],
        reads: &[("a", false), ("b", false)],
    },
    // A bitfield that starts mid-byte after a plain field, and a 40-bit one
    // after that — the case whose storage spans six bytes.
    Shape {
        name: "C",
        c_decl: "unsigned char a; unsigned b:3; unsigned long long d:40;",
        cplus_decl:
            "#[repr(C)] struct C { a: u8, #[bits(3)] b: u32, #[bits(40)] d: u64 }",
        writes: &[("a", 0xAB), ("b", 6), ("d", 0x00DEADBEEF)],
        reads: &[("a", false), ("b", false), ("d", false)],
    },
    // Packing with no bitfields: every field at its declaration order with no
    // padding at all.
    Shape {
        name: "E",
        c_decl: "unsigned char a; unsigned b; unsigned short c;",
        cplus_decl: "#[repr(C, packed)] struct E { a: u8, b: u32, c: u16 }",
        writes: &[("a", 0x11), ("b", 0x22334455), ("c", 0x6677)],
        reads: &[("a", false), ("b", false), ("c", false)],
    },
    // Packed AND bitfielded: with no storage unit to respect, `b` straddles
    // the byte boundary instead of moving past it.
    Shape {
        name: "F",
        c_decl: "unsigned a:3; unsigned b:30; unsigned char c;",
        cplus_decl:
            "#[repr(C, packed)] struct F { #[bits(3)] a: u32, #[bits(30)] b: u32, c: u8 }",
        writes: &[("a", 5), ("b", 700000001), ("c", 0x5A)],
        reads: &[("a", false), ("b", false), ("c", false)],
    },
    // A signed bitfield: the value read back must sign-extend, not zero-fill.
    Shape {
        name: "G",
        c_decl: "signed s:3; unsigned u:3; signed t:11;",
        cplus_decl:
            "#[repr(C)] struct G { #[bits(3)] s: i32, #[bits(3)] u: u32, #[bits(11)] t: i32 }",
        writes: &[("s", -3), ("u", 5), ("t", -1000)],
        reads: &[("s", true), ("u", false), ("t", true)],
    },
    // `packed = N` caps alignment without flattening it: `b` lands at 2, not
    // at 1 (packed) and not at 4 (natural).
    Shape {
        name: "H",
        c_decl: "unsigned char a; unsigned b; double d;",
        cplus_decl: "#[repr(C, packed = 2)] struct H { a: u8, b: u32, d: f64 }",
        writes: &[("a", 3), ("b", 0x0BADF00D)],
        reads: &[("a", false), ("b", false)],
    },
    // Truncation: a value wider than the field keeps only the low bits, and
    // the neighbour it shares the unit with is untouched.
    Shape {
        name: "I",
        c_decl: "unsigned lo:4; unsigned hi:4;",
        cplus_decl: "#[repr(C)] struct I { #[bits(4)] lo: u32, #[bits(4)] hi: u32 }",
        writes: &[("lo", 0xFF), ("hi", 0x3)],
        reads: &[("lo", false), ("hi", false)],
    },
];

fn c_program() -> String {
    let mut s = String::from("#include <stdio.h>\n#include <string.h>\n");
    for sh in SHAPES {
        let decl = match sh.name {
            "E" | "F" => format!(
                "struct __attribute__((packed)) {} {{ {} }};\n",
                sh.name, sh.c_decl
            ),
            "H" => format!(
                "#pragma pack(2)\nstruct {} {{ {} }};\n#pragma pack()\n",
                sh.name, sh.c_decl
            ),
            _ => format!("struct {} {{ {} }};\n", sh.name, sh.c_decl),
        };
        s.push_str(&decl);
    }
    s.push_str("int main(void) {\n");
    for sh in SHAPES {
        let n = sh.name;
        s.push_str(&format!("  {{\n    struct {n} v; memset(&v, 0, sizeof v);\n"));
        s.push_str(&format!(
            "    printf(\"%d\\n\", (int)sizeof v);\n    printf(\"%d\\n\", (int)_Alignof(struct {n}));\n"
        ));
        for (f, val) in sh.writes {
            s.push_str(&format!("    v.{f} = ({val}LL);\n"));
        }
        s.push_str(
            "    { const unsigned char *p = (const unsigned char *)&v;\n      for (unsigned i = 0; i < sizeof v; i++) printf(\"%d\\n\", (int)p[i]); }\n",
        );
        for (f, _signed) in sh.reads {
            s.push_str(&format!(
                "    {{ unsigned long long t = (unsigned long long)(long long)v.{f};\n      printf(\"%d\\n\", (int)(t & 0xFFFFULL));\n      printf(\"%d\\n\", (int)((t >> 16) & 0xFFFFULL));\n      printf(\"%d\\n\", (int)((t >> 32) & 0xFFFFULL)); }}\n"
            ));
        }
        s.push_str("  }\n");
    }
    s.push_str("  return 0;\n}\n");
    s
}

fn cplus_program() -> String {
    let mut s = String::new();
    for sh in SHAPES {
        s.push_str(sh.cplus_decl);
        s.push('\n');
    }
    s.push_str("fn main() -> i32 {\n");
    for sh in SHAPES {
        let n = sh.name;
        s.push_str(&format!("  {{\n    var v: {n} = #zero::[{n}]();\n"));
        s.push_str(&format!(
            "    #println(#size_of::[{n}]() as i32);\n    #println(#align_of::[{n}]() as i32);\n"
        ));
        for (f, val) in sh.writes {
            // Written in the field's own declared type: the cast IS the
            // truncation C performs on assignment into a narrow bitfield.
            s.push_str(&format!("    v.{f} = ({val} as i64) as _FIELDTY_{n}_{f};\n"));
        }
        s.push_str(&format!(
            "    {{\n      let p: *u8 = #addr_of(v) as *u8;\n      var i: usize = 0 as usize;\n      while i < #size_of::[{n}]() {{\n        #println(p[i] as i32);\n        i = i + (1 as usize);\n      }}\n    }}\n"
        ));
        for (f, _signed) in sh.reads {
            s.push_str(&format!(
                "    {{\n      let t: u64 = (v.{f} as i64) as u64;\n      #println((t & (65535 as u64)) as i32);\n      #println(((t >> (16 as u64)) & (65535 as u64)) as i32);\n      #println(((t >> (32 as u64)) & (65535 as u64)) as i32);\n    }}\n"
            ));
        }
        s.push_str("  }\n");
    }
    s.push_str("  return 0;\n}\n");
    s
}

/// Resolve the `_FIELDTY_<struct>_<field>` placeholders from the C+
/// declarations, so a field's cast is written in exactly the type it was
/// declared with and the table stays the only place a type appears.
fn resolve_field_types(mut prog: String) -> String {
    for sh in SHAPES {
        for (f, _) in sh.writes {
            let decl = sh.cplus_decl;
            let key = format!("{f}: ");
            let ty = decl
                .find(&key)
                .map(|i| {
                    let rest = &decl[i + key.len()..];
                    let end = rest
                        .find(|c: char| c == ',' || c == '}')
                        .unwrap_or(rest.len());
                    rest[..end].trim().to_string()
                })
                .unwrap_or_else(|| panic!("field `{f}` not found in `{decl}`"));
            prog = prog.replace(&format!("_FIELDTY_{}_{}", sh.name, f), &ty);
        }
    }
    prog
}

#[test]
fn packed_and_bitfield_layout_matches_clang() {
    if !clang_available() {
        eprintln!("skipping: no clang");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");

    let cpath = dir.path().join("shapes.c");
    std::fs::write(&cpath, c_program()).expect("write C");
    let cbin = dir.path().join("shapes_c");
    let st = Command::new("clang")
        .arg(&cpath)
        .arg("-o")
        .arg(&cbin)
        .arg("-w")
        .status()
        .expect("invoke clang");
    assert!(st.success(), "C reference failed to build");
    let c_out = Command::new(&cbin).output().expect("run C reference");
    let c_out = String::from_utf8_lossy(&c_out.stdout).into_owned();

    let src = resolve_field_types(cplus_program());
    let srcp = dir.path().join("shapes.cplus");
    std::fs::write(&srcp, &src).expect("write C+");
    let pbin = dir.path().join("shapes_cplus");
    let out = Command::new(env!("CARGO_BIN_EXE_cpc"))
        .arg(&srcp)
        .arg("-o")
        .arg(&pbin)
        .output()
        .expect("invoke cpc");
    assert!(
        out.status.success(),
        "C+ build failed:\n{}\n--- source ---\n{src}",
        String::from_utf8_lossy(&out.stderr)
    );
    let p_out = Command::new(&pbin).output().expect("run C+ program");
    let p_out = String::from_utf8_lossy(&p_out.stdout).into_owned();

    if c_out != p_out {
        let c_lines: Vec<&str> = c_out.lines().collect();
        let p_lines: Vec<&str> = p_out.lines().collect();
        let mut diff = String::new();
        for i in 0..c_lines.len().max(p_lines.len()) {
            let c = c_lines.get(i).copied().unwrap_or("<missing>");
            let p = p_lines.get(i).copied().unwrap_or("<missing>");
            if c != p {
                diff.push_str(&format!("  clang: {c}\n  cpc  : {p}\n"));
            }
        }
        panic!("layout differs from clang:\n{diff}");
    }
    // Guard against both sides printing nothing and "matching".
    assert!(
        c_out.lines().count() >= SHAPES.len() * 3,
        "reference output looks empty:\n{c_out}"
    );
}
