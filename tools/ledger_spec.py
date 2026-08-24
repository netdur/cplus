#!/usr/bin/env python3
"""ledger_spec.py — extract facet's portable surface spec from the ledger's public API.

Strategy: don't hand-curate the portable API (lossy). The ledger is a native-control-
backed cross-platform framework (like facet, unlike Flutter which renders), and
its portable surface is published as a machine-readable Roslyn public-API
manifest — the `netstandard` (platform-agnostic) PublicAPI.Shipped.txt.

v2 (row_type-regen Phase 1): the first pass consumed ONE band (settable bindable
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
  BASE=https://raw.githubusercontent.com/dotnet/row_type/main/src/Controls/src/Core/PublicAPI/netstandard
  curl -s $BASE/PublicAPI.Shipped.txt   -o plans/facet/spec/ledger_PublicAPI.Shipped.txt
  curl -s $BASE/PublicAPI.Unshipped.txt -o plans/facet/spec/ledger_PublicAPI.Unshipped.txt

Usage:
  python3 tools/ledger_spec.py <manifest.txt> [<manifest2.txt> ...]
Writes plans/facet/spec/ledger-spec.json + plans/facet/ledger-spec-report.md and
prints the per-type band counts.
"""
import json
import os
import re
import sys
from collections import OrderedDict, defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# The controls facet cares about (the ledger type -> facet widget). A control is a
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
    # Stage 1 closure: a type that is genuinely a control is extracted, because
    # "not this pass" is not one of the three DROP reasons and there is no
    # FUTURE bucket to park it in.
    ("ScrollView", "scroll"), ("ImageButton", "icon_button"),
    ("WebView", "web"), ("HybridWebView", "web (bridged)"),
    ("GraphicsView", "canvas"), ("RefreshView", "refreshable"),
    ("SwipeView", "swipeable"), ("SwipeItem", "swipe action"),
    ("CarouselView", "carousel"), ("IndicatorView", "page_dots"),
    ("Span", "span"), ("MenuItem", "menu_item"),
    ("MenuBarItem", "menu"), ("MenuFlyout", "context_menu"),
    ("MenuFlyoutItem", "context_menu_item"),
    ("ToolbarItem", "toolbar_item"),
])

# ---- what the ledger renders and facet has not built ------------------------
#
# The type list has been closed since Stage 1 — `check_type_closure` fails on a
# manifest type that is not EXTRACT / ALIAS / DROP. But it builds its work list
# from `getters`, `events` and `methods`, so it only ever saw a type that
# declares AT LEAST ONE ROW OF ITS OWN. A type whose whole surface is inherited
# declares none and was never asked about — and that is exactly the shape of a
# marker type. `MenuFlyoutSeparator` has one member, its constructor.
#
# So the floor is now the ledger's own definition of "this renders": every type
# MAUI puts on screen has an `I<Name>Handler` interface. `check_handler_closure`
# takes every one in the manifests and requires `<Name>` to classify. Nothing
# else has to be decided, and nothing that has to be can hide behind having no
# rows.
#
# UNBUILT is the fourth outcome that floor needed. EXTRACT/ALIAS/DROP could not
# say this: ALIAS means "already covered, nothing is being refused" and DROP
# means "facet answers this elsewhere or refuses it by rule". A type facet
# WOULD carry and simply has not is neither, and saying either of the other two
# is how three of these hid — `MenuFlyoutSubItem` was aliased to `MenuFlyout`
# and `SwipeItemView` to `SwipeItem`, which reads as "covered" for surfaces
# that do not exist.
#
# Each entry names the platform answer, so the next person reads a debt with a
# route rather than a hole.
UNBUILT = OrderedDict([
    # The menu family. facet has `context_menu` (MenuFlyout) and
    # `context_menu_item` (MenuFlyoutItem) and stops there.
    ("MenuFlyoutSubItem",
     "a submenu. NSMenu nests through `setSubmenu:`, so the platform answer is "
     "on the backend facet already has — was ALIAS:MenuFlyout, which claimed a "
     "flyout and a submenu are one surface"),
    ("MenuFlyoutSeparator",
     "the ledger has a separator as a TYPE; facet infers one from an empty "
     "title, which is what every menu-building API means by one, but there is "
     "no kind for it and nothing said so"),
    ("MenuBar",
     "facet has `menu` (MenuBarItem) and no bar to hold them — the app-menu "
     "tier assembles one in screen.cplus — was ALIAS:MenuBarItem, which is the "
     "ITEM, not the bar"),

    # The swipe family beyond the adopted `swipe_item`.
    ("SwipeItemView",
     "a swipe action that is an arbitrary view rather than a titled item — was "
     "ALIAS:SwipeItem, which is the titled one"),
    ("SwipeItemMenuItem",
     "the ledger's menu-flavoured swipe action; `swipe_item` is the adopted "
     "shape and facet_appkit already builds swipe actions AS a menu"),

    # Drawing.
    ("ShapeView",
     "vector shapes. `canvas` replays a recorded display list instead, which "
     "covers the drawing but not the ledger's retained shape objects"),
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
    ("Application", "the app tier"),
    ("Page", "a screen's base surface"),
    ("ContentPage", "a screen with one content root"),
    ("Toolbar", "the window toolbar"),
    ("TitleBar", "custom titlebar content"),
])

