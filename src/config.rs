//! `~/.config/chora/config.toml`. No env vars; `--dir` overrides for one run.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    pub main: Main,
    /// Missing `[calendar]` table falls back to defaults, key by key.
    #[serde(default)]
    pub calendar: Calendar,
}

/// Calendar display settings. Terminal palette only, no RGB.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Calendar {
    /// Color for events recorded on that date.
    #[serde(default = "default_day_color")]
    pub day_color: String,
    /// Color for weekday repeatables.
    #[serde(default = "default_repeat_color")]
    pub repeat_color: String,
    /// Whether repeatables are marked on the calendar at all.
    #[serde(default = "default_show_repeat")]
    pub show_repeat: bool,
    /// `strftime` format of the selected date, top-left of the calendar.
    #[serde(default = "default_header")]
    pub header: String,
    /// Today hint, top-right, only when looking at another date.
    #[serde(default = "default_show_today")]
    pub show_today: bool,
}

fn default_day_color() -> String {
    String::from("green")
}

fn default_repeat_color() -> String {
    String::from("blue")
}

fn default_show_repeat() -> bool {
    true
}

fn default_header() -> String {
    String::from("%A, %d.%m.%Y")
}

fn default_show_today() -> bool {
    true
}

impl Default for Calendar {
    fn default() -> Self {
        Self {
            day_color: default_day_color(),
            repeat_color: default_repeat_color(),
            show_repeat: default_show_repeat(),
            header: default_header(),
            show_today: default_show_today(),
        }
    }
}

/// What to do with day files older than today on every event action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OldEventPolicy {
    /// Delete passed day files.
    Delete,
    /// Move passed day files to `events/backlog/`.
    Backlog,
    /// Leave passed day files alone; the sweep never runs.
    #[default]
    Nothing,
}

impl OldEventPolicy {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Delete => "delete",
            Self::Backlog => "backlog",
            Self::Nothing => "nothing",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Main {
    /// `~/` expands to home.
    pub chora_dir: String,
    /// File opener. Empty = $EDITOR.
    #[serde(default)]
    pub editor: String,
    /// Directory opener. Empty = $EDITOR.
    #[serde(default)]
    pub open_dirs: String,
    /// Auto-commit while the editor is open, in minutes. 0 = off.
    #[serde(default)]
    pub auto_commit: u64,
    /// Old day files (before today) on every event action.
    #[serde(default)]
    pub manage_old_event: OldEventPolicy,
    /// Git remote. Empty = local only.
    #[serde(default)]
    pub remote_url: String,
}

impl Default for Main {
    fn default() -> Self {
        Self {
            chora_dir: String::from("~/chora"),
            editor: String::new(),
            open_dirs: String::new(),
            auto_commit: 0,
            manage_old_event: OldEventPolicy::Nothing,
            remote_url: String::new(),
        }
    }
}

#[derive(Debug)]
pub struct Ensured {
    pub path: PathBuf,
    /// Keys appended under `# NEW` (empty when nothing was missing).
    pub added: Vec<String>,
}

/// Display order of `[main]` keys in fresh files. Detection never uses
/// this: missing keys are derived from the serialized defaults below,
/// so a new struct field migrates even if nobody touches this list.
const MAIN_ORDER: [&str; 6] = [
    "chora_dir",
    "editor",
    "open_dirs",
    "auto_commit",
    "manage_old_event",
    "remote_url",
];

/// Display order of `[calendar]` keys in fresh files. Same contract.
const CALENDAR_ORDER: [&str; 5] = [
    "day_color",
    "repeat_color",
    "show_repeat",
    "header",
    "show_today",
];

/// Serialize the defaults: the single source for migration detection
/// and appended values. A new `#[serde(default)]` field is picked up
/// automatically — no list to forget.
fn defaults_table(defaults: &impl serde::Serialize) -> anyhow::Result<toml::Table> {
    match toml::Value::try_from(defaults) {
        Ok(toml::Value::Table(table)) => Ok(table),
        _ => anyhow::bail!("defaults do not serialize to a table"),
    }
}

/// Comment for a key in the generated file.
fn field_comment(key: &str) -> Option<&'static str> {
    match key {
        "chora_dir" => Some("# Storage root. A leading ~/ expands to your home directory."),
        "editor" => Some(
            "# Opener for files (day files). Empty = use $EDITOR. Try \"hx\" or \"code --wait\".",
        ),
        "open_dirs" => {
            Some("# Opener for directories (notes/, events/). Empty = use $EDITOR. Try \"yazi\".")
        }
        "auto_commit" => Some(
            "# Auto-commit while the editor is open, in minutes. 0 = off. Local commits only; the push happens on exit.",
        ),
        "day_color" => Some(
            "# Color for events recorded on that date. Terminal palette: black, red, green, yellow, blue, magenta, cyan, white, gray, darkgray, lightred, lightgreen, lightyellow, lightblue, lightmagenta, lightcyan.",
        ),
        "repeat_color" => Some("# Color for weekday repeatables. Same palette as day_color."),
        "show_repeat" => {
            Some("# Mark weekday repeatables on the calendar. false = day events only.")
        }
        "header" => Some(
            "# strftime format of the selected date, top-left of the calendar. NONE hides the header row.",
        ),
        "show_today" => Some("# Today hint, top-right, only when looking at another date."),
        "manage_old_event" => Some(
            "# Passed day files (before today) on every event action: delete, backlog (= move to events/backlog/), or nothing.",
        ),
        "remote_url" => Some("# SSH URL of the git remote. Empty = local-only git."),
        _ => None,
    }
}

