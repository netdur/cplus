# Guide

## Delivery model

`fswatch` deliberately separates native notification collection from callback
delivery. macOS queues vnode changes in `kqueue`; the caller drains that queue
through `Watcher.poll()` or `Watcher.run()`. Snapshot comparison and
`events::Signal` delivery then happen synchronously on that caller's thread.

This makes bound-method receivers follow the existing `events` lifetime rule:
the receiver must outlive its subscription, but it is never called concurrently
by a hidden worker.

## Shallow and recursive watching

`Shallow` snapshots the root plus immediate children. Changes below a child
directory are not emitted. `Recursive` walks all visible descendants and adds
or removes registrations as the tree changes.

Darwin's vnode filter does not provide the changed child's path, and a directory
notification alone does not reliably describe content-only writes to an
existing child. Consequently the backend keeps one descriptor per visible
snapshot node, plus the watched root's parent. This gives reliable writes and
atomic replacements but means very large recursive trees can approach the
process descriptor limit. Ignore rules should prune build caches and dependency
trees that are not relevant.

## Ignore matching

Ignores are fixed when the watcher is constructed. Matching is byte-oriented
and case-sensitive, like normal macOS path equality on a case-sensitive volume.
On a case-insensitive volume the filesystem may treat names as equal even though
the matcher does not.

- no slash: match every basename (`*.tmp`);
- slash present: match the full path relative to the root (`src/*.cplus`);
- `*`: zero or more bytes except `/`;
- `?`: one byte except `/`;
- `**`: zero or more bytes including `/`.

An ignored directory is omitted from the snapshot and never registered with
`kqueue`. Its descendants therefore cost no descriptors and produce no events.

## Snapshot normalization

Every non-empty native queue drain triggers one snapshot diff, regardless of
how many vnode records were coalesced by macOS. The normalized outcomes are:

- path appears: `Created`;
- file identity, size, or modification time changes: `Modified`;
- permission/mode bits change: `Metadata`;
- path disappears: `Removed`;
- a missing old path and new path share an inode: `Renamed`;
- native queue reports an error: rescan plus `Overflow`.

Replacing a file atomically at the same path is `Modified`, and the underlying
registration is rebound to the new inode. Recursive directory renames may
produce a rename for the directory and for visible descendants because every
snapshot path changed.

## Paths and ownership

The watcher owns its root, patterns, snapshots, and native descriptors. A
`Change` contains borrowed `str` views valid only until its callback returns.
Copy a path into `Text` if it must be retained.

`path` is full; `relative_path` is rooted at the watched path. Rename events
also fill `previous_path` and `previous_relative_path`; all other events leave
those fields empty.

## Lifecycle

Dropping a watcher closes every vnode descriptor and its `kqueue`. `stop()`
only stops the cooperative `run()` loop; manual `poll()` remains available.
Nested calls to `poll()` return `WatchError::Busy`.

The package can observe a deleted root because its parent remains registered.
If the same path is recreated, the next parent notification creates a new
snapshot and registration.

## Current platform scope

Three backends behind one seam, each a platform override of `backend.cplus`:
`kqueue` vnode notifications on macOS, `inotify` on Linux, and
`ReadDirectoryChangesW` on Windows. None is emulated with timestamp polling.

The seam is `queue_open` / `watch_path` / `poll_one`, and it survives the three
shapes because the handle it carries is only ever an opaque `i32` — an
`O_EVTONLY` fd on kqueue, a watch descriptor on inotify, a table index on
Windows. Nothing outside a backend interprets it.

**Windows has no queue**, which is the one place the seam had to be synthesised
rather than mapped. `ReadDirectoryChangesW` is issued PER DIRECTORY HANDLE and
its result arrives through an `OVERLAPPED` with an event to wait on, so the
backend keeps a table of watch slots tagged with a queue id and `poll_one` walks
the slots asking each event whether it is signalled, with a zero timeout.

Events are a WAKEUP, not a payload, on all three. `poll_one` never reports what
changed, because the engine above is a snapshot differ that rescans and
compares. On Windows that means the `FILE_NOTIFY_INFORMATION` records are read
and discarded — the buffer exists only because the call requires one — and a
buffer overflow is therefore not a correctness problem: the rescan finds
everything regardless.

### Windows: mtime is coarse, so a fourth field breaks the tie

The engine tells one version of a file from another by (inode, size, mtime).
Windows stamps `LastWriteTime` from the system clock, which advances about every
13ms — the FILETIME has 100ns UNITS but nothing like 100ns RESOLUTION — so two
writes of the same size inside one tick produce three identical fields. Measured,
rewriting a 1-byte file with no gap:

```
before  size=1 mtime=1788879411.288213800
after   size=1 mtime=1788879411.288213800   <- the same, byte for byte
with a 1ms gap: .289201200 -> .302375600    <- one tick apart
```

That is not "reported late". It is **invisible, permanently**: mtime is stamped
at write time, so no amount of waiting makes the second write appear.

macOS and Linux stamp from a high-resolution clock and do not collide, so this
is the one place the three platforms differ in what they can *report*.

**`Metadata` therefore carries a fourth field, `version`**, and the Windows
backend fills it from the NTFS USN — a per-file counter that moves on every
change, read with `FSCTL_READ_FILE_USN_DATA`. macOS and Linux answer a constant
`0`, which compares equal every time and so changes nothing for them.

It costs one `DeviceIoControl` and no extra file open: `metadata` already opens
a handle for `GetFileInformationByHandle`, and the ioctl needs no more access
than the `FILE_READ_ATTRIBUTES` that open already asks for.

**Zero is the answer on any refusal, and that is the whole error policy.** A
non-NTFS volume (FAT32, exFAT, a network share) answers `ERROR_INVALID_FUNCTION`
and a volume whose journal is off answers zeroes. Both come back as `0`, which
restores exactly the old mtime-only behaviour rather than reporting spurious
changes — a watcher that cried wolf on a USB stick would be worse than one that
keeps the 13ms blind spot there.

The regression test is `rapid_same_size_rewrites_are_each_reported`: ten
same-length rewrites, each followed by a poll, all ten seen. It fails without
the tie-breaker, which was confirmed by neutering `_usn_of` and watching it go
red.

### Windows: a file root is watched through its directory

`ReadDirectoryChangesW` takes a DIRECTORY handle, so watching a single file
means watching its parent and filtering. Several file roots in one directory
therefore collapse to one registration, deduplicated by directory identity
(volume serial number plus file index) rather than by path string — two
different spellings of the same directory are the same watch.
