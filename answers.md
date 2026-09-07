# IRIS CTO Alignment Answers

**Prepared:** 3 September 2026  
**Repository basis:** C+ commit `0589a231` (`v0.0.27-313-g0589a231`) and the IRIS working tree based on commit `d88129b`  
**Purpose:** Technical answers to section 16 of the IRIS Project Alignment Document

## Executive position

The repository supports a credible technical foundation, but it does not yet support every claim in the alignment document.

What is real today is C+: an ownership-checked language and native compiler; Facet: a shared UI model with working native backends for macOS, iOS, Linux, and Android; compiler-backed code intelligence exposed to editors and agents; a growing package layer for device capabilities; and an implemented, Board-first IRIS workspace. IRIS is not primarily a code editor with chat attached: the product model is a Kanban project surface where the user defines work as cards and the agent reads and updates the same visible record while doing the work. Code, compiler feedback, builds, deployment, versioned Saves, and live application inspection remain available underneath that task workflow. Windows is the Facet platform still in progress: the compiler's Windows target and the Win32 bindings work, but there is no `facet_win32` renderer yet.

What is not yet established is that IRIS can reliably take a non-technical user from prompt to a production application across a broad task set, reduce token cost, outperform mature coding agents, or make every platform capability available from one description.

The correct public posture is therefore:

- Sell the outcome and workflow, not the C+ language.
- Describe C+ and Facet as the technical foundation, not as magic that removes platform differences.
- Say “shared application code with native platform backends,” not “identical behavior on every platform.”
- Treat agent reliability, token reduction, maintainability, and competitive superiority as hypotheses until benchmarked.
- Use “production-ready” only for a named version, platform, and capability set that has passed its release evidence. Do not apply it unqualifiedly to the entire development head.

## 1. What exactly does C+ make possible for AI agents that current languages do not?

Strictly speaking, C+ does not make something categorically impossible in every other language. Other languages can expose compiler services, language servers, structured errors, and agent tools. The defensible distinction is that C+ makes these capabilities part of one deliberately small, agent-oriented stack rather than adding them around a general-purpose ecosystem after the fact.

C+ currently gives an agent:

- A compact, version-matched language reference through `cpc skill`, including package-specific skills for the dependencies in the project.
- A resolved, typed code-knowledge graph through `cpc graph`, `cpc query`, and the resident `cpc mcp` server. It answers definitions, references, callers, callees, call hierarchies, function context, types, value references, scope, and completion without asking the model to infer those relationships from text search.
- Live-buffer awareness: the resident graph accepts unsaved buffers, rebuilds in the background, keeps the last valid graph while code is temporarily unparseable, and reports its current state.
- Structured diagnostics with stable error codes, source spans, labels, notes, and machine-applicable suggestions in JSON.
- A language with a relatively small surface, explicit ownership modes, no implicit `null` in safe code, deterministic cleanup, and package-defined UI syntax. These choices reduce some ambiguity in generated code and move many errors into the compiler loop.
- An optional semantic application surface. Facet keys identify controls in source, at runtime, and to an authorized agent. The `agent_core`, `agent_mcp`, `agent_inapp`, and `facet_agent` packages can describe and operate a live interface using named controls rather than screen coordinates, with explicit grants and sensitivity tiers.

The strongest claim is therefore not “agents can do something impossible in other languages.” It is: **C+ gives agents a version-matched language, semantic source graph, structured correction loop, and semantic runtime UI surface in one stack.**

This integration is implemented. The claim that it makes agents more accurate, faster overall, or cheaper in tokens has not yet been measured against other languages and agent harnesses.

Evidence: [C+ tooling](docs/lang/tooling.md), [structured diagnostics](cplus-core/src/diagnostics.rs), [agent core](vendor/agent_core/README.md), [Facet agent](vendor/facet_agent/README.md).

## 2. Which claimed C+ advantages are already demonstrated, and which remain hypotheses?

### Demonstrated in the repositories or existing project evidence

- C+ parses, type-checks, borrow-checks, monomorphizes, emits LLVM IR, and uses Clang to produce native objects, libraries, and executables.
- The language implements ownership and borrow checking, deterministic drop, tagged enums, generics, interfaces, C ABI interoperability, SIMD, atomics, threads, async/futures, and compiler-checked real-time restrictions.
- `cpc` supports project builds, checks, tests, formatting, documentation, package operations, target selection, sanitizers, debug information, code-graph queries, LSP, and MCP.
- `cpc-bindgen` has implementation for C, Objective-C, Swift, GObject Introspection, and Java/JNI input surfaces.
- The package model supports a substantial standard library and native SDK/framework bindings outside the compiler.
- Facet has working backends for AppKit, UIKit, GTK, and Android Views, with a shared control vocabulary and layout engine.
- Native capability packages exist for camera, foreground location, sensors, local notifications, permissions, secure storage, biometrics, haptics, sharing, file selection, and application links, with important platform limits.
- Source-level and live-application agent interfaces exist and include explicit authorization concepts.
- IRIS implements a persistent Kanban Board as a shared user-agent record, not a decorative project panel. Users can create and organize tasks; the agent is instructed to read the Board at the start of a turn and can visibly open cards, read their requirements, edit titles and bodies, set status/progress/blockers, post comments, move or reorder work, link cards, archive completed work, and delete with confirmation through the same semantic controls available to the user.
- An existing live-vision project provides a materially stronger cross-platform proof than a gallery: unchanged C+ application code accesses the camera, displays the live preview, performs detection per frame, and draws bounding boxes on macOS, iOS, and Linux. This demonstrates shared hardware-to-inference-to-UI application logic across three native platforms, not just shared static controls.

