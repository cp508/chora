//! Opener launching: config `editor` > `$EDITOR`, split like a shell line.
//!
//! We do not write our own split (`shell-words`): the opener may hold
//! `code --wait` or `yazi`. `$VISUAL` is ignored on purpose — one source,
//! less ambiguity. The opener runs with cwd = storage root.

use std::path::Path;
use std::process::Command;

use crate::error::ChoraError;

/// Open `target` in the opener and wait for it to exit.
///
/// `configured` is the `editor` value from the config; an empty/absent
/// value falls back to `$EDITOR`. Empty everywhere is an explicit error.
pub fn open(target: &Path, cwd: &Path, configured: Option<&str>) -> anyhow::Result<()> {
    let raw = match configured {
        Some(cmd) if !cmd.trim().is_empty() => cmd.to_owned(),
        _ => std::env::var("EDITOR").unwrap_or_default(),
    };
    let mut parts =
        shell_words::split(&raw).map_err(|err| ChoraError::EditorSpawn(err.to_string()))?;
    if parts.is_empty() {
        return Err(ChoraError::EditorMissing.into());
    }
    let program = parts.remove(0);
    crate::debug_log!("opener: {program} {}", target.display());
    let status = Command::new(&program)
        .args(&parts)
        .arg(target)
        .current_dir(cwd)
        .status()
        .map_err(|err| ChoraError::EditorSpawn(format!("{program}: {err}")))?;
    if status.success() {
        Ok(())
    } else {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| String::from("signal"));
        Err(ChoraError::EditorFailed(code).into())
    }
}
