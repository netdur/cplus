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
    "Microsoft.Maui.Controls.DoubleCollection": "Dashes",   # a stroke dash pattern
    "float[]": "f64[]",
    "System.Collections.Generic.IList<string>": "TextList",  # an owned list of strings
    # A subtree in the description is a Node, whatever MAUI's static type.
    "object": "Node",
    "Microsoft.Maui.Controls.Element": "Node",
    "Microsoft.Maui.Controls.View": "Node",
    "Microsoft.Maui.Controls.Page": "Node",
    "Microsoft.Maui.Controls.TableRoot": "Node",
    "Microsoft.Maui.ITitleBar": "Node",
    "Microsoft.Maui.Controls.Window": "Window",
    # Stage 1 closure additions
    "Microsoft.Maui.Graphics.Point": "Point",
    "Microsoft.Maui.IView": "Node",
    "Microsoft.Maui.Controls.VisualElement": "Node",
    "Microsoft.Maui.Controls.WebViewSource": "str",
    "Microsoft.Maui.Graphics.IDrawable": "Drawable",
    "Microsoft.Maui.Controls.SwipeItems": "SwipeItem[]",
    "System.Collections.Generic.IList<Microsoft.Maui.Controls.KeyboardAccelerator!>": "Shortcut",
    "System.Collections.Generic.IList<Microsoft.Maui.Controls.MenuBarItem>": "MenuBarItem[]",
    "System.Collections.Generic.IList<Microsoft.Maui.Controls.ToolbarItem>": "ToolbarItem[]",
    "System.Collections.Generic.IEnumerable<Microsoft.Maui.Controls.ToolbarItem>": "ToolbarItem[]",
    "System.Collections.Generic.IList<Microsoft.Maui.IView!>": "Node[]",
    "System.Collections.Generic.IReadOnlyList<Microsoft.Maui.Controls.Element>": "Node[]",
    "System.Collections.ObjectModel.ObservableCollection<Microsoft.Maui.Controls.View>": "Node[]",
    "System.Collections.Generic.IReadOnlyList<Microsoft.Maui.Controls.Window!>": "Window[]",
}

