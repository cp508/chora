//! Typed errors: `thiserror` in the library, `anyhow` in `main`.

use std::path::PathBuf;

/// Errors shown to the user directly.
#[derive(Debug, thiserror::Error)]
pub enum ChoraError {
    #[error("config not found: {0} (run `chora init`)")]
    ConfigMissing(PathBuf),

    #[error("invalid config {path}: {msg}")]
    ConfigInvalid { path: PathBuf, msg: String },

    #[error("not a git repository: {0} (run `chora init`)")]
    NotARepo(PathBuf),

    #[error("git not found in PATH")]
    GitMissing,

    #[error("git: {0}")]
    Git(String),

    #[error("remote diverged, resolve it inside the storage dir: {0}")]
    GitDiverged(String),

    #[error("empty $EDITOR: set an editor, e.g. EDITOR=\"hx .\"")]
    EditorMissing,

    #[error("failed to launch editor: {0}")]
    EditorSpawn(String),

    #[error("editor exited: {0}")]
    EditorFailed(String),

    #[error("bad date `{0}`: expected today|tomorrow|YYYY-MM-DD|DD.MM.YYYY")]
    BadDate(String),

    #[error("bad duration `{0}`: expected 10s|45m|2h|1h30m")]
    BadDuration(String),

    #[error("pass either --date/--confirm or `show`, not both")]
    EventExclusiveAction,
}
