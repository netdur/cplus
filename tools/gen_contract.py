#!/usr/bin/env python3
"""gen_contract.py v2 — emit facet's generated contract as a WHOLE FILE.

maui-regen Phase 2. The v1 generator patched marker regions inside
hand-written files; that interleaving is what let 67 silently-no-op slots
drift in. v2 owns entire files instead (legal since EXT.1, the same-package
impl-extension feature): regeneration is file replacement, and facet.cplus
never carries generated code again.

Outputs:
  vendor/facet/src/contract.cplus   hook statics + registrars (backend-facing;
                                    apps never import this module) + one
                                    `impl core::Handle` extension block with
                                    the app-facing methods
  vendor/facet/docs/contract.md     the generated manifest — the answer to
                                    "does facet have X?" is one grep of this

Emission policy (the no-silent-slot rule): a slot is emitted ONLY when at
least one backend wires it with a verified backing. Everything else stays in
the MAP as ADOPT/FUTURE and lands here when its backing + probe arrive
(Phase 3). UNSUPPORTED-per-backend is recorded in the manifest, and a backend
impl may return false at runtime for a kind mismatch — but a slot with NO
backing anywhere emits NO method at all: a compile error at the call site
beats a silent no-op.

The SLOTS table below is the v2 seed: exactly the surface that was wired and
verified in v1 (minus set_is_visible — killed in favor of the hand-written
set_hidden, one visibility verb, per the MAP). Each entry is checked against
tools/maui_map.py so a slot the MAP dropped cannot be emitted.
"""
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import maui_map  # noqa: E402 — the curated MAP is the authority

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

# slot name -> (facet value type, MAUI (Type, Member) provenance, appkit status note)
# appkit status: "wired" or "UNSUPPORTED: reason" — manifest content, and the
# wired flag is what permits emission at all.
SLOTS = [
    ("set_background_color", "Color", ("VisualElement", "Background"), "wired: setBackgroundColor:"),
    ("set_is_enabled", "bool", ("VisualElement", "IsEnabled"), "wired: setEnabled:"),
    ("set_is_focused", "bool", ("VisualElement", "IsFocused"), "wired: responder chain (hand impl — focus is a behavior, not a setter)"),
    ("set_is_read_only", "bool", ("InputView", "IsReadOnly"), "wired: setEditable: (inverted)"),
    ("set_opacity", "f64", ("VisualElement", "Opacity"), "wired: setAlphaValue:"),
    ("set_placeholder", "str", ("Entry", "Placeholder"), "wired: setPlaceholderString:"),
    ("set_progress", "f64", ("ProgressBar", "Progress"), "wired: setDoubleValue:"),
    ("set_text_color", "Color", ("Label", "TextColor"), "wired: setTextColor:"),
    ("set_title", "str", ("Button", "Text"), "wired: setTitle:"),
    ("set_minimum", "f64", ("Slider", "Minimum"), "wired: setMinValue:"),
    ("set_maximum", "f64", ("Slider", "Maximum"), "wired: setMaxValue:"),
    ("set_minimum_track_color", "Color", ("Slider", "MinimumTrackColor"), "wired: setTrackFillColor:"),
    ("set_maximum_track_color", "Color", ("Slider", "MaximumTrackColor"), "UNSUPPORTED on AppKit: NSSlider has no equivalent"),
    ("set_thumb_color", "Color", ("Slider", "ThumbColor"), "UNSUPPORTED on AppKit: NSSlider has no equivalent"),
    ("set_thumb_image", "str", ("Slider", "ThumbImageSource"), "UNSUPPORTED on AppKit: NSSlider has no equivalent"),
    # ---- Phase 3 expansion: every slot below ships with a GENERATED impl and
    # a GENERATED round-trip probe test in contract_appkit.cplus.
    ("set_max_lines", "i64", ("Label", "MaxLines"), "wired: setMaximumNumberOfLines: (probe-verified)"),
    ("set_selected_index", "i64", ("Picker", "SelectedIndex"), "wired: selectItemAtIndex: (probe-verified)"),
    ("set_increment", "f64", ("Stepper", "Increment"), "wired: setIncrement: (probe-verified)"),
]

