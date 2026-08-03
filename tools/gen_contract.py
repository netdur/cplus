#!/usr/bin/env python3
"""gen_contract.py — emit facet's declared contract from the Stage 1 ledger.

Stage 2. Input is the curated MAP (tools/maui_map.py) over the extracted spec
(tools/maui_spec.py). Output is whole generated files under vendor/facet/src —
the generator owns them entirely, so regeneration is file replacement and no
hand edit can drift.

Shape, settled by the slice in commit c20c4e5:

  a control  =  a props struct
              + dirty bits
              + a constructor (every prop a defaulted named parameter)
              + a typed cursor (the live handle `find` hands back)

  the shared band (21 verbs every element carries) lives ONCE on Node in the
  hand-written facet.cplus, reached through generated one-line forwards on
  each cursor. There is no generic element type: `find` returns the control,
  with everything it can do and nothing it cannot.

Emission is unconditional: facet declares the whole vocabulary, and a backend
answers it or states that it cannot. A verb absent from a control is absent
because the ledger dropped that row, never because nothing wired it.

  python3 tools/gen_contract.py
"""
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import maui_map  # noqa: E402 — the curated MAP is the authority
import maui_spec  # noqa: E402 — the control list and its facet names

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ROOT, "vendor", "facet", "src")
DOCS = os.path.join(ROOT, "vendor", "facet", "docs")

# ---------------------------------------------------------------------------
# Which shared bases merge into which control. MAUI expresses this with
# inheritance; C+ has none, so the generator does the merge and every control
# ends up carrying its own copy of what it inherits.
COMMON_BASES = ["VisualElement", "View", "GestureElement"]

EXTRA_BASES = {
    "Entry": ["InputView"], "Editor": ["InputView"], "SearchBar": ["InputView"],
    "CollectionView": ["ItemsView", "StructuredItemsView", "SelectableItemsView",
                       "GroupableItemsView", "ReorderableItemsView"],
    "CarouselView": ["ItemsView", "StructuredItemsView"],
}

# Module + cursor names. maui_spec's facet_widget values are prose in places.
MODULE = {
    "Label": "label", "Button": "button", "Entry": "text_field",
    "Editor": "text_area", "SearchBar": "search_field", "Picker": "popup",
    "Slider": "slider", "Stepper": "stepper", "Switch": "toggle",
    "DatePicker": "date_picker", "TimePicker": "time_picker",
    "ProgressBar": "progress", "ActivityIndicator": "spinner",
    "Image": "image", "CollectionView": "collection", "ListView": "list",
    "TableView": "table", "TabbedPage": "tabs", "CheckBox": "checkbox",
    "RadioButton": "radio", "BoxView": "box", "Border": "bordered",
    "ScrollView": "scroll", "ImageButton": "icon_button", "WebView": "web",
    "HybridWebView": "hybrid_web", "GraphicsView": "canvas",
    "RefreshView": "refreshable", "SwipeView": "swipeable",
    "SwipeItem": "swipe_item", "CarouselView": "carousel",
    "IndicatorView": "page_dots", "Span": "span", "MenuItem": "menu_item",
    "MenuBarItem": "menu", "MenuFlyout": "context_menu",
    "MenuFlyoutItem": "context_menu_item", "ToolbarItem": "toolbar_item",
}

# ---------------------------------------------------------------------------
# facet types -> (C+ type, zero value, is_owned_string)
TYPES = {
    "str":   ("text::Text", "text::new()", "str"),
    "f64":   ("f64", "0.0f64", None),
    "i64":   ("i64", "0 as i64", None),
    "bool":  ("bool", "false", None),
    "Color": ("vocab::Color", "vocab::Color::clear()", None),
    "Corners": ("vocab::Corners", "vocab::Corners::none()", None),
    "Insets": ("vocab::Insets", "vocab::Insets::zero()", None),
    "Point": ("vocab::Point", "vocab::Point::zero()", None),
    "Rect":  ("vocab::Rect", "vocab::Rect::zero()", None),
    "Size":  ("vocab::Size", "vocab::Size::zero()", None),
    "Brush": ("vocab::Brush", "vocab::Brush::none()", None),
    "Shadow": ("vocab::Shadow", "vocab::Shadow::none()", None),
    "Shape": ("vocab::Shape", "vocab::Shape::none()", None),
    "Spans": ("vocab::Spans", "vocab::Spans::none()", None),
    "Drawable": ("vocab::Drawable", "vocab::Drawable::none()", None),
    "Date":  ("vocab::Date", "vocab::Date::zero()", None),
    "Duration": ("vocab::Duration", "vocab::Duration::zero()", None),
    "Window": ("vocab::WindowRef", "vocab::WindowRef::none()", None),
}