### Implemented, but not yet proven at product scale

- Facet is working on macOS/AppKit, iOS/UIKit, Linux/GTK, and Android Views. These are implemented backends, not four works in progress. Windows is the remaining Facet backend under development; C+ already has Windows code generation and the `win32` and `agent_win32` packages. The remaining validation question for the four working backends is product-level breadth: supported controls and device capabilities still need release matrices, accessibility checks, and real-device evidence per feature.
- The complete IRIS workflow is present in the IRIS repository, not just as disconnected C+ components. The intended flow is Board-first: the user defines or opens a task, asks the agent to work on it, and the agent uses the Board as persistent context and updates the card as work progresses. IRIS then connects that task to a persistent Claude Code session, streamed tool activity and permission requests, project files and an editable code view, compiler-backed completion/navigation/diagnostics, Git-based Saves and restoration, destination-aware run/deploy flows, and live Facet inspection. Its security model is also explicit: external control of IRIS is consent-gated, protected content is excluded from the default grant, and dangerous agent actions require a deliberate click. It is not yet a hardened authentication boundary, because MCP client names are self-reported, remembered trust is keyed by that name, there is no revocation UI, and an open IRIS gap records that `describe_tree` does not yet honor the excluded-subtree policy.
- One checked-in detail currently falls short of the Board-first product claim: `Workspace::restore_view()` remembers the user's last Board/Code selection but falls back to `"code"` when no preference exists. The product model and agent guidance are Board-first, but the first-run fallback should be changed to `"board"` before saying unqualifiedly that every new workspace opens on Kanban.
- Native SDK **reach** means C+ can name and call a large portion of an operating system SDK through generated C, Objective-C, Swift-bridge, GIR, or Java/JNI bindings. That establishes ABI access, but it is the lowest layer of support. A product-ready API also needs safe ownership and lifetime rules, C+/native type conversion, callback or async adaptation, main-thread scheduling, consistent errors, permission and lifecycle handling, required manifest or entitlement setup, documentation, examples, and simulator/device tests. Curated packages such as `camera`, `location`, `notifications`, and `securestore` add much of that higher layer for specific capabilities; the existence of a generated SDK directory alone does not.
- Editable source improves **recoverability** because the project is ordinary text and assets rather than an opaque builder format. IRIS adds autosave, visible compiler errors, source navigation, a terminal log, Git-backed Saves, whole-project and single-file restoration, and optional remote backup. That makes a failed generation inspectable and reversible. **Maintainability** is a higher standard: an unfamiliar developer must be able to understand ownership and Facet conventions, reproduce every platform build, modify native integrations, upgrade dependencies, diagnose failures, and ship again without the original team. The repository provides the mechanisms, but no independent onboarding or maintenance study yet measures that outcome.

### Still hypotheses

- C+ causes models to generate correct code more often than established languages.
- Structured compiler feedback materially reduces agent iterations.
- C+ reduces LLM token or context consumption enough to matter economically.
- Non-C+ experts can maintain generated projects successfully.
- The shared foundation lowers total cross-platform maintenance cost versus separate native applications.
- IRIS can produce production-useful applications reliably across a wide range of categories.
- IRIS is materially better than mature coding agents or prompt-to-app products for its target customer.

Evidence: [project README](README.md), [platform model](docs/lang/platforms.md), [technical assessment](report/CPLUS_PROJECT_OVERVIEW_2026-08-26.md), [IRIS workspace](../iris/src/screens/workspace.cplus), [IRIS agent runner](../iris/src/services/agent_runner.cplus), [IRIS consent policy](../iris/src/services/consent.cplus), and [IRIS open gaps](../iris/gaps/README.txt).

## 3. What does Facet guarantee today across platforms?

Facet can currently guarantee the following architecture. The AppKit, UIKit, GTK, and Android Views backends work today; Windows is the backend still in progress.

1. The application can express a shared UI tree, component model, state mutations, layout, keys, and handlers in C+.
2. C+ compiles the application to native machine code; there is no language VM or garbage collector.
3. A platform backend maps the Facet tree to AppKit on macOS, UIKit on iOS/iPadOS, GTK on Linux, and Android Views through JNI on Android.
4. Layout is calculated by the shared `flex_layout` engine. Platform backends own native view creation, input, measurement, painting integration, lifecycle, and platform behavior.
5. Backend differences are explicit in manifests and parity audits rather than silently described as equivalent. On Windows, C+ code generation and Win32 access already work, but a shared Facet tree cannot yet be rendered because `facet_win32` does not exist.

Facet does **not** currently guarantee exact feature parity, pixel-identical rendering, identical interaction behavior, identical accessibility behavior, or support for every native control on every platform. Some controls are composites assembled from native primitives, and some capabilities have no direct platform equivalent.

The current cross-backend static parity scan reports:

