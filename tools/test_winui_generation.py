"""Integration checks using the pinned SDK WinMD files (no UI required)."""
import argparse
import collections
import hashlib
import json
import re
from pathlib import Path
import subprocess
import uuid

repo = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser()
parser.add_argument('--packages', type=Path, default=repo / 'playground/qwen_winui_preview/packages')
parser.add_argument('--windows-metadata', type=Path, default=Path('C:/Program Files (x86)/Windows Kits/10/UnionMetadata/10.0.26100.0/Windows.winmd'))
args = parser.parse_args()
main = args.packages / 'Microsoft.WindowsAppSDK.WinUI.2.3.9/metadata/Microsoft.UI.Xaml.winmd'
refs = [args.windows_metadata]
refs += [args.packages / 'Microsoft.WindowsAppSDK.InteractiveExperiences.2.1.9/metadata/10.0.17763.0' / (name + '.winmd') for name in ('Microsoft.UI', 'Microsoft.Foundation', 'Microsoft.Graphics')]
refs += [args.packages / 'Microsoft.Web.WebView2.1.0.3719.77/lib/Microsoft.Web.WebView2.Core.winmd']
command = [str(repo / 'target/release/cpc-bindgen.exe'), '--winmd', str(main)]
for ref in refs:
    command += ['--reference', str(ref)]
selected = ['--selection', str(repo / 'vendor/winui/selection.json'), '--package', 'winui']
test_root = repo / 'target/winui-generation-tests' / str(uuid.uuid4())
outputs = [test_root / name for name in ('first', 'second')]
for output in outputs:
    subprocess.run(command + selected + ['--out', str(output)], check=True)
for name in ('src/winui.cplus', 'Cplus.toml', 'MANIFEST.json'):
    values = [(output / name).read_bytes() for output in outputs]
    assert values[0] == values[1], f'Non-deterministic generation: {name}'
    assert values[0] == (repo / 'vendor/winui' / name).read_bytes(), f'Stale checked-in output: {name}'
manifest = json.loads((outputs[0] / 'MANIFEST.json').read_text())
types = {t['name']: t for t in manifest['types']}
fixture = json.loads((repo / 'cpc-bindgen/tests/winui-abi.json').read_text())
checked = 0
for name, expected in fixture['interfaces'].items():
    actual = types[name]
    assert actual['iid'].lower() == expected['iid'].lower(), f'{name}: IID mismatch'
    methods = {m['name']: m for m in actual['methods']}
    for method, details in expected['methods'].items():
        assert methods[method]['slot'] == details['slot'], f'{name}.{method}: wrong slot'
        checked += 1
for source, path in zip(manifest['sources'], [main] + refs):
    assert source['sha1'] == hashlib.sha1(path.read_bytes()).hexdigest()
assert types['TypeName']['skipped']
assert types['XmlnsDefinition']['skipped']
assert types['RoutedEventHandler']['callback'] == 'emitted'
# Independently derive IReference<PointerEventHandler> from the WinRT
# parameterized-IID algorithm and the SDK's delegate IID. AddHandler needs
# this inspectable box, not the delegate's IUnknown pointer.
signature = ('pinterface({61c17706-2d65-11e0-9ae8-d48564015472};'
             'delegate({a48a71e1-8bb4-5597-9e31-903a3f6a04fb}))')
boxed_iid = uuid.uuid5(uuid.UUID('11f47ad5-7b73-42c0-abae-878b1e16adee'), signature)
generated = (outputs[0] / 'src/winui.cplus').read_text()
delegate = generated.split('impl PointerEventHandler {', 1)[1].split('\n}', 1)[0]
boxed = re.search(r'fn boxed\(this\).*?rt::Guid \{(.*?)\}', delegate)[1]
fields = list(map(int, re.findall(r'\b(\d+)(?:u32| as u16| as u8)', boxed)))
actual_iid = uuid.UUID(fields=(*fields[:5], int.from_bytes(bytes(fields[5:]), 'big')))
assert actual_iid == boxed_iid, 'Delegate box has the wrong IReference IID'
for t in types.values():
    if t.get('callback') == 'emitted':
        body = generated.split('impl ' + t['name'] + ' {', 1)[1].split('\n}', 1)[0]
        assert 'fn boxed(this)' in body, f"{t['name']}: missing delegate boxing"
font_constructor = types['IFontFamilyFactory']['methods'][0]
assert font_constructor['name'] == 'CreateInstanceWithName'
assert font_constructor['status'] == 'emitted', 'Parameterized composition constructor lost'
invalid = test_root / 'invalid'
failed = subprocess.run(command + ['--include', 'No.Such.Type', '--out', str(invalid)], capture_output=True, text=True)
assert failed.returncode != 0 and 'did not match' in failed.stderr
assert not invalid.exists(), 'Bad input wrote partial output'
counts = collections.Counter(m['status'] for t in manifest['types'] for m in t['methods'])
print(f'PASS: deterministic output, provenance, invalid selection, {checked} independent SDK slots; {dict(counts)}')
print(f'Artifacts: {test_root}')
