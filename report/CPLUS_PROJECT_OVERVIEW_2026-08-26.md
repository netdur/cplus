# C+, Packages, and Facet: Project Overview and Current Status

**Prepared:** 26 August 2026  
**Audience:** Technical evaluators, potential collaborators, and prospective investors  
**Basis:** The repository at commit `26fc10b` on 26 August 2026  
**Project:** C+ programming language, package platform, and Facet application framework
**License:** MIT

## Executive summary

C+ is a feature-complete, deliberately compact native programming language at the foundation of a larger software platform. The language is no longer in a phase of active feature invention. Its intended surface is frozen, with current language work limited to bug fixes, implementation hardening, diagnostics, and small corrective improvements.

The name expresses the architecture: **C + packages**.

The compiler provides the foundations that packages cannot provide for themselves: syntax and semantics, ownership and borrow checking, deterministic cleanup, native code generation, C-compatible interoperability, target modelling, and compiler-grade source knowledge. Nearly everything above that foundation—including the standard library, networking, cryptography, databases, platform APIs, layout, UI, inspection, and agent integration—is supplied as a package.

This separation is the project’s strategy for reaching language stability without stopping the platform from advancing. A defect in `stdlib` is resolved through a package update rather than a language update. A new platform capability belongs in its platform or framework package. Packages can evolve at the pace of their domains while the language remains small, learnable, auditable, and stable.

Facet is a principal part of the project, not merely a demonstration package. It is a cross-platform application framework comparable in ambition to .NET MAUI, but with a different technical proposition: the application language is native and the UI is native. C+ compiles to native code without a managed virtual machine or garbage collector, while Facet renders through AppKit, UIKit, GTK, and Android’s native view system rather than replacing the host platform with a single custom-drawn UI engine.

The project has also deliberately built the surface a new language needs in order to be judged as usable: a complete compiler pipeline, build and test tooling, a language server, compiler-resolved code intelligence, package management, bindings to existing SDKs, a standard library, native UI, inspection, agent interfaces, and multi-platform examples. This is a conscious response to the ecosystem cold-start problem. Depending on an early community to create all essential tools and libraries would be unrealistic; C+ is bootstrapping that foundation directly.

As of this assessment, the latest tagged version is **v0.0.27**, released on 14 August 2026. The main branch is **217 commits beyond that tag**, with substantial package, platform, tooling, and Facet work. The pre-1.0 version signals that public compatibility, distribution, and ecosystem commitments are still being formalized. It should not be interpreted as evidence that the language lacks its intended features.

The project’s strongest assets are its frozen language core, disciplined package boundary, breadth of implemented systems, extensive automated testing, direct native interoperability, and Facet’s native-language/native-UI model. Its principal remaining risks are cross-platform release consistency, package compatibility policy, documentation drift, incomplete backend parity, key-person concentration, independent safety validation, and external adoption.

## At a glance

| Area | Current repository evidence |
|---|---|
| Language stage | Feature-complete and feature-frozen; latest tag v0.0.27 |
| Compiler implementation | Rust workspace; direct textual LLVM IR generation, then clang assembly/linking |
| Workspace components | `cplus-core`, `cpc`, `cpc-lsp`, `cpc-bindgen`, `cpc-wasm`, `cplus-pm` |
| Core safety model | Ownership, moves, flow-sensitive borrow checking, deterministic drop, no `null` in safe code, accountable raw pointers |
| Primary native ABI | C ABI in both directions; C header emission and binding generation |
| Explicit targets | Host, iOS device/simulator, Android arm64, ESP32 Xtensa, ESP32-C3 RISC-V; internal browser Wasm target |
| Package ecosystem in tree | 63 vendored packages, including generated and hand-written packages |
| Cross-platform application framework | Facet with native AppKit, UIKit, GTK, and Android backends |
| Agent capabilities | Compiler code graph/MCP plus permissioned live-application agent surfaces |
| Repository scale | 1,003 tracked files; about 175,000 Rust LOC and 688,000 C+ LOC in `vendor/` and `examples/` |
| Test evidence on 26 Aug 2026 | 3,076 Rust workspace tests passed; five principal C+ package suites also passed |
| License | MIT |

The line counts demonstrate scope, not hand-written code volume. A large share of the C+ total is generated platform binding code, which is itself an intended product of `cpc-bindgen`.

## 1. What the project is

C+ is best understood as several related products sharing one architectural foundation.

### 1.1 A systems programming language

The language provides the constructs needed to write native software without a garbage-collected virtual machine:

