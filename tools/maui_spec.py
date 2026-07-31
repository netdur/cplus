#!/usr/bin/env python3
"""maui_spec.py — extract facet's portable surface spec from MAUI's public API.

Strategy: don't hand-curate the portable API (lossy). MAUI is a native-control-
backed cross-platform framework (like facet, unlike Flutter which renders), and
its portable surface is published as a machine-readable Roslyn public-API
manifest — the `netstandard` (platform-agnostic) PublicAPI.Shipped.txt.

v2 (maui-regen Phase 1): the first pass consumed ONE band (settable bindable
properties) and that scope is what made it useless — every gap iris later hit
lived in the other bands. This version extracts SIX:

  writes       `Type.Prop.set -> void` with a public getter (read-write prop)
  reads        `Type.Prop.get -> T` with NO public setter (get-only prop)
  events       `Type.Event -> System.EventHandler[<Args>]`
  methods      `Type.Method(args) -> R` (plain methods; no accessors/ctors)
  recognizers  the gesture-recognizer types as first-class entries, with
               their own four bands (Tap/Drag/Drop/Pointer/Pinch/Pan/Swipe)
  runtime      non-control types facet's runtime owns (Window)

Controls keep their own declared surface; the shared bases (VisualElement /
View / InputView / ItemsView / Element / GestureElement) are reported ONCE as
a shared section rather than duplicated per control — the Phase-2 generator
does the per-control merge.

Refresh the manifest (netstandard = the portable contract):
  BASE=https://raw.githubusercontent.com/dotnet/maui/main/src/Controls/src/Core/PublicAPI/netstandard
  curl -s $BASE/PublicAPI.Shipped.txt   -o plans/facet/spec/maui_PublicAPI.Shipped.txt
  curl -s $BASE/PublicAPI.Unshipped.txt -o plans/facet/spec/maui_PublicAPI.Unshipped.txt

Usage:
  python3 tools/maui_spec.py <manifest.txt> [<manifest2.txt> ...]
Writes plans/facet/spec/maui-spec.json + plans/facet/maui-spec-report.md and
prints the per-type band counts.
"""
import json
import os
import re
import sys
from collections import OrderedDict, defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# The controls facet cares about (MAUI type -> facet widget). A control is a
# semantic mapping a human owns; the band extraction is automated.
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

# Shared bases: surface every control inherits. Reported once.
SHARED_BASES = OrderedDict([
    ("VisualElement", "every element"),
    ("View", "every view"),
    ("InputView", "text inputs"),
    ("ItemsView", "collection views"),
    ("StructuredItemsView", "collection views (layout/header/footer)"),
    ("SelectableItemsView", "collection views (selection)"),
    ("GroupableItemsView", "collection views (grouping)"),
    ("ReorderableItemsView", "collection views (user reorder)"),
    ("GestureElement", "gesture hosts"),
])

# Gesture recognizers: the band the first pass never read — where drag & drop
# and the pointer live.
RECOGNIZERS = OrderedDict([
    ("TapGestureRecognizer", "tap"),
    ("DragGestureRecognizer", "drag source"),
    ("DropGestureRecognizer", "drop target"),
    ("PointerGestureRecognizer", "pointer enter/exit/move"),
    ("PinchGestureRecognizer", "pinch zoom"),
    ("PanGestureRecognizer", "pan"),
    ("SwipeGestureRecognizer", "swipe"),
])

# Non-control types facet's runtime owns.
RUNTIME_TYPES = OrderedDict([
    ("Window", "the app window (X/Y/Width/Height, activation)"),
])

ALL_TYPES = list(CONTROLS) + list(SHARED_BASES) + list(RECOGNIZERS) + list(RUNTIME_TYPES)

# Roslyn PublicAPI lines carry a nullability marker and modifier prefix, e.g.
# "~static readonly Microsoft.Maui.Controls.Slider.MaximumProperty -> ...".
MODS = re.compile(r"^~?(?:(?:static|readonly|const|abstract|virtual|override|"
                  r"sealed|event|new)\s+)*")
NS = r"Microsoft\.Maui\.Controls\."
GETTER = re.compile(rf"^{NS}(\w+)\.(\w+)\.get -> (.+?)[!?]*$")
SETTER = re.compile(rf"^{NS}(\w+)\.(\w+)\.set -> void$")
EVENT = re.compile(rf"^{NS}(\w+)\.(\w+) -> System\.EventHandler(?:<(.+?)[!?]*>)?[!?]*$")
METHOD = re.compile(rf"^{NS}(\w+)\.(\w+)\((.*)\) -> (.+?)[!?]*$")


