#!/usr/bin/env python3
"""maui_map.py — the curated MAP: MAUI surface -> facet contract names.

This file IS the curation (maui-regen Phase 1). The Phase-2 generator imports
it; running it emits the human review document plans/facet/maui-map-draft.md.

Every row of the MAUI spec (plans/facet/spec/maui-spec.json, six bands) lands
in exactly one status:

  ADOPT       generated into the contract under the given facet name
  KEEP-FACET  facet already has this, under a name that stays — the row
              records the equivalence so the checklist shows it COVERED
  DROP        deliberately absent, with the reason (MVVM artifact, layout
              belongs to flex_layout, styling belongs to the theme, ...)
  FUTURE      real surface, not this pass (unmappable type, new control
              band, or low demand) — visible, never silent

Default naming: PascalCase -> snake_case; writes gain `set_`; reads keep the
bare name; events gain `on_` (discrete) or `observe_` (continuous). Anything
the defaults get wrong is overridden here, once.
"""
import json
import os
import re
import sys
from collections import OrderedDict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC = os.path.join(ROOT, "plans", "facet", "spec", "maui-spec.json")

# ---- value-type map: MAUI -> facet. A type not listed here sends the row to
# FUTURE (visible), never to a silent drop.
TYMAP = {
    "string": "str",
    "double": "f64",
    "float": "f64",
    "bool": "bool",
    "int": "i64",
    "Microsoft.Maui.Graphics.Color": "Color",
    "Microsoft.Maui.Controls.ImageSource": "str",   # facet image() takes a path
}

# ---- global DROP rules, by property-name pattern. The reasons are the point.
DROP_PATTERNS = [
    (re.compile(r"Command(Parameter)?$"), "MVVM artifact — facet callbacks are fn-ptrs with ctx"),
    (re.compile(r"^(Margin|Padding|WidthRequest|HeightRequest|MinimumWidthRequest|"
                r"MinimumHeightRequest|MaximumWidthRequest|MaximumHeightRequest|"
                r"HorizontalOptions|VerticalOptions)$"),
     "layout belongs to flex_layout (facet Node modifiers)"),
    (re.compile(r"^(Style|StyleClass|Resources|Triggers|Behaviors|BindingContext|"
                r"ControlTemplate)$"),
     "binding/styling machinery — facet's theme system owns styling"),
    (re.compile(r"^(AnchorX|AnchorY|Rotation|RotationX|RotationY|Scale|ScaleX|ScaleY|"
                r"TranslationX|TranslationY)$"),
     "transform band — no facet transform story yet; FUTURE as a band, not per-prop"),
    (re.compile(r"^AutomationId$"), "agent surface has its own deterministic ids"),
    (re.compile(r"^(ClassId|StyleId)$"), "binding-era identity — facet keys are the identity"),
]

