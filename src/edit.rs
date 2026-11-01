//! Checked edits of user files. Every write is atomic, and every edit of
//! existing content proves the content is still what the caller read:
//! notes by their full text, events by value. A sync between read and
//! write fails with [`ChoraError::Changed`] instead of clobbering it.

use std::path::{Path, PathBuf};

use chrono::{Datelike, NaiveDate, Weekday};

use crate::error::ChoraError;
use crate::event::{self, Event};
use crate::fsx;
use crate::note;
use crate::store::Store;

/// Append `block` to the note at `path` (created when missing), or at the
/// end of the section `header` (`Title` or `Parent/Child`) of an existing note.
///
/// # Errors
///
/// Fails when `header` is given for a missing file or matches nothing, or on IO.
pub fn append_note(
    path: &Path,
    rel: &str,
    header: Option<&str>,
    block: &str,
) -> anyhow::Result<()> {
    let updated = match (fsx::read_opt(path)?, header) {
        (Some(text), Some(header)) => note::insert_under(&text, header, block.trim_matches('\n'))?,
        (Some(text), None) => fsx::append_block(&text, block),
        (None, Some(_)) => anyhow::bail!("no such file `{rel}`: a section needs an existing file"),
        (None, None) => fsx::append_block("", block),
    };
    fsx::write_atomic(path, &updated)
}

/// Overwrite the note at `path` with `text`, only while it still holds `base`
/// (a missing file counts as empty).
///
/// # Errors
///
/// [`ChoraError::Changed`] when the file moved on since `base`; IO errors.
pub fn save_note(path: &Path, rel: &str, text: &str, base: &str) -> anyhow::Result<()> {
    if fsx::read_opt(path)?.unwrap_or_default() != base {
        return Err(ChoraError::Changed(rel.to_owned()).into());
    }
    fsx::write_atomic(path, text)
}

/// Flip the task on raw line `line` (0-based, `\n`-split) of `base` and save.
/// Returns the new text.
///
/// # Errors
///
/// [`ChoraError::NotATask`], [`ChoraError::Changed`], IO errors.
pub fn toggle_task(path: &Path, rel: &str, line: usize, base: &str) -> anyhow::Result<String> {
    let updated = toggled(base, line)?;
    save_note(path, rel, &updated, base)?;
    Ok(updated)
}

/// `- [ ]` <-> `- [x]` (also `*`/`+` bullets, `[X]`) at line start after
/// indent. Lines inside fenced code blocks are never tasks. Line endings kept.
///
/// # Errors
///
/// [`ChoraError::NotATask`] when `line` is not a task line.
pub fn toggled(text: &str, line: usize) -> Result<String, ChoraError> {
    let mut in_fence = false;
    let mut out = String::with_capacity(text.len());
    let mut done = false;
    for (i, raw) in text.split_inclusive('\n').enumerate() {
        let trimmed = raw.trim_start();
        let fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if fence {
            in_fence = !in_fence;
        }
        match (i == line && !in_fence && !fence)
            .then(|| task_mark(trimmed))
            .flatten()
        {
            Some(mark) => {
                let at = raw.len() - trimmed.len() + 3;
                out.push_str(&raw[..at]);
                out.push(mark);
                out.push_str(&raw[at + 1..]);
                done = true;
            }
            None => out.push_str(raw),
        }
    }
    if done {
        Ok(out)
    } else {
        Err(ChoraError::NotATask(line + 1))
    }
}

fn task_mark(line: &str) -> Option<char> {
    let bytes = line.as_bytes();
    let shaped = bytes.len() >= 5
        && matches!(bytes[0], b'-' | b'*' | b'+')
        && &bytes[1..3] == b" ["
        && bytes[4] == b']';
    match bytes.get(3) {
        Some(b' ') if shaped => Some('x'),
        Some(b'x' | b'X') if shaped => Some(' '),
        _ => None,
    }
}

/// The file an event lives in: a day file or a weekly (repeatable) file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventFile {
    /// `events/YYYY-MM-DD.md`.
    Day(NaiveDate),
    /// `events/repeatable/<weekday>.md`.
    Weekly(Weekday),
}

impl EventFile {
    /// Path of this file under `store`.
    #[must_use]
    pub fn path(self, store: &Store) -> PathBuf {
        match self {
            Self::Day(date) => store.day_path(date),
            Self::Weekly(day) => store
                .events_dir()
                .join("repeatable")
                .join(format!("{}.md", weekday_name(day))),
        }
    }