def parse(paths):
    getters = defaultdict(dict)   # type -> prop -> value type
    setters = defaultdict(set)    # type -> {prop}
    events = defaultdict(dict)    # type -> event -> args type ("" = plain)
    methods = defaultdict(dict)   # type -> name -> (params, ret)
    for path in paths:
        for raw in open(path):
            line = MODS.sub("", raw.strip())
            m = GETTER.match(line)
            if m:
                getters[m.group(1)][m.group(2)] = m.group(3)
                continue
            m = SETTER.match(line)
            if m:
                setters[m.group(1)].add(m.group(2))
                continue
            m = EVENT.match(line)
            if m:
                events[m.group(1)][m.group(2)] = m.group(3) or ""
                continue
            m = METHOD.match(line)
            if m:
                ty, name, params, ret = m.groups()
                # Not methods: constructors, operators, explicit accessors.
                if name == ty or name.startswith(("op_", "get_", "set_", "add_", "remove_")):
                    continue
                methods[ty][name] = (params, ret)
    return getters, setters, events, methods


def band_split(getters, setters, ty):
    """(writes, reads): read-write props vs get-only props for `ty`."""
    writes, reads = {}, {}
    for prop, vt in sorted(getters.get(ty, {}).items()):
        if prop in setters.get(ty, set()):
            writes[prop] = vt
        else:
            reads[prop] = vt
    return writes, reads


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    getters, setters, events, methods = parse(sys.argv[1:])

    out = OrderedDict()
    md = [
        "# MAUI portable-surface spec — GENERATED by tools/maui_spec.py\n\n",
        "Six bands per type (writes / reads / events / methods), for the\n"
        "controls facet declares, the shared bases, the gesture recognizers,\n"
        "and the runtime's Window. This file is the permanent checklist:\n"
        "\"facet is missing X\" starts by looking X up HERE.\n\n",
    ]

    def section(title, table, describe):
        md.append(f"\n## {title}\n\n")
        for maui, facet_name in table.items():
            w, r = band_split(getters, setters, maui)
            ev = events.get(maui, {})
            me = methods.get(maui, {})
            out[maui] = {
                describe: facet_name,
                "writes": w,
                "reads": r,
                "events": {k: v for k, v in sorted(ev.items())},
                "methods": {k: {"params": p, "returns": ret}
                            for k, (p, ret) in sorted(me.items())},
            }
            md.append(f"### {maui} — {facet_name}\n\n")
            md.append(f"- writes ({len(w)}): {', '.join(w) if w else '—'}\n")
            md.append(f"- reads ({len(r)}): {', '.join(r) if r else '—'}\n")
            ev_fmt = [f"{k}({v.rsplit('.', 1)[-1]})" if v else k for k, v in sorted(ev.items())]
            md.append(f"- events ({len(ev)}): {', '.join(ev_fmt) if ev else '—'}\n")
            me_fmt = [f"{k}({p}) -> {ret.rsplit('.', 1)[-1]}" for k, (p, ret) in sorted(me.items())]
            md.append(f"- methods ({len(me)}): {'; '.join(me_fmt) if me else '—'}\n\n")

    section("Controls", CONTROLS, "facet_widget")
    section("Shared bases (inherited by the controls above; listed once)", SHARED_BASES, "applies_to")
    section("Gesture recognizers (the band the first pass never read)", RECOGNIZERS, "role")
    section("Runtime types", RUNTIME_TYPES, "role")

    spec_dir = os.path.join(ROOT, "plans", "facet", "spec")
    os.makedirs(spec_dir, exist_ok=True)
    with open(os.path.join(spec_dir, "maui-spec.json"), "w") as f:
        json.dump(out, f, indent=1)
    with open(os.path.join(ROOT, "plans", "facet", "maui-spec-report.md"), "w") as f:
        f.writelines(md)

    print(f"{'type':<26}{'writes':>7}{'reads':>7}{'events':>7}{'methods':>8}")
    print("-" * 55)
    totals = [0, 0, 0, 0]
    for maui in ALL_TYPES:
        w, r = band_split(getters, setters, maui)
        ev, me = events.get(maui, {}), methods.get(maui, {})
        for i, n in enumerate((len(w), len(r), len(ev), len(me))):
            totals[i] += n
        print(f"{maui:<26}{len(w):>7}{len(r):>7}{len(ev):>7}{len(me):>8}")
    print("-" * 55)
    print(f"{'TOTAL':<26}{totals[0]:>7}{totals[1]:>7}{totals[2]:>7}{totals[3]:>8}")
    print("\nwrote plans/facet/spec/maui-spec.json + plans/facet/maui-spec-report.md")


if __name__ == "__main__":
    main()
