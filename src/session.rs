//! `chora note`: opener + autocommit ticker — the key mechanism.
//!
//! The ticker is a plain `std::thread`; no async runtime is needed since
//! all the work is synchronous (opener + `git` CLI).
//!
//! Exit is instant: once the opener closes, a detached `__finalize` child
//! finishes the commit in the background (disown semantics).
//!
//! `Ctrl-C`: the process and the child opener share the terminal's
//! foreground group, so SIGINT reaches both and nothing is left hanging.
//! A final commit on `Ctrl-C` is not MVP.

use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use crate::git;

/// Run a session: open `target` (usually `notes/`) + autocommit `root`
/// every `every`. When the opener exits — a final `add + commit + push`.
pub fn run(
    root: &Path,
    target: &Path,
    every: Duration,
    opener: Option<&str>,
) -> anyhow::Result<()> {
    if every.is_zero() {
        anyhow::bail!("autocommit interval must be > 0");
    }
    let stop = Arc::new(AtomicBool::new(false));
    let ticker = {
        let stop = Arc::clone(&stop);
        let root = root.to_path_buf();
        std::thread::spawn(move || ticker_loop(&root, every, &stop))
    };

    let edit = crate::editor::open(target, root, opener);
    stop.store(true, Ordering::Relaxed);
    let _ = ticker.join();

    // Back to the terminal instantly: the final commit finishes detached.
    // If spawning fails, commit inline so no data is lost.
    let message = format!("final: {}", git::stamp_now());
    if let Err(err) = spawn_finalize(root, &message) {
        crate::debug_log!("finalize spawn failed ({err}), committing inline");
        final_tick(root)?;
    }
    edit
}

/// Fire-and-forget final commit: stdio goes to null, own process group —
/// the shell is usable the moment the opener exits.
fn spawn_finalize(root: &Path, message: &str) -> anyhow::Result<()> {
    use anyhow::Context;
    use std::process::Stdio;

    let exe = std::env::current_exe().context("failed to locate the chora binary")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--dir")
        .arg(root)
        .arg("__finalize")
        .arg("--message")
        .arg(message);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    cmd.spawn()
        .context("failed to spawn the background commit")?;
    Ok(())
}

/// Ticker: sleep in short chunks so stopping after the opener exits
/// is instant instead of taking the whole interval.
fn ticker_loop(root: &Path, every: Duration, stop: &AtomicBool) {
    let mut waited = Duration::ZERO;
    let step = Duration::from_millis(100);
    loop {
        std::thread::sleep(step);
        if stop.load(Ordering::Relaxed) {
            break;
        }
        waited += step;
        if waited >= every {
            waited = Duration::ZERO;
            let _ = autocommit(root, false);
        }
    }
}

/// One pass: `add` -> commit on changes -> push.
/// Ticks are best-effort (offline is normal), the final pass is strict.
fn autocommit(root: &Path, is_final: bool) -> anyhow::Result<()> {
    git::add_all(root)?;
    let stamp = git::stamp_now();
    let message = if is_final {
        format!("final: {stamp}")
    } else {
        format!("autocommit: {stamp}")
    };
    if git::commit(root, &message)? {
        if is_final {
            git::push(root)?;
        } else {
            let _ = git::push(root);
        }
    }
    Ok(())
}

/// Final pass with visible errors.
fn final_tick(root: &Path) -> anyhow::Result<()> {
    autocommit(root, true)
}