- primitive numeric and Boolean types;
- structs, tagged enums, arrays, slices, tuples, and function pointers;
- methods, interfaces, generic functions and types, and monomorphization;
- explicit moves, read and write borrows, deterministic destructors, and raw-pointer escape hatches;
- pattern matching, guards, loops, `defer`, tests, assertions, async functions, generators, and scoped threads;
- C-compatible data layout and function boundaries;
- SIMD primitives, atomics, inline assembly, and compiler-checked real-time restrictions;
- package-extensible builder blocks such as `@view { ... }`, allowing a library to define declarative construction syntax without a general macro system or compiler plugin.

The language intentionally resembles familiar C and Rust concepts, but it is not a dialect of either. Its design favours explicit ownership at function boundaries, limited ambiguity, no implicit numeric conversions, no general exception mechanism, and a smaller semantic surface than Rust, Swift, or C++.

The language is feature-complete for its intended design. The project declared a language feature freeze in v0.0.22: new capability belongs in packages and tooling, while the language itself receives bug fixes and corrective hardening. A small number of later changes were explicitly treated as bounded exceptions or final corrections, not as a return to continuous language expansion.

### 1.2 A native compiler and build toolchain

The compiler front end is implemented in Rust. The principal pipeline is:

```text
C+ source
  -> lexer and parser
  -> attribute checking and lowering
  -> module/package resolution
  -> semantic and type analysis
  -> borrow and lifetime checks
  -> generic monomorphization
  -> textual LLVM IR generation
  -> clang assembly and linking
  -> native object, library, or executable
```

C+ does not embed LLVM as a library. It emits LLVM IR text and invokes clang for the platform-specific final stages. This keeps the compiler distribution comparatively self-contained while reusing the mature object generation, optimization, and linker integration supplied by clang.

The `cpc` command is more than a single-file compiler. It provides project creation, checking, building, testing, formatting, documentation generation, header emission, package commands, target selection, sanitizer modes, real-time reports, code graph queries, an LSP entry point, and an MCP server for compiler-backed source navigation.

### 1.3 The package platform—the “+” in C+

C+ deliberately puts its standard library and most domain capability outside the compiler. Projects declare dependencies in `Cplus.toml`; the build system resolves source packages and target-specific prebuilt archives, validates their declared link metadata, and passes required frameworks, libraries, and objects to clang.

This boundary decouples platform growth from language releases. A standard-library defect is a `stdlib` package issue; a new control or corrected behaviour is a Facet package issue; and an SDK change can be handled by regenerating or updating its binding package. The language can therefore remain fixed while its practical capabilities continue to expand.

The in-tree package manager, exposed through `cpc pm`, installs exact-version packages into either a project-local vendor directory or a per-user store. Its design is intentionally simple: exact pins rather than version ranges, the manifest as the lock, and no dependency solver. Toolchain packages such as `stdlib` are version-coupled to the compiler.

Interop is a first-order capability rather than an afterthought. C+ can call C through `extern fn`, export C ABI symbols, express `#[repr(C)]` layouts, emit C headers, and generate bindings from external metadata. The binding generator now supports five major input families:

- C headers through clang AST data;
- Objective-C headers and complete Apple frameworks;
- Swift symbol graphs, including generated Swift-to-C bridge packages;
- GObject Introspection data for the Linux/GNOME stack;
- Java class metadata for Android/JNI bindings.

This breadth matters strategically. A new language normally faces an ecosystem cold start. C+ shortens that cold start by making existing operating-system and SDK surfaces mechanically reachable, while preserving explicit ownership and typed wrappers on the C+ side. The first-party package surface is deliberately broad because relying on a future community to supply every essential capability would leave the language looking incomplete regardless of the quality of its core.

### 1.4 Facet: a principal cross-platform product

Facet is the project’s cross-platform native application framework and a principal product alongside C+ itself. Its role is comparable in ambition to .NET MAUI: one shared application and UI model spanning desktop and mobile platforms. The difference is fundamental to its positioning—C+ is a native language, and Facet renders native UI.

Facet defines a shared control vocabulary and state/update contract, uses a pure-C+ Flexbox/Grid engine for geometry, and delegates native widget creation and behaviour to platform renderers.

Current backends include:

- AppKit on macOS;
- UIKit on iOS and iPadOS;
- GTK 4/libadwaita on Linux;
- Android views through JNI, currently under rapid development.

Facet is not a custom raster engine in the Flutter sense. Applications compile to native machine code without a managed VM or garbage collector, and the controls remain AppKit, UIKit, GTK, or Android controls. Facet shares application structure, layout vocabulary, theme rules, update semantics, and agent identity while documenting platform differences as explicit capabilities or gaps rather than hiding them behind inaccurate equivalence.

