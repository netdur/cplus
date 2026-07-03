#!/usr/bin/env python3
"""Complete each generated GObject vendor package's [dependencies] to its full
transitive closure. cpc vendor deps are FLAT: a package must declare every
package transitively reachable through its imports, not just direct imports.
generate_package emits only direct refs, so this pass fixes up the manifests."""
import re, pathlib

ROOT = pathlib.Path(__file__).resolve().parents[2] / "vendor"
PKGS = ["glib","gobject_gir","gmodule","gio","cairo","graphene","harfbuzz",
        "pango","pangocairo","gdkpixbuf","gdk","gsk","gtk4","adwaita"]
PKGSET = set(PKGS)

# direct vendor-package deps of each package, from `import "X/..."` lines
direct = {}
for p in PKGS:
    src = (ROOT/p/"src"/f"{p}.cplus").read_text()
    deps = set()
    for m in re.finditer(r'^import "([a-z_0-9]+)/', src, re.M):
        x = m.group(1)
        if x in PKGSET and x != p:
            deps.add(x)
    direct[p] = deps

# transitive closure (fixpoint)
closure = {p: set(direct[p]) for p in PKGS}
changed = True
while changed:
    changed = False
    for p in PKGS:
        for d in list(closure[p]):
            new = closure[d] - closure[p] - {p}
            if new:
                closure[p] |= new
                changed = True

# rewrite each manifest's [dependencies] block
for p in PKGS:
    mf = ROOT/p/"Cplus.toml"
    txt = mf.read_text()
    deps = sorted(closure[p])
    lines = ['gobject = "*"', 'stdlib  = "*"']
    for d in deps:
        lines.append(f'{d:<11} = "*"')
    block = "[dependencies]\n" + "\n".join(lines) + "\n\n"
    new = re.sub(r'\[dependencies\]\n.*?\n\n', block, txt, count=1, flags=re.S)
    if new != txt:
        mf.write_text(new)
        print(f"{p}: closure = {deps}")
    else:
        print(f"{p}: unchanged")
