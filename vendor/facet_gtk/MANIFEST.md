# facet_gtk — the manifest

INTENT's rule, the same one `facet_appkit/MANIFEST.md` is held to: for each
thing facet declares, this package either implements it with what GTK offers,
or **states plainly that GTK cannot**. A verb that is neither implemented nor
listed here is a gap, and the gap is a bug.

**Status: early, and the number says so.**

```
360 declared prop bits      29 answered      8%
                            (appkit 318/88%, uikit 311/86%)
```

`python3 tools/parity.py` prints it and `--check` fails if it drops. Run it
before believing any adjective in this file.

Read the 8% precisely. It does not mean "GTK cannot do the other 92%" — almost
nothing here is in the cannot-ledger. It means **not written yet**, and the two
must never be blurred: a reader who cannot tell them apart cannot tell an early
backend from an abandoned one.

## What is live

| | |
|---|---|
| the shared band | `background_color`, `corner_radius`, `opacity`, `is_enabled`, `is_visible`, `tooltip` — every backed node, whatever kind |
| layout | facet's frames placed into `GtkFixed`; intrinsic size via `gtk_widget_measure`, height measured against the offered width |
| label | text, alignment, max lines, wrap, ellipsize, font size/family/weight/italic, colour |
| button | title, bordered (GTK's `flat`), font size/family/weight/italic, colour |
| text_field | text, placeholder, secure, read-only, max length, alignment |
| toggle · checkbox · progress · slider | value/state, plus the checkbox's accent colour and the slider's range |
| appearance | `is_dark` from GtkSettings; a flip re-applies through `C_RESTYLE`, not through a registry of painted widgets |

Every other kind gets a `GtkFixed` that honours the shared band and holds its
children. That is more than nothing and less than a claim: the tree lays out
and the boxes are in the right places, and the control's own verbs are unwritten.

## 1. Decided absent — GTK has no such thing

**Empty.** Nothing has been investigated hard enough to earn a row here yet,
and putting a "probably not" in this section would be exactly the blurring the
header warns about. A verb belongs here only after the answer has been looked
for and is genuinely not there — the bar `facet_appkit` records as "a control
does not have to be one native widget", where sixty-odd rows left its ledger
once the answer was allowed to be built from two classes instead of one.

## 2. Not built yet — the debt

Everything not listed as live above. The large ones, in the order they matter:

- **No handlers at all.** `gestures::install_key_reader` and
  `component::install_sender_readers` are unset, so nothing reads back from a
  control: a button does not call `on_click`, a switch does not report a
  change. A backend has two surfaces — props are writes, handlers are reads —
  and `facet_uikit/MANIFEST.md` records the whole reason the handler axis
  exists, which is that a `text_button` with every prop bit honoured was
  tapped and called nothing. This backend currently has *only* the write half.
- **No recycler**, so `collection` / `list` / `table` build every row.
- **No text_area / composer**, no `web`, no `canvas` drawing.
- **`observe_size` answers 0** (a cancellation token meaning "nothing
  registered") rather than half-wiring a size observer.
- **`after` holds ONE outstanding timer.** A second call before the first fires
  replaces it. facet's own uses are one-at-a-time; a caller needing more will
  find this written down instead of discovering it.
- **`insert` ignores `slot`.** Children land in insertion order, which is
  right for a fresh mount and wrong for an insert into the middle of a live
  list.
- **RTL.** `corner_radius` maps facet's leading/trailing onto CSS's
  left/right, which coincide only in a left-to-right flow. `C_FLOW_DIRECTION`
  is unanswered.

## 3. Works, but does not look like its name

- **Every unimplemented kind is a bare container.** It is in the right place at
  the right size and honours the band; it does not look like a carousel.
