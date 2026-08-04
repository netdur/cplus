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

  the shared band (the rows every element carries) lives ONCE on Node in the
  hand-written facet.cplus, reached through generated one-line forwards on
  each cursor. There is no generic element type: `find` returns the control,
  with everything it can do and nothing it cannot.

Emission is unconditional: facet declares the whole vocabulary, and a backend
answers it or states that it cannot. A verb absent from a control is absent
because the ledger dropped that row, never because nothing wired it.

`check` is what makes that last sentence true. Four guards run before anything
is written, and each exits non-zero naming what it found:

  1. an ADOPT row reaching a control that nothing carries
  2. a command writing a field its control does not have
  3. a shared-band row in neither SHARED_FORWARDS nor DEFERRED_SHARED
  4. a verb naming_guideline.md rejects

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
    "Drawable": ("vocab::Drawable", "vocab::Drawable::none()", None),
    "Date":  ("vocab::Date", "vocab::Date::zero()", None),
    "Duration": ("vocab::Duration", "vocab::Duration::zero()", None),
    "Window": ("vocab::WindowRef", "vocab::WindowRef::none()", None),
    "Dashes": ("vocab::Dashes", "vocab::Dashes::none()", None),
    "Shortcut": ("vocab::Shortcut", "vocab::Shortcut::none()", None),
}

# ---------------------------------------------------------------------------
# Value types that CARRY a view. `Shortcut` is Copy and holds `key: str` — a
# borrowed {ptr, len}: sound for the literal it usually is, a use-after-free
# for anything computed. Props outlive every caller frame, so props may not
# hold the view; the type is stored DECOMPOSED, one owned field per part.
#
# That is the rule a bare `str` prop already follows — the surface takes a
# view, the storage owns the bytes — applied one level in. The surface does
# not change: the setter still takes the whole value and the getter still
# returns one, so `Shortcut` stays Copy and free to pass around.
#
# facet type -> (parts, how the value is rebuilt from its parts). A part is
# (field suffix, facet type, zero override) and is stored by the same rules
# as a bare prop of that type; the override exists because a zero is not
# always an enum's first member.
CARRIER_TYPES = {
    "Shortcut": (
        [("key", "str", None),
         ("modifiers", "KeyModifiers", "vocab::KeyModifiers::None")],
        "vocab::Shortcut::of({key}, modifiers: {modifiers})",
    ),
}


def store_expr(ty, src):
    """How a value of facet type `ty` is written into owned storage. A view
    is copied; everything else is stored as it arrives."""
    return f"text::from_str({src})" if ty == "str" else src


def read_expr(ty, place):
    """How owned storage of facet type `ty` reads back at the surface."""
    return f"{{ {place}.view() }}" if ty == "str" else f"{{ {place} }}"


def carrier_reads(ty, path):
    """The rebuilt surface value of a carrier prop stored at `path`."""
    parts, rebuild = CARRIER_TYPES[ty]
    return rebuild.format(**{
        suffix: read_expr(pty, f"(*p).{path}_{suffix}")
        for suffix, pty, _z in parts
    })

# ---------------------------------------------------------------------------
# How a facet type is carried. Every ADOPT row reaching a control lands in
# exactly one of these; a row that lands in none fails the run (see `check`).
# Stage 1 exits non-zero on an unbucketed row and an unclassified type — this
# is Stage 2's equivalent, and it did not exist until the iris audit found 31
# rows the generator was discarding in silence.

# A subtree is a CHILD, not a property: a Node-typed row becomes a named slot,
# a child that core::set_slot replaces in place. The name is recorded in the
# child's own Data.slot rather than in its key, so the application's key
# survives and `find` still reaches the node.
# SwipeItems is the same shape one level up — a subtree of swipe_item nodes.
SLOT_TYPES = {"Node", "SwipeItem[]"}

# Owned, not Copy: written by `take`, read through accessors rather than
# returned by value. The value is the element type an accessor hands back.
OWNED_TYPES = {"TextList": "str", "Spans": "*vocab::TextSpan"}

# Carried by the shared band on Node, so a per-control copy would be a second
# way to say the same thing. `children()` is child_count() + child(at:);
# `native()` is the escape hatch, read off Data.
SHARED_TYPES = {"Node[]", "*u8"}

# Rows that reach a control but belong to an owner that is not this control.
# Recorded, not silent: the reason is emitted into the manifest.
NOT_EMITTED = {
    ("Window[]", None): "a window list belongs to the runtime, not to a control",
    ("ToolbarItem[]", None): "Chrome's, not a control's — Stage 3 declares the tier",
    ("MenuBarItem[]", None): "Chrome's, not a control's — Stage 3 declares the tier",
}


def cplus_type(t):
    if t in TYPES:
        return TYPES[t][0]
    if t in OWNED_TYPES:
        return "vocab::" + t
    if t in ENUM_BY_FACET:
        return "vocab::" + t
    return None


def zero_of(t):
    if t in TYPES:
        return TYPES[t][1]
    if t in OWNED_TYPES:
        return "vocab::" + t + "::new()"
    if t in ENUM_BY_FACET:
        return "vocab::" + t + "::" + ENUM_BY_FACET[t][0][0]
    return None


