//! v0.0.21 multi-backend slice 1: the target model.
//!
//! `cpc` is an IR/object producer with a pluggable handoff, not a host-native
//! linker (see `plans/plan.backends.md`). A [`TargetSpec`] carries everything
//! driver and codegen need to produce target-correct output: the clang
//! triple, pointer width, endianness, object format, the ABI/intrinsic
//! selectors (`arch` / `os`), and the handoff mode (link on the host, or stop
//! at object emission for an external builder such as Xcode).
//!
//! The active target is process-global state set once by the driver before
//! codegen — the same pattern as `codegen::set_coro_end_returns_void`. The
//! default is [`HOST`], which reproduces today's `cfg!`-derived behavior
//! byte-for-byte, so every path that never sees `--target` is unchanged.
//! Unit tests that need a non-host target use the `*_for` codegen helpers
//! with an explicit spec instead of mutating the global (tests run in
//! parallel in one process).

use std::sync::Mutex;

/// CPU architecture, as codegen's ABI classifier and intrinsic gating see it.
/// C+ supports 64-bit little-endian targets today; 32-bit (ESP32) arrives in
/// a later rung of the backends plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetArch {
    Aarch64,
    X86_64,
}

/// Operating system, as codegen's ABI classifier and the driver's link
/// behavior see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    Macos,
    Linux,
    Windows,
    Ios,
}

/// Relocatable-object container format the target's toolchain consumes.
/// Informational for now (clang derives it from the triple); carried so
/// driver-side packaging decisions don't re-derive it from strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFormat {
    MachO,
    Elf,
    Coff,
}

/// Who runs the final link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    /// cpc invokes the host clang to link an executable / shared library.
    HostLink,
    /// cpc stops at object / static-archive emission; an external build
    /// system (Xcode, NDK, ESP-IDF) owns the final link and packaging.
    ExternalBuilder,
}

/// One compilation target. `Copy` and built from `&'static` strings so the
/// active spec can live in cheap process-global state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TargetSpec {
    /// The name the user passes to `--target` (`ios-arm64`, ...). The host
    /// spec is named `host`.
    pub name: &'static str,
    pub arch: TargetArch,
    pub os: TargetOs,
    /// Pointer width in bits. 64 for every current target; carried so the
    /// 32-bit rung (ESP32) lands as data, not a new mechanism.
    pub pointer_width: u32,
    pub little_endian: bool,
    pub object_format: ObjectFormat,
    /// Full clang `-target` triple, also emitted as the IR-level
    /// `target triple`. `None` = host: clang's own default, and no triple
    /// line in the IR (today's output, byte-for-byte).
    pub triple: Option<&'static str>,
    /// Stable directory name for vendor bundled-artifact lookup
    /// (`vendor/<dep>/src/lib/<artifact-triple>/` and `[link].triples`).
    /// Unversioned, unlike `triple` (no `13.0`) — packages ship one binary
    /// per target, not per minimum OS version. `None` = host: the dep
    /// walker uses `clang -print-target-triple` as before.
    pub artifact_triple: Option<&'static str>,
    /// Apple SDK name for `xcrun --sdk <name> --show-sdk-path`, when the
    /// target wants `-isysroot` on clang invocations.
    pub apple_sdk: Option<&'static str>,
    pub handoff: Handoff,
}

/// The host target: everything `--target`-less compilation did before
/// v0.0.21, expressed as data. Arch/OS come from compile-time `cfg!`, so the
/// runtime checks in codegen resolve exactly like the `cfg!` gates they
/// replaced.
pub const HOST: TargetSpec = TargetSpec {
    name: "host",
    arch: if cfg!(target_arch = "aarch64") {
        TargetArch::Aarch64
    } else {
        TargetArch::X86_64
    },
    os: if cfg!(target_os = "macos") {
        TargetOs::Macos
    } else if cfg!(windows) {
        TargetOs::Windows
    } else {
        TargetOs::Linux
    },
    pointer_width: 64,
    little_endian: true,
    object_format: if cfg!(target_os = "macos") {
        ObjectFormat::MachO
    } else if cfg!(windows) {
        ObjectFormat::Coff
    } else {
        ObjectFormat::Elf
    },
    triple: None,
    artifact_triple: None,
    apple_sdk: None,
    handoff: Handoff::HostLink,
};

/// iOS device (arm64). Reuses the host clang family on macOS; the
/// `arm64-apple-ios` ABI matches the existing `arm64-apple-darwin`
/// classification. The `13.0` minimum is the floor Xcode templates target;
/// it lands in the object's `LC_BUILD_VERSION`.
pub const IOS_ARM64: TargetSpec = TargetSpec {
    name: "ios-arm64",
    arch: TargetArch::Aarch64,
    os: TargetOs::Ios,
    pointer_width: 64,
    little_endian: true,
    object_format: ObjectFormat::MachO,
    triple: Some("arm64-apple-ios13.0"),
    artifact_triple: Some("arm64-apple-ios"),
    apple_sdk: Some("iphoneos"),
    handoff: Handoff::ExternalBuilder,
};

