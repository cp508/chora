//! Sync policy in one home: rebase after editing, auto-commit during it.
//!
//! Foreground and background share these paths instead of each inventing
//! git sequencing. Nothing here blocks on the network: merges happen in
//! the detached child, diverged histories land loudly in `sync.log`.

use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

use anyhow::Context;

use crate::store::Store;
use crate::{git, warn_log};

/// Run `edit` (usually: block inside the editor) while committing dirty
/// state every `every`. `Duration::ZERO` disables the timer. Commits are
/// local only — the push happens on exit via [`sync_after_open`].
/// Timer failures never break editing; they are debug-logged and skipped.
pub fn with_auto_commit_every(
    root: &Path,
    every: Duration,
    label: &str,
    edit: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if every.is_zero() {
        return edit();
    }
    let (stop, stopped) = mpsc::channel::<()>();
    let root = root.to_path_buf();
    let message = format!("auto: {label}");
    let checkpoint = std::thread::spawn(move || {
        // `Timeout` = checkpoint time. Anything else (sender dropped when
        // the editor closes) stops the timer quietly.
        while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(every) {
            let _ = with_sync_lock(&root, || {
                git::add_all(&root)?;
                let _ = git::commit(&root, &message)?;
                Ok(())
            });
        }
    });
    let result = edit();
    drop(stop);
    let _ = checkpoint.join();
    result
}

/// [`with_auto_commit_every`] with the config unit: whole minutes.
pub fn with_auto_commit(
    root: &Path,
    minutes: u64,
    label: &str,
    edit: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    with_auto_commit_every(
        root,
        Duration::from_secs(minutes.saturating_mul(60)),
        label,
        edit,
    )
}

/// Tail of `open` and `event` (detached via `setsid`, failures go to
/// `sync.log`; on spawn failure runs in the foreground instead): commit
/// the exit state, rebase it onto fresh remote work, then push.
pub fn sync_after_open(store: &Store, message: &str) -> anyhow::Result<()> {
    with_sync_lock(store.root(), || {
        git::add_all(store.root())?;
        let committed = git::commit(store.root(), message)?;
        // Virgin repo (no commits, no tracking): nothing to merge or push.
        // First edits have no upstream yet — `push` below sets it via `-u`.
        // Otherwise rebase the exit commit onto fresh remote work.
        let synced = git::has_remote(store.root()) && git::has_upstream(store.root());
        if synced {
            git::pull_rebase(store.root())?;
        }
        if committed || synced {
            git::push(store.root())?;
        }
        Ok(())
    })
}

/// Serialize add/commit/pull/push across concurrent chora processes.
/// The lock file holds the holder PID: a dead holder (killed process) is
/// taken over immediately instead of stalling for a minute. A live holder
/// waits up to 60s, then proceeds with a warning rather than blocking
/// forever. Previous-generation dir locks are cleaned up.
fn with_sync_lock(root: &Path, sync: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
    let lock = root.join(".git").join("chora-sync.lock");
    if lock.is_dir() {
        let _ = std::fs::remove_dir(&lock);
    }
    let me = std::process::id().to_string();
    let mut waited = 0;
    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(mut file) => {
                use std::io::Write;
                let _ = writeln!(file, "{me}");
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if !holder_alive(&lock) {
                    let _ = std::fs::remove_file(&lock);
                    continue;
                }
                if waited >= 60 {
                    warn_log!("sync lock is stale, proceeding anyway");
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
                waited += 1;
            }
            Err(err) => {
                return Err(err).with_context(|| format!("failed to lock {}", lock.display()));
            }
        }
    }
    let result = sync();
    let _ = std::fs::remove_file(&lock);
    result
}

/// A lock whose PID is gone (or unreadable garbage) is stale.
/// An unreadable file is treated as live — better a bounded wait than
/// deleting a lock we cannot see.
fn holder_alive(lock: &Path) -> bool {
    let Ok(text) = std::fs::read_to_string(lock) else {
        return true;
    };
    let Some(pid) = text
        .split_whitespace()
        .next()
        .and_then(|pid| pid.parse::<u32>().ok())
    else {
        return false;
    };
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn git(args: &[&str], dir: &Path) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    fn repo() -> (tempfile::TempDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().to_path_buf();
        git(&["init", "-q"], &root);
        git(&["config", "user.email", "t@t.t"], &root);
        git(&["config", "user.name", "t"], &root);
        (tmp, root)
    }

    fn log(root: &Path) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["log", "--format=%s"])
            .output()
            .unwrap();
        assert!(out.status.success());
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn zero_interval_runs_editor_inline() {
        let (_tmp, root) = repo();
        let ran = std::sync::atomic::AtomicBool::new(false);
        let out = with_auto_commit_every(root.as_path(), Duration::ZERO, "x", || {
            ran.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        });
        assert!(out.is_ok());
        assert!(ran.load(std::sync::atomic::Ordering::SeqCst));
    }

    #[test]
    fn zero_interval_propagates_editor_error() {
        let (_tmp, root) = repo();
        let out: anyhow::Result<()> =
            with_auto_commit_every(root.as_path(), Duration::ZERO, "x", || {
                anyhow::bail!("editor blew up")
            });
        assert!(out.is_err());
    }

    #[test]
    fn timer_commits_dirty_state_mid_edit() {
        let (_tmp, root) = repo();
        std::fs::write(root.join("a.md"), "# a\n").unwrap();
        let out = with_auto_commit_every(root.as_path(), Duration::from_millis(50), "test", || {
            // Longer than one interval: at least one checkpoint must fire.
            std::thread::sleep(Duration::from_millis(400));
            Ok(())
        });
        assert!(out.is_ok());
        assert!(
            log(&root).lines().any(|line| line == "auto: test"),
            "timer never checkpointed"
        );
    }

    #[test]
    fn timer_leaves_clean_tree_alone() {
        let (_tmp, root) = repo();
        let out = with_auto_commit_every(root.as_path(), Duration::from_millis(50), "test", || {
            std::thread::sleep(Duration::from_millis(200));
            Ok(())
        });
        assert!(out.is_ok());
        // No commits at all: nothing was ever dirty.
        assert!(!git_has_head(&root));
    }

    fn git_has_head(root: &Path) -> bool {
        std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--verify", "HEAD"])
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false)
    }
}