Facet also acts as a demanding integration proof for the language. It exercises generics, ownership, generated SDK bindings, callbacks without closures, cross-package interfaces, layout, async services, native lifetimes, target-specific files, and package prebuilding. Its existence demonstrates the central architectural claim: a large cross-platform application system can be built as packages without continually expanding the compiler.

### 1.5 An agent-native software stack

The project uses the phrase “built for tools and LLMs” in a concrete way.

At source level, the compiler retains the resolved information it already computes and exposes it as a typed code-knowledge graph. `cpc graph`, `cpc query`, the LSP, and `cpc mcp` can answer symbol definitions, references, callers, callees, call hierarchies, types, local scope, completions, and contextual edit information. The aim is for developer tools and coding agents to navigate by compiler-resolved symbols and types rather than by text search.

At application level, the `agent_core`, `agent_mcp`, `agent_inapp`, and platform agent packages expose a live UI as a semantic identity tree. Agents can describe the interface and perform operations such as clicking, setting text, scrolling, navigation, hit testing, or menu invocation through backend vtables. Capability grants, sensitivity tiers, channels, and authorization outcomes are part of this design. A protected field may be visible to an agent as a node while its value remains unreadable; external and in-app agents may receive different permissions.

This creates a distinctive end-to-end proposition: the same ecosystem is designed to be understandable by agents while code is being written and operable by agents after the application is running.

## 2. What C+ is trying to achieve

The repository reveals five connected objectives.

### 2.1 Make native systems work safer without requiring a managed runtime

C remains the universal systems ABI, but it leaves ownership, lifetime, aliasing, and cleanup largely to convention. Languages such as Swift and Dart provide strong safety and productivity but bring larger runtimes and different deployment assumptions. Rust provides powerful compile-time guarantees but has a large language and ecosystem surface.

C+ occupies a narrower point in that design space: C-compatible native artifacts, direct memory and hardware access, deterministic cleanup, and no language VM or garbage collector, combined with compile-time checking designed to prevent common ownership and borrowing mistakes in safe code.

The goal is not to make unsafe systems programming disappear. Raw pointers, FFI, inline assembly, and platform calls remain necessary. The goal is to make their boundaries visible and keep ordinary application and library code inside a more strongly checked model.

### 2.2 Keep the language stable while packages grow

Many languages accumulate special syntax, compiler intrinsics, and built-in frameworks as they grow. C+ takes the opposite approach: the language supplies only primitives that packages cannot express, and practical capabilities live at package level.

Examples already present include:

- the standard library as a normal package;
- SIMD vector math as a package over primitive vector operations;
- Metal and CUDA access through generated or hand-wrapped SDK bindings;
- AppKit, UIKit, Win32, GTK, Android, SQLite, WebKit, and llama.cpp integrations through packages;
- UI construction through package-supplied builder types;
- agent protocols and authorization as libraries rather than compiler concepts.

This model allows the compiler and language contract to remain understandable and stable while the package ecosystem grows independently. It is already exercised by the standard library, SDK bindings, Facet, layout, platform services, inspection, and agent packages.

### 2.3 Reduce the cost of reaching native platforms

Native SDK integration is expensive because every platform has a different metadata and runtime model. C+ is building one binding tool that can consume C, Objective-C, Swift, GObject, and Java descriptions, then emit C+ packages with reproducible provenance and explicit skipped cases.

The long-term achievement would not be “one abstraction hides every platform.” It would be a language in which developers can share code where semantics genuinely match, access the native SDK when they do not, and generate much of the bridge rather than maintaining it by hand.

### 2.4 Make applications inherently inspectable and operable by software agents

Most agent integrations are added after an application is built and rely on screenshots, accessibility trees, or brittle automation. C+ and Facet are trying to make semantic identity, action wiring, authorization, and inspection part of the application architecture itself.

This could support several use cases:

- higher-quality automated testing;
- in-application assistants;
- remote support and guided workflows;
- accessible automation that acts on named controls rather than coordinates;
- coding agents that understand both the source graph and the live UI surface.

The authorization work is essential to this ambition. A system that makes every UI value automatically readable by an external agent would be powerful but unsafe. The current design instead includes explicit channels, grants, sensitivity tiers, and refusals.

### 2.5 Span desktop, mobile, embedded, and browser tooling with one language core

