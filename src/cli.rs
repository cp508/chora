//! Clap declaration of the CLI: one command — one job.
//!
//! Help flags are disabled on purpose: bare `chora` prints [`HELP_TREE`].

use std::path::PathBuf;

use anyhow::Context;
use chrono::NaiveDate;
use clap::{Parser, Subcommand, ValueEnum};

use crate::error::ChoraError;

/// Full command tree, printed for bare `chora`.
pub const HELP_TREE: &str = r#"Commands:
  init           Bootstrap the config and the storage. Takes no flags.
  open           Open a storage dir once in the dirs-opener, commit on exit.
    <notes|events>
                 which directory to open
  event          Day events.
    --date <DATE>
                 append a stub to the day file and open it in the file opener
    --tui
                 TUI form for creating an event
    --confirm
                 with --tui: open the day file after submit
  show           Show the events of a day.
    --date <DATE>
                 which day to show
    --json
                 print a JSON array for scripts
Options:
  --dir <PATH>   override chora_dir for one run
  --debug        verbose internals on stderr
<DATE> is today, tomorrow, or YYYY-MM-DD.
Openers: `editor` for files, `open_dirs` for directories, $EDITOR when empty.
"#;

/// `chora [--dir <path>] [--debug] <command>`. No command prints the tree.
#[derive(Debug, Parser)]
#[command(
    name = "chora",
    version,
    about = "Lightweight CLI organizer: markdown, events, git sync",
    disable_help_flag = true,
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Override `chora_dir` from the config for one run.
    #[arg(long, global = true, value_name = "PATH")]
    pub dir: Option<PathBuf>,

    /// Verbose internals on stderr (levels: error, warn, info, debug).
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

/// Which storage directory to open.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OpenTarget {
    /// The `notes/` directory.
    Notes,
    /// The `events/` directory.
    Events,
}

/// chora subcommands.
#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Bootstrap the config and the storage. No flags — everything comes from the config.
    #[command(disable_help_flag = true)]
    Init,
    /// Open a storage dir once in the dirs-opener, commit on exit.
    #[command(disable_help_flag = true)]
    Open {
        /// Which directory to open.
        target: OpenTarget,
    },
    /// Day events.
    #[command(disable_help_flag = true)]
    Event {
        /// Append an event stub to the date file and open it in the file opener.
        #[arg(long, value_name = "DATE")]
        date: Option<String>,
        /// TUI form for creating an event.
        #[arg(long)]
        tui: bool,
        /// After the TUI, open the day file in the file opener right away.
        #[arg(long)]
        confirm: bool,
    },
    /// Show the events of a day.
    #[command(disable_help_flag = true)]
    Show {
        /// Date: today|tomorrow|YYYY-MM-DD.
        #[arg(long)]
        date: String,
        /// JSON array for scripts.
        #[arg(long)]
        json: bool,
    },
}

/// Today (local date).
#[must_use]
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// Parse a date argument: `today|tomorrow|YYYY-MM-DD`.
pub fn parse_date_arg(raw: &str) -> anyhow::Result<NaiveDate> {
    match raw {
        "today" => Ok(today()),
        "tomorrow" => today().succ_opt().context("date overflow for tomorrow"),
        _ => NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .map_err(|_| ChoraError::BadDate(raw.to_owned()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_chora_has_no_command() {
        let cli = Cli::try_parse_from(["chora"]).unwrap();
        assert!(cli.cmd.is_none());
    }

    #[test]
    fn parses_show_with_json() {
        let cli = Cli::try_parse_from(["chora", "show", "--date", "2026-11-20", "--json"]).unwrap();
        assert!(matches!(cli.cmd, Some(Cmd::Show { json: true, .. })));
    }

    #[test]
    fn parses_open_notes() {
        let cli = Cli::try_parse_from(["chora", "open", "notes"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Open {
                target: OpenTarget::Notes
            })
        ));
    }

    #[test]
    fn dir_is_global() {
        let cli =
            Cli::try_parse_from(["chora", "--dir", "/tmp/c", "show", "--date", "today"]).unwrap();
        assert_eq!(cli.dir, Some(PathBuf::from("/tmp/c")));
    }

    #[test]
    fn parses_event_tui_confirm() {
        let cli = Cli::try_parse_from(["chora", "event", "--tui", "--confirm"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Event {
                tui: true,
                confirm: true,
                date: None
            })
        ));
    }

    #[test]
    fn date_arg_keywords() {
        assert_eq!(parse_date_arg("today").unwrap(), today());
        assert_eq!(
            parse_date_arg("tomorrow").unwrap(),
            today().succ_opt().unwrap()
        );
    }

    #[test]
    fn date_arg_iso() {
        assert_eq!(
            parse_date_arg("2026-11-20").unwrap(),
            NaiveDate::from_ymd_opt(2026, 11, 20).unwrap()
        );
    }

    #[test]
    fn date_arg_rejects_garbage() {
        for raw in ["", "yesterday", "20-11-2026", "2026-13-01"] {
            assert!(parse_date_arg(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn debug_flag_is_global() {
        let cli = Cli::try_parse_from(["chora", "--debug", "show", "--date", "today"]).unwrap();
        assert!(cli.debug);
        let cli = Cli::try_parse_from(["chora", "show", "--date", "today"]).unwrap();
        assert!(!cli.debug);
    }
}
