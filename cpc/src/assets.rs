//! Project assets: `<root>/assets/*`, staged for whatever runs or packages the
//! build.
//!
//! THE RULE IS ONE RULE, AND IT LIVES HERE. A name is relative to `assets/`:
//! `assets/defaults/settings.json` is the asset `defaults/settings.json`. That
//! is what `appenv::read_asset` resolves against `resource_dir()`, and what
//! facet's `imageNamed:` / `AssetManager.open` look up. Before this module the
//! rule existed only in `cpc package`, and every other tool that built a
//! container re-derived it — iris three times, once with the `assets/` folder
//! kept, so one name worked in one build and failed in the other
//! (bugs/closed/project-assets-reach-only-a-packaged-macos-app.md).
//!
//! Two places get the files:
//!
//! - **The staging directory**, `target/assets/<target>/<mode>/` (`host` for a
//!   native build), on every target. Its contents are exactly the project's
//!   assets, nothing else. It is deliberately NOT inside `target/<mode>/`:
//!   that is a bare binary's resource directory, and a staged `assets/` there
//!   made `read_asset("assets/x")` succeed in `cpc build` and fail in every
//!   package — the very disagreement this module exists to end. It is what
//!   `cpc package` copies into `Contents/Resources/`, and what an outside
//!   builder (iris, Xcode, Gradle) copies into an `.app` or hands to
//!   `aapt2 link -A`; `cpc build --print-asset-dir` prints it.
//! - **Beside a bare binary**, on a self-linked platform, because for a binary
//!   outside a bundle `appenv::resource_dir()` IS the executable's directory.
//!   Without this `cpc build && ./target/debug/app` could read no asset at all.
//!   These are hard links to the staged files where the filesystem allows it,
//!   so a large model is not stored twice, and they are listed in a stamp file
//!   so a removed asset does not outlive its source.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What `mirror_beside` placed beside the binary last time, one entry per line:
/// `f <rel>` for a file, `d <rel>` for a directory it created.
const STAMP: &str = ".cplus-assets";

/// `<root>/target/assets/<target>/<mode>` — the staging directory for one
/// target (`host`, `ios-arm64-simulator`, ...) and mode (`debug`/`release`).
pub fn staging_dir(root: &Path, target: &str, mode: &str) -> PathBuf {
    root.join("target").join("assets").join(target).join(mode)
}

/// Bring `dst` (a `staging_dir`) to exactly the contents of `<root>/assets/`
/// and return it. Always leaves a directory, empty when the project has no
/// assets, so a consumer can copy `"$dir/."` without testing for it first.
///
/// Incremental: a file whose size and modification time match its source is
/// not copied again (a copy carries its source's mtime), so a multi-gigabyte
/// model costs one copy, not one per build. Anything in the staging directory
/// that the source no longer has is removed.
pub fn stage(root: &Path, dst: &Path) -> io::Result<PathBuf> {
    let dst = dst.to_path_buf();
    let src = root.join("assets");
    if src.is_dir() {
        sync_tree(&src, &dst)?;
    } else {
        let _ = fs::remove_dir_all(&dst);
        fs::create_dir_all(&dst)?;
    }
    Ok(dst)
}

fn sync_tree(src: &Path, dst: &Path) -> io::Result<()> {
    if fs::symlink_metadata(dst).is_ok_and(|m| !m.is_dir()) {
        fs::remove_file(dst)?;
    }
    fs::create_dir_all(dst)?;
    // Removals first: an entry whose kind changed (a file that became a
    // directory) is removed here and recreated below.
    for entry in fs::read_dir(dst)? {
        let entry = entry?;
        let theirs = src.join(entry.file_name());
        let ours = entry.file_type()?;
        let keep = match fs::metadata(&theirs) {
            Ok(m) => m.is_dir() == ours.is_dir(),
            Err(_) => false,
        };
        if !keep {
            if ours.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
    }
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        // `metadata` follows a symlink: a linked file or folder in assets/ is
        // staged as what it points at, which is what a bundle can carry.
        let meta = fs::metadata(&from)?;
        if meta.is_dir() {
            sync_tree(&from, &to)?;
        } else if !same_file_state(&meta, &to) {
            let _ = fs::remove_file(&to);
            fs::copy(&from, &to)?;
            if let Ok(t) = meta.modified() {
                let _ = fs::File::options().write(true).open(&to).and_then(|f| f.set_modified(t));
            }
        }
    }
    Ok(())
}

