# Spike: embedded agent inside a Facet application

Status: **working vertical slice** (2026-08-07). Revised 2026-08-07 after
review — the authorization finding below reverses the first draft's.

## Question

Facet already lets an external process inspect and control a running app over
`agent_mcp`. How should an assistant embedded in that same app see and control
the UI?

## Finding

Do not connect the application back to its own MCP socket. `agent_core` is the
access-and-control layer; everything else is an adapter over it. `agent_mcp`
adds a socket and JSON-RPC. `agent_inapp` adds nothing at all.

```text
model/provider loop
       │ typed tool calls
       ▼
agent_inapp::Session ── an adapter, and nothing else
       │
       ▼
agent_core::Backend ── describe / click / set_text / navigate / hit_test
       │
       ▼
agent_appkit::Surface ── exposure + affordance ceiling + version checks
       │
       ▼
live Facet native views and their real handlers
```

## Authorization does not belong in-process

The first draft admitted the embedded assistant through
`auth::Channel::InApp` and checked it on every call. **That was wrong**, and the
package no longer does it.

MCP checks a channel because a socket is a boundary between trust domains: any
process that can reach it can drive the app. In-process there is no such
boundary. The assistant was compiled into the binary by the same developer who
wrote the handlers, and it already holds the whole address space. A policy check
in front of `click` asks permission from itself — whoever could fail it could
call the handler directly instead.

What the check cost was not only ceremony. It made `inapp` diverge from `mcp`
in both directions at once: it carried an `Error::AccessDenied` that forced a
`Result` on every caller for a case that could never fire, and it had quietly
lost `poll_event`, so an embedded assistant could not observe events at all.
Nobody decided either.

**The caller is not what is untrusted here.** The untrusted input is the
model's output — tokens from a provider, shaped by whatever text the app fed it,
including content the user never wrote. Admission cannot check that.

## What does constrain it: the UI is the capability boundary

An agent driving this surface **has no hands**. It can only do what a person
could do through the interface, through the same handlers and the same
validation. That property is structural rather than declared: there is no list
of forbidden operations to keep in sync, and a hallucinated instruction can at
worst press a button a user could press.

Two consequences worth building on:

- **Confirmation can be physical.** If a destructive action routes through a
  surface the agent cannot reach, the human is in the loop by construction
  rather than by policy.
- **Exposure is an attention budget, not a wall.** The curated tree's job is
  keeping the model's context small and its affordances legible.

The agent reads only what the UI shows. It does not read application state
directly — not because state is secret, but because the moment it can read
something the UI never showed, "every agent action is a UI action" stops being
true, and that invariant is the whole safety story.

## Keeping the assistant out of its own tree

An embedded assistant must not reason about, or click, its own panel.

**Leaving the panel unkeyed does not achieve this**, which the first draft got
wrong. Exposure is per NODE: `identity::describe()` emits every exposed node and
re-parents it to its nearest exposed ancestor, so an unkeyed panel disappears
and every keyed control inside it remains, hanging off whatever was above. The
assistant would read its own Send button and be able to press it.

The mechanism is `Registry::set_excluded(node, on)`. Marking the subtree ROOT is
the whole declaration; descendants follow without being enumerated. It folds
into `is_exposed`, so one answer covers both what the agent sees and what it may
act on — `surface::authorize_action` gates on the same predicate. It lives on
the surface rather than in a consumer, so both adapters answer the same thing: a
subtree that is not the application's content is not content for an external
agent either.

## Adapter parity is enforced, not promised

`tools/adapter_parity.py` fails the build when the adapters disagree. It checks
that every `Backend` vtable field is reached by every adapter, and that the
adapters answer the same verb set. Both defects above — the missing `poll_event`
and the extra authorization — were drift rather than decisions, and drift is
what a guard is for.

## Recommended embedded-agent loop

The assistant panel owns conversation state and provider I/O. It does not own a
second UI representation.

1. The user submits a message.
2. In a worker, the controller calls `session.describe_ui()` and serializes the
   compact exposed tree. (`refresh_surface` hops to the main thread itself, so
   this is safe off it.)
3. It sends the user message, tree, and tool declarations to the model.
4. For every model tool call, it calls the matching `Session` method and sends
   the exact result back.
5. After navigation, `stale`, or a UI-change signal, it re-describes.
6. When the model returns text, the controller updates the panel on the main
   thread.

`describe_ui(full: true)` is the diagnostic view — structural nodes, frames,
classes. It is the oracle a test asserts against, the one view reporting what
the platform actually did rather than what the framework believes; it is how
every split-geometry bug this week was found. **It is not for a model's prompt**,
which is why the small tree is what you get without asking.

## Gaps before production

- A provider adapter and streaming chat controller are still product work; this
  spike stops at the tool boundary on purpose.
- **A modal blocks the loop.** `perform_on_main` uses `waitUntilDone: YES`, so an
  agent-initiated click whose handler opens a modal freezes the calling thread
  until a human dismisses it — and the panel is main-thread too, so it freezes
  with everything else. Recoverable, but it needs a timeout and a story.
- `agent_core::Request` carries only the channel. Conversation id and requested
  capability would be needed for finer policy — for the EXTERNAL channel.
- Events reach `poll_event` now, but nothing drives a describe from them. The
  safe first loop is still describe-after-action.
- The global Facet surface targets the most recently attached window, and
  `attach_window` replaces it — so a `Session` held across navigation silently
  retargets. A multi-window product needs an explicit selector.
- `facet/agent::admit` still returns `Allow` unconditionally, ignoring the
  channel. That is the EXTERNAL consent path and it is not implemented.
- `docs/examples/recipes/facet_inapp_agent` is an empty directory. The first
  draft listed it as landed.
- Add cancellation and serialize model turns so two tool loops cannot race each
  other or the user.

## Decision

Keep `agent_mcp` transport-specific. `agent_inapp` is that same surface minus
the transport and minus the authorization — nothing more, nothing less — and
`tools/adapter_parity.py` keeps it so.
