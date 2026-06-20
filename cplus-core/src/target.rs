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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetArch {
    Aarch64,
    X86_64,
    /// Xtensa LX6/LX7 (ESP32 classic / S2 / S3): 32-bit, windowed ABI, no
    /// FP registers for doubles. Not in mainline LLVM — objects come from
    /// Espressif's esp-clang.
    Xtensa,
    /// RISC-V RV32 (ESP32-C3/C6/H2 class), ilp32 ABI. Mainline LLVM;
    /// compiled here with esp-clang for the ESP-IDF pairing.
    Riscv32,
    /// WebAssembly (`wasm32`). 32-bit, little-endian, structured control
    /// flow. Unlike every other arch, cpc emits the final artifact itself
    /// (WebAssembly text via `wasm_emit`) — there is no external clang. Used
    /// by the browser playground; see `plans/plan.wasm-playground.md`.
    Wasm32,
}

/// Operating system, as codegen's ABI classifier and the driver's link
/// behavior see it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetOs {
    Macos,
    Linux,
    Windows,
    Ios,
    Android,
    /// ESP-IDF (FreeRTOS + newlib). Not a POSIX desktop OS: no processes,
    /// no kqueue/epoll, heap discouraged in real-time contexts.
    EspIdf,
    /// No OS — freestanding `wasm32-unknown-unknown`. The host environment
    /// (the browser, via JS imports) supplies the libc floor; there is no
    /// syscall layer, no processes, no filesystem.
    Unknown,
}

/// Which clang consumes the IR cpc emits for this target. Rung 2 of the
/// backends plan: an external-builder target may also need an *external
/// toolchain* — the Android NDK ships its own clang with the Android
/// sysroot baked in, and ESP32 (rung 4) will need esp-clang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolchainKind {
    /// The host clang `cpc` already resolves (`$CPC_CLANG`, `clang`,
    /// `clang-21`..`clang-19`). Also right for iOS: Apple/mainline clang
    /// emits `arm64-apple-ios` objects.
    HostClang,
    /// The Android NDK's clang, resolved from `$CPC_NDK_CLANG`,
    /// `$ANDROID_NDK_HOME` / `$ANDROID_NDK_ROOT` / `$ANDROID_NDK_LATEST_HOME`,
    /// or the SDK's default `ndk/` directory (newest version).
    AndroidNdk,
    /// Espressif's esp-clang (the LLVM fork carrying the Xtensa backend),
    /// resolved from `$CPC_ESP_CLANG` or `~/.espressif/tools/esp-clang/`
    /// (newest version) — the ESP-IDF `idf_tools.py install esp-clang`
    /// location. Verified to accept cpc's textual IR (2026-06-11 spike).
    EspClang,
    /// No external compiler: cpc emits the final artifact (WebAssembly text)
    /// itself. The wasm32 playground target. The native driver rejects it —
    /// it is consumed only by `wasm_emit` in the browser front end.
    Internal,
}

/// Relocatable-object container format the target's toolchain consumes.
/// Informational for now (clang derives it from the triple); carried so
/// driver-side packaging decisions don't re-derive it from strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectFormat {
    MachO,
    Elf,
    Coff,
    /// WebAssembly module. cpc emits it directly (no relocatable-object
    /// handoff); carried for symmetry with the other targets.
    Wasm,
}

/// Who runs the final link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handoff {
    /// cpc invokes the host clang to link an executable / shared library.
    HostLink,
    /// cpc stops at object / static-archive emission; an external build
    /// system (Xcode, NDK, ESP-IDF) owns the final link and packaging.
    ExternalBuilder,
    /// cpc produces the final, runnable artifact itself (WebAssembly text,
    /// assembled in-browser). No clang, no linker, no external builder.
    Internal,
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
    pub toolchain: ToolchainKind,
    /// Extra clang arguments the target's object emission requires beyond
    /// `-target` (e.g. RV32's `-march`/`-mabi` selection). Empty for most.
    pub extra_clang_args: &'static [&'static str],
    /// The OS-version token inside `triple` that `--min-os` replaces
    /// (`"13.0"` for the iOS targets, `"24"` for Android). `None` = the
    /// triple is unversioned and `--min-os` is rejected.
    pub min_os_default: Option<&'static str>,
    /// stdlib modules excluded from this target's package profile —
    /// modules whose mechanism does not exist there (kqueue/epoll
    /// reactor, pthreads, process environment). Importing one fails at
    /// resolve time with E0866 instead of reaching the IR verifier.
    /// Includes their stdlib-internal consumers (executor/time/fs reach
    /// the reactor through relative imports).
    pub unsupported_stdlib: &'static [&'static str],
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
    toolchain: ToolchainKind::HostClang,
    extra_clang_args: &[],
    min_os_default: None,
    unsupported_stdlib: &[],
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
    toolchain: ToolchainKind::HostClang,
    extra_clang_args: &[],
    min_os_default: Some("13.0"),
    unsupported_stdlib: &[],
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
    toolchain: ToolchainKind::HostClang,
    extra_clang_args: &[],
    min_os_default: Some("13.0"),
    unsupported_stdlib: &[],
};