# ---- per-row curation overlays: (Type, Member) -> (status, facet_name, note).
# KEEP-FACET rows are the 2026-07-31 verbs and the wired v1 slots; their names
# stay. DROP/FUTURE overlays where the default rules are wrong.
OVERLAY = {
    # -- the five audit gaps: already portable, names stay --
    ("VisualElement", "Width"):        ("KEEP-FACET", "Handle.size().width", "shipped 0b813b7"),
    ("VisualElement", "Height"):       ("KEEP-FACET", "Handle.size().height", "shipped 0b813b7"),
    ("VisualElement", "SizeChanged"):  ("KEEP-FACET", "facet::observe_size -> Cancellable", "shipped 0b813b7"),
    ("Window", "X"):                   ("KEEP-FACET", "runtime::window_frame().x", "shipped 71eb2e1"),
    ("Window", "Y"):                   ("KEEP-FACET", "runtime::window_frame().y", "shipped 71eb2e1"),
    ("Window", "Width"):               ("KEEP-FACET", "runtime::window_frame().width", "shipped 71eb2e1"),
    ("Window", "Height"):              ("KEEP-FACET", "runtime::window_frame().height", "shipped 71eb2e1"),
    ("PointerGestureRecognizer", "PointerMoved"): ("KEEP-FACET", "Handle.pointer_position()", "the read half shipped 72d9a23; a moved-event observe_* is FUTURE"),
    # -- existing facet verbs the defaults would rename --
    ("Switch", "IsToggled"):           ("KEEP-FACET", "Handle.set_on / value()", "facet's toggle verb"),
    ("VisualElement", "IsVisible"):    ("KEEP-FACET", "Handle.set_hidden(!v)", "one verb, inverted; v1's set_is_visible dies with the regen"),
    ("VisualElement", "IsEnabled"):    ("KEEP-FACET", "Handle.set_is_enabled", "wired in v1, stays"),
    ("VisualElement", "Opacity"):      ("KEEP-FACET", "Handle.set_opacity", "wired in v1, stays"),
    ("VisualElement", "IsFocused"):    ("KEEP-FACET", "Handle.set_is_focused / read FUTURE", "write shipped with v1 (hand-backed)"),
    ("VisualElement", "Focus"):        ("KEEP-FACET", "Handle.set_is_focused(true)", "method folds into the verb"),
    ("VisualElement", "Unfocus"):      ("KEEP-FACET", "Handle.set_is_focused(false)", "method folds into the verb"),
    ("Label", "Text"):                 ("KEEP-FACET", "Handle.set_text / text()", "core verb"),
    ("Label", "TextColor"):            ("KEEP-FACET", "Handle.set_text_color", "wired in v1, stays"),
    ("Button", "Text"):                ("KEEP-FACET", "Handle.set_title", "wired in v1, stays"),
    ("Button", "Clicked"):             ("KEEP-FACET", "on_click at build time", "facet wires handlers in the description"),
    ("Entry", "Placeholder"):          ("KEEP-FACET", "Handle.set_placeholder", "wired in v1, stays"),
    ("InputView", "IsReadOnly"):       ("KEEP-FACET", "Handle.set_is_read_only", "wired in v1, stays"),
    ("ProgressBar", "Progress"):       ("KEEP-FACET", "Handle.set_progress", "wired in v1, stays"),
    ("Picker", "SelectedIndex"):       ("KEEP-FACET", "Handle.selected_index()", "read shipped; the WRITE half is ADOPT: set_selected_index"),
    ("Slider", "Value"):               ("KEEP-FACET", "Handle.set_value / value()", "core verb"),
    ("Slider", "Minimum"):             ("KEEP-FACET", "Handle.set_minimum", "v1 slider contract, stays"),
    ("Slider", "Maximum"):             ("KEEP-FACET", "Handle.set_maximum", "v1 slider contract, stays"),
    ("ItemsView", "ItemsSource"):      ("DROP", "", "binding-reactive channel — facet lists are count+row builder (keyed-direct); updates via Handle.set_count"),
    ("ItemsView", "ScrollTo"):         ("KEEP-FACET", "Handle.set_scroll_offset / agent scroll_to", "shipped"),
    ("ListView", "ScrollTo"):          ("KEEP-FACET", "Handle.set_scroll_offset / agent scroll_to", "shipped"),
    # -- drag & drop: facet's drop half exists; the source half is the adopt --
    ("DropGestureRecognizer", "Drop"): ("KEEP-FACET", "on_drop + dropped_text + pointer_position", "shipped surface"),
    ("DropGestureRecognizer", "DragOver"): ("KEEP-FACET", "drag_targeted", "shipped 1c71b20"),
    ("DropGestureRecognizer", "AllowDrop"): ("ADOPT", "set_accepts_drop", "portable toggle for a drop target"),
    ("DragGestureRecognizer", "DragStarting"): ("ADOPT", "on_drag_start", "the SOURCE half facet lacks — start payload set portably"),
    ("DragGestureRecognizer", "CanDrag"): ("ADOPT", "set_draggable", "portable toggle for a drag source"),
    ("TapGestureRecognizer", "Tapped"): ("KEEP-FACET", "on_click", "facet's click"),
    ("TapGestureRecognizer", "NumberOfTapsRequired"): ("ADOPT", "on_double_click (as its own hook)", "double-click is the real demand"),
    ("PinchGestureRecognizer", "PinchUpdated"): ("KEEP-FACET", "Chrome zoomable (app-level)", "facet zooms the window content, not per-node"),
    # -- window runtime --
    ("Window", "Activated"):           ("ADOPT", "runtime::on_window_active(cb)", "focus/blur pair"),
    ("Window", "Deactivated"):         ("ADOPT", "runtime::on_window_inactive(cb)", "focus/blur pair"),
    ("Window", "Title"):               ("KEEP-FACET", "Chrome title / present_window", "set at open"),
}

