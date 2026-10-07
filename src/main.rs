//! Thin dispatcher; logic lives in the library.

use std::path::{Path, PathBuf};

use anyhow::Context;
use chora::cli::{
    Cli, Cmd, CompletionShell, EventAction, OpenTarget, parse_date_arg, render_help_tree, today,
};
use chora::config::{Config, OldEventPolicy};
use chora::error::ChoraError;
use chora::store::Store;
use chora::{editor, event, git, output, sync, tui};
use clap::Parser;

fn main() -> anyhow::Result<()> {
    // Internal background child first: it is not a clap subcommand, so it
    // never leaks into help or generated shell completions.
    if let Some(internal) = chora::cli::take_sync_after_open(std::env::args_os().skip(1)) {
        return cmd_sync_after_open(internal.dir.as_deref(), &internal.message);
    }
    // Same story: hand-parsed, invisible to generated completions.
    if let Some(shell) = chora::cli::take_completion(std::env::args_os().skip(1)) {
        return cmd_completion(shell);
    }
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
                OpenTarget::Note => (store.notes_dir(), "note"),
                OpenTarget::Event => (store.events_dir(), "event"),
            };
            // cwd = target: `sh -c '... $(fzf)'` openers scan cwd and ignore
            // the target arg (it lands in $0), so root would show both dirs.
            // No network up here: the opener must start instantly, and nothing
            // may delay the detach below. The pull lives in the detached
            // `sync-after-open` child.
            if matches!(target, OpenTarget::Event) {
                sweep_old_events(&store, &cfg)?;
            }
            sync::with_auto_commit(store.root(), cfg.main.auto_commit, name, || {
                editor::open(&dir, &dir, dirs_opener(&cfg))
            })?;
            sync_in_background(&store, cli.dir.as_deref(), &format!("open: {name}"))
        }
        Cmd::Event {
            date,
            tui: use_tui,
            confirm,
            action,
        } => {
            let creating = date.is_some() || use_tui || confirm;
            match action {
                Some(_) if creating => Err(ChoraError::EventExclusiveAction.into()),
                // Read-only: showing never creates storage or needs a repo.
                Some(EventAction::Show { date, json }) => {
                    let (_cfg, store) = load_store(cli.dir.as_deref(), false)?;
                    cmd_event_show(&store, date.as_deref(), json)
                }
                None => {
                    let (cfg, store) = load_store(cli.dir.as_deref(), true)?;
                    require_repo(&store)?;
                    let open = files_opener(&cfg);
                    match (date, use_tui) {
                        (Some(_), true) => Err(ChoraError::EventExclusiveTarget.into()),
                        (None, false) => Err(ChoraError::EventNeedsTarget.into()),
                        (Some(raw), false) => {
                            cmd_event_stub(&store, &cfg, &raw, open, cli.dir.as_deref())
                        }
                        (None, true) => {
                            cmd_event_tui(&store, &cfg, confirm, open, cli.dir.as_deref())
                        }
                    }
                }
            }
        }
    }
}

/// The detached background child: commit the exit state, rebase onto fresh
/// remote work, push. Failures land in `sync.log` via redirected stderr.
fn cmd_sync_after_open(dir: Option<&Path>, message: &str) -> anyhow::Result<()> {
    let (_cfg, store) = load_store(dir, true)?;
    require_repo(&store)?;
    // Timestamp header: the log is append-only, runs must delimit.
    eprintln!(
        "--- sync-after-open {} ---",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
    );
    sync::sync_after_open(&store, message)
}

