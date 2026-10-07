//! End-to-end flows: drive the built binary with an isolated HOME.
//!
//! Each test gets its own temp HOME, so the real `~/.config` and `~/chora`
//! are never touched. Git identity comes from per-command env vars.

use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_chora"))
}

struct Env {
    _tmp: tempfile::TempDir,
    home: PathBuf,
}

fn setup() -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    Env { _tmp: tmp, home }
}

/// Isolated HOME plus a pinned default branch, so bare remotes, clones
/// and fresh repos always agree on `main` regardless of host git config.
fn git_env(cmd: &mut Command, env: &Env) {
    cmd.env("HOME", &env.home);
    cmd.env_remove("XDG_CONFIG_HOME");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd.env("GIT_CONFIG_COUNT", "1");
    cmd.env("GIT_CONFIG_KEY_0", "init.defaultBranch");
    cmd.env("GIT_CONFIG_VALUE_0", "main");
}

/// Base command: isolated HOME, no XDG leakage, deterministic git identity.
fn cmd(env: &Env) -> Command {
    let mut cmd = Command::new(bin());
    git_env(&mut cmd, env);
    cmd.env("GIT_AUTHOR_NAME", "tester");
    cmd.env("GIT_AUTHOR_EMAIL", "tester@example.com");
    cmd.env("GIT_COMMITTER_NAME", "tester");
    cmd.env("GIT_COMMITTER_EMAIL", "tester@example.com");
    cmd
}

/// Write an `sh` script and return an EDITOR value that runs it.
/// No chmod needed: the editor is invoked as `sh <script> <target>`.
fn editor_script(env: &Env, name: &str, body: &str) -> String {
    let path = env.home.join(name);
    std::fs::write(&path, body).unwrap();
    format!("sh {}", path.display())
}

fn storage(env: &Env) -> PathBuf {
    env.home.join("chora")
}

#[test]
fn init_creates_only_config() {
    let env = setup();
    let out = cmd(&env).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let cfg = env.home.join(".config").join("chora").join("config.toml");
    assert!(cfg.exists());
    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("chora_dir"));
    assert!(text.contains("editor = "));
    assert!(text.contains("open_dirs"));
    assert!(text.contains("remote_url"));

    // The storage appears on first use, not at init.
    assert!(!storage(&env).exists());

    // Second run is a quiet no-op.
    let again = cmd(&env).arg("init").output().unwrap();
    assert!(again.status.success());
}

#[test]
fn first_event_bootstraps_storage() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());
    let root = storage(&env);
    assert!(!root.exists());

    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("events").is_dir());
    assert!(root.join("notes").is_dir());
    assert!(root.join(".git").is_dir());
    assert!(root.join(".gitignore").exists());
    // The commit lands from the detached background child.
    poll_log(&root, "event:");
}

#[test]
fn event_then_show_roundtrip() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let editor = editor_script(
        &env,
        "append.sh",
        "#!/bin/sh\ncat >> \"$1\" <<'EOF'\n## Standup\nTime: 10:00\nDuration: 1h\n\nAgenda.\nEOF\n",
    );

    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let day = std::fs::read_to_string(root.join("events").join("2026-11-20.md")).unwrap();
    assert!(day.starts_with("# 2026-11-20\n"));
    assert!(day.contains("## Standup"));

    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("Standup"));
    assert!(text.contains("10:00"));

    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "2026-11-20",
            "--json",
        ])
        .output()
        .unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(parsed[0]["title"], "Standup");
    assert_eq!(parsed[0]["date"], "2026-11-20");
    assert_eq!(parsed[0]["time"], "10:00");
    assert_eq!(parsed[0]["duration"], "1h");
    assert_eq!(parsed[0]["context"], "Agenda.");
}

#[test]
fn event_stub_appends_template_and_commits() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-21",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let day = std::fs::read_to_string(root.join("events").join("2026-11-21.md")).unwrap();
    assert!(day.contains("Time:"));
    assert!(day.contains("Duration:"));
    // The commit lands from the detached background child.
    poll_log(&root, "event:");
}

#[test]
fn show_missing_day_is_empty() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let root = storage(&env);
    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "2030-01-01",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("no events"));

    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "2030-01-01",
            "--json",
        ])
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "[]");
    // Reading creates nothing.
    assert!(!root.exists());
}

