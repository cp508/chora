//! Thin dispatcher; logic lives in the library.

use std::path::{Path, PathBuf};

use anyhow::Context;
use chora::cli::{Cli, Cmd, OpenTarget, parse_date_arg, render_help_tree};
use chora::config::Config;
use chora::error::ChoraError;
use chora::store::Store;
use chora::{editor, event, git, output, tui};
use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    chora::log::init(cli.debug);
    chora::debug_log!("debug logging on");
    let Some(cmd) = cli.cmd else {
        print!("{}", render_help_tree());
        return Ok(());
    };
    match cmd {
        Cmd::Init => cmd_init(),
        Cmd::Open { target } => {
            let (cfg, store) = load_store(cli.dir.as_deref(), true)?;
            require_repo(&store)?;
            let (dir, name) = match target {
                OpenTarget::Notes => (store.notes_dir(), "notes"),
                OpenTarget::Events => (store.events_dir(), "events"),
            };
            // cwd = target: `sh -c '... $(fzf)'` openers scan cwd and ignore
            // the target arg (it lands in $0), so root would show both dirs.
            editor::open(&dir, &dir, dirs_opener(&cfg))?;
            sync_in_background(&store, cli.dir.as_deref(), &format!("open: {name}"))
        }
        Cmd::Event {
            date,
            tui: use_tui,
            confirm,
        } => {
            let (cfg, store) = load_store(cli.dir.as_deref(), true)?;
            require_repo(&store)?;
            let open = files_opener(&cfg);
            match (date, use_tui) {
                (Some(_), true) => Err(ChoraError::EventExclusiveTarget.into()),
                (None, false) => Err(ChoraError::EventNeedsTarget.into()),
                (Some(raw), false) => cmd_event_stub(&store, &raw, open),
                (None, true) => cmd_event_tui(&store, confirm, open),
            }
        }
        Cmd::Show { date, json } => {
            let (_cfg, store) = load_store(cli.dir.as_deref(), false)?;
            let day = parse_date_arg(&date)?;
            let text = store.read_day(day)?;
            let events = event::parse_day(day, &text);
            if json {
                println!("{}", output::json(&events)?);
            } else {
                print!("{}", output::plaintext(day, &events));
            }
            Ok(())
        }
        Cmd::SyncAfterOpen { message } => {
            let (_cfg, store) = load_store(cli.dir.as_deref(), true)?;
            require_repo(&store)?;
            commit_and_push(&store, &message)
        }
    }
}

fn load_store(dir: Option<&Path>, bootstrap: bool) -> anyhow::Result<(Config, Store)> {
    let cfg = Config::load()?;
    let store = Store::new(cfg.root(dir));
    chora::debug_log!("root: {}", store.root().display());
    if bootstrap {
        store.ensure_layout()?;
        store.ensure_gitignore()?;
        if !git::is_repo(store.root()) {
            git::init_repo(store.root())?;
        }
        if !cfg.main.remote_url.is_empty() && !git::has_remote(store.root()) {
            git::add_remote(store.root(), &cfg.main.remote_url)?;
        }
    }
    Ok((cfg, store))
}

fn files_opener(cfg: &Config) -> Option<&str> {
    picked(&cfg.main.editor)
}

fn dirs_opener(cfg: &Config) -> Option<&str> {
    picked(&cfg.main.open_dirs)
}

fn picked(raw: &str) -> Option<&str> {
    let cmd = raw.trim();
    (!cmd.is_empty()).then_some(cmd)
}

fn require_repo(store: &Store) -> anyhow::Result<()> {
    if git::is_repo(store.root()) {
        Ok(())
    } else {
        Err(ChoraError::NotARepo(store.root().to_path_buf()).into())
    }
}

fn commit_and_push(store: &Store, message: &str) -> anyhow::Result<()> {
    with_sync_lock(store.root(), || {
        git::add_all(store.root())?;
        if git::commit(store.root(), message)? {
            git::push(store.root())?;
        }
        Ok(())
    })
}