| Backend | Property/command bits named | Handlers wired | Shared-band bits named |
|---|---:|---:|---:|
| AppKit | 336/362 (92%) | 68/68 (100%) | 20/21 (95%) |
| UIKit | 323/362 (89%) | 65/68 (95%) | 18/21 (85%) |
| GTK | 358/362 (98%) | 68/68 (100%) | 19/21 (90%) |
| Android | 321/362 (88%) | 67/68 (98%) | 19/21 (90%) |

These percentages are source-accounting measurements from `vendor/facet_android/tools/parity.py`; they are not a maturity score and do not mean the four backends are unfinished. A backend may deliberately implement a platform-specific equivalent or document that a shared verb has no native meaning. Conversely, naming a bit in source is not proof that the behavior works correctly on a device. The separate AppKit gate currently identifies five properties and two handlers whose source-accounting decision needs to be updated, so the contract audit is not fully green even though the AppKit renderer is working.

Evidence: [Facet guide](vendor/facet/docs/guide.md), [Facet contract](vendor/facet/docs/contract.md), [AppKit manifest](vendor/facet_appkit/MANIFEST.md), [UIKit manifest](vendor/facet_uikit/MANIFEST.md), [GTK manifest](vendor/facet_gtk/MANIFEST.md), [Android manifest](vendor/facet_android/MANIFEST.md).

## 4. When we say “one description, every platform,” what does that mean technically and what are the exceptions?

Technically, it means that the same C+ application modules and Facet UI description can be compiled for several targets, while a platform backend interprets the shared tree using that platform’s native UI system.

The strongest concrete example is the existing live camera-detection project. The same C+ application code opens the camera, mounts a live preview, receives frames, runs detection, and draws bounding boxes on macOS, iOS, and Linux without changing that application source. That is the intended meaning of “one description”: the application-level behavior stays shared while Facet, capability packages, generated bindings, and native platform toolchains implement the target-specific boundary.

It does **not** mean one binary, one packaging process, identical output, or zero platform-specific work.

The platform boundary works in three ways:

- Platform-specific implementation files such as `module_ios.cplus` or `module_android.cplus` replace a base module when imports or SDK calls differ.
- Platform-specific manifest sections select dependencies and entry points.
- `#platform()`, `#arch()`, and `#target()` select values inside otherwise portable code. They cannot hide imports or unavailable APIs.

Expected exceptions include:

- A capability that has no equivalent on a platform, such as a desktop menu bar on iOS.
- A capability that exists but has different semantics, such as notification scheduling, file paths versus Android `content://` URIs, or background location policies.
- A backend feature that is not implemented yet.
- Platform metadata and permissions in `Info.plist`, entitlements, `AndroidManifest.xml`, signing, provisioning, and store configuration.
- Platform-specific assets, SDK initialization, or native bridge code.
- Final packaging: C+ emits archives for iOS and Android, while Xcode and the Android toolchain own the final application bundle.

A more accurate phrase would be: **“One shared application description, rendered by native platform backends, with explicit platform-specific extensions where required.”**

Evidence: [platforms and targets](docs/lang/platforms.md).

## 5. What native APIs and hardware capabilities are already supported?

“Supported” must distinguish an implemented package from a fully validated product capability. The repository currently contains the following application-facing surfaces:

| Capability | Current scope | Important limit |
|---|---|---|
| Native UI | AppKit, UIKit, GTK, Android Views | Backend parity and real-device behavior differ; no Facet Windows backend is present |
| Camera | macOS, iOS, Android; preview, still capture, live frames | iOS device validation is limited; camera frame/still orientation and front-camera mirroring remain incomplete |
| Shared live vision application | Unchanged C+ application source demonstrated on macOS, iOS, and Linux with camera preview, per-frame detection, and bounding-box overlays | This is evidence for a concrete end-to-end application, not proof that every camera model, orientation, accelerator, or device behaves identically; preserve the exact project revision and run evidence for each platform |
| Location | macOS, iOS, Android; one fix or foreground stream | No background tracking/service |
| Sensors | Accelerometer, gyroscope, magnetometer, barometer on iOS/Android where hardware permits | macOS usually reports unavailable; current host test is failing |
| Local notifications | macOS, iOS, Android; schedule, cancel, tap/action routing | No push-token provisioning; Android deferred delivery does not survive process death; Linux unsupported |
| Permissions | macOS/iOS and Android | Linux returns unsupported; application manifest entries remain the app’s responsibility |
| Secure storage | Apple Keychain and Android Keystore-backed encryption | Small secrets only; does not protect against a compromised running process |
| Biometrics | macOS, iOS, Android | Proves device-owner presence, not server identity |
| Haptics | macOS where hardware permits, iOS, Android | Acceptance of a request is not proof the user felt it |
| Share sheet | macOS, iOS, Android | Android file sharing still needs an app-level `FileProvider` |
| File picker | macOS, iOS, Android | Android returns provider URIs; save behavior and filters differ |
| App/deep links | Facet application-event path across supported app backends | Platform manifests, entitlements, and web association files are still required |
| Web and hybrid web views | Present in Facet backends | Backend behavior and process lifecycle vary |
| Apple SDKs | Generated/hand-wrapped AppKit, UIKit, AVFoundation, CoreLocation, CoreMotion, CoreML, Vision, MapKit, Metal, WebKit and related APIs | Binding presence does not imply every SDK API has an ergonomic or validated wrapper |
| Android SDK access | JNI, Java binding generation, native Android view package, checked-in adapter DEX files | Some features require Java/Dex adapters or AndroidX/Gradle integration not supplied by the core stack |
| Linux APIs | GTK/libadwaita and GObject/GIR package stack | Device capability packages are much narrower than mobile |
| Windows | Native compiler target, Win32 bindings, and agent package | No current Facet Windows renderer |
| Embedded | ESP32 Xtensa and ESP32-C3 targets with real-time checks | Reduced standard-library surface and external ESP-IDF handoff |