#[test]
fn bad_date_fails() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let root = storage(&env);
    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "nope",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn invalid_subcommand_shows_tree() {
    let env = setup();
    let out = cmd(&env).arg("bogus").output().unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    // Same tree as bare `chora`, then the clap error.
    for token in ["Commands:", "• init", "• open", "• event", "Options:"] {
        assert!(stderr.contains(token), "stderr misses `{token}`");
    }
    assert!(stderr.contains("bogus"));
}

#[test]
fn missing_config_points_to_init() {
    let env = setup();
    let root = storage(&env);
    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "today",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("chora init"));
}

#[test]
fn open_uses_config_dirs_opener() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    // The dirs-opener comes from the config, $EDITOR is not even set.
    let script = env.home.join("opener.sh");
    std::fs::write(&script, "#!/bin/sh\nprintf '# hi\\n' > \"$1/hi.md\"\n").unwrap();
    let cfg = env.home.join(".config").join("chora").join("config.toml");
    let text = format!(
        "[main]\nchora_dir = \"~/chora\"\neditor = \"\"\nopen_dirs = \"sh {}\"\nremote_url = \"\"\n",
        script.display()
    );
    std::fs::write(&cfg, text).unwrap();

    let root = storage(&env);
    let out = cmd(&env)
        .env_remove("EDITOR")
        .args(["--dir", &root.to_string_lossy(), "open", "note"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("notes").join("hi.md").exists());
}

#[test]
fn init_migrates_old_config_with_new_marker() {
    let env = setup();
    // A config from an older program version: no editor/remote_url yet.
    let cfg = env.home.join(".config").join("chora").join("config.toml");
    std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg,
        "# my tweaks\n[main]\nchora_dir = \"~/chora\"\nauto_commit = 7\n",
    )
    .unwrap();

    let out = cmd(&env).arg("init").output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("new config fields added"));

    let text = std::fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("# my tweaks"));
    assert!(text.contains("auto_commit = 7"));
    assert!(text.contains("# NEW\n"));
    assert!(text.contains("editor = \"\""));
    assert!(text.contains("open_dirs = \"\""));
    assert!(text.contains("manage_old_event = \"nothing\""));
    assert!(text.contains("remote_url = \"\""));

    // Second init stays quiet.
    let out = cmd(&env).arg("init").output().unwrap();
    assert!(out.status.success());
    assert!(!String::from_utf8_lossy(&out.stdout).contains("new config fields added"));
}

/// The `open` commit lands from the detached background child,
/// so poll instead of asserting it right away. A missing HEAD (the child
/// has not committed yet) counts as "not yet", not as failure.
fn poll_log(root: &Path, prefix: &str) {
    for _ in 0..200 {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["log", "--format=%s"])
            .output()
            .unwrap();
        if out.status.success()
            && String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.starts_with(prefix))
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("background sync never committed '{prefix}'");
}

#[test]
fn open_notes_commits_on_exit() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let editor = editor_script(
        &env,
        "browse.sh",
        "#!/bin/sh\nprintf '# picked\\n' > \"$1/picked.md\"\n",
    );
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args(["--dir", &root.to_string_lossy(), "open", "note"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("notes").join("picked.md").exists());
    poll_log(&root, "open: note");
}

#[test]
fn open_syncs_in_background_after_exit() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let editor = editor_script(
        &env,
        "browse.sh",
        "#!/bin/sh\nprintf '# picked\\n' > \"$1/picked.md\"\n",
    );
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .env_remove("XDG_DATA_HOME")
        .args(["--dir", &root.to_string_lossy(), "open", "note"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("syncing in background"),
        "foreground must detach, not block on push"
    );
    poll_log(&root, "open: note");
    assert!(root.join("notes").join("picked.md").exists());
}

#[test]
fn open_events_opens_the_events_dir() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let editor = editor_script(
        &env,
        "browse.sh",
        "#!/bin/sh\nprintf '# seen\\n' > \"$1/seen.md\"\n",
    );
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args(["--dir", &root.to_string_lossy(), "open", "event"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("events").join("seen.md").exists());
}

