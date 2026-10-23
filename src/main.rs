//! Thin dispatcher; logic lives in the library.

use std::path::{Path, PathBuf};

use anyhow::Context;
use chora::cli::{
    Cli, Cmd, CompletionShell, EventAction, NoteAction, OpenTarget, parse_date_arg,
    render_help_tree, render_skill_help, today,
};
use chora::config::{Config, OldEventPolicy};
use chora::error::ChoraError;
use chora::index::{AccessLevel, Index};
use chora::store::Store;
use chora::{calendar, editor, event, git, note, output, sync, tui};
use clap::Parser;

fn main() -> anyhow::Result<()> {
    if let Ok(message) = std::env::var("CHORA_SYNC_AFTER_OPEN") {
        return cmd_sync_after_open(&message);
    }
    // Invalid input shows the same tree as bare `chora`, then the error.
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            use clap::error::ErrorKind;
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                err.exit();
            }
            eprint!("{}", chora::cli::render_help_tree_for_stderr());
            err.exit();
        }
    };
    chora::log::init(cli.debug);
    chora::debug_log!("debug logging on");
    let Some(cmd) = cli.cmd else {
        page_or_print(&render_help_tree());
        return Ok(());
    };
    match cmd {
        Cmd::Init => cmd_init(),
        Cmd::Settings => cmd_settings(),
        Cmd::Help => {
            // Machine-readable output for agents: never paged.
            print!("{}", render_skill_help());
            Ok(())
        }
        Cmd::Completion { shell } => cmd_completion(shell),
        Cmd::Open { target } => {
            let (cfg, store) = load_store(cli.dir.as_deref(), true)?;
            require_repo(&store)?;
            // cwd = target: `sh -c '... $(fzf)'` openers scan cwd and ignore
            // the target arg (it lands in $0), so root would show both dirs.
            // No network up here: the opener must start instantly, and nothing
            // may delay the detach below. The pull lives in the detached
            // `sync-after-open` child.
            if matches!(target, OpenTarget::Index) {
                let path = store.index_path();
                if !path.exists() {
                    let dirs = note::top_dirs(&store.notes_dir())?;
                    std::fs::write(&path, Index::skeleton(&dirs))
                        .with_context(|| format!("failed to write {}", path.display()))?;
                }
                sync::with_auto_commit(store.root(), cfg.main.auto_commit, "index", || {
                    editor::open(&path, store.root(), &cfg.main.editor)
                })?;
                return sync_in_background(&store, "open: index");
            }
            let (dir, name) = match target {
                OpenTarget::Note => (store.notes_dir(), "note"),
                OpenTarget::Event => (store.events_dir(), "event"),
                OpenTarget::Index => unreachable!("handled above"),
            };
            if matches!(target, OpenTarget::Event) {
                sweep_old_events(&store, &cfg)?;
            }
            sync::with_auto_commit(store.root(), cfg.main.auto_commit, name, || {
                editor::open(&dir, &dir, &cfg.main.open_dirs)
            })?;
            sync_in_background(&store, &format!("open: {name}"))
        }
        Cmd::Calendar => {
            let (cfg, store) = load_store(cli.dir.as_deref(), false)?;
            cmd_calendar(&store, &cfg, cli.dir.as_deref())
        }
        Cmd::Note { action } => {
            let max = cli.max_access;
            match action {
                NoteAction::Add {
                    dirname,
                    path,
                    under,
                } => {
                    let (_cfg, store) = load_store(cli.dir.as_deref(), true)?;
                    require_repo(&store)?;
                    cmd_note_add(&store, &dirname, &path, under.as_deref(), cli.max_access)
                }
                NoteAction::Override { dirname, path } => {
                    let (_cfg, store) = load_store(cli.dir.as_deref(), true)?;
                    require_repo(&store)?;
                    cmd_note_override(&store, &dirname, &path, cli.max_access)
                }
                NoteAction::Show {
                    dirname,
                    path,
                    json,
                } => {
                    let (_cfg, store) = load_store(cli.dir.as_deref(), false)?;
                    cmd_note_show(&store, dirname.as_deref(), &path, json, max)
                }
                NoteAction::Headers { dirname, path } => {
                    let (_cfg, store) = load_store(cli.dir.as_deref(), false)?;
                    cmd_note_headers(&store, &dirname, &path, max)
                }
            }
        }
        Cmd::Index { json } => {
            let (_cfg, store) = load_store(cli.dir.as_deref(), false)?;
            cmd_index(&store, json, cli.max_access)
        }
        Cmd::Event {
            date,
            confirm,
            title,
            time,
            duration,
            body,
            action,
        } => {
            let direct = title.is_some() || time.is_some() || duration.is_some() || body.is_some();
            let creating = date.is_some() || confirm || direct;
            match action {
                Some(_) if creating => Err(ChoraError::EventExclusiveAction.into()),
                // Read-only: showing never creates storage or needs a repo.
                Some(EventAction::Show { date, json }) => {
                    let (_cfg, store) = load_store(cli.dir.as_deref(), false)?;
                    cmd_event_show(&store, date.as_deref(), json)
                }
                None if direct => {
                    let (cfg, store) = load_store(cli.dir.as_deref(), true)?;
                    require_repo(&store)?;
                    let (Some(title), Some(time), Some(duration)) = (title, time, duration) else {
                        return Err(ChoraError::EventNeedsFields.into());
                    };
                    cmd_event_add(
                        &store,
                        &cfg,
                        date.as_deref(),
                        &title,
                        &time,
                        &duration,
                        body.as_deref().unwrap_or_default(),
                        confirm,
                    )
                }
                None => {
                    let (cfg, store) = load_store(cli.dir.as_deref(), true)?;
                    require_repo(&store)?;
                    match date {
                        Some(raw) => cmd_event_stub(&store, &cfg, &raw),
                        // Bare `event` is the TUI form; `--confirm` opens
                        // the day file after submit.
                        None => cmd_event_tui(&store, &cfg, confirm),
                    }
                }
            }
        }
    }
}