# ---- MAUI enums -> facet enums. Vocabulary facet must DECLARE, not a drop. ---
ENUMS = {
    "Microsoft.Maui.TextAlignment": "TextAlign",
    "Microsoft.Maui.Controls.FontAttributes": "FontWeight",
    "Microsoft.Maui.TextTransform": "TextTransform",
    "Microsoft.Maui.TextDecorations": "TextDecoration",
    "Microsoft.Maui.LineBreakMode": "LineBreak",
    "Microsoft.Maui.ReturnType": "ReturnKey",
    "Microsoft.Maui.ScrollBarVisibility": "ScrollBars",
    "Microsoft.Maui.Aspect": "ImageFit",
    "Microsoft.Maui.FlowDirection": "FlowDirection",
    "Microsoft.Maui.ClearButtonVisibility": "ClearButton",
    "Microsoft.Maui.Keyboard": "Keyboard",
    "Microsoft.Maui.TextType": "TextFormat",
    "Microsoft.Maui.SafeAreaEdges": "SafeArea",
    "Microsoft.Maui.SwipeDirection": "SwipeDirection",
    "Microsoft.Maui.Controls.ButtonsMask": "Buttons",
    "Microsoft.Maui.Controls.Button.ButtonContentLayout": "ContentLayout",
    "Microsoft.Maui.Controls.SelectionMode": "SelectionMode",
    "Microsoft.Maui.Controls.ListViewSelectionMode": "SelectionMode",
    "Microsoft.Maui.Controls.SeparatorVisibility": "Separator",
    "Microsoft.Maui.Controls.EditorAutoSizeOption": "AutoSize",
    "Microsoft.Maui.Controls.ItemSizingStrategy": "ItemSizing",
    "Microsoft.Maui.Controls.ItemsUpdatingScrollMode": "ScrollAnchor",
    "Microsoft.Maui.Controls.TableIntent": "TableStyle",
    "Microsoft.Maui.Controls.Shapes.PenLineCap": "LineCap",
    "Microsoft.Maui.Controls.Shapes.PenLineJoin": "LineJoin",
    "Microsoft.Maui.ScrollOrientation": "ScrollAxis",
    "Microsoft.Maui.ApplicationModel.AppTheme": "Appearance",
    "Microsoft.Maui.Controls.IndicatorShape": "DotShape",
    "Microsoft.Maui.KeyboardAcceleratorModifiers": "KeyModifiers",
    "Microsoft.Maui.Controls.ToolbarItemOrder": "ToolbarPlacement",
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
    "System.Net.CookieContainer": ENGINE,
    "Microsoft.Maui.Controls.IAppLinks": ENGINE,
    "Microsoft.Maui.Controls.Internals.NavigationProxy": ENGINE,
    "Microsoft.Maui.IElementHandler": ENGINE,
    "Microsoft.Maui.Controls.Application": ENGINE,
    "Microsoft.Maui.Controls.IBindableLayout": LAYOUT,
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
                r"Handler|HandlerChanged|HandlerChanging|Visual|"
                r"InternalChildren)$"), ENGINE),
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
    # The same inversion dropped the whole of `web`'s navigation band, and a
    # web view you cannot navigate, reload or script is not one. Stage 4 item 5
    # (adopting vendor/webkit) is what surfaced it.
    ("WebView", "GoBack"): "go_back",
    ("WebView", "GoForward"): "go_forward",
    ("WebView", "Reload"): "reload",
    ("WebView", "Eval"): "eval(script:)",
    # The other half of the hybrid bridge: `on_raw_message_received` was
    # adopted and the verb that SENDS one was not, so the channel only ran one
    # way. EvaluateJavaScriptAsync / InvokeJavaScriptAsync stay dropped — they
    # return a Task, and facet has no async in the UI; a page returns a value
    # by posting a message back, which is what the channel is for.
    ("HybridWebView", "SendRawMessage"): "send_message(body:)",
    # Stage 4 found this dropped as ENGINE by the methods-band inversion, and
    # it is not engine: GraphicsView.Invalidate is how an application says its
    # drawing changed. Without it a canvas could be described once and never
    # redrawn.
    ("GraphicsView", "Invalidate"): "redraw",
    ("ProgressBar", "ProgressTo"): "animate_progress(to:duration:)",
    ("ItemsView", "ScrollTo"): "scroll_to(index:)",
    ("ListView", "ScrollTo"): "scroll_to(index:)",
    ("ListView", "BeginRefresh"): "begin_refresh",
    ("ListView", "EndRefresh"): "end_refresh",
}

