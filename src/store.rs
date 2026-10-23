//! Storage layout: `events/`, `notes/`, day files, repeatables.

use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::NaiveDate;

use crate::config::OldEventPolicy;
use crate::event::Event;

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

    #[must_use]
    pub fn notes_dir(&self) -> PathBuf {
        self.root.join("notes")
    }

    /// Human-owned context + `security`/`about` per notes path.
    #[must_use]
    pub fn index_path(&self) -> PathBuf {
        self.root.join("index.md")
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

    /// Idempotent. Creates `events/` and `notes/` when missing.
    pub fn ensure_layout(&self) -> anyhow::Result<()> {
        for dir in [self.events_dir(), self.notes_dir()] {
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
                    if dest.exists() {
                        continue;
                    }
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

    /// All days with a file, oldest first. Missing `events/` is empty.
    /// `backlog/` and `repeatable/` are not day files and never listed.
    pub fn list_days(&self) -> anyhow::Result<Vec<NaiveDate>> {
        let events = self.events_dir();
        if !events.is_dir() {
            return Ok(Vec::new());
        }
        let mut days = Vec::new();
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
            if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
                days.push(date);
            }
        }
        days.sort();
        Ok(days)
    }

    /// Repeatable events for `date`: the `events/repeatable/<weekday>.md`
    /// file (e.g. `monday.md`), parsed with `date` as the event date.
    /// Missing file or dir is no repeatables, not an error.
    pub fn read_repeatables(&self, date: NaiveDate) -> anyhow::Result<Vec<Event>> {
        let weekday = date.format("%A").to_string().to_lowercase();
        let path = self
            .events_dir()
            .join("repeatable")
            .join(format!("{weekday}.md"));
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(crate::event::parse_day(date, &text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
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

    #[test]
    fn list_days_sorts_and_skips_non_days() {
        let (_tmp, store) = sweep_fixture();
        // sweep_fixture: 2026-11-19/20/21.md + stray seen.md.
        let days = store.list_days().unwrap();
        assert_eq!(
            days,
            vec![
                NaiveDate::from_ymd_opt(2026, 11, 19).unwrap(),
                NaiveDate::from_ymd_opt(2026, 11, 20).unwrap(),
                NaiveDate::from_ymd_opt(2026, 11, 21).unwrap(),
            ]
        );
    }

    #[test]
    fn list_days_ignores_subdirs() {
        let (_tmp, store) = sweep_fixture();
        std::fs::create_dir_all(store.events_dir().join("repeatable")).unwrap();
        std::fs::create_dir_all(store.events_dir().join("backlog")).unwrap();
        assert_eq!(store.list_days().unwrap().len(), 3);
    }

    #[test]
    fn list_days_without_events_dir_is_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        assert!(store.list_days().unwrap().is_empty());
    }

    #[test]
    fn repeatables_match_weekday() {
        let (_tmp, store) = sweep_fixture();
        // 2026-11-23 is a Monday.
        let monday = NaiveDate::from_ymd_opt(2026, 11, 23).unwrap();
        assert_eq!(monday.format("%A").to_string(), "Monday");
        assert!(store.read_repeatables(monday).unwrap().is_empty());

        let dir = store.events_dir().join("repeatable");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("monday.md"),
            "# Monday\n\n## Teeth\nTime: 09:00\nDuration: 10m\n\nBrush.\n",
        )
        .unwrap();
        let events = store.read_repeatables(monday).unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Teeth");
        // Parsed with the queried date, not the file's.
        assert_eq!(events[0].date, monday);

        // Other weekdays are untouched by a Monday query.
        let tuesday = NaiveDate::from_ymd_opt(2026, 11, 24).unwrap();
        assert!(store.read_repeatables(tuesday).unwrap().is_empty());
    }
}
