#!/usr/bin/env python3
"""methods_audit.py — automated widget method-gap matrix for facet.

The problem: a hand-written audit of "what does AppKit expose that facet doesn't"
is lossy — an LLM or a human will miss methods, inconsistently. The bindings are
machine-generated with a uniform `impl X { fn ... }` shape and facet's exposed
surface is a known set of calls, so the diff should be COMPUTED and regenerable.

What it does, per widget:
  1. parse the (bindgen-generated) backend binding into class -> [methods] plus
     upcast edges (`fn as_control(this) -> Control` => Slider inherits Control).
  2. resolve each widget's backing class's FULL config surface = own methods +
     inherited config from its non-generic bases (Control/TextField/Button/...);
     generic bases (View/Responder/Cell/AnyObject) are excluded as noise.
  3. mark a method COVERED if facet's source calls it (`.<method>(` appears),
     else it is a GAP.
  4. emit a per-widget matrix + totals (markdown + JSON).

Re-run after any binding regen. Extend BACKENDS/WIDGETS to add gtk4/uikit/etc.
Graduation path: the most accurate source is cpc-bindgen's clang AST — this
parses the emitted .cplus, which is good enough because that output is uniform.
"""
import json
import re
import sys
from collections import defaultdict, OrderedDict

ROOT = "/Users/adel/Workspace/C+"

# The backend whose surface we diff against. Structured so other backends slot in
# (each is bindgen-generated with the same impl shape); a portable audit would
# run all of them and intersect. AppKit is facet's primary backend.
BACKEND_BINDING = f"{ROOT}/vendor/appkit/src/appkit.cplus"

# facet's exposed surface = everything it CALLS in these files (constructors,
# ak_ handlers, ui wrappers, Handle registry).
FACET_SRC = [
    f"{ROOT}/vendor/facet/src/facet.cplus",
    f"{ROOT}/vendor/facet_appkit/src/ui.cplus",
    f"{ROOT}/vendor/facet_appkit/src/facet_appkit.cplus",
]

# The one curated input: widget -> backing AppKit class. A "widget" is a semantic
# mapping a human owns; everything else is extracted. (For a multi-backend audit
# this becomes widget -> {backend: class}.)
WIDGETS = OrderedDict([
    ("label", "TextField"), ("button", "Button"), ("text_field", "TextField"),
    ("secure_field", "SecureTextField"), ("text_area", "TextView"),
    ("search_field", "SearchField"), ("combo_box", "ComboBox"),
    ("token_field", "TokenField"),
    ("slider", "Slider"), ("stepper", "Stepper"), ("segmented", "SegmentedControl"),
    ("popup", "PopUpButton"), ("date_picker", "DatePicker"),
    ("color_picker", "ColorWell"), ("progress", "ProgressIndicator"),
    ("gauge", "LevelIndicator"),
    ("image", "ImageView"), ("path_control", "PathControl"), ("divider", "Box"),
    ("material", "VisualEffectView"), ("popover", "Popover"),
    ("tree", "OutlineView"), ("list", "TableView"), ("grid", "CollectionView"),
    ("tabs", "TabView"), ("split", "SplitView"), ("scroll", "ScrollView"),
    ("window", "Window"), ("toolbar", "Toolbar"), ("toolbar_item", "ToolbarItem"),
    ("menu", "Menu"), ("menu_item", "MenuItem"), ("alert", "Alert"),
    ("status_item", "StatusItem"), ("save_panel", "SavePanel"),
    ("open_panel", "OpenPanel"),
])

# Bases whose methods are too generic to count as widget-specific config gaps.
GENERIC_BASES = {"View", "Responder", "AnyObject", "Object", "Cell", "ActionCell"}

# Not config/state: constructors, upcasts, raw handles.
BOILERPLATE = re.compile(
    r"^(from_raw|raw|new|new_with_frame|new_with_coder|new_with_frame_.*|init.*|"
    r"alloc|as_[a-z_]+)$"
)


def parse_binding(path):
    """class -> {'methods': [...], 'upcasts': [target_class, ...]}"""
    classes = defaultdict(lambda: {"methods": [], "upcasts": []})
    cur = None
    for line in open(path):
        m = re.match(r"^impl (\w+) \{", line)
        if m:
            cur = m.group(1)
            continue
        if cur is None:
            continue
        if re.match(r"^\}", line):
            cur = None
            continue
        up = re.search(r"\bfn as_(\w+)\(this\) -> (\w+)", line)
        if up:
            classes[cur]["upcasts"].append(up.group(2))
            classes[cur]["methods"].append("as_" + up.group(1))
            continue
        fm = re.search(r"\bfn (\w+)\(", line)
        if fm:
            classes[cur]["methods"].append(fm.group(1))
    return classes