# ---------------------------------------------------------------------------
# Enum members, read out of the two PublicAPI manifests. Four enums have no
# machine-readable source (MAUI models them as classes or they live outside
# both manifests); those are authored here and marked.
HAND_ENUMS = {
    "Appearance": ["Unspecified", "Light", "Dark"],
    # A SCALE (NSFont ultraLight..black, CSS 100..900, the icon font's wght
    # axis 100..700) — not MAUI's Bold|Italic|None, which is why the manifest
    # harvest must not win (HAND_WINS). Default = keep the native weight.
    "FontWeight": ["Default", "UltraLight", "Thin", "Light", "Regular",
                   "Medium", "Semibold", "Bold", "Heavy", "Black"],
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
    # Enums whose facet shape deliberately DIVERGES from the MAUI members —
    # the hand list is the contract, the manifest is only provenance.
    hand_wins = {"FontWeight"}
    for maui, facet in maui_map.ENUMS.items():
        ms = re.findall(rf"^{re.escape(maui)}\.(\w+) = -?\d+ ->", blob, re.M)
        if ms and facet not in out and facet not in hand_wins:
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


# ---------------------------------------------------------------------------
# A method row is a write plus a dirty bit — the same shape as a setter, so the
# backend seam stays the ~37 apply functions and gains no verb-indexed table.
# `extra` fields exist only to carry a command's argument; `writes` may name a
# field the control already has, and `check` fails the run if it does not.
COMMANDS = {
    "scroll_to": {
        "params": [("index", "usize")],
        "extra":  [("scroll_to_index", "i64")],
        "writes": [("scroll_to_index", "index as i64")],
    },
    "begin_refresh": {
        "params": [], "extra": [], "writes": [("is_refreshing", "true")],
    },
    "end_refresh": {
        "params": [], "extra": [], "writes": [("is_refreshing", "false")],
    },
    "animate_progress": {
        "params": [("to", "f64"), ("duration", "Duration")],
        "extra":  [("animate_duration", "Duration")],
        "writes": [("progress", "to"), ("animate_duration", "duration")],
    },
}


def _field_and_type(fn, note):
    verb = fn.split(" / ")[0].split("(")[0].strip()
    field = verb[4:] if verb.startswith("set_") else verb
    field = RESERVED.get(field, field)
    ty = note.split(" — ")[0].replace(" (read-only)", "").strip()
    return verb, field, ty


def row_kind(band, fn, note):
    """How an ADOPT row is carried. Returns (kind, name, detail), or None when
    nothing carries it — which fails the run rather than dropping it."""
    verb, field, ty = _field_and_type(fn, note)
    if band == "methods":
        if verb in COMMANDS:
            return ("command", verb, None)
        if verb in SHARED_VERBS:
            return ("shared", verb, "the shared band declares it once, on Node")
        return None
    if band == "events":
        if verb.startswith("on_"):
            return ("event", verb, None)
        # Continuous observers (observe_scrolled, observe_drag_interaction,
        # ...) are fn+ctx pairs like every other event — they just fire
        # repeatedly. The Subscription-shaped alternative died on decision 5's
        # caveat: events::Subscription carries a *Bus its drop dereferences.
        return ("event", verb, None)
    if ty in SLOT_TYPES:
        return ("slot", field, ty)
    if ty in OWNED_TYPES:
        return ("owned", field, ty)
    if ty in SHARED_TYPES:
        return ("shared", verb, "the shared band declares it once, on Node")
    # MAUI redeclares IsEnabled on MenuItem, BackgroundColor on Span, IsVisible
    # on SwipeItem, because those are Elements and not VisualElements. facet has
    # one Node, so the shared band already answers them and a second copy would
    # be a second way to say the same thing (E0326 besides).
    if field in SHARED_VERBS:
        return ("shared", field,
                "MAUI declares it again off the VisualElement branch; facet has "
                "one Node, so the shared band already carries it")
    if cplus_type(ty) is not None:
        return ("prop", field, ty)
    for (t, _m), why in NOT_EMITTED.items():
        if t == ty:
            return ("skip", field, why)
    return None


def prop_of(fn, note):
    """(field name, facet type) for a row carried as a plain prop, else None."""
    verb, field, ty = _field_and_type(fn, note)
    if ty in TYPES or ty in ENUM_BY_FACET:
        return (field, ty)
    return None


VALUE_TYPES_SRC = '''
// ---- value types the contract carries by name -------------------------------

// A color: a semantic token (mapped to the platform's native color) or explicit
// RGBA. token 0 = none/unset; token 255 = Rgba (use r/g/b/a, 0..1); token 254 =
// an adaptive light/dark pair. Flat + Copy. A color is a NAME (token) or a
// literal, resolved by the backend at paint time. Two tiers of names:
//
//   Tier 1 — the PLATFORM's semantic colors (text tiers, surfaces, fills,
//   selection, separator, accent, the system palette). Pass-through: their
//   job is "look native"; they are never themeable. The escape hatch when
//   the theme roles are not what you mean.
//
//   Tier 2 — THEME roles (primary/secondary brand slots, ink, surface tiers,
//   outline, status). The app retints these with `theme::set_theme`; unset
//   roles fall back to the nearest Tier-1 color, so a themeless app is native
//   in both appearances and a themed app looks the same on every backend.
struct Color {
    token: u32,
    r: f64,
    g: f64,
    b: f64,
    a: f64,
    // The dark side of an `adaptive` pair (token 254); unused otherwise.
    r2: f64,
    g2: f64,
    b2: f64,
    a2: f64,
}

impl Color {
    // A bare token, all channels zero.
    fn tok(t: u32) -> Color {
        return Color {
            token: t,
            r: 0.0f64, g: 0.0f64, b: 0.0f64, a: 0.0f64,
            r2: 0.0f64, g2: 0.0f64, b2: 0.0f64, a2: 0.0f64,
        };
    }

    fn none() -> Color { return Color::tok(0 as u32); }
    // The regen's spelling of the unset color (every generated default).
    fn clear() -> Color { return Color::none(); }

    // ---- Tier 1: platform semantic colors -----------------------------------
    // Text tiers.
    fn text() -> Color { return Color::tok(1 as u32); }
    fn text_secondary() -> Color { return Color::tok(2 as u32); }
    fn text_tertiary() -> Color { return Color::tok(9 as u32); }
    fn placeholder() -> Color { return Color::tok(15 as u32); }
    fn link() -> Color { return Color::tok(16 as u32); }
    // The user's chosen system accent.
    fn accent() -> Color { return Color::tok(3 as u32); }
    // Area tiers.
    fn window_background() -> Color { return Color::tok(11 as u32); }
    fn under_page_background() -> Color { return Color::tok(12 as u32); }
    fn control_background() -> Color { return Color::tok(13 as u32); }
    // Neutral fills.
    fn fill() -> Color { return Color::tok(14 as u32); }
    fn fill_secondary() -> Color { return Color::tok(19 as u32); }
    // Selection.
    fn selected_content_background() -> Color { return Color::tok(17 as u32); }
    fn selected_text_background() -> Color { return Color::tok(18 as u32); }
    fn separator() -> Color { return Color::tok(10 as u32); }
    // The system palette.
    fn system_red() -> Color { return Color::tok(4 as u32); }
    fn system_green() -> Color { return Color::tok(5 as u32); }
    fn system_blue() -> Color { return Color::tok(6 as u32); }
    fn system_orange() -> Color { return Color::tok(7 as u32); }
    fn system_gray() -> Color { return Color::tok(8 as u32); }
    fn system_yellow() -> Color { return Color::tok(20 as u32); }
    fn system_purple() -> Color { return Color::tok(21 as u32); }
    fn system_pink() -> Color { return Color::tok(22 as u32); }
    fn system_teal() -> Color { return Color::tok(23 as u32); }
    fn system_indigo() -> Color { return Color::tok(24 as u32); }

    // ---- literals + pairs ---------------------------------------------------
    fn rgba(r: f64, g: f64, b: f64, a: f64 = 1.0f64) -> Color {
        return Color {
            token: 255 as u32,
            r: r, g: g, b: b, a: a,
            r2: 0.0f64, g2: 0.0f64, b2: 0.0f64, a2: 0.0f64,
        };
    }

    // A light/dark rgba pair in one Color, resolved by the CURRENT appearance
    // at paint time (and re-resolved live on an appearance flip). Pass rgba
    // colors for both sides.
    fn adaptive(light: Color, dark: Color) -> Color {
        return Color {
            token: 254 as u32,
            r: light.r, g: light.g, b: light.b, a: light.a,
            r2: dark.r, g2: dark.g, b2: dark.b, a2: dark.a,
        };
    }

    // ---- Tier 2: theme roles ------------------------------------------------
    // The brand slot: buttons, selection marks, links, badges.
    fn primary() -> Color { return Color::tok(100 as u32); }
    fn on_primary() -> Color { return Color::tok(101 as u32); }
    // The second brand slot.
    fn secondary() -> Color { return Color::tok(102 as u32); }
    fn on_secondary() -> Color { return Color::tok(103 as u32); }
    // The mark base (text / glyph / hairline / translucent fill) at `a` —
    // the same alpha reads identically over both appearances' surfaces.
    fn ink(a: f64 = 1.0f64) -> Color {
        var c: Color = Color::tok(104 as u32);
        c.a = a;
        return c;
    }
    // Surface tiers: the floor, raised chrome (panel / card), and recessed
    // wells.
    fn surface() -> Color { return Color::tok(105 as u32); }
    fn raised() -> Color { return Color::tok(106 as u32); }
    fn sunken() -> Color { return Color::tok(107 as u32); }
    // Extended surface tiers — what IDE-grade chrome needs beyond the three
    // base tiers, each independently retintable by set_theme:
    fn content() -> Color { return Color::tok(112 as u32); }    // editor / document body
    fn toolbar() -> Color { return Color::tok(113 as u32); }    // the chrome row above content
    fn tabstrip() -> Color { return Color::tok(114 as u32); }   // a tab row / strip behind tabs
    fn track() -> Color { return Color::tok(115 as u32); }      // a segmented / switcher track
    fn chip() -> Color { return Color::tok(116 as u32); }       // a discrete raised control chip
    fn recessed() -> Color { return Color::tok(117 as u32); }   // a translucent recessed well
    // Separators and borders.
    fn outline() -> Color { return Color::tok(108 as u32); }
    // Status.
    fn success() -> Color { return Color::tok(109 as u32); }
    fn warning() -> Color { return Color::tok(110 as u32); }
    fn danger() -> Color { return Color::tok(111 as u32); }

    fn is_set(this) -> bool { return this.token != (0 as u32); }
    // Resolution depends on the theme or the appearance (any named token or
    // pair — everything except unset and a fixed rgba literal). What the
    // backend's repaint registry keys on.
    fn is_dynamic(this) -> bool {
        return this.token != (0 as u32) && this.token != (255 as u32);
    }
    fn is_adaptive(this) -> bool { return this.token == (254 as u32); }
    fn is_theme_role(this) -> bool {
        return this.token >= (100 as u32) && this.token <= (117 as u32);
    }
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

// ---- styled text ------------------------------------------------------------
// One run of text with its own styling. MAUI's FormattedString is a list of
// Spans; the list existed in the ledger and the builder that fills it did not,
// so `set_formatted_text` was a verb nothing could feed.
struct TextSpan {
    text: text::Text,
    font_size: f64,
    font_family: text::Text,
    font_weight: FontWeight,
    is_italic: bool,
    color: Color,
    background_color: Color,
    decoration: TextDecoration,
    // A destination. Non-empty makes the run a link.
    link: text::Text,
}

impl TextSpan {
    fn of(text: str,
          font_size: f64 = 0.0f64,
          font_family: str = "",
          font_weight: FontWeight = FontWeight::Default,
          italic: bool = false,
          color: Color = Color::clear(),
          background_color: Color = Color::clear(),
          decoration: TextDecoration = TextDecoration::None,
          link: str = "") -> TextSpan {
        return TextSpan {
            text: text::from_str(text), font_size: font_size,
            font_family: text::from_str(font_family), font_weight: font_weight,
            is_italic: italic,
            color: color, background_color: background_color,
            decoration: decoration, link: text::from_str(link),
        };
    }
    fn view(this) -> str { return this.text.view(); }
    fn is_link(this) -> bool { return this.link.view() != ""; }
}

// Styled runs over a text surface. Owned: written by `take`, read one run at
// a time, because `-> ref T` does not parse.
struct Spans { _runs: vec::Vec[TextSpan] }
impl Spans {
    fn new() -> Spans { return Spans { _runs: vec::new::[TextSpan]() }; }
    fn add(ref this, take run: TextSpan) -> status::Status { return this._runs.append(run); }
    // The common case: plain text with no styling of its own.
    fn add_text(ref this, s: str) -> status::Status { return this._runs.append(TextSpan::of(s)); }
    fn count(this) -> usize { return this._runs.count(); }
    fn at(this, index: usize) -> *TextSpan {
        return match this._runs.at_ptr(index) {
            option::Option[*TextSpan]::Some(p) => p,
            option::Option[*TextSpan]::None => 0 as *TextSpan,
        };
    }
    fn is_empty(this) -> bool { return this._runs.count() == (0 as usize); }
}

// A draw callback for a canvas: (ctx, width, height).
struct Drawable { draw: fn(*u8, f64, f64), opaque ctx: *u8 }
fn draw_none(ctx: *u8, w: f64, h: f64) { return; }
impl Drawable { fn none() -> Drawable { return Drawable { draw: draw_none, ctx: 0 as *u8 }; } }

// A stroke dash pattern: on/off run lengths. Fixed at eight, which is more
// than any dash any platform draws, so the value stays Copy and a Border's
// props stay free of an allocation.
const DASH_SLOTS: usize = 8 as usize;
struct Dashes { count: usize, runs: [f64; 8] }
impl Dashes {
    fn none() -> Dashes { return Dashes { count: 0 as usize, runs: [0.0f64; 8] }; }
    fn of(on: f64, off: f64) -> Dashes {
        var d: Dashes = Dashes::none();
        d.runs[0 as usize] = on;
        d.runs[1 as usize] = off;
        d.count = 2 as usize;
        return d;
    }
    fn run(this, at: usize) -> f64 {
        if at >= this.count { return 0.0f64; }
        return this.runs[at];
    }
    fn is_solid(this) -> bool { return this.count == (0 as usize); }
}

// A keyboard shortcut. MAUI models a list; a menu item shows one.
struct Shortcut { key: str, modifiers: KeyModifiers }
impl Shortcut {
    fn none() -> Shortcut { return Shortcut { key: "", modifiers: KeyModifiers::None }; }
    fn of(key: str, modifiers: KeyModifiers = KeyModifiers::None) -> Shortcut {
        return Shortcut { key: key, modifiers: modifiers };
    }
    fn is_set(this) -> bool { return this.key != ""; }
}

// An owned list of strings — a popup's items. Not Copy: written by `take`,
// read one element at a time, because `-> ref T` does not parse.
struct TextList { _v: vec::Vec[text::Text] }
impl TextList {
    fn new() -> TextList { return TextList { _v: vec::new::[text::Text]() }; }
    fn add(ref this, s: str) -> status::Status { return this._v.append(text::from_str(s)); }
    fn count(this) -> usize { return this._v.count(); }
    fn at(this, index: usize) -> str {
        return match this._v.at_ptr(index) {
            option::Option[*text::Text]::Some(t) => { (*t).view() },
            option::Option[*text::Text]::None => "",
        };
    }
}

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
           "// per naming_guideline.md; members come from the PublicAPI manifests.\n\n",
           'import "stdlib/text" as text;\n',
           'import "stdlib/vec" as vec;\n',
           'import "stdlib/option" as option;\n',
           'import "stdlib/status" as status;\n\n']
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
    // The shared-band handlers (Stage 3): focus/blur fired by the backend,
    // attach/detach fired by the mount walk (post-walk, M4). fn + ctx pairs,
    // the same shape as every control event.
    on_focus: fn(*u8, *u8), opaque on_focus_ctx: *u8,
    on_blur: fn(*u8, *u8), opaque on_blur_ctx: *u8,
    on_attach: fn(*u8, *u8), opaque on_attach_ctx: *u8,
    on_detach: fn(*u8, *u8), opaque on_detach_ctx: *u8,
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
            on_focus: no_handler, on_focus_ctx: 0 as *u8,
            on_blur: no_handler, on_blur_ctx: 0 as *u8,
            on_attach: no_handler, on_attach_ctx: 0 as *u8,
            on_detach: no_handler, on_detach_ctx: 0 as *u8,
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
// Commands, not state: the backend acts on the bit and clears it.
const C_FOCUS: u64 = 288230376151711744u64;
const C_BLUR: u64 = 576460752303423488u64;
const C_FLUSH: u64 = 1152921504606846976u64;
// One bit for the four shared-band handler pairs — a live handler swap is
// rare enough that the backend re-reads all four.
const C_HANDLERS: u64 = 2305843009213693952u64;
'''


def base_block(src):
    """The embedded-block field name for a base, or None for a control's own row."""
    return snake(src) if src in ALL_BASES else None


ALL_BASES = sorted({b for bs in EXTRA_BASES.values() for b in bs})


# facet's own words that are props on a control that already exists. The rest
# of maui_map.FACET_ORIGIN names controls Stage 3 declares; the ledger records
# every one of them either way.
#
# MAUI binds ItemsSource and realizes rows through a DataTemplate; Stage 1
# dropped both as MODEL. The imperative replacement is a row count plus a row
# builder, and it is what makes a list fillable at all.
ROW_SOURCE = ("list", "collection")

ROW_SOURCE_FIELDS = [
    ("count", "i64", "0 as i64", "facet — how many rows the control shows"),
    ("row", "fn(*u8, usize) -> flex::Node", "no_row", "facet — builds row `i`"),
    ("opaque row_ctx", "*u8", "0 as *u8", None),
]


def _fields_for(rows):
    """(field lines, init lines) for the prop/owned/event/command rows of one
    struct. Each row carries its own provenance, so the comment is per-field."""
    fields, inits = [], []
    for kind, name, detail, member, band, src in rows:
        if kind in ("prop", "owned") and detail in CARRIER_TYPES:
            # Stored decomposed: the whole value carries a view, and props
            # outlive every frame that could have supplied one.
            parts, _rebuild = CARRIER_TYPES[detail]
            for suffix, pty, zero in parts:
                fields.append(f"    {name}_{suffix}: {cplus_type(pty)},"
                              f"    // {src}.{member}, {suffix}\n")
                inits.append(f"            {name}_{suffix}: {zero or zero_of(pty)},\n")
        elif kind in ("prop", "owned"):
            fields.append(f"    {name}: {cplus_type(detail)},    // {src}.{member}\n")
            inits.append(f"            {name}: {zero_of(detail)},\n")
        elif kind == "event":
            fields.append(f"    {name}: fn(*u8, *u8),    // {src}.{member}\n")
            fields.append(f"    opaque {name}_ctx: *u8,\n")
            inits.append(f"            {name}: no_handler,\n")
            inits.append(f"            {name}_ctx: 0 as *u8,\n")
        elif kind == "command":
            for f, t in COMMANDS[name]["extra"]:
                fields.append(f"    {f}: {cplus_type(t)},    // {src}.{member}, "
                              f"the argument of {name}()\n")
                inits.append(f"            {f}: {zero_of(t)},\n")
    return fields, inits


def emit_base_props(by_type):
    """One struct per shared base, declared once and embedded by each control
    that MAUI would have had inherit it. C+ has no inheritance; composition is
    the workaround, exactly as Data embeds CommonProps."""
    o = []
    for base in ALL_BASES:
        name = pascal(snake(base)) + "Props"
        rows = [(k[0], k[1], k[2], member, band, base)
                for member, band, fn, note in by_type.get(base, [])
                for k in [row_kind(band, fn, note)] if k is not None]
        fields, inits = _fields_for(rows)
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
           'import "flex_layout/flex_layout" as flex;\n',
           'import "stdlib/text" as text;\n',
           'import "stdlib/box" as box;\n',
           'import "./vocabulary" as vocab;\n',
           "\nfn no_handler(sender: *u8, ctx: *u8) { return; }\n",
           "\n// The row builder a list carries until the application sets one.\n"
           "fn no_row(ctx: *u8, index: usize) -> flex::Node { return flex::Node::new(); }\n",
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
        own = [(k[0], k[1], k[2], member, band, src)
               for member, band, fn, note, src in merged
               if base_block(src) is None
               for k in [row_kind(band, fn, note)] if k is not None]
        f2, i2 = _fields_for(own)
        fields += f2
        inits += i2
        if mod in ROW_SOURCE:
            for f, t, zero, why in ROW_SOURCE_FIELDS:
                fields.append(f"    {f}: {t},"
                              + (f"    // {why}\n" if why else "\n"))
                inits.append(f"            {f.replace('opaque ', '')}: {zero},\n")

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


# ---------------------------------------------------------------------------
# The shared band: the 41 ADOPT rows on MAUI's VisualElement/View, which every
# element carries. They live ONCE on Node in the hand-written facet.cplus and
# reach each cursor through the one-line forwards below, so no generic element
# type exists anywhere in the package.
#
# (facet verb, params, body, return type) — a None return type means the
# forward returns the cursor, so writes chain.
SHARED_FORWARDS = [
    # -- paint
    ("set_opacity", "v: f64", "core::set_opacity(this._p, v)", None),
    ("opacity", "", "return core::opacity(this._p)", "f64"),
    ("set_background_color", "v: vocab::Color", "core::set_background_color(this._p, v)", None),
    ("background_color", "", "return core::background_color(this._p)", "vocab::Color"),
    ("set_background", "v: vocab::Brush", "core::set_background(this._p, v)", None),
    ("background", "", "return core::background(this._p)", "vocab::Brush"),
    ("set_shadow", "v: vocab::Shadow", "core::set_shadow(this._p, v)", None),
    ("shadow", "", "return core::shadow(this._p)", "vocab::Shadow"),
    ("set_clip", "v: vocab::Shape", "core::set_clip(this._p, v)", None),
    ("clip", "", "return core::clip(this._p)", "vocab::Shape"),
    # -- state
    ("set_enabled", "v: bool", "core::set_enabled(this._p, v)", None),
    ("is_enabled", "", "return core::is_enabled(this._p)", "bool"),
    ("set_visible", "v: bool", "core::set_visible(this._p, v)", None),
    ("is_visible", "", "return core::is_visible(this._p)", "bool"),
    ("set_input_transparent", "v: bool", "core::set_input_transparent(this._p, v)", None),
    ("input_transparent", "", "return core::input_transparent(this._p)", "bool"),
    ("set_flow_direction", "v: vocab::FlowDirection", "core::set_flow_direction(this._p, v)", None),
    ("flow_direction", "", "return core::flow_direction(this._p)", "vocab::FlowDirection"),
    # -- the transform band, whole
    ("set_rotation", "v: f64", "core::set_rotation(this._p, v)", None),
    ("rotation", "", "return core::rotation(this._p)", "f64"),
    ("set_rotation_x", "v: f64", "core::set_rotation_x(this._p, v)", None),
    ("rotation_x", "", "return core::rotation_x(this._p)", "f64"),
    ("set_rotation_y", "v: f64", "core::set_rotation_y(this._p, v)", None),
    ("rotation_y", "", "return core::rotation_y(this._p)", "f64"),
    ("set_scale", "v: f64", "core::set_scale(this._p, v)", None),
    ("scale", "", "return core::scale(this._p)", "f64"),
    ("set_scale_x", "v: f64", "core::set_scale_x(this._p, v)", None),
    ("scale_x", "", "return core::scale_x(this._p)", "f64"),
    ("set_scale_y", "v: f64", "core::set_scale_y(this._p, v)", None),
    ("scale_y", "", "return core::scale_y(this._p)", "f64"),
    ("set_translation_x", "v: f64", "core::set_translation_x(this._p, v)", None),
    ("translation_x", "", "return core::translation_x(this._p)", "f64"),
    ("set_translation_y", "v: f64", "core::set_translation_y(this._p, v)", None),
    ("translation_y", "", "return core::translation_y(this._p)", "f64"),
    ("set_anchor_x", "v: f64", "core::set_anchor_x(this._p, v)", None),
    ("anchor_x", "", "return core::anchor_x(this._p)", "f64"),
    ("set_anchor_y", "v: f64", "core::set_anchor_y(this._p, v)", None),
    ("anchor_y", "", "return core::anchor_y(this._p)", "f64"),
    # -- z order is flex's style field; facet forwards rather than copies
    ("set_z_index", "v: i64", "core::set_z_index(this._p, v)", None),
    ("z_index", "", "return core::z_index(this._p)", "i64"),
    # -- geometry, read from flex and stored nowhere
    ("width", "", "return core::width_of(this._p)", "f64"),
    ("height", "", "return core::height_of(this._p)", "f64"),
    ("x", "", "return core::x_of(this._p)", "f64"),
    ("y", "", "return core::y_of(this._p)", "f64"),
    ("bounds", "", "return core::bounds_of(this._p)", "vocab::Rect"),
    # -- what the platform learns first, written back through core::set_*
    ("is_focused", "", "return core::is_focused(this._p)", "bool"),
    ("is_attached", "", "return core::is_attached(this._p)", "bool"),
    ("window", "", "return core::window_of(this._p)", "vocab::WindowRef"),
    # -- the escape hatch INTENT blesses: the platform view, or 0 unmounted
    ("native", "", "return core::native(this._p)", "*u8"),
    # -- children
    ("child_count", "", "return core::child_count_of(this._p)", "usize"),
    ("child", "at: usize", "return core::child_of(this._p, at)", "option::Option[*core::Node]"),
    # -- live layout: flex's own fluent modifier names, reachable after mount.
    #    NOT a second vocabulary — the same words. `node()` reaches the rest.
    ("set_grow", "v: f64", "core::set_grow(this._p, v)", None),
    ("set_shrink", "v: f64", "core::set_shrink(this._p, v)", None),
    ("set_width", "v: f64", "core::set_width(this._p, v)", None),
    ("set_height", "v: f64", "core::set_height(this._p, v)", None),
    ("set_width_percent", "v: f64", "core::set_width_percent(this._p, v)", None),
    ("set_height_percent", "v: f64", "core::set_height_percent(this._p, v)", None),
    ("set_padding", "v: f64", "core::set_padding(this._p, v)", None),
    ("set_margin", "v: f64", "core::set_margin(this._p, v)", None),
    ("set_gap", "v: f64", "core::set_gap(this._p, v)", None),
    ("set_justify", "v: flex::Justify", "core::set_justify(this._p, v)", None),
    ("set_align", "v: flex::Align", "core::set_align(this._p, v)", None),
    ("set_wrap", "v: flex::Wrap", "core::set_wrap(this._p, v)", None),
    # -- commands
    ("focus", "", "core::focus(this._p)", None),
    ("blur", "", "core::blur(this._p)", None),
    ("relayout", "", "core::relayout(this._p)", None),
    ("measure", "width: f64, height: f64", "return core::measure(this._p, width, height)", "vocab::Size"),
    ("begin_updates", "", "core::begin_updates(this._p)", None),
    ("end_updates", "", "core::end_updates(this._p)", None),
    ("on_focus", "f: fn(*u8, *u8), ctx: *u8 = 0 as *u8",
     "core::set_on_focus(this._p, f, ctx)", None),
    ("on_blur", "f: fn(*u8, *u8), ctx: *u8 = 0 as *u8",
     "core::set_on_blur(this._p, f, ctx)", None),
    ("on_attach", "f: fn(*u8, *u8), ctx: *u8 = 0 as *u8",
     "core::set_on_attach(this._p, f, ctx)", None),
    ("on_detach", "f: fn(*u8, *u8), ctx: *u8 = 0 as *u8",
     "core::set_on_detach(this._p, f, ctx)", None),
]

# The ledger verbs the forwards above answer. `set_x`/`x()` pairs collapse to
# one row, so this is matched against the ledger's `set_foo / foo()` form.
SHARED_VERBS = {n[4:] if n.startswith("set_") else n for n, _a, _b, _r in SHARED_FORWARDS}
SHARED_VERBS |= {"children", "native", "measure"}

# The shared-band rows NOT forwarded, each with the reason. `check` fails the
# run on a row that is in neither table, so the band cannot quietly shrink
# again the way it did between Stage 2 and the iris audit.
DEFERRED_SHARED = {
    "observe_size": "answered in services.cplus — observe_size(n, cb, ctx:) -> "
                    "Cancellable, the backend-hook shape the whole tier uses "
                    "(events::Subscription was rejected: it carries a *Bus)",
}


def split_rows(merged):
    """merged ledger rows -> (writes, reads, events, slots, owned, commands)."""
    writes, reads, events, slots, owned, commands = [], [], [], [], [], []
    for member, band, fn, note, src in merged:
        k = row_kind(band, fn, note)
        if k is None:
            continue                          # `check` already failed the run
        kind, name, detail = k
        blk = base_block(src)
        path = (blk + "." + name) if blk else name
        if kind == "prop":
            (writes if band == "writes" else reads).append((name, detail, member, src, path))
        elif kind == "event":
            events.append((name, member, src, path))
        elif kind == "slot":
            slots.append((name, detail, member, src))
        elif kind == "owned":
            owned.append((name, detail, member, src, path))
        elif kind == "command":
            commands.append((name, member, src, blk))
    return writes, reads, events, slots, owned, commands


def ctor_params(maui, writes, reads, events):
    """The constructor's ordered parameter list: (name, type, default|None).

    One authority for the signature — `emit_control` prints it and
    `emit_elements` forwards it, so the two can never drift.
    """
    taken = {f for f, _t, _m, _s, _p in writes + reads}
    params = []
    ordered = writes
    primary = PRIMARY.get(maui)
    if primary is not None and any(f == primary for f, _t, _m, _s, _p in writes):
        ordered = ([w for w in writes if w[0] == primary]
                   + [w for w in writes if w[0] != primary])
        f0, t0, _m0, _s0, _p0 = ordered[0]
        p0 = "str" if t0 == "str" else cplus_type(t0)
        params.append((verb_stem(f0, t0, taken), p0, None))
        ordered = ordered[1:]
    params.append(("key", "str", '""'))
    for field, ty, _m, _s, _p in ordered:
        pty = "str" if ty == "str" else cplus_type(ty)
        params.append((verb_stem(field, ty, taken), pty,
                       '""' if ty == "str" else zero_of(ty)))
    for verb, _m, _s, _p in events:
        params.append((verb, "fn(*u8, *u8)", "props::no_handler"))
        params.append((verb + "_ctx", "*u8", "0 as *u8"))
    return params


def emit_control(maui, merged):
    mod = MODULE[maui]
    cur = pascal(mod)
    props = cur + "Props"
    up = mod.upper()

    writes, reads, events, slots, owned, commands = split_rows(merged)

    # Where each field lives: on the control, or inside an embedded base block.
    # A command writes fields by name, so it resolves through this.
    paths = {f: p for f, _t, _m, _s, p in writes + reads + owned}
    for verb, member, src, blk in commands:
        for f, _t in COMMANDS[verb]["extra"]:
            paths[f] = (blk + "." + f) if blk else f

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
         'import "./vocabulary" as vocab;\n',
         'import "./mount" as mount;\n\n']

    o.append("// ---- dirty bits --------------------------------------------------------\n")
    o.append("// One bit per write, then one per owned prop and one per command. The\n")
    o.append("// top 16 bits of the word are the shared band's (props::C_*).\n\n")
    bits = ([f.upper() for f, _t, _m, _s, _p in writes]
            + [f.upper() for f, _t, _m, _s, _p in owned]
            + [v.upper() for v, _m, _s, _b in commands]
            + (["COUNT", "ROW"] if mod in ROW_SOURCE else []))
    for i, b in enumerate(bits):
        o.append(f"const P_{b}: u64 = {1 << i}u64;\n")
    o.append("\n")

    # ---- constructor
    o.append("// ---- build -------------------------------------------------------------\n")
    o.append("// Typed by the parameter list: a verb another control owns is not a\n")
    o.append("// parameter here, so passing it is a compile error.\n\n")
    o.append(f"fn {mod}(\n")
    taken = {f for f, _t, _m, _s, _p in writes + reads}
    for nm, pty, dflt in ctor_params(maui, writes, reads, events):
        o.append(f"    {nm}: {pty},\n" if dflt is None else f"    {nm}: {pty} = {dflt},\n")
    o.append(") -> core::Node {\n")
    o.append(f"    var p: props::{props} = props::{props}::new();\n")
    for field, ty, member, src, path in writes:
        nm = verb_stem(field, ty, taken)
        if ty in CARRIER_TYPES:
            for suffix, pty, _z in CARRIER_TYPES[ty][0]:
                o.append(f"    p.{path}_{suffix} = {store_expr(pty, f'{nm}.{suffix}')};\n")
            continue
        rhs = store_expr(ty, nm)
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
    o.append(f"    // Unchecked, and module-private for that reason: it trusts the\n")
    o.append(f"    // caller about the kind. `from` is the public door.\n")
    o.append(f"    fn _of(p: *core::Node) -> {cur} {{ return {cur} {{ _p: p, _seen: flex::removal_count() }}; }}\n")

    o.append("    fn is_live(this) -> bool { return this._seen == flex::removal_count(); }\n\n")
    o.append(f"    fn _props(this) -> *props::{props} {{\n")
    o.append("        let d: *core::Data = { (*this._p).data() };\n")
    o.append(f"        if d == (0 as *core::Data) {{ return 0 as *props::{props}; }}\n")
    o.append(f"        if {{ (*d).kind }} != props::K_{up} {{ return 0 as *props::{props}; }}\n")
    o.append(f"        return {{ (*d).props }} as *props::{props};\n    }}\n")

    for field, ty, member, src, path in writes:
        pty = "str" if ty == "str" else cplus_type(ty)
        o.append(f"\n    // {src}.{member}\n")
        o.append(f"    fn set_{verb_stem(field, ty, taken)}(this, v: {pty}) -> {cur} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return this; }}\n")
        if ty in CARRIER_TYPES:
            for suffix, ppty, _z in CARRIER_TYPES[ty][0]:
                o.append(f"        {{ (*p).{path}_{suffix} = "
                         f"{store_expr(ppty, f'v.{suffix}')} }};\n")
        else:
            o.append(f"        {{ (*p).{path} = {store_expr(ty, 'v')} }};\n")
        o.append(f"        core::touch(this._p, P_{field.upper()});\n")
        o.append("        return this;\n    }\n")

    for field, ty, member, src, path in writes + reads:
        rty = "str" if ty == "str" else cplus_type(ty)
        body = (carrier_reads(ty, path) if ty in CARRIER_TYPES
                else read_expr(ty, f"(*p).{path}"))
        dflt = '""' if ty == "str" else zero_of(ty)
        o.append(f"\n    fn {field}(this) -> {rty} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return {dflt}; }}\n")
        o.append(f"        return {body};\n    }}\n")

    # ---- owned props: not Copy, so written by `take` and read element-wise
    for field, ty, member, src, path in owned:
        # `items` -> item_count() / item(at:); `formatted_text` ->
        # formatted_text_count() / formatted_text_at(at:). The count always
        # names the field, so it never reads as `formatted_text_at_count`.
        one = field[:-1] if field.endswith("s") else field + "_at"
        cnt = field[:-1] if field.endswith("s") else field
        o.append(f"\n    // {src}.{member} — owned, so it is written whole and read\n")
        o.append("    // one element at a time: `-> ref T` does not parse.\n")
        o.append(f"    fn set_{field}(this, take v: {cplus_type(ty)}) -> {cur} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return this; }}\n")
        o.append(f"        {{ (*p).{path} = v }};\n")
        o.append(f"        core::touch(this._p, P_{field.upper()});\n")
        o.append("        return this;\n    }\n")
        o.append(f"\n    fn {cnt}_count(this) -> usize {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return 0 as usize; }}\n")
        o.append(f"        return {{ (*p).{path}.count() }};\n    }}\n")
        elem = OWNED_TYPES[ty]
        zero = '""' if elem == "str" else f"0 as {elem}"
        o.append(f"\n    fn {one}(this, at: usize) -> {elem} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return {zero}; }}\n")
        o.append(f"        return {{ (*p).{path}.at(at) }};\n    }}\n")

    # ---- slots: a subtree is a CHILD, under a reserved `@` key
    for field, ty, member, src in slots:
        o.append(f"\n    // {src}.{member} — a named child, not a property. `@` keys are\n")
        o.append("    // facet's; an application's key never starts with one.\n")
        o.append(f"    fn set_{field}(this, take n: core::Node) -> {cur} {{\n")
        o.append(f"        core::set_slot(this._p, \"@{field}\", n);\n")
        o.append("        return this;\n    }\n")
        o.append(f"\n    fn {field}(this) -> option::Option[*core::Node] {{\n")
        o.append(f"        return core::slot(this._p, \"@{field}\");\n    }}\n")

    # ---- commands: a write plus a dirty bit, same as a setter
    for verb, member, src, blk in commands:
        spec = COMMANDS[verb]
        args = "".join(f", {n}: {cplus_type(t) or t}" for n, t in spec["params"])
        o.append(f"\n    // {src}.{member}\n")
        o.append(f"    fn {verb}(this{args}) -> {cur} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return this; }}\n")
        for f, rhs in spec["writes"]:
            o.append(f"        {{ (*p).{paths[f]} = {rhs} }};\n")
        o.append(f"        core::touch(this._p, P_{verb.upper()});\n")
        o.append("        return this;\n    }\n")

    # ---- facet's own: a row count plus a row builder
    if mod in ROW_SOURCE:
        o.append("\n    // facet's own word. MAUI binds ItemsSource and realizes rows from a\n")
        o.append("    // DataTemplate; Stage 1 dropped both as MODEL, so the imperative\n")
        o.append("    // replacement is a count plus a builder. Without it a list cannot\n")
        o.append("    // be filled at all.\n")
        o.append(f"    fn set_count(this, v: usize) -> {cur} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return this; }}\n")
        o.append("        { (*p).count = v as i64 };\n")
        o.append("        core::touch(this._p, P_COUNT);\n")
        o.append("        return this;\n    }\n")
        o.append("\n    fn count(this) -> usize {\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return 0 as usize; }}\n")
        o.append("        return { (*p).count } as usize;\n    }\n")
        o.append(f"\n    fn set_row(this, f: fn(*u8, usize) -> core::Node, ctx: *u8 = 0 as *u8) -> {cur} {{\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return this; }}\n")
        o.append("        { (*p).row = f };\n")
        o.append("        { (*p).row_ctx = ctx };\n")
        o.append("        core::touch(this._p, P_ROW);\n")
        o.append("        return this;\n    }\n")
        o.append("\n    // Build row `at`. Empty until set_row names a builder.\n")
        o.append("    fn build_row(this, at: usize) -> core::Node {\n")
        o.append(f"        let p: *props::{props} = this._props();\n")
        o.append(f"        if p == (0 as *props::{props}) {{ return flex::Node::new(); }}\n")
        o.append("        let f: fn(*u8, usize) -> core::Node = { (*p).row };\n")
        o.append("        return f({ (*p).row_ctx }, at);\n    }\n")

    o.append("\n    // The flex node this cursor points at. The DOM division, and the\n")
    o.append("    // reason facet declares no layout verb of its own:\n")
    o.append("    //\n")
    o.append("    //   the control's own   b.set_title(\"Saving…\")     this module\n")
    o.append("    //   the shared band     b.set_opacity(0.5)         facet.cplus, once\n")
    o.append("    //   layout              { (*b.node()).set_width(…) }  ALL of flex\n")
    o.append("    //\n")
    o.append("    // Reaching flex through the node rather than forwarding its ~40 verbs\n")
    o.append("    // onto 42 cursors is what keeps facet from growing a second layout\n")
    o.append("    // vocabulary that can disagree with the first.\n")
    o.append(f"    fn node(this) -> *core::Node {{ return this._p; }}\n")
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
    o.append("// ---- lookup ------------------------------------------------------------\n")
    o.append(f"// `from` narrows a node to this control, the DOM step: a node carries what\n")
    o.append(f"// EVERY node has, and a checked cast adds what only a {mod} has. `None` on\n")
    o.append(f"// a wrong kind — never a cursor whose every verb silently does nothing.\n\n")
    o.append(f"fn from(p: *core::Node) -> option::Option[{cur}] {{\n")
    o.append(f"    if p == (0 as *core::Node) {{ return option::Option[{cur}]::None; }}\n")
    o.append("    let d: *core::Data = { (*p).data() };\n")
    o.append(f"    if d == (0 as *core::Data) {{ return option::Option[{cur}]::None; }}\n")
    o.append(f"    if {{ (*d).kind }} != props::K_{up} {{ return option::Option[{cur}]::None; }}\n")
    o.append(f"    return option::Option[{cur}]::Some({cur}::_of(p));\n}}\n\n")
    o.append("// Resolve a key, then narrow through the same checked door.\n\n")
    o.append("// `within:` and not `in:` — `in` is a C+ keyword. Defaults to the\n")
    o.append("// mounted tree, so an agent holding only a key can address it; pass a\n")
    o.append("// subtree to scope the search (stages/3, the global find).\n")
    o.append(f"fn find(key: str, within: *core::Node = mount::mounted_root()) -> option::Option[{cur}] {{\n")
    o.append(f"    if within == (0 as *core::Node) {{ return option::Option[{cur}]::None; }}\n")
    o.append("    return match core::find_in(within, key) {\n")
    o.append(f"        option::Option[*core::Node]::Some(p) => from(p),\n")
    o.append(f"        option::Option[*core::Node]::None => option::Option[{cur}]::None,\n")
    o.append("    };\n}\n")
    return "".join(o)


