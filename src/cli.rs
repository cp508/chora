//! Clap declaration of the CLI: one command — one job.
//!
//! Help flags are disabled on purpose: bare `chora` prints the tree from
//! [`render_help_tree`], built dynamically from the clap schema below.

use std::path::PathBuf;

use anyhow::Context;
use chrono::NaiveDate;
use clap::{CommandFactory, Parser, Subcommand};

use crate::error::ChoraError;

/// Static footnotes: not part of the clap schema.
const ANNOTATION: [&str; 2] = [
    "<DATE> is today, tomorrow, YYYY-MM-DD, or DD.MM.YYYY.",
    "Openers: `editor` for files, `open_dirs` for directories, $EDITOR when empty.",
];

/// One tree row: a command (blue, bold) or an argument (green).
/// `prefix` already holds the box-drawing guides and is never painted.
/// `new_group` starts a blank line before top-level commands.
struct Row {
    prefix: String,
    command: bool,
    new_group: bool,
    head: String,
    about: String,
}

fn about_of(cmd: &clap::Command) -> String {
    cmd.get_about()
        .map(|text| text.to_string())
        .unwrap_or_default()
}

/// `--date <DATE>`, `--tui`, or `<VALUES>` for positionals.
/// `get_value_names()` reports defaults even for flags without values,
/// so only `Set`/`Append` actions render `<NAME>`.
fn arg_head(arg: &clap::Arg) -> String {
    let takes_value = matches!(
        arg.get_action(),
        clap::ArgAction::Set | clap::ArgAction::Append
    );
    let values: Vec<String> = if takes_value {
        arg.get_value_names()
            .iter()
            .flat_map(|names| names.iter())
            .map(|name| name.to_string())
            .collect()
    } else {
        Vec::new()
    };
    match arg.get_long() {
        Some(long) => match values.first() {
            Some(name) => format!("--{long} <{name}>"),
            None => format!("--{long}"),
        },
        None if values.is_empty() => arg.get_id().to_string(),
        None => format!("<{}>", values.join("|")),
    }
}

fn arg_about(arg: &clap::Arg) -> String {
    arg.get_help()
        .map(|text| text.to_string())
        .unwrap_or_default()
}

fn visible_arg(arg: &clap::Arg) -> bool {
    !arg.is_hide_set() && !matches!(arg.get_long(), Some("help" | "version"))
}

fn branch(last: bool) -> &'static str {
    if last { "└── " } else { "├── " }
}

/// Flags first, then nested subcommands — mirrors the `event` layout.
/// `guides` carries the ancestors' `│` bars; the last sibling closes them.
fn push_children(cmd: &clap::Command, guides: &str, rows: &mut Vec<Row>) {
    let args: Vec<_> = cmd.get_arguments().filter(|arg| visible_arg(arg)).collect();
    let subs: Vec<_> = cmd.get_subcommands().collect();
    let total = args.len() + subs.len();
    for (index, arg) in args.iter().enumerate() {
        rows.push(Row {
            prefix: format!("{guides}{}", branch(index + 1 == total)),
            command: false,
            new_group: false,
            head: arg_head(arg),
            about: arg_about(arg),
        });
    }
    for (index, sub) in subs.iter().enumerate() {
        let last = index + 1 == subs.len();
        rows.push(Row {
            prefix: format!("{guides}{}", branch(last)),
            command: true,
            new_group: false,
            head: sub.get_name().to_owned(),
            about: about_of(sub),
        });
        let child_guides = format!("{guides}{}", if last { "    " } else { "│   " });
        push_children(sub, &child_guides, rows);
    }
}

/// The whole tree: schema subcommands plus the hand-parsed `completion`
/// (outside the clap schema, but still user-facing).
fn build_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for sub in Cli::command().get_subcommands() {
        rows.push(Row {
            prefix: String::from("  • "),
            command: true,
            new_group: true,
            head: sub.get_name().to_owned(),
            about: about_of(sub),
        });
        push_children(sub, "  ", &mut rows);
    }
    rows.push(Row {
        prefix: String::from("  • "),
        command: true,
        new_group: true,
        head: String::from("completion"),
        about: String::from("print shell completions to stdout"),
    });
    let shells = COMPLETION_SHELLS
        .iter()
        .map(|(name, _)| *name)
        .collect::<Vec<_>>()
        .join("|");
    rows.push(Row {
        prefix: String::from("  └── "),
        command: false,
        new_group: false,
        head: format!("<{shells}>"),
        about: String::from("which shell to complete"),
    });
    rows
}