# ---- the naming pass over the default snake_case ----------------------------
# naming_guideline.md: "Drop words the type or context already implies" and
# "Name a type for what it is, not the class it wraps." The default rule
# transcribes MAUI's member name, which carries MAUI's vocabulary through.
#
# Keyed by MEMBER, not by (Type, Member), because the rule is about the word:
# wherever MAUI writes FontAutoScalingEnabled, facet writes font_scales. Nine
# types declare that member and none of them means anything else. A rule that
# would read differently on a second type belongs in OVERLAY instead; the
# generator's lint fails the run if a rename leaves a name the guideline still
# rejects, so neither table can drift silently.
#
# Applied to the writes and reads bands only. Events and methods carry their
# own names (DISCRETE_EVENT_HINTS, METHOD_VOCABULARY).
RENAME = {
    # -- the trailing noun repeats what the facet TYPE already says. Stage 1
    #    renamed the enums (LineBreakMode -> LineBreak); the verbs did not
    #    follow, so each of these drifted from its own type.
    "HorizontalScrollBarVisibility": "horizontal_scroll_bars",  # ScrollBars
    "VerticalScrollBarVisibility":   "vertical_scroll_bars",    # ScrollBars
    "ClearButtonVisibility":         "clear_button",            # ClearButton
    "SeparatorVisibility":           "separator",               # Separator
    "ItemsUpdatingScrollMode":       "scroll_anchor",           # ScrollAnchor
    "ItemSizingStrategy":            "item_sizing",             # ItemSizing
    "LineBreakMode":                 "line_break",              # LineBreak
    "ReturnType":                    "return_key",              # ReturnKey
    "TextType":                      "text_format",             # TextFormat
    "FontAttributes":                "font_weight",             # FontWeight — a SCALE, not MAUI's Bold|Italic|None (stages/3 "found late" #1)
    "IndicatorsShape":               "dot_shape",               # DotShape
    "Aspect":                        "fit",                     # ImageFit
    "Orientation":                   "axis",                    # ScrollAxis
    "Intent":                        "style",                   # TableStyle
    "Order":                         "placement",               # ToolbarPlacement
    "SafeAreaEdges":                 "safe_area",               # SafeArea
    "TextDecorations":               "text_decoration",         # TextDecoration
    "StrokeDashArray":               "stroke_dash",             # Dashes

    # -- omit needless words
    "HorizontalTextAlignment": "text_align",           # horizontal is the default axis
    "VerticalTextAlignment":   "vertical_align",
    "RemainingItemsThreshold": "remaining_threshold",  # an items view's items
    "RefreshControlColor":     "refresh_color",        # "control" is the wrapped class
    "PeekAreaInsets":          "peek_insets",
    "GroupName":               "group",
    "StrokeThickness":         "stroke_width",         # pairs with border_width
    "StrokeLineCap":           "stroke_cap",
    "StrokeLineJoin":          "stroke_join",
    # IndicatorView is facet's `page_dots`: an indicator IS a dot.
    "IndicatorColor":          "dot_color",
    "SelectedIndicatorColor":  "selected_dot_color",
    "IndicatorSize":           "dot_size",
    "MaximumVisible":          "max_dots",

    # -- name for the role, not the class MAUI wraps. facet images take a path,
    #    so "Source" names a .NET type that is not in the signature.
    "ImageSource":      "image",
    "IconImageSource":  "icon",
    "ThumbImageSource": "thumb_image",

    # -- booleans: `_enabled` is what the type already says, and a reader must
    #    read as an assertion (naming_guideline.md). The setter drops any
    #    is_/has_/can_ prefix, so `set_grouped(true)` / `is_grouped()`.
    "FontAutoScalingEnabled":    "font_scales",
    "IsSpellCheckEnabled":       "checks_spelling",
    "IsTextPredictionEnabled":   "predicts_text",
    "IsBounceEnabled":           "bounces",
    "IsGroupingEnabled":         "is_grouped",      # one word with CollectionView's
    "IsPullToRefreshEnabled":    "is_refreshable",  # one word with RefreshView's
    "IsRefreshEnabled":          "is_refreshable",
    "IsSwipeEnabled":            "is_swipeable",
    "IsScrollAnimated":          "animates_scroll",
    "AnimateCurrentItemChanges": "animates_item",
    "AnimatePositionChanges":    "animates_position",
    "CascadeInputTransparent":   "cascades_input",  # input-transparency reaches descendants
    "HideSingle":                "hides_single",
    "RefreshAllowed":            "can_refresh",     # a read reads as an assertion
}

# ---- MAUI exposes a mutable collection through a getter ---------------------
# `Picker.Items` and `MenuFlyoutItem.KeyboardAccelerators` are declared as
# reads because .NET hands back an IList you then mutate. facet has no such
# shape: you write the whole value. So the band is corrected here, once, rather
# than emitting a reader nothing can fill.
BAND = {
    ("Picker", "Items"): "writes",
    ("MenuFlyoutItem", "KeyboardAccelerators"): "writes",
}

