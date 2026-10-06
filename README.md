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

`init` creates the config (`~/chora/` is the default storage).
Edit the config, then run any command — the storage appears on first use.

## Commands

Run `chora` with no arguments to see the full tree.

- `init` — bootstrap the config. No flags.
- `open <notes|events>` — open a dir once, commit on exit.
- `event --date <DATE>` — append an event stub, open the day file.
- `event --tui [--confirm]` — TUI form for creating an event.
- `show --date <DATE> [--json]` — show the events of a day.

`<DATE>` is `today`, `tomorrow`, or `YYYY-MM-DD`.

## Config

`~/.config/chora/config.toml`:

```toml
[main]
chora_dir = "~/chora"
editor = ""      # opener for files, e.g. "hx". Empty = $EDITOR.
open_dirs = ""   # opener for dirs, e.g. "yazi", "sh -c 'nvim $(fzf)'". Empty = $EDITOR.
manage_old_event = "nothing"  # passed day files on event actions: delete, backlog (= move to events/backlog/), nothing.
remote_url = ""  # SSH remote. Empty = local-only git.
```

## Develop

```sh
just qa      # fmt + clippy + tests
just debug   # debug build
just release # release build
```