# Value types the contract carries by name. Their own properties define the
# facet struct — without them the contract references a type whose surface was
# never read (the Span/Shadow/TitleBar hole that closed this stage).
VALUE_TYPES = OrderedDict([
    ("Shadow", "the Shadow value"),
    ("FontImageSource", "an icon drawn from a font glyph"),
    ("SemanticProperties", "the accessibility band"),
    # Both surfaced by the closure check below, and both real vocabulary that
    # would otherwise have been absent without ever being refused.
    ("ToolTipProperties", "a tooltip on any element"),
    ("KeyboardAccelerator", "a keyboard shortcut on a command"),
])

ALL_TYPES = (list(CONTROLS) + list(SHARED_BASES) + list(RECOGNIZERS)
             + list(RUNTIME_TYPES) + list(VALUE_TYPES))

# ---------------------------------------------------------------------------
# Type-list closure. The ROWS are self-enforcing (ledger_map.py exits non-zero on
# a row it cannot bucket). Until this stage the TYPE list was not: three
# hand-written dicts at the top of a pipeline that is rigorous downstream, with
# nothing asserting they were closed against the manifest. That is where the
# hole was — `Span`, `TitleBar` and `Shadow` were named BY the contract and
# never extracted.
#
# Every type in the manifest now lands in one of four outcomes:
#   EXTRACT  it is above (a control, a base, a recognizer, runtime, a value)
#   ALIAS    the same surface as an extracted type, under another name. Not a
#            drop — nothing is being refused, it is already covered.
#   DROP     one of the three reasons, by family
#   (fail)   anything else names itself and stops the build
ALIAS = {
    "Frame": "Border — ledger row superseded Frame with Border, which is extracted",
    "AutomationProperties": "SemanticProperties — the older accessibility API",
    "FormattedString": "Span — the container of the spans facet carries",
    "Brush": "Color/Brush — 148 of its rows are static named colors",
    "SolidColorBrush": "Brush",
    "GradientBrush": "Brush",
    "LinearGradientBrush": "Brush",
    "RadialGradientBrush": "Brush",
    "GradientStop": "Brush",
    "ImageSource": "str — facet images take a path",
    "UriImageSource": "str",
    "FileImageSource": "str",
    "StreamImageSource": "str",
    "HtmlWebViewSource": "str",
    "UrlWebViewSource": "str",
    "WebViewSource": "str",
    "MenuItemCollection": "MenuItem",
    "SwipeItems": "SwipeItem",
    "TemplatedPage": "Page",
    "ContentView": "Border — a plain content container",
    "KnownColor": "Color",
    # Named so the handler floor below has an answer for them. Each is already
    # covered by an extracted type or an existing family rule in spirit; these
    # say which, rather than leaving the floor to fail on a name nobody has
    # thought about.
    "NavigationView": "Page — the ledger's nav host; facet has nav + Screen",
    "TabbedView": "TabbedPage — the tab band is the `tabs` control",
    "Reload": "WebView — a verb on the control, not a type",
    "DynamicResource": "the ledger's resource dictionary — MVVM, and facet has no resource tier",
}

