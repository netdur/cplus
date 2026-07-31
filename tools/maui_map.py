#!/usr/bin/env python3
"""maui_map.py — the curated MAP: MAUI surface -> facet contract names.

Stage 1 of the bootstrap. facet is an EMPTY package: it has no verbs, so no
row can claim "facet already has this", and no row may be deleted because a
platform cannot do it — that sentence belongs to facet_appkit, which answers
the contract, not to facet, which writes it.

So every row of the MAUI spec (plans/facet/spec/maui-spec.json, six bands)
lands in exactly ONE of two buckets:

  ADOPT   facet declares it, under a facet name (naming_guideline.md)
  DROP    MAUI's model, not facet's — three reasons, never any other:
            MODEL   MVVM/binding/templates/identity. facet describes UI with
                    components, keys, and fn-ptr handlers.
            LAYOUT  layout is flex_layout's. facet Nodes carry flex modifiers.
            ENGINE  MAUI's own internals (Map*/Send*/On*/measure/arrange/
                    batch). Never application vocabulary in any framework.

There is no FUTURE bucket and no UNSUPPORTED bucket. A row the rules cannot
decide FAILS this script by name; parking is not a status.

Default naming: PascalCase -> snake_case; writes gain `set_` and a bare
reader; reads keep the bare name; events gain `on_` (discrete) or `observe_`
(continuous); methods keep the verb. Anything the defaults get wrong is
overridden in OVERLAY, once.
"""
import json
import os
import re
import sys
from collections import OrderedDict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SPEC = os.path.join(ROOT, "plans", "facet", "spec", "maui-spec.json")

# ---- the three DROP reasons. This list does not grow. -----------------------
MODEL = "MAUI's MVVM model — facet describes UI with components, keys, and fn-ptr handlers"
LAYOUT = "layout belongs to flex_layout — facet Nodes carry flex modifiers"
ENGINE = "MAUI engine internals, not application vocabulary"

# ---- value types -> facet types. A scalar/struct the contract can carry. -----
TYMAP = {
    "string": "str",
    "double": "f64",
    "float": "f64",
    "int": "i64",
    "uint": "i64",
    "bool": "bool",
    "System.DateTime": "Date",
    "System.TimeSpan": "Duration",
    "Microsoft.Maui.Graphics.Color": "Color",
    "Microsoft.Maui.Graphics.Rect": "Rect",
    "Microsoft.Maui.Graphics.Size": "Size",
    "Microsoft.Maui.CornerRadius": "Corners",
    "Microsoft.Maui.Thickness": "Insets",
    "Microsoft.Maui.Graphics.Paint": "Brush",
    "Microsoft.Maui.Controls.Brush": "Brush",
    "Microsoft.Maui.Controls.Shadow": "Shadow",
    "Microsoft.Maui.Controls.ImageSource": "str",          # facet images take a path
    "Microsoft.Maui.Controls.FormattedString": "Spans",    # styled runs of text
    "Microsoft.Maui.Controls.Shapes.Geometry": "Shape",
    "Microsoft.Maui.Graphics.IShape": "Shape",
    "Microsoft.Maui.Controls.DoubleCollection": "f64[]",
    "float[]": "f64[]",
    "System.Collections.Generic.IList<string>": "str[]",
    # A subtree in the description is a Node, whatever MAUI's static type.
    "object": "Node",
    "Microsoft.Maui.Controls.Element": "Node",
    "Microsoft.Maui.Controls.View": "Node",
    "Microsoft.Maui.Controls.Page": "Node",
    "Microsoft.Maui.Controls.TableRoot": "Node",
    "Microsoft.Maui.ITitleBar": "Node",
    "Microsoft.Maui.Controls.Window": "Node",
}