/// Android native, arm64 (rung 2 of the backends plan: the first non-host
/// external toolchain). No JVM, no UI: cpc emits an ELF object / static
/// archive the NDK (Gradle/CMake) build links into the app or binary. The
/// `24` minimum API (Android 7.0) is in range for every NDK cpc accepts
/// (r28+, the LLVM-19 floor) and predates them all, so the object loads on
/// any device those NDKs target.
pub const ANDROID_ARM64: TargetSpec = TargetSpec {
    name: "android-arm64",
    arch: TargetArch::Aarch64,
    os: TargetOs::Android,
    pointer_width: 64,
    little_endian: true,
    object_format: ObjectFormat::Elf,
    triple: Some("aarch64-linux-android24"),
    artifact_triple: Some("aarch64-linux-android"),
    apple_sdk: None,
    handoff: Handoff::ExternalBuilder,
    toolchain: ToolchainKind::AndroidNdk,
    extra_clang_args: &[],
    min_os_default: Some("24"),
    unsupported_stdlib: &[],
};

/// ESP32 classic (Xtensa LX6, the WROOM-32 module family) under ESP-IDF —
/// rung 4 of the backends plan, and the first 32-bit target. cpc emits the
/// object; ESP-IDF owns the firmware link, partition table, and flashing.
/// The Xtensa ABI facts encoded in codegen come from an empirical probe of
/// esp-clang 20.1.1 (`-target xtensa-esp32-elf`): datalayout
/// `e-m:e-p:32:32-v1:8:8-i64:64-i128:128-n32` (pointers 32-bit, i64 aligns
/// to 8), aggregate args ≤ 24 bytes coerce to arrays of align-sized chunks,
/// larger pass indirect `byval`; aggregate returns > 16 bytes use sret; no
/// FP-register HFAs.
pub const ESP32_XTENSA: TargetSpec = TargetSpec {
    name: "esp32-xtensa",
    arch: TargetArch::Xtensa,
    os: TargetOs::EspIdf,
    pointer_width: 32,
    little_endian: true,
    object_format: ObjectFormat::Elf,
    triple: Some("xtensa-esp32-elf"),
    artifact_triple: Some("xtensa-esp32-elf"),
    apple_sdk: None,
    handoff: Handoff::ExternalBuilder,
    toolchain: ToolchainKind::EspClang,
    extra_clang_args: &[],
    min_os_default: None,
    // The POSIX half of stdlib: pthread-backed (thread/mutex/channel),
    // the kqueue/epoll reactor and its consumers (executor/time/net/
    // netsys/fs), and the process environment. `vendor/espidf` covers
    // the embedded equivalents (timer, task sleep, console).
    unsupported_stdlib: &[
        "thread", "mutex", "channel", "env", "net", "netsys", "reactor", "executor", "time", "fs",
    ],
};

/// ESP32-C3 (RISC-V RV32IMC, single core) under ESP-IDF — the mainline-LLVM
/// comparison point for the embedded story. Same handoff and profile as
/// esp32-xtensa. ABI pinned against an esp-clang 20.1.1 ilp32 probe:
/// datalayout `e-m:e-p:32:32-i64:64-n32-S128`; aggregate args ≤ 8 bytes
/// coerce to align-sized chunks (`[2 x i32]`, bare `i64`), larger pass as a
/// bare pointer (no byval, unlike Xtensa); returns > 8 bytes use sret.
pub const ESP32C3_RISCV32: TargetSpec = TargetSpec {
    name: "esp32c3-riscv32",
    arch: TargetArch::Riscv32,
    os: TargetOs::EspIdf,
    pointer_width: 32,
    little_endian: true,
    object_format: ObjectFormat::Elf,
    triple: Some("riscv32-esp-elf"),
    artifact_triple: Some("riscv32-esp-elf"),
    apple_sdk: None,
    handoff: Handoff::ExternalBuilder,
    toolchain: ToolchainKind::EspClang,
    // The C3 core is RV32IMC; esp-clang needs the arch/abi selection
    // alongside `-target` (its multilibs are keyed on it).
    extra_clang_args: &["-march=rv32imc_zicsr_zifencei", "-mabi=ilp32"],
    min_os_default: None,
    unsupported_stdlib: &[
        "thread", "mutex", "channel", "env", "net", "netsys", "reactor", "executor", "time", "fs",
    ],
};

