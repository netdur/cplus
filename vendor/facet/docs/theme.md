# Theme

Semantic colors with platform-true defaults, light/dark handled by the
backend, and live re-theming — without ThemeData-sized constructors or
context threading. One record, installed once; roles resolved at paint time.

## Two tiers of color names

A `Color` is a NAME (token) or a literal. Two tiers of names:

**Tier 1 — the platform's own semantic colors.** Pass-through, never
themeable; their job is "look native". The escape hatch when a theme role is
not what you mean:

| family | constructors |
|---|---|
| text tiers | `text()`, `text_secondary()`, `text_tertiary()`, `placeholder()`, `link()` |
| areas | `window_background()`, `under_page_background()`, `control_background()` |
| fills | `fill()`, `fill_secondary()` |
| selection | `selected_content_background()`, `selected_text_background()` |
| accent / separator | `accent()`, `separator()` |
| system palette | `system_red/green/blue/orange/yellow/purple/pink/teal/indigo/gray()` |

**Tier 2 — theme roles.** What the app retints. Resolved by facet against
the installed theme, so a themed app looks the same on every backend:

| role | default (unset) |
|---|---|
| `primary()` / `on_primary()` | platform accent / white |
| `secondary()` / `on_secondary()` | platform accent / white |
| `ink(a = 1.0)` | the text color, at alpha `a` |
| `surface()` | `window_background()` |
| `raised()` | `control_background()` |
| `sunken()` | `under_page_background()` |
| `outline()` | `separator()` |
| `success()` / `warning()` / `danger()` | system green / orange / red |

`ink(a)` is the mark family: text, glyphs, hairlines, translucent fills at
an alpha that reads identically over both appearances' surfaces.

Literals: `rgba(r,g,b,a)` is fixed (same in both appearances);
`adaptive(light:, dark:)` packs a light/dark rgba pair into one Color,
resolved by the current appearance at paint time. `adaptive` is both the
Theme's pair primitive and the one-off escape for app colors outside the
role set.

## Installing a theme

```cplus
facet::set_theme(facet::Theme::new(
    primary: facet::Color::rgba(0.35f64, 0.42f64, 0.95f64, 1.0f64),
    surface: facet::Color::adaptive(
        light: facet::Color::rgba(0.94f64, 0.94f64, 0.96f64, 1.0f64),
        dark:  facet::Color::rgba(0.09f64, 0.09f64, 0.11f64, 1.0f64)),
));
```

Once — `main`, or `App::on_launch`. Every field is optional; name only what
differs. Never calling `set_theme` at all is valid: every role falls back to
the platform color, so a themeless app is native in both appearances.

There is no context and no scoping: `.background(Color::raised())` anywhere
— handlers, builders, services. One theme per app, deliberately.

## Light/dark and live re-theming

Text set to a Tier-1 token rides the platform's dynamic colors and adapts by
itself. Everything flattened — layer backgrounds, borders, gradients, and
theme-resolved text — is recorded in the backend's repaint registry at
apply, and re-resolved IN PLACE when the appearance flips or `set_theme` is
called again. No rebuild, no relayout, no `on_appearance_change` handler for
chrome. (`on_appearance_change` remains for CONTENT decisions: re-highlight
code, swap an image.)

Calling `set_theme` again at runtime re-themes the live app through the same
sweep — theme switching needs no machinery of its own.

## Styled containers

A container (`column`/`row`/`zstack`/`grid`) whose style operates on a view
— background, gradient, border, corner, clip, opacity, hidden, shadow,
transform, fade — gets a backing view at mount and paints it. A plain
container stays pure layout. (Historically `.background()` on a container
was a silent no-op; it is real now.)

## Native controls: the honest extent

`primary` fully colors facet-drawn chrome (fills, badges, clickables,
selection marks). Native controls are tinted only to the extent the platform
allows — AppKit per-control tinting, gtk's accent API, iOS's global tint —
and keep the system accent where it doesn't. Restyling every native widget
is the draw-it-yourself road, deliberately not taken.

## Against Flutter's shape

Same power — semantic scheme, good defaults, brightness handling — with:
no 40-field constructor (named defaults, name what differs), no
`Theme.of(context)` (roles are global names), no rebuild-on-brightness (the
backend repaints), no `copyWith` (a Theme is a plain record; build another
and `set_theme` again).