# ---- facet's own words, with no MAUI provenance -----------------------------
# INTENT: "it grows by what its applications need, and adds words of its own."
# Measured against iris in reports/facet-iris-gap-2026-08-01.md. Recorded here
# so they are declared deliberately and appear in the ledger, rather than
# turning up in a control module with nothing behind them.
#
# (owner, word, shape, why MAUI has none, disposition)
FACET_ORIGIN = [
    ("list, collection", "set_count / count()", "usize",
     "MAUI binds ItemsSource; Stage 1 dropped that as MVVM, so the imperative "
     "replacement — a row count plus a row builder — is facet's to invent",
     "Stage 2 — emitted"),
    ("list, collection", "set_row(_:ctx:)", "fn(*u8, usize) -> Node",
     "the other half of set_count: MAUI would have used a DataTemplate",
     "Stage 2 — emitted"),
    ("symbol", "symbol(icon:) / system_symbol(name:)", "a glyph",
     "MAUI has no glyph vocabulary at all — an icon is app content, not OS "
     "chrome, so facet declares it and ships the font rather than abstracting "
     "over three platforms' icon sets",
     "SHIPPED 2026-08-03 — hand-written vendor/facet/src/symbol.cplus, with "
     "the name -> codepoint table generated by tools/gen_icons.py from "
     "assets/MaterialSymbolsOutlined.ttf (4,268 icons, Apache 2.0, 927 KB). "
     "Three tiers: the bundled font (portable, checked at compile time), "
     "`system_symbol` (the OS's set — deliberately platform-specific), and an "
     "application's own font (portable, unchecked). Proved in "
     "examples/symbol_spike"),
    ("label", "wrap_label", "a label that wraps",
     "MAUI folds wrapping into Label via LineBreakMode, and so does facet",
     "NOT A CONTROL 2026-08-03 — `label(text, line_break: LineBreak::WordWrap)`. "
     "The predecessor was a separate kind only because AppKit has a second "
     "factory (`wrappingLabelWithString:` vs `labelWithString:`), and which "
     "factory to call is the backend's sentence, not the contract's"),
    ("(component)", "composer", "a multi-line input with chrome",
     "a compound widget, not a MAUI control",
     "NOT A CONTROL 2026-08-03 — `text_area` already carries the text, the "
     "placeholder, on_submit and on_text_changed; `bordered` carries the "
     "border and the shared band carries the background. An arrangement of "
     "existing words is a COMPONENT (Stage 3's tier), not a new word"),
    ("tree", "tree(root:) + TreeNode", "a hierarchical list",
     "MAUI has no tree control at all",
     "SHIPPED 2026-08-03 — hand-written vendor/facet/src/tree.cplus. GENERIC, "
     "not the file browser it came from: `path` -> `id` (stable identity) and "
     "`is_dir` -> `is_branch` (may be true before any child exists, which is "
     "what makes lazy loading expressible). The tree OWNS its model as "
     "Vec[Box[TreeNode]], flex's idiom — the predecessor malloc'd nodes and "
     "leaked them for the life of the application. Recycled underneath on "
     "every platform, but not `list`: count+index and parent->children are "
     "different protocols"),
    ("split", "split(b, axis:, position:) + Pane verbs", "a draggable divider",
     "behaviour, not layout — flex cannot own it because the divider is a "
     "control the USER drags, unlike spacer and zstack which are pure "
     "arrangement and did go to flex_layout",
     "SHIPPED 2026-08-03 — hand-written vendor/facet/src/split.cplus. "
     "`SplitAxis::Columns|Rows` names the RESULT, because a boolean `vertical` "
     "asks about the wrong object; panes are Leading/Trailing to match Insets "
     "and survive RTL. The position is clamped WHERE IT IS STORED, so an "
     "application that saves and restores reads back what was applied. "
     "Collapsing remembers the position"),
    ("any node", ".gesture(on_click:, on_hover:, …)", "raw input, as a modifier",
     "MAUI attaches behaviour by pushing recognizer objects into a "
     "GestureRecognizers collection, which Stage 1 dropped as MODEL. The 21 "
     "rows on the recognizer types themselves were ADOPTed and then reached no "
     "control, because those types are neither controls nor shared bases",
     "SHIPPED 2026-08-03 — hand-written vendor/facet/src/gestures.cplus. A "
     "MODIFIER on Node, not a control and not 21 parameters on 38 "
     "constructors: `label(\"x\").gesture(on_click: f)`. Data carries one "
     "pointer, null until a node asks. iris's `clickable` is this, and does "
     "not need to exist"),
    ("window_chrome", "window_buttons() and .window_drag()",
     "the titlebar an application draws itself",
     "platform chrome; MAUI's window is not a described tree, so there was "
     "never a row to adopt",
     "SHIPPED 2026-08-03 — hand-written vendor/facet/src/window_chrome.cplus. "
     "TWO SHAPES, deliberately: `window_buttons()` draws something so it is a "
     "control; `.window_drag()` draws nothing and only says what pointer "
     "input on a region means, so it is a modifier and lives in the gesture "
     "set beside the other things a drag can mean"),
    ("vocab", "TextSpan + Spans", "a styled run and the list of them",
     "`Spans` is adopted from FormattedString; the builder that fills it is "
     "facet's",
     "SHIPPED 2026-08-03 — `Spans` was a stub (`struct Spans { count: i64 }`), "
     "so `set_formatted_text` was a verb nothing could feed. Now a real owned "
     "list of `TextSpan` runs, each with its own size, family, style, colours, "
     "decoration and an optional link. Owned, so it is written by `take` and "
     "read a run at a time, the same shape as Picker's TextList"),
    ("tree", "select(id:) / expand(id:) / restore(expanded:selected:)",
     "selection and expansion, keyed on the id",
     "`tree` is facet's, so its verbs are too",
     "SHIPPED 2026-08-03 — the six path verbs iris recorded, now methods on "
     "the Tree cursor and named for identity rather than for paths. `restore` "
     "is one verb on purpose: EXPANSION BEFORE SELECTION, because a row in a "
     "collapsed branch is not visible and the platform drops the selection. "
     "That ordering was got wrong once and reads as 'selection randomly does "
     "not stick'"),
]

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

    ("VisualElement", "GetChildElements"): ("ADOPT", "children()", "Node[] (read-only)"),
    ("View", "GetChildElements"): ("ADOPT", "children()", "Node[] (read-only)"),
    ("Label", "GetChildElements"): ("ADOPT", "children()", "Node[] (read-only)"),
    ("ScrollView", "Children"): ("ADOPT", "children()", "Node[] (read-only)"),

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

    # A mutable collection MAUI hands back through a getter; see BAND.
    ("Picker", "Items"): ("ADOPT", "set_items / items()",
                          "TextList — the strings a popup shows"),
    ("MenuFlyoutItem", "KeyboardAccelerators"): (
        "ADOPT", "set_shortcut / shortcut()",
        "Shortcut — MAUI models a list; a menu item shows one key equivalent, "
        "and every platform that has menus agrees"),

    # ---- found by the iris audit (reports/facet-iris-gap-2026-08-01.md).
    # These four reached a control as ADOPT and were then dropped in silence by
    # the generator's SKIP_TYPES. Each is decided here, in the ledger.

    # Same rule already applied to Picker/ListView/SelectableItemsView
    # SelectedItem: a carousel's current item is its position.
    ("CarouselView", "CurrentItem"): ("DROP", "", MODEL + " — the current item "
                                      "is a position; set_position carries it"),
    # The realized views a virtualizing panel happens to be holding.
    ("CarouselView", "VisibleViews"): ("DROP", "", ENGINE + " — MAUI's realized-"
                                       "view bookkeeping; facet reads children()"),
    # Overriding the area a view is laid out in is layout, whoever asks for it.
    ("ScrollView", "LayoutAreaOverride"): ("DROP", "", LAYOUT),
    # MAUI carries the dash pattern twice: DoubleCollection for the app,
    # float[] for the handler. facet writes one and the backend reads it.
    ("Border", "StrokeDashPattern"): ("DROP", "", ENGINE + " — the handler-facing "
                                      "mirror of StrokeDashArray"),

    # One corner verb, one type. MAUI types Button/RadioButton corners as int
    # and BoxView's as a 4-corner struct; facet carries Corners everywhere.
    ("Button", "CornerRadius"): ("ADOPT", "set_corner_radius / corner_radius()", "Corners"),
    ("RadioButton", "CornerRadius"): ("ADOPT", "set_corner_radius / corner_radius()", "Corners"),
    ("ImageButton", "CornerRadius"): ("ADOPT", "set_corner_radius / corner_radius()", "Corners"),

    # Two different thresholds. MAUI names both `Threshold`: the recognizer's
    # is the distance that counts as a swipe, the view's is how far you drag
    # to reveal the actions. Distances are f64 in facet, not MAUI's uint.
    ("SwipeGestureRecognizer", "Threshold"): ("ADOPT", "set_swipe_threshold / swipe_threshold()", "f64"),
    ("SwipeView", "Threshold"): ("ADOPT", "set_reveal_threshold / reveal_threshold()", "f64"),

    # Name corrections where snake_case reads wrong at the call site.
    ("VisualElement", "IsVisible"): ("ADOPT", "set_is_visible / is_visible()", "bool"),
    ("Switch", "IsToggled"): ("ADOPT", "set_on / is_on()", "bool — reads as an assertion"),
    ("CheckBox", "IsChecked"): ("ADOPT", "set_on / is_on()", "bool — one toggle verb across toggle/checkbox/radio"),
    ("RadioButton", "IsChecked"): ("ADOPT", "set_on / is_on()", "bool"),
    ("Button", "Text"): ("ADOPT", "set_title / title()", "str — a button carries a title, not text"),
    ("Picker", "Title"): ("ADOPT", "set_title / title()", "str"),
    ("Entry", "IsPassword"): ("ADOPT", "set_is_secure / is_secure()", "bool"),
    ("Label", "MaxLines"): ("ADOPT", "set_max_lines / max_lines()", "i64"),

    # A pointer interaction: start/end/cancel happen once, drag and hover-move
    # keep happening. No suffix rule can split those, so they are split here.
    ("GraphicsView", "StartInteraction"): ("ADOPT", "on_press", "ctx-trailing callback"),
    ("GraphicsView", "EndInteraction"): ("ADOPT", "on_release", "ctx-trailing callback"),
    ("GraphicsView", "CancelInteraction"): ("ADOPT", "on_cancel", "ctx-trailing callback"),
    ("GraphicsView", "StartHoverInteraction"): ("ADOPT", "on_hover", "ctx-trailing callback"),
    ("GraphicsView", "EndHoverInteraction"): ("ADOPT", "on_unhover", "ctx-trailing callback"),

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