# Family rules, in order. The reason strings match ledger_map.py's three.
_MODEL = "the ledger's MVVM model"
_LAYOUT = "layout belongs to flex_layout"
_ENGINE = "the ledger engine internals"

DROP_TYPE_RULES = [
    # navigation and the Shell model — facet has nav/Screen/ScreenBox
    (re.compile(r"^(I?Shell|SearchHandler|BaseShellItem|BackButtonBehavior|"
                r"Rout(ing|e)|Flyout|NavigationPage|INavigation|Navigable)"),
     f"{_MODEL} — the ledger's Shell/page navigation; facet has nav + Screen"),
    # cells and table sections: the ledger's row model. facet rows are components.
    (re.compile(r"(\w*Cell|TableSection\w*)$"),
     f"{_MODEL} — the ledger's cell model; facet rows are components"),
    # binding, templates, styles, triggers, behaviors, effects
    (re.compile(r"^(Binding\w*|MultiBinding|TemplateBinding|"
                r"RelativeBindingSource|Bindable\w*|Command|DataTemplate\w*|"
                r"ControlTemplate|Style|StyleSheet|Setter|"
                r"\w*Trigger\w*|VisualState\w*|Behavior|"
                r"ResourceDictionary|ContentPresenter|TemplatedView|"
                r"StyleableElement|RadioButtonGroup)$|"
                r"(Effect|Condition|Template)$"),
     f"{_MODEL} — binding/template/style machinery"),
    # XAML markup plumbing: converters, markup extensions, attributes
    (re.compile(r"(TypeConverter|Converter|Extension|MarkupExtension|"
                r"Attribute)$|"
                r"^(Xmlns\w*|XamlC?\w*|OnPlatform|OnIdiom|On)$"),
     f"{_MODEL} — XAML markup machinery; facet's description is the @facet DSL"),
    # layout containers and the geometry that positions them
    (re.compile(r"^(Layout|FlexLayout|Grid|Stack\w*|HorizontalStackLayout|"
                r"VerticalStackLayout|AbsoluteLayout|RelativeLayout|"
                r"BindableLayout|CompressedLayout|GridItemsLayout|"
                r"LinearItemsLayout|ItemsLayout|LayoutOptions|Region|"
                r"RowDefinition\w*|ColumnDefinition\w*|GridLength\w*|"
                r"Thickness\w*|CornerRadius\w*|BoundsConstraint)$"),
     f"{_LAYOUT} — containers and geometry"),
    # engine: controller/element interfaces, event-arg payloads, statics
    (re.compile(r"^I\w+(Controller|Element)$"),
     f"{_ENGINE} — controller/mixin interfaces"),
    (re.compile(r"EventArgs$|Args$"),
     f"{_ENGINE} — event payload objects; facet callbacks carry values"),
    (re.compile(r"Extensions$|Manager$|Handler\w*$|^Device$|^Animation$|"
                r"^Registrar|^Internals|^Platform|^DesignMode$|"
                r"^DependencyService$|Callback$"),
     f"{_ENGINE} — statics, handlers, and platform plumbing"),
    (re.compile(r"^(DataPackage\w*|Element|Application\w+|VisualElement\w+|"
                r"PropertyChang\w*|WeakEvent\w*|AppLinkEntry|"
                r"VisualMarker|ChildGestureRecognizer)$"),
     f"{_ENGINE} — framework objects, not application vocabulary"),
    (re.compile(r"^I[A-Z]"),
     f"{_ENGINE} — interface surface behind a control facet already extracts"),
]