# ---------------------------------------------------------------------------
# elements.cplus — the DSL namespace. The `@` builder block resolves a bare
# element name in ONE context module; the regen scattered constructors
# across 38 modules, which orphaned the DSL (nothing to point `@` at).
# This module is the rendezvous: every constructor forwarded, one import.
#
#     import "facet/elements" as ui;
#     @ui { vstack(key: "body") { button("Save", key: "save") } }
#
# Containers forward facet core's (Builder first — the DSL passes the filled
# Builder as the first argument, so `vstack(key: "k") { ... }` is the same
# contract as bare `vstack { ... }`). The four facet-origin hand-written
# controls are forwarded from the literals below; if their signatures move,
# the build of THIS file breaks, which is the guard.

CONTAINER_FORWARDS = ["column", "row", "hstack", "vstack", "screen", "card",
                      "zstack"]

FACET_ORIGIN_FORWARDS = """\
// ---- facet-origin controls (hand-written modules; forwards mirrored here,
// ---- drift breaks this file's build) -----------------------------------
fn symbol(
    icon: u32,
    key: str = "",
    size: f64 = 16.0f64,
    fill: f64 = 0.0f64,
    color: vocab::Color = vocab::Color::clear(),
    font: str = "",
) -> core::Node {
    return m_symbol::symbol(icon, key: key, size: size, fill: fill,
                            color: color, font: font);
}

fn split(
    take b: Builder,
    key: str = "",
    axis: m_split::SplitAxis = m_split::SplitAxis::Columns,
    position: f64 = 220.0f64,
    min_leading: f64 = 150.0f64,
    min_trailing: f64 = 240.0f64,
    on_move: fn(*u8, *u8) = m_split::no_move,
    ctx: *u8 = 0 as *u8,
) -> core::Node {
    return m_split::split(b, key: key, axis: axis, position: position,
                          min_leading: min_leading, min_trailing: min_trailing,
                          on_move: on_move, ctx: ctx);
}

fn tree(
    take root: m_tree::TreeNode,
    key: str = "",
    row: fn(*m_tree::TreeNode, *u8) -> flex::Node = m_tree::no_row,
    on_select: fn(*m_tree::TreeNode, *u8) = m_tree::no_select,
    ctx: *u8 = 0 as *u8,
    row_height: f64 = 0.0f64,
) -> core::Node {
    return m_tree::tree(root, key: key, row: row, on_select: on_select,
                        ctx: ctx, row_height: row_height);
}

fn window_buttons(
    key: str = "",
    style: m_window_chrome::ButtonStyle = m_window_chrome::ButtonStyle::Native,
    spacing: f64 = 0.0f64,
) -> core::Node {
    return m_window_chrome::window_buttons(key: key, style: style,
                                           spacing: spacing);
}
"""


