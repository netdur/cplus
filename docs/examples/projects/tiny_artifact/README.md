# `tiny_artifact` — bundled-artifact package smoke test

The canonical reference for a C+ package that ships a prebuilt static
archive instead of source. Used by Phase 2 Slice 2D to demonstrate the
manifest-is-truth contract on `[link].bundled` and target-derived slices.

## Layout

```
tiny_artifact/                       # consumer
├── Cplus.toml                       # [dependencies] tiny_artifact = "*"
├── src/main.cplus                   # imports tiny_artifact/api
└── vendor/
    └── tiny_artifact/               # vendored package
        ├── Cplus.toml               # [link].bundled declaration
        ├── lib/<triple>/             # libtiny_artifact.a goes here
        ├── src/
        │   └── api.cplus            # extern fn + public wrapper
        └── upstream/                # package author's C source
            ├── tiny_artifact.c
            └── build.sh             # rebuilds the .a for the host
```

`upstream/` lives outside `src/` so it's absolutely clear cpc never
treats those files as C+ source. The build driver looks for
`lib/<target-triple>/` at the package root.

## Running

```bash
vendor/tiny_artifact/upstream/build.sh   # (re)build libtiny_artifact.a for this host
cpc build                                # link the consumer against the bundled .a
./target/debug/tiny_artifact             # prints nothing; exits with 42
```

The build script normalizes clang's host triple to the same stable spelling cpc
uses. For cross-compilation, place the corresponding archive under the target's
`lib/<target-triple>/` directory. With no directory for a target, cpc compiles
the package from source; an incomplete directory is **E0860**.

## What it proves

| Check | Surface |
|-------|---------|
| Vendor manifest is loaded and validated | E0854/E0855 if missing or name mismatch |
| Target slice is selected from the build target | no manifest triple list |
| Declared `.a` exists in a present slice | E0860 if absent from `lib/<target>/libtiny_artifact.a` |
| No orphan `.a` files | E0861 if extra binaries are present but undeclared |
| Bundled artifact reaches the linker | Final binary calls the C function via FFI |
