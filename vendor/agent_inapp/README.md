# agent_inapp

Transport-free access to a live `agent_core::Backend` for an assistant embedded
inside the application. A session carries an explicit grant and uses the same
backend surface as the external `agent_mcp` bridge.

Backends that track edits expose `session.text_version(id) -> Option[u64]`.
Unwrap `Some(version)` and pass the number to `set_text` as `base_version`; `None` means unavailable
or unauthorized. WinUI uses synchronous text-change notifications for this stamp.

```cplus
let session: inapp::Session = inapp::open(surface, backend);
let tree = session.describe_ui();
let acted = session.click("save");
```

Use `open_with_grant(surface, backend, grant)` for a narrower or wider session.
For Facet applications, `facet_agent/agent::in_app()` supplies the attached
surface and backend automatically. The model-provider loop is intentionally
outside this package: translate its tool calls to `describe_ui`, `click`,
`set_text`, `scroll_to`, and `hit_test`, then return the typed outcome.
