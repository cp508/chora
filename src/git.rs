//! Git sync via the `git` CLI. Pull/push without a remote are no-ops.

use std::path::Path;
use std::process::{Command, Output};

use crate::error::ChoraError;

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<Output> {
    crate::debug_log!("git -C {} {}", dir.display(), args.join(" "));
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        // English diagnostics: `classify` matches on message text.
        .env("LC_ALL", "C")
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                ChoraError::GitMissing.into()
            } else {
                anyhow::Error::new(err)
                    .context(format!("failed to launch git in {}", dir.display()))
            }
        })
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_owned()
}
/// Whether stderr reports a diverged history (rejected push, failed
/// fast-forward, rebase conflict). Matched on English (`LC_ALL=C`) stderr.
fn is_diverged(stderr: &str) -> bool {
    let text = stderr.to_lowercase();
    [
        "fast-forward",
        "[rejected]",
        "fetch first",
        "no tracking information",
        "no upstream",
        "divergent",
        "diverging",
        "unrelated histories",
        "need to merge",
        "would be overwritten",
        "conflict",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

/// Network-side failure with a classified message for remote operations.
fn remote_err(stderr: String) -> anyhow::Error {
    if is_diverged(&stderr) {
        ChoraError::GitDiverged(stderr).into()
    } else {
        ChoraError::Git(stderr).into()
    }
}

#[must_use]
pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--git-dir"])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

pub fn init_repo(dir: &Path) -> anyhow::Result<()> {
    let out = git(dir, &["init", "-q"])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

#[must_use]
pub fn has_remote(dir: &Path) -> bool {
    git(dir, &["config", "remote.origin.url"])
        .map(|out| out.status.success() && !out.stdout.is_empty())
        .unwrap_or(false)
}

#[must_use]
pub fn has_head(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--verify", "HEAD"])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[must_use]
pub fn has_upstream(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--verify", "@{u}"])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Merge remote changes on top of a local exit commit.
/// For the detached sync child: the tree already holds our commit, so
/// `--ff-only` would refuse any remote advance — rebase replays us on top
/// instead. Conflicts fail loudly into `sync.log`, never silently.
pub fn pull_rebase(dir: &Path) -> anyhow::Result<()> {
    let out = git(dir, &["pull", "--rebase", "-q"])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(remote_err(stderr_of(&out)))
    }
}

/// First run on a new device: fetch the remote and check out its default
/// branch. Must run before any layout files (`.gitignore`) are created —
/// an untracked `.gitignore` would block the checkout even with identical
/// content. No remote, no local HEAD needed, empty remote: all no-ops.
pub fn ensure_from_remote(dir: &Path) -> anyhow::Result<()> {
    if !has_remote(dir) || has_head(dir) {
        return Ok(());
    }
    let fetch = git(dir, &["fetch", "origin"])?;
    if !fetch.status.success() {
        return Err(remote_err(stderr_of(&fetch)));
    }
    let Some(branch) = default_remote_branch(dir)? else {
        return Ok(());
    };
    let out = git(dir, &["checkout", "-q", &branch])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

/// Remote default branch name: `origin/HEAD`, then `main`/`master`,
/// then the first remote branch. `None` = remote has no branches yet.
fn default_remote_branch(dir: &Path) -> anyhow::Result<Option<String>> {
    if let Ok(out) = git(dir, &["symbolic-ref", "refs/remotes/origin/HEAD"])
        && out.status.success()
    {
        let target = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if let Some(name) = target.strip_prefix("refs/remotes/origin/") {
            return Ok(Some(name.to_owned()));
        }
    }
    for branch in ["main", "master"] {
        let remote_ref = format!("origin/{branch}");
        let out = git(dir, &["rev-parse", "--verify", &remote_ref])?;
        if out.status.success() {
            return Ok(Some(branch.to_owned()));
        }
    }
    let out = git(
        dir,
        &[
            "for-each-ref",
            "--format=%(refname)",
            "refs/remotes/origin/",
        ],
    )?;
    if !out.status.success() {
        return Err(ChoraError::Git(stderr_of(&out)).into());
    }
    let first = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.ends_with("/HEAD"))
        .filter_map(|line| line.strip_prefix("refs/remotes/origin/"))
        .next()
        .map(str::to_owned);
    Ok(first)
}

pub fn add_remote(dir: &Path, url: &str) -> anyhow::Result<()> {
    let out = git(dir, &["remote", "add", "origin", url])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

pub fn is_dirty(dir: &Path) -> anyhow::Result<bool> {
    let out = git(dir, &["status", "--porcelain"])?;
    if out.status.success() {
        Ok(!out.stdout.is_empty())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

pub fn add_all(dir: &Path) -> anyhow::Result<()> {
    let out = git(dir, &["add", "-A"])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

/// Commits when dirty; returns whether it did.
pub fn commit(dir: &Path, message: &str) -> anyhow::Result<bool> {
    if !is_dirty(dir)? {
        return Ok(false);
    }
    let out = git(dir, &["commit", "-q", "-m", message])?;
    if out.status.success() {
        Ok(true)
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

/// No remote = no-op. First push sets upstream.
pub fn push(dir: &Path) -> anyhow::Result<()> {
    if !has_remote(dir) {
        return Ok(());
    }
    let out = git(dir, &["push", "-q", "-u", "origin", "HEAD"])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(remote_err(stderr_of(&out)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diverged_spots_rejected_push() {
        assert!(is_diverged(" ! [rejected] HEAD -> main (fetch first)"));
    }

    #[test]
    fn diverged_spots_failed_fast_forward() {
        for stderr in [
            "There is no tracking information for the current branch.",
            "hint: Diverging branches can't be fast-forwarded",
            "fatal: Not possible to fast-forward, aborting.",
            "fatal: Need to specify how to reconcile divergent branches.",
            "error: Your local changes would be overwritten by merge.",
            "CONFLICT (content): Merge conflict in events/2026-11-20.md",
        ] {
            assert!(is_diverged(stderr), "{stderr}");
        }
    }

    #[test]
    fn diverged_leaves_other_failures_alone() {
        for stderr in [
            "ssh: Could not resolve hostname gitlab.com: Temporary failure in name resolution",
            "git@gitlab.com: Permission denied (publickey).",
            "'/tmp/x/remote.git' does not appear to be a git repository",
            "some other failure",
        ] {
            assert!(!is_diverged(stderr), "{stderr}");
        }
    }
}
