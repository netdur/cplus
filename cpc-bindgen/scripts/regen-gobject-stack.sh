#!/usr/bin/env bash
# Regenerate the full GObject/GTK4 vendor stack from the system GIRs via
# `cpc-bindgen --gobject`, then complete each package's flat dependency closure.
#
# Run after any change to the --gobject generator. Order is irrelevant for
# generation (load_foreign reads each --use'd GIR directly), but is kept in
# dependency order for readability. The per-package `# Reproduce:` header in each
# vendor/<pkg>/Cplus.toml is the single-package form of the commands below.
#
# generate_package emits only each package's DIRECT foreign imports; cpc vendor
# deps are FLAT (a package must declare its full transitive closure), so
# close-vendor-deps.py runs afterwards to fix up every [dependencies] block.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
cd "$ROOT"

cargo build -p cpc-bindgen --release
BG="$ROOT/target/release/cpc-bindgen"

ALL="--use GLib=glib --use GObject=gobject_gir --use GModule=gmodule --use Gio=gio --use cairo=cairo --use Graphene=graphene --use HarfBuzz=harfbuzz --use Pango=pango --use PangoCairo=pangocairo --use GdkPixbuf=gdkpixbuf --use Gdk=gdk --use Gsk=gsk --use Gtk=gtk4"

# generate one package, stripping the package's own namespace from the --use set
gen() {
  local ns="$1" out="$2" selfns="$3"
  local uses="${ALL/--use $selfns=$out/}"
  "$BG" --gobject "$ns" --out "vendor/$out" $uses
}

gen GLib       glib        GLib
gen GObject    gobject_gir GObject
gen GModule    gmodule     GModule
gen Gio        gio         Gio
gen cairo      cairo       cairo
gen Graphene   graphene    Graphene
gen HarfBuzz   harfbuzz    HarfBuzz
gen Pango      pango       Pango
gen PangoCairo pangocairo  PangoCairo
gen GdkPixbuf  gdkpixbuf   GdkPixbuf
gen Gdk        gdk         Gdk
gen Gsk        gsk         Gsk
gen Gtk        gtk4        Gtk
gen Adw        adwaita     Adw

python3 "$HERE/close-vendor-deps.py"
echo "=== regen + closure done; run 'cpc check' in each vendor/<pkg> to verify ==="