def emit_elements(rows_by_control):
    o = ["// GENERATED by tools/gen_contract.py — DO NOT EDIT.\n",
         "// elements — every element name in one module, for the `@` builder\n",
         "// DSL's contextual lookup: `import \"facet/elements\" as ui;` then\n",
         "// `@ui { vstack(key: \"body\") { button(\"Save\", key: \"save\") } }`.\n",
         "// Forwards only. A control LIVES in its own module (find/cursors\n",
         "// resolve there); this module exists so a bare element name inside\n",
         "// an `@` block resolves in one context.\n\n",
         'import "flex_layout/flex_layout" as flex;\n',
         'import "./facet" as core;\n',
         'import "./props" as props;\n',
         'import "./vocabulary" as vocab;\n',
         'import "./symbol" as m_symbol;\n',
         'import "./split" as m_split;\n',
         'import "./tree" as m_tree;\n',
         'import "./window_chrome" as m_window_chrome;\n']
    for maui in rows_by_control:
        mod = MODULE[maui]
        o.append(f'import "./{mod}" as m_{mod};\n')
    o.append("\ntype Builder = flex::Builder;\n\n")
    o.append("// ---- containers (facet core owns these) --------------------------------\n")
    for c in CONTAINER_FORWARDS:
        o.append(f"fn {c}(take b: Builder, key: str = \"\") -> core::Node "
                 f"{{ return core::{c}(b, key: key); }}\n")
    o.append("fn spacer(key: str = \"\") -> core::Node "
             "{ return core::spacer(key: key); }\n\n")
    o.append(FACET_ORIGIN_FORWARDS)
    o.append("\n// ---- controls ----------------------------------------------------------\n\n")
    for maui, merged in rows_by_control.items():
        mod = MODULE[maui]
        writes, reads, events, _slots, _owned, _commands = split_rows(merged)
        params = ctor_params(maui, writes, reads, events)
        o.append(f"fn {mod}(\n")
        for nm, pty, dflt in params:
            o.append(f"    {nm}: {pty},\n" if dflt is None
                     else f"    {nm}: {pty} = {dflt},\n")
        o.append(") -> core::Node {\n")
        o.append(f"    return m_{mod}::{mod}(\n")
        fwd = []
        for i, (nm, _pty, dflt) in enumerate(params):
            fwd.append(f"        {nm}" if (i == 0 and dflt is None)
                       else f"        {nm}: {nm}")
        o.append(",\n".join(fwd) + ",\n    );\n}\n\n")
    return "".join(o)