The launch capability matrix should be generated from tested packages and real-device evidence, not inferred from the presence of a binding directory.

Evidence: package READMEs under `vendor/camera`, `vendor/location`, `vendor/sensors`, `vendor/notifications`, `vendor/permissions`, `vendor/securestore`, `vendor/biometrics`, `vendor/haptics`, `vendor/share`, `vendor/filepicker`, and `vendor/applinks`.

## 6. What happens when an application requires a platform-specific feature?

The application keeps a shared interface and supplies a platform implementation behind it.

The preferred path is:

1. Put the shared application contract and fallback behavior in portable C+.
2. Add a `_<platform>.cplus` implementation for platform-only imports and behavior.
3. Declare the required platform package in the matching manifest section.
4. Use an existing native binding package or generate bindings with `cpc-bindgen`.
5. Add the required application metadata, entitlement, permission, adapter, or external SDK to the native project shell.
6. Build and test that target separately; a platform-specific source file is not checked when building another target.

If a safe or ergonomic binding does not exist, the feature requires package or bridge work before an agent can use it reliably. C+ can call C directly and has generators for Objective-C, Swift bridges, GObject metadata, and Java/JNI, but generation is not a guarantee that every external SDK shape maps automatically.

The product should expose this honestly: IRIS may automate the platform extension, but it cannot abolish platform APIs, permissions, signing, SDK contracts, or review rules.

Evidence: [platform override model](docs/lang/platforms.md), [binding generator](cpc-bindgen/README.md).

## 7. How much of the final application is C+ versus generated platform-specific code?

There is no responsible fixed percentage. It depends on the application and on how far it stays inside the shared Facet and package surfaces.

For an application using supported Facet controls and supported capability packages:

- Application state, business logic, UI description, handlers, networking, and most shared behavior can be C+ source.
- The compiler turns that source and its C+ dependencies into native object code. The shipped binary no longer contains “C+ source”; it contains native machine code linked with system frameworks and libraries.
- Facet backends and platform packages contain the platform-specific implementation. Much of that is C+ calling native APIs through generated or hand-written bindings.
- iOS still needs an Xcode-owned application shell, metadata, signing, and final link.
- Android still needs an application package, manifest, resources, and native/Java integration. Several packages include small checked-in DEX adapters because certain Android callbacks cannot be implemented with reflective JNI alone.

When an application needs unsupported native behavior, the platform-specific share grows. That work may still be written in C+ against native bindings, but it is no longer shared code. In some cases it requires Objective-C, Swift, Java/Kotlin, C, build-system configuration, or a third-party SDK.

The live camera-detection project is the best present example of where the ratio can be highly favorable. Its application-level pipeline—camera access, preview, frame handling, detection, and bounding-box presentation—stays in unchanged C+ across macOS, iOS, and Linux. Platform backends and native SDK bindings still exist beneath that source, and each target still has its own toolchain, permission metadata, packaging, and validation. “Unchanged application code” therefore does not mean “no platform implementation exists”; it means the platform implementation is reusable infrastructure rather than a fork of the product feature.

The promise should be “maximize shared application code while preserving direct native escape hatches,” not “100% C+” or “no platform code.”

## 8. Can an external developer understand, debug, and modify the generated application without being a C+ expert?

They can inspect, debug, restore, and modify the complete project. They do not need to be a compiler expert, but they will need to learn the parts of C+ and Facet used by the application. “Editable by a developer” is defensible today; “maintainable by anyone without learning C+” is not.

What makes recovery practical:

- IRIS projects are normal source files, manifests, assets, and native platform shells. The agent edits the same files an external developer receives; there is no private visual-builder database required to reconstruct the application.
- IRIS exposes the code rather than hiding it: a file tree, multiple editable tabs, syntax highlighting, formatting, autosave, inline diagnostics, a Problems list, compiler-backed completion, go-to-definition, find-references, callers, callees, file symbols, and type-at-caret.
- IRIS's Saves surface uses Git. Before a whole-project or single-file restore overwrites current work, it first commits the current tree, so recovery itself does not discard the state being recovered from. A configured remote can be used as a backup.
- Builds and deployment steps run visibly in the terminal drawer. A failed compile, link, signing, installation, or launch leaves the underlying tool output available instead of collapsing it into a generic builder error.
- C+ provides a version-matched language reference and structured diagnostic codes. Builds can emit DWARF and use the supported sanitizers, while C ABI interoperability allows native debuggers and platform tools to participate.

What the developer still has to learn or reproduce:

- C+ ownership modes (`take`, `ref`, `view`, `opaque`, `drop`), explicit casts and pointer rules, package manifests, platform override files, and Facet's retained tree/state model are project concepts, not familiar Swift/Kotlin/TypeScript conventions.
- Generated native bindings are intentionally close to the foreign SDK. Debugging code at that layer requires knowledge of AppKit/UIKit, Android/JNI, GTK/GObject, or the relevant C SDK in addition to C+.
- iOS signing and provisioning, Android manifests/SDK tooling, permissions, entitlements, store metadata, and third-party SDK setup remain native-platform responsibilities even when IRIS automates common paths.
- C+ has a much smaller community, ecosystem, production history, and pool of experienced maintainers than established application languages.
- The current C+ development head has known correctness and test-gate issues listed below. An external maintainer would need a pinned, released compiler/package set and reproducible build instructions rather than an arbitrary development checkout.

So the claim should be separated into two parts:

- **Recoverability is implemented:** source, diagnostics, logs, semantic navigation, versioned Saves, and restoration are present.
- **Independent maintainability is unproven:** there is not yet evidence from developers outside the C+/IRIS team maintaining generated projects over time.

The right validation is an external handoff study. Give developers a pinned IRIS-generated project and ask them to reproduce all target builds, fix a defect, add a shared feature, add one platform-specific feature, upgrade a dependency, and ship a new build. Measure time-to-first-build, successful changes, regressions, documentation gaps, and how often the core team must intervene.

Evidence: [IRIS editor](../iris/src/screens/workspace/editor_tabs.cplus), [compiler service](../iris/src/services/cpc.cplus), [Saves service](../iris/src/screens/workspace/git_service.cplus), and [run/deploy workflow](../iris/src/screens/workspace.cplus).

## 9. How does IRIS handle agent context, project understanding, code navigation, and compiler feedback?

IRIS implements this as a connected workflow rather than leaving it to a hypothetical orchestration layer.

### Board-first task context

- The primary product model is the **Board**, not a blank editor. A user can put work into Kanban cards, organize it into lanes, open a specific card, and tell the agent to work on “this card.” This lets a non-technical user manage intent and progress without navigating source files.
- The Board is persistent project state with editable columns and ordered cards. A card carries a stable `CARD-00N` code, title, body, status, progress, blocked reason, links, archive state, and a timestamped comment trail. The default lane template is `IDEAS`, `AGENTS WORKING`, `NEEDS YOU`, and `SHIPPED`, but lanes are ordinary editable data rather than hard-coded workflow states.
- The agent is explicitly made aware of this workflow through project instructions generated by IRIS. Those instructions say that the Board is the shared record, require the agent to read it at the start of a turn, keep it current while working, leave a comment describing what changed, and record the reason when work is blocked.
- The agent reads and manipulates cards through IRIS's visible semantic UI using `describe_ui`, `click`, and `set_text`. It has no hidden Board API: reading a card means visibly opening it, and changing status, progress, body, comments, lane, order, links, archive state, or deletion goes through the same keyed controls a person can use. This is an important trust property because the user's screen remains the authoritative, observable state.
- Shared attention is explicit. When the user says “this card,” the agent resolves the card sheet that is actually open; if no card is open, its instructions say to ask rather than guess. The Board survives a scrolled-away conversation, restarted CLI session, or closed laptop, so task intent and work history do not depend on the chat transcript.

This is a substantive IRIS differentiator: the agent is attached to a persistent project-management surface, not only to files and a conversation. One implementation correction remains before calling Kanban the literal default in every fresh workspace: the current `restore_view()` fallback is `"code"`, although it remembers Board after the user selects it. Change that fallback to `"board"` and add a first-open UI test so implementation and product positioning agree.

### Agent context and conversation state

- IRIS currently drives **Claude Code** as a persistent subprocess using its streaming JSON control protocol. It streams partial replies, displays tool calls and results, supports the agent's multiple-choice questions, lets the user stop a turn, preserves prompts queued during an active turn, and keeps the process alive between messages.
- Conversation history comes from Claude Code's own project session files. IRIS watches those files, rebuilds the visible transcript, supports named history, resumes an earlier session by UUID, and creates a clean new chat when requested.
- The prompt can carry selected file attachments. Project instructions are made explicit through a generated section in `AGENTS.md`; because Claude Code loads `CLAUDE.md` rather than `AGENTS.md`, IRIS writes a small `@AGENTS.md` import so the guidance actually enters the agent's context.
- The model menu is populated from Claude Code's live initialization response, so it reflects the installed CLI and account. The code contains future agent enum entries and install probes for Codex and Antigravity, but the turn runner and model catalog are **Claude-only today**.

### Project understanding and code navigation

- `cpc init` supplies the project's `.mcp.json` for the compiler server. IRIS adds a separate, launch-specific `.iris/mcp.json` for the visible IDE surface and passes both to Claude Code. The agent can therefore ask the compiler about resolved code and can operate the same IRIS window the user sees.
- IRIS also embeds compiler services directly. A resident `cpc mcp` child builds the typed graph once, is warmed when a project opens, accepts unsaved buffers, rebuilds in the background, and keeps answering from the last valid graph while the user has half-typed syntax.
- The editor exposes resolved go-to-definition, references, callers, callees, file symbols, type-at-caret, scope-aware completion, member completion, and module completion. Results carry source file, line, column, signature, and symbol identity and can be opened from the Results pane.
- A recursive file watcher updates the project view after either the user or agent changes a file. Board state and source state therefore form two complementary records: the Board says what is being attempted and what happened, while the working tree contains the implementation.

### Compiler feedback and runtime correction