# Collection-shaped facet types become a count + an opaque owner the backend
# fills; the description carries children as real tree nodes instead.
SKIP_TYPES = {"Node", "Node[]", "str[]", "f64[]", "SwipeItem[]", "Window[]",
              "ToolbarItem[]", "MenuBarItem[]", "KeyboardAccelerator[]",
              "*u8", "Drawable"}


def cplus_type(t):
    if t in TYPES:
        return TYPES[t][0]
    if t in ENUM_BY_FACET:
        return "vocab::" + t
    return None


def zero_of(t):
    if t in TYPES:
        return TYPES[t][1]
    if t in ENUM_BY_FACET:
        return "vocab::" + t + "::" + ENUM_BY_FACET[t][0][0]
    return None


# ---------------------------------------------------------------------------
# Enum members, read out of the two PublicAPI manifests. Four enums have no
# machine-readable source (MAUI models them as classes or they live outside
# both manifests); those are authored here and marked.
HAND_ENUMS = {
    "Appearance": ["Unspecified", "Light", "Dark"],
    "Keyboard": ["Default", "Plain", "Chat", "Email", "Numeric", "Telephone", "Url", "Text"],
    "SafeArea": ["Default", "None", "Container", "Content", "All"],
    "ContentLayout": ["ImageLeft", "ImageTop", "ImageRight", "ImageBottom"],
}

MANIFESTS = [
    os.path.join(ROOT, "plans", "facet", "spec", "maui_PublicAPI.Shipped.txt"),
    os.path.join(ROOT, "plans", "facet", "spec", "maui_PublicAPI.Unshipped.txt"),
    os.path.join(ROOT, "plans", "facet", "spec", "maui_Core_PublicAPI.Shipped.txt"),
]


def read_enums():
    """facet enum name -> (members, provenance). Manifest first, then HAND."""
    blob = ""
    for p in MANIFESTS:
        if os.path.exists(p):
            blob += open(p).read()
    out = {}
    for maui, facet in maui_map.ENUMS.items():
        ms = re.findall(rf"^{re.escape(maui)}\.(\w+) = -?\d+ ->", blob, re.M)
        if ms and facet not in out:
            out[facet] = (sorted(set(ms)), maui)
    for facet, ms in HAND_ENUMS.items():
        if facet not in out:
            src = [m for m, f in maui_map.ENUMS.items() if f == facet]
            out[facet] = (ms, (src[0] if src else "hand") + " (authored: no manifest source)")
    return out


ENUM_BY_FACET = {}


def snake(name):
    s = re.sub(r"(?<!^)(?=[A-Z])", "_", name).lower()
    return s.replace("__", "_")


def pascal(mod):
    return "".join(p.capitalize() for p in mod.split("_"))


# ---------------------------------------------------------------------------
def control_rows():
    """control -> {writes, reads, events} of ADOPT rows, bases merged in."""
    rows, undecided = maui_map.rows()
    assert not undecided
    by_type = {}
    for ty, member, band, vt, st, fn, note in rows:
        if st != "ADOPT":
            continue
        by_type.setdefault(ty, []).append((member, band, fn, note))

    out = {}
    for maui in MODULE:
        merged, seen = [], set()
        for src in [maui] + EXTRA_BASES.get(maui, []):
            for member, band, fn, note in by_type.get(src, []):
                verb = fn.split(" / ")[0].split("(")[0].strip()
                if verb in seen:
                    continue
                seen.add(verb)
                merged.append((member, band, fn, note, src))
        out[maui] = merged
    return out, by_type


