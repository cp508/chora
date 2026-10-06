//! `~/.config/chora/config.toml`. No env vars; `--dir` overrides for one run.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    pub main: Main,
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

/// Default `[main]` keys in file order.
fn known_fields() -> Vec<(&'static str, toml::Value)> {
    let defaults = Main::default();
    vec![
        ("chora_dir", toml::Value::String(defaults.chora_dir)),
        ("editor", toml::Value::String(defaults.editor)),
        ("open_dirs", toml::Value::String(defaults.open_dirs)),
        (
            "manage_old_event",
            toml::Value::String(defaults.manage_old_event.as_str().to_owned()),
        ),
        ("remote_url", toml::Value::String(defaults.remote_url)),
    ]
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
        "manage_old_event" => Some(
            "# Passed day files (before today) on every event action: delete, backlog (= move to events/backlog/), or nothing.",
        ),
        "remote_url" => Some("# SSH URL of the git remote. Empty = local-only git."),
        _ => None,
    }
}

/// Fresh file: header + commented keys.
fn default_file_text() -> String {
    let mut out = String::from(
        "# chora config. After upgrades, re-run `chora init`: missing keys\n# are appended at the bottom under `# NEW`, your values are kept.\n[main]\n",
    );
    for (key, value) in known_fields() {
        if let Some(comment) = field_comment(key) {
            out.push_str(comment);
            out.push('\n');
        }
        out.push_str(&format!("{key} = {value}\n"));
    }
    out
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
            let text = default_file_text();
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
        let has_main = table.contains_key("main");
        let present: Vec<String> = table
            .get("main")
            .and_then(|main| main.as_table())
            .map(|main| main.keys().cloned().collect())
            .unwrap_or_default();
        let missing: Vec<(&str, toml::Value)> = known_fields()
            .into_iter()
            .filter(|(key, _)| !present.iter().any(|got| got == key))
            .collect();
        if missing.is_empty() {
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
        if !has_main {
            out.push_str("[main]\n");
        }
        out.push_str("# NEW\n");
        let mut added = Vec::new();
        for (key, value) in &missing {
            if let Some(comment) = field_comment(key) {
                out.push_str(comment);
                out.push('\n');
            }
            out.push_str(&format!("{key} = {value}\n"));
            added.push((*key).to_owned());
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
    fn defaults_match_concept() {
        let cfg = Config::default();
        assert_eq!(cfg.main.chora_dir, "~/chora");
        assert!(cfg.main.editor.is_empty());
        assert!(cfg.main.open_dirs.is_empty());
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
        // Unknown keys from older versions are ignored.
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
    fn ensure_creates_defaults_from_scratch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let ensured = Config::ensure_exists_at(&path).unwrap();
        assert!(ensured.added.is_empty());
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.main.chora_dir, "~/chora");
        assert!(cfg.main.editor.is_empty());
        assert!(cfg.main.open_dirs.is_empty());
        assert_eq!(cfg.main.manage_old_event, OldEventPolicy::Nothing);
        assert!(cfg.main.remote_url.is_empty());
    }

    #[test]
    fn ensure_appends_new_fields_under_new_marker() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# my old config\n[main]\nchora_dir = \"/tmp/keep\"\nauto_commit = 3\n",
        )
        .unwrap();
        let ensured = Config::ensure_exists_at(&path).unwrap();
        assert_eq!(
            ensured.added,
            vec!["editor", "open_dirs", "manage_old_event", "remote_url"]
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("# my old config"));
        assert!(text.contains("# NEW\n"));
        assert!(text.contains("editor = \"\""));
        assert!(text.contains("open_dirs = \"\""));
        assert!(text.contains("manage_old_event = \"nothing\""));
        assert!(text.contains("remote_url = \"\""));
        let cfg = Config::load_from(&path).unwrap();
        assert_eq!(cfg.main.chora_dir, "/tmp/keep");
    }

    #[test]
    fn ensure_is_quiet_when_current() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[main]\nchora_dir = \"/tmp/c\"\nauto_commit = 5\neditor = \"yazi\"\nopen_dirs = \"yazi\"\nmanage_old_event = \"nothing\"\nremote_url = \"\"\n",
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
