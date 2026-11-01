//! Typed errors: `thiserror` in the library, `anyhow` in `main`.

use std::path::PathBuf;

/// Errors shown to the user directly.
#[derive(Debug, thiserror::Error)]
pub enum ChoraError {
    /// Config file does not exist; the user must run `chora init`.
    #[error("config not found: {0} (run `chora init`)")]
    ConfigMissing(
        /// Config file path that does not exist.
        PathBuf,
    ),

    /// Config file exists but has invalid TOML or values.
    #[error("invalid config {path}: {msg}")]
    ConfigInvalid {
        /// Config file path that failed to parse.
        path: PathBuf,
        /// TOML or validation error text.
        msg: String,
    },

    /// Storage root is not a git checkout.
    #[error("not a git repository: {0} (run `chora init`)")]
    NotARepo(
        /// Storage root that is not a git checkout.
        PathBuf,
    ),

    /// The `git` binary is missing from `PATH`.
    #[error("git not found in PATH")]
    GitMissing,

    /// `git` ran but reported a failure (stderr text).
    #[error("git: {0}")]
    Git(
        /// Classified stderr of the failed `git` call.
        String,
    ),

    /// Histories diverged; the user must resolve it by hand.
    #[error("remote diverged, resolve it inside the storage dir: {0}")]
    GitDiverged(
        /// Stderr describing the divergence.
        String,
    ),

    /// Neither the config nor `$EDITOR` names an opener.
    #[error("empty $EDITOR: set an editor, e.g. EDITOR=\"hx .\"")]
    EditorMissing,

    /// The editor command failed to parse or to launch.
    #[cfg(feature = "cli")]
    #[error("failed to parse editor command: {0}")]
    EditorParse(#[from] shell_words::ParseError),

    /// The editor binary failed to spawn; `source` keeps the `io::ErrorKind`.
    #[error("failed to launch editor `{program}`: {source}")]
    EditorSpawn {
        /// Editor binary that failed to spawn.
        program: String,
        /// Spawn failure (`NotFound`, `PermissionDenied`, …).
        #[source]
        source: std::io::Error,
    },

    /// The editor exited non-zero (exit code or signal name).
    #[error("editor exited: {0}")]
    EditorFailed(
        /// Exit code or `"signal"` when killed by one.
        String,
    ),

    /// Unparseable `--date` value.
    #[error("bad date `{0}`: expected today|tomorrow|YYYY-MM-DD|DD.MM.YYYY")]
    BadDate(
        /// Raw `--date` value the user passed.
        String,
    ),

    /// Unparseable `--duration` value.
    #[error("bad duration `{0}`: expected 30m|2h20m|2h 20m|2:00")]
    BadDuration(
        /// Raw `--duration` value the user passed.
        String,
    ),

    /// Unparseable `--time` value.
    #[error("bad time `{0}`: expected HH:MM")]
    BadTime(
        /// Raw `--time` value the user passed.
        String,
    ),

    /// Unsafe, absolute, missing, or non-`.md` note path.
    #[error("bad note path `{0}`")]
    NoteBadPath(
        /// Rejected user-supplied path.
        String,
    ),

    /// `notes/<dir>` does not exist on disk.
    #[error("no such notes dir `{0}`")]
    NoteNoDir(
        /// Unknown dirname the user asked for.
        String,
    ),

    /// `--under` header not found in the note file.
    #[error("header not found: `{0}`")]
    NoteNoHeader(
        /// Header path that matched nothing.
        String,
    ),

    /// Empty stdin piped into `note add`/`note override`.
    #[error("empty body: pipe markdown via stdin, e.g. printf '# T\\n' | chora note ...")]
    NoteEmptyBody,

    /// Write target needs a higher clearance than `--max-access`.
    #[error("refusing to write `{path}`: needs {need} clearance (max-access {max})")]
    NoteDenied {
        /// Write target the user was refused.
        path: String,
        /// Clearance the target needs.
        need: &'static str,
        /// Clearance the invocation has.
        max: &'static str,
    },

    /// Creation flags and `show` were mixed on one `event` call.
    #[error(
        "pass either creation flags (--date/--confirm/--title/--time/--duration/--body) or `show`, not both"
    )]
    EventExclusiveAction,

    /// Non-interactive `event` misses `--title`, `--time` or `--duration`.
    #[error(
        "event needs --title, --time and --duration (optional --date defaults to today, --body for context)"
    )]
    EventNeedsFields,

    /// The file changed on disk since it was read (another device synced).
    #[error("`{0}` changed on disk, reload it")]
    Changed(
        /// What changed: a note path or an event title.
        String,
    ),

    /// A checkbox toggle aimed at a line that is not a task.
    #[error("line {0} is not a task")]
    NotATask(
        /// 1-based line number.
        usize,
    ),

    /// A path that is not a Syncthing `.sync-conflict-` copy.
    #[error("`{0}` is not a sync conflict copy")]
    NotAConflict(
        /// Rejected path.
        String,
    ),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn editor_spawn_keeps_io_source() {
        let err = ChoraError::EditorSpawn {
            program: String::from("hx"),
            source: io::Error::new(io::ErrorKind::NotFound, "no such binary"),
        };
        let text = err.to_string();
        assert!(text.contains("hx"), "{text}");
        assert!(text.contains("failed to launch editor"), "{text}");
        let source = std::error::Error::source(&err).expect("source must exist");
        assert_eq!(
            source.downcast_ref::<io::Error>().unwrap().kind(),
            io::ErrorKind::NotFound
        );
    }
}
