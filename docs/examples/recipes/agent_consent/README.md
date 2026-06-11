# agent_consent — reference consent middleware

`agent_core`'s `AuthGate` is a pure yes/no predicate: a bare
`fn(Request) -> Decision` with no memory and a channel-only `Request`. That is
deliberate — "ask the user", remembered choices, and standing settings are
*policy*, and policy belongs to the app, above the gate.

This recipe is that app-side layer: a small **consent middleware** a developer
drops between the agent bridge and the gate.

```
agent ──▶ app ──▶ consent::decide(...) ──▶ (dialog ──▶ user, only if needed)
```

## The decision flow

[`src/consent.cplus`](src/consent.cplus) — `decide(rules_dir, mode, agent_id, prompt)`
resolves one agent's request in three steps:

1. **Remembered rule.** If the user already chose for this agent (a rule
   persisted to disk), honor it — no prompt.
2. **Standing setting.** Otherwise the app's `Mode` decides: `AllowAll` (e.g. a
   trusted dev build) or `DenyAll` — no prompt.
3. **Ask.** Otherwise show the user a dialog, then **remember** the answer so the
   next request (and the next run) skips the prompt.

The result maps onto a real `auth::AuthGate` (`serve` if allowed, `deny_all`
otherwise), which `agent_mcp` already consults on every request.

## Notes

- **No closures in C+.** `prompt` is a bare `fn(str) -> auth::Decision`. A real
  app shows an `NSAlert` and returns the user's click; it reaches app state
  through globals, exactly like the policy fn. The test passes a preset.
- **Persistence** is one tiny file per agent under `rules_dir` (`allow` / `deny`).
  `agent_id` is used verbatim as the file name, so pass a filesystem-safe id
  (a PID / uuid — what `agent_core/identity` already hands out); a real
  deployment would hash or namespace it. A token layer could sit on top (store a
  grant credential beside each rule); the persisted `allow` already serves as
  the durable grant here.
- Runs headless — no GUI — so it is covered by an end-to-end build+run test
  (`agent_consent_middleware_*` in `cpc/tests/e2e.rs`).

## Run it

```bash
# from a checkout with vendor/stdlib and vendor/agent_core available
cpc build && ./target/debug/agent_consent
# -> consent middleware OK: remembered rule + standing mode + prompt-and-persist
```
