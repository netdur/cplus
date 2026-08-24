#!/usr/bin/env python3
"""ledger_map.py — the curated MAP: the ledger surface -> facet contract names.

Stage 1 of the bootstrap. facet is an EMPTY package: it has no verbs, so no
row can claim "facet already has this", and no row may be deleted because a
platform cannot do it — that sentence belongs to facet_appkit, which answers
the contract, not to facet, which writes it.

So every row of the ledger spec (plans/facet/spec/ledger-spec.json, six bands)
lands in exactly ONE of two buckets:

  ADOPT   facet declares it, under a facet name (naming_guideline.md)
  DROP    the ledger's model, not facet's — three reasons, never any other:
            MODEL   MVVM/binding/templates/identity. facet describes UI with
                    components, keys, and fn-ptr handlers.
            LAYOUT  layout is flex_layout's. facet Nodes carry flex modifiers.
            ENGINE  the ledger's own internals (Map*/Send*/On*/measure/arrange/
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
SPEC = os.path.join(ROOT, "plans", "facet", "spec", "ledger-spec.json")

# ---- the three DROP reasons. This list does not grow. -----------------------
MODEL = "the ledger's MVVM model — facet describes UI with components, keys, and fn-ptr handlers"
LAYOUT = "layout belongs to flex_layout — facet Nodes carry flex modifiers"
ENGINE = "the ledger engine internals, not application vocabulary"

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
    "Color": "Color",
    "Rect": "Rect",
    "Size": "Size",
    "CornerRadius": "Corners",
    "Thickness": "Insets",
    "Paint": "Brush",
    "Brush": "Brush",
    "Shadow": "Shadow",
    "ImageSource": "str",          # facet images take a path
    "FormattedString": "Spans",    # styled runs of text
    "Shapes.Geometry": "Shape",
    "IShape": "Shape",
    "DoubleCollection": "Dashes",   # a stroke dash pattern
    "float[]": "f64[]",
    "System.Collections.Generic.IList<string>": "TextList",  # an owned list of strings
    # A subtree in the description is a Node, whatever the ledger's static type.
    "object": "Node",
    "Element": "Node",
    "View": "Node",
    "Page": "Node",
    "TableRoot": "Node",
    "ITitleBar": "Node",
    "Window": "Window",
    # Stage 1 closure additions
    "Point": "Point",
    "IView": "Node",
    "VisualElement": "Node",
    "WebViewSource": "str",
    "IDrawable": "Drawable",
    "SwipeItems": "SwipeItem[]",
    "System.Collections.Generic.IList<KeyboardAccelerator!>": "Shortcut",
    "System.Collections.Generic.IList<MenuBarItem>": "MenuBarItem[]",
    "System.Collections.Generic.IList<ToolbarItem>": "ToolbarItem[]",
    "System.Collections.Generic.IEnumerable<ToolbarItem>": "ToolbarItem[]",
    "System.Collections.Generic.IList<IView!>": "Node[]",
    "System.Collections.Generic.IReadOnlyList<Element>": "Node[]",
    "System.Collections.ObjectModel.ObservableCollection<View>": "Node[]",
    "System.Collections.Generic.IReadOnlyList<Window!>": "Window[]",
}

# ---- the ledger enums -> facet enums. Vocabulary facet must DECLARE, not a drop. ---
ENUMS = {
    "TextAlignment": "TextAlign",
    "FontAttributes": "FontWeight",
    "TextTransform": "TextTransform",
    "TextDecorations": "TextDecoration",
    "LineBreakMode": "LineBreak",
    "ReturnType": "ReturnKey",
    "ScrollBarVisibility": "ScrollBars",
    "Aspect": "ImageFit",
    "FlowDirection": "FlowDirection",
    "ClearButtonVisibility": "ClearButton",
    "Keyboard": "Keyboard",
    "TextType": "TextFormat",
    "SafeAreaEdges": "SafeArea",
    "SwipeDirection": "SwipeDirection",
    "ButtonsMask": "Buttons",
    "Button.ButtonContentLayout": "ContentLayout",
    "SelectionMode": "SelectionMode",
    "ListViewSelectionMode": "SelectionMode",
    "SeparatorVisibility": "Separator",
    "EditorAutoSizeOption": "AutoSize",
    "ItemSizingStrategy": "ItemSizing",
    "ItemsUpdatingScrollMode": "ScrollAnchor",
    "TableIntent": "TableStyle",
    "Shapes.PenLineCap": "LineCap",
    "Shapes.PenLineJoin": "LineJoin",
    "ScrollOrientation": "ScrollAxis",
    "AppTheme": "Appearance",
    "IndicatorShape": "DotShape",
    "KeyboardAcceleratorModifiers": "KeyModifiers",
    "ToolbarItemOrder": "ToolbarPlacement",
}

# ---- types that carry the ledger's model, wherever they appear ---------------------
DROP_TYPES = {
    "System.Windows.Input.ICommand": MODEL,
    "BindingBase": MODEL,
    "DataTemplate": MODEL,
    "ControlTemplate": MODEL,
    "ResourceDictionary": MODEL,
    "System.Collections.Generic.IList<Behavior>": MODEL,
    "System.Collections.Generic.IList<TriggerBase>": MODEL,
    "System.Collections.IEnumerable": MODEL,
    "System.Collections.IList": MODEL,
    "System.Collections.Generic.IList<object>": MODEL,
    "System.Collections.Generic.IList<IGestureRecognizer>": MODEL,
    "LayoutOptions": LAYOUT,
    "IItemsLayout": LAYOUT,
    "IViewHandler": ENGINE,
    "IVisual": ENGINE,
    "Internals.IGestureController": ENGINE,
    "Internals.AutoId": ENGINE,
    "Internals.TableModel": ENGINE,
    "ListViewCachingStrategy": ENGINE,
    "System.Collections.Generic.IReadOnlyCollection<IWindowOverlay!>": ENGINE,
    "IVisualDiagnosticsOverlay": ENGINE,
    "System.Net.CookieContainer": ENGINE,
    "IAppLinks": ENGINE,
    "Internals.NavigationProxy": ENGINE,
    "IElementHandler": ENGINE,
    "Application": ENGINE,
    "IBindableLayout": LAYOUT,
}

# What `CommandParameter` became. Not a drop reason — a POINTER, so the map
# stops contradicting the code.
#
# `Command` is `ICommand` and is MVVM by definition: the view-model IS the
# command, so dropping it is right and the reason above says so. For a long time
# `CommandParameter` was dropped by the SAME regex — `Command(Parameter)?$` —
# and inherited that reason word for word. It is not the same thing. `Command`
# is the handler; `CommandParameter` is the ARGUMENT the handler needs, and
# facet's replacement for `Command` (a fn-pointer plus a ctx slot) does not
# carry one, because a bound method consumes the ctx slot for its receiver
# (E0824).
#
# So facet built it anyway, from first principles, and did not name it after
# this row: `core::set_item` / `core::item_of`, whose own comment reads "an item
# IS the payload half of a handler" — which is this row's definition. One
# concept, decided absent in the map and present in the tree.
#
# It stays out of the props band, and that part of the old decision was right:
# a `CommandParameter` is not per-control state, it is one payload per NODE, so
# it belongs on the shared band beside `key` — key is the ADDRESS, item is the
# PAYLOAD. `gen_contract.ctor_params` names it at construction for the controls
# whose ledger type declares this row, which is what makes it reachable where
# the handler is set instead of through a second statement afterwards.
PARAM = ("the payload half of a handler — facet carries it on the shared band as "
         "the node's `item`, named at construction and read with "
         "`component::item_of(sender)`; see gen_contract.ctor_params")

# ---- member-name rules, applied before the type rules -----------------------
DROP_PATTERNS = [
    (re.compile(r"CommandParameter$"), PARAM),
    (re.compile(r"Command$"), MODEL),
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

# ---- what an implementation claim COVERS ------------------------------------
#
# `METHOD_DROPS` says "facet says it as `runtime::prompt`", and until now that
# claim was checked at the METHOD level only: the verb exists, so the row is
# answered. It says nothing about the row's PARAMETERS, and that is where the
# drift hides.
#
# It hid twice. `MenuItem.CommandParameter` was a property rather than a
# parameter, but the same shape — a row claimed and a part of it missing. Then
# `DisplayPromptAsync`'s `initialValue` was absent from `runtime::prompt`, so a
# rename sheet opened on nothing and every application that renames anything
# wrote three lines to reach into the dialog and set the field itself.
#
# So a claim on a row with three or more parameters has to account for each one.
# `carried as X` or `absent: why` — either is fine, and neither can be silence.
# Three is the threshold because that is where a parameter can hide: a one-arg
# verb either takes its argument or obviously does not.
#
# `gen_contract`'s guard 8 fails the run on a covered row with no entry here, or
# an entry that misses a parameter, and prints the `absent` ones on every regen
# so a debt stays visible rather than becoming the shape of the API.
IMPLEMENTED_PARAMS = {
    ("Page", "DisplayAlert"): {
        "title":   "carried as `title`",
        "message": "carried as `message`",
        "cancel":  "carried as `secondary` — facet names the two buttons "
                   "`primary`/`secondary` rather than accept/cancel, because an "
                   "alert with one button has a primary and no cancel",
    },
    ("Page", "DisplayAlertAsync"): {
        "title":   "carried as `title`",
        "message": "carried as `message`",
        "cancel":  "carried as `secondary`",
    },
    ("Page", "DisplayActionSheet"): {
        "title":       "carried as `title`",
        "buttons":     "carried as `options`, a `vec::Vec[text::Text]` the node owns",
        "cancel":      "absent: a choose sheet has no cancel button, so Escape has "
                       "nothing to bind to either. NOT a decision — the row asks for "
                       "one and facet does not have it. See the NOT CARRIED report.",
        "destruction": "absent: no way to mark one option destructive, so the "
                       "delete-shaped choice looks like every other. `menu_item` "
                       "carries `destructive` one control over.",
    },
    ("Page", "DisplayActionSheetAsync"): {
        "title":       "carried as `title`",
        "buttons":     "carried as `options`",
        "cancel":      "absent: as DisplayActionSheet",
        "destruction": "absent: as DisplayActionSheet",
    },
    ("Page", "DisplayPromptAsync"): {
        "title":        "carried as `title`",
        "message":      "carried as `message`",
        "accept":       "carried as `primary`",
        "cancel":       "carried as `secondary`",
        "placeholder":  "carried as `placeholder` — the hint shown while the field "
                        "is EMPTY, which is not `initialValue` below",
        "initialValue": "carried as `initial` — what the field STARTS with and what "
                        "a person then edits. Added 2026-08-24; without it a rename "
                        "opened on nothing.",
        "maxLength":    "absent: facet's `text_field` has no length limit of its own, "
                        "so the dialog has nowhere to put one. It belongs on the "
                        "control before it belongs here.",
        "keyboard":     "absent: a keyboard TYPE is a touch idiom — macOS has one "
                        "keyboard. `text_field` would carry it for the iOS backend "
                        "the same way `swipeable` is decided per platform.",
    },
    ("ScrollView", "ScrollToAsync"): {
        "element":  "absent: facet scrolls to an OFFSET, not to a descendant — "
                    "`ScrollX`/`ScrollY` are writes on the control",
        "position": "absent: no scroll-to-element, so no position within it",
        "animated": "absent: as above",
    },
}

# ---- the methods band decides, it does not default -------------------------
#
# Writes and reads are OPT-OUT: an unmapped type returns None and this script
# fails, so a row cannot leave the ledger without someone writing a reason. The
# methods band was OPT-IN and had no such floor — anything outside
# METHOD_VOCABULARY fell through to `DROP as ENGINE`, and 237 of 256 rows took
# that reason without anybody reading them.
#
# It was not a harmless label. `ScrollView.ScrollToAsync` is the verb an
# application needs to restore a scroll position, and it carried "the ledger
# engine internals, not application vocabulary" until iris hit the gap twice.
# Four more rows said the same thing about capabilities facet HAD BUILT
# elsewhere, so the contract answered "no" for verbs that existed.
#
# So the plumbing is a RULE, written below and matchable, and everything else is
# a judgement in METHOD_DROPS. A row in neither now fails the run, exactly as an
# unmapped write does.
ENGINE_METHODS = re.compile(
    r"^("
    # the ledger's own property/handler plumbing
    r"On[A-Z]\w*|Map[A-Z]\w*|Should[A-Z]\w*|Update[A-Z]\w*|Send[A-Z]\w*|"
    r"Raise[A-Z]\w*|Lower[A-Z]\w*|Propagate[A-Z]\w*|Notify[A-Z]\w*|"
    r"Validate[A-Z]\w*|Unhook[A-Z]\w*|Refresh[A-Z]\w*Property|"
    r"Platform[A-Z]\w*|Invalidate[A-Z]\w*|Dispose|CleanUp|"
    r"\w*DefaultValueCreator|\w*Changed|SizeAllocated|"
    # per-control internals: visual-state machines, default cell factories,
    # and the engine's own is-pressed / is-dragging / scrolled-position writes
    r"ChangeVisualState|CreateDefault\w*|SetupContent|GetDisplayTextFromGroup|"
    r"GetScrollPositionForElement|SetScrolledPosition|Measure|SetIs[A-Z]\w*|"
    # IList surface on a collection the application never holds
    r"Contains|CopyTo|IndexOf|GetEnumerator"
    r")$")

# Methods that are the layout engine's, not the ledger's bookkeeping. Same verdict
# (DROP), honest reason: flex_layout arranges and measures.
LAYOUT_METHODS = re.compile(
    r"^(Arrange|ArrangeOverride|Layout|LayoutChildren|MeasureOverride|OnMeasure|"
    r"ComputeConstraintForView|CrossPlatformArrange|CrossPlatformMeasure|"
    r"InvalidateMeasureNonVirtual|InvalidateMeasureOverride)$")

# ---- the methods band inverts: most rows are the ledger's own engine, so the
# APPLICATION verbs are whitelisted and everything else drops as ENGINE.
#
# The ledger repeats a member on every control that overrides it (OnMeasure x7,
# UpdateFormsText x7, MapText x5); facet has ONE method on Node, so those
# repetitions collapse rather than multiply. Collapsing is not promoting: a
# binding-context hook is still a hook however many types declare it. What
# DOES promote is a capability no adopted row already carries — relayout,
# measure, batched updates, children, and the native handle below.
# Method rows that are NOT plumbing, judged one at a time. The reason is the
# point: "engine internals" was false for every row in the first group, and the
# contract repeated it until an application proved otherwise.
#
# FACET SAYS IT ANOTHER WAY. These are not refusals — the capability exists, and
# the reason names where, so the contract can answer "does facet do this".
METHOD_DROPS = {
    # The cookie store is asynchronous everywhere it exists — WKHTTPCookieStore
    # answers through a completion handler — and facet has no async in the UI.
    # Same rule that dropped EvaluateJavaScriptAsync, and the same escape: a
    # page that knows something posts it back through the message channel.
    ("WebView", "Cookies"):
        "the platform cookie store answers asynchronously; facet has no async in the UI",
    # `page_dots` DRAWS the row as sublayers rather than laying out child views,
    # so there is nothing for an items layout to arrange.
    ("IndicatorView", "IndicatorLayout"):
        "the dots are drawn as sublayers, not arranged children",
    ("Application", "ActivateWindow"):
        "facet says it as application::activate_window",
    ("Page", "DisplayActionSheet"):
        "facet says it as runtime::choose — a sheet of keyed buttons an agent can pick from",
    ("Page", "DisplayActionSheetAsync"):
        "facet says it as runtime::choose — no async in the UI; the answer is a handler",
    ("Page", "DisplayPromptAsync"):
        "facet says it as runtime::prompt — no async in the UI; the answer is a handler",
    ("SemanticProperties", "SetDescription"):
        "facet says it as set_accessibility_label on the shared band",
    ("SemanticProperties", "GetDescription"):
        "facet says it as accessibility_label on the shared band",
    ("SemanticProperties", "SetHint"):
        "facet says it as set_accessibility_hint on the shared band",
    ("SemanticProperties", "GetHint"):
        "facet says it as accessibility_hint on the shared band",
    ("SemanticProperties", "SetHeadingLevel"):
        "facet says it as set_heading_level on the shared band",
    ("SemanticProperties", "GetHeadingLevel"):
        "facet says it as heading_level on the shared band",
    ("ToolTipProperties", "SetText"):
        "facet says it as set_tooltip on the shared band",
    ("ToolTipProperties", "GetText"):
        "facet says it as tooltip on the shared band",

    ("Application", "Quit"):        "facet says it as nav::quit()",
    ("Application", "OpenWindow"):  "facet says it as application::open_window",
    ("Application", "CloseWindow"): "facet says it as application::close_window",
    ("Page", "DisplayAlert"):       "facet says it as runtime::alert",
    ("Page", "DisplayAlertAsync"):  "facet says it as runtime::alert — no async in the UI",
    ("Page", "ForceLayout"):        "facet says it as relayout, adopted from VisualElement.InvalidateMeasure",
    ("RadioButton", "ContentAsString"):
        "the OVERLAY adopts RadioButton.Content as set_text / text(); this is how the ledger reads it",
    # THE ONE THAT COST TWO GAP REPORTS. The ledger's verb returns a Task, so it
    # fell to the async rule — but the CAPABILITY is not async, and dropping the
    # method dropped the only way to write a scroll position. The offset is a
    # WRITE now (ScrollX/ScrollY promoted to the writes band), which is the same
    # capability in facet's own shape.
    ("ScrollView", "ScrollToAsync"):
        "facet says it as set_scroll_x / set_scroll_y — the offset is a write, not a command",

    # RETURNS A TASK, and facet has no async in the UI. A page that has an
    # ANSWER posts it back through the hybrid message channel, which is adopted.
    ("WebView", "EvaluateJavaScriptAsync"):
        "returns a Task; a page answers through the hybrid message channel instead",
    ("HybridWebView", "EvaluateJavaScriptAsync"):
        "returns a Task; a page answers through the hybrid message channel instead",
    ("HybridWebView", "InvokeJavaScriptAsync"):
        "returns a Task; a page answers through the hybrid message channel instead",

    # REFUSED ON PRINCIPLE, not missing. Opening a swipe from code is the drag
    # verb wearing a different hat: it makes a reveal that only a gesture can
    # reach viable again, and an agent has no hands. A UI that needs the swipe
    # opened owes its users a button.
    ("SwipeView", "Open"):
        "the reveal is the user's gesture; a UI that needs it opened owes a button (see the no-hands rule in backend.cplus)",
    ("SwipeView", "Close"):
        "the reveal is the user's gesture; a UI that needs it closed owes a button (see the no-hands rule in backend.cplus)",

    # A MENU IS BUILT, NOT MUTATED. facet hands back a vec of MenuItem from
    # `menu_items`, so there is no live collection for an application to add to
    # or remove from — the whole set is declared each time.
    ("MenuBarItem", "Add"):      "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuBarItem", "Insert"):   "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuBarItem", "Remove"):   "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuBarItem", "RemoveAt"): "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuBarItem", "Clear"):    "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuFlyout", "Add"):       "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuFlyout", "Insert"):    "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuFlyout", "Remove"):    "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuFlyout", "RemoveAt"):  "facet declares menus as a vec of MenuItem; the collection is not live",
    ("MenuFlyout", "Clear"):     "facet declares menus as a vec of MenuItem; the collection is not live",

    # The last four, judged rather than pattern-matched because each reads like
    # application vocabulary and is not.
    ("Application", "CreateWindow"):
        "the ledger's window FACTORY hook; facet says it as runtime::App.screen + open_window",
    ("Application", "SetCurrentApplication"):
        "sets the ledger's ambient singleton; facet has no ambient app to set",
    ("Application", "SetAppIndexingProvider"):
        "Android/iOS app-indexing registration — a platform service, not UI vocabulary",
    ("Page", "GetParentWindow"):
        "facet says it as the shared band's window() / vocab::WindowRef",
}

# NOT BUILT, and said so. These are application vocabulary facet has no answer
# for — the audit that produced this table found them by asking which dropped
# rows were not plumbing, which is the question nobody had asked.
#
# Kept OUT of METHOD_DROPS deliberately: a drop is a decision, and these are
# debts. `tools/gen_contract.py` reports them, so the count is visible rather
# than dissolved into 237 rows that all claimed to be engine internals.
#
# EVERY BAND, not only methods. The same audit run over the writes and reads
# bands found four more wearing "engine internals" or "layout belongs to
# flex_layout" — and one of them, a collection's item layout, is what a board
# of cards needs.
ABSENT = {
                # NOT an overlay LAYER. flex has absolute positioning and always did, so an
    # overlay is an ordinary node that says it is out of flow and where it sits
    # — `set_absolute` plus the four edges. A registry of overlays would have
    # been a second placement system beside the one the tree already runs.
    
    # ACCESSIBILITY. The agent surface already carries a name and a description
    # per node — `identity::NodeView` has both — but facet declares no verb an
    # application can use to SET them, so they are only ever what a key and a
    # control's own text happen to say. The same three rows serve VoiceOver.
                        
        
            
    # ---- found in the WRITES and READS bands by the same audit ----
    #
    # A COLLECTION CAN ONLY BE A LIST. This dropped as "layout belongs to
    # flex_layout", and it does not: a collection arranges its OWN items, the
    # way NSCollectionViewFlowLayout does, and nothing in the flex tree reaches
    # inside it. Without this a board of cards, a gallery, or any grid of
    # anything has to be hand-built out of rows.

}

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
# transcribes the ledger's member name, which carries the ledger's vocabulary through.
#
# Keyed by MEMBER, not by (Type, Member), because the rule is about the word:
# wherever the ledger writes FontAutoScalingEnabled, facet writes font_scales. Nine
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
    "FontAttributes":                "font_weight",             # FontWeight — a SCALE, not the ledger's Bold|Italic|None (stages/3 "found late" #1)
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

    # -- name for the role, not the class the ledger wraps. facet images take a path,
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

# ---- the ledger exposes a mutable collection through a getter ---------------------
# `Picker.Items` and `MenuFlyoutItem.KeyboardAccelerators` are declared as
# reads because .NET hands back an IList you then mutate. facet has no such
# shape: you write the whole value. So the band is corrected here, once, rather
# than emitting a reader nothing can fill.
BAND = {
    ("Picker", "Items"): "writes",
    ("MenuFlyoutItem", "KeyboardAccelerators"): "writes",
    # A scroll offset the application can WRITE, not only read.
    #
    # The ledger has these read-only and moves a scroll with ScrollToAsync,
    # which returns a Task — and facet has no async in the UI, so the method
    # dropped and took the write half with it. What was left is a position that
    # reports where the view is and gives no way to put it back.
    #
    # An application needs the write for one reason above all: rebuilding the
    # content of a scroll sends it back to the origin, and restoring the offset
    # afterwards is the only way to keep a user's place.
    ("ScrollView", "ScrollX"): "writes",
    ("ScrollView", "ScrollY"): "writes",
}

# ---- facet's own words, with no the ledger provenance -----------------------------
# INTENT: "it grows by what its applications need, and adds words of its own."
# Measured against iris in reports/facet-iris-gap-2026-08-01.md. Recorded here
# so they are declared deliberately and appear in the ledger, rather than
# turning up in a control module with nothing behind them.
#
# (owner, word, shape, why the ledger has none, disposition)
FACET_ORIGIN = [
    ("list, collection", "set_count / count()", "usize",
     "the ledger binds ItemsSource; Stage 1 dropped that as MVVM, so the imperative "
     "replacement — a row count plus a row builder — is facet's to invent",
     "Stage 2 — emitted"),
    ("list, collection", "set_row(_:ctx:)", "fn(*u8, usize) -> Node",
     "the other half of set_count: the ledger would have used a DataTemplate",
     "Stage 2 — emitted"),
    ("button, icon_button, text_button", "toggles: / is_on() / set_bordered()",
     "a button that flips, and a border that switches without being forgotten",
     "the ledger has no button of any kind that FLIPS — its toggle is a CheckBox or a "
     "Switch, which draw their own control and cannot be a plain word or an "
     "icon — and no way to switch a border off without losing its width, since "
     "BorderWidth is the only dial",
     "SHIPPED 2026-08-07 — the two gaps fill together, because a chip is a "
     "button that toggles and shows a border when it is on. `toggles` hands the "
     "state to AppKit's PushOnPushOff so `is_on()` cannot drift from what the "
     "user sees, and the click routes through a trampoline that syncs it into "
     "the props before the handler runs (the shape checkbox_action already "
     "used). `bordered` means the platform BEZEL on `button` and the drawn "
     "outline on `icon_button` / `text_button` — different mechanisms, one "
     "word, because from the application's side it is the same question"),
    ("text_button", "text_button(title:)", "a button with no bezel",
     "the ledger has ONE Button and expresses the flat posture by clearing three "
     "properties (BackgroundColor, BorderColor, BorderWidth), so facet "
     "inherited a button that always draws its bezel and no way to ask it not "
     "to — clearing facet's `background_color` clears a CALayer facet adds, "
     "while the bezel is drawn by NSButton's cell underneath it",
     "SHIPPED 2026-08-07 — hand-written vendor/facet/src/text_button.cplus. A "
     "control and not a flag on `button`, for the reason Flutter separates "
     "TextButton from ElevatedButton: half the parameters would be dead. "
     "`border_color`, `border_width` and `corner_radius` mean nothing on a "
     "control whose point is having none of them, and a flag would leave them "
     "present, settable and silently ignored. facet already made that call for "
     "`icon_button`. It stays a REAL NSButton — keyboard activation, "
     "VoiceOver, and role=button in the agent surface — which is what a "
     "`label(...).gesture(on_click:)` throws away"),
    ("symbol", "symbol(icon:) / system_symbol(name:)", "a glyph",
     "the ledger has no glyph vocabulary at all — an icon is app content, not OS "
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
     "the ledger folds wrapping into Label via LineBreakMode, and so does facet",
     "NOT A CONTROL 2026-08-03 — `label(text, line_break: LineBreak::WordWrap)`. "
     "The predecessor was a separate kind only because AppKit has a second "
     "factory (`wrappingLabelWithString:` vs `labelWithString:`), and which "
     "factory to call is the backend's sentence, not the contract's"),
    ("(component)", "composer", "a multi-line input with chrome",
     "a compound widget, not a ledger control",
     "NOT A CONTROL 2026-08-03 — `text_area` already carries the text, the "
     "placeholder, on_submit and on_text_changed; `bordered` carries the "
     "border and the shared band carries the background. An arrangement of "
     "existing words is a COMPONENT (Stage 3's tier), not a new word"),
    ("tree", "tree(root:) + TreeNode", "a hierarchical list",
     "the ledger has no tree control at all",
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
     "the ledger attaches behaviour by pushing recognizer objects into a "
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
     "platform chrome; the ledger's window is not a described tree, so there was "
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
# Rows whose DEFAULT reason was false: facet implements them, and the mechanical
# rule said otherwise. Same defect as the five method rows that claimed to be
# engine internals while `nav::quit` and `runtime::alert` sat in the tree — a
# contract that answers "no" for a verb that exists is worse than one with a
# gap in it.
OVERLAY = {
    ("Window", "AddOverlay"):
        ("DROP", "", "facet says it as set_absolute + set_left/top/right/bottom — an overlay is a node out of flow, not a layer"),
    ("Window", "RemoveOverlay"):
        ("DROP", "", "facet says it as set_absolute + set_left/top/right/bottom — an overlay is a node out of flow, not a layer"),
    ("Window", "Overlays"):
        ("DROP", "", "facet says it as set_absolute + set_left/top/right/bottom — an overlay is a node out of flow, not a layer"),

    # A GRID, said facet's way. The ledger models this as an IItemsLayout object
    # (LinearItemsLayout or GridItemsLayout with a Span), which is a class where
    # facet needs a number: a collection's items are ordinary facet children
    # inside its scroll, so N columns is flex wrap plus a width, and the only
    # thing the application has to say is N.
    #
    # 1 is a list, which is why the default is 1 and not 0 — "one column" is a
    # true description of a list, and it means no application has to know that
    # zero would have meant something.
    ("StructuredItemsView", "ItemsLayout"):
        ("ADOPT", "set_columns / columns()",
         "usize — facet's word for the ledger's IItemsLayout: 0 or 1 is a list, N is a grid of N columns"),
    # A carousel is ONE horizontal run. The ledger's layout object also carries
    # orientation and snap points, which facet fixes — so the only thing left
    # for it to say here is a multi-row carousel, and nothing has asked for one.
    # `collection` is where a grid lives.
    ("CarouselView", "ItemsLayout"):
        ("DROP", "", "a carousel is a single horizontal run; a grid is what `collection` is for"),
    ("VisualElement", "SafeAreaEdges"):
        ("DROP", "", "facet says it as set_safe_area on the shared band — NOT flex's, "
                     "the window's own insets"),
    ("View", "GestureRecognizers"):
        ("DROP", "", "facet says it as the .gesture() band — not a bound collection"),
    ("VisualElement", "GestureRecognizers"):
        ("DROP", "", "facet says it as the .gesture() band — not a bound collection"),

    # `object`-typed rows that ARE vocabulary: a subtree, or a selection index.
    ("Picker", "SelectedItem"): ("DROP", "", MODEL + " — the selection is an index; set_selected_index carries it"),
    ("ListView", "SelectedItem"): ("DROP", "", MODEL + " — the selection is an index"),
    ("SelectableItemsView", "SelectedItem"): ("DROP", "", MODEL + " — the selection is an index"),
    ("RadioButton", "Value"): ("DROP", "", MODEL + " — the binding-era group value; a radio's identity is its key"),
    # `Content` is typed `object` and the mechanical rule read that as a
    # subtree. It is not one: every the ledger handler renders it through
    # `ContentAsString`, and the type ships that method for exactly this
    # reason. A radio's content is its LABEL, so facet types it as one — which
    # is also what makes `text_color`, `text_transform` and
    # `character_spacing` mean something, since all three style a string a
    # radio did not have.
    ("RadioButton", "Content"): ("ADOPT", "set_text / text()",
                                 "str — ledger row types it `object`; ContentAsString is how every "
                                 "platform handler reads it"),

    # The escape hatch INTENT blesses: an app may drop beneath facet and use
    # the platform view directly. The ledger's Handler is that seam, so facet names
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

    # the ledger splits a bound Header/Footer from the realized *Element it produces.
    # That split IS the binding model; facet writes a Node and reads it back
    # through one verb.
    ("ListView", "HeaderElement"): ("DROP", "", MODEL + " — the realized half of a bound Header; facet has one header verb"),
    ("ListView", "FooterElement"): ("DROP", "", MODEL + " — the realized half of a bound Footer; facet has one footer verb"),

    # Safe-area insets change how a child is laid out — flex_layout's business.
    ("Border", "SafeAreaEdges"): ("DROP", "", LAYOUT),

    # A mutable collection the ledger hands back through a getter; see BAND.
    ("Picker", "Items"): ("ADOPT", "set_items / items()",
                          "TextList — the strings a popup shows"),
    ("MenuFlyoutItem", "KeyboardAccelerators"): (
        "ADOPT", "set_shortcut / shortcut()",
        "Shortcut — ledger row models a list; a menu item shows one key equivalent, "
        "and every platform that has menus agrees"),

    # ---- found by the iris audit (reports/facet-iris-gap-2026-08-01.md).
    # These four reached a control as ADOPT and were then dropped in silence by
    # the generator's SKIP_TYPES. Each is decided here, in the ledger.

    # Same rule already applied to Picker/ListView/SelectableItemsView
    # SelectedItem: a carousel's current item is its position.
    ("CarouselView", "CurrentItem"): ("DROP", "", MODEL + " — the current item "
                                      "is a position; set_position carries it"),
    # The realized views a virtualizing panel happens to be holding.
    ("CarouselView", "VisibleViews"): ("DROP", "", ENGINE + " — the ledger's realized-"
                                       "view bookkeeping; facet reads children()"),
    # Overriding the area a view is laid out in is layout, whoever asks for it.
    ("ScrollView", "LayoutAreaOverride"): ("DROP", "", LAYOUT),
    # the ledger carries the dash pattern twice: DoubleCollection for the app,
    # float[] for the handler. facet writes one and the backend reads it.
    ("Border", "StrokeDashPattern"): ("DROP", "", ENGINE + " — the handler-facing "
                                      "mirror of StrokeDashArray"),

    # One corner verb, one type. The ledger types Button/RadioButton corners as int
    # and BoxView's as a 4-corner struct; facet carries Corners everywhere.
    ("Button", "CornerRadius"): ("ADOPT", "set_corner_radius / corner_radius()", "Corners"),
    ("RadioButton", "CornerRadius"): ("ADOPT", "set_corner_radius / corner_radius()", "Corners"),
    ("ImageButton", "CornerRadius"): ("ADOPT", "set_corner_radius / corner_radius()", "Corners"),

    # Two different thresholds. The ledger names both `Threshold`: the recognizer's
    # is the distance that counts as a swipe, the view's is how far you drag
    # to reveal the actions. Distances are f64 in facet, not the ledger's uint.
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
    # A recorded ABSENCE outranks every rule below it: the row is dropped, and
    # the reason says plainly that nothing answers it. Checked for all four
    # bands, because the mislabels were not confined to methods.
    absent_any = ABSENT.get((ty, member))
    if absent_any:
        return ("DROP", "", "NOT BUILT — " + absent_any)
    if band == "methods":
        name = METHOD_VOCABULARY.get((ty, member))
        if name:
            return ("ADOPT", name, "method")
        if LAYOUT_METHODS.match(member):
            return ("DROP", "", LAYOUT)
        judged = METHOD_DROPS.get((ty, member))
        if judged:
            return ("DROP", "", judged)
        if ENGINE_METHODS.match(member):
            return ("DROP", "", ENGINE)
        # No rule matched and nobody judged it. Fail, exactly as an unmapped
        # write does — this is the floor the methods band never had.
        return None
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


# The upstream namespace is stripped where rows ENTER, so no map below has to
# spell it and nothing downstream can echo it into generated source. The bare
# type name is what the contract traces to; the assembly it came from is not
# part of the vocabulary and naming it invites a reader — a model especially —
# to import that framework's habits along with its words.
def _bare(t):
    """The type's own name, without whatever namespace declared it.

    Stripped where rows ENTER, so no map below has to spell an upstream
    assembly and nothing downstream can echo one into generated source. The
    bare name is what the contract traces to; the framework it came from is not
    part of the vocabulary, and naming it invites a reader — a model especially
    — to import that framework's habits along with its words.
    """
    if not isinstance(t, str):
        return t
    # Every dotted qualifier, including the ones inside generic arguments —
    # `IReadOnlyList<Foo.Bar!>` has two.
    return re.sub(r"[A-Za-z_][A-Za-z0-9_]*(?:\.[A-Za-z_][A-Za-z0-9_]*)*\.", "", t)


# Both sides of the lookup use the bare name: the maps above were written with
# whatever namespace the manifest declared, and rows now arrive stripped.
TYMAP = {_bare(k): v for k, v in TYMAP.items()}
ENUMS = {_bare(k): v for k, v in ENUMS.items()}
DROP_TYPES = {_bare(k): v for k, v in DROP_TYPES.items()}


def rows():
    spec = json.load(open(SPEC))
    out, undecided = [], []
    for ty, bands in spec.items():
        for declared in ("writes", "reads", "events", "methods"):
            for member, vt in bands.get(declared, {}).items():
                valuety = _bare(vt) if isinstance(vt, str) else ""
                band = BAND.get((ty, member), declared)
                r = OVERLAY.get((ty, member)) or default_row(ty, member, band, valuety)
                if r is None:
                    undecided.append((ty, member, band, valuety))
                    continue
                if ty == "Window" and r[0] == "ADOPT":
                    r = (r[0], r[1], f"{r[2]} — on {window_owner(member, band)}")
                out.append((ty, member, band, valuety) + r)
                # stages/3 "found late" #1: the ledger's FontAttributes folds weight
                # and slant into one enum, so "semibold italic" is unsayable.
                # facet splits the axes: the row above became a WEIGHT scale,
                # and every type carrying it gains a separate is_italic row.
                if member == "FontAttributes" and r[0] == "ADOPT":
                    out.append((ty, "FontAttributes (italic axis)", band, "bool",
                                "ADOPT", "is_italic",
                                "bool — facet-origin: the slant axis the ledger folded into "
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
        "# the ledger -> facet MAP — bootstrap Stage 1\n\n",
        "GENERATED by tools/ledger_map.py; the curation lives THERE (overlay dict,\n",
        "drop rules, type map, enum map). facet is an empty package, so every\n",
        "row is ADOPT (facet declares it) or DROP (the ledger's model, not facet's).\n",
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
    out.append("No the ledger provenance. INTENT: \"it grows by what its applications\n"
               "need, and adds words of its own.\" Measured against iris in\n"
               "`reports/facet-iris-gap-2026-08-01.md` and declared here so the\n"
               "ledger names every word facet has, not only the borrowed ones.\n\n")
    out.append("| owner | word | shape | why the ledger has none | disposition |\n")
    out.append("|---|---|---|---|---|\n")
    for owner, word, shape, why, when in FACET_ORIGIN:
        out.append(f"| {owner} | `{word}` | {shape} | {why} | {when} |\n")
    out.append("\n")
    cur = None
    for ty, member, band, valuety, status, fname, note in rs:
        if ty != cur:
            out.append(f"\n## {ty}\n\n")
            out.append("| the ledger member | band | status | facet | note |\n")
            out.append("|---|---|---|---|---|\n")
            cur = ty
        out.append(f"| {member} | {band} | **{status}** | {fname or '—'} | {note} |\n")

    path = os.path.join(ROOT, "plans", "facet", "row_type-map-draft.md")
    with open(path, "w") as f:
        f.writelines(out)
    print(f"{len(rs)} rows — " + ", ".join(f"{k} {v}" for k, v in sorted(counts.items())))
    for why, n in sorted(reasons.items(), key=lambda kv: -kv[1]):
        print(f"  DROP {n:>4}  {why}")
    print(f"wrote {os.path.relpath(path, ROOT)}")


if __name__ == "__main__":
    main()
