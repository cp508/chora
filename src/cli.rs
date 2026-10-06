//! Clap declaration of the CLI: one command — one job.

use std::path::PathBuf;

use anyhow::Context;
use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use crate::error::ChoraError;

/// `chora [--dir <path>] <command>`.
#[derive(Debug, Parser)]
#[command(
    name = "chora",
    version,
    about = "Lightweight CLI organizer: markdown, calendar, git sync",
    long_about = "COMMANDS:\n\n  init\n    Bootstrap the config and the storage. Takes no flags.\n\n  note\n    Session: open notes/ in the opener, autocommit every auto_commit minutes.\n\n  event --date <DATE>\n    Append an event stub to the day file and open it in the opener.\n\n  event --tui [--confirm]\n    TUI form for creating an event. With --confirm, open the day file after submit.\n\n  show --date <DATE> [--json]\n    Show the events of a day. With --json, print a JSON array for scripts.\n\n<DATE> is today, tomorrow, or YYYY-MM-DD.\nGlobal flag: --dir <PATH> overrides chora_dir for one run.\nOpener: config `editor`, or $EDITOR when empty."
)]
pub struct Cli {
    /// Override `chora_dir` from the config for one run.
    #[arg(long, global = true, value_name = "PATH")]
    pub dir: Option<PathBuf>,

    /// Verbose internals on stderr (levels: error, warn, info, debug).
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub cmd: Cmd,
}

/// chora subcommands.
#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Bootstrap the config and the storage. No flags — everything comes from the config.
    Init,
    /// Session: open notes/ in the opener + autocommit every `auto_commit` minutes.
    Note,
    /// Day events.
    Event {
        /// Append an event stub to the date file and open it in the opener.
        #[arg(long, value_name = "DATE")]
        date: Option<String>,
        /// TUI form for creating an event.
        #[arg(long)]
        tui: bool,
        /// After the TUI, open the day file in the opener right away.
        #[arg(long)]
        confirm: bool,
    },
    /// Show the events of a day.
    Show {
        /// Date: today|tomorrow|YYYY-MM-DD.
        #[arg(long)]
        date: String,
        /// JSON array for scripts.
        #[arg(long)]
        json: bool,
    },
    /// Background final commit (spawned detached by `note`, not for humans).
    #[command(name = "__finalize", hide = true)]
    __Finalize {
        /// Commit message, precomputed by the parent process.
        #[arg(long)]
        message: String,
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
    fn parses_show_with_json() {
        let cli = Cli::try_parse_from(["chora", "show", "--date", "2026-11-20", "--json"]).unwrap();
        assert!(matches!(cli.cmd, Cmd::Show { json: true, .. }));
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
            Cmd::Event {
                tui: true,
                confirm: true,
                date: None
            }
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