def config_surface(cls, classes, seen=None):
    """Own config methods + inherited config from non-generic bases."""
    if seen is None:
        seen = set()
    if cls in seen or cls not in classes:
        return {}
    seen.add(cls)
    out = {}
    for meth in classes[cls]["methods"]:
        if BOILERPLATE.match(meth):
            continue
        out.setdefault(meth, cls)  # first (most-derived) origin wins
    for base in classes[cls]["upcasts"]:
        if base in GENERIC_BASES:
            continue
        for meth, origin in config_surface(base, classes, seen).items():
            out.setdefault(meth, origin)
    return out


def is_config(meth):
    """A setter, or a getter/action that represents state (not boilerplate)."""
    return not BOILERPLATE.match(meth)


def main():
    classes = parse_binding(BACKEND_BINDING)
    facet_text = "".join(open(p).read() for p in FACET_SRC)

    def covered(meth):
        # facet drives this method if it calls it anywhere in its source.
        return f".{meth}(" in facet_text

    rows = []
    tot = defaultdict(int)
    matrix_md = ["# Methods matrix (GENERATED by tools/methods_audit.py)\n",
                 "Do not hand-edit. Re-run: `python3 tools/methods_audit.py`\n",
                 "Backend: AppKit. COVERED = facet source calls the method; "
                 "GAP = it does not. `set_*` = settable config (what you'd wrap); "
                 "own = declared on the class, inh = inherited from a non-generic "
                 "base (Control/TextField/…); View/Responder noise excluded.\n"]
    for widget, cls in WIDGETS.items():
        surface = config_surface(cls, classes)  # meth -> origin class
        setters = {m: o for m, o in surface.items() if m.startswith("set_")}
        own_set = {m for m, o in setters.items() if o == cls}
        set_gap = sorted(m for m in setters if not covered(m))
        own_set_gap = sorted(m for m in set_gap if m in own_set)  # widget-specific settable gap
        all_gap = sorted(m for m in surface if not covered(m))
        rows.append((widget, cls, len(setters), len(set_gap),
                     len(own_set_gap), len(surface), len(all_gap), own_set_gap))
        tot["setters"] += len(setters)
        tot["set_gap"] += len(set_gap)
        tot["own_set_gap"] += len(own_set_gap)
        tot["all"] += len(surface)
        tot["all_gap"] += len(all_gap)
        matrix_md.append(f"\n## {widget} → NS{cls}  "
                         f"(settable {len(setters)}, set-gap {len(set_gap)}, "
                         f"own-set-gap {len(own_set_gap)})\n")
        if own_set_gap:
            matrix_md.append("OWN SETTABLE GAP (widget-specific): "
                             + ", ".join(own_set_gap) + "\n")

    # console rollup
    hdr = f"{'widget':<15}{'class':<18}{'set':>5}{'setGAP':>7}{'ownGAP':>7}{'allGAP':>7}"
    print(hdr)
    print("-" * len(hdr))
    for w, c, nset, ngap, nown, nall, nallgap, _ in rows:
        print(f"{w:<15}{c:<18}{nset:>5}{ngap:>7}{nown:>7}{nallgap:>7}")
    print("-" * len(hdr))
    print(f"{'TOTAL':<33}{tot['setters']:>5}{tot['set_gap']:>7}"
          f"{tot['own_set_gap']:>7}{tot['all_gap']:>7}")
    print(f"\n{len(WIDGETS)} widgets")
    print(f"  settable config methods : {tot['setters']}  "
          f"(facet leaves {tot['set_gap']} unset)")
    print(f"  WIDGET-SPECIFIC settable gap (own, not inherited) : {tot['own_set_gap']}"
          f"  <- the realistic hand-wrap target")
    print(f"  every config method (incl getters/actions/inherited) : {tot['all']}  "
          f"(gap {tot['all_gap']})")

    with open(f"{ROOT}/plans/methods-matrix.generated.md", "w") as f:
        f.write("".join(matrix_md))
    with open(f"{ROOT}/plans/methods-matrix.generated.json", "w") as f:
        json.dump({w: {"class": c, "settable": nset, "set_gap": ngap,
                       "own_set_gap": nown, "all_config": nall, "all_gap": nallgap,
                       "own_settable_gap_methods": g}
                   for w, c, nset, ngap, nown, nall, nallgap, g in rows}, f, indent=2)


if __name__ == "__main__":
    main()
