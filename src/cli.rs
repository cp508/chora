//! Clap declaration of the CLI: one command — one job.
//!
//! Help flags are disabled on purpose: bare `chora` prints the tree from
//! [`render_help_tree`], built dynamically from the clap schema below.

use std::path::PathBuf;

use anyhow::Context;
use chrono::NaiveDate;
use clap::{CommandFactory, Parser, Subcommand};

use crate::error::ChoraError;
use crate::index::AccessLevel;

/// Static footnotes: not part of the clap schema.
const ANNOTATION: [&str; 3] = [
    "<DATE> is today, tomorrow, YYYY-MM-DD, or DD.MM.YYYY.",
    "Openers: `editor` for files, `open_dirs` for directories, $EDITOR when empty.",
    "--max-access gates notes reads/writes: open|restricted (default)|critical.",
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
    let subs: Vec<_> = cmd
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
        .collect();
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

/// The whole tree: built directly from the clap schema.
fn build_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for sub in Cli::command()
        .get_subcommands()
        .filter(|sub| !sub.is_hide_set())
    {
        rows.push(Row {
            prefix: String::from("  • "),
            command: true,
            new_group: true,
            head: sub.get_name().to_owned(),
            about: about_of(sub),
        });
        push_children(sub, "  ", &mut rows);
    }
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

const HEADER_STYLE: &str = "\x1b[1m\x1b[33m";
const COMMAND_STYLE: &str = "\x1b[1m\x1b[34m";
const ARG_STYLE: &str = "\x1b[32m";
const DIM_STYLE: &str = "\x1b[2m";
const GRAY_STYLE: &str = "\x1b[90m";
const RESET: &str = "\x1b[0m";

fn paint(colored: bool, style: &str, text: &str) -> String {
    if colored {
        format!("{style}{text}{RESET}")
    } else {
        text.to_owned()
    }
}

/// Paint a head: `<...>` placeholders go gray, the rest keeps the row style.
fn paint_head(colored: bool, head: &str, base: &str) -> String {
    if !colored {
        return head.to_owned();
    }
    let mut out = String::new();
    let mut rest = head;
    while let Some(start) = rest.find('<') {
        let (plain, tail) = rest.split_at(start);
        if !plain.is_empty() {
            out.push_str(&paint(true, base, plain));
        }
        match tail.find('>') {
            Some(end) => {
                let (placeholder, after) = tail.split_at(end + 1);
                out.push_str(&paint(true, GRAY_STYLE, placeholder));
                rest = after;
            }
            None => {
                out.push_str(&paint(true, base, tail));
                rest = "";
            }
        }
    }
    if !rest.is_empty() {
        out.push_str(&paint(true, base, rest));
    }
    out
}

/// Paint one row: guides stay plain, the head keeps its style
/// with gray `<...>` placeholders.
fn paint_row(colored: bool, row: &Row, style: &str) -> String {
    let line = row_line(row);
    if !colored {
        return line;
    }
    let head_start = line.find(&row.head).unwrap_or(0);
    let mut out = String::from(&line[..head_start]);
    out.push_str(&paint_head(true, &row.head, style));
    out.push_str(&line[head_start + row.head.len()..]);
    out
}

/// Render the CLI help tree: plain when `colored == false`, ANSI-styled when `true`.
fn help_text(colored: bool) -> String {
    let cmd = Cli::command();
    let mut out = format!(
        "{} - {}\n\n{}\n",
        paint(colored, COMMAND_STYLE, cmd.get_name()),
        about_of(&cmd),
        paint(colored, HEADER_STYLE, "Commands:")
    );
    let mut first_group = true;
    for row in build_rows() {
        if row.new_group && !first_group {
            out.push('\n');
        }
        first_group = false;
        let style = if row.command {
            COMMAND_STYLE
        } else {
            ARG_STYLE
        };
        out.push_str(&paint_row(colored, &row, style));
        out.push('\n');
    }
    out.push_str(&format!(
        "\n{}\n",
        paint(colored, HEADER_STYLE, "Annotation:")
    ));
    for line in ANNOTATION {
        out.push_str(&paint(colored, DIM_STYLE, line));
        out.push('\n');
    }
    out.push_str(&format!("\n{}\n", paint(colored, HEADER_STYLE, "Options:")));
    for row in global_options() {
        out.push_str(&paint_row(colored, &row, ARG_STYLE));
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
    help_text(use_color(std::io::stdout().is_terminal()))
}

/// Same tree for parse errors (stderr): colored on a terminal, plain otherwise.
#[must_use]
pub fn render_help_tree_for_stderr() -> String {
    use std::io::IsTerminal;
    help_text(use_color(std::io::stderr().is_terminal()))
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

    /// Notes clearance: hides or refuses entries above this level in
    /// `note show`, `note headers`, `note add`, `note override` and `chora index`.
    #[arg(
        long,
        global = true,
        value_enum,
        value_name = "open|restricted|critical",
        default_value = "restricted"
    )]
    pub max_access: AccessLevel,

    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

/// Which storage entry to open. A subcommand (not a value) so static
/// completions list it in every shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Subcommand)]
pub enum OpenTarget {
    /// The `notes/` directory.
    #[command(disable_help_flag = true)]
    Note,
    /// The `events/` directory.
    #[command(disable_help_flag = true)]
    Event,
    /// The `index.md` context file (human-owned; agents must not write it).
    #[command(disable_help_flag = true)]
    Index,
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

/// Actions under `note`.
#[derive(Debug, Subcommand)]
pub enum NoteAction {
    /// Append stdin markdown to a note file (creates it when missing).
    /// Without `--under` appends at the end, else at the end of the section.
    #[command(disable_help_flag = true)]
    Add {
        /// Notes subdir (`.` = notes root, e.g. for loose `now.md` files).
        #[arg(value_name = "DIRNAME")]
        dirname: String,
        /// File inside the dir, e.g. `main.md` or `worker-task/high.md`.
        #[arg(long, value_name = "PATH")]
        path: String,
        /// Header or `Parent/Child` path to append under, e.g. `--under "Tasks"`.
        #[arg(long, value_name = "HEADER")]
        under: Option<String>,
    },
    /// Overwrite a note file with stdin markdown (creates it when missing).
    #[command(disable_help_flag = true)]
    Override {
        /// Notes subdir (`.` = notes root).
        #[arg(value_name = "DIRNAME")]
        dirname: String,
        /// File inside the dir, e.g. `main.md` or `worker-task/high.md`.
        #[arg(long, value_name = "PATH")]
        path: String,
    },
    /// Show notes: all dirs with metadata, or one dir recursive with bodies.
    /// Unknown dirname errors and prints the all-dirs listing as a guide.
    #[command(disable_help_flag = true)]
    Show {
        /// Notes subdir to show recursive with bodies (`.` = notes root).
        /// Empty = all dirs, no bodies.
        #[arg(value_name = "DIRNAME")]
        dirname: Option<String>,
        /// Scope to files/subdirs inside (repeatable, root-relative with `.`).
        #[arg(long, value_name = "PATH")]
        path: Vec<String>,
        /// JSON for scripts (`--max-access` still filters).
        #[arg(long)]
        json: bool,
    },
    /// List `#`-headers of a notes dir, recursive, with file names and lines.
    #[command(disable_help_flag = true)]
    Headers {
        /// Notes subdir to scan (`.` = notes root).
        #[arg(value_name = "DIRNAME")]
        dirname: String,
        /// Scope to files/subdirs inside (repeatable). Empty = whole dir.
        #[arg(long, value_name = "PATH")]
        path: Vec<String>,
    },
}

/// Actions under `index`.
#[derive(Debug, Subcommand)]
pub enum IndexAction {
    /// Show only the events guide (how events, weekdays, daily events work).
    #[command(disable_help_flag = true)]
    Events {
        /// JSON object for scripts.
        #[arg(long)]
        json: bool,
    },
}

/// Shells with generated completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
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
    /// Day events: TUI form by default, `show` to display.
    #[command(disable_help_flag = true)]
    Event {
        /// Append an event stub to the date file and open it in the file opener.
        #[arg(long, value_name = "DATE", value_hint = clap::ValueHint::Other)]
        date: Option<String>,
        /// After submit, open the day file in the file opener right away.
        #[arg(long)]
        confirm: bool,
        /// Event title for non-interactive creation.
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// Event time HH:MM (or H, e.g. 9 = 09:00).
        #[arg(long, value_name = "TIME")]
        time: Option<String>,
        /// Event duration: 30m, 2h20m, 2h 20m, 2:00.
        #[arg(long, value_name = "DURATION")]
        duration: Option<String>,
        /// Free-form body text for non-interactive creation.
        #[arg(long, value_name = "BODY")]
        body: Option<String>,
        #[command(subcommand)]
        action: Option<EventAction>,
    },
    /// Month calendar with event markers. Read-only.
    #[command(disable_help_flag = true)]
    Calendar,
    /// Interactive TUI editor for all config settings.
    #[command(disable_help_flag = true)]
    Settings,
    /// Agent-readable notes: add/override/show/headers under `notes/`.
    #[command(disable_help_flag = true)]
    Note {
        #[command(subcommand)]
        action: NoteAction,
    },
    /// Show `index.md` context + events guide + security/about tree (`--json` for scripts).
    #[command(disable_help_flag = true)]
    Index {
        /// JSON object for scripts (`--max-access` still filters notes).
        #[arg(long)]
        json: bool,
        #[command(subcommand)]
        action: Option<IndexAction>,
    },
    /// Short machine-readable usage for AI agents (same as `skill`).
    #[command(alias = "skill", disable_help_flag = true)]
    Help,
    /// Print shell completions to stdout.
    #[command(alias = "completions", disable_help_flag = true)]
    Completion {
        /// Which shell to complete.
        #[arg(value_enum, value_name = "bash|fish")]
        shell: CompletionShell,
    },
}

/// Top-level commands agents must never call: they block on an editor or TUI,
/// or bootstrap the human-owned config.
const AGENT_BANNED: [&str; 4] = ["open", "calendar", "settings", "init"];

/// Compressed command list from the clap schema (same source as the tree).
/// Skips banned interactive commands and `help` itself.
fn skill_commands() -> Vec<String> {
    fn walk(cmd: &clap::Command, prefix: &str, out: &mut Vec<String>) {
        for sub in cmd.get_subcommands().filter(|sub| !sub.is_hide_set()) {
            let path = if prefix.is_empty() {
                sub.get_name().to_owned()
            } else {
                format!("{prefix} {}", sub.get_name())
            };
            if AGENT_BANNED.contains(&sub.get_name()) || sub.get_name() == "help" {
                continue;
            }
            let args: Vec<String> = sub
                .get_arguments()
                .filter(|arg| visible_arg(arg))
                // `--confirm` opens an editor: banned for agents, hidden here.
                .filter(|arg| arg.get_long() != Some("confirm"))
                .map(|arg| {
                    let head = arg_head(arg);
                    // Optional positionals read as required (`<X>`); mark them.
                    if arg.get_long().is_none() && !arg.is_required_set() {
                        format!("[{head}]")
                    } else {
                        head
                    }
                })
                .collect();
            let mut line = format!("chora {path}");
            if !args.is_empty() {
                line.push(' ');
                line.push_str(&args.join(" "));
            }
            let about = about_of(sub);
            if !about.is_empty() {
                line.push_str(&format!(" — {about}"));
            }
            out.push(line);
            walk(sub, &path, out);
        }
    }
    let mut out = Vec::new();
    let globals: Vec<String> = Cli::command()
        .get_arguments()
        .filter(|arg| visible_arg(arg))
        .map(arg_head)
        .collect();
    if !globals.is_empty() {
        out.push(format!("chora {} <command>", globals.join(" ")));
    }
    walk(&Cli::command(), "", &mut out);
    out
}

/// Short machine-readable usage for AI agents. Plain text, no colors.
/// Command lines come from the clap schema (see [`skill_commands`]);
/// only the policy block below is static.
#[must_use]
pub fn render_skill_help() -> String {
    let mut lines: Vec<String> = [
        "chora: markdown notes + day events in a git repo. No daemons.",
        "",
        "Storage (default ~/chora): notes/<DIR>/...md, nested ok; events/YYYY-MM-DD.md;",
        "events/repeatable/<weekday>.md; index.md at storage root is HUMAN-owned.",
        "",
        "Commands:",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    lines.extend(skill_commands());
    lines.extend(
        [
            "",
            "Forbidden (interactive or human-owned — never use): open, calendar,",
            "settings, init, bare `event`, `event --date` alone, `--confirm`.",
            "`event` only as: --title <T> --time <HH:MM> --duration <DUR> [--date] [--body].",
            "Note bodies only via stdin pipe: printf '# T\\n' | chora note override <D> --path <F>.",
            "<DATE>: today|tomorrow|YYYY-MM-DD|DD.MM.YYYY. <TIME>: HH:MM or H (9 = 09:00).",
            "<DUR>: 30m, 2h20m, 2h 20m, 2:00 (=2h), 1:30 (=1h30m), 10s.",
            "index.md: agents never write it. --max-access open|restricted|critical",
            "(default restricted) gates notes reads and writes: open sees/writes only open,",
            "restricted sees/writes open+restricted, critical sees/writes all.",
            "Read with --json; day output sorted by time; unknown note dir errors + lists all.",
            "",
        ]
        .into_iter()
        .map(String::from),
    );
    lines.join("\n")
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
            for sub in cmd.get_subcommands().filter(|sub| !sub.is_hide_set()) {
                names.push(sub.get_name().to_owned());
                collect(sub, names);
            }
        }
        let mut names = Vec::new();
        collect(&Cli::command(), &mut names);
        assert!(!names.is_empty());
        let plain = help_text(false);
        for name in names {
            assert!(plain.contains(&name), "tree misses `{name}`");
        }
        for token in [
            "completion",
            "<bash|fish>",
            "--dir <PATH>",
            "--debug",
            "--max-access",
        ] {
            assert!(plain.contains(token), "tree misses `{token}`");
        }
        assert!(!plain.contains("sync-after-open"));
    }

    #[test]
    fn tree_paints_commands_blue_bold_and_args_green() {
        let colored = help_text(true);
        // Bold blue command, plain green argument, gray placeholder.
        assert!(colored.contains("\u{1b}[1m\u{1b}[34minit"));
        assert!(colored.contains("\u{1b}[32m--date "));
        assert!(colored.contains("\u{1b}[90m<DATE>"));
        assert_eq!(strip_ansi(&colored), help_text(false));
    }

    #[test]
    fn tree_has_title_annotation_and_options() {
        let plain = help_text(false);
        let mut lines = plain.lines();
        assert!(lines.next().unwrap().starts_with("chora - "));
        assert!(plain.contains("\nCommands:\n"));
        assert!(plain.contains("\nAnnotation:\n"));
        assert!(plain.contains("\nOptions:\n"));
        assert!(plain.contains("<DATE> is today"));
        // Breathing room between command groups.
        for name in ["open", "event", "calendar", "completion"] {
            assert!(
                plain.contains(&format!("\n\n  • {name}")),
                "no blank line before `{name}`"
            );
        }
    }

    #[test]
    fn readme_lists_every_command() {
        const README: &str = include_str!("../README.md");
        fn collect(cmd: &clap::Command, names: &mut Vec<String>) {
            for sub in cmd.get_subcommands().filter(|sub| !sub.is_hide_set()) {
                names.push(sub.get_name().to_owned());
                collect(sub, names);
            }
        }
        let mut names = vec![String::from("completion")];
        collect(&Cli::command(), &mut names);
        assert!(!names.is_empty());
        for name in names {
            assert!(README.contains(&name), "README misses `{name}`");
        }
    }

    #[test]
    fn parses_calendar() {
        let cli = Cli::try_parse_from(["chora", "calendar"]).unwrap();
        assert!(matches!(cli.cmd, Some(Cmd::Calendar)));
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
        for name in ["completion", "completions"] {
            let cli = Cli::try_parse_from(["chora", name, "bash"]).unwrap();
            assert!(matches!(
                cli.cmd,
                Some(Cmd::Completion {
                    shell: CompletionShell::Bash
                })
            ));
            let cli = Cli::try_parse_from(["chora", name, "fish"]).unwrap();
            assert!(matches!(
                cli.cmd,
                Some(Cmd::Completion {
                    shell: CompletionShell::Fish
                })
            ));
        }
    }

    #[test]
    fn completion_rejects_invalid_shell() {
        for args in [
            &["chora", "completion"][..],
            &["chora", "completion", "zsh"][..],
            &["chora", "completion", "bash", "extra"][..],
        ] {
            assert!(Cli::try_parse_from(args).is_err(), "{args:?}");
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
                action: None,
                ..
            })
        ));
        let cli = Cli::try_parse_from(["chora", "event"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Event {
                confirm: false,
                date: None,
                action: None,
                ..
            })
        ));
    }

    #[test]
    fn parses_event_direct_flags() {
        let cli = Cli::try_parse_from([
            "chora",
            "event",
            "--date",
            "2026-11-20",
            "--title",
            "Standup",
            "--time",
            "10:00",
            "--duration",
            "30m",
            "--body",
            "Agenda.",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Event {
                title: Some(_),
                time: Some(_),
                duration: Some(_),
                body: Some(_),
                ..
            })
        ));
    }

    #[test]
    fn parses_help_and_skill_alias() {
        for name in ["help", "skill"] {
            let cli = Cli::try_parse_from(["chora", name]).unwrap();
            assert!(matches!(cli.cmd, Some(Cmd::Help)));
        }
    }

    #[test]
    fn skill_help_is_plain_and_short() {
        let text = render_skill_help();
        // Dynamic: every allowed command path is listed, banned ones are not.
        for path in [
            "chora event",
            "chora event show",
            "chora note",
            "chora note add",
            "chora note override",
            "chora note show",
            "chora note headers",
            "chora index",
            "chora index events",
        ] {
            assert!(text.contains(path), "skill misses `{path}`");
        }
        for banned in [
            "chora open",
            "chora calendar",
            "chora settings",
            "chora init",
        ] {
            assert!(
                !text.contains(banned),
                "skill lists banned `{banned}` as a command"
            );
        }
        assert!(text.contains("Forbidden"));
        assert!(
            !text.contains("--confirm --title"),
            "skill advertises --confirm"
        );
        assert!(text.contains("--max-access"));
        assert!(!text.contains('\u{1b}'));
        assert!(text.lines().count() < 60);
    }

    #[test]
    fn max_access_defaults_to_restricted() {
        use crate::index::AccessLevel;
        let cli = Cli::try_parse_from(["chora", "calendar"]).unwrap();
        assert_eq!(cli.max_access, AccessLevel::Restricted);
        let cli = Cli::try_parse_from(["chora", "--max-access", "open", "calendar"]).unwrap();
        assert_eq!(cli.max_access, AccessLevel::Open);
        assert!(Cli::try_parse_from(["chora", "--max-access", "staff", "calendar"]).is_err());
    }

    #[test]
    fn parses_note_subcommands() {
        let cli = Cli::try_parse_from(["chora", "note", "show", "biletmir", "--json"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Note {
                action: NoteAction::Show { json: true, .. }
            })
        ));
        let cli = Cli::try_parse_from(["chora", "note", "show"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Note {
                action: NoteAction::Show {
                    dirname: None,
                    json: false,
                    ..
                }
            })
        ));
        let cli = Cli::try_parse_from([
            "chora",
            "note",
            "show",
            ".",
            "--path",
            "now.md",
            "--path",
            "docs/a.md",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Note {
                action: NoteAction::Show {
                    dirname: Some(_),
                    path,
                    json: false,
                }
            }) if path == vec!["now.md".to_owned(), "docs/a.md".to_owned()]
        ));
        let cli = Cli::try_parse_from([
            "chora", "note", "add", "biletmir", "--path", "main.md", "--under", "Tasks",
        ])
        .unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Note {
                action: NoteAction::Add { .. }
            })
        ));
        let cli =
            Cli::try_parse_from(["chora", "note", "override", "biletmir", "--path", "main.md"])
                .unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Note {
                action: NoteAction::Override { .. }
            })
        ));
        let cli = Cli::try_parse_from(["chora", "note", "headers", "biletmir"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Note {
                action: NoteAction::Headers { .. }
            })
        ));
        let cli = Cli::try_parse_from(["chora", "index", "--json"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Index {
                json: true,
                action: None
            })
        ));
    }

    #[test]
    fn parses_index_events() {
        let cli = Cli::try_parse_from(["chora", "index", "events"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Index {
                json: false,
                action: Some(IndexAction::Events { json: false })
            })
        ));
        let cli = Cli::try_parse_from(["chora", "index", "events", "--json"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Index {
                action: Some(IndexAction::Events { json: true }),
                ..
            })
        ));
        let cli = Cli::try_parse_from(["chora", "index", "--json", "events"]).unwrap();
        assert!(matches!(
            cli.cmd,
            Some(Cmd::Index {
                json: true,
                action: Some(IndexAction::Events { .. })
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
