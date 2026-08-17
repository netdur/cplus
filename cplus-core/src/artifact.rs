//! What a built artifact actually IS, read from its own bytes.
//!
//! A prebuild fingerprint is computed from **inputs** — source text, triple,
//! debug/release, compiler version. That is the right thing to hash to decide
//! *whether to rebuild*, and it is structurally blind to the one question this
//! module answers: are the bytes sitting in `lib/<triple>/` actually FOR that
//! triple?
//!
//! They were not, once. `ensure_one_slice` copied a cross-target archive out of
//! the host artifact directory, so a macOS archive landed in
//! `lib/arm64-apple-ios-simulator/` with a perfectly valid fingerprint beside
//! it — and a valid fingerprint means "current", so it was reused forever.
//! Twelve slices were in that state before anything complained, and what
//! finally complained was the linker, three packages downstream:
//!
//! ```text
//! ld: building for 'iOS-simulator', but linking in object file
//!     (…/lib/arm64-apple-ios-simulator/libagent_core.a[2](agent_core.o))
//!     built for 'macOS'
//! ```
//!
//! See `bugs/closed/prebuild-put-host-archives-in-cross-triple-slots.md`. The
//! path bug is fixed; this exists so the *class* of failure is a rebuild with a
//! reason rather than a link error with none.
//!
//! ## The conservative rule
//!
//! Every entry point returns `Option`, and `None` means **"could not tell"**,
//! never "wrong". A caller must only act on a positive mismatch — two tags that
//! are both known and differ. Treating "could not tell" as a mismatch would
//! rebuild the slice, fail to tell any more the second time, and rebuild
//! forever.

use crate::target::{ObjectFormat, TargetArch, TargetOs, TargetSpec};
use std::path::Path;

/// One canonical architecture spelling, shared by every object format, so a
/// tag read out of Mach-O compares equal to one derived from a `TargetSpec`.
fn arch_name(a: TargetArch) -> &'static str {
    match a {
        TargetArch::Aarch64 => "arm64",
        TargetArch::X86_64 => "x86_64",
        TargetArch::Xtensa => "xtensa",
        TargetArch::Riscv32 => "riscv32",
        TargetArch::Wasm32 => "wasm32",
    }
}

/// The tag an artifact built for `t` must carry.
///
/// `None` for targets whose format records nothing worth checking (wasm), which
/// switches the check off rather than failing it.
pub fn expected_tag(t: &TargetSpec) -> Option<String> {
    let arch = arch_name(t.arch);
    match t.object_format {
        ObjectFormat::MachO => {
            // The simulator is a distinct Mach-O platform from the device, and
            // an identical architecture — arm64 either way on Apple silicon.
            // Nothing but this field separates them, which is exactly why the
            // host/simulator mix-up linked cleanly into the archive and only
            // failed later.
            let sim = t.triple.map(|s| s.contains("simulator")).unwrap_or(false);
            let plat = match (t.os, sim) {
                (TargetOs::Macos, _) => "macos",
                (TargetOs::Ios, false) => "ios",
                (TargetOs::Ios, true) => "ios-simulator",
                _ => return None,
            };
            Some(format!("macho/{arch}/{plat}"))
        }
        // ELF and COFF carry an architecture but no OS worth trusting (Android
        // and Linux are both `ELFOSABI_SYSV`), so the tag is the architecture.
        // That still catches every cross-arch mix-up, which is the whole of the
        // risk on those targets.
        ObjectFormat::Elf => Some(format!("elf/{arch}")),
        ObjectFormat::Coff => Some(format!("coff/{arch}")),
        ObjectFormat::Wasm => None,
    }
}

/// Read the tag out of a static archive (or a bare object file).
///
/// `None` when the file is unreadable, is not a format this understands, or
/// carries no platform record — see the conservative rule above.
pub fn tag_of_file(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    tag_of_bytes(&bytes)
}

/// The parsing half, split out so it is testable without touching a disk.
pub fn tag_of_bytes(bytes: &[u8]) -> Option<String> {
    let obj = first_object(bytes)?;
    if let Some((cputype, platform)) = macho_info(obj) {
        let arch = macho_arch(cputype)?;
        let plat = macho_platform_name(platform?)?;
        return Some(format!("macho/{arch}/{plat}"));
    }
    if let Some(arch) = elf_arch(obj) {
        return Some(format!("elf/{arch}"));
    }
    if let Some(arch) = coff_arch(obj) {
        return Some(format!("coff/{arch}"));
    }
    None
}

// ---- ar ---------------------------------------------------------------------