# The content parameter: naming_guideline's "constructors take their content".
# It leads the signature, then `key`, then everything else defaulted.
PRIMARY = {
    "Button": "title", "Label": "text", "Entry": "text", "Editor": "text",
    "SearchBar": "text", "Picker": "title", "Slider": "value",
    "Stepper": "value", "Switch": "on", "CheckBox": "on", "RadioButton": "on",
    "Image": "source", "ImageButton": "source", "ProgressBar": "progress",
    "Span": "text", "MenuItem": "text", "MenuFlyoutItem": "text",
    "ToolbarItem": "text", "MenuBarItem": "text", "WebView": "source",
    "HybridWebView": "source", "BoxView": "color", "ActivityIndicator": "color",
    "DatePicker": "date", "TimePicker": "time", "IndicatorView": "count",
}

# MAUI member names that collide with C+ keywords. Renamed once, here.
RESERVED = {
    "loop": "wraps", "type": "kind", "match": "matches", "ref": "reference",
    "static": "is_static", "const": "constant", "return": "result",
    "break": "breaks", "continue": "continues", "as": "as_kind",
    "if": "condition", "else": "otherwise", "for": "target", "while": "during",
    "fn": "callback", "impl": "implementation", "enum": "choice",
    "struct": "shape", "let": "binding", "var": "variable", "this": "self",
    "true": "yes", "false": "no", "take": "takes", "defer": "deferred",
    "assert": "asserts", "export": "exported", "import": "imported",
    "interface": "protocol", "unsafe": "unchecked", "extern": "external",
    "opaque": "opaque_value",
}


# naming_guideline.md: "Booleans read as assertions: is_editable... A boolean
# parameter is named so the call reads naturally: set_editable(false)." So the
# reader keeps the assertion prefix and the setter drops it.
BOOL_PREFIXES = ("is_", "has_", "can_")


def verb_stem(field, ty, taken):
    """The name a setter and a constructor parameter use for `field`."""
    if ty != "bool":
        return field
    for pre in BOOL_PREFIXES:
        if field.startswith(pre):
            stem = field[len(pre):]
            # a stem that collides with a sibling field or a C+ keyword keeps
            # the assertion form rather than becoming unspellable
            if stem and stem not in taken and stem not in RESERVED:
                return stem
            return field
    return field


def prop_of(fn, note):
    """(field name, facet type) for a write/read row, or None if unmappable."""
    verb = fn.split(" / ")[0].split("(")[0].strip()
    field = verb[4:] if verb.startswith("set_") else verb
    field = RESERVED.get(field, field)
    ty = note.split(" — ")[0].replace(" (read-only)", "").strip()
    if ty in SKIP_TYPES or cplus_type(ty) is None:
        return None
    return (field, ty)


