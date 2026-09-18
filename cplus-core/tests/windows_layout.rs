//! Exercise the Windows layout on every host. This integration-test process
//! owns its target state; it cannot change the target of parallel unit tests.
use cplus_core::{lexer, parser, sema, target};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
fn windows_and_sysv_bitfield_layouts_are_distinct() {
    let src = r#"
        #[repr(C)] struct A { #[bits(3)] a: u32, #[bits(5)] b: u32, c: u8 }
        #[repr(C)] struct B { #[bits(3)] a: u32, #[bits(30)] b: u32 }
        #[repr(C)] struct C { a: u8, #[bits(3)] b: u32, #[bits(40)] d: u64 }
        #[repr(C, packed)] struct F { #[bits(3)] a: u32, #[bits(30)] b: u32, c: u8 }
        #[repr(C)] struct Mixed { #[bits(3)] a: u16, #[bits(3)] b: u32 }
        #[repr(C, packed = 2)] struct Packed { tag: u8, #[bits(3)] a: u32, tail: u8 }
        fn main() -> i32 { return 0; }
    "#;
    let program = parser::parse(lexer::tokenize(src).unwrap()).unwrap();
    for (os, expected) in [
        (
            target::TargetOs::Windows,
            vec![
                ("A", 8, 4),
                ("B", 8, 4),
                ("C", 16, 8),
                ("F", 9, 1),
                ("Mixed", 8, 4),
                ("Packed", 8, 2),
            ],
        ),
        (
            target::TargetOs::Linux,
            vec![
                ("A", 4, 4),
                ("B", 8, 4),
                ("C", 8, 8),
                ("F", 6, 1),
                ("Mixed", 4, 4),
                ("Packed", 4, 2),
            ],
        ),
    ] {
        target::set_active_target(target::TargetSpec {
            arch: target::TargetArch::X86_64,
            os,
            ..target::HOST
        });
        let layouts =
            sema::classify_layout(&program, PathBuf::from("test.cplus"), src, BTreeMap::new());
        for (name, size, align) in expected {
            assert!(
                layouts.contains(&(name.to_string(), size, align)),
                "{os:?}: expected {name} {size}/{align}, got {layouts:?}"
            );
        }
    }
}
