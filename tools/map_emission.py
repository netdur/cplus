#!/usr/bin/env python3
"""map_emission.py — did the MAP's promises actually reach facet's runtime tier?

THE HOLE THIS CLOSES. `ledger_map.py` sorts every row of the ledger spec into
ADOPT ("facet declares it") or DROP. Nothing ever checked the first half. For
the CONTROL rows that does not matter much: they land in modules
`gen_contract.py` owns, and `verb_coverage.py` gates every bit and field per
backend. But the RUNTIME tier — Window, Application, Page, Toolbar, TitleBar —
is hand-written (`screen.cplus`, `window.cplus`, `application.cplus`,
`window_chrome.cplus`, `app_events.cplus`), the generator does not emit a line
of it, and no gate looked at it at all.

So a row could read **ADOPT** in a document that presents itself as a manifest
and name something that has never existed. Measured when this was written: 30
of them, including `Window.X` and `Window.Y` — facet's `Chrome` cannot state a
window's POSITION, so a window cannot be opened where the application asked —
and every one of the five modal events, because facet has no sheet vocabulary
at all.

    python3 tools/map_emission.py            # the summary
    python3 tools/map_emission.py --list     # every row, by bucket
    python3 tools/map_emission.py --check    # the gate

WHY THE SEARCH IS SCOPED, and it is the whole accuracy of this tool. A plain
"does this name appear in vendor/facet/src" check is wrong in BOTH directions:

  it OVER-reports, because the map's default naming does not always survive
  curation. `Window.IsMaximizable` becomes the field `maximizable` on `Chrome`,
  not `set_is_maximizable`. Thirteen of Window's rows are that.

  it UNDER-reports, and this is the one that hides real gaps. `Window.X` maps
  to `set_x`, and `set_x` exists — on `box`, a generated CONTROL module. The
  name collides and the miss disappears. `Window.Y` and `Window.FlowDirection`
  hid behind exactly that.

So the search runs over the HAND-WRITTEN modules only, and naming drift is
recorded in `ALIASES` rather than guessed. An alias is itself checked to exist:
a row that claims facet calls it something else, naming something that is also
absent, fails.
"""
import os
import re
import sys
import glob

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FACET = os.path.join(ROOT, "vendor", "facet", "src")
# THE RUNTIME TIER IS TWO PACKAGES, and the first version of this tool searched
# only the first — which is the same mistake it was written to catch.
#
# `facet` is the pure core. The app-facing runtime verbs — the dialogs, the
# window facade, the lifecycle observers — live in `facet_runtime`, which was
# split out for exactly that reason. Searching only `vendor/facet/src` reported
# `present_window`, `alert`, `observe_backgrounding` and a dozen more as never
# written, while they sit in `runtime.cplus` with a per-platform facade each.
RUNTIME_PKG = os.path.join(ROOT, "vendor", "facet_runtime", "src")
MAP = os.path.join(ROOT, "plans", "facet", "ledger-map-draft.md")

# The tier the generator does not own. `ledger_spec.py` calls these the runtime
# types; the point here is only that no generated module answers them.
RUNTIME_TYPES = {"Window", "Application", "Page", "ContentPage", "Toolbar", "TitleBar"}


def hand_written():
    """The modules `gen_contract.py` does NOT emit.

    Read from the files rather than listed, so a module that stops being
    generated cannot quietly fall out of scope.
    """
    out = []
    for p in sorted(glob.glob(os.path.join(FACET, "*.cplus"))):
        if p.endswith("test_main.cplus"):
            continue
        head = "".join(open(p).readlines()[:8])
        if "GENERATED" not in head:
            out.append(p)
    # Every file of facet_runtime: none of it is generated, and all of it is
    # the runtime tier by definition.
    out += [p for p in sorted(glob.glob(os.path.join(RUNTIME_PKG, "*.cplus")))
            if not p.endswith("test_main.cplus")]
    return out