- Autosave triggers `cpc check` off the UI thread. IRIS paints diagnostic spans in the editor and publishes a Problems pane with file/severity counts. Dependency diagnostics are filtered from the primary list but counted and disclosed rather than silently erased.
- Run uses one ordered, visible plan. IRIS detects installed toolchains and live destinations, then builds and launches on the host or assembles, signs, installs, and launches iOS simulator/device and Android emulator/device builds as required. Failures retain the original terminal output and update the visible status.
- The running Facet application exposes semantic control keys. On desktop, IRIS's Inspect tab discovers the app independently of how it was launched, reads its live tree, highlights controls, edits properties, and records changes. The current working tree is adding HTTP endpoint discovery for simulators/devices; that mobile Inspect connection is not yet a completed, verified path.
- The agent can also reach the running application's own MCP surface and verify behavior with `describe_ui`, `click`, `set_text`, and related verbs. This is the closed-loop step: change source, compile, run, inspect the actual native application, act on it, and read the resulting state.

### Security boundaries

IRIS has two relevant permission layers:

1. For Claude Code tool requests, IRIS displays a permission card. Routine reads and in-project edits receive a five-second visible countdown that the user can hold; destructive commands, privilege changes, dangerous Git/package/container operations, paths the CLI already marked blocked, and unknown tools require an explicit click. This mediates Claude Code's permission protocol; the external CLI still owns its sandbox and underlying filesystem/network enforcement.
2. For another agent controlling the IRIS window over MCP, the first connection is gated by a visible **Allow Once / Always Allow / Deny** sheet. An allowed client gets read-and-act capability but not protected-content access. The HTTP listener is loopback-only and the Unix socket is process-specific. However, `clientInfo.name` is self-reported rather than authenticated, remembered trust is keyed by that name, there is no revocation UI yet, and the open `describe_tree` exclusion gap must be closed before presenting this as a hardened security boundary.

What IRIS does not yet implement is a general provider-agnostic context scheduler, a measured token-budget policy, or a proven multi-agent orchestration system. The implemented product is a Claude Code host with unusually deep C+/Facet source, build, and runtime integration.

Evidence: [agent runner](../iris/src/services/agent_runner.cplus), [conversation history](../iris/src/services/chat_history.cplus), [project guidance](../iris/src/services/agents_md.cplus), [resident graph](../iris/src/services/graph_service.cplus), [editor tabs](../iris/src/screens/workspace/editor_tabs.cplus), [destination-aware run flow](../iris/src/screens/workspace.cplus), [Inspect panel](../iris/src/screens/workspace/inspect_panel.cplus), [danger classification](../iris/src/services/danger.cplus), and [consent policy](../iris/src/services/consent.cplus).

## 10. What makes IRIS materially better than Cursor/Windsurf plus Claude on a standard stack?

There is not yet evidence for a general claim that IRIS is materially better.

