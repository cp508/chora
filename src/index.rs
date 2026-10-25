//! `index.md`: human-owned context + `security`/`about` per notes path.
//!
//! Mostly soft parse: a missing entry means `open` + empty `about`.
//! A bad `security:` value is a hard error (fail-closed: a typo must not
//! silently downgrade a note to `open`). Agents must never write this file
//! (only `chora open index`, by a human).

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Context;

/// Note visibility: `open < restricted < critical`.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[cfg_attr(feature = "cli", derive(clap::ValueEnum))]
pub enum AccessLevel {
    /// Visible to everyone, including the default clearance.
    Open,
    /// Visible with `--max-access restricted` and above.
    Restricted,
    /// Visible only with `--max-access critical`.
    Critical,
}

impl AccessLevel {
    /// Lowercase level name for output and errors.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Restricted => "restricted",
            Self::Critical => "critical",
        }
    }

    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "open" => Some(Self::Open),
            "restricted" => Some(Self::Restricted),
            "critical" => Some(Self::Critical),
            _ => None,
        }
    }

    /// Visible when the entry level fits the reader clearance.
    #[must_use]
    pub fn permits(self, max: Self) -> bool {
        self <= max
    }
}

/// One parsed warning. `path` scopes it for `--max-access` filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    /// Entry path the warning belongs to; `None` = file-wide.
    pub path: Option<String>,
    /// Human-readable warning text.
    pub msg: String,
}

/// One `###`/`####`+ entry: `path` is `/`-joined (`docs/guides/install.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// `/`-joined note path (`docs` or `docs/guides/install.md`).
    pub path: String,
    /// Effective level: explicit `security:` or inherited from the parent.
    pub security: AccessLevel,
    /// Explicit `about:` text (never inherited from parents).
    pub about: String,
}

/// Parsed `index.md`: human context plus the notes tree.
#[derive(Debug, Clone, Default)]
pub struct Index {
    /// Free-form `# Context` text (legacy placeholder line dropped).
    pub context: String,
    /// Entries in file order.
    pub entries: Vec<Entry>,
    /// Soft-parse warnings (bad `security:` is a hard error instead).
    pub warnings: Vec<Warning>,
}

/// Split a markdown header line into `(level, title)`.
fn header(line: &str) -> Option<(usize, &str)> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    if hashes == 0 {
        return None;
    }
    Some((hashes, line[hashes..].trim()))
}

fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (name, value) = line.split_once(':')?;
    name.trim().eq_ignore_ascii_case(key).then(|| value.trim())
}

impl Index {
    /// Empty index: no context, entries or warnings.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parse `index.md` text. Bad `security:` values are a hard error
    /// (a typo must not silently downgrade a note to `open`); other
    /// problems become warnings.
    ///
    /// # Errors
    ///
    /// Fails on an unknown `security:` value at any entry.
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let mut index = Self::empty();
        let mut in_context = false;
        let mut in_notes = false;
        let mut context_lines: Vec<&str> = Vec::new();
        let mut current: Option<String> = None;
        let mut stack: Vec<String> = Vec::new();
        let mut entry: Option<usize> = None;