def classify(ty):
    """EXTRACT / ALIAS:<what> / DROP:<reason>, or None when unclassified."""
    if ty in ALL_TYPES:
        return "EXTRACT"
    if ty in ALIAS:
        return "ALIAS:" + ALIAS[ty]
    if ty in UNBUILT:
        return "UNBUILT:" + UNBUILT[ty]
    for pat, why in DROP_TYPE_RULES:
        if pat.search(ty):
            return "DROP:" + why
    return None

# Roslyn PublicAPI lines carry a nullability marker and modifier prefix, e.g.
# "~static readonly Slider.MaximumProperty -> ...".
MODS = re.compile(r"^~?(?:(?:static|readonly|const|abstract|virtual|override|"
                  r"sealed|event|new)\s+)*")
# Whatever namespace the manifest declares its controls under. Matched
# generically rather than spelled: the framework the rows were read from is
# not part of facet's vocabulary, and a name in the source is a name a
# reader — a model especially — will take as a pattern to follow.
NS = r"(?:[A-Za-z_][A-Za-z0-9_]*\.)+"
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


def check_handler_closure(paths):
    """Every type the ledger RENDERS is decided, whether or not it declares rows.

    `check_type_closure` below builds its work list from the rows a type
    declares, so a type whose surface is entirely inherited was never asked
    about — which is exactly what a marker type looks like.
    `MenuFlyoutSeparator` declares one member, its constructor.

    The ledger says which types it renders without anyone having to judge:
    each one has an `I<Name>Handler` interface. That IS the definition of "a
    thing on screen with a platform body behind it", so it is the floor.

    CALLED FROM `gen_contract.check`, not from `main` below, because that is
    where it RUNS. This file regenerates the spec from the raw manifests and
    has not been runnable for some time — `check_type_closure` fails on
    `Matrix`, `NavigationProxy`, `Shape`, `TableModel` and the GIF decoder,
    none of which have a family rule. So `ledger-spec.json` is a frozen
    artifact, and a floor that only fires here would be a floor that never
    fires. Repairing this tool is its own job; putting the check in the
    generator that runs on every regen is not waiting for it.
    """
    seen = set()
    for path in paths:
        seen |= set(re.findall(r"(?<![A-Za-z])I([A-Z][A-Za-z]*)Handler\b",
                               open(path).read()))
    unclassified = sorted(t for t in seen if classify(t) is None)
    if unclassified:
        print("UNDECIDED RENDERABLE TYPES — the ledger has a handler for each of\n"
              "these, so it renders them, and nothing here says what facet does\n"
              "about them. Extract, alias to an extracted type, drop by family,\n"
              "or record it in UNBUILT with the platform answer.\n", file=sys.stderr)
        for ty in unclassified:
            print(f"  {ty}", file=sys.stderr)
        raise SystemExit(1)
    return {t: classify(t)[8:] for t in sorted(seen) if classify(t).startswith("UNBUILT:")}


def check_type_closure(getters, events, methods):
    """Every manifest type with surface must be EXTRACT / ALIAS / DROP.

    The rows have been self-enforcing since Stage 1 (ledger_map.py exits on a row
    it cannot bucket). This gives the TYPE list the same property, so a control
    cannot go missing the next time the ledger adds one. Returns (aliases, drops) for
    the report; raises on anything unclassified.
    """
    surface = defaultdict(int)
    for d in (getters, events, methods):
        for ty, members in d.items():
            surface[ty] += len(members)
    unclassified, aliases, drops = [], {}, {}
    for ty in sorted(surface):
        verdict = classify(ty)
        if verdict is None:
            unclassified.append((surface[ty], ty))
        elif verdict.startswith("ALIAS:"):
            aliases[ty] = verdict[6:]
        elif verdict.startswith("DROP:"):
            drops[ty] = verdict[5:]
    if unclassified:
        unclassified.sort(reverse=True)
        print("UNCLASSIFIED TYPES — the type list is not closed. Each must be\n"
              "extracted, aliased to an extracted type, or dropped by family\n"
              "with one of the three reasons.\n", file=sys.stderr)
        for n, ty in unclassified:
            print(f"  {ty}  ({n} surface rows)", file=sys.stderr)
        raise SystemExit(1)
    return aliases, drops