The compiler has explicit target models for host builds, iOS, Android arm64, ESP32 Xtensa, and ESP32-C3 RISC-V. The browser front end compiles source to diagnostics and LLVM IR client-side, and an intentionally limited Wasm emitter can execute a numeric core directly in the browser.

The intended through-line is a single small language usable from microcontroller real-time code to native desktop/mobile applications, while sharing packages and tooling where the environment permits.

## 3. What the project does today

### 3.1 Language and safety capabilities

The current compiler implements a broad language surface, including:

- ownership-aware parameter and receiver modes;
- flow-sensitive move and borrow analysis;
- checks preventing views such as `str` or slices from outliving their backing storage;
- automatic and user-defined drop behaviour;
- structural `Copy`, plus derived equality, ordering, hashing, cloning, and text conversion;
- generic functions, structs, enums, methods, and interface bounds;
- tagged unions with exhaustive matching and nested patterns;
- function contracts through `#[requires(...)]`;
- scoped threads and `Send`/`Sync` constraints;
- async/future execution and explicit cancellation paths;
- checked array access and debug arithmetic checks, with explicit wrapping operations;
- sanitizers and debug information for native builds;
- compiler-checked `#[no_alloc]`, `#[no_block]`, and real-time profiles.

The safety story is meaningful but not absolute. Unsafe FFI and raw-pointer code can still violate invariants, and the implementation has not received the decades of production exposure or independent scrutiny available to established systems languages. “Ownership-checked and safety-oriented” is supported by the repository; “formally proven memory-safe” would overstate the available evidence.

### 3.2 Developer tools

The toolchain supports the normal development loop:

- `cpc init` creates CLI or GUI projects and target-specific scaffolding;
- `cpc check` runs the complete front end without code generation;
- `cpc build` emits applications and libraries;
- `cpc test` discovers unit tests and documentation tests;
- `cpc fmt` formats source;
- `cpc doc` produces Markdown API documentation;
- `cpc explain` describes diagnostic codes;
- `cpc skill` emits a version-matched language reference intended for humans and coding agents;
- `cpc lsp` and `cpc-lsp` provide editor integration;
- graph, query, completion, scope, and MCP commands provide compiler-backed code intelligence.

The browser package `cpc-wasm` can perform front-end checking and show generated LLVM IR without a server. Its runnable Wasm subset currently covers scalar/numeric control-flow programs, not the full language, FFI, heap, or standard library.

### 3.3 Standard library and supporting packages

The standard library includes owned text, slices, vectors, options and results, maps and sets, files, networking, processes, PTYs, cryptography, dates, threading, atomics, mutexes, channels, reference-counted values, futures, an executor, and platform-specific reactors.

Other packages demonstrate the ecosystem model across several domains:

- `json`, `uuid`, `log`, `http`, `sqlite`, and filesystem watching;
- `simd`, `accelerate`, `cblas`, `cuda`, `metal`, and `llama_cpp`;
- `objc`, `jni`, Android views, AppKit, UIKit, WebKit, Win32, and the GTK/GObject stack;
- `flex_layout`, Facet renderers, terminal widgets, inspectors, and agent packages;
- ESP-IDF and real-time support packages.

Some packages are generated raw bindings, some are ergonomic hand-written wrappers, and some are complete subsystems. They should not all be interpreted as equally mature product APIs.

### 3.4 Multi-platform application work

Facet's platform work has become the dominant recent development thread.

AppKit is currently the most mature native backend in the locally verified set. GTK reports 98% of declared Facet properties and 100% of declared handlers, with every remaining gap documented. UIKit reports 89% property coverage and 95% handler coverage. Android is the newest backend and its package is developing quickly; against the assessed commit its parity tool reports:

- 230 of 360 declared per-control property bits named: **63%**;
- 42 of 68 declared handlers fired: **61%**;
- 18 of 21 shared property groups handled: **85%**.

The Android backend already includes common text and value controls, scrolling, list recycling, tree views, split views, canvas replay, swipe actions, symbols, images, date selection, animation, theme colour resolution, pull-to-refresh, and an agent surface. It uses native Android widgets behind JNI, and a gallery application can be packaged without app-authored Java. Remaining work includes several collection, popup, web, tab, time-picker, selection, reorder, and lifecycle behaviours.

### 3.5 Distribution

The repository documents Homebrew installation for macOS Apple Silicon and release assets for Linux and Windows. The release workflows build macOS binaries automatically on tags. Linux and Windows workflows remain available for manual dispatch but their tag triggers were disabled on 14 August 2026. This means the repository contains genuine multi-platform work, but the continuously enforced release matrix is currently narrower than the advertised platform surface.

