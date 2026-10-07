//! Clap declaration of the CLI: one command — one job.
//!
//! Help flags are disabled on purpose: bare `chora` prints [`HELP_TREE`].

use std::path::PathBuf;

use anyhow::Context;
use chrono::NaiveDate;
use clap::{Parser, Subcommand};

use crate::error::ChoraError;

/// Full command tree, printed for bare `chora`.
pub const HELP_TREE: &str = r#"Commands:
  init           Bootstrap the config. Storage appears on first use.
  open           Open a storage dir once, sync on exit.
    <note|event>
                 which directory to open
  event          Day events.
    --date <DATE>
                 append a stub to the day file and open it in the file opener
    --tui
                 TUI form for creating an event
    --confirm
                 with --tui: open the day file after submit
    show
                 show events: one day with --date, all days without
      --date <DATE>
                 which day to show (default: all days)
      --json
                 print a JSON array for scripts
  completion     Print shell completions to stdout.
    <bash|fish>
                 which shell to complete
Options:
  --dir <PATH>   override chora_dir for one run
  --debug        verbose internals on stderr
<DATE> is today, tomorrow, YYYY-MM-DD, or DD.MM.YYYY.
Openers: `editor` for files, `open_dirs` for directories, $EDITOR when empty.
"#;

/// Nesting colors for the tree: headers, level-1 commands, level-2 args.
fn header_style() -> anstyle::Style {
    anstyle::Style::new()
        .bold()
        .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Yellow)))
}

fn command_style() -> anstyle::Style {
    anstyle::Style::new()
        .bold()
        .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Green)))
}

fn arg_style() -> anstyle::Style {
    anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Cyan)))
}

fn dim_style() -> anstyle::Style {
    anstyle::Style::new().dimmed()
}

fn paint(style: anstyle::Style, text: &str) -> String {
    format!("{style}{text}{style:#}")
}

/// Paint one tree line by its nesting level. Descriptions keep the base color.
fn style_line(line: &str) -> String {
    if line.trim().is_empty() {
        return line.to_owned();
    }
    let indent = line.len() - line.trim_start().len();
    if indent == 0 {
        if line.ends_with(':') {
            return paint(header_style(), line);
        }
        return paint(dim_style(), line);
    }
    let trimmed = line.trim_start();
    let pad = &line[..line.len() - trimmed.len()];
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (head, tail) = (&trimmed[..end], &trimmed[end..]);
    let style = if indent == 2 {
        command_style()
    } else if indent == 4 {
        arg_style()
    } else {
        return paint(dim_style(), line);
    };
    format!("{pad}{}{tail}", paint(style, head))
}

/// The tree with nesting colors. Pure: always paints.
fn colorize_tree(text: &str) -> String {
    text.lines().map(style_line).collect::<Vec<_>>().join("\n") + "\n"
}

/// Colors only for a live terminal; pipes and `NO_COLOR` stay plain.
fn use_color() -> bool {
    use std::io::IsTerminal;
    std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

/// The tree for bare `chora`: colored on a terminal, plain otherwise.
#[must_use]
pub fn render_help_tree() -> String {
    if use_color() {
        colorize_tree(HELP_TREE)
    } else {
        HELP_TREE.to_owned()
    }
}

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
    #[arg(long, global = true, value_name = "PATH", value_hint = clap::ValueHint::DirPath)]
    pub dir: Option<PathBuf>,

    /// Verbose internals on stderr (levels: error, warn, info, debug).
    #[arg(long, global = true)]
    pub debug: bool,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

/// Which storage directory to open. A subcommand (not a value) so static
/// completions list it in every shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum OpenTarget {
    /// The `notes/` directory.
    #[command(disable_help_flag = true)]
    Note,
    /// The `events/` directory.
    #[command(disable_help_flag = true)]
    Event,
}

/// Actions under `event`.
#[derive(Debug, Subcommand)]
pub enum EventAction {
    /// Show events: one day with `--date`, all days without.
    #[command(disable_help_flag = true)]
    Show {
        /// Date: today|tomorrow|YYYY-MM-DD|DD.MM.YYYY. Empty = all days.
        #[arg(long, value_hint = clap::ValueHint::Other)]
        date: Option<String>,
        /// JSON array for scripts.
        #[arg(long)]
        json: bool,
    },
}

