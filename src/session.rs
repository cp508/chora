//! `chora note`: opener + autocommit ticker — the key mechanism.
//!
//! The ticker is a plain `std::thread`; no async runtime is needed since
//! all the work is synchronous (opener + `git` CLI).
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

    // Data matters more than the exit code: final commit even if the opener failed.
    final_tick(root)?;
    edit
}

/// Ticker: sleep in one-second chunks so stopping after the opener exits
/// is fast instead of taking the whole interval.
fn ticker_loop(root: &Path, every: Duration, stop: &AtomicBool) {
    let mut waited = Duration::ZERO;
    let step = Duration::from_secs(1);
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
