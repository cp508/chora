//! File writes that never leave a half-written note behind.

use std::io::Write as _;
use std::path::Path;

use anyhow::Context;

/// Write `text` to `path` atomically: temp file in the same dir, fsync,
/// rename over the target. Readers and sync tools see the old or the new
/// file, never a partial one. Creates missing parent dirs.
///
/// # Errors
///
/// Fails when the dir, temp file or rename fails; the target is untouched then.
pub fn write_atomic(path: &Path, text: &str) -> anyhow::Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("no parent dir for {}", path.display()))?;
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let name = path.file_name().map_or_else(
        || String::from("file"),
        |n| n.to_string_lossy().into_owned(),
    );
    // `.tmp` suffix: default ignore lists (git, Syncthing) skip it.
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        // Best effort: the target is untouched; a stray temp file is harmless.
        let _ = std::fs::remove_file(&tmp);
    }
    result.with_context(|| format!("failed to write {}", path.display()))
}

/// File text, or `None` when the file does not exist.
///
/// # Errors
///
/// Fails on IO errors other than "not found".
pub fn read_opt(path: &Path) -> anyhow::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("failed to read {}", path.display())),
    }
}

/// `text` plus one blank line and `block`, ending with a newline.
#[must_use]
pub fn append_block(text: &str, block: &str) -> String {
    let mut out = text.to_owned();
    if !out.is_empty() {
        if !out.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }
    out.push_str(block.trim_matches('\n'));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_and_leaves_no_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b.md");
        write_atomic(&path, "one").unwrap();
        write_atomic(&path, "two").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "two");
        assert_eq!(std::fs::read_dir(tmp.path().join("a")).unwrap().count(), 1);
    }

    #[test]
    fn failed_write_keeps_old_file() {
        let tmp = tempfile::tempdir().unwrap();
        let blocked = tmp.path().join("dir.md");
        std::fs::create_dir_all(blocked.join("inner")).unwrap();
        assert!(write_atomic(&blocked, "x").is_err());
        assert!(blocked.join("inner").is_dir());
        assert_eq!(std::fs::read_dir(tmp.path()).unwrap().count(), 1);
    }

    #[test]
    fn append_block_spacing() {
        assert_eq!(append_block("", "## A"), "## A\n");
        assert_eq!(append_block("# T", "## A\n"), "# T\n\n## A\n");
        assert_eq!(append_block("# T\n", "\n## A"), "# T\n\n## A\n");
    }
}