# ---- MAUI enums -> facet enums. Vocabulary facet must DECLARE, not a drop. ---
ENUMS = {
    "Microsoft.Maui.TextAlignment": "TextAlign",
    "Microsoft.Maui.Controls.FontAttributes": "FontStyle",
    "Microsoft.Maui.TextTransform": "TextTransform",
    "Microsoft.Maui.TextDecorations": "TextDecoration",
    "Microsoft.Maui.LineBreakMode": "LineBreak",
    "Microsoft.Maui.ReturnType": "ReturnKey",
    "Microsoft.Maui.ScrollBarVisibility": "ScrollBars",
    "Microsoft.Maui.Aspect": "ImageFit",
    "Microsoft.Maui.FlowDirection": "FlowDirection",
    "Microsoft.Maui.ClearButtonVisibility": "ClearButton",
    "Microsoft.Maui.Keyboard": "Keyboard",
    "Microsoft.Maui.TextType": "TextType",
    "Microsoft.Maui.SafeAreaEdges": "SafeArea",
    "Microsoft.Maui.SwipeDirection": "SwipeDirection",
    "Microsoft.Maui.Controls.ButtonsMask": "Buttons",
    "Microsoft.Maui.Controls.Button.ButtonContentLayout": "ContentLayout",
    "Microsoft.Maui.Controls.SelectionMode": "SelectionMode",
    "Microsoft.Maui.Controls.ListViewSelectionMode": "SelectionMode",
    "Microsoft.Maui.Controls.SeparatorVisibility": "SeparatorVisibility",
    "Microsoft.Maui.Controls.EditorAutoSizeOption": "AutoSize",
    "Microsoft.Maui.Controls.ItemSizingStrategy": "ItemSizing",
    "Microsoft.Maui.Controls.ItemsUpdatingScrollMode": "ScrollAnchor",
    "Microsoft.Maui.Controls.TableIntent": "TableStyle",
    "Microsoft.Maui.Controls.Shapes.PenLineCap": "LineCap",
    "Microsoft.Maui.Controls.Shapes.PenLineJoin": "LineJoin",
}

# ---- types that carry MAUI's model, wherever they appear ---------------------
DROP_TYPES = {
    "System.Windows.Input.ICommand": MODEL,
    "Microsoft.Maui.Controls.BindingBase": MODEL,
    "Microsoft.Maui.Controls.DataTemplate": MODEL,
    "Microsoft.Maui.Controls.ControlTemplate": MODEL,
    "Microsoft.Maui.Controls.ResourceDictionary": MODEL,
    "System.Collections.Generic.IList<Microsoft.Maui.Controls.Behavior>": MODEL,
    "System.Collections.Generic.IList<Microsoft.Maui.Controls.TriggerBase>": MODEL,
    "System.Collections.IEnumerable": MODEL,
    "System.Collections.IList": MODEL,
    "System.Collections.Generic.IList<object>": MODEL,
    "System.Collections.Generic.IList<Microsoft.Maui.Controls.IGestureRecognizer>": MODEL,
    "Microsoft.Maui.Controls.LayoutOptions": LAYOUT,
    "Microsoft.Maui.Controls.IItemsLayout": LAYOUT,
    "Microsoft.Maui.IViewHandler": ENGINE,
    "Microsoft.Maui.Controls.IVisual": ENGINE,
    "Microsoft.Maui.Controls.Internals.IGestureController": ENGINE,
    "Microsoft.Maui.Controls.Internals.AutoId": ENGINE,
    "Microsoft.Maui.Controls.Internals.TableModel": ENGINE,
    "Microsoft.Maui.Controls.ListViewCachingStrategy": ENGINE,
    "System.Collections.Generic.IReadOnlyCollection<Microsoft.Maui.IWindowOverlay!>": ENGINE,
    "Microsoft.Maui.IVisualDiagnosticsOverlay": ENGINE,
}

# ---- member-name rules, applied before the type rules -----------------------
DROP_PATTERNS = [
    (re.compile(r"Command(Parameter)?$"), MODEL),
    (re.compile(r"Template$"), MODEL),
    (re.compile(r"^(ItemsSource|SelectedItems|BindingContext|Style|StyleClass|"
                r"Resources|Triggers|Behaviors|Effects|Navigation|Parent|"
                r"LogicalChildren|GestureRecognizers)$"), MODEL),
    (re.compile(r"^(AutomationId|ClassId|StyleId)$"), MODEL),
    (re.compile(r"^(Margin|Padding|WidthRequest|HeightRequest|"
                r"MinimumWidthRequest|MinimumHeightRequest|"
                r"MaximumWidthRequest|MaximumHeightRequest|"
                r"HorizontalOptions|VerticalOptions|ItemsLayout)$"), LAYOUT),
    (re.compile(r"^(IsInPlatformLayout|DisableLayout|DesiredSize|Frame)$"), LAYOUT),
    (re.compile(r"^(IsEnabledCore|IsPlatformEnabled|IsPlatformStateConsistent|"
                r"Batched|BatchCommitted|"
                r"MeasureInvalidated|FocusChangeRequested|ChildrenReordered|"
                r"ScrollToRequested|ModelChanged|PlatformSizeChanged|"
                r"HandlerChanged|HandlerChanging|Window|Visual)$"), ENGINE),
]