# `on_` for something that happens once, `observe_` for something that keeps
# happening. The distinction decides whether a row is a build-time handler or
# an events::Subscription, so a word missing from this list is not a cosmetic
# slip: it makes the row an `observe_` that Stage 2 cannot emit at all.
DISCRETE_EVENT_HINTS = ("Clicked", "Pressed", "Released", "Completed", "Tapped",
                        "Focused", "Unfocused", "Changed", "Toggled", "Selected",
                        "Appearing", "Disappearing", "Started", "Reached",
                        "Swiped", "Updated", "Drop", "Leave", "Over", "Canceled",
                        "Pushed", "Popped", "Pushing", "Popping", "Refreshing",
                        "Opened", "Closed",
                        "Entered", "Exited", "Moved", "Starting", "Created",
                        "Destroying", "Activated", "Deactivated", "Resumed",
                        "Stopped", "Backgrounding",
                        # found by Stage 2's row guard: each of these was
                        # becoming an observe_ and then vanishing in silence.
                        "Invoked", "Requested", "Received", "Terminated",
                        "Navigated", "Navigating", "Initialized", "Initializing",
                        "Ended")


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
        base = RENAME.get(member) or snake(member)
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
        for declared in ("writes", "reads", "events", "methods"):
            for member, vt in bands.get(declared, {}).items():
                valuety = vt if isinstance(vt, str) else ""
                band = BAND.get((ty, member), declared)
                r = OVERLAY.get((ty, member)) or default_row(ty, member, band, valuety)
                if r is None:
                    undecided.append((ty, member, band, valuety))
                    continue
                if ty == "Window" and r[0] == "ADOPT":
                    r = (r[0], r[1], f"{r[2]} — on {window_owner(member, band)}")
                out.append((ty, member, band, valuety) + r)
                # stages/3 "found late" #1: MAUI's FontAttributes folds weight
                # and slant into one enum, so "semibold italic" is unsayable.
                # facet splits the axes: the row above became a WEIGHT scale,
                # and every type carrying it gains a separate is_italic row.
                if member == "FontAttributes" and r[0] == "ADOPT":
                    out.append((ty, "FontAttributes (italic axis)", band, "bool",
                                "ADOPT", "is_italic",
                                "bool — facet-origin: the slant axis MAUI folded into "
                                "FontAttributes, split out so weight and slant compose"))
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

    out.append("\n## facet's own words\n\n")
    out.append("No MAUI provenance. INTENT: \"it grows by what its applications\n"
               "need, and adds words of its own.\" Measured against iris in\n"
               "`reports/facet-iris-gap-2026-08-01.md` and declared here so the\n"
               "ledger names every word facet has, not only the borrowed ones.\n\n")
    out.append("| owner | word | shape | why MAUI has none | disposition |\n")
    out.append("|---|---|---|---|---|\n")
    for owner, word, shape, why, when in FACET_ORIGIN:
        out.append(f"| {owner} | `{word}` | {shape} | {why} | {when} |\n")
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
