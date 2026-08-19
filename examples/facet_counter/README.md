# facet_counter

The worked example from [`vendor/facet/SKILL.md`](../../vendor/facet/SKILL.md) §2,
as a program that runs.

```bash
cd examples/facet_counter
ln -s ../../vendor/stdlib vendor/stdlib     # first time only
../../target/release/cpc build && ./target/debug/facet_counter
```

It is the canonical component shape, and nothing more: state is a field,
handlers and node helpers are methods, `build` runs **once**, and the handler
mutates the live tree through a typed cursor (`label::find`) rather than
rebuilding.

The skill file quotes this source. If you change one, change both — a reference
whose examples do not compile is worse than no reference at all.
