# Standalone WinUI agent verification

Real WinUI controls, generated bindings, and the shared agent packages. No Facet.
The app runs assertions and closes itself. Logs are in `out/run.log` and `out/run.err`.

From the repository root: `./tools/test_agent_winui.ps1` (or `-Release`).
This also runs the Facet connector test. See [agent_winui](../../vendor/agent_winui/README.md)
for the API and supported operations.