/// Fresh file: header + commented keys.
fn default_file_text() -> anyhow::Result<String> {
    let main_defaults = defaults_table(&Main::default())?;
    let calendar_defaults = defaults_table(&Calendar::default())?;
    let mut out = String::from(
        "# chora config. After upgrades, re-run `chora init`: missing keys\n# are appended at the bottom under `# NEW`, your values are kept.\n[main]\n",
    );
    for key in MAIN_ORDER {
        let value = main_defaults
            .get(key)
            .cloned()
            .unwrap_or(toml::Value::String(String::new()));
        if let Some(comment) = field_comment(key) {
            out.push_str(comment);
            out.push('\n');
        }
        out.push_str(&format!("{key} = {value}\n"));
    }
    out.push_str("[calendar]\n");
    for key in CALENDAR_ORDER {
        let value = calendar_defaults
            .get(key)
            .cloned()
            .unwrap_or(toml::Value::String(String::new()));
        if let Some(comment) = field_comment(key) {
            out.push_str(comment);
            out.push('\n');
        }
        out.push_str(&format!("{key} = {value}\n"));
    }
    Ok(out)
}

impl Config {
    #[must_use]
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("chora").join("config.toml"))
    }

    /// The file must exist — `init` creates it.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path().context("failed to determine the config directory")?;
        Self::load_from(&path)
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        crate::debug_log!("config: {}", path.display());
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Err(crate::error::ChoraError::ConfigMissing(path.to_path_buf()).into());
            }
            Err(err) => {
                return Err(err).with_context(|| format!("failed to read {}", path.display()));
            }
        };
        let cfg: Self =
            toml::from_str(&text).map_err(|err| crate::error::ChoraError::ConfigInvalid {
                path: path.to_path_buf(),
                msg: err.to_string(),
            })?;
        Ok(cfg)
    }

    /// Missing keys are appended under `# NEW`; values are never touched.
    pub fn ensure_exists() -> anyhow::Result<Ensured> {
        let path = Self::path().context("failed to determine the config directory")?;
        Self::ensure_exists_at(&path)
    }

    pub fn ensure_exists_at(path: &Path) -> anyhow::Result<Ensured> {
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            let text = default_file_text()?;
            std::fs::write(path, text)
                .with_context(|| format!("failed to write {}", path.display()))?;
            return Ok(Ensured {
                path: path.to_path_buf(),
                added: Vec::new(),
            });
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let table: toml::Table = match toml::from_str(&text) {
            Ok(table) => table,
            // Leave broken files to load(): it reports them precisely.
            Err(_) => {
                return Ok(Ensured {
                    path: path.to_path_buf(),
                    added: Vec::new(),
                });
            }
        };
        let main_present: Vec<String> = table
            .get("main")
            .and_then(|main| main.as_table())
            .map(|main| main.keys().cloned().collect())
            .unwrap_or_default();
        let calendar_present: Vec<String> = table
            .get("calendar")
            .and_then(|calendar| calendar.as_table())
            .map(|calendar| calendar.keys().cloned().collect())
            .unwrap_or_default();
        let main_defaults = defaults_table(&Main::default())?;
        let calendar_defaults = defaults_table(&Calendar::default())?;
        // Derived from the serialized defaults: a new `#[serde(default)]`
        // field migrates with no list to update. `*_ORDER` is display-only.
        let main_missing: Vec<(String, toml::Value)> = main_defaults
            .iter()
            .filter(|(key, _)| !main_present.iter().any(|got| got == *key))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        let calendar_missing: Vec<(String, toml::Value)> = calendar_defaults
            .iter()
            .filter(|(key, _)| !calendar_present.iter().any(|got| got == *key))
            .map(|(key, default)| {
                // Carry over values from the retired `[main]` keys so
                // custom colors survive the move to `[calendar]`.
                let legacy = match key.as_str() {
                    "day_color" => table
                        .get("main")
                        .and_then(|main| main.as_table())
                        .and_then(|main| main.get("calendar_day_color"))
                        .cloned(),
                    "repeat_color" => table
                        .get("main")
                        .and_then(|main| main.as_table())
                        .and_then(|main| main.get("calendar_repeat_color"))
                        .cloned(),
                    _ => None,
                };
                (
                    format!("calendar.{key}"),
                    legacy.unwrap_or_else(|| default.clone()),
                )
            })
            .collect();
        if main_missing.is_empty() && calendar_missing.is_empty() {
            return Ok(Ensured {
                path: path.to_path_buf(),
                added: Vec::new(),
            });
        }
        let mut out = text;
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
        let mut added = Vec::new();
        if !main_missing.is_empty() {
            if !table.contains_key("main") {
                out.push_str("[main]\n");
            }
            out.push_str("# NEW\n");
            for (key, value) in &main_missing {
                if let Some(comment) = field_comment(key) {
                    out.push_str(comment);
                    out.push('\n');
                }
                out.push_str(&format!("{key} = {value}\n"));
                added.push(key.clone());
            }
        }
        if !calendar_missing.is_empty() {
            if !table.contains_key("calendar") {
                out.push_str("[calendar]\n");
            }
            out.push_str("# NEW\n");
            for (qualified, value) in &calendar_missing {
                let key = qualified.strip_prefix("calendar.").unwrap_or(qualified);
                if let Some(comment) = field_comment(key) {
                    out.push_str(comment);
                    out.push('\n');
                }
                out.push_str(&format!("{key} = {value}\n"));
                added.push(qualified.clone());
            }
        }
        std::fs::write(path, &out)
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(Ensured {
            path: path.to_path_buf(),
            added,
        })
    }

    /// `--dir` wins over `chora_dir`.
    #[must_use]
    pub fn root(&self, dir_override: Option<&Path>) -> PathBuf {
        match dir_override {
            Some(dir) => dir.to_path_buf(),
            None => expand_tilde(&self.main.chora_dir),
        }
    }
}