# ---- Phase 3 NATIVE table: slot -> generated AppKit backing + probe.
# kind picks the ABI-correct send; ctor/setup build the probe widget headless;
# probe_value round-trips through the raw getter. A slot NOT here keeps its
# hand backing (the v1 seed) or is UNSUPPORTED.
NATIVE = {
    "set_max_lines": dict(
        kind="i64", set_sel="setMaximumNumberOfLines:", get_sel="maximumNumberOfLines",
        relayout=True,
        # The widget must OUTLIVE the probe: .payload()/.raw() on a temporary
        # drops the owner at statement end and frees the view (the temp-drop
        # rule) — the segfault class this comment exists to prevent.
        decl="var probe_w: flex::Node = ui::label(\"probe\", size: 13.0f64);",
        view="probe_w.payload()",
        setup=[],
        probe_value="3 as i64", probe_expect="3 as i64",
    ),
    "set_selected_index": dict(
        kind="i64", set_sel="selectItemAtIndex:", get_sel="indexOfSelectedItem",
        relayout=False,
        decl="let probe_w: ak::PopUpButton = ak::PopUpButton::new(rt::Rect { x: 0.0, y: 0.0, w: 0.0, h: 0.0 }, false);",
        view="probe_w.raw()",
        setup=[
            'rt::msg_void_id(v, rt::sel(#str_ptr("addItemWithTitle:\\0")), bridge::nsstring("a"));',
            'rt::msg_void_id(v, rt::sel(#str_ptr("addItemWithTitle:\\0")), bridge::nsstring("b"));',
            'rt::msg_void_id(v, rt::sel(#str_ptr("addItemWithTitle:\\0")), bridge::nsstring("c"));',
        ],
        probe_value="1 as i64", probe_expect="1 as i64",
    ),
    "set_increment": dict(
        kind="f64", set_sel="setIncrement:", get_sel="increment",
        relayout=False,
        decl="let probe_w: ak::Stepper = ak::Stepper::new();",
        view="probe_w.raw()",
        setup=[],
        probe_value="5.0f64", probe_expect="5.0f64",
    ),
}

SEND = {
    "i64": "rt::msg_void_i64(view, s, v);",
    "f64": "rt::msg_void_f64(view, s, v);",
}
GETTER = {
    "i64": lambda sel: f'rt::msg_i64(v, rt::sel(#str_ptr("{sel}\\0")))',
    "f64": lambda sel: f'rt::msg_f64(v, rt::sel(#str_ptr("{sel}\\0")))',
}


def emit_backend():
    lines = [
        "// GENERATED by tools/gen_contract.py — DO NOT EDIT.\n",
        "// facet_appkit's generated backings: one impl per NATIVE-table slot,\n",
        "// registered by install_generated() (called from the runtime facade\n",
        "// beside backend::install()), each verified by its own round-trip\n",
        "// probe test below — write through the Handle, read back through the\n",
        "// raw getter. A slot that cannot pass its probe does not ship.\n",
        "\n",
        'import "facet/facet" as facet;\n',
        'import "facet/contract" as contract;\n',
        'import "objc/runtime" as rt;\n',
        'import "objc/bridge" as bridge;\n',
        'import "appkit/appkit" as ak;\n',
        'import "flex_layout/flex_layout" as flex;\n',
        'import "./facet_appkit" as impl_mod;\n',
        'import "./ui" as ui;\n',
        "\n",
    ]
    gen = [(n, t, pr, note) for (n, t, pr, note) in SLOTS if n in NATIVE]
    for name, ty, prov, note in gen:
        nt = NATIVE[name]
        lines.append(f"// {prov[0]}.{prov[1]} -> {nt['set_sel']}\n")
        lines.append(f"fn g_{name}(h: facet::Handle, v: {FNTY[ty].replace('core::', 'facet::')}) -> bool {{\n")
        lines.append("    let view: *u8 = h.view();\n")
        lines.append("    if view == (0 as *u8) { return false; }\n")
        lines.append(f'    let s: *u8 = rt::sel(#str_ptr("{nt["set_sel"]}\\0"));\n')
        lines.append("    if !impl_mod::slot_responds(view, s) { return false; }\n")
        lines.append(f"    {SEND[nt['kind']]}\n")
        if nt["relayout"]:
            lines.append("    impl_mod::slot_relayout(h, view);\n")
        lines.append("    return true;\n}\n\n")
    lines.append("// Register every generated backing. The runtime facade calls this right\n")
    lines.append("// beside backend::install().\n")
    lines.append("fn install_generated() {\n")
    for name, ty, prov, note in gen:
        lines.append(f"    contract::{name}_fn(g_{name});\n")
    lines.append("    return;\n}\n\n")
    for name, ty, prov, note in gen:
        nt = NATIVE[name]
        lines.append("#[test]\n")
        lines.append(f"fn probe_{name}_round_trips() {{\n")
        lines.append(f"    contract::{name}_fn(g_{name});\n")
        lines.append(f"    {nt['decl']}\n")
        lines.append(f"    let v: *u8 = {nt['view']};\n")
        for su in nt["setup"]:
            lines.append(f"    {su}\n")
        lines.append("    let h: facet::Handle = facet::Handle::of(v, 0 as *u8, 0 as *u8, 0 as u32);\n")
        lines.append(f"    let _r: facet::Handle = h.{name}({nt['probe_value']});\n")
        lines.append(f"    assert {GETTER[nt['kind']](nt['get_sel'])} == ({nt['probe_expect']});\n")
        lines.append("}\n\n")
    return "".join(lines)