# The map's name, and what facet actually calls it. Every entry was checked
# against the source by hand; the tool re-checks that the right-hand side
# exists, so a stale row here fails rather than silently excusing a gap.
ALIASES = {
    # Chrome is a flat value struct, so its rows are FIELDS, not set_/get pairs.
    ("Window", "IsMaximizable"):  "maximizable",
    ("Window", "IsMinimizable"):  "minimizable",
    ("Window", "MaximumHeight"):  "max_height",
    ("Window", "MaximumWidth"):   "max_width",
    ("Window", "MinimumHeight"):  "min_height",
    ("Window", "MinimumWidth"):   "min_width",
    # The live handle answers these, under shorter names than the map's default.
    ("Window", "DisplayDensity"):        "density",
    ("Window", "IsActivated"):           "is_active",
    ("Window", "Activated"):             "on_active",
    ("Window", "Deactivated"):           "on_inactive",
    ("Window", "SizeChanged"):           "on_resize",
    ("Window", "DisplayDensityChanged"): "on_density",
    ("Window", "Destroying"):            "on_quit_fn",
    ("Window", "Created"):               "on_launch_fn",
    # The map filed these "on Chrome" and that is the wrong TIER, not a gap:
    # `Window::set_frame(Rect)` / `frame()` already move and report a live
    # window, which is what the ledger's X/Y are — settable properties on a
    # window that exists. One facet verb answers both rows.
    #
    # WHAT IS STILL MISSING, and it is smaller than "you cannot position a
    # window": `Chrome` cannot STATE a position, so a window is centred
    # (`window.cplus:377` calls `win.center()`) and can only be moved after it
    # is on screen. Opening at a chosen position needs a Chrome field and a
    # sentinel decision — 0,0 is a legitimate origin, so it cannot mean "unset"
    # the way `max_width: 0.0` means unbounded. Not invented here.
    # The tuple form names the TIER the alias lives in, because this one moves
    # between them — the row is hinted "on Chrome" and the answer is on the
    # runtime handle. Without it the check looks for `set_frame` in
    # `screen.cplus` and correctly refuses; an alias that crosses tiers has to
    # say so rather than be excused.
    ("Window", "X"): ("set_frame", "runtime"),
    ("Window", "Y"): ("set_frame", "runtime"),
    # The THEME family. facet says the app's appearance on the runtime facade
    # and the system's read on `theme`, so none of the map's four default names
    # exist and all four capabilities do.
    ("Application", "UserAppTheme"):         "set_app_appearance",
    ("Application", "RequestedTheme"):       "app_appearance",
    ("Application", "PlatformAppTheme"):     "set_is_dark_fn",
    ("Application", "RequestedThemeChanged"): "set_theme_changed_fn",
    # A page appearing and disappearing is facet's COMPONENT LIFECYCLE, with
    # the `Attach` / `Detach` reason that splits focus from visibility — a
    # distinction the ledger's two events do not make.
    ("Page", "Appearing"):    "on_attach",
    ("Page", "Disappearing"): "on_detach",
    # The TITLEBAR. `Chrome.bar` is the bar's STYLE (Native / Blended / Hidden /
    # Custom); its CONTENT rides reserved keys, because Chrome is a value struct
    # copied into every window and a subtree is not a value — window_chrome.cplus
    # names these two ledger rows outright and answers them.
    ("Window", "TitleBar"):        ("bar", "Chrome"),
    ("TitleBar", "LeadingContent"):  "titlebar_leading",
    ("TitleBar", "TrailingContent"): "titlebar_trailing",
    # The MENU and TOOLBAR of a page are not properties in facet: a Screen
    # declares its menu, and toolbar items are NODES the backend lifts out of
    # the tree (facet_appkit/window.cplus:369 `install_toolbar(win, root)`).
    ("Page", "MenuBarItems"):     "menu_items",
    ("Page", "ToolbarItems"):     ("toolbar_item", "any"),
    ("Toolbar", "ToolbarItems"):  ("toolbar_item", "any"),
    # The SHARED BAND answers these three; they are not page properties.
    # `SafeArea` is the vocabulary behind both container-area rows.
    ("Page", "BackgroundImageSource"): "set_background_image",
    ("Page", "ContainerArea"):         "set_safe_area",
    ("Page", "IgnoresContainerArea"):  "set_safe_area",
    # `App::screen(name, factory)` registers the root screen.
    ("Application", "MainPage"): "screen",
    # A page's layout changing is `services::observe_size` on the node — the
    # seam every backend fills, not a page-only event.
    ("Page", "LayoutChanged"): "observe_size",
    # `flow_direction` is on the SHARED BAND (`C_FLOW_DIRECTION`), so every node
    # has it and a window's is its ROOT's. appkit honours the bit at
    # `paint.cplus:747`. Nothing window-specific was ever needed.
    ("Window", "FlowDirection"): ("set_flow_direction", "any"),
    # facet's accent IS the `primary` role: `Color::accent()` is what an UNSET
    # `primary` falls back to (theme.cplus:100), so an app states its accent by
    # theming that role.
    ("Application", "AccentColor"): ("set_theme", "any"),
    # The app LIFECYCLE band answers these, not the window: facet fires them for
    # the process, and a component binds them with `bind_app_lifecycle`.
    # VISIBILITY, not focus — facet splits the two and these are the visibility
    # half (`E_ACTIVE` / `E_INACTIVE` are focus, and Activated/Deactivated
    # above already take them). `Backgrounding` and `Stopped` collapse onto the
    # same facet event: one is "about to leave", the other "has left", and
    # facet fires once.
    ("Window", "Backgrounding"): "E_BACKGROUND",
    ("Window", "Stopped"):       "E_BACKGROUND",
    ("Window", "Resumed"):       "E_FOREGROUND",
}


