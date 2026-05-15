# C+ Security Review

## Findings

During a high-level security review of the C+ codebase (`cplus-core`, `cpc`, `cpc-lsp`), one significant vulnerability pattern was identified: **Insecure Temporary File Creation (CWE-377)**.

### Insecure Temporary File Creation

Across the codebase, temporary files are created in the shared `/tmp` directory using highly predictable file names based on the process ID (`std::process::id()`).

#### Examples
In `cpc/src/main.rs`:
```rust
let tmp = env::temp_dir().join(format!("cpc-{}.ll", std::process::id()));
```

In `cpc/tests/e2e.rs` and `cpc-lsp/tests/e2e.rs`:
```rust
let p = std::env::temp_dir().join(format!("cpc-lsp-test-{}-{}", std::process::id(), n));
```

#### Impact
Because `/tmp` is world-writable on Unix-like operating systems, a local attacker can predict the filename that `cpc` will generate. The attacker can pre-create this file as a symbolic link pointing to a sensitive file owned by the victim (e.g., `~/.bashrc`, `/etc/passwd`). 

When the user runs `cpc`, the compiler will open the predictable path and write intermediate LLVM IR (or object files) to it. Because it is a symlink, the OS will follow it, causing the compiler to blindly overwrite the attacker's chosen target file. This can lead to **data destruction** or **privilege escalation**.

#### Recommendation
The application should use the standard Rust `tempfile` crate to securely delegate temporary file and directory creation to the OS. The `tempfile` crate ensures that files are created with randomized names and secure permissions, which prevents symlink attacks. 

Specifically:
- Add `tempfile = "3"` to `[workspace.dependencies]` in `Cargo.toml`.
- Replace instances of `env::temp_dir().join(...)` with `tempfile::Builder::new().prefix("cpc-").tempfile()`.
- Use `NamedTempFile::into_temp_path()` for paths that need to be passed to `clang`.

---

## Other Considerations

### Command Execution
The compiler shells out to `clang` using `std::process::Command::new("clang")` in `cpc/src/main.rs`. User input (such as linker arguments from `Cplus.toml`) is passed via `.arg()` correctly without invoking a shell. This protects against arbitrary shell command injection.

### Path Traversal
While resolving imports (e.g., `import "../../../etc/passwd"`), `cplus-core` currently does not enforce a strict sandbox for file-relative paths if they traverse outside the project root. However, because this is a compiler intended to execute locally on the user's machine (and not a remote web service), attempting to include non-C+ files will simply result in a local parse error. This is standard behavior for local compilers. Protective measures are already in place preventing `..` traversals escaping `vendor/` dependency directories (firing `E0859`).

### Memory Safety
The compiler frontend is written in safe Rust. The `unsafe` keyword is only used within `cplus-core`'s parser and semantic analysis to handle the C+ language's own `unsafe { ... }` blocks. The compiler itself does not perform unsafe memory operations, neutralizing entire classes of memory corruption vulnerabilities within the compiler driver itself.
