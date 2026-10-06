//! Sync policy in one home: pull before editing, rebase after `open`.
//!
//! Foreground and background share these paths instead of each inventing
//! git sequencing. Offline/auth failures keep working locally with a
//! warning; the later push reports them again loudly. Diverged histories
//! always fail fast — never edit or push on a stale base silently.

use std::path::Path;

use anyhow::Context;

use crate::store::Store;
use crate::{git, warn_log};

/// Pull remote changes before editing. Skips when there is nothing to merge
/// onto (no remote, no upstream yet). Offline/auth warns and continues with
/// the local copy; diverged histories fail — never edit stale silently.
pub fn pull_before_edit(store: &Store) -> anyhow::Result<()> {
    with_sync_lock(store.root(), || match git::pull(store.root()) {
        Err(err) if git::is_transient(&err) => {
            warn_log!("pull failed ({err}); continuing with local copy");
            Ok(())
        }
        other => other,
    })
}

/// Foreground tail of `event`: commit the new section, push when it did.
pub fn commit_and_push(store: &Store, message: &str) -> anyhow::Result<()> {
    with_sync_lock(store.root(), || {
        git::add_all(store.root())?;
        if git::commit(store.root(), message)? {
            git::push(store.root())?;
        }
        Ok(())
    })
}

/// Background tail of `open` (detached, failures go to `sync.log`): commit
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