/// Shells with generated completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionShell {
    Bash,
    Fish,
}

/// chora subcommands.
#[derive(Debug, Subcommand)]
pub enum Cmd {
    /// Bootstrap the config. No flags; storage appears on first use.
    #[command(disable_help_flag = true)]
    Init,
    /// Open a storage dir once in the dirs-opener, sync on exit.
    #[command(disable_help_flag = true)]
    Open {
        /// Which directory to open.
        #[command(subcommand)]
        target: OpenTarget,
    },
    /// Day events.
    #[command(disable_help_flag = true)]
    Event {
        /// Append an event stub to the date file and open it in the file opener.
        #[arg(long, value_name = "DATE", value_hint = clap::ValueHint::Other)]
        date: Option<String>,
        /// TUI form for creating an event.
        #[arg(long)]
        tui: bool,
        /// After the TUI, open the day file in the file opener right away.
        #[arg(long)]
        confirm: bool,
        #[command(subcommand)]
        action: Option<EventAction>,
    },
}

/// `chora completion|completions <bash|fish>`, parsed by hand (not a clap
/// subcommand) so it never leaks into generated shell completions.
pub fn take_completion(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Option<CompletionShell> {
    use std::ffi::OsStr;

    let args: Vec<_> = args.into_iter().collect();
    let (name, shell) = match args.as_slice() {
        [name, shell] => (name, shell),
        _ => return None,
    };
    if name.as_os_str() != OsStr::new("completion") && name.as_os_str() != OsStr::new("completions")
    {
        return None;
    }
    match shell.as_os_str().to_str()? {
        "bash" => Some(CompletionShell::Bash),
        "fish" => Some(CompletionShell::Fish),
        _ => None,
    }
}

/// Internal background child, built by `spawn_sync_child`:
/// `chora [--dir <PATH>] sync-after-open --message <MSG>`.
/// Parsed by hand (not a clap subcommand) so it never leaks into
/// generated shell completions or help.
#[derive(Debug, PartialEq, Eq)]
pub struct SyncAfterOpen {
    pub dir: Option<PathBuf>,
    pub message: String,
}

pub fn take_sync_after_open(
    args: impl IntoIterator<Item = std::ffi::OsString>,
) -> Option<SyncAfterOpen> {
    use std::ffi::OsStr;

    let args: Vec<_> = args.into_iter().collect();
    let (dir, rest) = match args.as_slice() {
        [flag, path, rest @ ..] if flag.as_os_str() == OsStr::new("--dir") => {
            (Some(PathBuf::from(path)), rest)
        }
        rest => (None, rest),
    };
    match rest {
        [cmd, flag, msg]
            if cmd.as_os_str() == OsStr::new("sync-after-open")
                && flag.as_os_str() == OsStr::new("--message") =>
        {
            Some(SyncAfterOpen {
                dir,
                message: msg.to_str()?.to_owned(),
            })
        }
        _ => None,
    }
}

/// Today (local date).
#[must_use]
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

/// Parse a date argument: `today|tomorrow|YYYY-MM-DD|DD.MM.YYYY`.
pub fn parse_date_arg(raw: &str) -> anyhow::Result<NaiveDate> {
    match raw {
        "today" => Ok(today()),
        "tomorrow" => today().succ_opt().context("date overflow for tomorrow"),
        _ => NaiveDate::parse_from_str(raw, "%Y-%m-%d")
            .or_else(|_| NaiveDate::parse_from_str(raw, "%d.%m.%Y"))
            .map_err(|_| ChoraError::BadDate(raw.to_owned()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(ch) = chars.next() {
            if ch == '\u{1b}' {
                for ch in chars.by_ref() {
                    if ch.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn bare_chora_has_no_command() {
        let cli = Cli::try_parse_from(["chora"]).unwrap();
        assert!(cli.cmd.is_none());
    }

    #[test]
    fn tree_coloring_survives_ansi_strip() {
        let colored = colorize_tree(HELP_TREE);
        assert!(colored.contains("\u{1b}["));
        assert_eq!(strip_ansi(&colored), HELP_TREE);
    }

    #[test]
    fn tree_paints_nesting_levels() {
        let out = colorize_tree("Commands:\n  init           Bootstrap.\n    --date <DATE>\n");
        let mut lines = out.lines();
        let header = lines.next().unwrap();
        assert!(header.starts_with("\u{1b}["));
        assert!(header.ends_with("\u{1b}[0m"));
        let cmd = lines.next().unwrap();
        assert!(cmd.starts_with("  \u{1b}["));
        let arg = lines.next().unwrap();
        assert!(arg.starts_with("    \u{1b}["));
        assert_ne!(cmd, arg);
    }

    #[test]
    fn parses_event_show_with_json() {
        let cli = Cli::try_parse_from(["chora", "event", "show", "--date", "2026-11-20", "--json"])
            .unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Event {
                action: Some(EventAction::Show { json: true, .. }),
                ..
            })
        ));
    }

    #[test]
    fn parses_event_show_without_date() {
        let cli = Cli::try_parse_from(["chora", "event", "show"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Event {
                action: Some(EventAction::Show {
                    date: None,
                    json: false
                }),
                ..
            })
        ));
    }

    #[test]
    fn parses_open_note() {
        let cli = Cli::try_parse_from(["chora", "open", "note"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Open {
                target: OpenTarget::Note
            })
        ));
    }

    #[test]
    fn parses_open_event() {
        let cli = Cli::try_parse_from(["chora", "open", "event"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Open {
                target: OpenTarget::Event
            })
        ));
    }

    #[test]
    fn completion_parses_both_names() {
        use std::ffi::OsString;

        for name in ["completion", "completions"] {
            assert_eq!(
                take_completion([name, "bash"].map(OsString::from)),
                Some(CompletionShell::Bash)
            );
            assert_eq!(
                take_completion([name, "fish"].map(OsString::from)),
                Some(CompletionShell::Fish)
            );
        }
    }

    #[test]
    fn completion_rejects_other_shapes() {
        use std::ffi::OsString;

        for args in [
            vec!["completion"],
            vec!["completion", "zsh"],
            vec!["completion", "bash", "extra"],
            vec!["open", "note"],
            vec!["event", "show"],
        ] {
            assert!(
                take_completion(args.iter().map(OsString::from)).is_none(),
                "{args:?}"
            );
        }
    }

    #[test]
    fn sync_after_open_parses_full_shape() {
        let args = [
            "--dir",
            "/tmp/c",
            "sync-after-open",
            "--message",
            "open: note",
        ];
        assert_eq!(
            take_sync_after_open(args.map(std::ffi::OsString::from)),
            Some(SyncAfterOpen {
                dir: Some(PathBuf::from("/tmp/c")),
                message: String::from("open: note"),
            })
        );
    }

    #[test]
    fn sync_after_open_parses_without_dir() {
        let args = ["sync-after-open", "--message", "event: 2026-11-20"];
        assert_eq!(
            take_sync_after_open(args.map(std::ffi::OsString::from)),
            Some(SyncAfterOpen {
                dir: None,
                message: String::from("event: 2026-11-20"),
            })
        );
    }

    #[test]
    fn sync_after_open_rejects_other_shapes() {
        for args in [
            vec!["open", "note"],
            vec!["sync-after-open"],
            vec!["sync-after-open", "--message"],
            vec!["--dir", "/tmp/c", "sync-after-open"],
            vec!["event", "show"],
        ] {
            assert!(
                take_sync_after_open(args.iter().map(std::ffi::OsString::from)).is_none(),
                "{args:?}"
            );
        }
    }

    #[test]
    fn dir_is_global() {
        let cli = Cli::try_parse_from([
            "chora", "--dir", "/tmp/c", "event", "show", "--date", "today",
        ])
        .unwrap();
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
                date: None,
                action: None
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
    fn date_arg_dots() {
        assert_eq!(
            parse_date_arg("20.05.2026").unwrap(),
            NaiveDate::from_ymd_opt(2026, 5, 20).unwrap()
        );
    }

    #[test]
    fn date_arg_rejects_garbage() {
        for raw in ["", "yesterday", "20-11-2026", "2026-13-01", "32.01.2026"] {
            assert!(parse_date_arg(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn debug_flag_is_global() {
        let cli =
            Cli::try_parse_from(["chora", "--debug", "event", "show", "--date", "today"]).unwrap();
        assert!(cli.debug);
        let cli = Cli::try_parse_from(["chora", "event", "show", "--date", "today"]).unwrap();
        assert!(!cli.debug);
    }
}
