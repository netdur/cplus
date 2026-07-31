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
    (re.compile(r"^(IsEnabledCore|IsPlatformEnabled|IsPlatformStateConsistent|"
                r"IsInPlatformLayout|DisableLayout|Batched|BatchCommitted|"
                r"MeasureInvalidated|FocusChangeRequested|ChildrenReordered)$"),
     "MAUI engine plumbing, not app surface"),
    (re.compile(r"^FontAutoScalingEnabled$"),
     "mobile dynamic-type; no desktop equivalent"),
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
    ("DropGestureRecognizer", "AllowDrop"): ("KEEP-FACET", "Node.on_drop at build", "declaring a drop handler IS accepting drops"),
    ("DragGestureRecognizer", "DragStarting"): ("FUTURE", "", "payload is fixed at build (draggable(text)); a lazy start-callback waits for a consumer"),
    ("DragGestureRecognizer", "CanDrag"): ("KEEP-FACET", "Node.draggable(text)", "build-time source payload; empty = not draggable — iris board uses it"),
    ("TapGestureRecognizer", "Tapped"): ("KEEP-FACET", "on_click", "facet's click"),
    ("TapGestureRecognizer", "NumberOfTapsRequired"): ("ADOPT", "on_double_click (as its own hook)", "double-click is the real demand"),
    ("PinchGestureRecognizer", "PinchUpdated"): ("KEEP-FACET", "Chrome zoomable (app-level)", "facet zooms the window content, not per-node"),
    # ---- disposition pass (maui-regen completion): every former ADOPT row
    # lands in a final bucket. KEEP-FACET = covered by an existing verb;
    # UNSUPPORTED rows live in gen_contract's table with the AppKit reason;
    # FUTURE = real surface awaiting a consumer, visible here.

    # Fonts: build-time Node.font(size)/weight() cover styling; live font
    # mutation and family selection wait for a font story the THEME owns.
    ("Label", "FontSize"):             ("KEEP-FACET", "Node.font(size) at build", "live font mutation is FUTURE"),
    ("Button", "FontSize"):            ("KEEP-FACET", "Node.font(size) at build", ""),
    ("Picker", "FontSize"):            ("KEEP-FACET", "Node.font(size) at build", ""),
    ("DatePicker", "FontSize"):        ("KEEP-FACET", "Node.font(size) at build", ""),
    ("TimePicker", "FontSize"):        ("KEEP-FACET", "Node.font(size) at build", ""),
    ("RadioButton", "FontSize"):       ("KEEP-FACET", "Node.font(size) at build", ""),
    ("InputView", "FontSize"):         ("KEEP-FACET", "Node.font(size) at build", ""),
    ("Label", "FontFamily"):           ("FUTURE", "", "font family is the theme's to own; no facet story yet"),
    ("Button", "FontFamily"):          ("FUTURE", "", "theme-owned; no story yet"),
    ("Picker", "FontFamily"):          ("FUTURE", "", "theme-owned; no story yet"),
    ("DatePicker", "FontFamily"):      ("FUTURE", "", "theme-owned; no story yet"),
    ("TimePicker", "FontFamily"):      ("FUTURE", "", "theme-owned; no story yet"),
    ("RadioButton", "FontFamily"):     ("FUTURE", "", "theme-owned; no story yet"),
    ("InputView", "FontFamily"):       ("FUTURE", "", "theme-owned; no story yet"),

    # Attributed-string band: kerning/line metrics need an attributed text
    # pipeline the backend does not have.
    ("Label", "CharacterSpacing"):     ("UNSUPPORTED", "", "needs the attributed-string pipeline"),
    ("Button", "CharacterSpacing"):    ("UNSUPPORTED", "", "needs the attributed-string pipeline"),
    ("Picker", "CharacterSpacing"):    ("UNSUPPORTED", "", "needs the attributed-string pipeline"),
    ("DatePicker", "CharacterSpacing"): ("UNSUPPORTED", "", "needs the attributed-string pipeline"),
    ("TimePicker", "CharacterSpacing"): ("UNSUPPORTED", "", "needs the attributed-string pipeline"),
    ("RadioButton", "CharacterSpacing"): ("UNSUPPORTED", "", "needs the attributed-string pipeline"),
    ("InputView", "CharacterSpacing"): ("UNSUPPORTED", "", "needs the attributed-string pipeline"),
    ("Label", "LineHeight"):           ("UNSUPPORTED", "", "needs the attributed-string pipeline"),

    # Covered by existing verbs (the audit's own gap-fills or hand verbs).
    ("VisualElement", "BackgroundColor"): ("KEEP-FACET", "Handle.set_background_color (wired slot)", "same slot, provenance row"),
    ("VisualElement", "ZIndex"):       ("KEEP-FACET", "facet::raise(sender, key)", "the z verb; explicit z-index died with v1"),
    ("VisualElement", "IsLoaded"):     ("KEEP-FACET", "facet::is_attached", "lifecycle read"),
    ("VisualElement", "Loaded"):       ("KEEP-FACET", "Lifecycle.on_attach", ""),
    ("VisualElement", "Unloaded"):     ("KEEP-FACET", "Lifecycle.on_detach", ""),
    ("InputView", "Text"):             ("KEEP-FACET", "Handle.set_text / text()", ""),
    ("InputView", "Placeholder"):      ("KEEP-FACET", "Handle.set_placeholder (wired slot)", ""),
    ("InputView", "TextColor"):        ("KEEP-FACET", "Handle.set_text_color (wired slot)", ""),
    ("Picker", "Title"):               ("KEEP-FACET", "Handle.set_title (wired slot)", ""),
    ("Picker", "TextColor"):           ("UNSUPPORTED", "", "NSPopUpButton renders its title via the menu; no text-color API"),
    ("DatePicker", "TextColor"):       ("KEEP-FACET", "Handle.set_text_color (wired slot)", "NSDatePicker answers setTextColor:"),
    ("Stepper", "Minimum"):            ("KEEP-FACET", "Handle.set_minimum (wired slot)", "NSStepper answers setMinValue:"),
    ("Stepper", "Maximum"):            ("KEEP-FACET", "Handle.set_maximum (wired slot)", ""),
    ("Stepper", "Value"):              ("KEEP-FACET", "Handle.set_value", ""),
    ("CheckBox", "IsChecked"):         ("KEEP-FACET", "Handle.set_on / value()", ""),
    ("RadioButton", "IsChecked"):      ("KEEP-FACET", "Handle.set_on / value()", ""),
    ("Button", "BorderColor"):         ("KEEP-FACET", "Handle.set_border (live-restyle verb)", ""),
    ("Button", "BorderWidth"):         ("KEEP-FACET", "Handle.set_border (live-restyle verb)", ""),
    ("Button", "CornerRadius"):        ("KEEP-FACET", "Handle.set_corner (live-restyle verb)", ""),
    ("Button", "ImageSource"):         ("KEEP-FACET", "facet::icon_button at build", ""),
    ("Image", "Source"):               ("KEEP-FACET", "facet::image(path) at build", ""),
    ("BoxView", "Color"):              ("KEEP-FACET", "Handle.set_background (live-restyle verb)", ""),
    ("Border", "StrokeThickness"):     ("KEEP-FACET", "Handle.set_border (live-restyle verb)", ""),
    ("ListView", "RowHeight"):         ("KEEP-FACET", "facet::list(row_height:)", ""),
    ("ListView", "HasUnevenRows"):     ("KEEP-FACET", "facet lists measure rows by default", ""),
    ("TableView", "RowHeight"):        ("KEEP-FACET", "facet::list(row_height:)", ""),
    ("TableView", "HasUnevenRows"):    ("KEEP-FACET", "facet lists measure rows by default", ""),
    ("Window", "MinimumWidth"):        ("KEEP-FACET", "Chrome min_width", ""),
    ("Window", "MinimumHeight"):       ("KEEP-FACET", "Chrome min_height", ""),
    ("Window", "IsMaximizable"):       ("KEEP-FACET", "Chrome zoomable", ""),
    ("Window", "IsActivated"):         ("KEEP-FACET", "runtime observe_window_active/inactive", ""),
    ("Window", "SizeChanged"):         ("KEEP-FACET", "facet::observe_size on the root element", ""),

    # Value-change events: facet handlers live in the DESCRIPTION — build-time
    # on_change/on_submit/on_click are the model, not runtime event streams.
    ("Picker", "SelectedIndexChanged"): ("KEEP-FACET", "on_change at build", ""),
    ("Slider", "ValueChanged"):        ("KEEP-FACET", "on_change at build", ""),
    ("Stepper", "ValueChanged"):       ("KEEP-FACET", "on_change at build", ""),
    ("Switch", "Toggled"):             ("KEEP-FACET", "on_change at build", ""),
    ("CheckBox", "CheckedChanged"):    ("KEEP-FACET", "on_change at build", ""),
    ("RadioButton", "CheckedChanged"): ("KEEP-FACET", "on_change at build", ""),
    ("InputView", "TextChanged"):      ("KEEP-FACET", "on_change at build", ""),
    ("Entry", "Completed"):            ("KEEP-FACET", "on_submit at build", ""),
    ("Editor", "Completed"):           ("KEEP-FACET", "on_submit at build", ""),
    ("SearchBar", "SearchButtonPressed"): ("KEEP-FACET", "on_submit at build", ""),
    ("DatePicker", "DateSelected"):    ("KEEP-FACET", "on_change at build", ""),
    ("TimePicker", "TimeSelected"):    ("KEEP-FACET", "on_change at build", ""),
    ("ListView", "ItemSelected"):      ("KEEP-FACET", "rows are components; clicks are the row's handlers", ""),
    ("ListView", "ItemTapped"):        ("KEEP-FACET", "rows are components; clicks are the row's handlers", ""),
    ("SelectableItemsView", "SelectionChanged"): ("KEEP-FACET", "rows are components; clicks are the row's handlers", ""),
    ("TableView", "ModelChanged"):     ("DROP", "", "binding-model artifact"),

    # UNSUPPORTED on AppKit: the control has no public API for it.
    ("Entry", "IsPassword"):           ("UNSUPPORTED", "", "NSSecureTextField is a class, not a flag — use facet::secure_field"),
    ("SearchBar", "CancelButtonColor"): ("UNSUPPORTED", "", "NSSearchField cell internals; no public API"),
    ("SearchBar", "SearchIconColor"):  ("UNSUPPORTED", "", "NSSearchField cell internals; no public API"),
    ("Picker", "IsOpen"):              ("UNSUPPORTED", "", "NSPopUpButton has no open/close API"),
    ("Picker", "TitleColor"):          ("UNSUPPORTED", "", "menu-rendered title; no color API"),
    ("DatePicker", "IsOpen"):          ("UNSUPPORTED", "", "no popover control API"),
    ("DatePicker", "Format"):          ("UNSUPPORTED", "", "NSDatePicker uses element flags, not format strings"),
    ("TimePicker", "IsOpen"):          ("UNSUPPORTED", "", "no popover control API"),
    ("TimePicker", "Format"):          ("UNSUPPORTED", "", "NSDatePicker uses element flags, not format strings"),
    ("TimePicker", "TextColor"):       ("KEEP-FACET", "Handle.set_text_color (wired slot)", "NSDatePicker answers setTextColor:"),
    ("Switch", "OnColor"):             ("UNSUPPORTED", "", "NSSwitch has no tint API"),
    ("Switch", "OffColor"):            ("UNSUPPORTED", "", "NSSwitch has no tint API"),
    ("Switch", "ThumbColor"):          ("UNSUPPORTED", "", "NSSwitch has no tint API"),
    ("ProgressBar", "ProgressColor"):  ("UNSUPPORTED", "", "NSProgressIndicator has no public tint API"),
    ("ActivityIndicator", "Color"):    ("UNSUPPORTED", "", "NSProgressIndicator has no public tint API"),
    ("CheckBox", "Color"):             ("UNSUPPORTED", "", "NSButton checkbox has no tint API"),
    ("RadioButton", "BorderColor"):    ("UNSUPPORTED", "", "NSButton radio draws its own bezel"),
    ("RadioButton", "BorderWidth"):    ("UNSUPPORTED", "", "NSButton radio draws its own bezel"),
    ("RadioButton", "CornerRadius"):   ("UNSUPPORTED", "", "NSButton radio draws its own bezel"),
    ("RadioButton", "TextColor"):      ("UNSUPPORTED", "", "attributed title needed; same as Button.TextColor"),
    ("Button", "TextColor"):           ("UNSUPPORTED", "", "NSButton titles color via attributed strings only"),
    ("Image", "IsOpaque"):             ("UNSUPPORTED", "", "NSImageView has no opacity toggle; set_opacity covers alpha"),
    ("InputView", "MaxLength"):        ("UNSUPPORTED", "", "needs a formatter/delegate, not a property"),

    # FUTURE: real surface, no consumer yet — promote on demand.
    ("RadioButton", "GroupName"):      ("FUTURE", "", "radio grouping story"),
    ("Border", "StrokeDashOffset"):    ("FUTURE", "", "dash styling beyond set_border"),
    ("Border", "StrokeMiterLimit"):    ("FUTURE", "", "stroke styling beyond set_border"),
    ("VisualElement", "InputTransparent"): ("FUTURE", "", "needs a hitTest override"),
    ("InputView", "CursorPosition"):   ("FUTURE", "", "selection/caret band — likely first promoted"),
    ("InputView", "SelectionLength"):  ("FUTURE", "", "selection/caret band"),
    ("InputView", "IsSpellCheckEnabled"): ("FUTURE", "", "text_area needs documentView routing"),
    ("InputView", "IsTextPredictionEnabled"): ("FUTURE", "", "text_area needs documentView routing"),
    ("ActivityIndicator", "IsRunning"): ("FUTURE", "", "start/stopAnimation is behavioral; no read-back"),
    ("Image", "IsLoading"):            ("FUTURE", "", "async image loading band"),
    ("Button", "IsPressed"):           ("FUTURE", "", "press-state read"),
    ("Button", "Pressed"):             ("FUTURE", "", "press eventing"),
    ("Button", "Released"):            ("FUTURE", "", "press eventing"),
    ("VisualElement", "Focused"):      ("FUTURE", "", "focus eventing; set_is_focused write exists"),
    ("VisualElement", "Unfocused"):    ("FUTURE", "", "focus eventing"),
    ("VisualElement", "X"):            ("FUTURE", "", "position read; size() covers layout math so far"),
    ("VisualElement", "Y"):            ("FUTURE", "", "position read"),
    ("Picker", "Opened"):              ("FUTURE", "", "popover eventing"),
    ("Picker", "Closed"):              ("FUTURE", "", "popover eventing"),
    ("DatePicker", "Opened"):          ("FUTURE", "", "popover eventing"),
    ("DatePicker", "Closed"):          ("FUTURE", "", "popover eventing"),
    ("TimePicker", "Opened"):          ("FUTURE", "", "popover eventing"),
    ("TimePicker", "Closed"):          ("FUTURE", "", "popover eventing"),
    ("Slider", "DragStarted"):         ("FUTURE", "", "fine-grained slider eventing"),
    ("Slider", "DragCompleted"):       ("FUTURE", "", "fine-grained slider eventing"),
    ("ListView", "IsGroupingEnabled"): ("FUTURE", "", "grouped lists"),
    ("ListView", "SeparatorColor"):    ("FUTURE", "", "facet list cells draw separator-less"),
    ("ListView", "IsPullToRefreshEnabled"): ("FUTURE", "", "touch refresh idiom"),
    ("ListView", "IsRefreshing"):      ("FUTURE", "", "touch refresh idiom"),
    ("ListView", "RefreshAllowed"):    ("FUTURE", "", "touch refresh idiom"),
    ("ListView", "RefreshControlColor"): ("FUTURE", "", "touch refresh idiom"),
    ("ListView", "ItemAppearing"):     ("FUTURE", "", "list visibility eventing"),
    ("ListView", "ItemDisappearing"):  ("FUTURE", "", "list visibility eventing"),
    ("ListView", "Refreshing"):        ("FUTURE", "", "touch refresh idiom"),
    ("ListView", "Scrolled"):          ("FUTURE", "", "scroll eventing; scroll_offset reads exist"),
    ("ListView", "ScrollToRequested"): ("DROP", "", "internal request plumbing"),
    ("ItemsView", "Scrolled"):         ("FUTURE", "", "scroll eventing"),
    ("ItemsView", "ScrollToRequested"): ("DROP", "", "internal request plumbing"),
    ("ItemsView", "RemainingItemsThreshold"): ("FUTURE", "", "infinite-scroll band"),
    ("ItemsView", "RemainingItemsThresholdReached"): ("FUTURE", "", "infinite-scroll band"),
    ("GroupableItemsView", "IsGrouped"): ("FUTURE", "", "grouped lists"),
    ("ReorderableItemsView", "CanReorderItems"): ("FUTURE", "", "user-reorder band"),
    ("ReorderableItemsView", "CanMixGroups"): ("FUTURE", "", "user-reorder band"),
    ("ReorderableItemsView", "ReorderCompleted"): ("FUTURE", "", "user-reorder band"),
    ("TabbedPage", "BarBackgroundColor"): ("FUTURE", "", "tab chrome styling waits for a tabs contract"),
    ("TabbedPage", "BarTextColor"):    ("FUTURE", "", "tab chrome styling"),
    ("TabbedPage", "SelectedTabColor"): ("FUTURE", "", "tab chrome styling"),
    ("TabbedPage", "UnselectedTabColor"): ("FUTURE", "", "tab chrome styling"),
    ("TapGestureRecognizer", "NumberOfTapsRequired"): ("FUTURE", "", "double-click hook waits for a consumer"),
    ("PanGestureRecognizer", "TouchPoints"): ("FUTURE", "", "recognizer model not adopted"),
    ("PanGestureRecognizer", "PanUpdated"): ("FUTURE", "", "recognizer model not adopted"),
    ("SwipeGestureRecognizer", "Swiped"): ("FUTURE", "", "recognizer model not adopted"),
    ("PointerGestureRecognizer", "PointerEntered"): ("FUTURE", "", "hover band; pointer_position covers drags"),
    ("PointerGestureRecognizer", "PointerExited"): ("FUTURE", "", "hover band"),
    ("PointerGestureRecognizer", "PointerPressed"): ("FUTURE", "", "hover band"),
    ("PointerGestureRecognizer", "PointerReleased"): ("FUTURE", "", "hover band"),
    ("DragGestureRecognizer", "DropCompleted"): ("FUTURE", "", "drag-source eventing"),
    ("DropGestureRecognizer", "DragLeave"): ("FUTURE", "", "drag_targeted covers enter; leave waits"),
    ("Window", "MaximumWidth"):        ("FUTURE", "", "Chrome max floor waits for demand"),
    ("Window", "MaximumHeight"):       ("FUTURE", "", "Chrome max floor waits for demand"),
    ("Window", "IsMinimizable"):       ("FUTURE", "", "chrome flag"),
    ("Window", "DisplayDensity"):      ("FUTURE", "", "backing scale read"),
    ("Window", "DisplayDensityChanged"): ("FUTURE", "", "backing scale eventing"),
    ("Window", "Created"):             ("KEEP-FACET", "App.on_launch", ""),
    ("Window", "Destroying"):          ("KEEP-FACET", "App.on_quit", ""),
    ("Window", "Resumed"):             ("FUTURE", "", "mobile lifecycle"),
    ("Window", "Stopped"):             ("FUTURE", "", "mobile lifecycle"),
    ("Window", "Backgrounding"):       ("FUTURE", "", "mobile lifecycle"),
    ("Window", "ModalPushed"):         ("FUTURE", "", "modal stack"),
    ("Window", "ModalPushing"):        ("FUTURE", "", "modal stack"),
    ("Window", "ModalPopped"):         ("FUTURE", "", "modal stack"),
    ("Window", "ModalPopping"):        ("FUTURE", "", "modal stack"),
    ("Window", "PopCanceled"):         ("DROP", "", "Shell navigation artifact"),

    # Wired in this pass (probe-verified):
    ("Image", "IsAnimationPlaying"):   ("ADOPT", "set_is_animation_playing", "NSImageView setAnimates: — probeable"),
    ("InputView", "PlaceholderColor"): ("UNSUPPORTED", "", "placeholder color needs the attributed-string pipeline"),
    ("Slider", "MaximumTrackColor"):   ("UNSUPPORTED", "", "NSSlider has no equivalent"),
    ("Slider", "ThumbColor"):          ("UNSUPPORTED", "", "NSSlider has no equivalent"),
    ("Slider", "ThumbImageSource"):    ("UNSUPPORTED", "", "NSSlider has no equivalent"),

    # -- window runtime --
    ("Window", "Activated"):           ("KEEP-FACET", "runtime::observe_window_active -> Cancellable", "shipped eb5b1b7"),
    ("Window", "Deactivated"):         ("KEEP-FACET", "runtime::observe_window_inactive -> Cancellable", "shipped eb5b1b7"),
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