#[test]
fn open_runs_dirs_opener_inside_target_dir() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    // `sh -c '... $(fzf)'` openers scan cwd, so cwd must be the target dir,
    // otherwise both notes/ and events/ show up at once.
    let editor = editor_script(&env, "pwd.sh", "#!/bin/sh\npwd > \"$1/pwd.txt\"\n");
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args(["--dir", &root.to_string_lossy(), "open", "note"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let seen = std::fs::read_to_string(root.join("notes").join("pwd.txt")).unwrap();
    assert_eq!(
        Path::new(seen.trim()),
        root.join("notes"),
        "dirs-opener cwd must be the target dir"
    );
}

#[test]
fn event_sweeps_old_days_per_policy() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let cfg = env.home.join(".config").join("chora").join("config.toml");
    let text = "[main]\nchora_dir = \"~/chora\"\neditor = \"\"\nopen_dirs = \"\"\nmanage_old_event = \"delete\"\nremote_url = \"\"\n";
    std::fs::write(&cfg, text).unwrap();

    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let root = storage(&env);
    std::fs::create_dir_all(root.join("events")).unwrap();
    std::fs::write(root.join("events").join("2020-01-01.md"), "# 2020-01-01\n").unwrap();

    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2099-01-01",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!root.join("events").join("2020-01-01.md").exists());
    assert!(root.join("events").join("2099-01-01.md").exists());
}

