//! Syncthing conflict copies and a cheap change stamp for the storage.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::Context;

use crate::error::ChoraError;
use crate::note;

const MARKER: &str = ".sync-conflict-";

/// One conflict copy, paths relative to the storage root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The `.sync-conflict-` copy.
    pub path: String,
    /// The file it conflicts with.
    pub original: String,
}

/// Every conflict copy under `root`, sorted; dot dirs (`.git`, `.stversions`) skipped.
///
/// # Errors
///
/// Fails when a directory cannot be listed.
pub fn list(root: &Path) -> anyhow::Result<Vec<Conflict>> {
    let mut out = Vec::new();
    collect(root, root, &mut out)?;
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<Conflict>) -> anyhow::Result<()> {
    let entries =
        std::fs::read_dir(dir).with_context(|| format!("failed to list {}", dir.display()))?;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect(root, &path, out)?;
        } else if let Some(original) = original_name(&name) {
            out.push(Conflict {
                path: note::rel_of(root, &path),
                original: note::rel_of(root, &path.with_file_name(original)),
            });
        }
    }
    Ok(())
}

/// `todo.sync-conflict-20261101-101010-ABCDEFG.md` -> `todo.md`.
#[must_use]
pub fn original_name(name: &str) -> Option<String> {
    let at = name.find(MARKER)?;
    let tail = &name[at + MARKER.len()..];
    let ext = tail.find('.').map_or("", |dot| &tail[dot..]);
    Some(format!("{}{ext}", &name[..at]))
}

/// Resolve the copy at root-relative `rel`: keep the current file (delete the
/// copy) or replace the current file with the copy.
///
/// # Errors
///
/// [`ChoraError::NotAConflict`] for other paths, unsafe paths, IO errors.
pub fn resolve(root: &Path, rel: &str, keep_copy: bool) -> anyhow::Result<()> {
    let path = note::join_safe(root, rel)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let original = original_name(&name).ok_or_else(|| ChoraError::NotAConflict(rel.to_owned()))?;
    if keep_copy {
        std::fs::rename(&path, path.with_file_name(original))
    } else {
        std::fs::remove_file(&path)
    }
    .with_context(|| format!("failed to resolve {rel}"))
}

/// Fingerprint of `index.md`, `notes/` and `events/` from metadata only
/// (paths, sizes, mtimes). Equal stamps: nothing visible changed.
///
/// # Errors
///
/// Fails when a directory cannot be listed.
pub fn stamp(root: &Path) -> anyhow::Result<u64> {
    let mut hasher = DefaultHasher::new();
    for rel in ["index.md", "notes", "events"] {
        hash_tree(&root.join(rel), &mut hasher)?;
    }
    Ok(hasher.finish())
}

fn hash_tree(path: &Path, hasher: &mut DefaultHasher) -> anyhow::Result<()> {
    let meta = match std::fs::metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).with_context(|| format!("failed to stat {}", path.display())),
    };
    path.hash(hasher);
    if meta.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .with_context(|| format!("failed to list {}", path.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for entry in entries {
            hash_tree(&entry, hasher)?;
        }
    } else {
        meta.len().hash(hasher);
        meta.modified().ok().hash(hasher);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_and_resolves_both_ways() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let copy = "notes/todo.sync-conflict-20261101-101010-ABCDEFG.md";
        std::fs::create_dir_all(root.join("notes")).unwrap();
        std::fs::write(root.join("notes/todo.md"), "mine").unwrap();
        std::fs::write(root.join(copy), "theirs").unwrap();
        let found = list(root).unwrap();
        assert_eq!(
            found,
            [Conflict {
                path: copy.into(),
                original: "notes/todo.md".into()
            }]
        );
        resolve(root, copy, true).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("notes/todo.md")).unwrap(),
            "theirs"
        );
        std::fs::write(root.join(copy), "again").unwrap();
        resolve(root, copy, false).unwrap();
        assert!(list(root).unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("notes/todo.md")).unwrap(),
            "theirs"
        );
        assert!(resolve(root, "notes/todo.md", false).is_err());
        assert!(resolve(root, "../x.sync-conflict-1.md", false).is_err());
    }

    #[test]
    fn stamp_moves_only_on_change() {
        let tmp = tempfile::tempdir().unwrap();
        let before = stamp(tmp.path()).unwrap();
        assert_eq!(before, stamp(tmp.path()).unwrap());
        std::fs::create_dir_all(tmp.path().join("notes")).unwrap();
        std::fs::write(tmp.path().join("notes/a.md"), "x").unwrap();
        assert_ne!(before, stamp(tmp.path()).unwrap());
    }
}
