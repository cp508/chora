//! Storage layout: `events/`, `notes/`, day files.

use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::NaiveDate;

use crate::config::OldEventPolicy;

pub struct Store {
    root: PathBuf,
}

/// What a sweep of passed day files did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    pub deleted: usize,
    pub backlogged: usize,
}

impl Store {
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    #[must_use]
    pub fn events_dir(&self) -> PathBuf {
        self.root.join("events")
    }

    /// Old location, moved to `events/` once.
    fn calendar_legacy_dir(&self) -> PathBuf {
        self.root.join("calendar")
    }

    #[must_use]
    pub fn notes_dir(&self) -> PathBuf {
        self.root.join("notes")
    }

    /// Archive for passed day files under the `backlog` policy.
    #[must_use]
    pub fn backlog_dir(&self) -> PathBuf {
        self.events_dir().join("backlog")
    }

    /// Strictly ISO `events/YYYY-MM-DD.md`.
    #[must_use]
    pub fn day_path(&self, date: NaiveDate) -> PathBuf {
        self.events_dir()
            .join(format!("{}.md", date.format("%Y-%m-%d")))
    }

    /// Idempotent. Moves a pre-rename `calendar/` to `events/` once.
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

    /// Creates `.gitignore` unless the user has one.
    pub fn ensure_gitignore(&self) -> anyhow::Result<()> {
        let path = self.root.join(".gitignore");
        if !path.exists() {
            std::fs::write(&path, "*.swp\n*~\n.DS_Store\n")
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        Ok(())
    }

    /// Delete or archive day files older than `today` per `policy`.
    /// Only top-level `events/YYYY-MM-DD.md` files; anything else
    /// (notes, `backlog/`, stray files) is left alone. `Nothing` never
    /// even scans.
    pub fn sweep_old_events(
        &self,
        policy: OldEventPolicy,
        today: NaiveDate,
    ) -> anyhow::Result<SweepReport> {
        let mut report = SweepReport::default();
        if policy == OldEventPolicy::Nothing {
            return Ok(report);
        }
        let events = self.events_dir();
        if !events.is_dir() {
            return Ok(report);
        }
        for entry in std::fs::read_dir(&events)
            .with_context(|| format!("failed to list {}", events.display()))?
        {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(raw) = name.strip_suffix(".md") else {
                continue;
            };
            let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") else {
                continue;
            };
            if date >= today {
                continue;
            }
            let path = entry.path();
            match policy {
                OldEventPolicy::Nothing => {}
                OldEventPolicy::Delete => {
                    std::fs::remove_file(&path)
                        .with_context(|| format!("failed to delete {}", path.display()))?;
                    report.deleted += 1;
                }
                OldEventPolicy::Backlog => {
                    let dest = self.backlog_dir().join(&name);
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)
                            .with_context(|| format!("failed to create {}", parent.display()))?;
                    }
                    std::fs::rename(&path, &dest).with_context(|| {
                        format!("failed to move {} to {}", path.display(), dest.display())
                    })?;
                    report.backlogged += 1;
                }
            }
        }
        Ok(report)
    }

    #[must_use]
    pub fn day_template(date: NaiveDate) -> String {
        format!("# {}\n", date.format("%Y-%m-%d"))
    }

    /// Creates the day file from the template if missing.
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

    /// A missing file is an empty day, not an error.
    pub fn read_day(&self, date: NaiveDate) -> anyhow::Result<String> {
        let path = self.day_path(date);
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::day_template(date)),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    /// Appends a section; creates the day file first.
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

    fn sweep_fixture() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        store.ensure_layout().unwrap();
        std::fs::write(store.events_dir().join("2026-11-19.md"), "# old\n").unwrap();
        std::fs::write(store.events_dir().join("2026-11-20.md"), "# today\n").unwrap();
        std::fs::write(store.events_dir().join("2026-11-21.md"), "# future\n").unwrap();
        std::fs::write(store.events_dir().join("seen.md"), "# stray\n").unwrap();
        (tmp, store)
    }

    #[test]
    fn sweep_delete_removes_only_past_days() {
        use crate::config::OldEventPolicy;
        let (_tmp, store) = sweep_fixture();
        let today = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let report = store
            .sweep_old_events(OldEventPolicy::Delete, today)
            .unwrap();
        assert_eq!(
            report,
            SweepReport {
                deleted: 1,
                backlogged: 0
            }
        );
        assert!(!store.events_dir().join("2026-11-19.md").exists());
        assert!(store.events_dir().join("2026-11-20.md").exists());
        assert!(store.events_dir().join("2026-11-21.md").exists());
        assert!(store.events_dir().join("seen.md").exists());
    }

    #[test]
    fn sweep_backlog_moves_past_days() {
        use crate::config::OldEventPolicy;
        let (_tmp, store) = sweep_fixture();
        let today = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let report = store
            .sweep_old_events(OldEventPolicy::Backlog, today)
            .unwrap();
        assert_eq!(
            report,
            SweepReport {
                deleted: 0,
                backlogged: 1
            }
        );
        assert!(!store.events_dir().join("2026-11-19.md").exists());
        assert!(store.backlog_dir().join("2026-11-19.md").exists());
        assert!(store.events_dir().join("2026-11-20.md").exists());
    }

    #[test]
    fn sweep_backlog_ignores_previous_backlog() {
        use crate::config::OldEventPolicy;
        let (_tmp, store) = sweep_fixture();
        let today = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        store
            .sweep_old_events(OldEventPolicy::Backlog, today)
            .unwrap();
        let again = store
            .sweep_old_events(OldEventPolicy::Backlog, today)
            .unwrap();
        assert_eq!(again, SweepReport::default());
    }

    #[test]
    fn sweep_nothing_leaves_everything() {
        use crate::config::OldEventPolicy;
        let (_tmp, store) = sweep_fixture();
        let today = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let report = store
            .sweep_old_events(OldEventPolicy::Nothing, today)
            .unwrap();
        assert_eq!(report, SweepReport::default());
        assert!(store.events_dir().join("2026-11-19.md").exists());
    }

    #[test]
    fn sweep_without_events_dir_is_noop() {
        use crate::config::OldEventPolicy;
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        let today = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let report = store
            .sweep_old_events(OldEventPolicy::Delete, today)
            .unwrap();
        assert_eq!(report, SweepReport::default());
    }
}