def main():
    if len(sys.argv) < 2:
        print(__doc__)
        sys.exit(2)
    getters, setters, events, methods = parse(sys.argv[1:])
    aliases, drops = check_type_closure(getters, events, methods)

    out = OrderedDict()
    md = [
        "# the ledger portable-surface spec — GENERATED by tools/ledger_spec.py\n\n",
        "Six bands per type (writes / reads / events / methods), for the\n"
        "controls facet declares, the shared bases, the gesture recognizers,\n"
        "and the runtime's Window. This file is the permanent checklist:\n"
        "\"facet is missing X\" starts by looking X up HERE.\n\n",
    ]

    def section(title, table, describe):
        md.append(f"\n## {title}\n\n")
        for row_type, facet_name in table.items():
            w, r = band_split(getters, setters, row_type)
            ev = events.get(row_type, {})
            me = methods.get(row_type, {})
            out[row_type] = {
                describe: facet_name,
                "writes": w,
                "reads": r,
                "events": {k: v for k, v in sorted(ev.items())},
                "methods": {k: {"params": p, "returns": ret}
                            for k, (p, ret) in sorted(me.items())},
            }
            md.append(f"### {row_type} — {facet_name}\n\n")
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
    section("Value types the contract carries by name", VALUE_TYPES, "role")

    # The closure ledger: what is NOT extracted, and why. Absence is stated.
    md.append("\n## Not extracted — aliased to a type that is\n\n")
    md.append("Not a refusal: the same surface under another name.\n\n")
    md.append("| the ledger type | covered by |\n|---|---|\n")
    for ty, what in sorted(aliases.items()):
        md.append(f"| {ty} | {what} |\n")
    md.append("\n## Not extracted — dropped by family\n\n")
    by_reason = defaultdict(list)
    for ty, why in drops.items():
        by_reason[why].append(ty)
    for why in sorted(by_reason, key=lambda k: -len(by_reason[k])):
        md.append(f"\n**{len(by_reason[why])} types** — {why}\n\n")
        md.append("> " + ", ".join(sorted(by_reason[why])) + "\n")

    spec_dir = os.path.join(ROOT, "plans", "facet", "spec")
    os.makedirs(spec_dir, exist_ok=True)
    with open(os.path.join(spec_dir, "ledger-spec.json"), "w") as f:
        json.dump(out, f, indent=1)
    with open(os.path.join(ROOT, "plans", "facet", "ledger-spec-report.md"), "w") as f:
        f.writelines(md)

    print(f"{'type':<26}{'writes':>7}{'reads':>7}{'events':>7}{'methods':>8}")
    print("-" * 55)
    totals = [0, 0, 0, 0]
    for row_type in ALL_TYPES:
        w, r = band_split(getters, setters, row_type)
        ev, me = events.get(row_type, {}), methods.get(row_type, {})
        for i, n in enumerate((len(w), len(r), len(ev), len(me))):
            totals[i] += n
        print(f"{row_type:<26}{len(w):>7}{len(r):>7}{len(ev):>7}{len(me):>8}")
    print("-" * 55)
    print(f"{'TOTAL':<26}{totals[0]:>7}{totals[1]:>7}{totals[2]:>7}{totals[3]:>8}")
    print("\nwrote plans/facet/spec/ledger-spec.json + plans/facet/ledger-spec-report.md")


if __name__ == "__main__":
    main()
