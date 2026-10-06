//! Day events: `##`-section parser <-> [`Event`].
//!
//! The parser is deliberately lenient: a section without valid
//! `Time:`/`Duration:` fields is just day text and is skipped, not an error.

use std::time::Duration;

use chrono::{NaiveDate, NaiveTime};

use crate::error::ChoraError;

/// One event of a day.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// The `##` header (= `name` from the TUI).
    pub title: String,
    /// Date taken from the file name.
    pub date: NaiveDate,
    /// `Time: HH:MM`.
    pub time: NaiveTime,
    /// `Duration:`, normalized to seconds by the parser.
    pub duration: Duration,
    /// The rest of the section, free form.
    pub context: String,
}

/// Parse a duration: `10s`, `45m`, `2h`, `1h30m`.
///
/// Groups of `<number><s|m|h>` run back to back, at least one is required;
/// dangling digits and unknown suffixes are an error.
pub fn parse_duration(raw: &str) -> Result<Duration, ChoraError> {
    let bad = || ChoraError::BadDuration(raw.to_owned());
    let text = raw.trim();
    let mut total_secs: u64 = 0;
    let mut groups = 0;
    let mut num: u64 = 0;
    let mut have_num = false;
    for ch in text.chars() {
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

/// Compress a duration back to a string: `2h`, `45m`, `10s`, `1h30m`.
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let (hours, mins, secs) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    let mut out = String::new();
    if hours > 0 {
        out.push_str(&format!("{hours}h"));
    }
    if mins > 0 {
        out.push_str(&format!("{mins}m"));
    }
    if secs > 0 || out.is_empty() {
        out.push_str(&format!("{secs}s"));
    }
    out
}

/// Parse a TUI time entry against `now`.
///
/// Accepted forms: `HH:MM`, `HHMM`, `H`/`HH` (hour sharp), `now`/empty
/// (keep `now`), `+NN` (NN minutes from now), `+<duration>` (`+1h30m`).
/// Offsets wrap past midnight.
pub fn parse_time_entry(raw: &str, now: NaiveTime) -> Result<NaiveTime, ChoraError> {
    let bad = || ChoraError::BadTime(raw.to_owned());
    let text = raw.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("now") {
        return Ok(now);
    }
    if let Some(rest) = text.strip_prefix('+') {
        let rest = rest.trim();
        if !rest.is_empty() && rest.chars().all(|ch| ch.is_ascii_digit()) {
            let mins: i64 = rest.parse().map_err(|_| bad())?;
            return Ok(shift_time_of_day(now, mins));
        }
        let duration = parse_duration(rest)?;
        let secs = duration.as_secs();
        let mins = secs.div_ceil(60) as i64;
        return Ok(shift_time_of_day(now, mins));
    }
    if let Some((hours, mins)) = text.split_once(':') {
        let hours: u32 = hours.parse().map_err(|_| bad())?;
        let mins: u32 = mins.parse().map_err(|_| bad())?;
        return NaiveTime::from_hms_opt(hours, mins, 0).ok_or_else(bad);
    }
    if !text.chars().all(|ch| ch.is_ascii_digit()) {
        return Err(bad());
    }
    match text.len() {
        1 | 2 => {
            let hours: u32 = text.parse().map_err(|_| bad())?;
            NaiveTime::from_hms_opt(hours, 0, 0).ok_or_else(bad)
        }
        3 | 4 => {
            let (hours, mins) = text.split_at(text.len() - 2);
            let hours: u32 = hours.parse().map_err(|_| bad())?;
            let mins: u32 = mins.parse().map_err(|_| bad())?;
            NaiveTime::from_hms_opt(hours, mins, 0).ok_or_else(bad)
        }
        _ => Err(bad()),
    }
}

/// Shift a time of day, wrapping past midnight.
#[must_use]
pub fn shift_time_of_day(time: NaiveTime, mins: i64) -> NaiveTime {
    time.overflowing_add_signed(chrono::Duration::minutes(mins))
        .0
}

/// Parse a day file into events. Sections without `Time:` are skipped.
#[must_use]
pub fn parse_day(date: NaiveDate, text: &str) -> Vec<Event> {
    let mut events = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    for line in text.lines() {
        if let Some(title) = line.strip_prefix("##") {
            if let Some((prev_title, prev_body)) = current.take() {
                push_section(&mut events, date, &prev_title, &prev_body);
            }
            current = Some((title.trim().to_owned(), Vec::new()));
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
    }
    if let Some((title, body)) = current.take() {
        push_section(&mut events, date, &title, &body);
    }
    events
}

/// Turn one section into an event when it holds valid fields.
fn push_section(events: &mut Vec<Event>, date: NaiveDate, title: &str, body: &[&str]) {
    if title.is_empty() {
        return;
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
    if let (Some(time), Some(duration)) = (time, duration) {
        events.push(Event {
            title: title.to_owned(),
            date,
            time,
            duration,
            context: context.join("\n").trim().to_owned(),
        });
    }
}

/// Serialize an event into a day-file section.
#[must_use]
pub fn render_section(event: &Event) -> String {
    let mut out = String::from("## ");
    out.push_str(&event.title);
    out.push_str(&format!("\nTime: {}\n", event.time.format("%H:%M")));
    out.push_str(&format!("Duration: {}\n", format_duration(event.duration)));
    if event.context.is_empty() {
        out.push('\n');
    } else {
        out.push('\n');
        out.push_str(event.context.trim());
        out.push('\n');
    }
    out
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
        ] {
            assert!(parse_duration(raw).is_err(), "{raw}");
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
    fn parse_time_entry_table() {
        let now = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        let at = |h, m| NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let cases = [
            ("14:30", at(14, 30)),
            (" 9:05 ", at(9, 5)),
            ("1430", at(14, 30)),
            ("930", at(9, 30)),
            ("9", at(9, 0)),
            ("09", at(9, 0)),
            ("", now),
            ("now", now),
            ("NOW", now),
            ("+30", at(14, 30)),
            ("+1h", at(15, 0)),
            ("+1h30m", at(15, 30)),
            ("+600m", at(0, 0)),
        ];
        for (raw, expected) in cases {
            assert_eq!(parse_time_entry(raw, now).unwrap(), expected, "{raw}");
        }
    }

    #[test]
    fn parse_time_entry_rejects_garbage() {
        let now = NaiveTime::from_hms_opt(14, 0, 0).unwrap();
        for raw in [
            "25:00", "12:60", "abc", "14:30:00", "12345", "+abc", "+", "-", "14-30", "1h",
        ] {
            assert!(parse_time_entry(raw, now).is_err(), "{raw}");
        }
    }
}
