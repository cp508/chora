//! Day events: `##` sections <-> [`Event`]. Lenient: sections without
//! valid `Time:`/`Duration:` are day text, skipped without error.

use std::fmt::Write as _;
use std::time::Duration;

use chrono::{NaiveDate, NaiveTime};

use crate::error::ChoraError;

/// One parsed day event: a `##` section with valid `Time:`/`Duration:`.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// The `##` header (= TUI `name`).
    pub title: String,
    /// The day this event belongs to (file date, or queried date for repeatables).
    pub date: NaiveDate,
    /// Start time from `Time: HH:MM`.
    pub time: NaiveTime,
    /// Seconds, normalized by the parser.
    pub duration: Duration,
    /// Free-form body text below the `Time:`/`Duration:` lines.
    pub context: String,
}

/// `30m`, `2h`, `1h30m`, `2h 20m` (spaces ignored), `2:00` (=2h),
/// `1:30` (=1h30m), `1:02:03` (=1h2m3s). Groups run back to back, at least one.
///
/// # Errors
///
/// Returns [`ChoraError::BadDuration`] for empty input, unknown units,
/// missing numbers, out-of-range minutes/seconds, or arithmetic overflow.
pub fn parse_duration(raw: &str) -> Result<Duration, ChoraError> {
    let bad = || ChoraError::BadDuration(raw.to_owned());
    let text = raw.trim();
    if text.is_empty() {
        return Err(bad());
    }
    if text.contains(':') {
        let parts: Vec<&str> = text.split(':').map(str::trim).collect();
        if parts.len() != 2 && parts.len() != 3 {
            return Err(bad());
        }
        if parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_digit()))
        {
            return Err(bad());
        }
        let h: u64 = parts[0].parse().map_err(|_| bad())?;
        let m: u64 = parts[1].parse().map_err(|_| bad())?;
        let s: u64 = if parts.len() == 3 {
            parts[2].parse().map_err(|_| bad())?
        } else {
            0
        };
        if m >= 60 || s >= 60 {
            return Err(bad());
        }
        let total = h
            .checked_mul(3600)
            .and_then(|v| v.checked_add(m.checked_mul(60)?))
            .and_then(|v| v.checked_add(s))
            .ok_or_else(bad)?;
        return Ok(Duration::from_secs(total));
    }
    let compact: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    let mut total_secs: u64 = 0;
    let mut groups = 0;
    let mut num: u64 = 0;
    let mut have_num = false;
    for ch in compact.chars() {
        if ch.is_ascii_digit() {
            let digit = u64::from(ch as u8 - b'0');
            num = num
                .checked_mul(10)
                .and_then(|v| v.checked_add(digit))
                .ok_or_else(bad)?;
            have_num = true;
        } else {
            let mult: u64 = match ch {
                's' => 1,
                'm' => 60,
                'h' => 3600,
                _ => return Err(bad()),
            };
            if !have_num {
                return Err(bad());
            }
            let add = num.checked_mul(mult).ok_or_else(bad)?;
            total_secs = total_secs.checked_add(add).ok_or_else(bad)?;
            num = 0;
            have_num = false;
            groups += 1;
        }
    }
    if have_num || groups == 0 {
        return Err(bad());
    }
    Ok(Duration::from_secs(total_secs))
}

/// `HH:MM` (`9:00`, `09:30`) or `H` (`9` = 09:00). Strict, no shorthands.
///
/// # Errors
///
/// Returns [`ChoraError::BadTime`] for anything outside `HH:MM` or a bare hour.
pub fn parse_time_arg(raw: &str) -> Result<NaiveTime, ChoraError> {
    let bad = || ChoraError::BadTime(raw.to_owned());
    let text = raw.trim();
    if let Ok(t) = NaiveTime::parse_from_str(text, "%H:%M") {
        return Ok(t);
    }
    if !text.is_empty()
        && text.chars().all(|c| c.is_ascii_digit())
        && let Ok(h) = text.parse::<u32>()
        && let Some(t) = NaiveTime::from_hms_opt(h, 0, 0)
    {
        return Ok(t);
    }
    Err(bad())
}

/// Compact duration text: `2h`, `1h30m`, `45m`, `10s` (`0s` for zero).
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (hours, mins, secs) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let mut out = String::new();
    if hours > 0 {
        let _ = write!(out, "{hours}h");
    }
    if mins > 0 {
        let _ = write!(out, "{mins}m");
    }
    if secs > 0 || out.is_empty() {
        let _ = write!(out, "{secs}s");
    }
    out
}