VALUE_TYPES_SRC = '''
// ---- value types the contract carries by name -------------------------------

struct Color { r: f64, g: f64, b: f64, a: f64 }
impl Color {
    fn rgba(r: f64, g: f64, b: f64, a: f64 = 1.0f64) -> Color { return Color { r: r, g: g, b: b, a: a }; }
    fn clear() -> Color { return Color::rgba(0.0f64, 0.0f64, 0.0f64, 0.0f64); }
}

struct Corners { top_leading: f64, top_trailing: f64, bottom_leading: f64, bottom_trailing: f64 }
impl Corners {
    fn all(v: f64) -> Corners { return Corners { top_leading: v, top_trailing: v, bottom_leading: v, bottom_trailing: v }; }
    fn none() -> Corners { return Corners::all(0.0f64); }
}

struct Insets { leading: f64, top: f64, trailing: f64, bottom: f64 }
impl Insets {
    fn all(v: f64) -> Insets { return Insets { leading: v, top: v, trailing: v, bottom: v }; }
    fn zero() -> Insets { return Insets::all(0.0f64); }
}

struct Point { x: f64, y: f64 }
impl Point { fn zero() -> Point { return Point { x: 0.0f64, y: 0.0f64 }; } }

struct Size { width: f64, height: f64 }
impl Size { fn zero() -> Size { return Size { width: 0.0f64, height: 0.0f64 }; } }

struct Rect { x: f64, y: f64, width: f64, height: f64 }
impl Rect { fn zero() -> Rect { return Rect { x: 0.0f64, y: 0.0f64, width: 0.0f64, height: 0.0f64 }; } }

// A fill: a flat colour, or a gradient the backend resolves.
struct Brush { start: Color, end: Color, angle: f64, is_gradient: bool }
impl Brush {
    fn solid(c: Color) -> Brush { return Brush { start: c, end: c, angle: 0.0f64, is_gradient: false }; }
    fn none() -> Brush { return Brush::solid(Color::clear()); }
}

struct Shadow { color: Color, offset: Point, radius: f64, opacity: f64 }
impl Shadow {
    fn none() -> Shadow { return Shadow { color: Color::clear(), offset: Point::zero(), radius: 0.0f64, opacity: 0.0f64 }; }
}

// A clip / stroke outline. `kind` 0 = none, 1 = rect, 2 = rounded, 3 = ellipse.
struct Shape { kind: i64, corners: Corners }
impl Shape {
    fn none() -> Shape { return Shape { kind: 0 as i64, corners: Corners::none() }; }
    fn rounded(r: f64) -> Shape { return Shape { kind: 2 as i64, corners: Corners::all(r) }; }
}

// Styled runs over a text surface. Empty until a run is added.
struct Spans { count: i64 }
impl Spans { fn none() -> Spans { return Spans { count: 0 as i64 }; } }

// A draw callback for a canvas: (ctx, width, height).
struct Drawable { draw: fn(*u8, f64, f64), opaque ctx: *u8 }
fn draw_none(ctx: *u8, w: f64, h: f64) { return; }
impl Drawable { fn none() -> Drawable { return Drawable { draw: draw_none, ctx: 0 as *u8 }; } }

struct Date { year: i64, month: i64, day: i64 }
impl Date { fn zero() -> Date { return Date { year: 0 as i64, month: 0 as i64, day: 0 as i64 }; } }

struct Duration { seconds: f64 }
impl Duration { fn zero() -> Duration { return Duration { seconds: 0.0f64 }; } }

// A live window. Opaque until a backend mounts one.
struct WindowRef { opaque _w: *u8 }
impl WindowRef {
    fn none() -> WindowRef { return WindowRef { _w: 0 as *u8 }; }
    fn is_open(this) -> bool { return this._w != (0 as *u8); }
}
'''


def emit_vocabulary():
    out = ["// GENERATED by tools/gen_contract.py — DO NOT EDIT.\n",
           "// The types facet's contract names. Enums are seeded from MAUI's, renamed\n",
           "// per naming_guideline.md; members come from the PublicAPI manifests.\n\n"]
    for facet in sorted(ENUM_BY_FACET):
        members, prov = ENUM_BY_FACET[facet]
        out.append(f"// {prov}\n")
        out.append(f"enum {facet} {{\n")
        for m in members:
            out.append(f"    {m},\n")
        out.append("}\n\n")
    out.append(VALUE_TYPES_SRC.lstrip("\n"))
    return "".join(out)


COMMON_SRC = '''
// ---- the shared band --------------------------------------------------------
// The write rows every element carries (MAUI VisualElement), minus the ones
// flex already owns and the ones only the platform knows. Inline in Data, not
// boxed: every node has exactly one, so no allocation and no cast.

struct CommonProps {
    opacity: f64,
    background_color: vocab::Color,
    background: vocab::Brush,
    shadow: vocab::Shadow,
    clip: vocab::Shape,
    is_enabled: bool,
    is_visible: bool,
    input_transparent: bool,
    flow_direction: vocab::FlowDirection,
    rotation: f64, rotation_x: f64, rotation_y: f64,
    scale: f64, scale_x: f64, scale_y: f64,
    translation_x: f64, translation_y: f64,
    anchor_x: f64, anchor_y: f64,
}

impl CommonProps {
    fn new() -> CommonProps {
        return CommonProps {
            opacity: 1.0f64,
            background_color: vocab::Color::clear(),
            background: vocab::Brush::none(),
            shadow: vocab::Shadow::none(),
            clip: vocab::Shape::none(),
            is_enabled: true,
            is_visible: true,
            input_transparent: false,
            flow_direction: vocab::FlowDirection::MatchParent,
            rotation: 0.0f64, rotation_x: 0.0f64, rotation_y: 0.0f64,
            scale: 1.0f64, scale_x: 1.0f64, scale_y: 1.0f64,
            translation_x: 0.0f64, translation_y: 0.0f64,
            anchor_x: 0.5f64, anchor_y: 0.5f64,
        };
    }
}

// Common bits live at the top of the word; per-control bits start at 0.
const C_OPACITY: u64 = 281474976710656u64;
const C_BACKGROUND_COLOR: u64 = 562949953421312u64;
const C_BACKGROUND: u64 = 1125899906842624u64;
const C_SHADOW: u64 = 2251799813685248u64;
const C_CLIP: u64 = 4503599627370496u64;
const C_IS_ENABLED: u64 = 9007199254740992u64;
const C_IS_VISIBLE: u64 = 18014398509481984u64;
const C_INPUT_TRANSPARENT: u64 = 36028797018963968u64;
const C_FLOW_DIRECTION: u64 = 72057594037927936u64;
const C_TRANSFORM: u64 = 144115188075855872u64;
'''