def emit_manifest(rows_by_control):
    o = ["# facet contract manifest — GENERATED by tools/gen_contract.py\n\n",
         "Every declared word, its type, and the MAUI row it came from. A verb\n",
         "absent here does not exist: calling it is a compile error, never a\n",
         "silent no-op. What a backend cannot implement is recorded in that\n",
         "backend's own manifest, not here.\n\n"]
    total, skipped = 0, []
    for maui, merged in sorted(rows_by_control.items(), key=lambda kv: MODULE[kv[0]]):
        mod = MODULE[maui]
        o.append(f"\n## {mod} — MAUI {maui}\n\n")
        o.append("| verb | type | provenance |\n|---|---|---|\n")
        taken = {row_kind(b, f, n)[1] for _m, b, f, n, _s in merged
                 if row_kind(b, f, n) and row_kind(b, f, n)[0] in ("prop", "owned")}
        for member, band, fn, note, src in merged:
            k = row_kind(band, fn, note)
            if k is None:
                continue
            kind, name, detail = k
            if kind == "prop":
                stem = verb_stem(name, detail, taken)
                verb = (f"`set_{stem}` / `{name}()`" if band == "writes"
                        else f"`{name}()`")
                o.append(f"| {verb} | {detail} | {src}.{member} |\n")
                total += 1
            elif kind == "owned":
                one = name[:-1] if name.endswith("s") else name + "_at"
                cnt = name[:-1] if name.endswith("s") else name
                o.append(f"| `set_{name}` / `{cnt}_count()` / `{one}(at:)` "
                         f"| {detail} | {src}.{member} |\n")
                total += 1
            elif kind == "slot":
                o.append(f"| `set_{name}` / `{name}()` | Node (a named child) "
                         f"| {src}.{member} |\n")
                total += 1
            elif kind == "command":
                args = ", ".join(f"{n}:" for n, _t in COMMANDS[name]["params"])
                o.append(f"| `{name}({args})` | command | {src}.{member} |\n")
                total += 1
            elif kind == "event":
                o.append(f"| `{name}` | callback + ctx | {src}.{member} |\n")
                total += 1
            elif kind == "shared":
                o.append(f"| `{name}` | shared band | {src}.{member} |\n")
            elif kind == "skip":
                skipped.append((mod, src, member, name, detail))
        if mod in ROW_SOURCE:
            o.append("| `set_count` / `count()` | usize | **facet's own** |\n")
            o.append("| `set_row(_:ctx:)` / `build_row(at:)` | fn(*u8, usize) -> Node "
                     "| **facet's own** |\n")
            total += 2

    o.append("\n## the shared band\n\n")
    o.append("Declared once on `Node` and forwarded onto every cursor, so a verb\n")
    o.append("every element carries is written in one place and no generic element\n")
    o.append("type exists.\n\n")
    o.append("| verb | returns |\n|---|---|\n")
    for name, arg, _b, ret in SHARED_FORWARDS:
        sig = f"{name}({arg.split(':')[0] + ':' if arg else ''})"
        o.append(f"| `{sig}` | {ret or 'the cursor, so writes chain'} |\n")
    o.append("| `frame()` | flex::Frame |\n")
    o.append(f"\n{len(DEFERRED_SHARED)} rows of the ledger's shared band are not "
             "forwarded yet. Each is here\nwith its reason; the generator fails "
             "the run on a row that is in neither\nlist.\n\n")
    o.append("| verb | why not yet |\n|---|---|\n")
    for verb, why in sorted(DEFERRED_SHARED.items()):
        o.append(f"| `{verb}` | {why} |\n")

    if skipped:
        o.append("\n## reached a control, not emitted\n\n")
        o.append("Recorded, not silent. Stage 1 fails on an unbucketed row; this\n")
        o.append("is the same discipline one stage later.\n\n")
        o.append("| control | row | why |\n|---|---|---|\n")
        for mod, src, member, name, why in sorted(skipped):
            o.append(f"| {mod} | {src}.{member} | {why} |\n")

    o.insert(5, f"{total} declared verbs over {len(rows_by_control)} controls, plus the\n"
                "shared band every element carries.\n\n")
    return "".join(o)