## 4. Architecture and why it matters

The major systems form a layered architecture:

```text
Applications and examples
    |
    +-- Facet UI / terminal / inspector / agent surfaces
    |
    +-- Domain packages and generated platform SDK bindings
    |
    +-- stdlib, layout, runtimes, Objective-C/GObject/JNI bridges
    |
    +-- C+ language, package resolver, ownership/type analysis
    |
    +-- LLVM IR emission and target model
    |
    +-- clang / platform linker / Xcode / NDK / ESP toolchain
```

This separation creates several advantages:

1. **The language core is tested by real consumers.** Facet, generated bindings, async networking, and real-time packages expose compiler limitations sooner than synthetic examples would.
2. **Platform capability can evolve without new language syntax.** A new framework generally needs metadata, bindings, wrappers, and a package manifest rather than a compiler fork.
3. **Generated and hand-written layers are distinguishable.** Binding packages record the generator version, SDK version, and reproduction command, while ergonomic additions can live in stable extension files.
4. **The same semantic seams serve tools and agents.** Typed compiler information powers source queries; identity and backend vtables power live application queries.
5. **Target-specific ownership remains external.** C+ emits objects or archives for targets such as iOS, Android, and ESP32, while the platform's established build system owns final application packaging.

The architecture also has costs. The direct AST-to-LLVM approach concentrates a large amount of responsibility in semantic analysis and code generation. Generated binding volume makes repository-wide changes expensive. Target-specific overrides and prebuilt archive selection create subtle consistency requirements. The project's tests and detailed bug records show that these costs are real, not merely theoretical.

## 5. Technical and strategic differentiation

### 5.1 Compared with C and C++

C+ offers stronger ownership, borrowing, exhaustiveness, and type checking while retaining direct C ABI interoperability and native artifacts. It does not attempt to reproduce the full C++ object model, template system, exceptions, or language complexity.

The opportunity is a safer language for software that still needs to live close to C libraries, operating-system APIs, devices, or strict runtime budgets. The risk is that C and C++ have enormous ecosystems, toolchains, and institutional familiarity that a new language cannot quickly reproduce.

### 5.2 Compared with Rust

C+ shares the ambition of compile-time memory and concurrency safety, but pursues a smaller language, more explicit parameter-level ownership vocabulary, direct textual LLVM IR generation, and a stronger “everything above primitives is a package” rule.

Rust's advantages remain decisive in maturity, formal and practical scrutiny, tooling breadth, community, package availability, supported architectures, and production evidence. C+'s differentiation would need to come from lower conceptual surface, C/platform integration, agent-native tooling, or the unified native application stack—not from claiming stronger safety than Rust.

### 5.3 Compared with Swift, Kotlin, Dart, and cross-platform UI frameworks

C+ can build native platform integrations without a language VM or garbage collector and can use native controls through Facet. It also spans systems and embedded use cases that these application languages do not primarily target.

Conversely, Swift, Kotlin, Dart/Flutter, React Native, and established native SDKs have far larger developer ecosystems, mature debugging and deployment tools, production libraries, and proven application distribution paths. Facet already implements its core architectural proposition across four native UI families, but it has not yet matched the full breadth, external adoption, or production history of those established ecosystems.

### 5.4 The potentially distinctive combination

No single feature is sufficient to justify a new language. The more compelling proposition is the combination:

- a small ownership-checked systems language;
- direct C and platform SDK reach;
- generated multi-runtime bindings;
- shared native UI with explicit platform truth;
- compiler-backed source intelligence;
- permissioned live-application agent control;
- desktop, mobile, embedded, and browser-tooling targets.

Together, these systems make C+ more than another syntax for native code: they form a coherent environment for building software that is both low-level and agent-operable.

## 6. Current project status

### 6.1 Release and development state

The latest tagged release is **v0.0.27**, dated 14 August 2026. It covers major language and memory-model hardening, Facet growth, GObject/GTK and SQLite bindings, tooling, inspection, and generated framework work.

The language itself is feature-complete and frozen. Current development activity is principally compiler correctness and hardening, packages, native bindings, developer tooling, target support, and Facet backend work. The volume of post-release commits should therefore not be read as ongoing language-design churn.

At the time of this report, `main` points to commit `26fc10b` and matches `origin/main`. It is 217 commits beyond v0.0.27, spanning 605 changed files in the tag-to-head comparison. The crate versions still report 0.0.27, so the development head should be treated as unreleased post-v0.0.27 work rather than a published version. The latest commit adds Android pull-to-refresh, button-image support, and additional list behaviour; there are no uncommitted project-source modifications in the assessed tree.