fn same_file_state(src: &fs::Metadata, dst: &Path) -> bool {
    let Ok(d) = fs::metadata(dst) else {
        return false;
    };
    d.is_file() && d.len() == src.len() && d.modified().ok() == src.modified().ok()
}

/// Place the staged files flat beside a bare binary in `dir`, replacing what
/// the previous build placed there. A staged top-level name that is already
/// taken by something this step did not put there (the binary, its `.objs/`,
/// a packaged `.app`) is an error naming it —
/// overwriting the binary with a picture would be a worse surprise.
pub fn mirror_beside(staged: &Path, dir: &Path) -> Result<(), String> {
    let stamp = dir.join(STAMP);
    remove_previous(dir, &stamp);
    let mut placed: Vec<String> = Vec::new();
    let result = (|| -> Result<(), String> {
        let mut top: Vec<_> = fs::read_dir(staged)
            .map_err(|e| format!("reading {}: {e}", staged.display()))?
            .filter_map(|e| e.ok())
            .collect();
        top.sort_by_key(|e| e.file_name());
        for entry in &top {
            let to = dir.join(entry.file_name());
            if fs::symlink_metadata(&to).is_ok() {
                return Err(format!(
                    "the asset `{}` would replace {}, which the build owns — rename it in assets/",
                    entry.file_name().to_string_lossy(),
                    to.display()
                ));
            }
        }
        for entry in &top {
            place(&entry.path(), dir, Path::new(&entry.file_name()), &mut placed)?;
        }
        Ok(())
    })();
    // Written even on failure, so whatever was placed is cleaned up next time.
    let _ = fs::write(&stamp, placed.join("\n"));
    result
}

