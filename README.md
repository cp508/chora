# chora

Lightweight CLI organizer: markdown notes, day events, git sync. No daemons.

## Start

```sh
just release
```

The binary is at `./target/release/chora`. Then:

```sh
chora init && $EDITOR ~/.config/chora/config.toml
```

`init` creates only the config (`~/chora/` is the default storage).
Edit the config, then run any command — the storage appears on first use.

## Storage

A plain git repository, safe to poke by hand:

- `notes/*.md` — free-form notes. Never parsed, just markdown.
- `events/YYYY-MM-DD.md` — day files. `##` sections with valid `Time:`/`Duration:` are events.
- `events/repeatable/<weekday>.md` — repeatable events (`monday.md` … `sunday.md`). Same section format as day files. Created and edited only via `chora open event`; shown by `chora event show --date`.
- `events/backlog/` — archive of old day files (see `manage_old_event`).
- `.gitignore` is created automatically when missing.

## Commands

Bare `chora` prints the whole tree.

- `init` — bootstrap the config. No flags, never touches the storage.
- `open <note|event>` — open a directory in the dirs-opener. On exit, in the background: pull (rebase), commit, push. The shell is back instantly, the log is `sync.log` (see below).
- `event --date <DATE>` — append an event stub to the day file and open it in the file opener. Sync runs in the background after closing.
- `event --tui [--confirm]` — TUI form for creating an event. With `--confirm`, opens the day file after submit.
- `event show [--date <DATE>] [--json]` — show events: one day plus its repeatables with `--date`, all days in a row without. `--json` prints an array for scripts.
- `completion <bash|fish>` — print shell completions to stdout.

`<DATE>` is `today`, `tomorrow`, `YYYY-MM-DD`, or `DD.MM.YYYY` (e.g. `20.05.2026`).

### Event format

Only sections like this are parsed (`example/` mirrors the storage layout:
`example/events/2026-11-20.md`, `example/events/repeatable/monday.md`,
`example/notes/vacation.md`):

```md
## Title

Time: 9:00
Duration: 10m

Arbitrary context.
```

- `Time:` — `HH:MM` (the TUI also accepts shorthands: `930`, `9`, `now`, `+30`).
- `Duration:` — `10s`, `45m`, `2h`, `1h30m`.
- Sections without valid `Time:`/`Duration:` are plain day text, skipped silently.
- Day output is sorted by time; repeatables are mixed in with no markers.

### Repeatable events

`events/repeatable/monday.md` shows every Monday in `event show --date <that-monday>`. The file name is the lowercase English weekday: `monday` … `sunday`. Files with other names are invisible to chora. There is no dedicated command: create and edit them via `chora open event`. The old-day sweep (`manage_old_event`) never touches `repeatable/`.

## Git sync

- No network in the foreground ever: the opener starts instantly, the shell is back instantly. Everything networked runs in a detached child (`setsid`) doing commit → pull (rebase) → push.
- Child log: `<data-dir>/chora/sync.log` (usually `~/.local/share/chora/sync.log`); every run is separated by a timestamp header.
- Conflicts are never auto-resolved: the error lands in `sync.log` tagged `diverged`, the repository stays mid-rebase — fix it by hand inside the storage (`git rebase --abort` / `--continue`).
- First run on a new device: `fetch` + `checkout` of the remote branch before `.gitignore` is created (otherwise checkout would trip over the untracked file).
- Concurrent runs serialize on the `.git/chora-sync.lock` lock (a file holding a PID; a dead holder is detected via `/proc`, a live one is waited on for up to 60 seconds).
- The first push sets upstream (`-u`); afterwards plain manual `git pull`/`git push` work.

## Config

`~/.config/chora/config.toml`:

```toml
[main]
chora_dir = "~/chora"
editor = ""      # opener for files, e.g. "hx". Empty = $EDITOR.
open_dirs = ""   # opener for directories, e.g. "yazi", "sh -c 'nvim $(fzf)'". Empty = $EDITOR.
auto_commit = 0  # auto-commit while the editor is open, in minutes. 0 = off. Local "auto: ..." commits only; the push happens on exit.
manage_old_event = "nothing"  # old day files on every event action: delete, backlog (= into events/backlog/), nothing.
remote_url = ""  # SSH remote. Empty = local-only git.
```

After upgrades, re-run `chora init`: missing keys are appended at the bottom under `# NEW`, your values are kept.

## Develop

```sh
just qa      # fmt + clippy + tests
just debug   # debug build
just release # release build
```