/// WebAssembly (`wasm32-unknown-unknown`) for the browser playground —
/// `plans/plan.wasm-playground.md`. Unlike every other spec, cpc emits the
/// final artifact itself (WebAssembly text via `wasm_emit`); there is no
/// clang, no linker, no external builder, no OS. The host (the browser, via
/// JS imports) supplies the libc floor.
///
/// Deliberately **not** in [`SUPPORTED`]: the native `cpc` driver cannot
/// build it (it has no in-process wasm assembler), so `--target wasm32` stays
/// unresolvable. The spec is consumed only by `wasm_emit` / `cpc-wasm`, which
/// install it with [`set_active_target`] so layout and `usize` come out 32-bit.
pub const WASM32: TargetSpec = TargetSpec {
    name: "wasm32",
    arch: TargetArch::Wasm32,
    os: TargetOs::Unknown,
    pointer_width: 32,
    little_endian: true,
    object_format: ObjectFormat::Wasm,
    triple: Some("wasm32-unknown-unknown"),
    artifact_triple: Some("wasm32-unknown-unknown"),
    apple_sdk: None,
    handoff: Handoff::Internal,
    toolchain: ToolchainKind::Internal,
    extra_clang_args: &[],
    min_os_default: None,
    // The whole POSIX/embedded surface bottoms out at syscalls or an OS the
    // browser sandbox does not provide. The playground front end rejects
    // `import` outright today, so this list is informational for when the
    // virtual-FS stdlib lands.
    unsupported_stdlib: &[
        "thread", "mutex", "channel", "env", "net", "netsys", "reactor", "executor", "time", "fs",
    ],
};

/// Every named target `--target` accepts, in the order help text lists them.
pub const SUPPORTED: &[TargetSpec] = &[
    HOST,
    IOS_ARM64,
    IOS_ARM64_SIMULATOR,
    ANDROID_ARM64,
    ESP32_XTENSA,
    ESP32C3_RISCV32,
];

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
static MIN_OS_OVERRIDE: Mutex<Option<String>> = Mutex::new(None);

/// Install the `--min-os` override. The driver validates the target
/// supports one (`min_os_default` present) before calling.
pub fn set_min_os_override(version: String) {
    *MIN_OS_OVERRIDE.lock().unwrap() = Some(version);
}

/// The version-spliced triple for a spec: the spec's `min_os_default`
/// token replaced by `over`. Pure, for unit tests; `active_triple`
/// applies it with the process-global override.
pub fn spliced_triple(spec: &TargetSpec, over: Option<&str>) -> Option<String> {
    let triple = spec.triple?;
    match (spec.min_os_default, over) {
        (Some(default), Some(v)) => Some(triple.replace(default, v)),
        _ => Some(triple.to_string()),
    }
}

