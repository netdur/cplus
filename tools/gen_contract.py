#!/usr/bin/env python3
"""gen_contract.py — generate the FULL portable contract from the MAUI spec and
patch it into facet.cplus + facet_appkit.cplus (build-safe, no testing needed).

Every slot routes through a generic, `respondsToSelector:`-guarded dispatcher, so
an unsupported/wrong selector NO-OPS at runtime (MAUI-style fail-soft) instead of
crashing or breaking the build. That's what lets a whole generated contract
compile without per-slot verification.

Inputs:
  SPEC : MAUI netstandard PublicAPI manifest (property names + types).  Automated.
  MAP  : type table + selector rule + snake_case naming (guideline).    Curated.
  NATIVE: one generic dispatcher per value type in facet_appkit.        Hand, tiny.

It replaces the text after each of four one-line markers:
  facet.cplus:        // GEN:STATICS   // GEN:METHODS
  facet_appkit.cplus: // GEN:APPKIT    // GEN:REGS
Re-runnable: the region between a marker and the next `// GEN:END` is regenerated.
"""
import re
import sys

ROOT = "/Users/adel/Workspace/C+"
MANIFEST = ("/private/tmp/claude-501/-Users-adel-Workspace-C-/"
            "c9010693-d135-4ccf-9c35-38de1f80fbe8/scratchpad/maui_PublicAPI.Shipped.txt")
FACET = f"{ROOT}/vendor/facet/src/facet.cplus"
APPKIT = f"{ROOT}/vendor/facet_appkit/src/facet_appkit.cplus"

# Declaring types whose properties form the portable widget surface (controls +
# their shared bases — captures inherited text/visual/collection props).
TYPES = {"Label", "Button", "Entry", "Editor", "SearchBar", "Picker", "Slider",
         "Stepper", "Switch", "DatePicker", "TimePicker", "ProgressBar",
         "ActivityIndicator", "Image", "CollectionView", "ListView", "TableView",
         "CheckBox", "RadioButton", "BoxView", "Border",
         "View", "VisualElement", "InputView", "ItemsView"}

# MAUI type -> (facet.cplus type, facet_appkit type, dispatcher).
TYMAP = {
    "string": ("str", "str", "slot_str"),
    "double": ("f64", "f64", "slot_f64"),
    "bool": ("bool", "bool", "slot_bool"),
    "int": ("i64", "i64", "slot_i64"),
    "Microsoft.Maui.Graphics.Color": ("Color", "facet::Color", "slot_color"),
}

# Slots already on Handle (must not collide).
EXCLUDE = {"set_background", "set_border", "set_child", "set_corner_radius",
           "set_font", "set_foreground_color", "set_hidden", "set_maximum",
           "set_maximum_track_color", "set_minimum", "set_minimum_track_color",
           "set_on", "set_strong", "set_style", "set_text", "set_thumb_color",
           "set_thumb_image", "set_value", "set_weight"}

# VERIFIED native backings only. slot -> (selector, send-kind, needs_relayout).
# The kind picks the message ABI to MATCH the real AppKit method signature (not
# the MAUI type) — that is what prevents the wrong-register-bank UB. A slot NOT
# listed here is left unregistered: its Handle method no-ops through the null-fn
# path (no message sent), the genuinely-safe "inert". Expand this map as each
# (widget, property) mapping is verified against AppKit + covered by a test.
#   kinds: color str f64 i64 f64i(int->CGFloat) bool hidden(->setHidden: inverted)
#          editable_inv(->setEditable: inverted)
NATIVE = {
    "set_text_color":       ("setTextColor:",         "color",        False),
    "set_background_color":  ("setBackgroundColor:",   "color",        False),
    "set_placeholder":       ("setPlaceholderString:", "str",          False),
    "set_is_enabled":        ("setEnabled:",           "bool",         False),
    "set_is_visible":        ("setHidden:",            "hidden",       False),
    "set_is_read_only":      ("setEditable:",          "editable_inv", False),
    "set_opacity":           ("setAlphaValue:",        "f64",          False),
    "set_progress":          ("setDoubleValue:",       "f64",          False),
    "set_title":             ("setTitle:",             "str",          True),
}


def send(kind):
    if kind == "color":
        return "    rt::msg_void_id(view, s, ns_color(v).raw());"
    if kind == "str":
        return "    rt::msg_void_id(view, s, bridge::nsstring(v));"
    if kind == "f64":
        return "    rt::msg_void_f64(view, s, v);"
    if kind == "f64i":
        return "    rt::msg_void_f64(view, s, v as f64);"
    if kind == "i64":
        return "    rt::msg_void_i64(view, s, v);"
    if kind == "bool":
        return "    var b: i8 = 0 as i8;\n    if v { b = 1 as i8; }\n    rt::msg_void_i8(view, s, b);"
    # inverted booleans: is_visible -> setHidden:(!v); is_read_only -> setEditable:(!v)
    return "    var b: i8 = 1 as i8;\n    if v { b = 0 as i8; }\n    rt::msg_void_i8(view, s, b);"

MODS = re.compile(r"^~?(?:(?:static|readonly|const|abstract|virtual|override|"
                  r"sealed|event|new)\s+)*")
PROP = re.compile(r"^Microsoft\.Maui\.Controls\.(?:\w+\.)*(\w+)\.(\w+)Property -> "
                  r"Microsoft\.Maui\.Controls\.BindableProperty")
