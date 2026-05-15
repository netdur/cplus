# `tiny_artifact` — bundled-artifact package smoke test

The canonical reference for a C+ package that ships a prebuilt static
archive instead of source. Used by Phase 2 Slice 2D to demonstrate the
manifest-is-truth contract on `[link].bundled` / `[link].triples`.

## Layout

```
tiny_artifact/                       # consumer
├── Cplus.toml                       # [dependencies] tiny_artifact = "*"
├── src/main.cplus                   # imports tiny_artifact/api
└── vendor/
    └── tiny_artifact/               # vendored package
        ├── Cplus.toml               # [link] bundled, triples
        ├── src/
        │   ├── api.cplus            # extern fn + pub wrapper
        │   └── lib/<triple>/        # libtiny_artifact.a goes here
        └── upstream/                # package author's C source
            ├── tiny_artifact.c
            └── build.sh             # rebuilds the .a for the host
```

`upstream/` lives outside `src/` so it's absolutely clear cpc never
treats those files as C+ source. The build driver only looks at
`src/lib/<host-triple>/`.

## Running

```bash
vendor/tiny_artifact/upstream/build.sh   # (re)build libtiny_artifact.a for this host
cpc build                                # link the consumer against the bundled .a
./target/debug/tiny_artifact             # prints nothing; exits with 42
```

If your host triple isn't in `vendor/tiny_artifact/Cplus.toml`'s
`[link].triples` list, the build will fail with **E0862** naming the
host and the supported triples — add yours and rerun `build.sh`.

## What it proves

| Check | Surface |
|-------|---------|
| Vendor manifest is loaded and validated | E0854/E0855 if missing or name mismatch |
| Host triple is recognized | E0862 if absent from `[link].triples` |
| Declared `.a` exists | E0860 if not at `src/lib/<host>/libtiny_artifact.a` |
| No orphan `.a` files | E0861 if extra binaries are present but undeclared |
| Bundled artifact reaches the linker | Final binary calls the C function via FFI |
