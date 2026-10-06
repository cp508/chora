//! Opener launching: config value > `$EDITOR`, split like a shell line.

use std::path::Path;
use std::process::Command;

use crate::error::ChoraError;

/// Open `target` and wait for exit. Empty config and `$EDITOR` is an error.
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
