//! Storage layout: `events/`, `notes/`, day files, repeatables.

use std::path::{Path, PathBuf};

use anyhow::Context;
use chrono::NaiveDate;

use crate::config::OldEventPolicy;
use crate::event::Event;

/// Storage root: `<root>/` holding `events/`, `notes/` and `index.md`.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
}

/// What a sweep of passed day files did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Day files deleted under the `delete` policy.
    pub deleted: usize,
    /// Day files moved to `events/backlog/` under the `backlog` policy.
    pub backlogged: usize,
}

impl Store {
    /// Wrap an existing storage root (no IO).
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The storage root itself.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `<root>/events`.
    #[must_use]
    pub fn events_dir(&self) -> PathBuf {
        self.root.join("events")
    }

    /// `<root>/notes`.
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

    /// `<root>/to-agent`: tasks delegated from mobile Capture to a PC agent.
    #[must_use]
    pub fn to_agent_dir(&self) -> PathBuf {
        self.root.join("to-agent")
    }

    /// Write a delegated task file into `<root>/to-agent/YYYY-MM-DD-<slug>.md`.
    /// Creates `to-agent/` lazily if missing. Appends `-2`, `-3`, ... on name collision.
    ///
    /// # Errors
    ///
    /// Fails when the directory or task file cannot be created or written.
    pub fn write_agent_task(
        &self,
        date: NaiveDate,
        title: &str,
        priority: &str,
        project: Option<&str>,
        body: &str,
    ) -> anyhow::Result<PathBuf> {
        let dir = self.to_agent_dir();
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("failed to create {}", dir.display()))?;
        let slug = slugify(title);
        let date_str = date.format("%Y-%m-%d");
        let mut path = dir.join(format!("{date_str}-{slug}.md"));
        let mut counter = 2usize;
        while path.exists() {
            path = dir.join(format!("{date_str}-{slug}-{counter}.md"));
            counter += 1;
        }
        let project_str = project
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or("none");
        let mut text = format!(
            "# {}\nDate: {date_str}\nPriority: {}\nProject: {project_str}\n",
            title.trim(),
            priority.trim()
        );
        let trimmed_body = body.trim();
        if !trimmed_body.is_empty() {
            text.push('\n');
            text.push_str(trimmed_body);
            text.push('\n');
        }
        std::fs::write(&path, text)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(path)
    }

    /// Strictly ISO `events/YYYY-MM-DD.md`.
    #[must_use]
    pub fn day_path(&self, date: NaiveDate) -> PathBuf {
        self.events_dir()
            .join(format!("{}.md", date.format("%Y-%m-%d")))
    }

    /// Idempotent. Creates `events/` and `notes/` when missing.
    ///
    /// # Errors
    ///
    /// Fails when a directory cannot be created (permissions, IO).
    pub fn ensure_layout(&self) -> anyhow::Result<()> {
        for dir in [self.events_dir(), self.notes_dir()] {
            std::fs::create_dir_all(&dir)
                .with_context(|| format!("failed to create {}", dir.display()))?;
        }
        Ok(())
    }

    /// Creates `.gitignore` unless the user has one.
    ///
    /// # Errors
    ///
    /// Fails when the file cannot be written (permissions, IO).
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
    ///
    /// # Errors
    ///
    /// Fails on IO errors while listing, deleting or moving day files.
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

    /// Fresh day file contents: just the date header.
    #[must_use]
    pub fn day_template(date: NaiveDate) -> String {
        format!("# {}\n", date.format("%Y-%m-%d"))
    }

    /// Creates the day file from the template if missing.
    ///
    /// # Errors
    ///
    /// Fails when the file or its parent cannot be created (permissions, IO).
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
    ///
    /// # Errors
    ///
    /// Fails on IO errors other than "file not found".
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
    ///
    /// # Errors
    ///
    /// Fails when `events/` cannot be listed (permissions, IO).
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
    ///
    /// # Errors
    ///
    /// Fails on IO errors other than "file not found".
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
    ///
    /// # Errors
    ///
    /// Fails when the day file cannot be created, read or written.
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

fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = false;
    for ch in title.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if (ch.is_whitespace() || ch == '-' || ch == '_') && !out.is_empty() && !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        String::from("task")
    } else {
        trimmed.to_owned()
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

    #[test]
    fn write_agent_task_creates_file_and_handles_collisions() {
        let (_tmp, store) = store();
        let first = store
            .write_agent_task(
                date(),
                "Fix bug!",
                "high",
                Some("chora/store.md"),
                "- [ ] check\n",
            )
            .unwrap();
        assert_eq!(
            first.file_name().unwrap().to_str().unwrap(),
            "2026-11-20-fix-bug.md"
        );
        let second = store
            .write_agent_task(
                date(),
                "Fix bug!",
                "high",
                Some("chora/store.md"),
                "- [ ] check\n",
            )
            .unwrap();
        assert_eq!(
            second.file_name().unwrap().to_str().unwrap(),
            "2026-11-20-fix-bug-2.md"
        );
        let text = std::fs::read_to_string(&first).unwrap();
        assert!(text.contains("Priority: high"), "{text}");
        assert!(text.contains("Project: chora/store.md"), "{text}");
    }
}