/// Neighbouring `00`/`15`/`30`/`45` mark; full step when already on one.
#[must_use]
pub fn snap_quarter(time: NaiveTime, forward: bool) -> NaiveTime {
    use chrono::Timelike;
    const STEP: i64 = 15;
    const DAY: i64 = 24 * 60;
    let day_mins = (time.num_seconds_from_midnight() / 60) as i64;
    let next = if forward {
        (day_mins / STEP + 1) * STEP
    } else {
        (day_mins - 1).div_euclid(STEP) * STEP
    };
    let wrapped = next.rem_euclid(DAY);
    NaiveTime::from_hms_opt((wrapped / 60) as u32, (wrapped % 60) as u32, 0)
        .expect("BUG: rem_euclid(DAY) always yields a valid 00:00-23:59 time")
}

/// All valid events of a day file, sorted by time. Sections without
/// valid `Time:`/`Duration:` are day text and skipped, never an error.
#[must_use]
pub fn parse_day(date: NaiveDate, text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in text.lines() {
        if let Some(title) = section_title(line) {
            if let Some((prev_title, prev_body)) = current.take() {
                push_section(&mut events, date, &prev_title, &prev_body);
            }
            current = Some((title.to_owned(), Vec::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((title, body)) = current.take() {
        push_section(&mut events, date, &title, &body);
    }
    events.sort_by_key(|event| event.time);
    events
}

/// Parse one `##` section body into an [`Event`]. `None` for plain day
/// text (empty title or missing/invalid `Time:`/`Duration:`), never an error.
fn parse_section(date: NaiveDate, title: &str, body: &[&str]) -> Option<Event> {
    if title.is_empty() {
        return None;
    }
    let mut time: Option<NaiveTime> = None;
    let mut duration: Option<Duration> = None;
    let mut context: Vec<&str> = Vec::new();
    for line in body {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Time:") {
            if let Ok(parsed) = NaiveTime::parse_from_str(value.trim(), "%H:%M") {
                time = Some(parsed);
            }
        } else if let Some(value) = trimmed.strip_prefix("Duration:") {
            if let Ok(parsed) = parse_duration(value.trim()) {
                duration = Some(parsed);
            }
        } else {
            context.push(line);
        }
    }
    let (time, duration) = (time?, duration?);
    Some(Event {
        title: title.to_owned(),
        date,
        time,
        duration,
        // One allocation: trim the edges in place instead of
        // `join().trim().to_owned()`, which copies the whole body twice.
        context: trim_edges(context.join("\n")),
    })
}

fn push_section(events: &mut Vec<Event>, date: NaiveDate, title: &str, body: &[&str]) {
    if let Some(event) = parse_section(date, title, body) {
        events.push(event);
    }
}

/// Trim leading/trailing whitespace in place (same result as
/// `str::trim`, without copying the body).
fn trim_edges(mut text: String) -> String {
    let start = text.len() - text.trim_start().len();
    text.drain(..start);
    let end = text.trim_end().len();
    text.truncate(end);
    text
}

/// Render one event back to day-file section format.
#[must_use]
pub fn render_section(event: &Event) -> String {
    let mut out = String::from("## ");
    out.push_str(&event.title);
    let _ = writeln!(out, "\nTime: {}", event.time.format("%H:%M"));
    let _ = writeln!(out, "Duration: {}", format_duration(event.duration));
    if event.context.is_empty() {
        out.push('\n');
    } else {
        out.push('\n');
        out.push_str(event.context.trim());
        out.push('\n');
    }
    out
}

/// Replace the `index`-th valid event section (0-based, in file order before
/// time sorting, or matched by [`parse_day`] order) in `day_text` with `updated`.
/// Preserves the `# YYYY-MM-DD` header, plain-text `##` sections, and other events.
///
/// # Errors
///
/// Fails when `index` is out of bounds for the valid events in `day_text`.
pub fn replace_section(day_text: &str, index: usize, updated: &Event) -> anyhow::Result<String> {
    rewrite_event_section(day_text, index, Some(updated))
}

/// Remove the `index`-th valid event section (0-based, in [`parse_day`] order)
/// from `day_text`, keeping all other sections and plain day notes intact.
///
/// # Errors
///
/// Fails when `index` is out of bounds for the valid events in `day_text`.
pub fn remove_section(day_text: &str, index: usize) -> anyhow::Result<String> {
    rewrite_event_section(day_text, index, None)
}

fn section_title(line: &str) -> Option<&str> {
    line.strip_prefix("##")
        .filter(|rest| !rest.starts_with('#'))
        .map(str::trim)
}

fn is_section_header(line: &str) -> bool {
    section_title(line).is_some()
}

fn rewrite_event_section(
    day_text: &str,
    index: usize,
    replacement: Option<&Event>,
) -> anyhow::Result<String> {
    let lines: Vec<&str> = day_text.lines().collect();
    let first = lines.iter().position(|line| is_section_header(line));
    let (preamble, rest) = match first {
        Some(i) => (&lines[..i], &lines[i..]),
        None => (&lines[..], &[][..]),
    };
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in rest {
        if is_section_header(line) {
            blocks.push(vec![*line]);
        } else if let Some(last) = blocks.last_mut() {
            last.push(*line);
        }
    }
    let mut valid: Vec<(NaiveTime, usize)> = Vec::new();
    // Only the start time orders the UI index; the date is irrelevant here.
    let dummy = NaiveDate::from_ymd_opt(2000, 1, 1).expect("fixed dummy date is valid");
    for (idx, block) in blocks.iter().enumerate() {
        let title = section_title(block[0]).unwrap_or_default();
        if let Some(event) = parse_section(dummy, title, &block[1..]) {
            valid.push((event.time, idx));
        }
    }
    valid.sort_by_key(|(time, _)| *time);
    let target = valid
        .get(index)
        .map(|(_, idx)| *idx)
        .ok_or_else(|| anyhow::anyhow!("event index {index} out of bounds"))?;
    let preamble_text = preamble.join("\n").trim().to_owned();
    let mut rendered: Vec<String> = Vec::with_capacity(blocks.len());
    for (idx, block) in blocks.iter().enumerate() {
        if idx == target {
            if let Some(ev) = replacement {
                rendered.push(render_section(ev).trim().to_owned());
            }
        } else {
            let mut end = block.len();
            while end > 0 && block[end - 1].trim().is_empty() {
                end -= 1;
            }
            rendered.push(block[..end].join("\n"));
        }
    }
    let mut out = if preamble_text.is_empty() {
        rendered.join("\n\n")
    } else if rendered.is_empty() {
        preamble_text
    } else {
        format!("{preamble_text}\n\n{}", rendered.join("\n\n"))
    };
    out.push('\n');
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
    }

    #[test]
    fn parse_duration_table() {
        let cases = [
            ("10s", 10),
            ("45m", 2700),
            ("2h", 7200),
            ("1h30m", 5400),
            ("1h30m10s", 5410),
            ("0s", 0),
            ("2h 20m", 8400),
            ("1h 30m 10s", 5410),
            ("2:00", 7200),
            ("1:30", 5400),
            ("0:45", 2700),
            ("1:02:03", 3723),
        ];
        for (raw, secs) in cases {
            assert_eq!(
                parse_duration(raw).unwrap(),
                Duration::from_secs(secs),
                "{raw}"
            );
        }
    }

    #[test]
    fn parse_duration_rejects_garbage() {
        for raw in [
            "",
            "  ",
            "abc",
            "1d",
            "h",
            "10",
            "1h30",
            "1.5h",
            "-5m",
            "99999999999999999999h",
            "1:60",
            "1:2:3:4",
            "1:",
            ":30",
            "1h:30",
        ] {
            assert!(parse_duration(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn parse_time_arg_table() {
        let at = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        for (raw, expected) in [
            ("09:30", at(9, 30)),
            ("9:00", at(9, 0)),
            (" 18:30 ", at(18, 30)),
            ("9", at(9, 0)),
            ("23", at(23, 0)),
        ] {
            assert_eq!(parse_time_arg(raw).unwrap(), expected, "{raw}");
        }
        for raw in ["", "25:00", "12:60", "abc", "9:00:00", "930", "-1"] {
            assert!(parse_time_arg(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn format_duration_table() {
        let cases = [
            (10, "10s"),
            (2700, "45m"),
            (7200, "2h"),
            (5400, "1h30m"),
            (5410, "1h30m10s"),
            (0, "0s"),
        ];
        for (secs, text) in cases {
            assert_eq!(format_duration(Duration::from_secs(secs)), text);
        }
    }

    #[test]
    fn parse_day_reads_two_events() {
        let text = "# 2026-11-20\n\n## Standup\nTime: 11:00\nDuration: 2h\n\nAgenda here.\n\n## Dentist\nTime: 18:30\nDuration: 45m\n\nBring the card.\n";
        let events = parse_day(date(), text);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Standup");
        assert_eq!(events[0].time, NaiveTime::from_hms_opt(11, 0, 0).unwrap());
        assert_eq!(events[0].duration, Duration::from_secs(7200));
        assert_eq!(events[0].context, "Agenda here.");
        assert_eq!(events[1].title, "Dentist");
        assert_eq!(events[1].context, "Bring the card.");
    }

    #[test]
    fn parse_day_skips_plain_text_sections() {
        let text = "# 2026-11-20\n\n## Just a note\n\nNo time — not an event.\n\n## Event\nTime: 09:00\nDuration: 10m\n\nOk.\n";
        let events = parse_day(date(), text);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Event");
    }

    #[test]
    fn parse_day_skips_broken_fields() {
        let text = "## A\nTime: 25:99\nDuration: 1h\n\n## B\nTime: 10:00\nDuration: ages\n\n## C\nTime: 10:00\nDuration: 1h\n\n";
        let events = parse_day(date(), text);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "C");
    }

    #[test]
    fn parse_day_handles_multibyte_text() {
        let text = "## Café ☕\nTime: 08:00\nDuration: 15m\n\nCroissant.\n";
        let events = parse_day(date(), text);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].title, "Café ☕");
    }

    #[test]
    fn render_parse_roundtrip() {
        let event = Event {
            title: String::from("Call"),
            date: date(),
            time: NaiveTime::from_hms_opt(9, 5, 0).unwrap(),
            duration: Duration::from_secs(5400),
            context: String::from("Agenda\nand notes."),
        };
        let parsed = parse_day(date(), &render_section(&event));
        assert_eq!(parsed, vec![event]);
    }

    #[test]
    fn parse_day_keeps_subheadings_and_sorts_by_time() {
        let text = "## Late\nTime: 18:00\nDuration: 1h\n\nIntro\n### Details\nMore.\n\n## Early\nTime: 08:30\nDuration: 30m\n";
        let events = parse_day(date(), text);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Early");
        assert_eq!(events[1].title, "Late");
        assert_eq!(events[1].context, "Intro\n### Details\nMore.");
    }

    #[test]
    fn snap_quarter_table() {
        let at = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let cases = [
            // Between marks: snap to the nearest ahead/behind.
            ((14, 37, true), (14, 45)),
            ((14, 37, false), (14, 30)),
            ((14, 1, true), (14, 15)),
            ((14, 59, false), (14, 45)),
            // On a mark: a full step.
            ((14, 30, true), (14, 45)),
            ((14, 30, false), (14, 15)),
            ((14, 0, true), (14, 15)),
            // Midnight wrap.
            ((23, 45, true), (0, 0)),
            ((0, 0, false), (23, 45)),
            ((0, 7, false), (0, 0)),
        ];
        for ((h, m, forward), (eh, em)) in cases {
            assert_eq!(
                snap_quarter(at(h, m), forward),
                at(eh, em),
                "{h}:{m} {forward}"
            );
        }
    }

    #[test]
    fn replace_section_updates_target_in_sorted_order_and_keeps_notes() {
        let text = "# 2026-11-20\n\n## Note\nhello\n\n## Late\nTime: 18:00\nDuration: 1h\n\nLate body.\n\n## Early\nTime: 09:00\nDuration: 30m\n\nEarly body.\n";
        let events = parse_day(date(), text);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].title, "Early");
        assert_eq!(events[1].title, "Late");
        let updated = Event {
            title: String::from("Early updated"),
            date: date(),
            time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            duration: Duration::from_secs(1800),
            context: String::from("New body."),
        };
        let out = replace_section(text, 0, &updated).unwrap();
        assert!(out.contains("# 2026-11-20"), "{out}");
        assert!(out.contains("## Note"), "{out}");
        assert!(out.contains("hello"), "{out}");
        assert!(out.contains("## Late"), "{out}");
        assert!(out.contains("Time: 18:00"), "{out}");
        assert!(out.contains("## Early updated"), "{out}");
        assert!(!out.contains("## Early\n"), "{out}");
        let reparsed = parse_day(date(), &out);
        assert_eq!(reparsed.len(), 2);
        assert_eq!(reparsed[0].title, "Early updated");
        assert_eq!(reparsed[0].context, "New body.");
        assert_eq!(reparsed[1].title, "Late");
    }

    #[test]
    fn remove_section_deletes_only_target_event() {
        let text = "# 2026-11-20\n\n## Note\nhello\n\n## Late\nTime: 18:00\nDuration: 1h\n\n## Early\nTime: 09:00\nDuration: 30m\n";
        let out = remove_section(text, 1).unwrap();
        assert!(out.contains("## Note"), "{out}");
        assert!(out.contains("## Early"), "{out}");
        assert!(!out.contains("## Late"), "{out}");
        let reparsed = parse_day(date(), &out);
        assert_eq!(reparsed.len(), 1);
        assert_eq!(reparsed[0].title, "Early");
        assert!(remove_section(text, 2).is_err());
    }
}
