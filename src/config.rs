//! `~/.config/chora/config.toml` — the single place for configuration.
//!
//! No environment variables. The `--dir` flag overrides `chora_dir`
//! for one run.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::{Deserialize, Serialize};

/// Root structure of `config.toml`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Config {
    /// The `[main]` section.
    pub main: Main,
}

/// The `[main]` section: exactly four fields, nothing extra.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Main {
    /// Storage root. A leading `~/` expands to the home directory.
    pub chora_dir: String,
    /// `chora note` autocommit interval in minutes. Minimum 1.
    pub auto_commit: u64,
    /// Opener command for notes, e.g. "hx .", "yazi", "code --wait".
    /// Empty string = use $EDITOR. Split like a shell line.
    #[serde(default)]
    pub editor: String,
    /// SSH URL of the remote. Empty string = local-only git.
    #[serde(default)]
    pub remote_url: String,
}

impl Default for Main {
    fn default() -> Self {
        Self {
            chora_dir: String::from("~/chora"),
            auto_commit: 10,
            editor: String::new(),
            remote_url: String::new(),
        }
    }
}

impl Config {
    /// Path to the config file (`$HOME/.config/chora/config.toml`).
    #[must_use]
    pub fn path() -> Option<PathBuf> {
        dirs::config_dir().map(|dir| dir.join("chora").join("config.toml"))
    }

    /// Read the config from disk. The file must exist — `init` creates it.
    pub fn load() -> anyhow::Result<Self> {
        let path = Self::path().context("failed to determine the config directory")?;
        Self::load_from(&path)
    }

    /// Read the config from an explicit path (extension point for tests).
    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
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
        cfg.validate(path)?;
        Ok(cfg)
    }

    /// Create the config with defaults if missing. Returns the path.
    /// Idempotent: never touches an existing file.
    pub fn ensure_exists() -> anyhow::Result<PathBuf> {
        let path = Self::path().context("failed to determine the config directory")?;
        if !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            let text = toml::to_string_pretty(&Self::default())
                .context("failed to serialize the default config")?;
            std::fs::write(&path, text)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        Ok(path)
    }

    /// Storage root: `--dir` > `chora_dir` > `~/chora`.
    #[must_use]
    pub fn root(&self, dir_override: Option<&Path>) -> PathBuf {
        match dir_override {
            Some(dir) => dir.to_path_buf(),
            None => expand_tilde(&self.main.chora_dir),
        }
    }

    /// Autocommit interval as a `Duration`. Zero is rejected (protects
    /// the ticker from a busy loop even if the config was built by hand
    /// past `validate`).
    pub fn every(&self) -> anyhow::Result<std::time::Duration> {
        if self.main.auto_commit == 0 {
            anyhow::bail!("auto_commit must be >= 1 (minutes)");
        }
        let secs = self
            .main
            .auto_commit
            .checked_mul(60)
            .context("auto_commit is too large")?;
        Ok(std::time::Duration::from_secs(secs))
    }

    /// Config invariants. Runs on every load — fail fast.
    fn validate(&self, path: &Path) -> anyhow::Result<()> {
        if self.main.auto_commit == 0 {
            return Err(crate::error::ChoraError::ConfigInvalid {
                path: path.to_path_buf(),
                msg: String::from("auto_commit must be >= 1 (minutes)"),
            }
            .into());
        }
        Ok(())
    }
}

/// Expand a leading `~/` to the home directory. Everything else is untouched.
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
        assert_eq!(cfg.main.auto_commit, 10);
        assert!(cfg.main.editor.is_empty());
        assert!(cfg.main.remote_url.is_empty());
    }

    #[test]
    fn every_converts_minutes_to_duration() {
        let cfg = Config::default();
        assert_eq!(cfg.every().unwrap(), std::time::Duration::from_secs(600));
    }

    #[test]
    fn every_rejects_zero() {
        let mut cfg = Config::default();
        cfg.main.auto_commit = 0;
        assert!(cfg.every().is_err());
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
        assert_eq!(cfg.main.auto_commit, 5);
        assert_eq!(cfg.main.editor, "yazi");
        assert_eq!(cfg.main.remote_url, "ssh://vps/x");
    }

    #[test]
    fn load_from_defaults_missing_editor() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let text = "[main]\nchora_dir = \"/tmp/c\"\nauto_commit = 5\n";
        std::fs::write(&path, text).unwrap();
        let cfg = Config::load_from(&path).unwrap();
        assert!(cfg.main.editor.is_empty());
    }

    #[test]
    fn load_from_missing_file_is_config_missing() {
        let err = Config::load_from(Path::new("/definitely/not/here.toml")).unwrap_err();
        assert!(err.to_string().contains("chora init"));
    }

    #[test]
    fn load_from_rejects_zero_auto_commit() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[main]\nchora_dir = \"/tmp/c\"\nauto_commit = 0\n").unwrap();
        assert!(Config::load_from(&path).is_err());
    }

    #[test]
    fn load_from_rejects_garbage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not toml [[[").unwrap();
        assert!(Config::load_from(&path).is_err());
    }
}
