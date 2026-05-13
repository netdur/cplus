# CLI implementation benchmark

This directory, `proves/`, is the root directory for the controlled benchmark. All benchmark files, commands, reads, and writes must stay inside `proves/`. Do not read from, write to, create files in, delete files from, or otherwise touch the parent directory of `proves/` or any path outside `proves/`.

You will be invoked in a separate session for each combination of program and language. Each session is independent — assume you have no memory of other sessions and no knowledge of how the same program was implemented in another language.

## Directory layout

```
proves/
  README.md
  benchmark/
    programs/
      <NN-name>/
        spec.md           # the program specification (shared across all supported languages)
        tests/
          run.sh          # the test harness — exit 0 means success
          fixtures/       # input files, expected outputs
        cplus/            # C+ scaffold + your implementation
        swift/
        rust/
```

When you are invoked, you will be placed inside one of the language directories under `proves/benchmark/` (e.g. `proves/benchmark/programs/01-echo-rev/rust/`). Your working directory is that language directory. Paths such as `../spec.md` and `../tests/run.sh` refer to files inside `proves/`; they do not authorize accessing anything outside `proves/`.

## Your task per session

1. Read `../spec.md` for the program specification.
2. Implement the program in the assigned language so that `../tests/run.sh` exits with code 0.
3. Run `../tests/run.sh` yourself as often as you need. It is there for you to use.
4. When all tests pass, stop. Do not refactor, polish, or add features beyond the spec.

The directory you start in contains a minimal scaffold that compiles a hello-world. Do not delete the scaffold's build files (`Makefile`, `Cargo.toml`, `Package.swift`, etc.). The test harness expects the binary at the path the scaffold produces.

## Language rules

Use the standard library freely. External / third-party dependencies are allowed when they help. Prefer stdlib where reasonable; don't reinvent things that have well-known libraries.

- **C+**: use the project scaffold and its supported standard/runtime libraries. The local `SKILL.md` contains everything an AI agent needs to write C+ code for this benchmark.
- **Rust**: `std` and crates from crates.io.
- **Swift**: Swift stdlib, Foundation, and Swift Package Manager dependencies.

## Definition of done

The only criterion is: `../tests/run.sh` exits 0. No performance target, no style requirement, no documentation requirement. Functional correctness only.

If you believe a test is wrong, say so in your final message — but do not modify any files under `tests/`. The test suite is the contract.

If you cannot get all tests passing after reasonable effort, stop and report which tests fail and why. Do not silently leave a partial implementation pretending to be complete.

## What's being measured

These sessions are being measured for total tokens used and the number of times you self-correct after an error (compile error, test failure, or runtime crash). **Do not try to optimize for either number.** Work the way you normally would. The reason you're being told is so you understand why the test harness is set up the way it is — not so you change your behavior.

## What not to do

- Do not modify any files under `tests/`.
- Do not look at or read sibling language directories (`../cplus/`, `../swift/`, `../rust/`). Each language session must be independent. If you need to know what the program does, read `../spec.md`, not another implementation.
- Do not delete or restructure the scaffold's build system files unless it's strictly necessary and you've verified the test harness still finds the binary.
- Do not add your own tests under `tests/`. If you want scratch checks, run them ad-hoc in the shell or put them in a file outside `tests/` but still inside `proves/`.
- Do not touch the parent directory of `proves/` or any other path outside `proves/`.

## Build conventions

The scaffold's build command works out of the box on a hello-world before you change anything. If the obvious command fails (`make`, `cargo build`, `swift build`), inspect the scaffold first rather than rebuilding it from scratch.

The test harness `../tests/run.sh` knows where the scaffold puts the binary. If you change build output paths, you will break the harness — don't.