fn load_store(dir: Option<&Path>, bootstrap: bool) -> anyhow::Result<(Config, Store)> {
    let cfg = Config::load()?;
    let store = Store::new(cfg.root(dir));
    chora::debug_log!("root: {}", store.root().display());
    if bootstrap {
        // Empty dirs never block a checkout; files (`.gitignore`) would,
        // so the gitignore comes last, after the first fetch.
        store.ensure_layout()?;
        if !git::is_repo(store.root()) {
            git::init_repo(store.root())?;
        }
        if !cfg.main.remote_url.is_empty() && !git::has_remote(store.root()) {
            git::add_remote(store.root(), &cfg.main.remote_url)?;
        }
        // New device: fetch + checkout before `.gitignore` exists.
        git::ensure_from_remote(store.root())?;
        store.ensure_gitignore()?;
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

/// Apply `manage_old_event` to passed day files. `Nothing` skips the scan
/// entirely; otherwise the sweep rides along with the action's own commit.
fn sweep_old_events(store: &Store, cfg: &Config) -> anyhow::Result<()> {
    let policy = cfg.main.manage_old_event;
    if policy == OldEventPolicy::Nothing {
        return Ok(());
    }
    let report = store.sweep_old_events(policy, today())?;
    if report.deleted > 0 {
        println!("old events: deleted {}", report.deleted);
    }
    if report.backlogged > 0 {
        println!("old events: moved {} to backlog", report.backlogged);
    }
    Ok(())
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
    // Probe: the detached child appends there; fail loudly now if unwritable.
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .with_context(|| format!("failed to open {}", log.display()))?;
    match spawn_sync_child(&exe, dir, message, store.root(), log_file) {
        Ok(_) => {
            println!("syncing in background (log: {})", log.display());
            Ok(())
        }
        Err(err) => {
            chora::debug_log!("background sync spawn failed ({err}); syncing in foreground");
            sync::sync_after_open(store, message)
        }
    }
}

/// One detached `sync-after-open` child. `process_group(0)` gives it its own
/// process group at fork time, before `spawn` even returns: closing the foot
/// window group-kills chora's group, but the child was never in it.
/// `setsid` inside adds its own session (tty hangup immunity).
/// No new deps, no waiting.
fn spawn_sync_child(
    exe: &Path,
    dir: Option<&Path>,
    message: &str,
    root: &Path,
    log_file: std::fs::File,
) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new("setsid");
    cmd.arg(exe);
    if let Some(dir) = dir {
        cmd.arg("--dir").arg(dir);
    }
    cmd.arg("sync-after-open").arg("--message").arg(message);
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(log_file)
        .current_dir(root)
        .process_group(0)
        .spawn()
}

/// Append-only log for background sync failures. Outside the storage root,
/// so the sync itself never commits it.
fn sync_log_path() -> anyhow::Result<PathBuf> {
    let data = dirs::data_dir().context("failed to determine the data directory")?;
    Ok(data.join("chora").join("sync.log"))
}

/// Print shell completions to stdout. Pure: needs no config or storage.
fn cmd_completion(shell: CompletionShell) -> anyhow::Result<()> {
    use clap::CommandFactory;
    use clap_complete::{generate, shells::Bash, shells::Fish};

    let mut cmd = Cli::command();
    match shell {
        CompletionShell::Bash => generate(Bash, &mut cmd, "chora", &mut std::io::stdout()),
        CompletionShell::Fish => generate(Fish, &mut cmd, "chora", &mut std::io::stdout()),
    }
    Ok(())
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

/// Read-only show: one day (plus its repeatables) or all days.
/// Showing never creates storage, needs no repo, and touches no network.
fn cmd_event_show(store: &Store, raw: Option<&str>, json: bool) -> anyhow::Result<()> {
    if let Some(raw) = raw {
        let day = parse_date_arg(raw)?;
        let mut events = event::parse_day(day, &store.read_day(day)?);
        events.extend(store.read_repeatables(day)?);
        events.sort_by_key(|event| event.time);
        if json {
            println!("{}", output::json(&events)?);
        } else {
            print!("{}", output::plaintext(day, &events));
        }
        return Ok(());
    }
    let mut all = Vec::new();
    let mut chunks = Vec::new();
    for day in store.list_days()? {
        let events = event::parse_day(day, &store.read_day(day)?);
        if events.is_empty() {
            continue;
        }
        if json {
            all.extend(events);
        } else {
            chunks.push(output::plaintext(day, &events));
        }
    }
    if json {
        println!("{}", output::json(&all)?);
    } else if chunks.is_empty() {
        println!("(no events)");
    } else {
        for chunk in chunks {
            print!("{chunk}");
        }
    }
    Ok(())
}

fn cmd_event_stub(
    store: &Store,
    cfg: &Config,
    raw: &str,
    open: Option<&str>,
    dir: Option<&Path>,
) -> anyhow::Result<()> {
    let day = parse_date_arg(raw)?;
    // No network up here: the opener must start instantly. Fresh remote
    // work merges in the detached child on exit (rebase heals fast-forwards;
    // real conflicts land loudly in `sync.log`).
    sweep_old_events(store, cfg)?;
    let path = store.ensure_day_file(day)?;
    store.append_section(day, "## \nTime: \nDuration: \n")?;
    sync::with_auto_commit(store.root(), cfg.main.auto_commit, "event", || {
        editor::open(&path, store.root(), open)
    })?;
    // Same detach as `open` on exit: the shell is back instantly, the
    // commit + rebase + push finish in the background child.
    sync_in_background(store, dir, &format!("event: {}", day.format("%Y-%m-%d")))
}

fn cmd_event_tui(
    store: &Store,
    cfg: &Config,
    confirm: bool,
    open: Option<&str>,
    dir: Option<&Path>,
) -> anyhow::Result<()> {
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
    // No network up here either: the form opens instantly, the merge
    // happens in the detached child after submit.
    store.append_section(data.date, &rendered)?;
    let message = format!("event: {} {}", data.date.format("%Y-%m-%d"), data.title);
    sweep_old_events(store, cfg)?;
    sync_in_background(store, dir, &message)?;
    if confirm {
        let path = store.ensure_day_file(data.date)?;
        sync::with_auto_commit(store.root(), cfg.main.auto_commit, "event", || {
            editor::open(&path, store.root(), open)
        })?;
        sync_in_background(store, dir, &message)?;
    }
    Ok(())
}