### 6.2 Repository scale

Measured locally:

- 1,003 tracked files;
- approximately 174,919 lines of Rust across the six workspace components;
- approximately 688,091 lines of C+ under `vendor/` and `examples/`;
- 63 package manifests under `vendor/`.

These numbers show that C+ has moved far beyond a minimal language experiment. They do not imply equivalent maturity in every subsystem, and generated bindings account for a significant portion of the C+ volume.

### 6.3 Verification performed for this report

The following commands were run against the current workspace on macOS Apple Silicon:

```sh
cargo test --workspace --release --no-fail-fast

cd vendor/stdlib       && ../../target/release/cpc test
cd vendor/facet        && ../../target/release/cpc test
cd vendor/facet_appkit && ../../target/release/cpc test
cd vendor/terminal     && ../../target/release/cpc test
cd vendor/events       && ../../target/release/cpc test
```

Results:

| Suite | Result |
|---|---:|
| Rust workspace | 3,076 passed, 0 failed |
| C+ standard library | 362 passed, 0 failed |
| Facet plus dependencies | 609 passed, 0 failed |
| Facet/AppKit plus dependencies | 880 passed, 0 failed |
| Terminal plus dependencies | 421 passed, 0 failed |
| Events plus dependencies | 72 passed, 0 failed |

The Rust build emitted one duplicate-test-attribute warning in `cplus-core/src/codegen.rs`. The C+ suites emit expected warnings where the compiler cannot statically prove that conditionally owned raw control blocks are released on every path. The AppKit suite also produced non-fatal WebKit cache-directory diagnostics from its temporary test executable.

Two standard-library network tests initially failed in the restricted execution sandbox because they could not bind local sockets. Re-running the same suite with local network permissions produced 362/362 passing, confirming that those two results were environmental rather than project regressions.

This verification is substantial but not universal. It does not replace testing on Linux, Windows, physical iOS devices, Android devices/emulators, or ESP hardware. It also does not visually validate UI feel, rendering fidelity, gestures, accessibility, or deployment flows.

### 6.4 Platform maturity

| Platform or target | Current position |
|---|---|
| macOS / Apple Silicon | Primary development and tested configuration; AppKit is the most mature Facet backend; Homebrew release path exists |
| iOS / iPadOS | Explicit device and simulator targets, generated UIKit bindings, Facet/UIKit gallery and agent surface; requires Xcode to package and sign |
| Linux x86_64 | Native host port, generated GTK/libadwaita stack, high Facet parity; Linux tag CI currently paused and Linux-specific stdlib testing is a known process concern |
| Windows x86_64 | Native compiler port, Win32 bindings, agent package, release workflow retained for manual runs; tag CI currently paused |
| Android arm64 | Compiler target, JNI and Java binding generation, Android-view package, Facet backend and gallery; backend is active work and incomplete |
| ESP32 Xtensa / ESP32-C3 RISC-V | Explicit 32-bit targets and real-time checks; final integration relies on Espressif tooling |
| Browser / Wasm | Complete single-file front-end diagnostics and IR display; runnable numeric subset only, not yet the full language/runtime |

### 6.5 Open defects and visible debt

The top-level bug ledger currently contains three open reports:

1. An Android static-library edge case can omit a `_linux`-only platform module even though normal Android resolution correctly falls back to Linux.
2. Facet/UIKit does not yet render the portable bundled-symbol tier, although platform/system symbols work and other backends implement both tiers.
3. The cancellation design report remains open for residual work even though its principal thread and async cancellation phases have landed. Remaining items include implicit future drop, cancellation cleanup for local spawned tasks, and timeout/deadline conveniences.

There are also broader maturity issues visible outside the bug directory:

- the active `plan.md` still describes v0.0.27 candidate work even though v0.0.27 has shipped and main is far beyond it;
- the Android plan begins by saying no backend exists, while later sections and the code describe a substantial working backend;
- Android README and manifest counts lag the current parity script and sometimes contradict later sections of the same document;
- Linux and Windows release-tag CI are disabled;
- the project has one implementation and a small in-tree ecosystem compared with established languages;
- several platform behaviours can only be assessed on real devices or by a human tester;
- some generated binding surfaces explicitly skip constructs that cannot yet be modelled safely.

These are manageable engineering problems, but they are important signals for anyone assessing production readiness. The code is advancing faster than release governance and narrative documentation.

## 7. What is implemented, what is still being productized, and what is not yet established

### Proven within the repository

