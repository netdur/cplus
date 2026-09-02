#!/usr/bin/env python3
"""Resolve a Maven/AAR dependency closure and print what it costs.

    tools/mvnresolve.py androidx.camera:camera-camera2:1.6.2

Artifacts are downloaded and cached under ./m2 (override with MVNRESOLVE_CACHE),
laid out as a Maven repo, so a second run is offline and `d8` can be pointed
straight at it. Run it from a scratch directory, not from the repo.

Why this exists: the Android toolchain ships no dependency resolver. There is
no `mvn` and no `cs`; Android Studio carries maven-resolver as a LIBRARY for
its own indexing, and Gradle itself is downloaded by the wrapper rather than
shipped. Gradle IS the resolver. But resolution over pinned coordinates is
reading XML, so this is that, and an AAR closure then dexes with stock `d8`.
Measured end to end in plans/aar.md.

Run this BEFORE taking on an AAR dependency. It prints the artifact count and
the megabytes, which is the number the decision actually turns on.

TWO REPOS, and the order matters: androidx lives on Google's Maven, NOT on
Central, which answers 404 for every `androidx.*` coordinate and looks exactly
like a bad version number.

WHERE THIS DIFFERS FROM GRADLE, both real:

  * Conflict resolution is NEAREST-WINS, which is Maven's rule. Gradle uses
    HIGHEST-WINS. Two paths to different versions of one artifact will resolve
    differently here, and this side can pick the older.
  * `.module` Gradle metadata is ignored. AndroidX publishes it and its POM is
    a compatibility shim that says so in a comment, so variant selection does
    not happen. Fine for plain AAR consumption, wrong for anything shipping
    platform-specific variants.

Handled, because each was silent until it was not: `<parent>` chains,
`<properties>` interpolation, `<dependencyManagement>`, BOM
`<scope>import</scope>` (without it the coroutines artifacts have no version
and vanish), and hard version ranges `[1.2.3]` meaning exactly that version.

NOT handled: soft/open ranges `[1.0,2.0)`, classifiers, exclusions, mirrors.
Each would fail loudly rather than quietly; add when something needs one.
"""
import sys, os, urllib.request, xml.etree.ElementTree as ET

REPOS = ["https://dl.google.com/dl/android/maven2",
         "https://repo1.maven.org/maven2"]
CACHE = os.environ.get("MVNRESOLVE_CACHE", os.path.join(os.getcwd(), "m2"))
NS = "{http://maven.apache.org/POM/4.0.0}"
unresolved, bom_imports = [], []

def fetch(path):
    local = os.path.join(CACHE, path)
    if os.path.exists(local):
        return open(local, "rb").read()
    for r in REPOS:
        try:
            data = urllib.request.urlopen(r + "/" + path, timeout=30).read()
        except Exception:
            continue
        os.makedirs(os.path.dirname(local), exist_ok=True)
        open(local, "wb").write(data)
        return data
    return None

def t(node, tag):
    e = node.find(NS + tag)
    return e.text.strip() if e is not None and e.text else None

def coord_path(g, a, v, ext):
    return "%s/%s/%s/%s-%s.%s" % (g.replace(".", "/"), a, v, a, v, ext)

def load_pom(g, a, v):
    raw = fetch(coord_path(g, a, v, "pom"))
    return ET.fromstring(raw) if raw else None

def chain(g, a, v):
    """pom + its parents, nearest first; merged properties and depMgmt."""
    poms, props, mgmt = [], {}, {}
    cg, ca, cv = g, a, v
    while cg:
        p = load_pom(cg, ca, cv)
        if p is None:
            break
        poms.append(p)
        pr = p.find(NS + "properties")
        if pr is not None:
            for c in pr:
                props.setdefault(c.tag.replace(NS, ""), (c.text or "").strip())
        dm = p.find(NS + "dependencyManagement")
        if dm is not None:
            for d in dm.iter(NS + "dependency"):
                if t(d, "scope") == "import":
                    bg, ba, bv = t(d, "groupId"), t(d, "artifactId"), t(d, "version")
                    bom_imports.append("%s:%s:%s" % (bg, ba, bv))
                    bp = load_pom(bg, ba, bv)
                    if bp is not None:
                        for bd in bp.iter(NS + "dependency"):
                            mgmt.setdefault((t(bd, "groupId"), t(bd, "artifactId")),
                                            t(bd, "version"))
                    continue
                mgmt.setdefault((t(d, "groupId"), t(d, "artifactId")), t(d, "version"))
        par = p.find(NS + "parent")
        if par is None:
            break
        cg, ca, cv = t(par, "groupId"), t(par, "artifactId"), t(par, "version")
    props.setdefault("project.version", v)
    return poms, props, mgmt

def interp(s, props):
    for _ in range(4):
        if not s or "${" not in s:
            break
        for k, val in props.items():
            s = s.replace("${%s}" % k, val)
    return s

def resolve(root):
    seen, order, queue = {}, [], [tuple(root.split(":"))]
    while queue:
        g, a, v = queue.pop(0)
        if (g, a) in seen:
            continue
        seen[(g, a)] = v
        order.append((g, a, v))
        poms, props, mgmt = chain(g, a, v)
        if not poms:
            unresolved.append("%s:%s:%s" % (g, a, v))
            continue
        deps = poms[0].find(NS + "dependencies")
        for d in (deps if deps is not None else []):
            scope = t(d, "scope") or "compile"
            if scope not in ("compile", "runtime"):
                continue
            if (t(d, "optional") or "false") == "true":
                continue
            dg, da = interp(t(d, "groupId"), props), interp(t(d, "artifactId"), props)
            dv = t(d, "version")
            dv = interp(dv, props) if dv else mgmt.get((dg, da))
            if dv and dv.startswith("[") and dv.endswith("]") and "," not in dv:
                dv = dv[1:-1]
            if not dv:
                unresolved.append("%s:%s (no version)" % (dg, da))
                continue
            queue.append((dg, da, dv))
    return order

if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    got = resolve(sys.argv[1])
    print("closure: %d artifacts" % len(got))
    total = 0
    for g, a, v in got:
        blob = ext = None
        for e in ("aar", "jar"):
            blob = fetch(coord_path(g, a, v, e))
            if blob:
                ext = e
                break
        n = len(blob) if blob else 0
        total += n
        print("  %-9s %8s KB  %s:%s:%s" % (ext or "MISSING", n // 1024, g, a, v))
    print("total: %.1f MB" % (total / 1048576.0))
    if bom_imports:
        print("BOM imports followed: %s" % ", ".join(sorted(set(bom_imports))))
    if unresolved:
        print("UNRESOLVED: %s" % ", ".join(sorted(set(unresolved))))