# Methods that are the layout engine's, not MAUI's bookkeeping. Same verdict
# (DROP), honest reason: flex_layout arranges and measures.
LAYOUT_METHODS = re.compile(
    r"^(Arrange|ArrangeOverride|Layout|LayoutChildren|MeasureOverride|OnMeasure|"
    r"ComputeConstraintForView|CrossPlatformArrange|CrossPlatformMeasure|"
    r"InvalidateMeasureNonVirtual|InvalidateMeasureOverride)$")

# ---- the methods band inverts: most rows are MAUI's own engine, so the
# APPLICATION verbs are whitelisted and everything else drops as ENGINE.
#
# MAUI repeats a member on every control that overrides it (OnMeasure x7,
# UpdateFormsText x7, MapText x5); facet has ONE method on Node, so those
# repetitions collapse rather than multiply. Collapsing is not promoting: a
# binding-context hook is still a hook however many types declare it. What
# DOES promote is a capability no adopted row already carries — relayout,
# measure, batched updates, children, and the native handle below.
METHOD_VOCABULARY = {
    ("VisualElement", "Focus"): "focus",
    ("VisualElement", "Unfocus"): "blur",
    # Shared Node methods: every element is a Node, so these are declared once.
    ("VisualElement", "InvalidateMeasure"): "relayout",
    ("VisualElement", "Measure"): "measure(width:height:)",
    ("VisualElement", "BatchBegin"): "begin_updates",
    ("VisualElement", "BatchCommit"): "end_updates",
    ("VisualElement", "GetChildElements"): "children()",
    ("View", "GetChildElements"): "children()",
    ("Label", "GetChildElements"): "children()",
    ("ProgressBar", "ProgressTo"): "animate_progress(to:duration:)",
    ("ItemsView", "ScrollTo"): "scroll_to(index:)",
    ("ListView", "ScrollTo"): "scroll_to(index:)",
    ("ListView", "BeginRefresh"): "begin_refresh",
    ("ListView", "EndRefresh"): "end_refresh",
}

