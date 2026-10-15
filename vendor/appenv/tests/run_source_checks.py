#!/usr/bin/env python3
"""Check desktop sources on the current host; this is not native platform validation."""
import os
import json
from pathlib import Path
import re
import subprocess
import tempfile

TESTS = Path(__file__).resolve().parent
ROOT = TESTS.parents[2]
SOURCE = TESTS.parent / "src"
CPC = Path(os.environ.get("CPC", ROOT / "target/debug/cpc"))


def run(*args, cwd):
    result = subprocess.run([str(a) for a in args], cwd=cwd, text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    print(result.stdout.strip())


def check_backend(platform):
    with tempfile.TemporaryDirectory(prefix="appenv-source-") as directory:
        probe = Path(directory)
        source = probe / "src"
        source.mkdir()
        (probe / "vendor").symlink_to(ROOT / "vendor", target_is_directory=True)
        (probe / "Cplus.toml").write_text(
            '[package]\nname="backend_probe"\nversion="0.0.1"\nedition="2026"\n'
            '[dependencies]\nstdlib="*"\n', encoding="utf-8")
        backend = (SOURCE / f"appenv_backend_{platform}.cplus").read_text()
        if platform == "linux":
            gio = Path("/opt/homebrew/opt/glib/lib/libgio-2.0.0.dylib")
            if gio.exists():
                # Exercise the unchanged GIO reader against the real library on
                # macOS. This proves that API contract, not a native Linux run.
                backend += gio_test(str(gio))
            (source / "backend_probe.cplus").write_text(backend, encoding="utf-8")
            run(CPC, "test", "--filter", "appenv_linux", cwd=probe)
        else:
            (source / "appenv_backend.cplus").write_text(backend, encoding="utf-8")
            for name in ["appenv.cplus", "asset_reader_windows.cplus"]:
                destination = "asset_reader.cplus" if name.startswith("asset_reader") else name
                (source / destination).write_text((SOURCE / name).read_text(), encoding="utf-8")
            entry = source / "backend_probe.cplus"
            entry.write_text('import "./appenv" as _;\nimport "./appenv_backend" as _;\n'
                             'import "./asset_reader" as _;\n', encoding="utf-8")
            run(CPC, "check", cwd=probe)
            # The macOS codegen preamble cannot declare Win32 CloseHandle.
            # Type-check the UTF-16 asset reader, then object-build the backend.
            (source / "appenv.cplus").unlink()
            (source / "asset_reader.cplus").unlink()
            entry.unlink()
            run(CPC, "build", cwd=probe)
        print(f"appenv {platform} source: PASS")


def gio_test(library):
    literal = json.dumps(library + "\0")
    # C+ spells a NUL escape as \0 rather than JSON's \u0000.
    literal = literal.replace("\\u0000", "\\0")
    return '''
fn _fixture_symbol(handle: *u8, name: str) -> *u8 {
    let symbol: *u8 = { dlsym(handle, #str_ptr(name)) };
    assert symbol.is_not_null();
    return symbol;
}

#[test]
fn _appenv_linux_gio_identity_uses_owned_values() {
    let library: *u8 = { dlopen(#str_ptr(LIBRARY), 2) };
    assert library.is_not_null();
    let create: fn(*u8, u32) -> *u8 = { _fixture_symbol(library, "g_application_new\\0") as fn(*u8, u32) -> *u8 };
    let set_default: fn(*u8) = { _fixture_symbol(library, "g_application_set_default\\0") as fn(*u8) };
    let set_name: fn(*u8) = { _fixture_symbol(library, "g_set_application_name\\0") as fn(*u8) };
    let unref: fn(*u8) = { _fixture_symbol(library, "g_object_unref\\0") as fn(*u8) };
    let app: *u8 = create(#str_ptr("dev.cplus.appenvprobe\\0"), 32 as u32);
    assert app.is_not_null();
    set_default(app);
    set_name(#str_ptr("AppEnv 🧭\\0"));
    let version_symbol: *u8 = { dlsym(library, #str_ptr("g_application_set_version\\0")) };
    if version_symbol.is_not_null() {
        let set_version: fn(*u8, *u8) = { version_symbol as fn(*u8, *u8) };
        set_version(app, #str_ptr("1.2.3\\0"));
    }
    let id: option::Option[text::Text] = _gio_info(library, _ID);
    let name: option::Option[text::Text] = _gio_info(library, _NAME);
    let version: option::Option[text::Text] = _gio_info(library, _VERSION);
    match _gio_info(library, _BUILD) {
        option::Option::None => {}
        option::Option::Some(value) => { assert false; }
    }
    set_default(0 as *u8);
    match _gio_info(library, _ID) {
        option::Option::None => {}
        option::Option::Some(value) => { assert false; }
    }
    unref(app);
    let _closed = { dlclose(library) };
    match id {
        option::Option::Some(value) => { assert value.view() == "dev.cplus.appenvprobe"; }
        option::Option::None => { assert false; }
    }
    match name {
        option::Option::Some(value) => { assert value.view() == "AppEnv 🧭"; }
        option::Option::None => { assert false; }
    }
    if version_symbol.is_not_null() {
        match version {
            option::Option::Some(value) => { assert value.view() == "1.2.3"; }
            option::Option::None => { assert false; }
        }
    }
}
'''.replace("LIBRARY", literal)


def check_docs():
    public = set(re.findall(r"^fn ([a-z][a-z_]+)\(", (SOURCE / "appenv.cplus").read_text(), re.M))
    reference = set(re.findall(r"^### `([a-z_]+)`$", (TESTS.parent / "docs/ref.md").read_text(), re.M))
    assert public == reference, (public - reference, reference - public)
    assert len(public) == 24, public
    print(f"appenv reference: all {len(public)} public functions documented")
    with tempfile.TemporaryDirectory(prefix="appenv-docs-") as directory:
        probe = Path(directory)
        (probe / "vendor").symlink_to(ROOT / "vendor", target_is_directory=True)
        (probe / "Cplus.toml").write_text((TESTS / "Cplus.toml").read_text(), encoding="utf-8")
        for document in [TESTS.parent / "README.md", TESTS.parent / "docs/tutorial.md"]:
            for index, snippet in enumerate(re.findall(r"```cplus\n(.*?)```", document.read_text(), re.S)):
                (probe / "main.cplus").write_text(snippet + "\nfn main() -> i32 { return 0; }\n", encoding="utf-8")
                run(CPC, "check", cwd=probe)
                print(f"appenv {document.name} snippet {index + 1}: PASS")


if __name__ == "__main__":
    check_backend("linux")
    check_backend("windows")
    check_docs()
