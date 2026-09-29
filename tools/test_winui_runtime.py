"""Check self-contained runtime staging, including WebView2 and cache repair."""
from pathlib import Path
import collections
import hashlib
import subprocess
import sys
import uuid
import xml.etree.ElementTree as ET

repo = Path(__file__).resolve().parents[1]
packages = repo / 'playground/qwen_winui_preview/packages'
out = repo / 'target/winui-runtime-tests' / str(uuid.uuid4())
command = [sys.executable, str(repo / 'examples/winui_standalone/prepare_runtime.py'),
           '--packages', str(packages), '--out', str(out)]
subprocess.run(command, check=True)
manifest = ET.parse(out / 'app.manifest').getroot()
files = [n.attrib['name'] for n in manifest.findall('.//{*}file')]
assert len(files) == len(set(files)), 'Duplicate DLL registrations'
assert all((out / name).is_file() for name in files), 'Missing registered DLL'
classes = collections.Counter(n.attrib['name'] for n in manifest.findall('.//{*}activatableClass'))
assert all(n == 1 for n in classes.values()), 'Duplicate activation classes'
assert classes['Microsoft.Web.WebView2.Core.CoreWebView2Environment'] == 1
core = out / 'Microsoft.Web.WebView2.Core.dll'
noise = out / 'Microsoft.UI.Xaml/Assets/NoiseAsset_256x256_PNG.png'
noise_bytes = noise.read_bytes()
assert noise_bytes.startswith(b'\x89PNG\r\n\x1a\n'), 'Missing acrylic texture'
sdk_core = packages / 'Microsoft.Web.WebView2.1.0.3719.77/runtimes/win-x64/native_uap' / core.name
digest = hashlib.sha256(sdk_core.read_bytes()).digest()
assert hashlib.sha256(core.read_bytes()).digest() == digest
cached = subprocess.run(command, check=True, capture_output=True, text=True)
assert 'already staged' in cached.stdout
core.unlink()  # Only the exact DLL in this test's unique output directory.
noise.unlink()
subprocess.run(command, check=True)
assert hashlib.sha256(core.read_bytes()).digest() == digest
assert noise.read_bytes() == noise_bytes
print('PASS: runtime DLLs, unique activation entries, WebView2, acrylic texture and cache repair')
