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

/// Base command: isolated HOME, no XDG leakage, deterministic git identity.
fn cmd(env: &Env) -> Command {
    let mut cmd = Command::new(bin());
    cmd.env("HOME", &env.home);
    cmd.env_remove("XDG_CONFIG_HOME");
    cmd.env("GIT_AUTHOR_NAME", "tester");
    cmd.env("GIT_AUTHOR_EMAIL", "tester@example.com");
    cmd.env("GIT_COMMITTER_NAME", "tester");
    cmd.env("GIT_COMMITTER_EMAIL", "tester@example.com");
    cmd.env("GIT_CONFIG_NOSYSTEM", "1");
    cmd
}

/// Write an `sh` script and return an EDITOR value that runs it.
/// No chmod needed: the editor is invoked as `sh <script> <target>`.
fn editor_script(env: &Env, name: &str, body: &str) -> String {
    let path = env.home.join(name);
    std::fs::write(&path, body).unwrap();
    format!("sh {}", path.display())
}

fn git_log(root: &Path) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    assert!(out.status.success());
    String::from_utf8_lossy(&out.stdout).into_owned()
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
    assert!(
        git_log(&root)
            .lines()
            .any(|line| line.starts_with("event:"))
    );
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
    assert!(
        git_log(&root)
            .lines()
            .any(|line| line.starts_with("event:"))
    );
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
        .args(["--dir", &root.to_string_lossy(), "show", "--date", "nope"])
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn missing_config_points_to_init() {
    let env = setup();
    let root = storage(&env);
    let out = cmd(&env)
        .args(["--dir", &root.to_string_lossy(), "show", "--date", "today"])
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
        .args(["--dir", &root.to_string_lossy(), "open", "notes"])
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
        .args(["--dir", &root.to_string_lossy(), "open", "notes"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(root.join("notes").join("picked.md").exists());
    poll_log(&root, "open: notes");
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
        .args(["--dir", &root.to_string_lossy(), "open", "notes"])
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
    poll_log(&root, "open: notes");
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
        .args(["--dir", &root.to_string_lossy(), "open", "events"])
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
        .args(["--dir", &root.to_string_lossy(), "open", "notes"])
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