def base_block(src):
    """The embedded-block field name for a base, or None for a control's own row."""
    return snake(src) if src in ALL_BASES else None


ALL_BASES = sorted({b for bs in EXTRA_BASES.values() for b in bs})


def emit_base_props(by_type):
    """One struct per shared base, declared once and embedded by each control
    that MAUI would have had inherit it. C+ has no inheritance; composition is
    the workaround, exactly as Data embeds CommonProps."""
    o = []
    for base in ALL_BASES:
        name = pascal(snake(base)) + "Props"
        fields, inits = [], []
        for member, band, fn, note in by_type.get(base, []):
            if band in ("writes", "reads"):
                pr = prop_of(fn, note)
                if pr is None:
                    continue
                field, ty = pr
                fields.append(f"    {field}: {cplus_type(ty)},    // {base}.{member}\n")
                inits.append(f"            {field}: {zero_of(ty)},\n")
            elif band == "events":
                verb = fn.split(" / ")[0].strip()
                if verb.startswith("on_"):
                    fields.append(f"    {verb}: fn(*u8, *u8),    // {base}.{member}\n")
                    fields.append(f"    opaque {verb}_ctx: *u8,\n")
                    inits.append(f"            {verb}: no_handler,\n")
                    inits.append(f"            {verb}_ctx: 0 as *u8,\n")
        if not fields:
            continue
        o.append(f"\n// ---- {base} — shared by every control MAUI derives from it "
                 + "-" * max(0, 14 - len(base)) + "\n\n")
        o.append(f"struct {name} {{\n")
        o.extend(fields)
        o.append("}\n\n")
        o.append(f"impl {name} {{\n    fn new() -> {name} {{\n        return {name} {{\n")
        o.extend(inits)
        o.append("        };\n    }\n}\n")
    return "".join(o)


def emit_props(rows_by_control, by_type):
    out = ["// GENERATED by tools/gen_contract.py — DO NOT EDIT.\n",
           "// Per-control state. Data carries a kind tag plus an owned pointer to one\n",
           "// of these; NOT a tagged union, because an enum payload cannot be moved out\n",
           "// through the raw pointer flex's attachment hands back (E0509), and a union\n",
           "// would size every node by its largest variant.\n\n",
           'import "stdlib/text" as text;\n',
           'import "stdlib/box" as box;\n',
           'import "./vocabulary" as vocab;\n',
           "\nfn no_handler(sender: *u8, ctx: *u8) { return; }\n",
           COMMON_SRC]
    out.append(emit_base_props(by_type))

    for i, (maui, merged) in enumerate(sorted(rows_by_control.items())):
        mod = MODULE[maui]
        name = pascal(mod) + "Props"
        fields, inits = [], []
        for base in EXTRA_BASES.get(maui, []):
            blk = base_block(base)
            bname = pascal(snake(base)) + "Props"
            if any(base_block(s2) == blk for _m, _b, _f, _n, s2 in merged):
                fields.append(f"    {blk}: {bname},    // embedded, not restated\n")
                inits.append(f"            {blk}: {bname}::new(),\n")
        for member, band, fn, note, src in merged:
            if base_block(src) is not None:
                continue                      # lives in the embedded block
            if band in ("writes", "reads"):
                p = prop_of(fn, note)
                if p is None:
                    continue
                field, ty = p
                fields.append(f"    {field}: {cplus_type(ty)},"
                              f"    // {src}.{member}\n")
                inits.append(f"            {field}: {zero_of(ty)},\n")
            elif band == "events":
                verb = fn.split(" / ")[0].strip()
                if not verb.startswith("on_"):
                    continue
                fields.append(f"    {verb}: fn(*u8, *u8),    // {src}.{member}\n")
                fields.append(f"    opaque {verb}_ctx: *u8,\n")
                inits.append(f"            {verb}: no_handler,\n")
                inits.append(f"            {verb}_ctx: 0 as *u8,\n")

        out.append(f"\n// ---- {mod} — MAUI {maui} " + "-" * max(0, 46 - len(mod) - len(maui)) + "\n\n")
        out.append(f"struct {name} {{\n")
        out.extend(fields if fields else ["    _unused: bool,\n"])
        out.append("}\n\n")
        out.append(f"impl {name} {{\n    fn new() -> {name} {{\n        return {name} {{\n")
        out.extend(inits if inits else ["            _unused: false,\n"])
        out.append("        };\n    }\n}\n\n")
        out.append(f"const K_{mod.upper()}: u32 = {i + 1}u32;\n\n")
        out.append(f"fn release_{mod}_props(p: *u8) {{\n"
                   f"    if p == (0 as *u8) {{ return; }}\n"
                   f"    let _b: box::Box[{name}] = box::from_raw::[{name}](p);\n"
                   f"    return;\n}}\n")
    return "".join(out)