GGET = re.compile(r"^Microsoft\.Maui\.Controls\.(?:\w+\.)*(\w+)\.(\w+)\.get -> "
                  r"([\w\.<>]+)$")


def snake(s):
    return re.sub(r"(?<!^)(?=[A-Z])", "_", s).lower()


def slots():
    bind = set()
    typ = {}
    for line in open(MANIFEST):
        line = MODS.sub("", line.strip())
        m = PROP.match(line)
        if m and m.group(1) in TYPES:
            bind.add((m.group(1), m.group(2)))
        g = GGET.match(line)
        if g and g.group(1) in TYPES:
            typ[(g.group(1), g.group(2))] = g.group(3)
    out = {}
    for (t, p) in sorted(bind):
        rt = typ.get((t, p))
        if rt not in TYMAP:
            continue
        name = "set_" + snake(p)
        if name in EXCLUDE or name in out:
            continue
        out[name] = (p, TYMAP[rt])
    return out


def gen(s):
    # Contract SURFACE (statics + Handle methods) is generated for EVERY slot —
    # unregistered ones no-op safely. The BACKING (impls + regs) is generated
    # ONLY for verified NATIVE entries, each with the ABI-correct message.
    statics, methods, impls, regs = [], [], [], []
    for name, (prop, (fct, akt, _)) in sorted(s.items()):
        const = name.upper() + "_FN"
        statics.append(
            f"static {const}: fn(Handle, {fct}) -> bool = #zero::[fn(Handle, {fct}) -> bool]();\n"
            f"fn {name}_fn(f: fn(Handle, {fct}) -> bool) {{ {const} = f; return; }}")
        methods.append(
            f"    fn {name}(this, v: {fct}) -> Handle {{\n"
            f"        let f: fn(Handle, {fct}) -> bool = {const};\n"
            f"        if f != (0 as fn(Handle, {fct}) -> bool) {{ let _ok: bool = f(this, v); }}\n"
            f"        return this;\n    }}")
        if name in NATIVE:
            sel, kind, relayout = NATIVE[name]
            rl = "    slot_relayout(h, view);\n" if relayout else ""
            impls.append(
                f"fn c_{name}(h: facet::Handle, v: {akt}) -> bool {{\n"
                f"    let view: *u8 = h.view();\n"
                f"    if view == (0 as *u8) {{ return false; }}\n"
                f"    let s: *u8 = rt::sel(#str_ptr(\"{sel}\\0\"));\n"
                f"    if !slot_responds(view, s) {{ return false; }}\n"
                f"{send(kind)}\n"
                f"{rl}"
                f"    return true;\n}}")
            regs.append(f"    facet::{name}_fn(c_{name});")
    return ("\n".join(statics), "\n".join(methods),
            "\n".join(impls), "\n".join(regs))


DISPATCHERS = r'''// Two shared helpers for the verified per-slot backings below. Each backing
// sends its OWN ABI-correct message (the send-kind matches the real AppKit
// method signature); these only cover the guard and the layout invalidation.
fn slot_responds(view: *u8, sel: *u8) -> bool {
    return rt::msg_i8_id(view, rt::sel(#str_ptr("respondsToSelector:\0")), sel) != (0 as i8);
}
// A property that changes a control's intrinsic size must invalidate facet's
// retained layout, not just the native view (mirrors set_text_impl).
fn slot_relayout(h: facet::Handle, view: *u8) {
    ui::clear_measure_caches(view);
    if h.flex() != (0 as *u8) {
        let fp: *flex::Node = h.flex() as *flex::Node;
        { (*fp).mark_content_changed(); }
    }
    let _r: bool = ui::reflow_owner(h.cp(), view);
    return;
}
'''


def patch(path, marker, block):
    # Each region has its OWN end marker (marker + ":END") so regions never
    # overlap regardless of their order in the file.
    end_marker = marker + ":END\n"
    src = open(path).read()
    line_end = src.index(marker + "\n") + len(marker) + 1
    e = src.find(end_marker, line_end)
    if e == -1:                       # first run: create the region
        src = src[:line_end] + block + "\n" + end_marker + src[line_end:]
    else:                             # re-run: replace the region body
        src = src[:line_end] + block + "\n" + src[e:]
    open(path, "w").write(src)


def main():
    s = slots()
    statics, methods, impls, regs = gen(s)
    patch(FACET, "// GEN:STATICS",
          "// ==== GENERATED contract (tools/gen_contract.py) ====\n" + statics)
    patch(FACET, "// GEN:METHODS", methods)
    patch(APPKIT, "// GEN:APPKIT", DISPATCHERS + "\n" + impls)
    patch(APPKIT, "// GEN:REGS", regs)
    verified = sorted(n for n in s if n in NATIVE)
    print(f"contract surface: {len(s)} portable slots (all safe no-ops when unbacked)")
    print(f"verified AppKit backings registered: {len(verified)}")
    for n in verified:
        sel, kind, rl = NATIVE[n]
        print(f"    {n:<24} -> {sel:<22} {kind}{' +relayout' if rl else ''}")
    missing = [n for n in NATIVE if n not in s]
    if missing:
        print(f"WARNING: NATIVE entries with no generated slot: {missing}")


if __name__ == "__main__":
    main()