Cursor already describes an agent that searches a codebase, edits files, runs commands, uses a browser, and supports multiple frontier models. Cascade’s current official documentation similarly lists code/chat modes, model selection, planning, terminal access, MCP, checkpoints, real-time awareness, and application deployment. Chat, IDE integration, model choice, code search, terminal use, and autonomous edits are therefore category expectations, not IRIS differentiators. See the official [Cursor Agent overview](https://cursor.com/docs/agent/overview) and [Cascade overview](https://docs.devin.ai/desktop/cascade/cascade).

IRIS’s credible differentiation would be vertical integration around the application outcome:

- A Board-first IDE where a founder or product owner creates tasks in Kanban and the agent can read, execute, and visibly update those same cards. The unit of collaboration is a product task, not merely the currently open file or chat message.
- A language and package set deliberately constrained for agent authorship.
- Compiler-resolved source context rather than only editor-level search or embeddings.
- Structured compiler correction and a version-matched language/package reference.
- A shared native application framework plus built-in macOS, iOS, and Android build/run plans that discover real simulators, devices, and installed toolchains.
- Semantic inspection and operation of the running application through the same control keys used in source, rather than relying only on screenshots and coordinates.
- A visible agent permission flow and Git-backed Saves/restoration designed for a user supervising generated work.
- A concrete path from prompt to editable source, structured diagnostics, native build, target installation, and runtime verification in one focused product.

Those differentiators are present in the IRIS code. The performance advantage is not yet demonstrated. Cursor or Cascade can run compilers, use MCP servers, and drive browsers; an expert could connect them to the C+ compiler and application surface too. IRIS's case is that these pieces are product defaults with native-app-specific UX, not that competitors are technically unable to reproduce the loop.

To claim “materially better,” IRIS needs a head-to-head benchmark using the same Claude model and native-app tasks: successful build rate, time to a working application, tool calls, input/output tokens, human interventions, defects after change, device deployment success, runtime-verification coverage, and maintenance tasks after generation.

Until then, the accurate statement is: **IRIS is more deeply specialized around the complete C+/Facet native-application loop; Cursor and Cascade are broader, more mature general coding-agent environments.** IRIS currently hosts Claude Code only, so provider breadth is not a present differentiator. Existing agents could also use `cpc mcp`, which makes them potential channels or complements as well as competitors.

## 11. What makes IRIS materially better than Rork for a founder who only wants to ship an app?

For a founder whose only criterion is the shortest path to publishing, IRIS is not yet demonstrated to be better. Rork currently advertises browser-based generation, builds, and publishing for iPhone, Android, and web, and paid users can export and two-way-sync generated code through GitHub. IRIS should not compete by claiming that Rork only produces wrappers, provides no source, or cannot publish; those claims would be inaccurate. See Rork’s official [product documentation](https://docs.rork.com/) and [code export documentation](https://docs.rork.com/faq/code-export).

IRIS can become preferable for a narrower customer with one or more of these needs:

- A product-management-first workflow: put desired work on a Kanban Board, ask the agent to execute a selected card, and return later to its status, blocker, progress, and comment trail without having to direct work through files.
- Direct native platform and hardware integration beyond the builder’s supported catalog.
- Proven reuse for a demanding native feature: one unchanged C+ application source already drives camera preview, live detection, and bounding-box rendering on macOS, iOS, and Linux.
- A native systems-language foundation with no managed VM or garbage collector.
- Shared application code extending beyond mobile into desktop or constrained targets.
- Local, editable projects with compiler-level semantic tooling and a direct C/native SDK escape hatch.
- Native target discovery and visible build, signing, installation, and launch output instead of a hosted black box.
- Semantic inspection and live operation of the running native UI, plus Git-backed Saves and per-file/project restoration.
- Long-lived maintenance in the same source-based environment that generated the application.

The tradeoff is important for a founder. IRIS today is a local macOS IDE, requires the C+ toolchain and Claude Code, and expects native SDKs such as Xcode or the Android SDK for mobile deployment. It can run on iOS simulators and physical devices and on Android emulators/devices, but its distributable export flow is currently macOS-only; it does not yet provide a complete App Store/Play Store publishing service or a web target. Rork therefore remains the stronger current answer when zero-setup hosted generation and the shortest publishing path are the only priorities.

IRIS is the stronger technical proposition when the application needs native depth, transparent source, local control, desktop-plus-mobile reuse, runtime introspection, or an escape hatch below a hosted builder's catalog. That advantage must still be validated on applications that actually hit a native or maintenance ceiling, not on a simple app Rork already ships well.

Evidence: [IRIS destination discovery](../iris/src/services/destinations.cplus), [iOS deployment](../iris/src/services/ios_deploy.cplus), [Android deployment](../iris/src/services/android_deploy.cplus), [macOS export](../iris/src/services/export_mac.cplus), and [IRIS agent/model support](../iris/src/services/models.cplus).

## 12. What is the smallest technical promise we can make today that is unquestionably true?

The smallest promise supported by the C+ and IRIS repositories together is:

> **IRIS gives the user and Claude Code a shared Kanban Board for defining and tracking work, then connects that work to editable C+/Facet source, compiler feedback, native build/run controls, and live application inspection. Facet renders shared application code through working native backends on macOS, iOS, Linux, and Android.**

A fuller promise that captures IRIS's actual product shape is:

> **Start with work on a Kanban Board, ask the agent to execute a card, and follow its visible progress into editable source, native builds, and live application verification. C+ and Facet can keep substantial application code shared across macOS, iOS, Linux, and Android; an existing camera-detection application runs unchanged C+ code on macOS, iOS, and Linux.**

For a shorter customer-facing statement:

> **Plan work on a Kanban Board, ask an AI agent to build it, and verify the native app live.**

The technical qualification is that IRIS's current integrated agent is Claude Code; macOS, iOS, and Android have concrete run/deploy paths in IRIS; Linux Facet works when built on a Linux host; and Windows Facet remains in progress even though the compiler and Win32 layer work.

Use claims such as “production-ready” only with an explicit supported version, platform, capability, and test matrix. Avoid unqualified claims such as “any app,” “every native API,” “one description with no platform work,” “maintainable by anyone,” “fewer tokens,” “provider-agnostic,” or “better than Cursor/Rork” until each has its own evidence.

## Current engineering status and confidence

This document was checked against the current C+ development head and the actual IRIS repository, not only the older 26 August assessment.

### How to interpret the open items

An active software project can have defects, incomplete features, and failing development gates while its main workflows continue to work. The existence of an issue is therefore not enough to judge the product. The useful questions are:

- Does it affect the customer workflow being demonstrated or a separate tool/feature?
- Is the failure reproduced and bounded?
- Is there a workaround?
- Does it block a demo, a specific supported release, or only a broad future claim?
- Is it a runtime defect, a test/audit failure, or missing evidence?

Nothing in the list below says that C+, Facet, or IRIS is generally unusable. It describes the difference between a working development system and a fully evidenced release across every claimed platform and capability.

### Positive evidence and point of reference

The checks cited in this review contain **at least 1,990 passing tests**: 229 in `agent_core`, 371 in `camera`, 193 in `location`, 161 in `filepicker`, 372 in `notifications`, 184 in `permissions`, 156 in `share`, 154 in `securestore`, and 170 passing tests in the `cpc-bindgen` target that has one failing assertion. The Facet cross-backend parity script also completed and accounted for every Android difference as implemented or documented.

The IRIS repository contains another 355 inline C+ tests covering deployment planning, agent protocol handling, consent, danger classification, graph queries, project files, Saves, and the Board. Those IRIS tests were inspected but not run during this pass because the working tree already contained active uncommitted development.

Test counts are not a quality score and host tests do not replace real-device, visual, accessibility, signing, or store-distribution checks. They do provide the missing scale: the current concerns are a small number of scoped items inside a substantial tested codebase, not evidence that the whole system is failing.

### IRIS implementation audit

- The IRIS source reviewed is a working tree based on commit `d88129b` with pre-existing uncommitted changes. Those changes were preserved and included in the implementation reading where relevant.
- The code confirms a connected Claude Code workflow, compiler and IRIS MCP configuration, project/session context, source editing and semantic navigation, structured diagnostics, target-aware run/deploy plans, a persistent Board, Git-backed Saves, and live desktop inspection. The Board supports visible agent reads and edits for card content, status, progress, blockers, comments, movement, ordering, links, archiving, and deletion.
- Only Claude Code is currently wired as the active turn runner. Windows has no Facet renderer, distributable export is currently macOS-only, and mobile Inspect transport is still being completed. These are product-scope boundaries, not failures of the supported workflow.
- External MCP control is consent-gated, but the open `describe_tree` exclusion issue means it should not yet be described as a hardened authentication/security boundary. This limits a security claim; it does not prevent the normal local IRIS workflow.

### Open-item impact table

| Open item | Classification | Practical scope and impact | What it blocks |
|---|---|---|---|
| Fresh IRIS workspace falls back to Code instead of Board | Minor UX/configuration mismatch | The Board is implemented and the agent understands and manipulates it. IRIS also remembers Board once selected. The checked-in fallback value is simply `"code"` rather than `"board"`. | Blocks the literal claim that every first launch defaults to Kanban until the fallback and first-open test are changed. It does not block Board use or agent task manipulation. |
| Live camera-detection evidence is not attached to this review | Evidence-pack gap, not a known product defect | The founder reports unchanged C+ code running camera preview, live detection, and bounding boxes on macOS, iOS, and Linux. The exact project path/revision and three-platform run artifacts were not available in the two repositories inspected here. | Blocks independent reproduction of that proof during diligence. It does not imply the demonstrated application failed. |
| One `cpc-bindgen` Objective-C unit assertion fails | Narrow developer-tool regression | **170 of 171 tests pass.** The failure concerns how one locally known Objective-C wrapper type versus a foreign type is represented in generated output. Existing checked-in applications and bindings are not shown failing by this test. | Blocks a completely green binding-generator CI gate. It matters when generating or regenerating that class of Objective-C binding, not for every compiled application. |
| AppKit verb-coverage check reports five properties and two handlers | Contract-accounting gap | This script checks whether every shared Facet verb has an explicit AppKit implementation or documented decision. An unaccounted entry can be missing behavior or missing audit metadata; the check alone does not show a user-visible failure. | Blocks declaring the static AppKit parity audit complete. Each of the seven entries should be classified or implemented before publishing a complete parity matrix. |
| Four package test binaries end with signal 5 | Test-execution issue requiring triage | The affected test runners are `facet`, `agent_mcp`, `facet_agent`, and `sensors`. A signal from a test binary does not by itself prove that shipping applications crash; it means those suites did not provide a trustworthy pass/fail result in this environment. | Blocks saying the full development-head test suite is green. It does not, without a reproducer in an application workflow, establish that the IRIS IDE or a Facet application is blocked. |
| `ref` field write followed immediately by a branch can read the previous value | **High-severity but specific compiler correctness defect** | The reproduced shape is a write through a `ref` receiver/parameter followed by an immediate condition on that field. Plain locals and raw-pointer access are correct. Computing the new value into a local, storing it, and branching on the local is a known workaround already used by the camera probe. | Blocks releasing the affected compiler version as generally correctness-clean until fixed and similar patterns are audited. It does **not** mean all C+ code is miscompiled or that the current camera demonstration is blocked. |
| Camera orientation work is incomplete | Feature-scoped limitation | Back-camera preview rotation was fixed and verified on iPad and Galaxy Fold paths. Open work concerns sensor-oriented frame buffers, still-image orientation metadata, and front-camera mirroring. Desktop displays do not rotate, so macOS does not exercise the same issue. | Blocks claiming orientation-complete camera behavior on every mobile device. It does not block basic camera preview, live frames, or detection in the validated configurations. |
| Development head is 313 commits beyond `v0.0.27` | Release-management status, not a failure | The newest mobile and capability work exists on the development head rather than in the latest tagged release. | Blocks presenting the new work as part of a published stable version until it is tagged, packaged, and accompanied by release evidence. |

### Release decision in plain language

There is no evidence here of a single issue that invalidates the architecture or prevents every demo and application from working. The appropriate decision depends on what is being shipped:

- **For an internal or investor demonstration:** use a pinned known-working revision and the already validated workflows. None of the narrow tooling/audit items alone blocks that.
- **For a limited pilot:** state the supported platforms and capabilities, pin the compiler/packages, use the known compiler workaround, and test the exact customer workflow on the target devices.
- **For a broad public production claim:** fix the compiler correctness defect, obtain clean results from the incomplete test runners, close or classify the parity entries, and publish a reproducible device/build matrix for the promised targets.

That is normal release discipline for a large native, cross-platform system with compiler, framework, IDE, device, and agent layers. The open list is useful because it makes the remaining work visible and bounded; it should not be presented as if every item has equal severity or as if every unresolved test blocks the working product.