# The shared band forwards, generated onto every cursor. Kept here so the
# per-control emitters stay a single loop.
SHARED_FORWARDS = [
    ("set_opacity", "v: f64", "core::set_opacity(this._p, v)", None),
    ("opacity", "", "return core::opacity(this._p)", "f64"),
    ("set_enabled", "v: bool", "core::set_enabled(this._p, v)", None),
    ("is_enabled", "", "return core::is_enabled(this._p)", "bool"),
    ("set_visible", "v: bool", "core::set_visible(this._p, v)", None),
    ("is_visible", "", "return core::is_visible(this._p)", "bool"),
    ("set_background_color", "v: vocab::Color", "core::set_background_color(this._p, v)", None),
    ("background_color", "", "return core::background_color(this._p)", "vocab::Color"),
    ("set_rotation", "v: f64", "core::set_rotation(this._p, v)", None),
    ("set_scale", "v: f64", "core::set_scale(this._p, v)", None),
    ("width", "", "return core::width_of(this._p)", "f64"),
    ("height", "", "return core::height_of(this._p)", "f64"),
    ("x", "", "return core::x_of(this._p)", "f64"),
    ("y", "", "return core::y_of(this._p)", "f64"),
    ("child_count", "", "return core::child_count_of(this._p)", "usize"),
]