# ---------------------------------------------------------------------------
# The guards. Stage 1 exits non-zero on an unbucketed row and on an
# unclassified type. Stage 2 had no equivalent, so 31 ADOPT rows and 26 of the
# shared band's 41 verbs went missing without a word. These are that equivalent.

# naming_guideline.md, made checkable. A name here is one the guideline still
# rejects after the MAP's RENAME pass, so either RENAME is short a row or the
# exception is real and belongs in ALLOWED with its reason.
MAX_SETTER = 26          # `set_horizontal_scroll_bars` is the longest that reads

TYPE_NOUNS = ("_mode", "_type", "_strategy", "_visibility", "_source",
              "_kind", "_option", "_options", "_enabled")

ALLOWED = {
    "set_is_opaque": "`opaque` is a C+ keyword, so the stem is unspellable and "
                     "the assertion form is the only one left",
    "selection_mode": "the mode IS the vocabulary here — `selection()` would "
                      "read as the selected item, which is `selected_index`",
    "set_selection_mode": "see selection_mode",
}


def _stem(name):
    """The name with any assertion or setter prefix removed."""
    for pre in ("set_", "is_", "has_", "can_"):
        if name.startswith(pre):
            return _stem(name[len(pre):])
    return name


def lint_names(verbs):
    """verbs: {name: (control, facet type, is_setter)}. Returns a list of
    (name, control, rule) for every name the guideline rejects.

    A trailing noun is only needless when something precedes it: `source()` on
    an image names the role, `image_source()` names the .NET type MAUI wraps.
    Same for `enabled` — the assertion is the whole word, so `is_enabled` is
    right and `is_grouping_enabled` is not."""
    bad = []
    for name, (mod, ty, is_set) in sorted(verbs.items()):
        if name in ALLOWED:
            continue
        stem = _stem(name)
        if is_set and len(name) > MAX_SETTER:
            bad.append((name, mod, f"{len(name)} chars — omit needless words "
                                   f"(the guideline's budget is {MAX_SETTER})"))
            continue
        if is_set and name.startswith(("set_is_", "set_has_", "set_can_")):
            bad.append((name, mod, "a boolean setter keeps the assertion prefix; "
                                   "the reader asserts, the setter does not"))
            continue
        for noun in TYPE_NOUNS:
            if name.endswith(noun) and stem != noun[1:]:
                bad.append((name, mod, f"`{noun}` names the type, not the role — "
                                       "drop it, or rename the type it echoes"))
                break
    return bad


