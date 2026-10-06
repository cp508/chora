//! `show` rendering: plaintext for humans, JSON for scripts.

use chrono::NaiveDate;
use serde::Serialize;

use crate::event::{Event, format_duration};

/// One event in the `--json` output.
#[derive(Debug, Serialize)]
struct EventJson {
    title: String,
    date: String,
    time: String,
    duration: String,
    context: String,
}

impl From<&Event> for EventJson {
    fn from(event: &Event) -> Self {
        Self {
            title: event.title.clone(),
            date: event.date.format("%Y-%m-%d").to_string(),
            time: event.time.format("%H:%M").to_string(),
            duration: format_duration(event.duration),
            context: event.context.clone(),
        }
    }
}

/// Human-readable rendering of a day.
#[must_use]
pub fn plaintext(date: NaiveDate, events: &[Event]) -> String {
    let mut out = format!("# {}\n", date.format("%Y-%m-%d"));
    if events.is_empty() {
        out.push_str("(no events)\n");
        return out;
    }
    for event in events {
        out.push_str(&format!(
            "## {} — {} ({})\n",
            event.title,
            event.time.format("%H:%M"),
            format_duration(event.duration)
        ));
        if !event.context.is_empty() {
            out.push_str(&event.context);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

/// Machine-readable rendering of a day: an array of
/// `{title, date, time, duration, context}`.
pub fn json(events: &[Event]) -> anyhow::Result<String> {
    let items: Vec<EventJson> = events.iter().map(EventJson::from).collect();
    Ok(serde_json::to_string_pretty(&items)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use chrono::NaiveTime;

    fn sample() -> Vec<Event> {
        vec![Event {
            title: String::from("Standup"),
            date: NaiveDate::from_ymd_opt(2026, 11, 20).unwrap(),
            time: NaiveTime::from_hms_opt(11, 0, 0).unwrap(),
            duration: Duration::from_secs(7200),
            context: String::from("Agenda."),
        }]
    }

    #[test]
    fn plaintext_shows_title_time_and_context() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        let text = plaintext(date, &sample());
        assert!(text.contains("Standup"));
        assert!(text.contains("11:00"));
        assert!(text.contains("2h"));
        assert!(text.contains("Agenda."));
    }

    #[test]
    fn plaintext_empty_day() {
        let date = NaiveDate::from_ymd_opt(2026, 11, 20).unwrap();
        assert!(plaintext(date, &[]).contains("no events"));
    }

    #[test]
    fn json_shape() {
        let parsed: serde_json::Value = serde_json::from_str(&json(&sample()).unwrap()).unwrap();
        let item = &parsed[0];
        assert_eq!(item["title"], "Standup");
        assert_eq!(item["date"], "2026-11-20");
        assert_eq!(item["time"], "11:00");
        assert_eq!(item["duration"], "2h");
        assert_eq!(item["context"], "Agenda.");
    }

    #[test]
    fn json_empty_day_is_empty_array() {
        assert_eq!(json(&[]).unwrap(), "[]");
    }
}
