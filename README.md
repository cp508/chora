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

Bare `chora` prints the whole tree (through `less -R` on a terminal, colors kept).

- `init` — bootstrap the config. No flags, never touches the storage.
- `open <note|event|index>` — open a directory (or `index.md`) in the opener. On exit, in the background: pull (rebase), commit, push. The shell is back instantly, the log is `sync.log` (see below). `open index` edits the human-owned `index.md`.
- `note add <DIR> --path <FILE> [--under <HEADER>]` — append stdin markdown to a note file (creates it when missing). With `--under` (`Header` or `Parent/Child`), appends at the end of that section. `<DIR>` `.` = notes root.
- `note override <DIR> --path <FILE>` — rewrite a note file with stdin markdown (creates it when missing).
- `note show [<DIR>] [--path <FILE>]... [--json]` — all dirs with `security`/`about` (no bodies), or one dir / `.` (notes root) recursive with bodies, optionally scoped by `--path`. Unknown `<DIR>` errors and prints the all-dirs listing. `--json` prints an object for scripts.
- `note headers <DIR> [--path <FILE>]...` — `#`-header tree with file names and line numbers, recursive.
- `index [--json]` — show `index.md` context + `security`/`about` tree. Warnings go to stderr.
- `event --date <DATE> --title <TITLE> --time <TIME> --duration <DUR> [--body <TEXT>] [--confirm]` — create an event at once, no editor. `--date` defaults to today. Sync runs in the background.
- `event --date <DATE>` — append an event stub to the day file and open it in the file opener. Sync runs in the background after closing.
- `event [--confirm]` — TUI form for creating an event. With `--confirm`, opens the day file after submit.
- `event show [--date <DATE>] [--json]` — show events: one day plus its repeatables with `--date`, all days in a row without. `--json` prints an array for scripts.
- `calendar` — read-only month grid with event markers. Arrows/hjkl move, Enter opens the day, `t` jumps to today, `q` quits. Day-file events use `[calendar] day_color`, weekday repeatables use `repeat_color` (unless `show_repeat = false`).
- `settings` — interactive TUI editor for all settings in `~/.config/chora/config.toml`.
- `help` (`skill`) — short machine-readable usage for AI agents (always printed directly, never paged).
- `completion <bash|fish>` — print shell completions to stdout.

`<DATE>` is `today`, `tomorrow`, `YYYY-MM-DD`, or `DD.MM.YYYY` (e.g. `20.05.2026`).
`<TIME>` is `HH:MM` (e.g. `09:30`, `9:00`) or `H` (e.g. `9` = `09:00`).
`<DUR>` is `30m`, `2h20m`, `2h 20m`, `2:00` (= 2h), `1:30` (= 1h30m), `10s`.

Pipe note bodies via stdin, e.g. `printf '# T\n' | chora note override biletmir --path main.md`.
`--path` is relative inside `notes/<DIR>` (`main.md`, `worker-task/high.md`), always `.md`.
Empty stdin is an error. Writes sync in the background like events.

## Notes and index

`notes/<DIR>/...md` hold agent-readable markdown, nested dirs allowed.
Loose `notes/*.md` files are shown under the `(files)` group.

`<storage>/index.md` is HUMAN-owned: agents must never write it
(only `chora open index`, by a human). Format:

```md
# Context

English description of all notes.

## Notes

### biletmir
security: restricted
about: ticket marketplace notes

#### worker-task/high.md
security: critical
about: production passwords
```

- `###` names a top-level dir; deeper `#` = deeper path
  (`#### guides` + `##### install.md` under `### docs` = `docs/guides/install.md`).
- Missing entry = `open` + empty `about`. Bad `security` = hard error
  (a typo must not silently downgrade a note to `open`).
  Children without `security` inherit their parent's.
  `security` inherits (longest-prefix match, `open < restricted < critical`),
  but `about` never flows down: an h3 `about` stays on its own dir.
  Empty `about` falls back to the file's own h1 (`# Guide` → `Guide`);
  an explicit `about` in `index.md` always wins.
- `--max-access <open|restricted|critical>` (global flag, default `restricted`)
  gates `note show`, `note headers`, `note add`, `note override` and `chora index`:
  `open` sees/writes only `open`, `restricted` sees/writes `open`+`restricted`, `critical` sees/writes all.
  Events have no levels.
- nvim completion for `security:` levels (`open`, `restricted`, `critical`)
  via blink-cmp: `editors/nvim/chora-index.lua` is a blink source module —
  register it as a provider (shows in the Tab menu on `security:` lines
  inside `index.md`, nowhere else).

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

- `Time:` — `HH:MM`.
- `Duration:` — `30m`, `2h20m`, `2h 20m`, `2:00`, `1h30m`, `10s`.
- Sections without valid `Time:`/`Duration:` are plain day text, skipped silently.
- Day output is sorted by time; repeatables are mixed in with no markers.

### Repeatable events

`events/repeatable/monday.md` shows every Monday in `event show --date <that-monday>`. The file name is the lowercase English weekday: `monday` … `sunday`. Files with other names are invisible to chora. There is no dedicated command: create and edit them via `chora open event`. The old-day sweep (`manage_old_event`) never touches `repeatable/`.

## Git sync

- No network in the foreground ever: the opener starts instantly, the shell is back instantly. Everything networked runs in a detached child (`setsid`) doing commit → pull (rebase) → push.
- Child log: `<data-dir>/chora/sync.log` (usually `~/.local/share/chora/sync.log`); every run is separated by a timestamp header.
- Conflicts are never auto-resolved: the error lands in `sync.log` tagged `diverged`, the repository stays mid-rebase — fix it by hand inside the storage (`git rebase --abort` / `--continue`).
- First run on a new device: `fetch` + `checkout` of the remote branch before `.gitignore` is created (otherwise checkout would trip over the untracked file).
- Concurrent runs serialize on the `.git/chora-sync.lock` kernel file lock.
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

[calendar]
day_color = "green"  # color for events recorded on that date. Terminal palette: black, red, green, yellow, blue, magenta, cyan, white, gray, darkgray, lightred, lightgreen, lightyellow, lightblue, lightmagenta, lightcyan.
repeat_color = "blue"  # color for weekday repeatables. Same palette.
show_repeat = true  # mark weekday repeatables on the calendar. false = day events only.
fixed_cells = false  # fixed 6x3 cells (square on typical fonts) instead of full-width stretch.
header = "%A, %d.%m.%Y"  # strftime format of the selected date, top-left of the calendar. NONE hides the header row.
show_today = true  # today hint, top-right, only when looking at another date.
```

After upgrades, re-run `chora init`: missing keys are appended at the bottom under `# NEW`, your values are kept.

## Develop

```sh
just qa      # fmt + clippy + tests
just debug   # debug build
just release # release build
```

Manual check of every new feature in a sandbox (never touch real `~/chora`):

```sh
mkdir -p /tmp/chora-debug
DEBUG_DIR=/tmp/chora-debug chora <args for the feature>
```

`DEBUG_DIR` wins over `--dir`, which wins over `chora_dir` from the config.