def emit_control(maui, merged):
    mod = MODULE[maui]
    cur = pascal(mod)
    props = cur + "Props"
    up = mod.upper()

    writes, reads, events = [], [], []
    for member, band, fn, note, src in merged:
        if band in ("writes", "reads"):
            p = prop_of(fn, note)
            if p is None:
                continue
            blk = base_block(src)
            path = (blk + "." + p[0]) if blk else p[0]
            (writes if band == "writes" else reads).append((p[0], p[1], member, src, path))
        elif band == "events":
            verb = fn.split(" / ")[0].strip()
            if verb.startswith("on_"):
                blk2 = base_block(src)
                events.append((verb, member, src, (blk2 + "." + verb) if blk2 else verb))

    o = [f"// GENERATED by tools/gen_contract.py — DO NOT EDIT.\n",
         f"// {mod} — MAUI {maui}. {len(writes)} writes, {len(reads)} reads, "
         f"{len(events)} events.\n",
         "//\n",
         "// Importing this module is what makes these verbs exist; without it a call\n",
         "// is E0324, not a silent no-op.\n\n",
         'import "flex_layout/flex_layout" as flex;\n',
         'import "stdlib/text" as text;\n',
         'import "stdlib/option" as option;\n',
         'import "stdlib/box" as box;\n',
         'import "./facet" as core;\n',
         'import "./props" as props;\n',
         'import "./vocabulary" as vocab;\n\n']

    o.append("// ---- dirty bits --------------------------------------------------------\n\n")
    for i, (field, ty, member, src, path) in enumerate(writes):
        o.append(f"const P_{field.upper()}: u64 = {1 << i}u64;\n")
    o.append("\n")

    # ---- constructor
    o.append("// ---- build -------------------------------------------------------------\n")
    o.append("// Typed by the parameter list: a verb another control owns is not a\n")
    o.append("// parameter here, so passing it is a compile error.\n\n")
    o.append(f"fn {mod}(\n")
    primary = PRIMARY.get(maui)
    ordered = writes
    if primary is not None and any(f == primary for f, _t, _m, _s, _p in writes):
        ordered = ([w for w in writes if w[0] == primary]
                   + [w for w in writes if w[0] != primary])
        f0, t0, _m0, _s0, _p0 = ordered[0]
        p0 = "str" if t0 == "str" else cplus_type(t0)
        o.append(f"    {verb_stem(f0, t0, {f for f, _t, _m, _s, _p in writes + reads})}: {p0},\n")
        ordered = ordered[1:]
    o.append("    key: str = \"\",\n")
    taken = {f for f, _t, _m, _s, _p in writes + reads}
    for field, ty, member, src, path in ordered:
        pty = "str" if ty == "str" else cplus_type(ty)
        nm = verb_stem(field, ty, taken)
        o.append(f"    {nm}: {pty} = {'\"\"' if ty == 'str' else zero_of(ty)},\n")
    for verb, member, src, path in events:
        o.append(f"    {verb}: fn(*u8, *u8) = props::no_handler,\n")
        o.append(f"    {verb}_ctx: *u8 = 0 as *u8,\n")
    o.append(") -> core::Node {\n")
    o.append(f"    var p: props::{props} = props::{props}::new();\n")
    _ = ordered
    for field, ty, member, src, path in writes:
        nm = verb_stem(field, ty, taken)
        rhs = f"text::from_str({nm})" if ty == "str" else nm
        o.append(f"    p.{path} = {rhs};\n")
    for verb, member, src, path in events:
        o.append(f"    p.{path} = {verb};\n")
        o.append(f"    p.{path}_ctx = {verb}_ctx;\n")
    o.append(f"    return match box::new::[props::{props}](p) {{\n")
    o.append(f"        option::Option[box::Box[props::{props}]]::Some(b) =>\n")
    o.append(f"            core::node_with(key, props::K_{up}, b.into_raw(), props::release_{mod}_props),\n")
    o.append(f"        option::Option[box::Box[props::{props}]]::None =>\n")
    o.append(f"            core::node_with(key, props::K_{up}, 0 as *u8, props::release_{mod}_props),\n")
    o.append("    };\n}\n\n")

    # ---- cursor
    o.append("// ---- live --------------------------------------------------------------\n")
    o.append("// The typed handle `find` hands back. Copy, non-owning. `_seen` is flex's\n")
    o.append("// removal_count() at lookup: while it matches, no node anywhere has been\n")
    o.append("// removed, so `_p` is provably live without dereferencing it.\n\n")
    o.append(f"struct {cur} {{\n    opaque _p: *core::Node,\n    _seen: u64,\n}}\n\n")
    o.append(f"impl {cur} {{\n")
    o.append(f"    fn of(p: *core::Node) -> {cur} {{ return {cur} {{ _p: p, _seen: flex::removal_count() }}; }}\n")
    o.append("    fn is_live(this) -> bool { return this._seen == flex::removal_count(); }\n\n")
    o.append(f"    fn _props(this) -> *props::{props} {{\n")
    o.append("        let d: *core::Data = { (*this._p).data() };\n")
    o.append(f"        if d == (0 as *core::Data) {{ return 0 as *props::{props}; }}\n")
    o.append(f"        if {{ (*d).kind }} != props::K_{up} {{ return 0 as *props::{props}; }}\n")
    o.append(f"        return {{ (*d).props }} as *props::{props};\n    }}\n")

    for field, ty, member, src, path in writes:
        pty = "str" if ty == "str" else cplus_type(ty)
        rhs = f"text::from_str(v)" if ty == "str" else "v"
        o.append(f"\n    // {src}.{member}\n")
        o.append(f"    fn set_{verb_stem(field, ty, taken)}(this, v: {pty}) -> {cur} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return this; }}\n")
        o.append(f"        {{ (*p).{path} = {rhs} }};\n")
        o.append(f"        core::touch(this._p, P_{field.upper()});\n")
        o.append("        return this;\n    }\n")

    for field, ty, member, src, path in writes + reads:
        rty = "str" if ty == "str" else cplus_type(ty)
        body = f"{{ (*p).{path}.view() }}" if ty == "str" else f"{{ (*p).{path} }}"
        dflt = '""' if ty == "str" else zero_of(ty)
        o.append(f"\n    fn {field}(this) -> {rty} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return {dflt}; }}\n")
        o.append(f"        return {body};\n    }}\n")

    o.append("\n    // --- the shared band: one-line forwards, no generic element type ---\n")
    for name, arg, body, ret in SHARED_FORWARDS:
        sig = f"fn {name}(this{', ' + arg if arg else ''})"
        if ret is None:
            o.append(f"    {sig} -> {cur} {{ {body}; return this; }}\n")
        else:
            o.append(f"    {sig} -> {ret} {{ {body}; }}\n")
    o.append("    fn frame(this) -> core::Frame { return core::frame_of(this._p); }\n")
    o.append("}\n\n")

    # ---- lookup
    o.append("// ---- lookup: resolve the key, then check the kind -----------------------\n\n")
    o.append(f"fn find(root: *core::Node, key: str) -> option::Option[{cur}] {{\n")
    o.append("    return match core::find_in(root, key) {\n")
    o.append("        option::Option[*core::Node]::Some(p) => {\n")
    o.append("            let d: *core::Data = { (*p).data() };\n")
    o.append(f"            if d == (0 as *core::Data) {{ return option::Option[{cur}]::None; }}\n")
    o.append(f"            if {{ (*d).kind }} != props::K_{up} {{ return option::Option[{cur}]::None; }}\n")
    o.append(f"            option::Option[{cur}]::Some({cur}::of(p))\n")
    o.append("        }\n")
    o.append(f"        option::Option[*core::Node]::None => option::Option[{cur}]::None,\n")
    o.append("    };\n}\n")
    return "".join(o)


