//! Git sync on top of the `git` CLI (not libgit2): behavior 1-to-1 with by hand.
//!
//! Push is best-effort on ticks and guaranteed on the final pass. Without
//! a remote, push becomes a no-op instead of an error.

use std::path::Path;
use std::process::{Command, Output};

use crate::error::ChoraError;

/// Run `git -C <dir> <args>`, return the output as is.
fn git(dir: &Path, args: &[&str]) -> anyhow::Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
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

/// Stderr of a failed command, as a single line.
fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_owned()
}

/// Is the directory inside a git repo?
#[must_use]
pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--git-dir"])
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// `git init` (quiet). Idempotent.
pub fn init_repo(dir: &Path) -> anyhow::Result<()> {
    let out = git(dir, &["init", "-q"])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

/// Is there an `origin` remote?
#[must_use]
pub fn has_remote(dir: &Path) -> bool {
    git(dir, &["config", "remote.origin.url"])
        .map(|out| out.status.success() && !out.stdout.is_empty())
        .unwrap_or(false)
}

/// `git remote add origin <url>`.
pub fn add_remote(dir: &Path, url: &str) -> anyhow::Result<()> {
    let out = git(dir, &["remote", "add", "origin", url])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

/// Are `user.name` and `user.email` set (at least locally)?
#[must_use]
pub fn identity_ok(dir: &Path) -> bool {
    let value = |key: &str| {
        git(dir, &["config", key])
            .map(|out| !String::from_utf8_lossy(&out.stdout).trim().is_empty())
            .unwrap_or(false)
    };
    value("user.name") && value("user.email")
}

/// Are there changes (`git status --porcelain` is non-empty)?
pub fn is_dirty(dir: &Path) -> anyhow::Result<bool> {
    let out = git(dir, &["status", "--porcelain"])?;
    if out.status.success() {
        Ok(!out.stdout.is_empty())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

/// `git add -A`.
pub fn add_all(dir: &Path) -> anyhow::Result<()> {
    let out = git(dir, &["add", "-A"])?;
    if out.status.success() {
        Ok(())
    } else {
        Err(ChoraError::Git(stderr_of(&out)).into())
    }
}

/// Commit when there are changes. Returns `true` when a commit was created.
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

/// Push. Without a remote — a no-op. The first push sets upstream itself.
pub fn push(dir: &Path) -> anyhow::Result<()> {
    if !has_remote(dir) {
        return Ok(());
    }
    let out = git(dir, &["push", "-q"])?;
    if out.status.success() {
        return Ok(());
    }
    let upstream = git(dir, &["push", "-q", "-u", "origin", "HEAD"])?;
    if upstream.status.success() {
        Ok(())
    } else {
        Err(ChoraError::Git(stderr_of(&upstream)).into())
    }
}

/// Timestamp for commit messages: `autocommit: <ISO-timestamp>`.
#[must_use]
pub fn stamp_now() -> String {
    chrono::Local::now()
        .format("%Y-%m-%dT%H:%M:%S%z")
        .to_string()
}