- A sizeable compiler and toolchain builds and passes a large automated test suite.
- C+ can compile native programs and libraries through LLVM IR and clang.
- Ownership, borrow, view-lifetime, code generation, ABI, target, sanitizer, package, and code-graph behaviour have extensive regression coverage.
- The external-package model supports a real standard library and complex subsystems.
- The binding generator can ingest several fundamentally different metadata ecosystems.
- Facet runs against multiple native UI toolkits and has measurable backend parity.
- Source-level and live-application agent interfaces exist with explicit authorization concepts.
- Cross-compilation paths exist for mobile and embedded targets.

### Implemented but still being productized

- Facet’s shared native UI vocabulary exists across macOS, iOS, Linux, and Android, while backend parity and release validation continue.
- Generated packages already bootstrap major native SDK surfaces, while provenance, publishing, and compatibility processes continue to mature.
- The compiler code graph and live-application semantic agent surface are implemented, while their public security and developer experience are still being productized.
- Real-time and constrained-system compiler support is implemented, while broader hardware validation and external production evidence remain limited.
- The package store and exact-pin installation model exist, while the public ecosystem around them still needs distribution policy and independent users.

### Not established by this repository alone

- Production adoption, paying customers, user growth, or download volume.
- A repeatable commercial go-to-market strategy.
- Independent security review or formal verification of the compiler's safety guarantees.
- A demonstrated long-term compatibility record across multiple public compiler and package releases.
- Competitive whole-application performance across representative workloads and targets.
- A broad contributor base or low key-person dependency.
- Complete accessibility, localization, device, and app-store validation for Facet applications.

Potential investors should request these non-code metrics separately. They cannot be responsibly inferred from commit volume or repository size.

## 8. Recommended next-stage priorities

The following priorities are an assessment based on the repository, not promises already made by the project.

### 8.1 Convert development velocity into a reliable release train

The first need is not another language feature. It is a new release plan that reconciles post-v0.0.27 work, updates crate versions, refreshes the changelog and top-level status documents, and establishes explicit release gates.

A credible gate would include:

- macOS, Linux, and Windows workspace tests;
- principal C+ package suites;
- iOS simulator compilation and a gallery smoke test;
- Android arm64 build, package, install, and gallery smoke test;
- target-specific archive/link validation;
- parity reports checked into release evidence.

### 8.2 Restore a continuously enforced cross-platform matrix

Linux and Windows CI should move from manual-only back to an enforced cadence once its cost or instability is addressed. Android needs an automated emulator path, particularly because platform overrides, JNI tables, archive closure, and Activity recreation have already produced subtle bugs.

### 8.3 Protect the language freeze and define package compatibility

The language is already feature-frozen and has a normative specification and diagnostic catalogue. The next step is to turn that freeze into a clear public promise, separating compiler bug fixes from package releases and documenting compatibility between compiler versions, foundation packages, independently versioned domain packages, manifest fields, and generated bindings.

For external users, the important promise is that new capability will normally arrive through packages without destabilizing the language.

### 8.4 Productize Facet as a first-class framework

Facet should have its own clear positioning, application architecture documentation, backend compatibility tables, and reproducible cross-platform release evidence. Its central message is straightforward: shared C+ application code, native machine-code output, and native platform controls.

A suitable release milestone would exercise the same application modules on macOS, iOS, Linux, and Android; cover state, networking, lists, input, accessibility, theming, and agent operations; and publish platform-specific differences honestly. This would make Facet’s existing technical depth easy for external developers to evaluate.

### 8.5 Productize the agent-native differentiation

The agent stack should be demonstrated as a cohesive workflow:

1. a coding agent queries the compiler-resolved source graph;
2. the application builds and runs;
3. an authorized agent discovers the live semantic UI;
4. sensitivity rules prevent protected reads;
5. the agent performs and verifies a workflow;
6. the same identities support inspection and automated tests.

This is a stronger differentiator than language syntax alone, but it needs a polished security model, audit logs, threat analysis, and a clear developer experience.

### 8.6 Invest in independent validation

Before presenting C+ as suitable for high-assurance or safety-sensitive production use, the project would benefit from:

- an external compiler and standard-library security review;
- fuzzing of the parser, resolver, manifests, binding inputs, and code generator;
- differential ABI and code-generation tests against C/clang across more targets;
- reproducible performance and binary-size benchmarks in the main repository;
- documented unsafe-code audits for runtime and platform bridge packages.

### 8.7 Build the ecosystem deliberately

The package manager is technically present, but a public ecosystem also needs discoverability, publishing policy, provenance, compatibility expectations, documentation standards, and governance around generated versus curated packages.