# ---- per-row overrides, where a mechanical rule reads the row wrong ---------
# (Type, Member) -> (status, facet_name, note)
OVERLAY = {
    # `object`-typed rows that ARE vocabulary: a subtree, or a selection index.
    ("Picker", "SelectedItem"): ("DROP", "", MODEL + " — the selection is an index; set_selected_index carries it"),
    ("ListView", "SelectedItem"): ("DROP", "", MODEL + " — the selection is an index"),
    ("SelectableItemsView", "SelectedItem"): ("DROP", "", MODEL + " — the selection is an index"),
    ("RadioButton", "Value"): ("DROP", "", MODEL + " — the binding-era group value; a radio's identity is its key"),

    # The escape hatch INTENT blesses: an app may drop beneath facet and use
    # the platform view directly. MAUI's Handler is that seam, so facet names
    # it. A read, not a write — the backend owns what it points at.
    ("VisualElement", "Handler"): ("ADOPT", "native()", "*u8 (read-only) — deliberately platform-specific; the one row that names a backend"),

    # Window.Page is the root subtree, not a page object.
    ("Window", "Page"): ("ADOPT", "set_root / root()", "Node"),
    # TableView.Root is the same idea one level down: the content subtree.
    ("TableView", "Root"): ("ADOPT", "set_content / content()", "Node"),

    # MAUI splits a bound Header/Footer from the realized *Element it produces.
    # That split IS the binding model; facet writes a Node and reads it back
    # through one verb.
    ("ListView", "HeaderElement"): ("DROP", "", MODEL + " — the realized half of a bound Header; facet has one header verb"),
    ("ListView", "FooterElement"): ("DROP", "", MODEL + " — the realized half of a bound Footer; facet has one footer verb"),

    # Safe-area insets change how a child is laid out — flex_layout's business.
    ("Border", "SafeAreaEdges"): ("DROP", "", LAYOUT),

    # One corner verb, one type. MAUI types Button/RadioButton corners as int
    # and BoxView's as a 4-corner struct; facet carries Corners everywhere.
    ("Button", "CornerRadius"): ("ADOPT", "set_corner_radius / corner_radius()", "Corners"),
    ("RadioButton", "CornerRadius"): ("ADOPT", "set_corner_radius / corner_radius()", "Corners"),

    # Name corrections where snake_case reads wrong at the call site.
    ("VisualElement", "IsVisible"): ("ADOPT", "set_is_visible / is_visible()", "bool"),
    ("Switch", "IsToggled"): ("ADOPT", "set_on / is_on()", "bool — reads as an assertion"),
    ("CheckBox", "IsChecked"): ("ADOPT", "set_on / is_on()", "bool — one toggle verb across toggle/checkbox/radio"),
    ("RadioButton", "IsChecked"): ("ADOPT", "set_on / is_on()", "bool"),
    ("Button", "Text"): ("ADOPT", "set_title / title()", "str — a button carries a title, not text"),
    ("Picker", "Title"): ("ADOPT", "set_title / title()", "str"),
    ("Entry", "IsPassword"): ("ADOPT", "set_is_secure / is_secure()", "bool"),
    ("Label", "MaxLines"): ("ADOPT", "set_max_lines / max_lines()", "i64"),

    # Events whose default prefix reads wrong.
    ("Button", "Clicked"): ("ADOPT", "on_click", "ctx-trailing callback"),
    ("TapGestureRecognizer", "Tapped"): ("ADOPT", "on_click", "ctx-trailing callback"),
    ("Entry", "Completed"): ("ADOPT", "on_submit", "ctx-trailing callback"),
    ("Editor", "Completed"): ("ADOPT", "on_submit", "ctx-trailing callback"),
    ("SearchBar", "SearchButtonPressed"): ("ADOPT", "on_submit", "ctx-trailing callback"),
    ("VisualElement", "SizeChanged"): ("ADOPT", "observe_size", "events::Subscription — continuous"),
    ("VisualElement", "Loaded"): ("ADOPT", "on_attach", "lifecycle"),
    ("VisualElement", "Unloaded"): ("ADOPT", "on_detach", "lifecycle"),
    ("VisualElement", "IsLoaded"): ("ADOPT", "is_attached()", "bool (read-only) — attach is facet's word"),
    ("VisualElement", "Focused"): ("ADOPT", "on_focus", "ctx-trailing callback — pairs with the focus() verb"),
    ("VisualElement", "Unfocused"): ("ADOPT", "on_blur", "ctx-trailing callback — pairs with the blur() verb"),
    ("Window", "SizeChanged"): ("ADOPT", "observe_window_size", "events::Subscription — the window's own size"),
    ("Window", "Created"): ("ADOPT", "on_launch", "lifecycle"),
    ("Window", "Destroying"): ("ADOPT", "on_quit", "lifecycle"),
    ("Window", "Activated"): ("ADOPT", "observe_window_active", "events::Subscription"),
    ("Window", "Deactivated"): ("ADOPT", "observe_window_inactive", "events::Subscription"),
}

DISCRETE_EVENT_HINTS = ("Clicked", "Pressed", "Released", "Completed", "Tapped",
                        "Focused", "Unfocused", "Changed", "Toggled", "Selected",
                        "Appearing", "Disappearing", "Started", "Reached",
                        "Swiped", "Updated", "Drop", "Leave", "Over", "Canceled",
                        "Pushed", "Popped", "Pushing", "Popping", "Refreshing",
                        "Opened", "Closed",
                        "Entered", "Exited", "Moved", "Starting", "Created",
                        "Destroying", "Activated", "Deactivated", "Resumed",
                        "Stopped", "Backgrounding")


def snake(name: str) -> str:
    s = re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()
    return s.replace("__", "_")


# ---- who owns a Window row. facet's window story is Chrome: declarative
# presentation metadata a Screen hands the host (title/size/floors/zoom/
# titlebar), not a live object an app mutates. So the band decides the owner —
# you DESCRIBE a window at build time and READ or OBSERVE it at runtime.
def window_owner(member, band):
    if member == "Page":
        return "Screen"          # the root subtree comes from Screen::build
    return "Chrome" if band == "writes" else "runtime"