#[test]
fn open_rejects_unknown_target() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let root = storage(&env);
    let out = cmd(&env)
        .args(["--dir", &root.to_string_lossy(), "open", "bogus"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

/// `git` with deterministic identity in the isolated HOME, so branch
/// defaults match the ones chora itself sees under `cmd`.
fn git_ok(env: &Env, dir: &Path, args: &[&str]) {
    let mut cmd = Command::new("git");
    git_env(&mut cmd, env);
    let out = cmd
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "tester")
        .env("GIT_AUTHOR_EMAIL", "tester@example.com")
        .env("GIT_COMMITTER_NAME", "tester")
        .env("GIT_COMMITTER_EMAIL", "tester@example.com")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Bare remote inside the isolated HOME: same branch defaults as chora.
fn init_bare_remote(env: &Env, name: &str) -> PathBuf {
    let remote = env.home.join(name);
    let mut cmd = Command::new("git");
    git_env(&mut cmd, env);
    let out = cmd
        .args(["init", "--bare", "-q"])
        .arg(&remote)
        .output()
        .unwrap();
    assert!(out.status.success());
    remote
}

fn clone_remote(env: &Env, remote: &Path, dest: &Path) {
    let mut cmd = Command::new("git");
    git_env(&mut cmd, env);
    let out = cmd
        .args(["clone", "-q"])
        .arg(remote)
        .arg(dest)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git clone failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn write_config_with_remote(env: &Env, remote: &Path) {
    let cfg = env.home.join(".config").join("chora").join("config.toml");
    let text = format!(
        "[main]\nchora_dir = \"~/chora\"\neditor = \"\"\nopen_dirs = \"\"\nmanage_old_event = \"nothing\"\nremote_url = \"{}\"\n",
        remote.display()
    );
    std::fs::write(&cfg, text).unwrap();
}

#[test]
fn event_merges_remote_in_background() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let remote = init_bare_remote(&env, "remote.git");
    write_config_with_remote(&env, &remote);

    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Shared base must reach the remote before the other device clones.
    poll_remote(&remote, "event:");

    // Another device pushes while this one is idle.
    let other = env.home.join("other");
    clone_remote(&env, &remote, &other);
    std::fs::create_dir_all(other.join("notes")).unwrap();
    std::fs::write(other.join("notes").join("from-remote.md"), "# remote\n").unwrap();
    git_ok(&env, &other, &["add", "-A"]);
    git_ok(&env, &other, &["commit", "-qm", "remote note"]);
    git_ok(&env, &other, &["push", "-q", "-u", "origin", "HEAD"]);

    // The next edit merges the remote note in the background child.
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-21",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    poll_file(&root.join("notes").join("from-remote.md"));
    assert!(root.join("events").join("2026-11-20.md").exists());
    assert!(root.join("events").join("2026-11-21.md").exists());
    poll_log(&root, "event:");
}

/// Poll until the bare remote contains a commit with `prefix`.
/// Event/open pushes land from the background child; the next device
/// must clone only after they arrive, or it forks unrelated history.
fn poll_remote(remote: &Path, prefix: &str) {
    for _ in 0..200 {
        let out = Command::new("git")
            .arg("--git-dir")
            .arg(remote)
            .args(["log", "--format=%s"])
            .output()
            .unwrap();
        if out.status.success()
            && String::from_utf8_lossy(&out.stdout)
                .lines()
                .any(|line| line.starts_with(prefix))
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("remote never received '{prefix}': {}", remote.display());
}

/// Poll for a file the detached sync child is about to write.
fn poll_file(path: &Path) {
    for _ in 0..200 {
        if path.exists() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("background sync never wrote {}", path.display());
}

#[test]
fn open_pulls_remote_in_background_on_exit() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let remote = init_bare_remote(&env, "remote.git");
    write_config_with_remote(&env, &remote);

    // First open seeds the repo; the commit lands from the background child.
    let editor = editor_script(
        &env,
        "browse.sh",
        "#!/bin/sh\nprintf '# local\\n' > \"$1/local.md\"\n",
    );
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args(["--dir", &root.to_string_lossy(), "open", "note"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    poll_log(&root, "open: note");
    poll_remote(&remote, "open: note");

    // Another device pushes while this one is idle.
    let other = env.home.join("other");
    clone_remote(&env, &remote, &other);
    std::fs::create_dir_all(other.join("notes")).unwrap();
    std::fs::write(other.join("notes").join("from-remote.md"), "# remote\n").unwrap();
    git_ok(&env, &other, &["add", "-A"]);
    git_ok(&env, &other, &["commit", "-qm", "remote note"]);
    git_ok(&env, &other, &["push", "-q", "-u", "origin", "HEAD"]);

    // The editor opens without waiting for the network; the remote note
    // is merged by the detached sync child, so poll for it.
    let editor2 = editor_script(
        &env,
        "browse2.sh",
        "#!/bin/sh\nprintf '# local2\\n' > \"$1/local2.md\"\n",
    );
    let out = cmd(&env)
        .env("EDITOR", &editor2)
        .args(["--dir", &root.to_string_lossy(), "open", "note"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    poll_file(&root.join("notes").join("from-remote.md"));
    assert!(root.join("notes").join("local2.md").exists());
    poll_log(&root, "open: note");
}

#[test]
fn first_push_sets_upstream() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let remote = init_bare_remote(&env, "remote.git");
    write_config_with_remote(&env, &remote);

    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The first push establishes tracking: plain `git pull` works from here.
    // The upstream is set by the detached background push: poll for it.
    poll_upstream(&root);
}

/// Poll until `@{u}` resolves: the background push sets tracking.
fn poll_upstream(root: &Path) {
    for _ in 0..200 {
        let out = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--verify", "@{u}"])
            .output()
            .unwrap();
        if out.status.success() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("background push never set upstream in {}", root.display());
}

/// Poll until the sync log contains `needle`.
fn poll_sync_log(env: &Env, needle: &str) {
    let log = env
        .home
        .join(".local")
        .join("share")
        .join("chora")
        .join("sync.log");
    for _ in 0..200 {
        if std::fs::read_to_string(&log)
            .unwrap_or_default()
            .contains(needle)
        {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("sync log never contained '{needle}'");
}

#[test]
fn event_without_upstream_skips_pull_and_reports_push_in_log() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    // Remote with independent history.
    let remote = init_bare_remote(&env, "remote.git");
    let seed = env.home.join("seed");
    clone_remote(&env, &remote, &seed);
    std::fs::create_dir_all(seed.join("notes")).unwrap();
    std::fs::write(seed.join("notes").join("seed.md"), "# seed\n").unwrap();
    git_ok(&env, &seed, &["add", "-A"]);
    git_ok(&env, &seed, &["commit", "-qm", "seed"]);
    git_ok(&env, &seed, &["push", "-q", "-u", "origin", "HEAD"]);

    // Local repo with its own root commit, remote attached, no tracking:
    // e.g. the very first push never went out (was offline).
    let root = storage(&env);
    std::fs::create_dir_all(&root).unwrap();
    let mut init = Command::new("git");
    git_env(&mut init, &env);
    let out = init
        .arg("-C")
        .arg(&root)
        .args(["init", "-q"])
        .output()
        .unwrap();
    assert!(out.status.success());
    std::fs::create_dir_all(root.join("notes")).unwrap();
    std::fs::write(root.join("notes").join("mine.md"), "# mine\n").unwrap();
    git_ok(&env, &root, &["add", "-A"]);
    git_ok(&env, &root, &["commit", "-qm", "mine"]);
    git_ok(
        &env,
        &root,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    write_config_with_remote(&env, &remote);

    // Must not stick at pull with "no tracking information": the pull is
    // skipped, the edit is committed in the background child, and the
    // diverged push lands loudly in the sync log — the foreground exits 0.
    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .env_remove("XDG_DATA_HOME")
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !String::from_utf8_lossy(&out.stderr).contains("no tracking information"),
        "stuck at pull instead of push: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    poll_log(&root, "event:");
    poll_sync_log(&env, "diverged");
}

#[test]
fn event_refuses_diverged_day_loudly() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let remote = init_bare_remote(&env, "remote.git");
    write_config_with_remote(&env, &remote);

    // Shared base.
    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Shared base must reach the remote before the other device clones.
    poll_remote(&remote, "event:");

    // Another device rewrites the same day file and pushes.
    let other = env.home.join("other");
    clone_remote(&env, &remote, &other);
    let day = other.join("events").join("2026-11-20.md");
    let mut text = std::fs::read_to_string(&day).unwrap();
    text.push_str("\n## Remote\nTime: 12:00\nDuration: 1h\n");
    std::fs::write(&day, text).unwrap();
    git_ok(&env, &other, &["add", "-A"]);
    git_ok(&env, &other, &["commit", "-qm", "remote edit"]);
    git_ok(&env, &other, &["push", "-q"]);

    // Meanwhile this device edits the same day file without pushing
    // (e.g. it was offline): histories genuinely diverge now.
    let local_day = root.join("events").join("2026-11-20.md");
    let mut text = std::fs::read_to_string(&local_day).unwrap();
    text.push_str("\n## Local\nTime: 10:00\nDuration: 1h\n");
    std::fs::write(&local_day, text).unwrap();
    git_ok(&env, &root, &["add", "-A"]);
    git_ok(&env, &root, &["commit", "-qm", "local offline work"]);

    // Local base is stale: the foreground still exits 0 (nothing blocks
    // the editor), but the background rebase refuses and names it loudly.
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .env_remove("XDG_DATA_HOME")
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    poll_sync_log(&env, "diverged");
}

#[test]
fn event_fails_fast_on_bad_remote() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let remote = env.home.join("nope.git");
    write_config_with_remote(&env, &remote);

    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("git repository"),
        "misconfiguration must fail fast: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // Nothing half-created: the day file never appears.
    assert!(!root.join("events").join("2026-11-20.md").exists());
}

#[test]
fn open_without_changes_is_quiet() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    // No remote at all: pure local run.
    let editor = editor_script(&env, "empty.sh", "#!/bin/sh\nexit 0\n");
    let root = storage(&env);
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .env_remove("XDG_DATA_HOME")
        .args(["--dir", &root.to_string_lossy(), "open", "note"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    poll_log(&root, "open: note");
    let log = env
        .home
        .join(".local")
        .join("share")
        .join("chora")
        .join("sync.log");
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    assert!(
        !text.contains("Error"),
        "browse-only run must sync cleanly: {text}"
    );
}

#[test]
fn first_run_checks_out_remote_on_new_device() {
    // Seed device: its own HOME, its own bare remote.
    let seed_env = setup();
    let remote = init_bare_remote(&seed_env, "remote.git");
    let seed = seed_env.home.join("seed");
    clone_remote(&seed_env, &remote, &seed);
    std::fs::create_dir_all(seed.join("notes")).unwrap();
    std::fs::write(seed.join("notes").join("seed.md"), "# seed\n").unwrap();
    git_ok(&seed_env, &seed, &["add", "-A"]);
    git_ok(&seed_env, &seed, &["commit", "-qm", "seed"]);
    git_ok(&seed_env, &seed, &["push", "-q", "-u", "origin", "HEAD"]);

    // Fresh HOME, same remote: first use must fetch the seed before editing.
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());
    write_config_with_remote(&env, &remote);

    let editor = editor_script(&env, "noop.sh", "#!/bin/sh\nexit 0\n");
    let root = storage(&env);
    assert!(!root.exists());
    let out = cmd(&env)
        .env("EDITOR", &editor)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "2026-11-20",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("notes").join("seed.md").exists());
    assert!(root.join("events").join("2026-11-20.md").exists());
}

#[test]
fn completion_prints_shell_script() {
    let env = setup();
    // Needs no config or storage. `completion` itself is hand-parsed
    // (invisible to the generator), so only schema values are asserted.
    for (shell, marker, values) in [
        ("bash", "_chora()", vec!["note event"]),
        ("fish", "complete", vec!["-a \"note\""]),
    ] {
        for name in ["completion", "completions"] {
            let out = cmd(&env).args([name, shell]).output().unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            assert!(text.contains("chora"));
            assert!(text.contains(marker), "{shell} script misses {marker}");
            for value in &values {
                assert!(text.contains(value), "{shell} script misses {value}");
            }
            // The internal background child must never leak into completions.
            assert!(
                !text.contains("sync-after-open"),
                "{shell} script leaks the internal command"
            );
        }
    }
}

#[test]
fn open_rejects_old_plurals() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let root = storage(&env);
    for target in ["notes", "events"] {
        let out = cmd(&env)
            .args(["--dir", &root.to_string_lossy(), "open", target])
            .output()
            .unwrap();
        assert!(!out.status.success(), "open {target} must not parse");
    }
}

#[test]
fn event_rejects_mixed_show_flags() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let root = storage(&env);
    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "--date",
            "today",
            "show",
        ])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn event_show_lists_all_days() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    let root = storage(&env);
    std::fs::create_dir_all(root.join("events")).unwrap();
    std::fs::write(
        root.join("events").join("2026-11-20.md"),
        "# 2026-11-20\n\n## Standup\nTime: 10:00\nDuration: 1h\n\nAgenda.\n",
    )
    .unwrap();
    std::fs::write(
        root.join("events").join("2026-11-21.md"),
        "# 2026-11-21\n\n## Demo\nTime: 15:00\nDuration: 30m\n\nShip it.\n",
    )
    .unwrap();

    let out = cmd(&env)
        .args(["--dir", &root.to_string_lossy(), "event", "show"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("Standup"));
    assert!(text.contains("Demo"));

    let out = cmd(&env)
        .args(["--dir", &root.to_string_lossy(), "event", "show", "--json"])
        .output()
        .unwrap();
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 2);
}

#[test]
fn event_show_date_merges_repeatables() {
    let env = setup();
    assert!(cmd(&env).arg("init").output().unwrap().status.success());

    // 2026-11-23 is a Monday.
    let root = storage(&env);
    std::fs::create_dir_all(root.join("events").join("repeatable")).unwrap();
    std::fs::write(
        root.join("events").join("repeatable").join("monday.md"),
        "# Monday\n\n## Teeth\nTime: 09:00\nDuration: 10m\n\nBrush.\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("events")).unwrap();
    std::fs::write(
        root.join("events").join("2026-11-23.md"),
        "# 2026-11-23\n\n## Standup\nTime: 10:00\nDuration: 1h\n\nAgenda.\n",
    )
    .unwrap();

    // Dotted date format works too.
    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "23.11.2026",
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(text.contains("Standup"));
    assert!(text.contains("Teeth"));

    // Tuesday has no repeatable.
    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "2026-11-24",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    assert_eq!(parsed.as_array().unwrap().len(), 0);

    // Monday JSON carries the queried date on the repeatable.
    let out = cmd(&env)
        .args([
            "--dir",
            &root.to_string_lossy(),
            "event",
            "show",
            "--date",
            "2026-11-23",
            "--json",
        ])
        .output()
        .unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let items = parsed.as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert!(items.iter().all(|item| item["date"] == "2026-11-23"));
}