    /// Text of the file, or its fresh template when missing.
    ///
    /// # Errors
    ///
    /// Fails on IO errors other than "not found".
    pub fn read(self, store: &Store) -> anyhow::Result<String> {
        Ok(fsx::read_opt(&self.path(store))?.unwrap_or_else(|| self.template()))
    }

    fn template(self) -> String {
        match self {
            Self::Day(date) => Store::day_template(date),
            Self::Weekly(day) => {
                let name = weekday_name(day);
                format!("# {}{}\n", name[..1].to_uppercase(), &name[1..])
            }
        }
    }
}

/// Lowercase English weekday name, as used for weekly file names.
#[must_use]
pub fn weekday_name(day: Weekday) -> &'static str {
    match day {
        Weekday::Mon => "monday",
        Weekday::Tue => "tuesday",
        Weekday::Wed => "wednesday",
        Weekday::Thu => "thursday",
        Weekday::Fri => "friday",
        Weekday::Sat => "saturday",
        Weekday::Sun => "sunday",
    }
}

/// Events of `file` dated `date`, in [`event::parse_day`] order (their
/// positions are the `hint` for [`update_event`]/[`delete_event`]).
///
/// # Errors
///
/// Fails on IO errors other than "not found".
pub fn read_events(store: &Store, file: EventFile, date: NaiveDate) -> anyhow::Result<Vec<Event>> {
    Ok(event::parse_day(date, &file.read(store)?))
}

/// The file `event` belongs to for the given kind.
#[must_use]
pub fn file_of(event: &Event, weekly: bool) -> EventFile {
    if weekly {
        EventFile::Weekly(event.date.weekday())
    } else {
        EventFile::Day(event.date)
    }
}

/// Append `event` to `file`.
///
/// # Errors
///
/// Fails on IO errors.
pub fn add_event(store: &Store, file: EventFile, event: &Event) -> anyhow::Result<()> {
    let text = file.read(store)?;
    fsx::write_atomic(
        &file.path(store),
        &fsx::append_block(&text, &event::render_section(event)),
    )
}

/// Replace `expected` in `from` with `updated` stored in `to`. A move writes
/// the new copy first, so a failure duplicates and never loses the event.
///
/// # Errors
///
/// [`ChoraError::Changed`] when `expected` is no longer in `from`; IO errors.
pub fn update_event(
    store: &Store,
    from: EventFile,
    expected: &Event,
    hint: usize,
    to: EventFile,
    updated: &Event,
) -> anyhow::Result<()> {
    let text = from.read(store)?;
    let at = locate(&text, expected, hint)?;
    if from == to {
        return fsx::write_atomic(
            &from.path(store),
            &event::replace_section(&text, at, updated)?,
        );
    }
    add_event(store, to, updated)?;
    fsx::write_atomic(&from.path(store), &event::remove_section(&text, at)?)
}

/// Remove `expected` from `file`.
///
/// # Errors
///
/// [`ChoraError::Changed`] when `expected` is gone; IO errors.
pub fn delete_event(
    store: &Store,
    file: EventFile,
    expected: &Event,
    hint: usize,
) -> anyhow::Result<()> {
    let text = file.read(store)?;
    let at = locate(&text, expected, hint)?;
    fsx::write_atomic(&file.path(store), &event::remove_section(&text, at)?)
}