FNTY = {"Color": "core::Color", "f64": "f64", "bool": "bool", "str": "str", "i64": "i64"}


def map_status(prov):
    """The MAP's verdict for a (Type, Member) row — emission guard."""
    row = maui_map.OVERLAY.get(prov)
    if row:
        return row[0]
    return "ADOPT"  # default-adoptable band; the MAP would have dropped it otherwise


def emit_contract():
    lines = [
        "// GENERATED by tools/gen_contract.py — DO NOT EDIT.\n",
        "// facet's generated contract module: hook statics + registrars\n",
        "// (backend-facing — apps never import facet/contract) and the\n",
        "// app-facing Handle methods, attached through a same-package impl\n",
        "// extension (EXT.1). Regenerate with:\n",
        "//   python3 tools/gen_contract.py\n",
        "// The slot list is tools/gen_contract.py's SLOTS table, guarded by\n",
        "// the curated MAP (tools/maui_map.py). docs/contract.md is the\n",
        "// manifest twin of this file.\n",
        "\n",
        "import \"./facet\" as core;\n",
        "\n",
    ]
    for name, ty, prov, note in SLOTS:
        st = map_status(prov)
        if st == "DROP":
            raise SystemExit(f"slot {name} is DROP in the MAP; refusing to emit")
        f = FNTY[ty]
        up = name.upper()
        lines.append(f"// {prov[0]}.{prov[1]} — {note}\n")
        lines.append(f"static {up}_FN: fn(core::Handle, {f}) -> bool = #zero::[fn(core::Handle, {f}) -> bool]();\n")
        lines.append(f"fn {name}_fn(f: fn(core::Handle, {f}) -> bool) {{ {up}_FN = f; return; }}\n\n")
    lines.append("impl core::Handle {\n")
    for name, ty, prov, note in SLOTS:
        f = FNTY[ty]
        up = name.upper()
        lines.append(f"    // {prov[0]}.{prov[1]}. Chainable; a no-op on a miss, a kind\n")
        lines.append("    // without the property, or a backend that has not wired it.\n")
        lines.append(f"    fn {name}(this, v: {f}) -> core::Handle {{\n")
        lines.append(f"        let f: fn(core::Handle, {f}) -> bool = {up}_FN;\n")
        lines.append(f"        if f != (0 as fn(core::Handle, {f}) -> bool) {{ let _ok: bool = f(this, v); }}\n")
        lines.append("        return this;\n")
        lines.append("    }\n\n")
    lines.append("}\n")
    return "".join(lines)


def emit_manifest():
    lines = [
        "# facet contract manifest — GENERATED by tools/gen_contract.py\n\n",
        "Every generated contract slot, its type, its MAUI provenance, and its\n",
        "per-backend status. A slot absent from this file does not exist —\n",
        "calling it is a compile error, never a silent no-op. The hand-written\n",
        "contract (find/set_text/tree/size/observe_size/...) lives in\n",
        "facet.cplus and is documented in ref.md; this file is the generated\n",
        "band only.\n\n",
        "| slot | type | MAUI provenance | AppKit |\n",
        "|---|---|---|---|\n",
    ]
    for name, ty, prov, note in SLOTS:
        lines.append(f"| `{name}` | {ty} | {prov[0]}.{prov[1]} | {note} |\n")
    lines.append("\nGTK: nothing wired yet — every slot above is UNSUPPORTED there.\n")
    return "".join(lines)


def main():
    cp = os.path.join(ROOT, "vendor", "facet", "src", "contract.cplus")
    with open(cp, "w") as f:
        f.write(emit_contract())
    bp = os.path.join(ROOT, "vendor", "facet_appkit", "src", "contract_appkit.cplus")
    with open(bp, "w") as f:
        f.write(emit_backend())
    mp = os.path.join(ROOT, "vendor", "facet", "docs", "contract.md")
    with open(mp, "w") as f:
        f.write(emit_manifest())
    print(f"wrote {os.path.relpath(cp, ROOT)} ({len(SLOTS)} slots), "
          f"{os.path.relpath(bp, ROOT)} ({len(NATIVE)} generated backings) + docs/contract.md")


if __name__ == "__main__":
    main()
