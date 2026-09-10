# Decision record

The questions that come up every time someone reads this crate next to its
older plans, answered once. Statuses: **DECIDED** (the code embodies it;
changing it is a design change, not drift-fixing), **OPEN** (known gap or
deferred choice — the current behavior is stated so nobody mistakes it for
an oversight), and **DECIDED, not yet implemented** (agreed 2026-08-16; the
code still does the old thing until it lands — model.md and operations.md
describe implemented behavior only).

An earlier design (`plans/pm.md`, untracked) described a lockfile, SHA-256
content addressing, and a pubgrub solver. None of those exist, deliberately;
D2, D3, and D8 are the record of why. Where that plan and this file
disagree, this file and the code are the truth.

---

## D1 — One manifest: `Cplus.toml` — DECIDED 2026-06-21

The pm reads the same file the compiler reads. The earlier `pkg.toml`
(id-keyed schema, `[deps.public]`) is retired. The pm's view is deliberately
narrow — `[package] name`/`version` plus the dependency tables — and every
other table is ignored; the compiler's loader is the strict one.

## D2 — Exact pins, no ranges, no solver — DECIDED

Every dependency is either an exact `@version` pin or a sibling that
inherits its parent's pin. There are no version ranges, so there is nothing
to solve: resolution is a deterministic walk, not a search. The monorepo
model is what makes this hold — one repo, one tag, every package inside at
that tag.

*Reopens if:* third-party packages need range constraints (`^1.4`). That
brings a solver and a lockfile (D3) together, as one feature.

## D3 — No lockfile — DECIDED

A lockfile records a resolution when the manifest admits more than one.
Every C+ spec is exact, so the resolved set is a pure function of
`Cplus.toml` — the manifest *is* the lockfile. Writing a second file that
restates it would be drift waiting to happen.

*Reopens with:* D2 (ranges), or D8 if hash pinning is added — a recorded
hash needs a place to live, and that place is a lockfile.

## D4 — Root deps are explicit pins; no default org — DECIDED

The plan's convention — a bare `stdlib = "*"` at the root resolving to
`github.com/netdur/cplus` at the running compiler's version — was **not**
built, and stays out. The pm has no notion of an "official" package, no
baked-in org, and no knowledge of the toolchain version (it does not link
the compiler). A root dependency states its full source; a bare spec at the
root is an error.

Version-locking stdlib to the toolchain happens where that knowledge lives:
**`cpc init`** writes the stdlib pin using its own version. The trade-off is
accepted: upgrading the toolchain does not auto-bump existing pins — you
edit the `@version` (D6 is the door for sugar here).

*Amended by D15 (2026-08-16):* with toolchain tiers, a bare root `*` becomes
meaningful again — but through context `cpc` supplies at the call, not
through a default baked into this crate. The layering above is unchanged.

## D5 — A sibling's version annotation is ignored — DECIDED