The initial ecosystem should remain focused and high quality. The project already supplies the foundational packages required to demonstrate a credible platform. Community contribution should be treated as an outcome of a usable, documented surface—not as the mechanism expected to create that surface in the first place.

## 9. How additional investment could be used

The repository suggests several areas where capital or additional engineering capacity could materially reduce risk:

- dedicated release and infrastructure ownership for the multi-platform test matrix;
- platform engineers for Android, Linux/GTK, Windows, and mobile deployment;
- compiler specialists focused on soundness, fuzzing, intermediate representation strategy, and diagnostics;
- developer-experience work around installation, editors, debugging, package publishing, and documentation;
- security engineering for FFI boundaries and permissioned agent access;
- design and developer-relations work around the Facet gallery and public examples;
- external audits and reproducible benchmark infrastructure.

Investment should be milestone-based. Useful milestones are objective: a defined compatibility promise, green cross-platform release gates, one polished multi-platform application, an externally reviewed safety boundary, a documented agent-security model, and evidence of third-party developers successfully building with the toolchain.

## 10. Overall assessment

C+ is best understood as a feature-complete native language core, an independently evolving package platform, and a first-class cross-platform application framework. The architecture is captured by the name: **C + packages**. Language stability and platform growth are not opposing goals because the package boundary is designed to separate them.

The implementation evidence is substantial. The compiler and tools pass a large regression suite; the standard library and complex native integrations live outside the language; the binding generator reaches several different SDK ecosystems; and Facet maps a shared application model onto AppKit, UIKit, GTK, and Android native controls. This is not a compiler prototype waiting for others to construct its usable surface.

Facet is central to the project’s value. Its proposition is cross-platform application development with a native language, native binaries, and native UI. That makes C+ more than a systems-language design and gives the package architecture a concrete, demanding product purpose.

The project still carries meaningful execution risk. Public package compatibility, synchronized releases, multi-platform CI, Android parity, documentation coherence, independent safety review, third-party onboarding, and adoption all require further work. These are the risks of productizing and scaling an already broad platform, not evidence that the language itself is unfinished.

For technical readers, the important question is whether the frozen language and package boundary remain robust as the platform expands. For investors, the important question is whether the existing C+ and Facet foundation can be converted into repeatable releases, independent developer success, and commercial adoption. The repository provides credible evidence for the technology; the next phase is about public reliability, distribution, and use.

## Appendix A: Repository map

| Path | Role |
|---|---|
| `cplus-core/` | Lexer, parser, AST, lowering, semantic analysis, borrow checking, monomorphization, graph, LLVM/Wasm emission |
| `cpc/` | Command-line compiler, build driver, project scaffolding, test runner, package integration, MCP server |
| `cpc-lsp/` | Language Server Protocol implementation |
| `cpc-bindgen/` | C, Objective-C, Swift, GObject, and Java binding generation |
| `cpc-wasm/` | Browser front end and limited runnable Wasm backend |
| `cplus-pm/` | Exact-pin package installation and per-user package store |
| `docs/lang/` | Language specification, guide, reference, ownership, FFI, platform, testing, and tooling documentation |
| `docs/compiler/` | Compiler internals and design records |
| `vendor/stdlib/` | External standard library |
| `vendor/facet/` | Shared native UI vocabulary and state/update model |
| `vendor/flex_layout/` | Pure-C+ Flexbox/Grid layout engine |
| `vendor/facet_appkit/` | macOS renderer |
| `vendor/facet_uikit/` | iOS/iPadOS renderer |
| `vendor/facet_gtk/` | Linux GTK renderer |
| `vendor/facet_android/` | Android/JNI renderer under active development |
| `vendor/agent_*` | Live application agent identity, auth, transports, and platform backends |
| `vendor/inspector/` | Live Facet inspection and mutation tools |
| `examples/` | Tracked galleries and integration examples |
| `plans/` | Version roadmaps, design investigations, handoffs, and historical decisions |
| `bugs/` | Open reports at top level; resolved reports under `bugs/closed/` |

## Appendix B: Assessment method and limitations

This report was produced from local repository evidence: README and language documentation, manifests, compiler internals, current and archived plans, changelog and release files, Git history, workflows, package documentation, open bug reports, parity scripts, working-tree status, and locally executed tests.

No claims in this report rely on private customer information, market surveys, download analytics, financial statements, or interviews, because none were provided. The report does not constitute a security audit, legal opinion, valuation, or investment recommendation. It is a technical and strategic project assessment as of the stated date.