# Where a row's hint says its facet home is. The map writes "— on Chrome" /
# "— on runtime" / "— on Screen" in the note column for exactly the 30 rows
# where it matters, and honouring it is what makes this tool accurate.
#
# WITHOUT IT, `Window.X` READS AS PRESENT. Its facet name is `set_x / x()`, and
# `x()` exists — on `WindowButtons`, whose flex-modifier boilerplate lives in
# the hand-written `window_chrome.cplus`. `Window.Y` and
# `Window.FlowDirection` hid behind the same collision. Scoping the search to
# the module the hint names is the difference between 47 missing and the truth.
TIERS = {
    "Chrome":  ["screen.cplus"],
    "Screen":  ["screen.cplus"],
    "runtime": ["window.cplus", "application.cplus", "app_events.cplus",
                "runtime.cplus", "runtime_macos.cplus"],
    # For an alias whose target is a CONTROL KIND. `Page.ToolbarItems` is
    # answered by putting `toolbar_item` nodes in the tree — appkit reads the
    # toolbar out of the tree at `window.cplus:369` — and that kind lives in a
    # GENERATED module, which `hand_written()` deliberately excludes.
    "any": None,
}


def rows():
    """(type, member, band, names, tier) for every ADOPT row on the runtime tier."""
    out, cur = [], None
    for line in open(MAP):
        m = re.match(r"^## (\w+)\s*$", line)
        if m:
            cur = m.group(1)
            continue
        m = re.match(r"^\| (\S+) \| (\w+) \| \*\*ADOPT\*\* \| (.+?) \| (.*?) \|$", line)
        if m and cur in RUNTIME_TYPES:
            names = [n.strip().rstrip("()") for n in m.group(3).split("/")
                     if n.strip() and n.strip() != "—"]
            hint = re.search(r"—\s*on ([\w ]+)$", m.group(4).strip())
            out.append((cur, m.group(1), m.group(2), names,
                        hint.group(1).strip() if hint else None))
    return out


def main():
    whole = {os.path.basename(p): open(p).read() for p in hand_written()}
    everything = "\n".join(whole.values())

    generated = "\n".join(open(f).read() for f in glob.glob(os.path.join(FACET, "*.cplus"))
                          if not f.endswith("test_main.cplus"))

    def present(n, tier):
        if tier == "any":
            return re.search(r"\b" + re.escape(n) + r"\b",
                             everything + "\n" + generated) is not None
        files = TIERS.get(tier)
        src = ("\n".join(whole.get(f, "") for f in files) if files else everything)
        return re.search(r"\b" + re.escape(n) + r"\b", src) is not None

    emitted, aliased, missing, stale = [], [], [], []
    for ty, member, band, names, tier in rows():
        entry = f"{ty}.{member}"
        if any(present(n, tier) for n in names):
            emitted.append(entry)
        elif (ty, member) in ALIASES:
            alias = ALIASES[(ty, member)]
            name, where = alias if isinstance(alias, tuple) else (alias, tier)
            (aliased if present(name, where) else stale).append(f"{entry} -> {name}")
        else:
            missing.append(f"{entry} [{band}] -> {' / '.join(names)}")

    total = len(emitted) + len(aliased) + len(missing) + len(stale)
    print(f"facet runtime tier — {total} ADOPT rows on "
          f"{', '.join(sorted(RUNTIME_TYPES))}\n")
    print(f"  {len(emitted):>4}  emitted      the map's own name is in the hand-written tier")
    print(f"  {len(aliased):>4}  aliased      facet calls it something else, recorded above")
    print(f"  {len(stale):>4}  STALE ALIAS  recorded as renamed, and the new name is absent too")
    print(f"  {len(missing):>4}  missing      adopted in the map, never written")

    if "--list" in sys.argv:
        for name, rs in (("MISSING", missing), ("STALE ALIAS", stale),
                         ("ALIASED", aliased), ("EMITTED", emitted)):
            print(f"\n{name} ({len(rs)})")
            for r in rs:
                print(f"  {r}")
    elif missing or stale:
        print("\nmissing:")
        for r in missing:
            print(f"    {r}")
        for r in stale:
            print(f"    STALE ALIAS  {r}")

    if "--check" in sys.argv:
        # A FLOOR rather than zero, and rising — the same posture
        # facet_android's parity floor takes. These are not regressions to
        # guard against; they are a backlog to burn down, and the number only
        # means something if it cannot silently grow.
        FLOOR = 7
        if stale:
            print("\nFAIL: an alias names something that does not exist.", file=sys.stderr)
            return 1
        if len(missing) > FLOOR:
            print(f"\nFAIL: {len(missing)} rows adopted and never written, "
                  f"floor is {FLOOR} — the map grew a promise nobody kept.",
                  file=sys.stderr)
            return 1
        print(f"\nok: {len(missing)} missing (floor {FLOOR}), 0 stale aliases")
    return 0


if __name__ == "__main__":
    sys.exit(main())