/// Global flags for the `Options:` section.
fn global_options() -> Vec<Row> {
    Cli::command()
        .get_arguments()
        .filter(|arg| visible_arg(arg))
        .map(|arg| Row {
            prefix: String::from("  "),
            command: false,
            new_group: false,
            head: arg_head(arg),
            about: arg_about(arg),
        })
        .collect()
}

fn row_line(row: &Row) -> String {
    if row.about.is_empty() {
        format!("{}{}", row.prefix, row.head)
    } else {
        format!("{}{} - {}", row.prefix, row.head, row.about)
    }
}

/// Headers: `Commands:`, `Annotation:`, `Options:`. Footnotes stay dim.
fn header_style() -> anstyle::Style {
    anstyle::Style::new()
        .bold()
        .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Yellow)))
}

/// Commands: bold blue.
fn command_style() -> anstyle::Style {
    anstyle::Style::new()
        .bold()
        .fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Blue)))
}

/// Arguments: plain green.
fn arg_style() -> anstyle::Style {
    anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::Green)))
}

fn dim_style() -> anstyle::Style {
    anstyle::Style::new().dimmed()
}

/// Placeholders like `<DATE>`: gray.
fn gray_style() -> anstyle::Style {
    anstyle::Style::new().fg_color(Some(anstyle::Color::Ansi(anstyle::AnsiColor::BrightBlack)))
}

fn paint(style: anstyle::Style, text: &str) -> String {
    format!("{style}{text}{style:#}")
}

/// Paint a head: `<...>` placeholders go gray, the rest keeps the row style.
fn paint_head(head: &str, base: anstyle::Style) -> String {
    let mut out = String::new();
    let mut rest = head;
    while let Some(start) = rest.find('<') {
        let (plain, tail) = rest.split_at(start);
        if !plain.is_empty() {
            out.push_str(&paint(base, plain));
        }
        match tail.find('>') {
            Some(end) => {
                let (placeholder, after) = tail.split_at(end + 1);
                out.push_str(&paint(gray_style(), placeholder));
                rest = after;
            }
            None => {
                out.push_str(&paint(base, tail));
                rest = "";
            }
        }
    }
    if !rest.is_empty() {
        out.push_str(&paint(base, rest));
    }
    out
}

