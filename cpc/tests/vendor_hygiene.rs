//! Repo hygiene: vendor sources must stay TEXT.
//!
//! A `.cplus` file containing a raw 0x00 byte still compiles — the lexer is
//! byte-oriented and does not care — but every POSIX text tool downstream does.
//! BSD `grep` classifies such a file as binary and then answers NOTHING for a
//! pattern that is plainly in it: no match line, no "binary file matches", exit
//! 1. `file` calls it `data`; GitHub renders it as binary in diffs.
//!
//! That cost a wrong diagnosis on 2026-08-17 —
//! `vendor/agent_uikit/src/agent_uikit.cplus` had 21 literal NULs where every
//! sibling backend writes the two-character escape `\0`, and searching it for a
//! function that was right there came back empty, which read as "the package
//! must be compiling from a stale copy". It was not.
//!
//! The escape and the raw byte compile to the same thing: rewriting all 21
//! produced a byte-for-byte identical `agent_uikit.o`. So there is never a
//! reason to spend the raw byte, and this test says so.

use std::fs;
use std::path::{Path, PathBuf};

/// The repo root — `cpc/`'s parent.
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cpc/ has a parent")
        .to_path_buf()
}

/// Every `.cplus` under `dir`, skipping build output (`target/`, `lib/`) — those
/// hold generated copies whose contents this test does not govern.
///
/// SYMLINKS ARE NOT FOLLOWED, and that is load-bearing rather than tidy.
/// `vendor/agent_uikit/vendor -> ../../vendor` points at the tree this walk
/// started from, so following it recurses
/// `vendor/agent_uikit/vendor/agent_uikit/vendor/…` with no bound. Written with
/// `Path::is_dir` — which follows links — this test grew to 2 GB resident and
/// was still climbing when it was killed. `DirEntry::file_type` answers about
/// the link itself, which is the question worth asking here anyway: a symlinked
/// tree is not this package's source, and whatever it points at is walked once
/// under its own real path.
fn cplus_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let p = e.path();
        if ft.is_dir() {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if matches!(name, "target" | "lib" | ".git") {
                continue;
            }
            cplus_sources(&p, out);
        } else if p.extension().and_then(|x| x.to_str()) == Some("cplus") {
            out.push(p);
        }
    }
}

#[test]
fn vendor_cplus_sources_contain_no_raw_nul_bytes() {
    let root = repo_root();
    let vendor = root.join("vendor");
    if !vendor.is_dir() {
        // A packaged/checkout-less build has no vendor tree. Nothing to police.
        return;
    }

    let mut files = Vec::new();
    cplus_sources(&vendor, &mut files);
    assert!(
        files.len() > 50,
        "expected to find the vendor sources, walked only {} files — did the \
         layout move?",
        files.len()
    );

    let mut offenders = Vec::new();
    for f in &files {
        let Ok(bytes) = fs::read(f) else { continue };
        let count = bytes.iter().filter(|b| **b == 0).count();
        if count == 0 {
            continue;
        }
        // Name the first one by line so the fix is a jump, not a search — which
        // is the whole point, given that grep will not find it.
        let first = bytes.iter().position(|b| *b == 0).unwrap();
        let line = bytes[..first].iter().filter(|b| **b == b'\n').count() + 1;
        offenders.push(format!(
            "  {}: {count} raw NUL byte(s), first at byte {first} (line {line})",
            f.strip_prefix(&root).unwrap_or(f).display()
        ));
    }

    assert!(
        offenders.is_empty(),
        "vendor sources must be text — write NUL as the escape `\\0`, which \
         compiles identically:\n{}",
        offenders.join("\n")
    );
}