fn place(from: &Path, dir: &Path, rel: &Path, placed: &mut Vec<String>) -> Result<(), String> {
    let to = dir.join(rel);
    let shown = rel.to_string_lossy().replace('\\', "/");
    let meta = fs::metadata(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
    if meta.is_dir() {
        if !to.is_dir() {
            fs::create_dir(&to).map_err(|e| format!("creating {}: {e}", to.display()))?;
            placed.push(format!("d {shown}"));
        }
        let entries = fs::read_dir(from).map_err(|e| format!("reading {}: {e}", from.display()))?;
        for entry in entries.filter_map(|e| e.ok()) {
            place(&entry.path(), dir, &rel.join(entry.file_name()), placed)?;
        }
        return Ok(());
    }
    if fs::hard_link(from, &to).is_err() {
        fs::copy(from, &to).map_err(|e| format!("copying {} to {}: {e}", from.display(), to.display()))?;
    }
    placed.push(format!("f {shown}"));
    Ok(())
}

fn remove_previous(dir: &Path, stamp: &Path) {
    let Ok(listed) = fs::read_to_string(stamp) else {
        return;
    };
    let mut dirs: Vec<PathBuf> = Vec::new();
    for line in listed.lines() {
        let (kind, rel) = line.split_at(line.len().min(2));
        // Nothing outside `dir` is ever named, whatever the stamp says.
        if rel.is_empty() || rel.starts_with('/') || rel.split('/').any(|c| c == ".." || c.is_empty()) {
            continue;
        }
        let path = dir.join(rel);
        match kind {
            "f " => {
                let _ = fs::remove_file(&path);
            }
            "d " => dirs.push(path),
            _ => {}
        }
    }
    // Deepest first; a directory that still holds something not ours stays.
    for d in dirs.iter().rev() {
        let _ = fs::remove_dir(d);
    }
    let _ = fs::remove_file(stamp);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    #[test]
    fn staging_is_flat_and_exact() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("target/debug");
        let st = staging_dir(root.path(), "host", "debug");
        write(&root.path().join("assets/probe.txt"), "flat-root");
        write(&root.path().join("assets/defaults/probe.txt"), "nested");
        let dir = stage(root.path(), &st).unwrap();
        assert_eq!(dir, root.path().join("target/assets/host/debug"));
        assert_eq!(fs::read_to_string(dir.join("probe.txt")).unwrap(), "flat-root");
        assert_eq!(fs::read_to_string(dir.join("defaults/probe.txt")).unwrap(), "nested");
        assert!(!dir.join("assets").exists(), "the assets/ folder itself is not kept");

        // A removed asset leaves the staging directory; a changed one is recopied.
        fs::remove_file(root.path().join("assets/defaults/probe.txt")).unwrap();
        write(&root.path().join("assets/probe.txt"), "changed!!");
        stage(root.path(), &st).unwrap();
        assert!(!dir.join("defaults/probe.txt").exists());
        assert_eq!(fs::read_to_string(dir.join("probe.txt")).unwrap(), "changed!!");
    }

    #[test]
    fn a_changed_file_of_the_same_size_is_recopied() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("target/debug");
        let st = staging_dir(root.path(), "host", "debug");
        let src = root.path().join("assets/a.txt");
        write(&src, "one");
        stage(root.path(), &st).unwrap();
        write(&src, "two");
        // Same length; only the mtime tells them apart. Force it to differ even
        // on a filesystem with coarse timestamps.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        fs::File::options().write(true).open(&src).unwrap().set_modified(later).unwrap();
        stage(root.path(), &st).unwrap();
        assert_eq!(fs::read_to_string(st.join("a.txt")).unwrap(), "two");
    }

    #[test]
    fn no_assets_still_leaves_an_empty_staging_directory() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("target/debug");
        let st = staging_dir(root.path(), "host", "debug");
        write(&st.join("stale.txt"), "from an earlier build");
        let dir = stage(root.path(), &st).unwrap();
        assert!(dir.is_dir());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn mirror_places_beside_the_binary_and_cleans_up_after_itself() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("target/debug");
        let st = staging_dir(root.path(), "host", "debug");
        write(&out.join("app"), "binary");
        write(&root.path().join("assets/probe.txt"), "flat-root");
        write(&root.path().join("assets/defaults/probe.txt"), "nested");
        let staged = stage(root.path(), &st).unwrap();
        mirror_beside(&staged, &out).unwrap();
        assert_eq!(fs::read_to_string(out.join("probe.txt")).unwrap(), "flat-root");
        assert_eq!(fs::read_to_string(out.join("defaults/probe.txt")).unwrap(), "nested");

        fs::remove_dir_all(root.path().join("assets/defaults")).unwrap();
        let staged = stage(root.path(), &st).unwrap();
        mirror_beside(&staged, &out).unwrap();
        assert!(!out.join("defaults").exists(), "a removed asset folder is gone beside the binary too");
        assert_eq!(fs::read_to_string(out.join("app")).unwrap(), "binary");
    }

    #[test]
    fn mirror_refuses_to_replace_what_the_build_owns() {
        let root = tempfile::tempdir().unwrap();
        let out = root.path().join("target/debug");
        let st = staging_dir(root.path(), "host", "debug");
        write(&out.join("app"), "binary");
        write(&root.path().join("assets/app"), "a picture");
        let staged = stage(root.path(), &st).unwrap();
        let err = mirror_beside(&staged, &out).unwrap_err();
        assert!(err.contains("`app`"), "{err}");
        assert_eq!(fs::read_to_string(out.join("app")).unwrap(), "binary");
    }
}
