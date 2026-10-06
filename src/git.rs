//! Git sync via the `git` CLI. Push without a remote is a no-op.

use std::path::Path;
use std::process::{Command, Output};

use crate::error::ChoraError;

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<Output> {
    crate::debug_log!("git -C {} {}", dir.display(), args.join(" "));
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

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).trim().to_owned()
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
