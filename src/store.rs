//! Storage layout: `events/`, `notes/`, day files.
//!
//! No state cache — the state is the files themselves plus git.

use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::NaiveDate;

/// Storage root (`chora_dir` from the config, or `--dir`).
pub struct Store {
    root: PathBuf,
}

impl Store {
    /// A new store over the root. The root does not need to exist yet.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Storage root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The `events/` directory (day files live here).
    #[must_use]
    pub fn events_dir(&self) -> PathBuf {
        self.root.join("events")
    }

    /// Pre-rename location, moved aside once in [`Self::ensure_layout`].
    fn calendar_legacy_dir(&self) -> PathBuf {
        self.root.join("calendar")
    }

    /// The `notes/` directory.
    #[must_use]
    pub fn notes_dir(&self) -> PathBuf {
        self.root.join("notes")
    }

    /// Day file path: strictly ISO `events/YYYY-MM-DD.md`.
    #[must_use]
    pub fn day_path(&self, date: NaiveDate) -> PathBuf {
        self.events_dir()
            .join(format!("{}.md", date.format("%Y-%m-%d")))
    }

    /// Create `events/` + `notes/`. Idempotent.
    ///
    /// One-time migration: a pre-rename `calendar/` is moved to `events/`
    /// when `events/` does not exist yet.
    pub fn ensure_layout(&self) -> anyhow::Result<()> {
        let legacy = self.calendar_legacy_dir();
        let events = self.events_dir();
        if legacy.exists() && !events.exists() {
            std::fs::rename(&legacy, &events).with_context(|| {
                format!(
                    "failed to move {} to {}",
                    legacy.display(),
                    events.display()
                )
            })?;
            crate::debug_log!("moved {} to {}", legacy.display(), events.display());
        }
        for dir in [events, self.notes_dir()] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        Ok(())
    }

    /// `.gitignore` against editor junk. Never touches a user file.
    pub fn ensure_gitignore(&self) -> anyhow::Result<()> {
        let path = self.root.join(".gitignore");
        if !path.exists() {
            std::fs::write(&path, "*.swp\n*~\n.DS_Store\n")
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        Ok(())
    }

    /// Empty-day template: a single `# YYYY-MM-DD` header.
    #[must_use]
    pub fn day_template(date: NaiveDate) -> String {
        format!("# {}\n", date.format("%Y-%m-%d"))
    }

    /// Create the day file from the template if missing. Returns the path.
    pub fn ensure_day_file(&self, date: NaiveDate) -> anyhow::Result<PathBuf> {
        let path = self.day_path(date);
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            std::fs::write(&path, Self::day_template(date))
                .with_context(|| format!("failed to create {}", path.display()))?;
        }
        Ok(path)
    }

    /// Read the day file. A missing file is an empty day, not an error.
    pub fn read_day(&self, date: NaiveDate) -> anyhow::Result<String> {
        let path = self.day_path(date);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::day_template(date)),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    /// Append a section to the end of the day file (created from the template).
    pub fn append_section(&self, date: NaiveDate, section: &str) -> anyhow::Result<()> {
        let path = self.ensure_day_file(date)?;
        let mut text = std::fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        if !text.ends_with('\n') {
            text.push('\n');
        }
        text.push('\n');
        text.push_str(section.trim_matches('\n'));
        text.push('\n');
        std::fs::write(&path, text)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
    }

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        (tmp, store)
    }

    #[test]
    fn day_path_is_iso() {
        let (_tmp, store) = store();
        assert!(store.day_path(date()).ends_with("events/2026-11-20.md"));
    }

    #[test]
    fn ensure_day_file_is_idempotent() {
        let (_tmp, store) = store();
        let path = store.ensure_day_file(date()).unwrap();
        std::fs::write(&path, "changed\n").unwrap();
        let again = store.ensure_day_file(date()).unwrap();
        assert_eq!(path, again);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed\n");
    }

    #[test]
    fn read_missing_day_returns_template() {
        let (_tmp, store) = store();
        assert_eq!(store.read_day(date()).unwrap(), "# 2026-11-20\n");
    }

    #[test]
    fn append_section_grows_file() {
        let (_tmp, store) = store();
        store.append_section(date(), "## A\nTime: 10:00\n").unwrap();
        store.append_section(date(), "## B\nTime: 11:00\n").unwrap();
        let text = std::fs::read_to_string(store.day_path(date())).unwrap();
        assert!(text.starts_with("# 2026-11-20\n"));
        assert!(text.contains("## A"));
        assert!(text.contains("## B"));
    }

    #[test]
    fn ensure_layout_migrates_legacy_calendar() {
        let tmp = tempfile::tempdir().unwrap();
        let legacy = tmp.path().join("calendar");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("2026-11-20.md"), "# 2026-11-20\n").unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        store.ensure_layout().unwrap();
        assert!(!legacy.exists());
        assert!(store.day_path(date()).exists());
    }
}