/// The active target's triple with any `--min-os` override applied —
/// what the IR `target triple` line and clang's `-target` receive.
pub fn active_triple() -> Option<String> {
    let spec = active_target();
    let over = MIN_OS_OVERRIDE.lock().unwrap().clone();
    spliced_triple(&spec, over.as_deref())
}

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
    fn android_spec_is_external_builder_with_ndk_toolchain() {
        assert!(!ANDROID_ARM64.is_host());
        assert_eq!(ANDROID_ARM64.handoff, Handoff::ExternalBuilder);
        assert_eq!(ANDROID_ARM64.toolchain, ToolchainKind::AndroidNdk);
        assert_eq!(ANDROID_ARM64.arch, TargetArch::Aarch64);
        assert_eq!(ANDROID_ARM64.os, TargetOs::Android);
        assert_eq!(ANDROID_ARM64.object_format, ObjectFormat::Elf);
        assert_eq!(ANDROID_ARM64.triple, Some("aarch64-linux-android24"));
        // The artifact triple is the unversioned directory name (no API level).
        assert_eq!(ANDROID_ARM64.artifact_triple, Some("aarch64-linux-android"));
        assert_eq!(ANDROID_ARM64.apple_sdk, None);
    }

    #[test]
    fn only_android_targets_use_an_external_toolchain() {
        for spec in SUPPORTED {
            let expect_ndk = spec.os == TargetOs::Android;
            assert_eq!(
                spec.toolchain == ToolchainKind::AndroidNdk,
                expect_ndk,
                "toolchain kind mismatch for `{}`",
                spec.name
            );
        }
    }

    #[test]
    fn esp32_spec_is_the_first_32_bit_target() {
        assert!(!ESP32_XTENSA.is_host());
        assert_eq!(ESP32_XTENSA.pointer_width, 32);
        assert_eq!(ESP32_XTENSA.arch, TargetArch::Xtensa);
        assert_eq!(ESP32_XTENSA.os, TargetOs::EspIdf);
        assert_eq!(ESP32_XTENSA.object_format, ObjectFormat::Elf);
        assert_eq!(ESP32_XTENSA.handoff, Handoff::ExternalBuilder);
        assert_eq!(ESP32_XTENSA.toolchain, ToolchainKind::EspClang);
        assert_eq!(ESP32_XTENSA.triple, Some("xtensa-esp32-elf"));
        assert_eq!(ESP32_XTENSA.artifact_triple, Some("xtensa-esp32-elf"));
        // The C3 shares the 32-bit/ESP-IDF shape on a mainline-LLVM arch.
        assert_eq!(ESP32C3_RISCV32.pointer_width, 32);
        assert_eq!(ESP32C3_RISCV32.arch, TargetArch::Riscv32);
        assert_eq!(ESP32C3_RISCV32.toolchain, ToolchainKind::EspClang);
        assert!(!ESP32C3_RISCV32.extra_clang_args.is_empty());
        // Every other supported target stays 64-bit.
        for spec in SUPPORTED {
            if spec.pointer_width == 32 {
                continue;
            }
            assert_eq!(spec.pointer_width, 64, "`{}` must be 64-bit", spec.name);
        }
    }

    #[test]
    fn spliced_triple_replaces_only_the_version_token() {
        assert_eq!(
            spliced_triple(&IOS_ARM64, Some("15.2")).as_deref(),
            Some("arm64-apple-ios15.2")
        );
        assert_eq!(
            spliced_triple(&IOS_ARM64_SIMULATOR, Some("14.0")).as_deref(),
            Some("arm64-apple-ios14.0-simulator")
        );
        assert_eq!(
            spliced_triple(&ANDROID_ARM64, Some("28")).as_deref(),
            Some("aarch64-linux-android28")
        );
        // No override: the baked-in floor.
        assert_eq!(
            spliced_triple(&IOS_ARM64, None).as_deref(),
            Some("arm64-apple-ios13.0")
        );
        // Unversioned triples ignore the override; host has no triple.
        assert_eq!(
            spliced_triple(&ESP32_XTENSA, Some("9")).as_deref(),
            Some("xtensa-esp32-elf")
        );
        assert_eq!(spliced_triple(&HOST, Some("15.0")), None);
    }

    #[test]
    fn supported_names_lists_every_target_once() {
        let names = supported_names();
        assert_eq!(
            names,
            "host, ios-arm64, ios-arm64-simulator, android-arm64, esp32-xtensa, esp32c3-riscv32"
        );
    }

    #[test]
    fn wasm32_is_internal_browser_only_32_bit() {
        // 32-bit, little-endian, freestanding, self-emitted.
        assert_eq!(WASM32.pointer_width, 32);
        assert!(WASM32.little_endian);
        assert_eq!(WASM32.arch, TargetArch::Wasm32);
        assert_eq!(WASM32.os, TargetOs::Unknown);
        assert_eq!(WASM32.object_format, ObjectFormat::Wasm);
        assert_eq!(WASM32.handoff, Handoff::Internal);
        assert_eq!(WASM32.toolchain, ToolchainKind::Internal);
        assert_eq!(WASM32.triple, Some("wasm32-unknown-unknown"));
        assert!(!WASM32.is_host());

        // Deliberately not a `--target` name: the native driver can't build
        // wasm, so it must stay unresolvable and out of the help list.
        assert_eq!(TargetSpec::from_name("wasm32"), None);
        assert!(!SUPPORTED.iter().any(|t| t.name == "wasm32"));
        assert!(!supported_names().contains("wasm32"));
    }

    #[test]
    fn active_target_defaults_to_host() {
        // Other tests must not mutate the global (they use the `*_for`
        // helpers with explicit specs), so the default is observable here.
        assert!(active_target().is_host());
    }
}
