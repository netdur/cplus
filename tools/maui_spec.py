#!/usr/bin/env python3
"""maui_spec.py — extract facet's portable widget spec from MAUI's public API.

Strategy: don't hand-curate the portable API (lossy). MAUI is a native-control-
backed cross-platform framework (like facet, unlike Flutter which renders), and
its portable surface is published as a machine-readable Roslyn public-API
manifest — the `netstandard` (platform-agnostic) PublicAPI.Shipped.txt. Parse
that -> per-control portable capability set (properties + events). This is the
top-down spec each facet backend then implements; anything a native widget has
beyond it goes to the `.native()` escape.

Refresh the manifest (netstandard = the portable contract):
  BASE=https://raw.githubusercontent.com/dotnet/maui/main/src/Controls/src/Core/PublicAPI/netstandard
  curl -s $BASE/PublicAPI.Shipped.txt   -o maui_PublicAPI.Shipped.txt
  curl -s $BASE/PublicAPI.Unshipped.txt -o maui_PublicAPI.Unshipped.txt

Usage: python3 tools/maui_spec.py <manifest.txt> [<manifest2.txt> ...]
"""
import json
import re
import sys
from collections import defaultdict, OrderedDict

# The controls facet cares about (MAUI type -> facet widget). A control is a
# semantic mapping a human owns; the property/event extraction is automated.
CONTROLS = OrderedDict([
    ("Label", "label"), ("Button", "button"), ("Entry", "text_field"),
    ("Editor", "text_area"), ("SearchBar", "search_field"), ("Picker", "popup"),
    ("Slider", "slider"), ("Stepper", "stepper"), ("Switch", "toggle"),
    ("DatePicker", "date_picker"), ("TimePicker", "time_picker"),
    ("ProgressBar", "progress"), ("ActivityIndicator", "spinner"),
    ("Image", "image"), ("CollectionView", "grid/list"), ("ListView", "list"),
    ("TableView", "table"), ("TabbedPage", "tabs"), ("CheckBox", "checkbox"),
    ("RadioButton", "radio"), ("BoxView", "box"), ("Border", "bordered"),
])

# Roslyn PublicAPI lines carry a nullability/modifier prefix, e.g.
# "~static readonly Microsoft.Maui.Controls.Slider.MaximumProperty -> ...".
MODS = re.compile(r"^~?(?:(?:static|readonly|const|abstract|virtual|override|"
                  r"sealed|event|new)\s+)*")
PROP = re.compile(r"^Microsoft\.Maui\.Controls\.(\w+)\.(\w+)Property -> "
                  r"Microsoft\.Maui\.Controls\.BindableProperty")
EVENT = re.compile(r"^Microsoft\.Maui\.Controls\.(\w+)\.(\w+) -> "
                   r"System\.EventHandler")


def parse(paths):
    props = defaultdict(set)
    events = defaultdict(set)
    for path in paths:
        for line in open(path):
            line = MODS.sub("", line.strip())
            m = PROP.match(line)
            if m:
                props[m.group(1)].add(m.group(2))
                continue
            m = EVENT.match(line)
            if m:
                events[m.group(1)].add(m.group(2))
    return props, events


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    props, events = parse(sys.argv[1:])

    out = {}
    print(f"{'facet widget':<16}{'MAUI control':<18}{'props':>6}{'events':>7}")
    print("-" * 47)
    total_p = total_e = 0
    md = ["# Portable widget spec (GENERATED from MAUI netstandard public API)\n",
          "by tools/maui_spec.py. These are the portable capability slots each "
          "facet backend implements; native-only methods go to `.native()`.\n"]
    for maui, widget in CONTROLS.items():
        p = sorted(props.get(maui, []))
        e = sorted(events.get(maui, []))
        total_p += len(p)
        total_e += len(e)
        out[widget] = {"maui_control": maui, "properties": p, "events": e}
        print(f"{widget:<16}{maui:<18}{len(p):>6}{len(e):>7}")
        md.append(f"\n## {widget}  (MAUI {maui})\n")
        md.append(f"- properties ({len(p)}): {', '.join(p) if p else '—'}\n")
        md.append(f"- events ({len(e)}): {', '.join(e) if e else '—'}\n")
    print("-" * 47)
    print(f"{'TOTAL':<34}{total_p:>6}{total_e:>7}")
    print(f"\n{len(CONTROLS)} controls · {total_p} portable properties · "
          f"{total_e} portable events  (the portable slot budget)")

    root = "/Users/adel/Workspace/C+/plans"
    open(f"{root}/portable-spec.generated.md", "w").write("".join(md))
    json.dump(out, open(f"{root}/portable-spec.generated.json", "w"), indent=2)


if __name__ == "__main__":
    main()