# Guard 5 (stages/3.md): every ADOPT row reaches a control, a tier, or a
# recorded exception — nothing sits unclaimed the way the 21 gesture rows did
# until an audit found them. Tier types are the ones with no control module;
# each names the Stage 3 module that owns its rows.
TIER_TYPES = {
    "Window":      "application.cplus Window + runtime.cplus Window interface — an app owns many, all shown at once",
    "Application": "runtime.cplus App embedding application.cplus Application — one runs at a time, several may live in the exe",
    "Page":        "screen.cplus — Screen/Chrome",
    "ContentPage": "screen.cplus — Screen",
    "TitleBar":    "screen.cplus — Chrome (bar: Bar)",
    "Toolbar":     "screen.cplus — the app-menu/toolbar tier",
    # Owners that are neither controls nor Stage 3 tier modules:
    "VisualElement": "the shared band — CommonProps + facet.cplus forwards "
                     "(SHARED_FORWARDS/DEFERRED_SHARED guard each row by verb)",
    "View":          "the shared band — VisualElement's subclass adds margins, "
                     "carried by flex layout",
    "TapGestureRecognizer":     "gestures.cplus — .gesture()",
    "PanGestureRecognizer":     "gestures.cplus — .gesture()",
    "PinchGestureRecognizer":   "gestures.cplus — .gesture()",
    "SwipeGestureRecognizer":   "gestures.cplus — .gesture()",
    "PointerGestureRecognizer": "gestures.cplus — .gesture()",
    "DragGestureRecognizer":    "gestures.cplus — the drag band",
    "DropGestureRecognizer":    "gestures.cplus — the drop band + "
                                "component.cplus dropped_text/drag_targeted",
    "Shadow":              "vocabulary.cplus — vocab::Shadow, a value type",
    "FontImageSource":     "symbol.cplus — the icon-font glyph control",
    "KeyboardAccelerator": "vocabulary.cplus — vocab::Shortcut (decomposed "
                           "storage, see CARRIER_TYPES)",
}



# Guard 5b (review finding, 2026-08-04): type ownership alone let all 82 tier
# rows ride a green guard while ~4 in 5 had no surface. Every tier ADOPT row
# now carries an explicit disposition: ("implemented", where) names the verb
# that answers it TODAY; ("deferred", why) is an honest debt the backend
# stage pays. A tier row in neither is a failure — nothing silent.
TIER_ROWS = {
    # ---- Window (30) ----
    ("Window", "Title"):            ("implemented", "screen.Chrome.title"),
    ("Window", "Width"):            ("implemented", "screen.Chrome.width"),
    ("Window", "Height"):           ("implemented", "screen.Chrome.height"),
    ("Window", "MinimumWidth"):     ("implemented", "screen.Chrome.min_width"),
    ("Window", "MinimumHeight"):    ("implemented", "screen.Chrome.min_height"),
    ("Window", "X"):                ("implemented", "runtime.window_frame/set_window_frame"),
    ("Window", "Y"):                ("implemented", "runtime.window_frame/set_window_frame"),
    ("Window", "Page"):             ("implemented", "application.Window.root + mount"),
    ("Window", "FlowDirection"):    ("implemented", "the root node's shared band"),
    ("Window", "Activated"):        ("implemented", "runtime.observe_window_active"),
    ("Window", "Deactivated"):      ("implemented", "runtime.observe_window_inactive"),
    ("Window", "Created"):          ("implemented", "runtime.App.on_launch"),
    ("Window", "Destroying"):       ("implemented", "runtime.App.on_quit"),
    ("Window", "TitleBar"):         ("deferred", "TitleBar content tier — backend stage; Bar::Custom + window_buttons/window_drag are the shipped primitives"),
    ("Window", "MaximumWidth"):     ("deferred", "window sizing beyond min — backend stage"),
    ("Window", "MaximumHeight"):    ("deferred", "window sizing beyond min — backend stage"),
    ("Window", "IsMaximizable"):    ("deferred", "window button policy — backend stage"),
    ("Window", "IsMinimizable"):    ("deferred", "window button policy — backend stage"),
    ("Window", "DisplayDensity"):   ("deferred", "needs a native window — backend stage"),
    ("Window", "DisplayDensityChanged"): ("deferred", "needs a native window — backend stage"),
    ("Window", "IsActivated"):      ("deferred", "read half of activation — backend stage (the observers exist)"),
    ("Window", "SizeChanged"):      ("deferred", "observe_size on the window root once the facade mounts it"),
    ("Window", "Backgrounding"):    ("deferred", "process lifecycle — backend stage"),
    ("Window", "Resumed"):          ("deferred", "process lifecycle — backend stage"),
    ("Window", "Stopped"):          ("deferred", "process lifecycle — backend stage"),
    ("Window", "ModalPushed"):      ("deferred", "modal stack — rides with nav::push's facade"),
    ("Window", "ModalPushing"):     ("deferred", "modal stack — rides with nav::push's facade"),
    ("Window", "ModalPopped"):      ("deferred", "modal stack — rides with nav::push's facade"),
    ("Window", "ModalPopping"):     ("deferred", "modal stack — rides with nav::push's facade"),
    ("Window", "PopCanceled"):      ("deferred", "modal stack — rides with nav::push's facade"),
    # ---- Application (13) ----
    ("Application", "Windows"):         ("implemented", "application.window_count/window_root"),
    ("Application", "PlatformAppTheme"): ("implemented", "theme.is_dark"),
    ("Application", "RequestedTheme"):  ("implemented", "theme.is_dark"),
    ("Application", "RequestedThemeChanged"): ("implemented", "theme.on_appearance_change"),
    ("Application", "AccentColor"):     ("implemented", "theme role primary via set_theme"),
    ("Application", "MainPage"):        ("deferred", "superseded by the windows model; newest_window_root is the read"),
    ("Application", "UserAppTheme"):    ("deferred", "explicit light/dark override — backend stage appearance write"),
    ("Application", "ModalPushed"):     ("deferred", "modal stack — rides with nav::push's facade"),
    ("Application", "ModalPushing"):    ("deferred", "modal stack — rides with nav::push's facade"),
    ("Application", "ModalPopped"):     ("deferred", "modal stack — rides with nav::push's facade"),
    ("Application", "ModalPopping"):    ("deferred", "modal stack — rides with nav::push's facade"),
    ("Application", "PageAppearing"):   ("deferred", "app-level page observation — the per-screen Lifecycle covers the use"),
    ("Application", "PageDisappearing"): ("deferred", "app-level page observation — the per-screen Lifecycle covers the use"),
    # ---- Page (14) ----
    ("Page", "Title"):              ("implemented", "screen.Chrome.title"),
    ("Page", "Appearing"):          ("implemented", "component.Lifecycle.on_attach"),
    ("Page", "Disappearing"):       ("implemented", "component.Lifecycle.on_detach"),
    ("Page", "MenuBarItems"):       ("implemented", "screen.Screen.menu_items"),
    ("Page", "LayoutChanged"):      ("implemented", "services.observe_size on the page root"),
    ("Page", "BackgroundImageSource"): ("deferred", "page chrome imagery — backend stage"),
    ("Page", "IconImageSource"):    ("deferred", "page chrome imagery — backend stage"),
    ("Page", "ContainerArea"):      ("deferred", "safe-area plumbing — backend stage"),
    ("Page", "IgnoresContainerArea"): ("deferred", "safe-area plumbing — backend stage"),
    ("Page", "IsBusy"):             ("deferred", "page busy indicator — backend stage"),
    ("Page", "ToolbarItems"):       ("deferred", "toolbar tier — backend stage"),
    ("Page", "NavigatedTo"):        ("deferred", "nav observation — rides with the facade's nav loop"),
    ("Page", "NavigatedFrom"):      ("deferred", "nav observation — rides with the facade's nav loop"),
    ("Page", "NavigatingFrom"):     ("deferred", "nav observation — rides with the facade's nav loop"),
    # ---- ContentPage (3) ----
    ("ContentPage", "Content"):     ("implemented", "mount.set_content"),
    ("ContentPage", "SafeAreaEdges"): ("deferred", "safe-area plumbing — backend stage (vocab::SafeArea exists)"),
    ("ContentPage", "HideSoftInputOnTapped"): ("deferred", "soft-keyboard policy — mobile backend stage"),
    # ---- Toolbar (14) ----
    ("Toolbar", "Title"):           ("deferred", "toolbar tier — backend stage (AppKit NSToolbar)"),
    ("Toolbar", "TitleIcon"):       ("deferred", "toolbar tier — backend stage"),
    ("Toolbar", "TitleView"):       ("deferred", "toolbar tier — backend stage"),
    ("Toolbar", "ToolbarItems"):    ("deferred", "toolbar tier — backend stage"),
    ("Toolbar", "IsVisible"):       ("deferred", "toolbar tier — backend stage"),
    ("Toolbar", "IconColor"):       ("deferred", "toolbar tier — backend stage"),
    ("Toolbar", "BarBackground"):   ("deferred", "toolbar tier — backend stage"),
    ("Toolbar", "BarHeight"):       ("deferred", "toolbar tier — backend stage"),
    ("Toolbar", "BarTextColor"):    ("deferred", "toolbar tier — backend stage"),
    ("Toolbar", "BackButtonEnabled"): ("deferred", "nav-bar back button — mobile backend stage"),
    ("Toolbar", "BackButtonTitle"): ("deferred", "nav-bar back button — mobile backend stage"),
    ("Toolbar", "BackButtonVisible"): ("deferred", "nav-bar back button — mobile backend stage"),
    ("Toolbar", "DrawerToggleVisible"): ("deferred", "drawer affordance — mobile backend stage"),
    ("Toolbar", "DynamicOverflowEnabled"): ("deferred", "toolbar overflow — backend stage"),
    # ---- TitleBar (8) ----
    ("TitleBar", "Title"):          ("deferred", "custom titlebar content — Bar::Custom + window_buttons are the primitives"),
    ("TitleBar", "Subtitle"):       ("deferred", "custom titlebar content — backend stage"),
    ("TitleBar", "Icon"):           ("deferred", "custom titlebar content — backend stage"),
    ("TitleBar", "Content"):        ("deferred", "custom titlebar content — backend stage"),
    ("TitleBar", "LeadingContent"): ("deferred", "custom titlebar content — backend stage"),
    ("TitleBar", "TrailingContent"): ("deferred", "custom titlebar content — backend stage"),
    ("TitleBar", "ForegroundColor"): ("deferred", "custom titlebar content — backend stage"),
    ("TitleBar", "PassthroughElements"): ("deferred", "titlebar hit-testing — backend stage"),
}

