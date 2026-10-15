#!/usr/bin/env python3
"""Build a disposable consuming app and verify native paths/identity.

Usage: python3 vendor/appenv/tests/run_native.py host|ios|android
Requires Xcode for iOS, a booted simulator, or an ARM64 Android emulator/device
and Android SDK/NDK/JDK for Android. Test apps are uninstalled in finally blocks.
"""
import argparse
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import time

TESTS = Path(__file__).resolve().parent
ROOT = TESTS.parents[2]
CPC = Path(os.environ.get("CPC", ROOT / "target/debug/cpc"))
APP_ID = "dev.cplus.appenvprobe"


def run(*args, cwd=None, check=True):
    result = subprocess.run([str(arg) for arg in args], cwd=cwd,
                            text=True, capture_output=True)
    if check and result.returncode:
        raise RuntimeError(f"Command failed ({result.returncode}): {args}\n{result.stdout}{result.stderr}")
    return result


def link_args(probe, target):
    return run(CPC, "build", "--target", target, "--print-link-args", cwd=probe).stdout.splitlines()


def info_plist(destination, ios=False):
    values = {"CFBundleIdentifier": APP_ID, "CFBundleName": "AppEnvProbe",
              "CFBundleDisplayName": "AppEnv 🧭", "CFBundleExecutable": "AppEnvProbe",
              "CFBundlePackageType": "APPL", "CFBundleShortVersionString": "1.2.3",
              "CFBundleVersion": "42"}
    if ios:
        values.update(LSRequiresIPhoneOS=True, MinimumOSVersion="14.0",
                      UIDeviceFamily=[1, 2], UILaunchScreen={})
    destination.write_bytes(plistlib.dumps(values))


def write_assets(root):
    root.mkdir(parents=True, exist_ok=True)
    (root / "appenv-resource.txt").write_text("resource fixture", encoding="utf-8")
    (root / "nested").mkdir()
    (root / "nested/café 🧭.txt").write_text("café 🧭", encoding="utf-8")
    (root / "empty.txt").write_bytes(b"")
    (root / "binary.txt").write_bytes(bytes(i % 256 for i in range(20000)))
    (root / "invalid.txt").write_bytes(b"\xff\xfe")
    (root / "embedded.txt").write_bytes(b"a\0b")


def check_private_api(probe):
    """Consumers may use the facade but cannot call implementation helpers."""
    entry = probe / "main.cplus"
    original = entry.read_text()
    cases = [
        ('appenv/appenv', '_is_valid_namespace("notes")'),
        ('appenv/appenv', '_is_valid_asset_name("asset.txt")'),
        ('appenv/appenv_backend', '_read_path(0 as u32)'),
        ('appenv/appenv_backend', '_CURRENT'),
        ('appenv/asset_reader', '_appenv_read_asset_v1'),
        ('appenv/asset_reader', '_appenv_asset_path_v1'),
    ]
    try:
        for module, expression in cases:
            entry.write_text(f'import "{module}" as api;\n'
                             f'fn main() -> i32 {{ let _value = api::{expression}; return 0; }}\n')
            result = run(CPC, "check", cwd=probe, check=False)
            assert result.returncode != 0 and "E0403" in result.stderr, result.stdout + result.stderr
    finally:
        entry.write_text(original)


def host(probe, out, asan=False):
    check_private_api(probe)
    run(CPC, "build", *(["--asan"] if asan else []), cwd=probe)
    contents = out / "AppEnvProbe.app/Contents"
    (contents / "MacOS").mkdir(parents=True)
    (contents / "Resources").mkdir()
    info_plist(contents / "Info.plist")
    write_assets(contents / "Resources")
    executable = contents / "MacOS/AppEnvProbe"
    shutil.copy2(probe / "target/debug/appenv_probe", executable)
    # Exercise getcwd's growth and Unicode handling, not just a short repo path.
    deep = out
    for _ in range(3):
        deep = deep / ("café-" + "x" * 90)
    deep.mkdir(parents=True)
    result = run(executable, cwd=deep)
    assert "APPENV PASS" in result.stdout, result.stdout + result.stderr


def ios(probe, out):
    import json
    devices = json.loads(run("xcrun", "simctl", "list", "devices", "booted", "-j").stdout)
    available = [d["udid"] for group in devices["devices"].values() for d in group
                 if d["state"] == "Booted"]
    if not available:
        raise RuntimeError("Boot an iOS simulator before running the iOS probe")
    device = os.environ.get("SIMULATOR_UDID", available[0])
    target = "ios-arm64-simulator"
    run(CPC, "build", "--target", target, cwd=probe)
    app = out / "AppEnvProbe.app"
    app.mkdir()
    info_plist(app / "Info.plist", ios=True)
    write_assets(app)
    artifacts = probe / "target" / target / "debug"
    run("xcrun", "--sdk", "iphonesimulator", "clang", "-arch", "arm64",
        "-mios-simulator-version-min=14.0", "-I", artifacts, TESTS / "main.m",
        artifacts / "libappenv_probe.a", *link_args(probe, target), "-o", app / "AppEnvProbe")
    run("codesign", "--force", "--sign", "-", app)
    # Don't overwrite a pre-existing installation with the fixture's ID.
    existing = run("xcrun", "simctl", "get_app_container", device, APP_ID, "app", check=False)
    if existing.returncode == 0:
        raise RuntimeError(f"Remove the existing {APP_ID} test app before running")
    try:
        run("xcrun", "simctl", "install", device, app)
        # A short-lived runner can be reported as a launch failure. Its own
        # container report, rather than simctl's launch status, is the verdict.
        run("xcrun", "simctl", "launch", device, APP_ID, check=False)
        container = Path(run("xcrun", "simctl", "get_app_container", device, APP_ID, "data").stdout.strip())
        report = container / "appenv-result.txt"
        for _ in range(100):
            if report.exists():
                break
            time.sleep(0.1)
        assert report.read_text() == "APPENV PASS\n"
    finally:
        run("xcrun", "simctl", "uninstall", device, APP_ID, check=False)