DISCRETE_EVENT_HINTS = ("Clicked", "Pressed", "Released", "Completed", "Tapped",
                        "Focused", "Unfocused", "Changed", "Toggled", "Selected")


def snake(name: str) -> str:
    s = re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()
    return s.replace("__", "_")


def default_row(ty, member, band, valuety):
    """(status, facet_name, note) by the mechanical rules."""
    for pat, why in DROP_PATTERNS:
        if pat.search(member):
            return ("DROP", "", why)
    if band in ("writes", "reads"):
        if valuety not in TYMAP:
            return ("FUTURE", "", f"no facet mapping for `{valuety}` yet")
        base = snake(member)
        if band == "writes":
            return ("ADOPT", f"set_{base} / {base}()", f"{TYMAP[valuety]}")
        return ("ADOPT", f"{base}()", f"{TYMAP[valuety]} (read-only)")
    if band == "events":
        base = snake(member)
        prefix = "on_" if member.endswith(DISCRETE_EVENT_HINTS) else "observe_"
        return ("ADOPT", f"{prefix}{base}", "Cancellable" if prefix == "observe_" else "ctx-trailing callback")
    # methods: framework plumbing is dropped by shape; the rest wait for a
    # per-method mapping decision.
    if re.match(r"^(Map|On|Send|Update|Remap)", member):
        return ("DROP", "", "handler-mapper / internal plumbing, not app surface")
    return ("FUTURE", "", "methods adopt one by one, on demand")


def main():
    spec = json.load(open(SPEC))
    rows = []
    for ty, bands in spec.items():
        for band in ("writes", "reads", "events", "methods"):
            for member, vt in bands.get(band, {}).items():
                valuety = vt if isinstance(vt, str) else ""
                status, fname, note = OVERLAY.get(
                    (ty, member), default_row(ty, member, band, valuety))
                rows.append((ty, member, band, valuety, status, fname, note))

    counts = {}
    for r in rows:
        counts[r[4]] = counts.get(r[4], 0) + 1

    out = [
        "# MAUI -> facet MAP — DRAFT for review (maui-regen Phase 1)\n\n",
        "GENERATED by tools/maui_map.py; the curation lives THERE (overlay\n",
        "dict + drop patterns + type map). Mark rows up here or edit the\n",
        "overlay directly — the overlay is what Phase 2 consumes.\n\n",
        f"Row count: {len(rows)} — "
        + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) + "\n\n",
        "Statuses: ADOPT = generated into the contract; KEEP-FACET = already\n",
        "portable under facet's name (row is COVERED); DROP = deliberately\n",
        "absent, reason given; FUTURE = visible backlog, never silent.\n",
    ]
    cur = None
    for ty, member, band, valuety, status, fname, note in rows:
        if ty != cur:
            out.append(f"\n## {ty}\n\n")
            out.append("| MAUI member | band | status | facet | note |\n")
            out.append("|---|---|---|---|---|\n")
            cur = ty
        out.append(f"| {member} | {band} | **{status}** | {fname or '—'} | {note} |\n")

    path = os.path.join(ROOT, "plans", "facet", "maui-map-draft.md")
    with open(path, "w") as f:
        f.writelines(out)
    print(f"{len(rows)} rows — " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    print(f"wrote {os.path.relpath(path, ROOT)}")


if __name__ == "__main__":
    main()
