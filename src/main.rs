//! Thin subcommand dispatcher. All logic lives in the library.

use std::path::Path;

use chora::cli::{Cli, Cmd, parse_date_arg};
use chora::config::Config;
use chora::error::ChoraError;
use chora::store::Store;
use chora::{editor, event, git, output, session, tui};
use clap::Parser;

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    chora::log::init(cli.debug);
    chora::debug_log!("debug logging on");
    match cli.cmd {
        Cmd::Init => cmd_init(),
        Cmd::__Finalize { message } => {
            let (_cfg, store) = load_store(cli.dir.as_deref())?;
            require_repo(&store)?;
            git::finalize(store.root(), &message)
        }
        Cmd::Note => {
            let (cfg, store) = load_store(cli.dir.as_deref())?;
            require_repo(&store)?;
            session::run(store.root(), &store.notes_dir(), cfg.every()?, opener(&cfg))
        }
        Cmd::Event {
            date,
            tui: use_tui,
            confirm,
        } => {
            let (cfg, store) = load_store(cli.dir.as_deref())?;
            require_repo(&store)?;
            let open = opener(&cfg);
            match (date, use_tui) {
                (Some(_), true) => Err(ChoraError::EventExclusiveTarget.into()),
                (None, false) => Err(ChoraError::EventNeedsTarget.into()),
                (Some(raw), false) => cmd_event_stub(&store, &raw, open),
                (None, true) => cmd_event_tui(&store, confirm, open),
            }
        }
        Cmd::Show { date, json } => {
            let (_cfg, store) = load_store(cli.dir.as_deref())?;
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
    }
}

/// Load the config and make sure the layout exists.
fn load_store(dir: Option<&Path>) -> anyhow::Result<(Config, Store)> {
    let cfg = Config::load()?;
    let store = Store::new(cfg.root(dir));
    chora::debug_log!("root: {}", store.root().display());
    store.ensure_layout()?;
    Ok((cfg, store))
}

/// Opener command from the config (`editor`), if set. Empty means `$EDITOR`.
fn opener(cfg: &Config) -> Option<&str> {
    let cmd = cfg.main.editor.trim();
    (!cmd.is_empty()).then_some(cmd)
}

/// Fail unless the root is a git repo (fixed by `chora init`).
fn require_repo(store: &Store) -> anyhow::Result<()> {
    if git::is_repo(store.root()) {
        Ok(())
    } else {
        Err(ChoraError::NotARepo(store.root().to_path_buf()).into())
    }
}

/// `add + commit on changes + push` (push errors are visible here).
fn commit_and_push(store: &Store, message: &str) -> anyhow::Result<()> {
    git::add_all(store.root())?;
    if git::commit(store.root(), message)? {
        git::push(store.root())?;
    }
    Ok(())
}

/// `init` takes no flags: everything comes from the config file.
fn cmd_init() -> anyhow::Result<()> {
    let ensured = Config::ensure_exists()?;
    if !ensured.added.is_empty() {
        println!("new config fields added: {}", ensured.added.join(", "));
    }
    let cfg = Config::load()?;
    let store = Store::new(cfg.root(None));
    store.ensure_layout()?;
    store.ensure_gitignore()?;
    if !git::is_repo(store.root()) {
        git::init_repo(store.root())?;
    }
    if !cfg.main.remote_url.is_empty() && !git::has_remote(store.root()) {
        git::add_remote(store.root(), &cfg.main.remote_url)?;
    }
    if !git::identity_ok(store.root()) {
        chora::warn_log!(
            "git user.name/user.email are not set; commits will fail. Set them, e.g. `git config --global user.name \"you\"`"
        );
    }
    git::add_all(store.root())?;
    if git::commit(store.root(), &format!("init: {}", git::stamp_now()))? {
        // Best-effort: the remote may be empty or offline on first run.
        let _ = git::push(store.root());
    }
    println!("config: {}", ensured.path.display());
    println!("storage: {}", store.root().display());
    println!("Edit remote_url in the config, then re-run `chora init`.");
    Ok(())
}

/// `event --date`: append an empty stub and open the day file.
fn cmd_event_stub(store: &Store, raw: &str, open: Option<&str>) -> anyhow::Result<()> {
    let day = parse_date_arg(raw)?;
    let path = store.ensure_day_file(day)?;
    store.append_section(day, "## \nTime: \nDuration: \n")?;
    editor::open(&path, store.root(), open)?;
    commit_and_push(store, &format!("event: {}", day.format("%Y-%m-%d")))
}

/// `event --tui [--confirm]`: form submit appends the section, no editor.
fn cmd_event_tui(store: &Store, confirm: bool, open: Option<&str>) -> anyhow::Result<()> {
    let data = match tui::run()? {
        tui::TuiOutcome::Cancelled => return Ok(()),
        tui::TuiOutcome::Submitted(data) => data,
    };
    // Re-checked here: the TUI validated already, this is defense in depth.
    let duration = event::parse_duration(&data.duration_raw)?;
    let rendered = event::render_section(&event::Event {
        title: data.name.clone(),
        date: data.date,
        time: chrono::Local::now().time(),
        duration,
        context: data.context.clone(),
    });
    store.append_section(data.date, &rendered)?;
    let message = format!("event: {} {}", data.date.format("%Y-%m-%d"), data.name);
    commit_and_push(store, &message)?;
    if confirm {
        let path = store.ensure_day_file(data.date)?;
        editor::open(&path, store.root(), open)?;
        commit_and_push(store, &message)?;
    }
    Ok(())
}
