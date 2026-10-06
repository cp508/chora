//! Typed chora errors.
//!
//! Rule: `thiserror` for the library, `anyhow` at the `main` boundary
//! (`err-thiserror-lib`, `err-anyhow-app`).

use std::path::PathBuf;

/// Errors that chora shows to the user directly.
#[derive(Debug, thiserror::Error)]
pub enum ChoraError {
    /// Config file is missing — fixed by `chora init`.
    #[error("config not found: {0} (run `chora init`)")]
    ConfigMissing(PathBuf),

    /// Config exists but does not parse or is invalid.
    #[error("invalid config {path}: {msg}")]
    ConfigInvalid {
        /// Path to the config file.
        path: PathBuf,
        /// What exactly is wrong.
        msg: String,
    },

    /// The command needs a git repo, but there is none.
    #[error("not a git repository: {0} (run `chora init`)")]
    NotARepo(PathBuf),

    /// The `git` binary is not on PATH.
    #[error("git not found in PATH")]
    GitMissing,

    /// A git command failed (carries its stderr).
    #[error("git: {0}")]
    Git(String),

    /// `$EDITOR` is empty.
    #[error("empty $EDITOR: set an editor, e.g. EDITOR=\"hx .\"")]
    EditorMissing,

    /// Failed to launch the editor.
    #[error("failed to launch editor: {0}")]
    EditorSpawn(String),

    /// The editor exited unsuccessfully.
    #[error("editor exited: {0}")]
    EditorFailed(String),

    /// Bad date argument.
    #[error("bad date `{0}`: expected today|tomorrow|YYYY-MM-DD")]
    BadDate(String),

    /// Bad duration format.
    #[error("bad duration `{0}`: expected 10s|45m|2h|1h30m")]
    BadDuration(String),

    /// `event` was called with neither `--date` nor `--tui`.
    #[error("need --date <date> or --tui")]
    EventNeedsTarget,

    /// `event` was called with both `--date` and `--tui`.
    #[error("pass either --date or --tui, not both")]
    EventExclusiveTarget,
}
