//! `index.md`: human-owned context + `security`/`about` per notes path.
//!
//! Soft parse: a missing entry means `open` + empty `about`;
//! a bad `security` value means `open` plus a warning. Agents must never
//! write this file (only `chora open index`, by a human).

use std::path::Path;

use anyhow::Context;

/// Note visibility: `open < restricted < critical`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
pub enum AccessLevel {
    Open,
    Restricted,
    Critical,
}

impl AccessLevel {
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
    pub fn allows(self, max: Self) -> bool {
        self <= max
    }
}

/// One parsed warning. `path` scopes it for `--max-access` filtering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub path: Option<String>,
    pub msg: String,
}

/// One `###`/`####`+ entry: `path` is `/`-joined (`docs/guides/install.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    pub security: AccessLevel,
    pub about: String,
}

#[derive(Debug, Clone, Default)]
pub struct Index {
    pub context: String,
    pub entries: Vec<Entry>,
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
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Parse `index.md` text. Never fails: problems become warnings.
    #[must_use]
    pub fn parse(text: &str) -> Self {
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
                    let Some(dir) = current.clone() else {
                        index.warnings.push(Warning {
                            path: None,
                            msg: format!("orphan header `{title}`, skipped"),
                        });
                        continue;
                    };
                    if title.is_empty() {
                        index.warnings.push(Warning {
                            path: Some(dir),
                            msg: String::from("empty header, skipped"),
                        });
                        continue;
                    }
                    stack.truncate(level - 4);
                    stack.push(title.to_owned());
                    let path = format!("{dir}/{}", stack.join("/"));
                    index.entries.push(Entry {
                        path,
                        security: AccessLevel::Open,
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
                            index.warnings.push(Warning {
                                path: Some(path),
                                msg: format!("bad security `{value}`, treated as open"),
                            });
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
        index.context = context_lines.join("\n").trim().to_owned();
        index
    }

    /// Read `<root>/index.md`. Missing file = empty index, not an error.
    pub fn load(root: &Path) -> anyhow::Result<Self> {
        let path = root.join("index.md");
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::empty()),
            Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
        }
    }

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
        self.entries.iter().filter(move |e| e.security.allows(max))
    }

    /// Warnings visible under `max` clearance (path-less ones always show).
    pub fn visible_warnings(&self, max: AccessLevel) -> Vec<&Warning> {
        self.warnings
            .iter()
            .filter(|w| match &w.path {
                None => true,
                Some(path) => self.security_for(path).allows(max),
            })
            .collect()
    }

    /// Starter file: `# Context` + one `###` per existing top-level dirname.
    #[must_use]
    pub fn skeleton(dirs: &[String]) -> String {
        let mut out = String::from("# Context\n\nDescribe all notes here (English).\n\n## Notes\n");
        for dir in dirs {
            out.push_str(&format!("\n### {dir}\nsecurity: open\nabout: \n"));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Context\n\nAll project notes.\n\n## Notes\n\n### docs\nsecurity: restricted\nabout: Project docs\n\n#### guides\nsecurity: open\nabout: How-tos\n\n##### install.md\nabout: Install steps\n\n### secret\nsecurity: critical\nabout: Secrets\n\n### broken\nsecurity: nonsense\nabout: Bad level\n";

    #[test]
    fn parses_context_and_entries() {
        let index = Index::parse(SAMPLE);
        assert_eq!(index.context, "All project notes.");
        let paths: Vec<&str> = index.entries.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(
            paths,
            vec![
                "docs",
                "docs/guides",
                "docs/guides/install.md",
                "secret",
                "broken"
            ]
        );
        assert_eq!(index.entries[0].security, AccessLevel::Restricted);
        assert_eq!(index.entries[0].about, "Project docs");
        assert_eq!(index.entries[1].security, AccessLevel::Open);
        assert_eq!(index.entries[2].security, AccessLevel::Open);
        assert_eq!(index.entries[2].about, "Install steps");
        // Bad level falls back to open with a warning.
        assert_eq!(index.entries[4].security, AccessLevel::Open);
        assert_eq!(index.warnings.len(), 1);
        assert_eq!(index.warnings[0].path.as_deref(), Some("broken"));
    }

    #[test]
    fn security_prefers_longest_prefix_but_about_is_exact_only() {
        let index = Index::parse(SAMPLE);
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
        let index = Index::parse(SAMPLE);
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
        assert_eq!(all.len(), 5);
        // The `broken` warning resolves to open, so it shows even at open.
        assert_eq!(index.visible_warnings(AccessLevel::Open).len(), 1);
    }

    #[test]
    fn empty_text_is_empty_index() {
        let index = Index::parse("");
        assert!(index.context.is_empty());
        assert!(index.entries.is_empty());
        assert_eq!(index.security_for("a/b.md"), AccessLevel::Open);
        assert_eq!(index.about_for("a/b.md"), "");
    }
}
