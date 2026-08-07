# agent_inapp

Transport-free access to a live `agent_core::Backend` for an assistant embedded
inside the application. Every operation checks `auth::Channel::InApp`, then
uses the same backend surface as the external `agent_mcp` bridge.

```cplus
fn policy(req: auth::Request) -> auth::Decision {
    return match req.channel {
        auth::Channel::InApp => auth::Decision::Allow,
        auth::Channel::External => auth::Decision::Reject,
    };
}

let session: inapp::Session = inapp::open(surface, backend, policy);
let tree = session.describe_ui();
let acted = session.click("save");
```

For Facet applications, `facet/agent::in_app(policy)` supplies the attached
surface and backend automatically. The model-provider loop is intentionally
outside this package: translate its tool calls to `describe_ui`, `click`,
`set_text`, `scroll_to`, and `hit_test`, then return the typed outcome.