def android(probe, out):
    sdk = Path(os.environ.get("ANDROID_SDK_ROOT", Path.home() / "Library/Android/sdk"))
    ndk = Path(os.environ.get("ANDROID_NDK_HOME", sdk / "ndk/28.2.13676358"))
    stable_tools = [p for p in (sdk / "build-tools").iterdir()
                    if all(part.isdigit() for part in p.name.split("."))]
    build_tools = max(stable_tools, key=lambda p: tuple(map(int, p.name.split("."))))
    platforms = [p for p in (sdk / "platforms").glob("android-*/android.jar")
                 if all(part.isdigit() for part in p.parent.name.removeprefix("android-").split("."))]
    android_jar = max(platforms, key=lambda p: tuple(map(int, p.parent.name.removeprefix("android-").split("."))))
    adb = sdk / "platform-tools/adb"
    device = os.environ.get("ANDROID_SERIAL", "emulator-5554")
    adb_args = [adb, "-s", device]
    run(*adb_args, "get-state")
    # pm path returns 1 (with no output) when the package isn't installed.
    existing = run(*adb_args, "shell", "pm", "path", APP_ID, check=False).stdout.strip()
    if existing:
        raise RuntimeError(f"Remove the existing {APP_ID} test app before running")
    target = "android-arm64"
    run(CPC, "build", "--target", target, cwd=probe)
    artifacts = probe / "target" / target / "debug"
    library = out / "lib/arm64-v8a/libappenvprobe.so"
    library.parent.mkdir(parents=True)
    clang = ndk / "toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android24-clang"
    run(clang, "-shared", "-fPIC", "-Wl,--no-undefined", "-I", artifacts,
        TESTS / "android.c", artifacts / "libappenv_probe.a", *link_args(probe, target), "-o", library)
    classes = out / "classes"
    classes.mkdir()
    run("javac", "-source", "8", "-target", "8", "-cp", android_jar,
        "-d", classes, TESTS / "ProbeActivity.java")
    run(build_tools / "d8", "--lib", android_jar, "--min-api", "24", "--output", out,
        *classes.rglob("*.class"))
    manifest = out / "AndroidManifest.xml"
    manifest.write_text('''<manifest xmlns:android="http://schemas.android.com/apk/res/android"
 package="dev.cplus.appenvprobe" android:versionCode="42" android:versionCodeMajor="1" android:versionName="1.2.3">
 <uses-sdk android:minSdkVersion="24" android:targetSdkVersion="35"/>
 <application android:label="AppEnv 🧭" android:debuggable="true" android:hasCode="true">
 <activity android:name=".ProbeActivity" android:exported="true">
 <intent-filter><action android:name="android.intent.action.MAIN"/><category android:name="android.intent.category.LAUNCHER"/></intent-filter>
 </activity></application></manifest>''')
    base = out / "base.apk"
    assets = out / "assets"
    write_assets(assets)
    run(build_tools / "aapt2", "link", "--manifest", manifest, "-I", android_jar, "-A", assets, "-o", base)
    import zipfile
    with zipfile.ZipFile(base) as archive:
        assert archive.getinfo("assets/binary.txt").compress_type == zipfile.ZIP_DEFLATED
    run("zip", "-q", base, "classes.dex", "lib/arm64-v8a/libappenvprobe.so", cwd=out)
    apk = out / "probe.apk"
    run(build_tools / "zipalign", "-f", "4", base, apk)
    keystore = out / "debug.keystore"
    run("keytool", "-genkeypair", "-keystore", keystore, "-storepass", "android",
        "-keypass", "android", "-alias", "probe", "-dname", "CN=AppEnv Probe",
        "-keyalg", "RSA", "-validity", "1", "-noprompt")
    run(build_tools / "apksigner", "sign", "--ks", keystore, "--ks-pass", "pass:android", apk)
    try:
        run(*adb_args, "install", "-t", apk)
        run(*adb_args, "shell", "am", "start", "-n", APP_ID + "/.ProbeActivity")
        report = ""
        for _ in range(100):
            report = run(*adb_args, "shell", "run-as", APP_ID, "cat", "appenv-result.txt", check=False).stdout
            if report == "APPENV PASS\n":
                break
            time.sleep(0.1)
        assert report == "APPENV PASS\n", report
    finally:
        run(*adb_args, "uninstall", APP_ID, check=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=["host", "ios", "android"])
    parser.add_argument("--asan", action="store_true", help="enable AddressSanitizer for the host fixture")
    args = parser.parse_args()
    if args.asan and args.platform != "host":
        parser.error("--asan is supported by the host fixture")
    with tempfile.TemporaryDirectory(prefix="cplus-appenv-") as directory:
        out = Path(directory)
        probe = out / "probe"
        probe.mkdir()
        for source in [*TESTS.glob("*.cplus"), TESTS / "Cplus.toml"]:
            shutil.copy2(source, probe / source.name)
        (probe / "vendor").symlink_to(ROOT / "vendor", target_is_directory=True)
        if args.platform == "host":
            host(probe, out, asan=args.asan)
        else:
            globals()[args.platform](probe, out)
    print(f"appenv {args.platform}: PASS")


if __name__ == "__main__":
    main()