/// The first real member of a `!<arch>` archive — the symbol table and the
/// long-name table are not objects and are skipped. A file that is not an
/// archive at all is returned whole, so a bare `.o` works too.
fn first_object(bytes: &[u8]) -> Option<&[u8]> {
    const MAGIC: &[u8] = b"!<arch>\n";
    if !bytes.starts_with(MAGIC) {
        return Some(bytes);
    }
    let mut off = MAGIC.len();
    // A malformed archive must not spin: every iteration advances `off` past a
    // header, and the bound stops a zero-size cycle from running away.
    for _ in 0..10_000 {
        if off + 60 > bytes.len() {
            return None;
        }
        let hdr = &bytes[off..off + 60];
        if &hdr[58..60] != b"`\n" {
            return None;
        }
        let raw_name = std::str::from_utf8(&hdr[0..16]).ok()?.trim_end().to_string();
        let size: usize = std::str::from_utf8(&hdr[48..58]).ok()?.trim().parse().ok()?;
        let data_start = off + 60;
        let data_end = data_start.checked_add(size)?;
        if data_end > bytes.len() {
            return None;
        }
        let mut data = &bytes[data_start..data_end];
        let mut name = raw_name.clone();
        // BSD long names (what Apple's `ar` writes): the header name is
        // `#1/<len>` and the real name occupies the first <len> bytes of the
        // member data, counted inside `size`.
        if let Some(rest) = raw_name.strip_prefix("#1/") {
            let n: usize = rest.trim().parse().ok()?;
            if n > data.len() {
                return None;
            }
            name = String::from_utf8_lossy(&data[..n])
                .trim_end_matches('\0')
                .to_string();
            data = &data[n..];
        }
        let is_index = name.starts_with("__.SYMDEF") || name == "/" || name == "//" || name.is_empty();
        if !is_index {
            return Some(data);
        }
        // Members start on an even offset.
        off = data_end + (data_end & 1);
    }
    None
}

// ---- Mach-O -----------------------------------------------------------------

fn u32le(b: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    if end > b.len() {
        return None;
    }
    Some(u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]))
}

/// `(cputype, platform)`. Platform is `None` on an object too old to carry one.
fn macho_info(o: &[u8]) -> Option<(u32, Option<u32>)> {
    const MH_MAGIC_64: u32 = 0xfeed_facf;
    const MH_MAGIC_32: u32 = 0xfeed_face;
    const LC_BUILD_VERSION: u32 = 0x32;
    const LC_VERSION_MIN_MACOSX: u32 = 0x24;
    const LC_VERSION_MIN_IPHONEOS: u32 = 0x25;

    let is64 = match u32le(o, 0)? {
        MH_MAGIC_64 => true,
        MH_MAGIC_32 => false,
        _ => return None,
    };
    let cputype = u32le(o, 4)?;
    let ncmds = u32le(o, 16)?;
    let mut off = if is64 { 32 } else { 28 };
    let mut platform = None;
    for _ in 0..ncmds.min(10_000) {
        let cmd = u32le(o, off)?;
        let cmdsize = u32le(o, off + 4)? as usize;
        if cmdsize < 8 {
            return None;
        }
        match cmd {
            // Authoritative and unambiguous — the only load command that tells
            // the simulator apart from the device.
            LC_BUILD_VERSION => platform = u32le(o, off + 8),
            // Pre-LC_BUILD_VERSION fallbacks. `LC_VERSION_MIN_IPHONEOS` was
            // emitted for simulator objects too, so it cannot distinguish them;
            // it is only ever a last resort, never allowed to overwrite a real
            // LC_BUILD_VERSION.
            LC_VERSION_MIN_MACOSX => platform = platform.or(Some(1)),
            LC_VERSION_MIN_IPHONEOS => platform = platform.or(Some(2)),
            _ => {}
        }
        off = off.checked_add(cmdsize)?;
        if off > o.len() {
            return None;
        }
    }
    Some((cputype, platform))
}

fn macho_arch(cputype: u32) -> Option<&'static str> {
    match cputype {
        0x0100_000c => Some("arm64"),
        0x0100_0007 => Some("x86_64"),
        _ => None,
    }
}

fn macho_platform_name(p: u32) -> Option<&'static str> {
    match p {
        1 => Some("macos"),
        2 => Some("ios"),
        3 => Some("tvos"),
        4 => Some("watchos"),
        6 => Some("maccatalyst"),
        7 => Some("ios-simulator"),
        8 => Some("tvos-simulator"),
        9 => Some("watchos-simulator"),
        _ => None,
    }
}

// ---- ELF / COFF -------------------------------------------------------------

fn elf_arch(o: &[u8]) -> Option<&'static str> {
    if o.len() < 20 || &o[0..4] != b"\x7fELF" {
        return None;
    }
    let is64 = o[4] == 2;
    // e_machine: u16 at offset 18, little-endian (EI_DATA == 1).
    if o[5] != 1 {
        return None;
    }
    let machine = u16::from_le_bytes([o[18], o[19]]);
    match machine {
        183 => Some("arm64"),
        62 => Some("x86_64"),
        94 => Some("xtensa"),
        243 => Some(if is64 { "riscv64" } else { "riscv32" }),
        _ => None,
    }
}