/// Detach the commit+push after `open`: the editor is closed, the shell is
/// back immediately, and an orphaned copy of this binary finishes the sync
/// in the background and exits. A new session (`setsid`) means even closing
/// the terminal does not kill the push: nothing to wait out.
/// Falls back to a foreground sync when no spawn works, so no edit is ever
/// lost silently.
fn sync_in_background(store: &Store, dir: Option<&Path>, message: &str) -> anyhow::Result<()> {
    let exe = std::env::current_exe().context("failed to locate the chora binary")?;
    let log = sync_log_path()?;
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let open_log = || {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log)
            .with_context(|| format!("failed to open {}", log.display()))
    };
    let spawned =
        spawn_sync_child(&exe, dir, message, store.root(), open_log()?, true).or_else(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                // No setsid(1): plain orphan spawn, same session.
                match open_log() {
                    Ok(retry_log) => {
                        spawn_sync_child(&exe, dir, message, store.root(), retry_log, false)
                    }
                    Err(_) => Err(err),
                }
            } else {
                Err(err)
            }
        });
    match spawned {
        Ok(_) => {
            println!("syncing in background (log: {})", log.display());
            Ok(())
        }
        Err(err) => {
            chora::debug_log!("background sync spawn failed ({err}); syncing in foreground");
            commit_and_push(store, message)
        }
    }
}

/// One detached `sync-after-open` child. With a new session it outlives
/// the terminal; without `setsid` it is a plain orphan (same session).
fn spawn_sync_child(
    exe: &Path,
    dir: Option<&Path>,
    message: &str,
    root: &Path,
    log_file: std::fs::File,
    new_session: bool,
) -> std::io::Result<std::process::Child> {
    let mut cmd = if new_session {
        let mut cmd = std::process::Command::new("setsid");
        cmd.arg(exe);
        cmd
    } else {
        std::process::Command::new(exe)
    };
    if let Some(dir) = dir {
        cmd.arg("--dir").arg(dir);
    }
    cmd.arg("sync-after-open")
        .arg("--message")
        .arg(message)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log_file)
        .current_dir(root)
        .spawn()
}

/// Append-only log for background sync failures. Outside the storage root,
/// so the sync itself never commits it.
fn sync_log_path() -> anyhow::Result<PathBuf> {
    let data = dirs::data_dir().context("failed to determine the data directory")?;
    Ok(data.join("chora").join("sync.log"))
}

/// Serialize add/commit/push across concurrent chora processes.
/// Directory creation is atomic; a stale lock (killed process) times out
/// instead of blocking forever.
fn with_sync_lock(root: &Path, sync: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
    let lock = root.join(".git").join("chora-sync.lock");
    let mut waited = 0;
    while std::fs::create_dir(&lock).is_err() {
        if waited >= 60 {
            chora::warn_log!("sync lock is stale, proceeding anyway");
            break;
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
        waited += 1;
    }
    let result = sync();
    let _ = std::fs::remove_dir(&lock);
    result
}

/// No flags: everything comes from the config file.
/// Creates the config only; the storage appears on first use.
fn cmd_init() -> anyhow::Result<()> {
    let ensured = Config::ensure_exists()?;
    if !ensured.added.is_empty() {
        println!("new config fields added: {}", ensured.added.join(", "));
    }
    let cfg = Config::load()?;
    let root = cfg.root(None);
    println!("config: {}", ensured.path.display());
    println!("storage: {} (created on first use)", root.display());
    println!("Edit the config, then run any command.");
    Ok(())
}

fn cmd_event_stub(store: &Store, raw: &str, open: Option<&str>) -> anyhow::Result<()> {
    let day = parse_date_arg(raw)?;
    let path = store.ensure_day_file(day)?;
    store.append_section(day, "## \nTime: \nDuration: \n")?;
    editor::open(&path, store.root(), open)?;
    commit_and_push(store, &format!("event: {}", day.format("%Y-%m-%d")))
}

fn cmd_event_tui(store: &Store, confirm: bool, open: Option<&str>) -> anyhow::Result<()> {
    let data = match tui::run()? {
        tui::TuiOutcome::Cancelled => return Ok(()),
        tui::TuiOutcome::Submitted(data) => data,
    };
    // Re-checked here: the TUI validated already, this is defense in depth.
    let duration = event::parse_duration(&data.duration_raw)?;
    let rendered = event::render_section(&event::Event {
        title: data.title.clone(),
        date: data.date,
        time: data.time,
        duration,
        context: data.context.clone(),
    });
    store.append_section(data.date, &rendered)?;
    let message = format!("event: {} {}", data.date.format("%Y-%m-%d"), data.title);
    commit_and_push(store, &message)?;
    if confirm {
        let path = store.ensure_day_file(data.date)?;
        editor::open(&path, store.root(), open)?;
        commit_and_push(store, &message)?;
    }
    Ok(())
}