/// Position of `expected` among the events of `text`: `hint` when it still
/// matches, else the first equal event.
fn locate(text: &str, expected: &Event, hint: usize) -> Result<usize, ChoraError> {
    let events = event::parse_day(expected.date, text);
    if events.get(hint) == Some(expected) {
        return Ok(hint);
    }
    events
        .iter()
        .position(|ev| ev == expected)
        .ok_or_else(|| ChoraError::Changed(expected.title.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use chrono::NaiveTime;

    fn store() -> (tempfile::TempDir, Store) {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::new(tmp.path().to_path_buf());
        (tmp, store)
    }

    fn date(day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, day).unwrap()
    }

    fn ev(title: &str, day: u32, hour: u32) -> Event {
        Event {
            title: title.into(),
            date: date(day),
            time: NaiveTime::from_hms_opt(hour, 0, 0).unwrap(),
            duration: Duration::from_secs(1800),
            context: String::new(),
        }
    }

    fn titles(store: &Store, file: EventFile, day: u32) -> Vec<String> {
        read_events(store, file, date(day))
            .unwrap()
            .into_iter()
            .map(|e| e.title)
            .collect()
    }

    fn is_changed(err: &anyhow::Error) -> bool {
        matches!(
            err.downcast_ref::<ChoraError>(),
            Some(ChoraError::Changed(_))
        )
    }

    #[test]
    fn toggles_tasks_only_outside_fences() {
        let text = "# T\r\n- [ ] a\r\n  * [x] b\r\n```\r\n- [ ] code\r\n```\r\nplain\r\n";
        assert_eq!(
            toggled(text, 1).unwrap(),
            text.replacen("- [ ] a", "- [x] a", 1)
        );
        assert_eq!(
            toggled(text, 2).unwrap(),
            text.replacen("* [x] b", "* [ ] b", 1)
        );
        for line in [3, 4, 6, 99] {
            assert!(toggled(text, line).is_err(), "line {line}");
        }
    }

    #[test]
    fn checked_note_writes_refuse_stale_base() {
        let (tmp, _) = store();
        let path = tmp.path().join("notes/a.md");
        save_note(&path, "a.md", "one\n", "").unwrap();
        assert!(is_changed(
            &save_note(&path, "a.md", "two\n", "stale\n").unwrap_err()
        ));
        assert_eq!(
            toggle_task(&path, "a.md", 0, "one\n")
                .unwrap_err()
                .to_string(),
            "line 1 is not a task"
        );
        save_note(&path, "a.md", "- [ ] x\n", "one\n").unwrap();
        assert_eq!(
            toggle_task(&path, "a.md", 0, "- [ ] x\n").unwrap(),
            "- [x] x\n"
        );
        assert!(is_changed(
            &toggle_task(&path, "a.md", 0, "- [ ] x\n").unwrap_err()
        ));
    }

    #[test]
    fn append_note_creates_then_inserts_under_header() {
        let (tmp, _) = store();
        let path = tmp.path().join("todo.md");
        assert!(append_note(&path, "todo.md", Some("Tasks"), "- [ ] a").is_err());
        append_note(&path, "todo.md", None, "# Todo\n\n## Tasks\n\n## Bugs").unwrap();
        append_note(&path, "todo.md", Some("Tasks"), "- [ ] a").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.find("- [ ] a").unwrap() < text.find("## Bugs").unwrap(),
            "{text}"
        );
    }

    #[test]
    fn move_second_event_keeps_target_day_intact() {
        let (_tmp, store) = store();
        let (a, b) = (EventFile::Day(date(2)), EventFile::Day(date(3)));
        add_event(&store, a, &ev("A1", 2, 10)).unwrap();
        add_event(&store, a, &ev("A2", 2, 11)).unwrap();
        add_event(&store, b, &ev("B1", 3, 10)).unwrap();
        update_event(&store, a, &ev("A2", 2, 11), 1, b, &ev("A2", 3, 11)).unwrap();
        assert_eq!(titles(&store, a, 2), ["A1"]);
        assert_eq!(titles(&store, b, 3), ["B1", "A2"]);
    }

    #[test]
    fn edits_refuse_changed_events_and_follow_shifted_ones() {
        let (_tmp, store) = store();
        let day = EventFile::Day(date(2));
        add_event(&store, day, &ev("Late", 2, 15)).unwrap();
        add_event(&store, day, &ev("Early", 2, 9)).unwrap();
        // "Late" was read at index 0; it is now index 1 and still found.
        delete_event(&store, day, &ev("Late", 2, 15), 0).unwrap();
        assert_eq!(titles(&store, day, 2), ["Early"]);
        let err = delete_event(&store, day, &ev("Late", 2, 15), 0).unwrap_err();
        assert!(is_changed(&err));
    }

    #[test]
    fn weekly_files_keep_plain_text_and_template() {
        let (_tmp, store) = store();
        let monday = EventFile::Weekly(Weekday::Mon);
        add_event(&store, monday, &ev("Gym", 2, 19)).unwrap();
        let path = monday.path(&store);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# Monday\n"), "{text}");
        fsx::write_atomic(&path, &format!("{text}\n## Notes\n\nbring towel\n")).unwrap();
        delete_event(&store, monday, &ev("Gym", 2, 19), 0).unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("bring towel")
        );
        assert_eq!(file_of(&ev("x", 9, 9), true), monday);
    }
}
