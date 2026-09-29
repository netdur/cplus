# facet_agent_winui_smoke

This is a C+ project. C+ is a young language, so **do not write it from
memory** — the toolchain answers every question about it, offline and
version-matched to this project.

## Before you write any C+

Run `cpc skill`. It prints the language reference, and inside a project
it also prints the reference of every dependency that ships one — facet
contributes several hundred lines about its retained, non-reactive model
and the mistakes that compile anyway.

There is deliberately no SKILL.md checked in beside this file. A copy
drifts from the compiler that wrote it; `cpc skill` cannot, because it
IS the compiler answering — and it is the only form that also carries
your dependencies' references.

`cpc skill --lang-only` is the language alone, if that is all you need.

## When the compiler says no

Run `cpc explain <CODE>` before you guess. Every diagnostic code has a
cause, a fix and a worked example behind it — `cpc explain E0613` is
faster and more reliable than inferring from the message.

## Navigating this code

**Do not grep for definitions.** C+ has no dynamic dispatch, so every
call to a named function resolves and the graph's answer is COMPLETE —
which grep's never is:

```
cpc query definition <symbol>     where is it
cpc query references <symbol>     everywhere it is used
cpc query callers <symbol>        who calls it
cpc query symbols <file>          the outline of a file
cpc query scope-at <file:line:col> what you can type right there
cpc query complete <file:line:col> ...and what fits after a `.` or `::`
```

The same graph is available as MCP tools — see `.mcp.json`, which points
at `cpc mcp`. Prefer either over reading files to find things. Each
`cpc query` rebuilds the whole graph (~seconds on a large project) and
throws it away; the MCP server builds once and answers in microseconds,
so use it for anything more than a single lookup.

## Building

```
cpc build          compile and link
cpc test           run the tests
cpc fmt            canonical formatting (no arg = this project)
```

<!-- Sections below this line are written by your IDE and are rewritten
when it opens the project. Edit above the line, not below it. -->