#[must_use]
pub fn expand_tilde(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = dirs::home_dir()
    {
        return home.join(rest);
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_order_covers_all_defaults() {
        // Display order is manual, detection is derived: this test keeps
        // them in sync, so no key is ever missing from fresh files.
        let mut main_keys: Vec<&str> = MAIN_ORDER.to_vec();
        main_keys.sort_unstable();
        let main_table = defaults_table(&Main::default()).unwrap();
        let mut main_defaults: Vec<&str> = main_table.keys().map(String::as_str).collect();
        main_defaults.sort_unstable();
        assert_eq!(main_keys, main_defaults);
        let mut calendar_keys: Vec<&str> = CALENDAR_ORDER.to_vec();
        calendar_keys.sort_unstable();
        let calendar_table = defaults_table(&Calendar::default()).unwrap();
        let mut calendar_defaults: Vec<&str> = calendar_table.keys().map(String::as_str).collect();
        calendar_defaults.sort_unstable();
        assert_eq!(calendar_keys, calendar_defaults);
    }

    #[test]
    fn defaults_match_concept() {
        let cfg = Config::default();
        assert_eq!(cfg.main.chora_dir, "~/chora");
        assert!(cfg.main.editor.is_empty());
        assert!(cfg.main.open_dirs.is_empty());
        assert_eq!(cfg.main.auto_commit, 0);
        assert_eq!(cfg.calendar.day_color, "green");
        assert_eq!(cfg.calendar.repeat_color, "blue");
        assert!(cfg.calendar.show_repeat);
        assert_eq!(cfg.calendar.header, "%A, %d.%m.%Y");
        assert!(cfg.calendar.show_today);
        assert_eq!(cfg.main.manage_old_event, OldEventPolicy::Nothing);
        assert!(cfg.main.remote_url.is_empty());
    }

    #[test]
    fn expand_tilde_joins_home() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~/chora"), home.join("chora"));
    }

    #[test]
    fn expand_tilde_leaves_absolute_paths_alone() {
        assert_eq!(expand_tilde("/tmp/x"), PathBuf::from("/tmp/x"));
    }

    #[test]
    fn load_from_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[main]\nchora_dir = \"/tmp/c\"\nauto_commit = 5\neditor = \"yazi\"\nremote_url = \"ssh://vps/x\"\n";
        std::fs::write(&path, text).unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.main.chora_dir, "/tmp/c");
        // Revived legacy key: old configs keep their value.
        assert_eq!(cfg.main.auto_commit, 5);
        assert_eq!(cfg.main.editor, "yazi");
        assert!(cfg.main.open_dirs.is_empty());
        assert_eq!(cfg.main.remote_url, "ssh://vps/x");
    }

    #[test]
    fn load_from_defaults_missing_openers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[main]\nchora_dir = \"/tmp/c\"\nauto_commit = 5\n";
        std::fs::write(&path, text).unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert!(cfg.main.editor.is_empty());
        assert!(cfg.main.open_dirs.is_empty());
    }

    #[test]
    fn load_from_missing_file_is_config_missing() {
        let err = Config::load_from(Path::new("/definitely/not/here.toml")).unwrap_err();
        assert!(err.to_string().contains("chora init"));
    }

    #[test]
    fn load_from_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not toml [[[").unwrap();
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn load_from_parses_old_event_policy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        for (raw, expected) in [
            ("delete", OldEventPolicy::Delete),
            ("backlog", OldEventPolicy::Backlog),
            ("nothing", OldEventPolicy::Nothing),
        ] {
            std::fs::write(
                &path,
                format!("[main]\nchora_dir = \"/tmp/c\"\nmanage_old_event = \"{raw}\"\n"),
            )
            .unwrap();
            assert_eq!(
                Config::load_from(&path).unwrap().main.manage_old_event,
                expected
            );
        }
    }

    #[test]
    fn load_from_rejects_unknown_old_event_policy() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[main]\nchora_dir = \"/tmp/c\"\nmanage_old_event = \"sometimes\"\n",
        )
        .unwrap();
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn load_from_rejects_non_integer_auto_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[main]\nchora_dir = \"/tmp/c\"\nauto_commit = \"often\"\n",
        )
        .unwrap();
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn ensure_creates_defaults_from_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let ensured = Config::ensure_exists_at(&path).unwrap();
        assert!(ensured.added.is_empty());
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.main.chora_dir, "~/chora");
        assert!(cfg.main.editor.is_empty());
        assert!(cfg.main.open_dirs.is_empty());
        assert_eq!(cfg.main.auto_commit, 0);
        assert_eq!(cfg.calendar.day_color, "green");
        assert_eq!(cfg.calendar.repeat_color, "blue");
        assert!(cfg.calendar.show_repeat);
        assert_eq!(cfg.calendar.header, "%A, %d.%m.%Y");
        assert!(cfg.calendar.show_today);
        assert_eq!(cfg.main.manage_old_event, OldEventPolicy::Nothing);
        assert!(cfg.main.remote_url.is_empty());
    }

    #[test]
    fn ensure_appends_new_fields_under_new_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# my old config\n[main]\nchora_dir = \"/tmp/keep\"\nauto_commit = 3\ncalendar_day_color = \"magenta\"\n",
        )
        .unwrap();
        let ensured = Config::ensure_exists_at(&path).unwrap();
        // Derived order is alphabetical within each table.
        assert_eq!(
            ensured.added,
            vec![
                "editor",
                "manage_old_event",
                "open_dirs",
                "remote_url",
                "calendar.day_color",
                "calendar.header",
                "calendar.repeat_color",
                "calendar.show_repeat",
                "calendar.show_today"
            ]
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my old config"));
        assert!(text.contains("# NEW\n"));
        assert!(text.contains("editor = \"\""));
        assert!(text.contains("open_dirs = \"\""));
        assert!(text.contains("[calendar]\n"));
        assert!(text.contains("show_repeat = true"));
        assert!(text.contains("manage_old_event = \"nothing\""));
        assert!(text.contains("remote_url = \"\""));
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.main.chora_dir, "/tmp/keep");
        // Legacy value survives the migration untouched.
        assert_eq!(cfg.main.auto_commit, 3);
        // Retired [main] colors move to [calendar] with values kept.
        assert_eq!(cfg.calendar.day_color, "magenta");
        assert_eq!(cfg.calendar.repeat_color, "blue");
        assert!(cfg.calendar.show_repeat);
        assert_eq!(cfg.calendar.header, "%A, %d.%m.%Y");
        assert!(cfg.calendar.show_today);
    }

    #[test]
    fn ensure_is_quiet_when_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[main]\nchora_dir = \"/tmp/c\"\nauto_commit = 5\neditor = \"yazi\"\nopen_dirs = \"yazi\"\nmanage_old_event = \"nothing\"\nremote_url = \"\"\n[calendar]\nday_color = \"green\"\nrepeat_color = \"blue\"\nshow_repeat = true\nheader = \"%A, %d.%m.%Y\"\nshow_today = true\n",
        )
        .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        let ensured = Config::ensure_exists_at(&path).unwrap();
        assert!(ensured.added.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    }

    #[test]
    fn ensure_leaves_broken_files_to_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not toml [[[").unwrap();
        let ensured = Config::ensure_exists_at(&path).unwrap();
        assert!(ensured.added.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not toml [[[");
    }
}
