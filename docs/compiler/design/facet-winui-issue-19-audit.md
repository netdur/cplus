# Issue #19: technical claim audit

Investigated 2026-09-28 against the current checkout, Microsoft's documentation,
and the C++/WinRT headers generated for the working Qwen WinUI preview.
Issue: https://github.com/netdur/cplus/issues/19

**Follow-up, same date:** the authorized pure C+ probe now passes in debug and
release, including real pointer input and clean shutdown. See
[probe source, reproduction steps, and measured limitations](../../../playground/winui_probe/README.md).
Its GUIDs/slots are extracted from SDK headers; its application and COM lifetime
code are handwritten. It is not yet a WinMD generator or a Facet backend.
The audit below records the findings before that experiment.

Verdict: a direct C+ WinRT projection is technically credible. C++ is not an
intrinsic requirement. The issue nevertheless contains stale repository claims
and details that need correction before its proposed probe is implemented.
This audit did not build or run a pure C+ WinUI application. The existing visual
preview proves the appearance and C++ startup path, not the proposed C+ path.

## Confirmed foundations

- WinRT exposes a language-independent COM ABI. Ordinary WinRT interfaces have
  the six IInspectable slots; delegates instead have IUnknown plus Invoke.
  Parameterized IIDs are derived from type signatures using the specified hash
  algorithm. These are implementable without a C++ bridge.
  [WinRT type system](https://learn.microsoft.com/en-us/uwp/winrt-cref/winrt-type-system).
- WinMD describes types using ECMA-335 metadata. Microsoft's `windows-metadata`
  library is a plausible parser foundation; parsing metadata does not supply
  C+ ownership, callbacks, composition, or asynchronous execution semantics.
  [WinMD format](https://learn.microsoft.com/en-us/uwp/winrt-cref/winmd-files),
  [Microsoft parser](https://github.com/microsoft/windows-rs/tree/master/crates/libs/metadata).
- `cpc-bindgen/src/main.rs` has C/Objective-C, GObject, Java, and Swift paths,
  with no WinMD path found. Adding one is new generator work.
- More directly than WebView2, `vendor/share/src/share_backend_windows.cplus`
  already loads RoGetActivationFactory and WindowsCreateString and implements
  a WinRT event delegate in C+. This is source evidence, not a fresh runtime test.

## Corrections to the proposed implementation

1. **Do not assume RoActivateInstance constructs the three probe controls.**
   The installed SDK projection constructs Window through IWindowFactory,
   Canvas through ICanvasFactory, and Button through IButtonFactory, each using
   CreateInstance with outer/inner parameters. Follow those factory signatures.
   This audit did not experimentally establish whether a shortcut also works.
   Evidence: `playground/qwen_winui_preview/obj/Generated Files/winrt/`
   `Microsoft.UI.Xaml.h:13450` and `Microsoft.UI.Xaml.Controls.h:67932,68336`.

2. **“Structs pass by value” is insufficient guidance for a compiler backend.**
   Generated ABI declarations include Measure(Size) and Arrange(Rect).
   On Windows x64 an eight-byte aggregate and a sixteen-byte aggregate are
   passed differently; the latter requires an indirect argument. Verify C+
   indirect calls against an independent native ABI fixture. This is a test
   requirement, not a finding that C+ currently generates them incorrectly.
   [x64 calling convention](https://learn.microsoft.com/en-us/cpp/build/x64-calling-convention).

3. **Composition requires more than forwarding unknown QueryInterface calls.**
   Preserve controlling-object identity, delegated reference counting, and
   ownership of the inner interface. The generated `winrt/base.h` implements
   these separately. Event removal, callback lifetime, and error cleanup need
   equally explicit rules. Delegates and native COM interop must not receive
   an unconditional six-slot prefix.

4. **Startup includes resource deployment.** In the existing preview,
   XamlControlsResources is installed in OnLaunched; installing it during app
   construction previously failed with E_UNEXPECTED. Its project also copies
   Microsoft.UI.Xaml.Controls.pri to resources.pri to resolve theme resources.
   That is a measured workaround for this preview, not a universal packaging
   recipe. A code-built tree still uses WinUI's shipped templates/resources.
   The preview is self-contained and does not validate the issue's proposed
   framework-dependent bootstrap route.
   [Deployment distinction](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/self-contained-deploy/deploy-self-contained-apps).

5. **WinMD is not an independent sizeof oracle.** Field metadata is input to
   architecture-specific layout calculation. Compare generated sizes, offsets,
   and call behavior with SDK/native fixtures, not just the generator's own
   interpretation of the same metadata.

6. **Async needs an executor integration design.** `stdlib/future.cplus` exists,
   but documents compiler-created coroutine frames. A WinRT operation pointer
   cannot simply become a Future handle. Completion, cancellation, thread
   dispatch, and UI message pumping need an adapter and dedicated tests.

## Repository claims that are stale or too broad

- The six backends are not all stubs: share already uses WinRT; notifications
  implements Shell_NotifyIconW; permissions has notification-state and settings
  support. Biometrics, location, and sensors still contain unsupported paths.
  A projection enables implementation; it does not automatically finish those
  platform contracts or provide hardware.
- Switching Facet is not literally one import: runtime_windows.cplus imports
  backend, window, dialogs, and clipboard from facet_win32. Windows inspection
  also treats view handles as HWNDs and calls GetClientRect. These consumers
  need adaptation or deliberate retention of compatible Win32 services.
- The cited windows-rs PR really removed Xaml bindings in 2022, but its discussion
  distinguishes inbox Windows Xaml from Windows App SDK. It is historical
  context, not proof that a complete WinUI projection is straightforward.
  [PR #1836](https://github.com/microsoft/windows-rs/pull/1836).

## Recommended decision

**Follow-up:** the handwritten probe and then a generated standalone C+ WinUI
sample have passed native startup, real input, and shutdown in debug/release.
The direct projection approach is now demonstrated for the selected subset.
See [the standalone sample](../../../examples/winui_standalone/README.md) and
[adapter next steps](facet-winui-next-steps.md). The original decision below
records the investigation order; it is no longer an outstanding probe request.

Proceed with the issue's small handwritten C+ probe before generator work:
Application startup/composition, factory-created Window/Canvas/Button, visible
themed controls, click callback, and clean shutdown. Also exercise representative
aggregate arguments and reference lifetimes. Record exact runtime/deployment
requirements and HRESULTs. A passing probe would justify replacing the earlier
C++-bridge recommendation with a direct C+ projection; it would not establish
full Facet parity or general WinRT async support.