def emit_manifest(rows_by_control):
    o = ["# facet contract manifest — GENERATED by tools/gen_contract.py\n\n",
         "Every declared word, its type, and the MAUI row it came from. A verb\n",
         "absent here does not exist: calling it is a compile error, never a\n",
         "silent no-op. What a backend cannot implement is recorded in that\n",
         "backend's own manifest, not here.\n\n"]
    total = 0
    for maui, merged in sorted(rows_by_control.items(), key=lambda kv: MODULE[kv[0]]):
        mod = MODULE[maui]
        o.append(f"\n## {mod} — MAUI {maui}\n\n")
        o.append("| verb | type | provenance |\n|---|---|---|\n")
        for member, band, fn, note, src in merged:
            if band in ("writes", "reads"):
                p = prop_of(fn, note)
                if p is None:
                    continue
                field, ty = p
                verb = f"`set_{field}` / `{field}()`" if band == "writes" else f"`{field}()`"
                o.append(f"| {verb} | {ty} | {src}.{member} |\n")
                total += 1
            elif band == "events":
                verb = fn.split(" / ")[0].strip()
                if verb.startswith("on_"):
                    o.append(f"| `{verb}` | callback + ctx | {src}.{member} |\n")
                    total += 1
    o.insert(5, f"{total} declared verbs over {len(rows_by_control)} controls, plus the\n"
                "shared band every element carries.\n\n")
    return "".join(o)


def main():
    global ENUM_BY_FACET
    ENUM_BY_FACET = read_enums()
    rows_by_control, by_type = control_rows()

    os.makedirs(SRC, exist_ok=True)
    os.makedirs(DOCS, exist_ok=True)
    with open(os.path.join(SRC, "vocabulary.cplus"), "w") as f:
        f.write(emit_vocabulary())
    with open(os.path.join(SRC, "props.cplus"), "w") as f:
        f.write(emit_props(rows_by_control, by_type))
    for maui, merged in rows_by_control.items():
        with open(os.path.join(SRC, MODULE[maui] + ".cplus"), "w") as f:
            f.write(emit_control(maui, merged))
    with open(os.path.join(DOCS, "contract.md"), "w") as f:
        f.write(emit_manifest(rows_by_control))

    print(f"{len(ENUM_BY_FACET)} enums, {len(rows_by_control)} controls")
    print(f"wrote vendor/facet/src/{{vocabulary,props}}.cplus + "
          f"{len(rows_by_control)} control modules + docs/contract.md")


if __name__ == "__main__":
    main()
