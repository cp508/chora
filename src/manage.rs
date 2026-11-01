//! Note files and dirs for library users (mobile): create, rename, trash.
//!
//! Paths are relative to `notes/` and go through [`join_safe`]. Deleting
//! never unlinks: entries move to `<root>/trash/<YYYY-MM-DD>/<path>`, and
//! [`purge_trash`] drops day folders older than [`TRASH_DAYS`].

use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::NaiveDate;

use crate::error::ChoraError;
use crate::note::{join_safe, require_md};

/// Days a trashed entry is kept.
pub const TRASH_DAYS: i64 = 30;

/// Trash folder under the storage root.
#[must_use]
pub fn trash_dir(root: &Path) -> PathBuf {
    root.join("trash")
}

/// Create `rel` under `notes`: a trailing `/` makes a dir, else an empty
/// note (`.md` added when missing). Returns the created path.
///
/// # Errors
///
/// Bad path, an existing target, or an IO failure.
pub fn create(notes: &Path, rel: &str) -> anyhow::Result<String> {
    let rel = rel.trim();
    let (rel, dir) = match rel.strip_suffix('/') {
        Some(stem) => (stem.trim_end_matches('/').to_owned(), true),
        None if rel.ends_with(".md") => (rel.to_owned(), false),
        None => (format!("{rel}.md"), false),
    };
    let path = join_safe(notes, &rel)?;
    if path.exists() {
        return Err(ChoraError::NoteExists(rel).into());
    }
    if dir {
        std::fs::create_dir_all(&path)
            .with_context(|| format!("failed to create {}", path.display()))?;
    } else {
        crate::fsx::write_atomic(&path, "")?;
    }
    Ok(rel)
}

/// Move `from` to `to` (both under `notes`); notes keep the `.md` suffix.
///
/// # Errors
///
/// Bad path, a missing source, an existing target, or an IO failure.
pub fn rename(notes: &Path, from: &str, to: &str) -> anyhow::Result<()> {
    let (src, dst) = (join_safe(notes, from)?, join_safe(notes, to.trim())?);
    if src.is_file() {
        require_md(to.trim())?;
    } else if !src.is_dir() {
        return Err(ChoraError::NoteBadPath(format!("{from}: not found")).into());
    }
    if dst.exists() {
        return Err(ChoraError::NoteExists(to.trim().to_owned()).into());
    }
    if dst.starts_with(&src) {
        return Err(ChoraError::NoteBadPath(format!("{to}: inside {from}")).into());
    }
    move_entry(&src, &dst)
}

/// Move `rel` (file or dir under `<root>/notes`) into today's trash folder.
/// Returns the new path relative to `root`.
///
/// # Errors
///
/// Bad path, a missing source, or an IO failure.
pub fn trash(root: &Path, rel: &str, today: NaiveDate) -> anyhow::Result<String> {
    let src = join_safe(&root.join("notes"), rel)?;
    if !src.exists() {
        return Err(ChoraError::NoteBadPath(format!("{rel}: not found")).into());
    }
    let day = trash_dir(root).join(today.format("%Y-%m-%d").to_string());
    let mut dst = join_safe(&day, rel)?;
    // Same path trashed twice a day: keep both.
    for n in 2.. {
        if !dst.exists() {
            break;
        }
        dst = join_safe(&day, &format!("{rel}.{n}"))?;
    }
    move_entry(&src, &dst)?;
    Ok(crate::note::rel_of(root, &dst))
}

/// Delete trash day folders older than [`TRASH_DAYS`]; returns how many.
/// Anything that is not a `YYYY-MM-DD` folder is left alone.
///
/// # Errors
///
/// IO failure while listing or deleting.
pub fn purge_trash(root: &Path, today: NaiveDate) -> anyhow::Result<usize> {
    let dir = trash_dir(root);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err).with_context(|| format!("failed to list {}", dir.display())),
    };
    let mut purged = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Ok(day) = NaiveDate::parse_from_str(&name.to_string_lossy(), "%Y-%m-%d") else {
            continue;
        };
        if (today - day).num_days() > TRASH_DAYS && entry.file_type()?.is_dir() {
            std::fs::remove_dir_all(entry.path())
                .with_context(|| format!("failed to delete {}", entry.path().display()))?;
            purged += 1;
        }
    }
    Ok(purged)
}

fn move_entry(src: &Path, dst: &Path) -> anyhow::Result<()> {
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    std::fs::rename(src, dst)
        .with_context(|| format!("failed to move {} to {}", src.display(), dst.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    #[test]
    fn create_makes_notes_and_dirs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tmp.path();
        assert_eq!(create(notes, "work/ideas").unwrap(), "work/ideas.md");
        assert!(notes.join("work/ideas.md").is_file());
        assert_eq!(create(notes, " work/sub/ ").unwrap(), "work/sub");
        assert!(notes.join("work/sub").is_dir());
        assert!(create(notes, "work/ideas.md").is_err(), "exists");
        assert!(create(notes, "../evil/").is_err());
        assert!(create(notes, "/").is_err());
    }

    #[test]
    fn rename_moves_and_refuses_clobber_or_nesting() {
        let tmp = tempfile::tempdir().unwrap();
        let notes = tmp.path();
        create(notes, "a/x.md").unwrap();
        create(notes, "a/y.md").unwrap();
        rename(notes, "a/x.md", "b/z.md").unwrap();
        assert!(notes.join("b/z.md").is_file() && !notes.join("a/x.md").exists());
        assert!(rename(notes, "a/y.md", "b/z.md").is_err(), "exists");
        assert!(rename(notes, "a/y.md", "a/y.txt").is_err(), "not md");
        assert!(rename(notes, "a", "a/inner").is_err(), "into itself");
        assert!(rename(notes, "missing.md", "m.md").is_err());
        rename(notes, "a", "c").unwrap();
        assert!(notes.join("c/y.md").is_file());
    }

    #[test]
    fn trash_keeps_both_copies_and_purge_drops_old_days() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let notes = root.join("notes");
        create(&notes, "w/n.md").unwrap();
        assert_eq!(
            trash(root, "w/n.md", day(1)).unwrap(),
            "trash/2026-11-01/w/n.md"
        );
        create(&notes, "w/n.md").unwrap();
        assert_eq!(trash(root, "w", day(1)).unwrap(), "trash/2026-11-01/w.2");
        assert!(!notes.join("w").exists());
        assert!(trash(root, "w", day(1)).is_err(), "gone");
        std::fs::create_dir_all(trash_dir(root).join("keep-me")).unwrap();
        create(&notes, "late.md").unwrap();
        trash(root, "late.md", day(20)).unwrap();
        let in_31 = day(1) + chrono::Days::new(31);
        assert_eq!(purge_trash(root, in_31).unwrap(), 1);
        assert!(trash_dir(root).join("2026-11-20/late.md").is_file());
        assert!(trash_dir(root).join("keep-me").is_dir());
        assert_eq!(purge_trash(&root.join("none"), in_31).unwrap(), 0);
    }
}