def default_row(ty, member, band, valuety):
    """(status, facet_name, note) by the mechanical rules, or None if undecided."""
    for pat, why in DROP_PATTERNS:
        if pat.search(member):
            return ("DROP", "", why)
    if band == "methods":
        name = METHOD_VOCABULARY.get((ty, member))
        if name:
            return ("ADOPT", name, "method")
        if LAYOUT_METHODS.match(member):
            return ("DROP", "", LAYOUT)
        return ("DROP", "", ENGINE)
    if valuety in DROP_TYPES:
        return ("DROP", "", DROP_TYPES[valuety])
    if band in ("writes", "reads"):
        fty = TYMAP.get(valuety) or ENUMS.get(valuety)
        if not fty:
            return None                      # undecided — the script fails on it
        base = snake(member)
        if band == "writes":
            return ("ADOPT", f"set_{base} / {base}()", fty)
        return ("ADOPT", f"{base}()", f"{fty} (read-only)")
    if band == "events":
        base = snake(member)
        discrete = member.endswith(DISCRETE_EVENT_HINTS)
        prefix = "on_" if discrete else "observe_"
        return ("ADOPT", f"{prefix}{base}",
                "ctx-trailing callback" if discrete else "events::Subscription")
    return None


def rows():
    spec = json.load(open(SPEC))
    out, undecided = [], []
    for ty, bands in spec.items():
        for band in ("writes", "reads", "events", "methods"):
            for member, vt in bands.get(band, {}).items():
                valuety = vt if isinstance(vt, str) else ""
                r = OVERLAY.get((ty, member)) or default_row(ty, member, band, valuety)
                if r is None:
                    undecided.append((ty, member, band, valuety))
                    continue
                if ty == "Window" and r[0] == "ADOPT":
                    r = (r[0], r[1], f"{r[2]} — on {window_owner(member, band)}")
                out.append((ty, member, band, valuety) + r)
    return out, undecided


def main():
    rs, undecided = rows()
    if undecided:
        print(f"UNDECIDED — {len(undecided)} rows have no bucket. "
              "Every row is ADOPT or DROP; there is no third status.\n",
              file=sys.stderr)
        for ty, member, band, vt in undecided:
            print(f"  {ty}.{member} [{band}] : {vt}", file=sys.stderr)
        raise SystemExit(1)

    counts = {}
    for r in rs:
        counts[r[4]] = counts.get(r[4], 0) + 1
    # Group the tally by the three base reasons; a row may append detail.
    reasons = {}
    for r in rs:
        if r[4] != "DROP":
            continue
        base = next((b for b in (MODEL, LAYOUT, ENGINE) if r[6].startswith(b)), r[6])
        reasons[base] = reasons.get(base, 0) + 1

    out = [
        "# MAUI -> facet MAP — bootstrap Stage 1\n\n",
        "GENERATED by tools/maui_map.py; the curation lives THERE (overlay dict,\n",
        "drop rules, type map, enum map). facet is an empty package, so every\n",
        "row is ADOPT (facet declares it) or DROP (MAUI's model, not facet's).\n",
        "There is no FUTURE and no UNSUPPORTED: what a backend cannot do is\n",
        "facet_appkit's sentence to pass, against a contract facet has already\n",
        "written.\n\n",
        f"Row count: {len(rs)} — "
        + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())) + "\n\n",
        "DROP reasons:\n\n",
    ]
    for why, n in sorted(reasons.items(), key=lambda kv: -kv[1]):
        out.append(f"- **{n}** — {why}\n")
    out.append("\n")
    cur = None
    for ty, member, band, valuety, status, fname, note in rs:
        if ty != cur:
            out.append(f"\n## {ty}\n\n")
            out.append("| MAUI member | band | status | facet | note |\n")
            out.append("|---|---|---|---|---|\n")
            cur = ty
        out.append(f"| {member} | {band} | **{status}** | {fname or '—'} | {note} |\n")

    path = os.path.join(ROOT, "plans", "facet", "maui-map-draft.md")
    with open(path, "w") as f:
        f.writelines(out)
    print(f"{len(rs)} rows — " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    for why, n in sorted(reasons.items(), key=lambda kv: -kv[1]):
        print(f"  DROP {n:>4}  {why}")
    print(f"wrote {os.path.relpath(path, ROOT)}")


if __name__ == "__main__":
    main()