def check(rows_by_control, by_type):
    """Fail the run, by name, on anything the emitters would drop in silence."""
    problems = []

    # 5. every ADOPT row reaches SOMETHING: a control module (directly or
    # through EXTRA_BASES), a tier type, or a recorded exception.
    reachable = set(MODULE)
    for maui in MODULE:
        reachable |= set(EXTRA_BASES.get(maui, []))
    rows_all, _und = maui_map.rows()
    tier_types = {"Window", "Application", "Page", "ContentPage", "TitleBar", "Toolbar"}
    n_impl, n_defer = 0, 0
    for ty, member, band, _vt, st, _fn, _note in rows_all:
        if st != "ADOPT":
            continue
        if ty in tier_types:
            # Guard 5b: row-level dispositions — a tier row must name the
            # verb that answers it or the debt that defers it.
            d = TIER_ROWS.get((ty, member))
            if d is None:
                problems.append(
                    f"guard 5b: {ty}.{member} [{band}] is a tier ADOPT row with "
                    f"no disposition in TIER_ROWS — name the implementing verb "
                    f"or record the deferral with its reason.")
            elif d[0] == "implemented":
                n_impl += 1
            else:
                n_defer += 1
            continue
        if ty in reachable or ty in TIER_TYPES:
            continue
        problems.append(
            f"guard 5: {ty}.{member} [{band}] is ADOPT and `{ty}` reaches no "
            f"control module and no TIER_TYPES entry — an unclaimed row. Give "
            f"the type an owner or record the exception.")
    if not problems:
        print(f"tier ledger: {n_impl} rows carried by shipped surface, "
              f"{n_defer} explicitly deferred to the backend stage")

    # 1. every ADOPT row reaching a control is carried by something
    for maui, merged in sorted(rows_by_control.items()):
        for member, band, fn, note, src in merged:
            if row_kind(band, fn, note) is None:
                _v, _f, ty = _field_and_type(fn, note)
                problems.append(
                    f"{MODULE[maui]}: {src}.{member} [{band}] is ADOPT with facet "
                    f"type `{ty or '?'}` and nothing carries it. Emit it, or move "
                    f"the row to DROP in maui_map.OVERLAY with a reason.")

    # 2. every command writes a field the control actually has
    for maui, merged in sorted(rows_by_control.items()):
        have = set()
        for member, band, fn, note, src in merged:
            k = row_kind(band, fn, note)
            if k and k[0] in ("prop", "owned"):
                have.add(k[1])
        for member, band, fn, note, src in merged:
            k = row_kind(band, fn, note)
            if not k or k[0] != "command":
                continue
            have |= {f for f, _t in COMMANDS[k[1]]["extra"]}
            for f, _rhs in COMMANDS[k[1]]["writes"]:
                if f not in have:
                    problems.append(
                        f"{MODULE[maui]}: command `{k[1]}` writes `{f}`, which "
                        f"{MODULE[maui]} does not have.")

    # 3. the dirty word has room: the shared band owns the top 16 bits
    for maui, merged in sorted(rows_by_control.items()):
        n = sum(1 for member, band, fn, note, src in merged
                for k in [row_kind(band, fn, note)]
                if k and (k[0] == "owned" or k[0] == "command"
                          or (k[0] == "prop" and band == "writes")))
        n += 2 if MODULE[maui] in ROW_SOURCE else 0
        if n > 48:
            problems.append(f"{MODULE[maui]}: {n} dirty bits, and props::C_* owns "
                            "bits 48 and up.")

    # 4. the shared band: forwarded, or deferred with a reason
    for base in COMMON_BASES:
        for member, band, fn, note in by_type.get(base, []):
            verb = fn.split(" / ")[0].split("(")[0].strip()
            # a `set_foo / foo()` ledger row is one forward pair
            core_verb = verb[4:] if verb.startswith("set_") else verb
            if core_verb in SHARED_VERBS or verb in DEFERRED_SHARED:
                continue
            problems.append(
                f"shared band: {base}.{member} [{band}] -> `{verb}` is ADOPT and "
                "is neither in SHARED_FORWARDS nor recorded in DEFERRED_SHARED.")

    # 5. the names, against naming_guideline.md
    verbs = {}
    for maui, merged in sorted(rows_by_control.items()):
        mod = MODULE[maui]
        taken = {k[1] for member, band, fn, note, src in merged
                 for k in [row_kind(band, fn, note)]
                 if k and k[0] in ("prop", "owned")}
        for member, band, fn, note, src in merged:
            k = row_kind(band, fn, note)
            if not k:
                continue
            kind, name, detail = k
            if kind == "prop":
                verbs.setdefault(name, (mod, detail, False))
                if band == "writes":
                    stem = "set_" + verb_stem(name, detail, taken)
                    verbs.setdefault(stem, (mod, detail, True))
            elif kind in ("owned", "slot"):
                verbs.setdefault(name, (mod, detail, False))
                verbs.setdefault("set_" + name, (mod, detail, True))
            elif kind == "command":
                verbs.setdefault(name, (mod, "command", False))
    for name, arg, _b, ret in SHARED_FORWARDS:
        verbs.setdefault(name, ("<shared>", "shared", name.startswith("set_")))
    for name, mod, rule in lint_names(verbs):
        problems.append(f"naming: `{name}` ({mod}) — {rule}. Rename it in "
                        "maui_map.RENAME, or record the exception in ALLOWED.")

    if problems:
        print(f"gen_contract: {len(problems)} problems. Nothing was written.\n",
              file=sys.stderr)
        for p in problems:
            print(f"  {p}", file=sys.stderr)
        raise SystemExit(1)


def main():
    global ENUM_BY_FACET
    ENUM_BY_FACET = read_enums()
    rows_by_control, by_type = control_rows()
    check(rows_by_control, by_type)

    os.makedirs(SRC, exist_ok=True)
    os.makedirs(DOCS, exist_ok=True)
    with open(os.path.join(SRC, "vocabulary.cplus"), "w") as f:
        f.write(emit_vocabulary())
    with open(os.path.join(SRC, "props.cplus"), "w") as f:
        f.write(emit_props(rows_by_control, by_type))
    for maui, merged in rows_by_control.items():
        with open(os.path.join(SRC, MODULE[maui] + ".cplus"), "w") as f:
            f.write(emit_control(maui, merged))
    with open(os.path.join(SRC, "elements.cplus"), "w") as f:
        f.write(emit_elements(rows_by_control))
    with open(os.path.join(DOCS, "contract.md"), "w") as f:
        f.write(emit_manifest(rows_by_control))

    print(f"{len(ENUM_BY_FACET)} enums, {len(rows_by_control)} controls")
    print(f"wrote vendor/facet/src/{{vocabulary,props,elements}}.cplus + "
          f"{len(rows_by_control)} control modules + docs/contract.md")


if __name__ == "__main__":
    main()