/// Print `text` through `less -R` on a terminal (colors preserved),
/// plain print otherwise (pipes, scripts). Falls back to print when
/// `less` is missing. Write errors are ignored: quitting the pager
/// early closes the pipe, which is not a failure.
fn page_or_print(text: &str) {
    use std::io::{IsTerminal, Write};
    if !std::io::stdout().is_terminal() {
        print!("{text}");
        return;
    }
    let Ok(mut child) = std::process::Command::new("less")
        .arg("-R")
        .stdin(std::process::Stdio::piped())
        .spawn()
    else {
        print!("{text}");
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(text.as_bytes());
    }
    let _ = child.wait();
}

/// The detached background child: commit the exit state, rebase onto fresh
/// remote work, push. Failures land in `sync.log` via redirected stderr.
fn cmd_sync_after_open(message: &str) -> anyhow::Result<()> {
    let store = Store::new(std::env::current_dir()?);
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
fn sync_in_background(store: &Store, message: &str) -> anyhow::Result<()> {
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
    match spawn_sync_child(&exe, message, store.root(), log_file) {
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
    message: &str,
    root: &Path,
    log_file: std::fs::File,
) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt;
    std::process::Command::new("setsid")
        .arg(exe)
        .env("CHORA_SYNC_AFTER_OPEN", message)
        .stdin(std::process::Stdio::null())
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

/// Interactive TUI editor for `config.toml`; creates the config first if missing.
fn cmd_settings() -> anyhow::Result<()> {
    Config::ensure_exists()?;
    let cfg = Config::load()?;
    if let Some(updated) = tui::run_settings(&cfg)? {
        let path = updated.save()?;
        println!("saved: {}", path.display());
    }
    Ok(())
}

/// Read-only month calendar. Showing never creates storage or needs a repo;
/// unknown colors fail fast with the config path attached. `Enter` opens
/// the selected day exactly like `event --date` (stub, editor, background
/// sync) — that path bootstraps the store and needs a repo.
fn cmd_calendar(store: &Store, cfg: &Config, dir: Option<&Path>) -> anyhow::Result<()> {
    let path = Config::path().unwrap_or_else(|| store.root().join("config.toml"));
    let day = calendar::parse_color(&cfg.calendar.day_color).map_err(|msg| {
        ChoraError::ConfigInvalid {
            path: path.clone(),
            msg,
        }
    })?;
    let repeat = calendar::parse_color(&cfg.calendar.repeat_color)
        .map_err(|msg| ChoraError::ConfigInvalid { path, msg })?;
    let options = calendar::Options {
        day_color: day,
        repeat_color: repeat,
        show_repeat: cfg.calendar.show_repeat,
        fixed_cells: cfg.calendar.fixed_cells,
        header: cfg.calendar.header.clone(),
        show_today: cfg.calendar.show_today,
    };
    let Some(date) = calendar::run(store.root(), options)? else {
        return Ok(());
    };
    let (cfg, store) = load_store(dir, true)?;
    require_repo(&store)?;
    open_day_file(&store, &cfg, date)
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

fn cmd_event_stub(store: &Store, cfg: &Config, raw: &str) -> anyhow::Result<()> {
    let day = parse_date_arg(raw)?;
    open_day_file(store, cfg, day)
}

/// Non-interactive creation: no editor, no TUI. `--date` defaults to today;
/// past dates are allowed (unlike the TUI). Syncs in the background.
#[allow(clippy::too_many_arguments)]
fn cmd_event_add(
    store: &Store,
    cfg: &Config,
    date_raw: Option<&str>,
    title_raw: &str,
    time_raw: &str,
    duration_raw: &str,
    body_raw: &str,
    confirm: bool,
) -> anyhow::Result<()> {
    let day = match date_raw {
        Some(raw) => parse_date_arg(raw)?,
        None => today(),
    };
    let title = title_raw.trim();
    if title.is_empty() {
        anyhow::bail!("title is empty");
    }
    let time = event::parse_time_arg(time_raw)?;
    let duration = event::parse_duration(duration_raw)?;
    let rendered = event::render_section(&event::Event {
        title: title.to_owned(),
        date: day,
        time,
        duration,
        context: body_raw.trim().to_owned(),
    });
    if day >= today() {
        sweep_old_events(store, cfg)?;
    }
    sync::with_sync_lock(store.root(), || {
        store.append_section(day, &rendered)?;
        Ok(())
    })?;
    if confirm {
        let path = store.ensure_day_file(day)?;
        sync::with_auto_commit(store.root(), cfg.main.auto_commit, "event", || {
            editor::open(&path, store.root(), &cfg.main.editor)
        })?;
    }
    let message = format!("event: {} {title}", day.format("%Y-%m-%d"));
    sync_in_background(store, &message)?;
    println!(
        "created: {} {} {title}",
        day.format("%Y-%m-%d"),
        time.format("%H:%M")
    );
    Ok(())
}

/// Append a stub to the day file and open it in the file opener.
/// Shared by `event --date` and calendar `Enter`.
fn open_day_file(store: &Store, cfg: &Config, day: chrono::NaiveDate) -> anyhow::Result<()> {
    // No network up here: the opener must start instantly. Fresh remote
    // work merges in the detached child on exit (rebase heals fast-forwards;
    // real conflicts land loudly in `sync.log`).
    if day >= today() {
        sweep_old_events(store, cfg)?;
    }
    let path = sync::with_sync_lock(store.root(), || {
        let path = store.ensure_day_file(day)?;
        store.append_section(day, "## \nTime: \nDuration: \n")?;
        Ok(path)
    })?;
    sync::with_auto_commit(store.root(), cfg.main.auto_commit, "event", || {
        editor::open(&path, store.root(), &cfg.main.editor)
    })?;
    // Same detach as `open` on exit: the shell is back instantly, the
    // commit + rebase + push finish in the background child.
    sync_in_background(store, &format!("event: {}", day.format("%Y-%m-%d")))
}

fn cmd_event_tui(store: &Store, cfg: &Config, confirm: bool) -> anyhow::Result<()> {
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
    sync::with_sync_lock(store.root(), || {
        store.append_section(data.date, &rendered)?;
        Ok(())
    })?;
    let message = format!("event: {} {}", data.date.format("%Y-%m-%d"), data.title);
    sweep_old_events(store, cfg)?;
    if confirm {
        let path = store.ensure_day_file(data.date)?;
        sync::with_auto_commit(store.root(), cfg.main.auto_commit, "event", || {
            editor::open(&path, store.root(), &cfg.main.editor)
        })?;
    }
    sync_in_background(store, &message)
}

/// Read the whole stdin as a note body. Empty input is an error:
/// silently truncating a file would be data loss.
fn read_stdin_body() -> anyhow::Result<String> {
    use std::io::Read;
    let mut body = String::new();
    std::io::stdin()
        .read_to_string(&mut body)
        .context("failed to read stdin")?;
    if body.trim().is_empty() {
        return Err(ChoraError::NoteEmptyBody.into());
    }
    Ok(body)
}

/// Resolve `notes/<dirname>/<rel>` safely and check the `.md` suffix.
/// `.` addresses the notes root itself (loose `notes/*.md` files).
fn note_file(store: &Store, dirname: &str, rel: &str) -> anyhow::Result<(String, PathBuf)> {
    note::require_md(rel)?;
    let full = if dirname == "." {
        rel.to_owned()
    } else {
        format!("{dirname}/{rel}")
    };
    let path = note::join_safe(&store.notes_dir(), &full)?;
    Ok((full, path))
}

/// Append stdin markdown to a note file (creates it when missing).
/// `--under` inserts at the end of the first matching section instead.
/// Refuses targets above `max` clearance; the disk write holds the sync
/// lock so it never interleaves with a background pull --rebase.
fn cmd_note_add(
    store: &Store,
    dirname: &str,
    rel: &str,
    under: Option<&str>,
    max: AccessLevel,
) -> anyhow::Result<()> {
    let (full, path) = note_file(store, dirname, rel)?;
    let index = Index::load(store.root())?;
    let need = index.security_for(&full);
    if !need.allows(max) {
        return Err(ChoraError::NoteDenied {
            path: full,
            need: need.as_str(),
            max: max.as_str(),
        }
        .into());
    }
    let body = read_stdin_body()?;
    let body = format!("{}\n", body.trim_matches('\n'));
    sync::with_sync_lock(store.root(), || {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        if path.exists() {
            let mut text = std::fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            if let Some(header) = under {
                text = note::insert_under(&text, header, body.trim_matches('\n'))?;
            } else {
                if !text.ends_with('\n') {
                    text.push('\n');
                }
                text.push('\n');
                text.push_str(body.trim_matches('\n'));
                text.push('\n');
            }
            std::fs::write(&path, text)
                .with_context(|| format!("failed to write {}", path.display()))?;
        } else {
            if under.is_some() {
                anyhow::bail!("no such file `{full}`: `--under` needs an existing file");
            }
            std::fs::write(&path, &body)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        Ok(())
    })?;
    let message = format!("note add: {full}");
    sync_in_background(store, &message)?;
    println!("appended: {full}");
    Ok(())
}

/// Overwrite a note file with stdin markdown (creates it when missing).
/// Refuses targets above `max` clearance; the disk write holds the sync lock.
fn cmd_note_override(
    store: &Store,
    dirname: &str,
    rel: &str,
    max: AccessLevel,
) -> anyhow::Result<()> {
    let (full, path) = note_file(store, dirname, rel)?;
    let index = Index::load(store.root())?;
    let need = index.security_for(&full);
    if !need.allows(max) {
        return Err(ChoraError::NoteDenied {
            path: full,
            need: need.as_str(),
            max: max.as_str(),
        }
        .into());
    }
    let body = read_stdin_body()?;
    sync::with_sync_lock(store.root(), || {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        std::fs::write(&path, format!("{}\n", body.trim_matches('\n')))
            .with_context(|| format!("failed to write {}", path.display()))?;
        Ok(())
    })?;
    let message = format!("note override: {full}");
    sync_in_background(store, &message)?;
    println!("wrote: {full}");
    Ok(())
}

/// Print the all-dirs listing (metadata, no bodies). Shared by bare
/// `note show` and the unknown-dirname fallback.
fn print_notes_bare(store: &Store, max: AccessLevel, json: bool) -> anyhow::Result<()> {
    let index = Index::load(store.root())?;
    let mut dirs = Vec::new();
    for dirname in note::top_dirs(&store.notes_dir())? {
        let view = note::dir_view(store.root(), &index, &dirname, max, false, &[])?;
        // A dir above clearance still shows when it holds visible files.
        if view.security.allows(max) || !view.files.is_empty() {
            dirs.push(view);
        }
    }
    let loose = note::loose_files_view(store.root(), &index, max, false)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&note::bare_json(&dirs, &loose))?
        );
    } else {
        print!("{}", note::render_bare(&dirs, &loose));
    }
    Ok(())
}

/// Read-only show: one dir (or root `.`) recursive with bodies,
/// optionally scoped by `--path`; or all dirs with metadata when neither
/// is given. Unknown dirname errors and prints the all-dirs listing.
fn cmd_note_show(
    store: &Store,
    dirname: Option<&str>,
    scopes: &[String],
    json: bool,
    max: AccessLevel,
) -> anyhow::Result<()> {
    let index = Index::load(store.root())?;
    let (None, true) = (dirname, scopes.is_empty()) else {
        let view = match dirname {
            None | Some(".") => note::root_view(store.root(), &index, max, true, scopes),
            Some(dirname) => note::dir_view(store.root(), &index, dirname, max, true, scopes),
        };
        return print_scoped_view(store, dirname, scopes, view, json, max);
    };
    print_notes_bare(store, max, json)
}

/// Print one scoped view, or the all-dirs listing + error on unknown dir.
/// Above clearance without visible files also lands here (no confirmation).
fn print_scoped_view(
    store: &Store,
    dirname: Option<&str>,
    scopes: &[String],
    view: anyhow::Result<note::DirView>,
    json: bool,
    max: AccessLevel,
) -> anyhow::Result<()> {
    let visible = view
        .as_ref()
        .is_ok_and(|v| v.security.allows(max) || !v.files.is_empty());
    match view {
        Ok(view) if visible => {
            if json {
                println!("{}", serde_json::to_string_pretty(&note::dir_json(&view))?);
            } else {
                print!("{}", note::render_dir(&view));
            }
            Ok(())
        }
        // Unknown dirname — or above clearance (do not confirm it exists):
        // error and print the all-dirs listing as a guide. Real IO errors pass through.
        Ok(_) => {
            print_notes_bare(store, max, false)?;
            Err(ChoraError::NoteNoDir(scoped_name(dirname, scopes)).into())
        }
        Err(err)
            if matches!(
                err.downcast_ref::<ChoraError>(),
                Some(ChoraError::NoteNoDir(_))
            ) =>
        {
            print_notes_bare(store, max, false)?;
            Err(err)
        }
        Err(err) => Err(err),
    }
}

/// Name for the unknown-scope error: the dirname, or the first scope.
fn scoped_name(dirname: Option<&str>, scopes: &[String]) -> String {
    match dirname {
        Some(name) => name.to_owned(),
        None => scopes.first().cloned().unwrap_or_default(),
    }
}

/// Read-only header tree of a notes dir, recursive, with file names.
/// `--path` scopes to files/subdirs inside (repeatable).
fn cmd_note_headers(
    store: &Store,
    dirname: &str,
    paths: &[String],
    max: AccessLevel,
) -> anyhow::Result<()> {
    let index = Index::load(store.root())?;
    let notes = store.notes_dir();
    let dir = if dirname == "." {
        notes.clone()
    } else {
        note::join_safe(&notes, dirname)?
    };
    if !dir.is_dir() {
        return Err(ChoraError::NoteNoDir(dirname.to_owned()).into());
    }
    let files = note::collect_scoped(&dir, paths).map_err(|err| {
        match err.downcast_ref::<ChoraError>() {
            Some(ChoraError::NoteBadPath(msg)) if dirname != "." => {
                ChoraError::NoteBadPath(format!("{dirname}/{msg}")).into()
            }
            _ => err,
        }
    })?;
    let mut shown = 0;
    for abs in &files {
        let rel = if dirname == "." {
            note::rel_of(&dir, abs)
        } else {
            format!("{dirname}/{}", note::rel_of(&dir, abs))
        };
        let security = index.security_for(&rel);
        if !security.allows(max) {
            continue;
        }
        let text = std::fs::read_to_string(abs)
            .with_context(|| format!("failed to read {}", abs.display()))?;
        println!("{rel}:");
        let headers = note::read_headers(&text);
        if headers.is_empty() {
            println!("  (no headers)");
        }
        for header in &headers {
            println!(
                "  {}: {} {}",
                header.line,
                "#".repeat(header.level),
                header.title
            );
        }
        shown += 1;
    }
    if shown == 0 {
        println!("(no visible notes)");
    }
    Ok(())
}

/// Read-only: `index.md` context + security/about tree.
/// Showing never creates storage and touches no network.
fn cmd_index(store: &Store, json: bool, max: AccessLevel) -> anyhow::Result<()> {
    let index = Index::load(store.root())?;
    let mut warnings: Vec<String> = index
        .visible_warnings(max)
        .iter()
        .map(|w| match &w.path {
            Some(path) => format!("{path}: {}", w.msg),
            None => w.msg.clone(),
        })
        .collect();
    for extra in note::unknown_dir_warnings(&index, &store.notes_dir()) {
        if extra
            .path
            .as_deref()
            .is_some_and(|path| index.security_for(path).allows(max))
        {
            warnings.push(format!("{}: {}", extra.path.unwrap_or_default(), extra.msg));
        }
    }
    if json {
        let entries: Vec<serde_json::Value> = index
            .visible(max)
            .map(|e| {
                serde_json::json!({
                    "path": e.path,
                    "security": e.security.as_str(),
                    "about": e.about,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "context": index.context,
                "entries": entries,
                "warnings": warnings,
            }))?
        );
    } else {
        print!("{}", note::render_index(&index, max));
    }
    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}