`stdlib = "0.0.25"` inside a vendored package's manifest installs at the
*parent's* tag regardless. A sibling lives in the parent's checkout, and the
checkout is at one tag; the monorepo has one version. The annotation is
tolerated for readability, never obeyed. (If it ever should be obeyed, that
package's dep must become a pinned URL — which already works.)

## D6 — `update` = `install`; bumping is a manifest edit — DECIDED

With exact pins, update has nothing to decide: it re-runs the install
materialization, and the way to move a version is to edit `Cplus.toml`.

*Open sugar (not resolver features):* `cpc pm add NAME URL@VER` to write the
line for you, and something to bump an existing pin. Both are CLI
conveniences over manifest edits; neither changes the model. *Resolved by
D17 (2026-08-16):* `add` exists, with closure expansion.

*Amended by D14 (2026-08-16):* post-1.0 the verbs split — `install` resolves
`*` deps to the running binary's exact version, `update` advances within the
patch line. Pre-1.0, `update` = `install` stands as written.

## D7 — The store: `~/.cplus/` — DECIDED & IMPLEMENTED 2026-08-16

`./.pkgcache` is retired. The pm's on-disk home is `~/.cplus/`, the
toolchain's one dotdir, with two jobs under one roof:

```
~/.cplus/
  cache/                      # disposable git clones, keyed (repo, tag) — delete anytime
  v0.0.26/vendor/<name>/      # the STORE: the package set of one toolchain line
  v1.2/vendor/<name>/         # (post-1.0 tier naming — see D13)
```

`cache/` is the only true cache. The versioned tiers are a **store**:
durable state that builds resolve against — which is why the location is a
home dotdir and not `~/Library/Caches` (the OS may purge caches) and not
the project. The old default's wrinkles (CWD-relative, missing from `cpc
init`'s `.gitignore`) disappear because nothing lands in the project at
all. `--cache` remains as an override for CI and sandboxes.

## D8 — Record the commit, enforce only where the network is — DECIDED & IMPLEMENTED 2026-08-16

The convention is: **a release tag is immutable.** The monorepo keeps that
promise socially (every release is tagged, tags never move); third-party
publishers cannot be assumed to — which is exactly why the pm *records*
rather than trusts:

- **At clone time, the resolved commit SHA goes into the `.cplus-vendor`
  stamp** (its second line), in store tiers and project-local `vendor/`
  alike — same stamp code; local copies deserve provenance most. "Which
  bytes is this?" becomes a one-line read.
- **Install with a warm cache stays offline and trusting.** No network
  verification on install, by construction — that property is kept.
- **Enforcement lives wherever a fetch already happened.** The first time a
  tag is fetched, its commit is recorded at `<store root>/tags/<repo>/<tag>`
  — beside the tiers, so purging the cache forgets nothing. Every later
  fetch of that tag is compared to the record; a mismatch is a **hard
  error**: "tag `v1.2.4` of REPO moved from `abc123` to `def456`; a release
  tag is immutable." Never silently accommodated; accepting moved content
  is a deliberate act (the error names the escape hatch: delete the record
  and reinstall). Post-1.0, `update`'s tag listing (D14) joins the same
  check without cloning first.

Accepted one-time cost: the stamp format change invalidates existing
stamps, so everything refetches once on the first install after this lands.

Full verification — checking a recorded hash on every install and refusing
on mismatch — remains the later step for an untrusted third-party world. It
is lockfile-shaped (the hash needs a durable, shareable home; that reopens
D3), and the SHA recorded here is exactly what such a lockfile entry would
contain, so nothing done now is thrown away.

## D9 — Conflicts: first seen wins, and the loser is named — DECIDED & IMPLEMENTED 2026-08-16

Two packages pinning different versions of the same name install whichever
is reached first (breadth-first from the root, alphabetical within each
manifest — shallower wins, ties alphabetically). Because the root manifest
is processed first, any name the root declares is won by the root — and
that is the correct outcome, not an accident: the root is the bill of
materials. No solver arbitrates (D2). In the monorepo world a conflict
cannot arise — every sibling shares one tag — so this only enters through
explicit pins in transitive manifests.

The losing request is reported — one line per loss, install continues:

```
parser: installed 1.4.2 (via the root manifest); vendor/b wanted 1.5.0
```

An error was considered and rejected: it would let a dependency's internal
declaration veto the project's own manifest, inverting who owns the BOM.
The root wins, audibly.

## D10 — `vendor/` is pm-owned, edit-tolerant — DECIDED

A package whose stamp matches its pin is never touched, so local edits
survive re-installs — until the pin changes, at which point the directory is
replaced wholesale and edits are lost. Working on a vendored package is the
build side's `[build] dev = true` mode, not a pm state. The pm's contract is
exactly: `vendor/<name>` matches the pin, or gets replaced until it does.

## D11 — The pm places, the build validates — DECIDED

Everything that checks `vendor/` contents against the manifest-is-truth
contract lives in `cpc build`: E0854 (missing `Cplus.toml`), E0855
(name/directory mismatch), E0860/E0861 (bundled binaries vs `[link]`), the
prebuild/dev machinery, platform gating of imports (E0866). The pm performs
one validation of its own — `<subpath>/Cplus.toml` must exist at the tag,
so a typoed URL fails at install rather than at build — and otherwise copies
what the package ships. Source vs binary is invisible to it (model.md §6).

## D12 — The transitive walk stays, despite flat resolution — DECIDED

The compiler resolves imports against the root manifest only; the root names
the complete bill of materials ([docs/lang/packages.md](../../docs/lang/packages.md)).
So in a correct project the pm's transitive walk discovers nothing new. It
stays anyway: it materializes a monorepo's sibling closure from one clone,
and it turns an under-declared root into a *compile-time* error naming the
missing manifest line, instead of a missing-directory failure. The walk is a
safety net, not an alternative to declaring the closure.

*Consequence, and the open ergonomic gap:* an external project adding a
backend must write its whole closure as pinned URLs at the root — eight-ish
lines for `facet` on macOS. That is the flat-resolution rule, not a pm rule;
the `cpc pm add` sugar of D6 is where it would be softened (add a package,
expand its closure into the manifest, keep the manifest the complete BOM).
*Amended by D15:* for toolchain packages those lines become bare `*`
entries, which takes most of the sting out even before `add` exists.

## D13 — Toolchain tiers: the version is the universe — DECIDED & IMPLEMENTED 2026-08-16

The store is partitioned by the **compatibility line** of the running
toolchain: the exact version pre-1.0 (`v0.0.26` — every release its own
universe), `major.minor` post-1.0 (`v1.2` — what may move within a line is
D14). The tier a `cpc` uses is derived from its own version, so crossing
tiers is impossible by construction: a 0.0.26 binary resolves inside
`v0.0.26/` and the packages of 0.0.27 are a universe it never looks at —
no check enforces this, the layout does.

Consequences, all intended:

- **Toolchain upgrade = clean slate.** A new cpc starts an empty tier and
  installs fresh — no stale-source or stale-slice hazard. Old tiers keep
  serving old binaries; a `cpc pm gc` for tiers whose cpc is gone is an
  easy later add.
- **Prebuild slices are shared.** A `[build] prebuild` package's slice lives
  in its store tier: compiled once per machine per line, not once per
  project. (Implementation note: concurrent builds writing a shared slice
  need atomic-rename discipline.)

## D14 — Update is bounded by the patch digit — DECIDED 2026-08-16; activates post-1.0

Post-1.0 the version digits read: major = API break, minor = feature add,
patch = bug fix. `cpc pm update` may move a package **within the patch line
only**; minor and major arrive exclusively as a new toolchain, which is a
new tier (D13).

Why patch and not minor: the language is versioned by the same numbers. A
minor stdlib may use minor language features, so cpc 1.2 may be unable to
even compile stdlib 1.3 — update must never hand the compiler a package it
cannot build. A patch adds no features by definition, so stdlib 1.2.x stays
within the 1.2 language surface. Second reason: cpc embeds its own docs
(skill, error catalog); a patch cannot change API surface, so the embedded
docs stay truthful under patch-floating — a minor float would break them.

The verbs split (post-1.0):

- **`install`** resolves `*` deps to the running binary's **exact** version.
  Deterministic, no tag search, same result on every machine.
- **`update`** is the one verb that looks for newer: list the repo's tags,
  take the newest within the line, advance the tier. Accepted drift:
  machines that ran update at different times may sit on different patches
  of one line — exactly the drift the compatibility contract declares
  harmless.

Pre-1.0 the tier is exact, there is nothing to advance within it, and
`update` remains `install` (D6).

**The obligation this creates:** patch-floating is sound only if every
package patch in a line builds and passes under the line's **oldest**
compiler — a stdlib fix must not depend on a same-patch compiler fix. That
is a CI gate on the monorepo release ("build the line's packages with the
line's oldest cpc"), and it ships with this decision or the contract is
words.

## D15 — Bare `*` at the root, via toolchain context — DECIDED & IMPLEMENTED 2026-08-16; amends D4

With tiers, a bare `stdlib = "*"` at the project root becomes meaningful:
the tier answers *which version*, and cpc answers *which repo*. `cpc pm`
passes the pm a toolchain context (repo + version) the same way `cpc init`
already knows its own version. D4's layering survives intact — the
`cplus-pm` crate still has no notion of "official" and no baked-in org; the
*toolchain* is what supplies the context. `cpc init` scaffolds
`stdlib = "*"` instead of the long URL. Tree-URL pins remain the form for
third-party packages and for deliberately pinning outside the convention.

## D16 — Store resolution: local wins, divergence goes local — DECIDED & IMPLEMENTED 2026-08-16

Build resolution order: `<project>/vendor/<name>` first, else
`~/.cplus/<tier>/vendor/<name>`. The store path becomes shared vocabulary
between `cpc` and `cplus-pm`, the way `vendor/` already is. Flat resolution
(D12) is untouched — the manifest still names the complete bill of
materials; only where the files sit gains a second answer.

There is **no package-version tier inside the store.** For toolchain
packages it would restate the tier (`v0.0.26/vendor/0.0.26/stdlib` says the
same thing twice); for third-party the conflict is real but rare and has a
cheaper answer: a tier holds one directory per name, stamped with its
provenance (`repo@version subpath`, as today), and a project whose pin
differs from the stored stamp — different version *or* different repo —
gets its copy vendored into its own `<project>/vendor/` instead of
thrashing the shared one. Divergence creates locality; agreement shares.
Versioned store dirs (`parser@1.4.2/`) are the escape hatch if third-party
multi-version ever earns the complexity.

**Install is global by default.** `cpc pm install` populates the store; a
freshly-installed project has no `vendor/` directory at all, and builds
resolve from the store. Installing into the project is the explicit case:
an install flag (`--local` as the working name; spelling is an
implementation detail) vendors into `<project>/vendor/` instead.

Local `vendor/` stays, for exactly three reasons: divergent pins (above,
vendored locally by the pm itself), `[build] dev = true` work, and projects
that deliberately vendor (commit) their dependencies via the flag.

## D17 — `cpc pm add`: closure expansion, target-scoped — DECIDED 2026-08-16 & IMPLEMENTED 2026-08-17

`cpc pm add <name>` is how a dependency enters a manifest without a
documentation hunt. It fetches the package (into the store, as install
would), reads the *package's own* manifest, and writes into the project's:
the package's line plus its declared closure, platform sections mapped onto
matching platform sections. The package's manifest is the source of truth —
facet's `[dependencies]` / `[macos.dependencies]` / `[ios.dependencies]`
already carry exactly this knowledge; nothing new is invented.

Which platform sections get written — the project's **target set**, never a
guess:

1. Every platform the project manifest already mentions (a platform entry
   or an existing `[<platform>.dependencies]` section): the project said
   so.
2. If it mentions none: the **host**, the one target a fresh undeclared
   project certainly has.
3. `--platform <p>` extends the set explicitly.

Host vs target, said once: platform sections describe **targets**; the host
is where the compiler runs, and packages follow the target. An iOS-only
project on a macOS host gets facet and the `[ios.dependencies]` closure and
no `[macos.dependencies]` at all — facet_appkit is not missing there, it is
a package that project never compiles or links, and the build side already
treats an off-target dep's vendor dir as legitimately absent.

`add` is idempotent: deciding to target iOS next month is `cpc pm add facet
--platform ios` again — it fills only the missing sections. It edits the
manifest surgically, preserving comments and formatting. Rejected:
expanding all platforms unconditionally — the manifest is the readable bill
of materials, and sections for platforms a project will never target are
noise for every future reader.

---

## D18 — Maven/AAR dependencies, no Gradle — DECIDED & IMPLEMENTED 2026-09-03

The Android toolchain ships no dependency resolver. There is no `mvn` and no
`cs`; Android Studio carries maven-resolver as a LIBRARY for its own
indexing, and Gradle itself is downloaded by the wrapper rather than
installed. **Gradle is the resolver.** But resolution over pinned coordinates
is reading XML, and an AAR is a zip — so the pm does that, and the closure
dexes with stock `d8`. `plans/aar.md` is the measurement that settled it;
`cpc pm maven price androidx.camera:camera-camera2:1.6.2` reproduces it (35
artifacts, 8.2 MB, one 6,971,948-byte dex).

**The manifest surface is `[android.maven]`**, `"group:artifact" = "version"`.
Android-only by construction — the table under any other platform is E0877,
not a silently ignored key, because Maven is an Android ecosystem and a
coordinate that is quietly dropped is a class missing at runtime on a device.

**Only the ROOT coordinate is written**, unlike D17's closure expansion for
C+ packages. A coordinate's POM is immutable, so its closure is a function of
the pin and re-deriving it is reading cached XML. Writing 35 transitive
coordinates into `Cplus.toml` would be a lockfile with extra steps (D3), and
it would go stale the moment the root version changed. The closure is
PRINTED instead — the artifact count and the megabytes are what the decision
turns on.

**The local repo is `~/.cplus/m2`, and it is NOT tiered** (unlike the package
set, D13). A Maven coordinate is immutable: `androidx.core:core:1.3.2` is the
same bytes to every toolchain version, so tiering it would only duplicate
megabytes. It is laid out as a real Maven repo, and AARs are exploded beside
their archive — `d8` wants `classes.jar`, not a zip.

**An incomplete closure installs nothing.** A missing transitive artifact is
a `NoClassDefFoundError` on a device, and unlike a C+ package there is no
link step that would have caught it first.

**Conflicts are nearest-wins** — Maven's rule, applied the way D9 applies it
to git deps (the project is nearest, so the project wins what it names).
Gradle uses highest-wins, so the two can disagree, and the report says
exactly where: a conflict is warned about only when nearest-wins kept the
OLDER version. One CameraX resolve has 26 version conflicts and diverges from
Gradle in 2 of them; reporting all 26 buries the two that matter.

**Not handled, and loud about it:** open version ranges, classifiers,
exclusions, mirrors, and `.module` Gradle metadata. The last one is the
interesting exemption — AndroidX publishes `.module` and its POM is a
compatibility shim, but the shim declares the platform variant
(`tracing-android`) as an ordinary `compile` dependency, so the code arrives
through the closure anyway and the facade AAR simply carries no
`classes.jar`. That is why ignoring variant selection survives plain AAR
consumption.

**Still the build script's job:** merging the AAR manifest fragments and
running `aapt2` over their `res/`. `cpc pm maven manifests|res|jni` names
those inputs so a build can reach them; assembling them is
`plans/third-party-sdks.md` §3, and it is the biggest single work item there.
Rejected: doing that half now — the closures worth reaching for first (Play
Services, ML Kit) are code-heavy and resource-light, so `classpath` into `d8`
is already a working pipeline.
