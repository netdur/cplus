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

The backend currently describes Darwin ABI layouts and constants. Linux
`inotify` and Windows `ReadDirectoryChangesW` backends are planned as platform
overrides; they are not silently emulated with timestamp polling.
