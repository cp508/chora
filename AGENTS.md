# AGENTS.md — chora

Lightweight CLI organizer: markdown notes, day events, git sync. No daemons.
Single user, one or two machines. Storage is a plain git repo (`~/chora` by default).

## Layout

- `src/main.rs` — thin dispatcher; logic lives in the library.
- `src/cli.rs` — clap schema (single source for help tree, completions, `help`/`skill` text).
- `src/event.rs` — `##` sections <-> `Event`; `parse_day` sorts by time.
- `src/store.rs` — `events/`, `notes/` layout, day files, sweeps.
- `src/index.rs` — `index.md` parse (soft) + effective `security`/`about` per note path.
- `src/note.rs` — notes helpers: safe paths, header extraction, show/index rendering.
- `src/fsx.rs` — atomic writes (`write_atomic`); every user-file overwrite goes through it.
- `src/edit.rs` — checked edits for library users (mobile): note save/toggle against a `base` text,
  event add/update/delete by value (`EventFile::{Day, Weekly}`); a sync in between is `ChoraError::Changed`.
- `src/manage.rs` — note files/dirs for library users: create (trailing `/` = dir), rename, trash to
  `trash/<YYYY-MM-DD>/`, purge after 30 days.
- `src/conflict.rs` — Syncthing `.sync-conflict-` copies (list/resolve) and a metadata change `stamp`.
- `src/config.rs`, `src/git.rs`, `src/sync.rs`, `src/tui.rs`, `src/calendar.rs`, `src/output.rs`.
- `tests/e2e.rs` — end-to-end via built binary with isolated `HOME`.

## Storage

- `notes/<dirname>/...md` — agent-readable notes, nested allowed. Loose `notes/*.md`
  are shown under the `(files)` group.
- `events/YYYY-MM-DD.md` — day files, `##` sections with `Time:`/`Duration:`.
- `events/repeatable/<weekday>.md`, `events/backlog/`.
- `<root>/index.md` — human-owned context + `security`/`about` per notes path.
  Agents must NEVER write it (only `chora open index`, by a human).
  Exception: `DEBUG_DIR` sandbox, where the agent may edit it freely.

## index.md rules (soft parse)

- `# Context` — English description of all notes. `## Notes` — entries root.
- `### <dirname>` — top-level dir entry. `####`+ — children; deeper `#` = deeper path
  (`#### guides` + `##### install.md` under `### docs` = `docs/guides/install.md`).
- Under each header: `security: open|restricted|critical`, `about: <text>`.
- Missing entry = `open` + empty `about`. Bad `security` value = hard error.
- Children without `security` inherit their parent's.
- Effective level = longest-prefix match. Order: `open < restricted < critical`.
- `about` never flows down (exact entry only); empty `about` falls back to
  the file's h1; explicit `about` in `index.md` wins.

## Security

- `--max-access <open|restricted|critical>` (global flag, default `restricted`):
  clearance for `note show`, `note headers`, `note add`, `note override`, `chora index`.
  `open` sees/writes only `open`; `restricted` sees/writes `open`+`restricted`; `critical` sees/writes all.
- Events have no levels.
- `chora index` hides entries above clearance (warnings for hidden paths are hidden too).

## Dev workflow (mandatory)

1. Smallest diff that fully works. No new deps for a few lines. No speculative flags.
2. New non-trivial logic (branch, parser, security, writes) leaves a unit test
   plus an e2e test when it touches user flows.
3. After EVERY new feature, verify the added command by hand in a sandbox:
   `mkdir -p /tmp/chora-debug`, then
   `DEBUG_DIR=/tmp/chora-debug chora <args you choose for this feature>`.
   Never touch real `~/chora` for checks. In the sandbox you may edit `index.md`.
   Sandbox needs an isolated config too: `HOME=/tmp/chora-dbg-home` (and make
   sure `XDG_CONFIG_HOME` is unset — it wins over `HOME`). Otherwise a fresh
   sandbox repo clones the real `remote_url` and background sync pushes test
   commits to the real remote.
4. Gate, in order: `cargo fmt --all -- --check`,
   `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test`
   (or `just qa`).
5. `DEBUG_DIR` wins over `--dir`, which wins over `chora_dir` from the config.
6. Keep in schema: new command/flag must appear in `README.md`,
   bare-`chora` tree (automatic via clap), `chora help`/`chora skill` text,
   and completions (automatic). The `readme_lists_every_command` test enforces README.
7. Replies to the user in Russian, short. Reference code as `path:line`.