/// iOS simulator (arm64) — the cheap validation loop: no device, no signing.
pub const IOS_ARM64_SIMULATOR: TargetSpec = TargetSpec {
    name: "ios-arm64-simulator",
    arch: TargetArch::Aarch64,
    os: TargetOs::Ios,
    pointer_width: 64,
    little_endian: true,
    object_format: ObjectFormat::MachO,
    triple: Some("arm64-apple-ios13.0-simulator"),
    artifact_triple: Some("arm64-apple-ios-simulator"),
    apple_sdk: Some("iphonesimulator"),
    handoff: Handoff::ExternalBuilder,
};

/// Every named target `--target` accepts, in the order help text lists them.
pub const SUPPORTED: &[TargetSpec] = &[HOST, IOS_ARM64, IOS_ARM64_SIMULATOR];

impl TargetSpec {
    /// Resolve a `--target` name. `None` means unknown — the driver owns the
    /// diagnostic (it lists [`supported_names`]).
    pub fn from_name(name: &str) -> Option<TargetSpec> {
        SUPPORTED.iter().copied().find(|t| t.name == name)
    }

    /// Whether this spec is the host (no `-target` flag, no IR triple line,
    /// artifact lookup by `clang -print-target-triple`).
    pub fn is_host(&self) -> bool {
        self.triple.is_none()
    }
}

/// Comma-joined list of the names `--target` accepts, for diagnostics.
pub fn supported_names() -> String {
    SUPPORTED
        .iter()
        .map(|t| t.name)
        .collect::<Vec<_>>()
        .join(", ")
}

static ACTIVE: Mutex<TargetSpec> = Mutex::new(HOST);

/// Install the target the driver resolved from `--target`. Call before any
/// `codegen::generate*`. Defaults to [`HOST`] when never called.
pub fn set_active_target(t: TargetSpec) {
    *ACTIVE.lock().unwrap() = t;
}

/// The target codegen is emitting for. Reads are uncontended (the driver
/// writes once, before codegen).
pub fn active_target() -> TargetSpec {
    *ACTIVE.lock().unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_name_resolves_every_supported_target() {
        for spec in SUPPORTED {
            let hit = TargetSpec::from_name(spec.name);
            assert_eq!(hit, Some(*spec), "from_name should resolve `{}`", spec.name);
        }
    }

    #[test]
    fn from_name_rejects_unknown_names() {
        assert_eq!(TargetSpec::from_name("ios"), None);
        assert_eq!(TargetSpec::from_name("arm64-apple-ios"), None);
        assert_eq!(TargetSpec::from_name(""), None);
        assert_eq!(TargetSpec::from_name("HOST"), None);
    }

    #[test]
    fn host_spec_matches_compile_time_cfg() {
        assert!(HOST.is_host());
        assert_eq!(HOST.triple, None);
        assert_eq!(HOST.artifact_triple, None);
        assert_eq!(HOST.handoff, Handoff::HostLink);
        assert_eq!(HOST.pointer_width, 64);
        if cfg!(target_arch = "aarch64") {
            assert_eq!(HOST.arch, TargetArch::Aarch64);
        }
        if cfg!(target_arch = "x86_64") {
            assert_eq!(HOST.arch, TargetArch::X86_64);
        }
        if cfg!(target_os = "macos") {
            assert_eq!(HOST.os, TargetOs::Macos);
            assert_eq!(HOST.object_format, ObjectFormat::MachO);
        }
        if cfg!(target_os = "linux") {
            assert_eq!(HOST.os, TargetOs::Linux);
            assert_eq!(HOST.object_format, ObjectFormat::Elf);
        }
        if cfg!(windows) {
            assert_eq!(HOST.os, TargetOs::Windows);
            assert_eq!(HOST.object_format, ObjectFormat::Coff);
        }
    }

    #[test]
    fn ios_specs_stop_at_object_emission() {
        for spec in [IOS_ARM64, IOS_ARM64_SIMULATOR] {
            assert!(!spec.is_host());
            assert_eq!(spec.handoff, Handoff::ExternalBuilder);
            assert_eq!(spec.arch, TargetArch::Aarch64);
            assert_eq!(spec.os, TargetOs::Ios);
            assert_eq!(spec.object_format, ObjectFormat::MachO);
            let triple = spec.triple.unwrap();
            assert!(triple.starts_with("arm64-apple-ios"));
            // The artifact triple is the unversioned directory name.
            let artifact = spec.artifact_triple.unwrap();
            assert!(!artifact.contains("13.0"));
            assert!(spec.apple_sdk.is_some());
        }
        assert!(IOS_ARM64_SIMULATOR.triple.unwrap().ends_with("-simulator"));
        assert_eq!(IOS_ARM64.apple_sdk, Some("iphoneos"));
        assert_eq!(IOS_ARM64_SIMULATOR.apple_sdk, Some("iphonesimulator"));
    }

    #[test]
    fn supported_names_lists_every_target_once() {
        let names = supported_names();
        assert_eq!(names, "host, ios-arm64, ios-arm64-simulator");
    }

    #[test]
    fn active_target_defaults_to_host() {
        // Other tests must not mutate the global (they use the `*_for`
        // helpers with explicit specs), so the default is observable here.
        assert!(active_target().is_host());
    }
}
