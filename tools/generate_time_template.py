"""Adapt the pinned SDK TimePicker template for an explicitly owned native flyout.

Keep the SDK's display parts and styling. A collapsed native trigger lets
TimePicker update its locale-aware text; the visible trigger is wired by
facet_winui/time_popup. The backend handles Alt+Up/Down in PreviewKeyDown.
"""
import argparse
import json
import re
from pathlib import Path

root = Path(__file__).resolve().parents[1]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--check', action='store_true')
args = parser.parse_args()
source = root / 'playground/qwen_winui_preview/packages/Microsoft.WindowsAppSDK.WinUI.2.3.9/lib/native/Microsoft.UI/Themes/generic.xaml'
output = root / 'vendor/facet_winui/src/time_template.cplus'
xaml = source.read_text(encoding='utf-8-sig')
start = xaml.index('<ControlTemplate TargetType="TimePicker">')
depth = 0
for match in re.finditer(r'</?ControlTemplate\b[^>]*>', xaml[start:]):
    depth += -1 if match.group().startswith('</') else 1
    if depth == 0:
        end = start + match.end()
        break
else:
    raise ValueError('Unclosed TimePicker template')
xaml = xaml[start:end]
xaml = xaml.replace('<ControlTemplate TargetType="TimePicker">',
    '<ControlTemplate xmlns="http://schemas.microsoft.com/winfx/2006/xaml/presentation" '
    'xmlns:x="http://schemas.microsoft.com/winfx/2006/xaml" TargetType="TimePicker">')
assert xaml.count('x:Name="FlyoutButton"') == 1
xaml = xaml.replace('x:Name="FlyoutButton"', 'x:Name="FacetTimeButton"')
xaml = xaml.replace('Target="FlyoutButton.', 'Target="FacetTimeButton.')
position = xaml.rfind('</Grid>')
assert position >= 0
xaml = (xaml[:position] + '<Button x:Name="FlyoutButton" Visibility="Collapsed" IsTabStop="False" />'
        + xaml[position:])
xaml = ' '.join(xaml.split())
result = ('// Copyright (c) Microsoft Corporation. All Rights Reserved.\n'
          '// Adapted from Microsoft Windows App SDK 2.3.9 generic.xaml (MIT).\n'
          '// Retain native formatting parts while exposing an owned flyout trigger.\n'
          'fn source() -> str {return ' + json.dumps(xaml) + ';}\n')
if args.check:
    assert output.read_text(encoding='utf-8') == result, 'Stale time template; run tools/generate_time_template.py'
    print('PASS: pinned time-picker template reproduction')
else:
    output.write_text(result, encoding='utf-8')
    print(f'Generated {output}')