        for line in text.lines() {
            if let Some((level, title)) = header(line.trim_start()) {
                entry = None;
                if level == 1 {
                    in_context = title.eq_ignore_ascii_case("context");
                    in_notes = false;
                    current = None;
                } else if level == 2 {
                    in_notes = title.eq_ignore_ascii_case("notes");
                    in_context = false;
                    current = None;
                } else if level == 3 && in_notes {
                    if title.is_empty() {
                        index.warnings.push(Warning {
                            path: None,
                            msg: String::from("empty ### header, skipped"),
                        });
                        current = None;
                    } else {
                        current = Some(title.to_owned());
                        stack.clear();
                        index.entries.push(Entry {
                            path: title.to_owned(),
                            security: AccessLevel::Open,
                            about: String::new(),
                        });
                        entry = Some(index.entries.len() - 1);
                    }
                } else if level >= 4 && in_notes {
                    let Some(dir) = current.as_ref() else {
                        index.warnings.push(Warning {
                            path: None,
                            msg: format!("orphan header `{title}`, skipped"),
                        });
                        continue;
                    };
                    if title.is_empty() {
                        index.warnings.push(Warning {
                            path: current.clone(),
                            msg: String::from("empty header, skipped"),
                        });
                        continue;
                    }
                    stack.truncate(level - 4);
                    // Children inherit the nearest ancestor's security unless
                    // they set their own `security:` below.
                    let parent = if stack.is_empty() {
                        dir.clone()
                    } else {
                        format!("{dir}/{}", stack.join("/"))
                    };
                    let security = index.security_for(&parent);
                    stack.push(title.to_owned());
                    let path = format!("{dir}/{}", stack.join("/"));
                    index.entries.push(Entry {
                        path,
                        security,
                        about: String::new(),
                    });
                    entry = Some(index.entries.len() - 1);
                }
                continue;
            }
            if in_context && !in_notes {
                context_lines.push(line);
            } else if let Some(idx) = entry {
                if let Some(value) = field(line.trim(), "security") {
                    match AccessLevel::parse(value) {
                        Some(level) => index.entries[idx].security = level,
                        None => {
                            let path = index.entries[idx].path.clone();
                            anyhow::bail!(
                                "bad security `{value}` at `{path}`: expected open|restricted|critical"
                            );
                        }
                    }
                } else if let Some(value) = field(line.trim(), "about")
                    && index.entries[idx].about.is_empty()
                    && !value.is_empty()
                {
                    index.entries[idx].about = value.to_owned();
                }
            }
        }
        index.context = context_lines
            .join("\n")
            .lines()
            .filter(|line| line.trim() != Self::LEGACY_PLACEHOLDER)
            .collect::<Vec<_>>()
            .join("\n")
            .trim()
            .to_owned();
        Ok(index)
    }

    /// Read `<root>/index.md`. Missing file = empty index, not an error.
    pub fn load(root: &Path) -> anyhow::Result<Self> {
        let path = root.join("index.md");
        match std::fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
        }
    }

    /// Legacy skeleton placeholder: never shown in `chora index` output.
    /// Old files keep it on disk, but `parse` drops the line so agents
    /// never see it.
    pub const LEGACY_PLACEHOLDER: &'static str = "Describe all notes here (English).";

    /// Agent-readable events guide: how events are created, which weekdays
    /// exist, how daily events work. Shown by `chora index` and
    /// `chora index events` (events have no access levels).
    pub const EVENTS_GUIDE: &'static str = "Events live in events/YYYY-MM-DD.md as ## sections with Time: HH:MM and Duration: <DUR>.\nCreate one without an editor: chora event --date <DATE> --title <TITLE> --time <HH:MM> --duration <DUR> [--body <TEXT>] (--date defaults to today).\nOr append a stub and edit by hand: chora event --date <DATE>, or the TUI chora event, or chora open event.\n<DATE> is today|tomorrow|YYYY-MM-DD|DD.MM.YYYY. <TIME> is HH:MM or H (9 = 09:00). <DUR> is 30m, 2h20m, 2h 20m, 2:00 (=2h), 1:30 (=1h30m), 10s.\nSections without valid Time:/Duration: are plain day text, skipped by event show; day output is sorted by time.\nRepeatable events: events/repeatable/<weekday>.md, same section format, no dedicated command (edit via chora open event).\nWeekdays (lowercase English): monday, tuesday, wednesday, thursday, friday, saturday, sunday. Other file names are invisible.\nevent show --date <DATE> merges the day file with that weekday's repeatable. Daily event: put the same ## section into all 7 weekday files.\nOld-day sweep (manage_old_event) never touches repeatable/.\n";

    /// Effective security for a `/`-joined relative note path:
    /// the longest-prefix entry wins, else `open`.
    #[must_use]
    pub fn security_for(&self, rel: &str) -> AccessLevel {
        self.entries
            .iter()
            .filter(|e| rel == e.path || rel.starts_with(&format!("{}/", e.path)))
            .max_by_key(|e| e.path.len())
            .map(|e| e.security)
            .unwrap_or(AccessLevel::Open)
    }

    /// Explicit `about` of the exact entry, else `""`.
    /// Never inherited: an h3 `about` does not flow into nested files/dirs
    /// (files fall back to their own h1 at show time).
    #[must_use]
    pub fn about_for(&self, rel: &str) -> &str {
        self.entries
            .iter()
            .find(|e| e.path == rel)
            .map(|e| e.about.as_str())
            .unwrap_or("")
    }

    /// Entries visible under `max` clearance, in file order.
    pub fn visible(&self, max: AccessLevel) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(move |e| e.security.permits(max))
    }

    /// Warnings visible under `max` clearance (path-less ones always show).
    pub fn visible_warnings(&self, max: AccessLevel) -> Vec<&Warning> {
        self.warnings
            .iter()
            .filter(|w| match &w.path {
                None => true,
                Some(path) => self.security_for(path).permits(max),
            })
            .collect()
    }

    /// Starter file: `# Context` + one `###` per existing top-level dirname.
    #[must_use]
    pub fn skeleton(dirs: &[String]) -> String {
        let mut out = String::from("# Context\n\n## Notes\n");
        for dir in dirs {
            let _ = writeln!(out, "\n### {dir}\nsecurity: open\nabout: ");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Context\n\nAll project notes.\n\n## Notes\n\n### docs\nsecurity: restricted\nabout: Project docs\n\n#### guides\nsecurity: open\nabout: How-tos\n\n##### install.md\nabout: Install steps\n\n### secret\nsecurity: critical\nabout: Secrets\n";

    #[test]
    fn parses_context_and_entries() {
        let index = Index::parse(SAMPLE).unwrap();
        assert_eq!(index.context, "All project notes.");
        let paths: Vec<&str> = index.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["docs", "docs/guides", "docs/guides/install.md", "secret",]
        );
        assert_eq!(index.entries[0].security, AccessLevel::Restricted);
        assert_eq!(index.entries[0].about, "Project docs");
        assert_eq!(index.entries[1].security, AccessLevel::Open);
        assert_eq!(index.entries[2].security, AccessLevel::Open);
        assert_eq!(index.entries[2].about, "Install steps");
        assert!(index.warnings.is_empty());
    }

    #[test]
    fn children_inherit_parent_security_unless_set() {
        let index = Index::parse(
            "## Notes\n\n### work\nsecurity: critical\n\n#### sub.md\nabout: No level here\n\n#### open.md\nsecurity: open\n",
        )
        .unwrap();
        assert_eq!(index.security_for("work/sub.md"), AccessLevel::Critical);
        assert_eq!(index.security_for("work/open.md"), AccessLevel::Open);
        assert_eq!(index.about_for("work/sub.md"), "No level here");
    }

    #[test]
    fn bad_security_is_a_hard_error() {
        let err = Index::parse("## Notes\n\n### broken\nsecurity: nonsense\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("broken"), "{msg}");
        assert!(msg.contains("nonsense"), "{msg}");
    }

    #[test]
    fn security_prefers_longest_prefix_but_about_is_exact_only() {
        let index = Index::parse(SAMPLE).unwrap();
        assert_eq!(
            index.security_for("docs/guides/install.md"),
            AccessLevel::Open
        );
        assert_eq!(index.about_for("docs/guides/install.md"), "Install steps");
        // Security inherits from `docs`, but its `about` does not flow down.
        assert_eq!(index.security_for("docs/other.md"), AccessLevel::Restricted);
        assert_eq!(index.about_for("docs/other.md"), "");
        assert_eq!(index.about_for("docs"), "Project docs");
        assert_eq!(index.security_for("unknown/x.md"), AccessLevel::Open);
        assert_eq!(index.about_for("unknown/x.md"), "");
    }

    #[test]
    fn visibility_filters_by_clearance() {
        let index = Index::parse(SAMPLE).unwrap();
        let open: Vec<&str> = index
            .visible(AccessLevel::Open)
            .map(|e| e.path.as_str())
            .collect();
        assert!(!open.contains(&"docs"));
        assert!(open.contains(&"docs/guides"));
        assert!(!open.contains(&"secret"));
        let all: Vec<&str> = index
            .visible(AccessLevel::Critical)
            .map(|e| e.path.as_str())
            .collect();
        assert_eq!(all.len(), 4);
        assert!(index.visible_warnings(AccessLevel::Open).is_empty());
    }

    #[test]
    fn empty_text_is_empty_index() {
        let index = Index::parse("").unwrap();
        assert!(index.context.is_empty());
        assert!(index.entries.is_empty());
        assert_eq!(index.security_for("a/b.md"), AccessLevel::Open);
        assert_eq!(index.about_for("a/b.md"), "");
    }

    #[test]
    fn skeleton_has_no_placeholder() {
        let text = Index::skeleton(&[String::from("docs")]);
        assert!(!text.contains(Index::LEGACY_PLACEHOLDER));
        assert!(text.contains("### docs"));
    }

    #[test]
    fn parse_drops_legacy_placeholder() {
        let index =
            Index::parse("# Context\n\nDescribe all notes here (English).\n\n## Notes\n").unwrap();
        assert!(index.context.is_empty());
        let index = Index::parse(
            "# Context\n\nReal context.\nDescribe all notes here (English).\n\n## Notes\n",
        )
        .unwrap();
        assert_eq!(index.context, "Real context.");
    }

    #[test]
    fn events_guide_lists_weekdays_and_daily() {
        for day in [
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ] {
            assert!(Index::EVENTS_GUIDE.contains(day), "guide misses {day}");
        }
        assert!(Index::EVENTS_GUIDE.contains("repeatable"));
        assert!(Index::EVENTS_GUIDE.contains("all 7 weekday"));
        assert!(Index::EVENTS_GUIDE.contains("chora event --date"));
    }
}