/// Plain text, no colors. Pure: the single source for tests and pipes.
fn help_text_plain() -> String {
    let cmd = Cli::command();
    let mut out = format!("{} - {}\n", cmd.get_name(), about_of(&cmd));
    out.push_str("\nCommands:\n");
    let mut first_group = true;
    for row in build_rows() {
        if row.new_group && !first_group {
            out.push('\n');
        }
        first_group = false;
        out.push_str(&row_line(&row));
        out.push('\n');
    }
    out.push_str("\nAnnotation:\n");
    for line in ANNOTATION {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("\nOptions:\n");
    for row in global_options() {
        out.push_str(&row_line(&row));
        out.push('\n');
    }
    out
}

/// Paint one row: guides stay plain, the head keeps its style
/// with gray `<...>` placeholders.
fn paint_row(row: &Row, style: anstyle::Style) -> String {
    let line = row_line(row);
    let head_start = line.find(&row.head).unwrap_or(0);
    let mut out = String::from(&line[..head_start]);
    out.push_str(&paint_head(&row.head, style));
    out.push_str(&line[head_start + row.head.len()..]);
    out
}

/// Same text with colors: headers yellow, commands blue bold, args green.
fn help_text_colored() -> String {
    let cmd = Cli::command();
    let mut out = format!(
        "{} - {}\n",
        paint(command_style(), cmd.get_name()),
        about_of(&cmd)
    );
    out.push('\n');
    out.push_str(&paint(header_style(), "Commands:"));
    out.push('\n');
    let mut first_group = true;
    for row in build_rows() {
        if row.new_group && !first_group {
            out.push('\n');
        }
        first_group = false;
        let style = if row.command {
            command_style()
        } else {
            arg_style()
        };
        out.push_str(&paint_row(&row, style));
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&paint(header_style(), "Annotation:"));
    out.push('\n');
    for line in ANNOTATION {
        out.push_str(&paint(dim_style(), line));
        out.push('\n');
    }
    out.push('\n');
    out.push_str(&paint(header_style(), "Options:"));
    out.push('\n');
    for row in global_options() {
        out.push_str(&paint_row(&row, arg_style()));
        out.push('\n');
    }
    out
}

/// Colors only for a live terminal; pipes and `NO_COLOR` stay plain.
fn use_color(stream_is_terminal: bool) -> bool {
    stream_is_terminal && std::env::var_os("NO_COLOR").is_none()
}

/// The tree for bare `chora`: colored on a terminal, plain otherwise.
#[must_use]
pub fn render_help_tree() -> String {
    use std::io::IsTerminal;
    if use_color(std::io::stdout().is_terminal()) {
        help_text_colored()
    } else {
        help_text_plain()
    }
}

/// Same tree for parse errors (stderr): colored on a terminal, plain otherwise.
#[must_use]
pub fn render_help_tree_for_stderr() -> String {
    use std::io::IsTerminal;
    if use_color(std::io::stderr().is_terminal()) {
        help_text_colored()
    } else {
        help_text_plain()
    }
}

/// `chora [--dir <path>] [--debug] <command>`. No command prints the tree.
#[derive(Debug, Parser)]
#[command(
    name = "chora",
    version,
    about = "lightweight CLI organizer: markdown notes, day events, git sync. No daemons.",
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

/// Single source for the shell names: the hand parser and the tree below
/// both read this, so they cannot drift apart.
const COMPLETION_SHELLS: [(&str, CompletionShell); 2] = [
    ("bash", CompletionShell::Bash),
    ("fish", CompletionShell::Fish),
];

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
    /// Day events: TUI form by default, `show` to display.
    #[command(disable_help_flag = true)]
    Event {
        /// Append an event stub to the date file and open it in the file opener.
        #[arg(long, value_name = "DATE", value_hint = clap::ValueHint::Other)]
        date: Option<String>,
        /// After submit, open the day file in the file opener right away.
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
    let shell = shell.as_os_str().to_str()?;
    COMPLETION_SHELLS
        .iter()
        .find(|(known, _)| *known == shell)
        .map(|(_, parsed)| *parsed)
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
    fn tree_lists_every_schema_subcommand() {
        fn collect(cmd: &clap::Command, names: &mut Vec<String>) {
            for sub in cmd.get_subcommands() {
                names.push(sub.get_name().to_owned());
                collect(sub, names);
            }
        }
        let mut names = Vec::new();
        collect(&Cli::command(), &mut names);
        assert!(!names.is_empty());
        let plain = help_text_plain();
        for name in names {
            assert!(plain.contains(&name), "tree misses `{name}`");
        }
        // Hand-parsed, outside the schema — still shown.
        for token in ["completion", "<bash|fish>", "--dir <PATH>", "--debug"] {
            assert!(plain.contains(token), "tree misses `{token}`");
        }
    }

    #[test]
    fn tree_paints_commands_blue_bold_and_args_green() {
        let colored = help_text_colored();
        // Bold blue command, plain green argument, gray placeholder.
        assert!(colored.contains("\u{1b}[1m\u{1b}[34minit"));
        assert!(colored.contains("\u{1b}[32m--date "));
        assert!(colored.contains("\u{1b}[90m<DATE>"));
        assert_eq!(strip_ansi(&colored), help_text_plain());
    }

    #[test]
    fn tree_has_title_annotation_and_options() {
        let plain = help_text_plain();
        let mut lines = plain.lines();
        assert!(lines.next().unwrap().starts_with("chora - "));
        assert!(plain.contains("\nCommands:\n"));
        assert!(plain.contains("\nAnnotation:\n"));
        assert!(plain.contains("\nOptions:\n"));
        assert!(plain.contains("<DATE> is today"));
    }

    #[test]
    fn readme_lists_every_command() {
        const README: &str = include_str!("../README.md");
        fn collect(cmd: &clap::Command, names: &mut Vec<String>) {
            for sub in cmd.get_subcommands() {
                names.push(sub.get_name().to_owned());
                collect(sub, names);
            }
        }
        // `completion` is hand-parsed outside the schema, but documented.
        let mut names = vec![String::from("completion")];
        collect(&Cli::command(), &mut names);
        assert!(!names.is_empty());
        for name in names {
            assert!(README.contains(&name), "README misses `{name}`");
        }
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
    fn parses_bare_event_as_tui_with_confirm() {
        let cli = Cli::try_parse_from(["chora", "event", "--confirm"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Event {
                confirm: true,
                date: None,
                action: None
            })
        ));
        let cli = Cli::try_parse_from(["chora", "event"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Event {
                confirm: false,
                date: None,
                action: None
            })
        ));
    }

    #[test]
    fn event_rejects_removed_tui_flag() {
        assert!(Cli::try_parse_from(["chora", "event", "--tui"]).is_err());
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