fn coff_arch(o: &[u8]) -> Option<&'static str> {
    if o.len() < 2 {
        return None;
    }
    match u16::from_le_bytes([o[0], o[1]]) {
        0xaa64 => Some("arm64"),
        0x8664 => Some("x86_64"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::target;

    #[test]
    fn expected_tag_separates_the_simulator_from_the_device() {
        // The whole point: same architecture, and nothing but the platform
        // field to tell them apart.
        let dev = expected_tag(&target::IOS_ARM64).unwrap();
        let sim = expected_tag(&target::IOS_ARM64_SIMULATOR).unwrap();
        assert_eq!(dev, "macho/arm64/ios");
        assert_eq!(sim, "macho/arm64/ios-simulator");
        assert_ne!(dev, sim);
    }

    #[test]
    fn a_host_macos_archive_does_not_match_an_ios_slot() {
        // The exact confusion that shipped twelve wrong archives.
        let host = "macho/arm64/macos";
        assert_ne!(host, expected_tag(&target::IOS_ARM64_SIMULATOR).unwrap());
        assert_ne!(host, expected_tag(&target::IOS_ARM64).unwrap());
    }

    /// A minimal 64-bit Mach-O object carrying one `LC_BUILD_VERSION`.
    fn macho(cputype: u32, platform: u32) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&0xfeed_facfu32.to_le_bytes()); // magic
        o.extend_from_slice(&cputype.to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes()); // cpusubtype
        o.extend_from_slice(&1u32.to_le_bytes()); // filetype
        o.extend_from_slice(&1u32.to_le_bytes()); // ncmds
        o.extend_from_slice(&24u32.to_le_bytes()); // sizeofcmds
        o.extend_from_slice(&0u32.to_le_bytes()); // flags
        o.extend_from_slice(&0u32.to_le_bytes()); // reserved
        o.extend_from_slice(&0x32u32.to_le_bytes()); // LC_BUILD_VERSION
        o.extend_from_slice(&24u32.to_le_bytes()); // cmdsize
        o.extend_from_slice(&platform.to_le_bytes());
        o.extend_from_slice(&0u32.to_le_bytes()); // minos
        o.extend_from_slice(&0u32.to_le_bytes()); // sdk
        o.extend_from_slice(&0u32.to_le_bytes()); // ntools
        o
    }

    #[test]
    fn a_bare_macho_object_reads_its_platform() {
        assert_eq!(
            tag_of_bytes(&macho(0x0100_000c, 7)).unwrap(),
            "macho/arm64/ios-simulator"
        );
        assert_eq!(
            tag_of_bytes(&macho(0x0100_000c, 1)).unwrap(),
            "macho/arm64/macos"
        );
    }

    /// Wrap members in a BSD `ar` archive with a `__.SYMDEF` index first, which
    /// is what Apple's `ar` writes and therefore what must be skipped.
    fn ar(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut a = b"!<arch>\n".to_vec();
        for (name, data) in members {
            // Long-name form, so the skip logic is exercised the way real
            // archives exercise it.
            let name_bytes = name.as_bytes();
            let size = name_bytes.len() + data.len();
            let hdr = format!(
                "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
                format!("#1/{}", name_bytes.len()),
                0,
                0,
                0,
                100644,
                size
            );
            a.extend_from_slice(hdr.as_bytes());
            a.extend_from_slice(name_bytes);
            a.extend_from_slice(data);
            if a.len() % 2 == 1 {
                a.push(b'\n');
            }
        }
        a
    }

    #[test]
    fn an_archive_is_read_past_its_symbol_table() {
        let obj = macho(0x0100_000c, 7);
        let archive = ar(&[("__.SYMDEF SORTED", &[0u8; 8]), ("thing.o", &obj)]);
        assert_eq!(
            tag_of_bytes(&archive).unwrap(),
            "macho/arm64/ios-simulator",
            "the index member must be skipped, not parsed as an object"
        );
    }

    #[test]
    fn unreadable_input_answers_could_not_tell_rather_than_wrong() {
        // The conservative rule. Anything else rebuilds forever.
        assert_eq!(tag_of_bytes(b""), None);
        assert_eq!(tag_of_bytes(b"not an object at all"), None);
        assert_eq!(tag_of_bytes(b"!<arch>\ntruncated"), None);
    }

    #[test]
    fn elf_and_coff_report_their_architecture() {
        let mut elf = vec![0u8; 24];
        elf[0..4].copy_from_slice(b"\x7fELF");
        elf[4] = 2; // 64-bit
        elf[5] = 1; // little-endian
        elf[18] = 183; // EM_AARCH64
        assert_eq!(tag_of_bytes(&elf).unwrap(), "elf/arm64");

        let coff = vec![0x64u8, 0xaa, 0, 0];
        assert_eq!(tag_of_bytes(&coff).unwrap(), "coff/arm64");
    }
}
