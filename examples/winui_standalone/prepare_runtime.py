"""Stage the pinned SDK runtime and derive activation registrations from its MSIX.

No C++ project, generated C++ headers, package installation, or registry writes.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import xml.etree.ElementTree as ET
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument('--packages', type=Path, required=True)
parser.add_argument('--out', type=Path, required=True)
args = parser.parse_args()
package = args.packages / 'Microsoft.WindowsAppSDK.Runtime.2.5.1'
msix = package / 'tools/MSIX/win10-x64/Microsoft.WindowsAppRuntime.2.msix'
destination = args.out.resolve()
destination.mkdir(parents=True, exist_ok=True)
digest = hashlib.sha256(msix.read_bytes()).hexdigest()
stamp = destination / 'runtime-source.json'
source = {'package': package.name, 'sha256': digest, 'deployment_revision': 2}
if stamp.exists() and json.loads(stamp.read_text()) == source:
    if all((destination / name).exists() for name in ('app.manifest', 'Microsoft.UI.Xaml.dll', 'resources.pri')):
        print(f'Runtime already staged: {destination}')
        raise SystemExit(0)

asm = 'urn:schemas-microsoft-com:asm.v1'
asm3 = 'urn:schemas-microsoft-com:asm.v3'
winrt = 'urn:schemas-microsoft-com:winrt.v1'
ET.register_namespace('', asm)
ET.register_namespace('asmv3', asm3)
ET.register_namespace('winrtv1', winrt)
manifest = ET.Element(f'{{{asm}}}assembly', {'manifestVersion': '1.0'})
ET.SubElement(manifest, f'{{{asm}}}assemblyIdentity', {'version': '1.0.0.0', 'name': 'Cplus.WinUIStandalone'})
application = ET.SubElement(manifest, f'{{{asm3}}}application')
settings = ET.SubElement(application, f'{{{asm3}}}windowsSettings')
ET.SubElement(settings, '{http://schemas.microsoft.com/SMI/2005/WindowsSettings}dpiAware').text = 'true/pm'
ET.SubElement(settings, '{http://schemas.microsoft.com/SMI/2016/WindowsSettings}dpiAwareness').text = 'PerMonitorV2'
with zipfile.ZipFile(msix) as archive:
    for entry in archive.infolist():
        if entry.is_dir() or Path(entry.filename).suffix.lower() not in ('.dll', '.pri', '.mui', '.exe'):
            continue
        target = (destination / entry.filename).resolve()
        if not target.is_relative_to(destination):
            raise ValueError(f'Archive path escapes destination: {entry.filename}')
        target.parent.mkdir(parents=True, exist_ok=True)
        with archive.open(entry) as incoming, target.open('wb') as outgoing:
            shutil.copyfileobj(incoming, outgoing)
    metadata = ET.fromstring(archive.read('AppxManifest.xml'))
    for server in metadata.findall('.//{*}InProcessServer'):
        dll = server.find('{*}Path').text
        node = ET.SubElement(manifest, f'{{{asm3}}}file', {'name': dll})
        for entry in server.findall('{*}ActivatableClass'):
            ET.SubElement(node, f'{{{winrt}}}activatableClass', {
                'name': entry.attrib['ActivatableClassId'],
                'threadingModel': entry.attrib['ThreadingModel'],
            })
# This code-built sample has no application PRI. Use WinUI's theme resources.
shutil.copyfile(destination / 'Microsoft.UI.Xaml.Controls.pri', destination / 'resources.pri')
for name in ('license.txt', 'NOTICE.txt'):
    if (package / name).exists():
        shutil.copyfile(package / name, destination / name)
ET.indent(manifest)
ET.ElementTree(manifest).write(destination / 'app.manifest', encoding='utf-8', xml_declaration=True)
stamp.write_text(json.dumps(source, indent=2) + '\n')
print(f'Staged SDK runtime and activation manifest: {destination}')
