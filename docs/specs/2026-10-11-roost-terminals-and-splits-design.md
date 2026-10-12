# Roost: terminals and splits (addendum)

Date: 2026-10-11
Status: approved in brainstorming, pending written-spec review
Builds on: `2026-10-11-roost-design.md` (v1). Anything not changed here stays as v1 describes it.

## Goal

Two changes:

- A session is a **terminal**: your login shell in a folder, not Claude itself. Claude is something you run in it, and Roost still tracks it.
- The main area is a **free split layout**: panes can sit side by side or stacked, be resized, and be rearranged by dragging.

### Success criteria

- ⌘N opens a shell in the repo within a second. Typing `claude` in it lights up the row's status; exiting Claude returns the row to the shell state.
- ⌘D and ⌘⇧D split the focused pane; dragging a sidebar row or a pane header onto a pane's edge docks it there.
- After a quit and relaunch, the same layout comes back, every terminal reopens in its folder, and a terminal that was running Claude resumes that conversation.
- Agents' `roost new "task"` still produces a running Claude session.

### Out of scope

- Restoring shell scrollback or commands other than Claude (dev servers and so on) after a relaunch.
- Zooming one pane to fill the window.
- Multiple windows or saved layout presets.
- Tracking Claude started through an alias, `npx`, an absolute path, or a shell other than zsh whose startup files hide the shim.

## Sessions are terminals

### Launch

A new session spawns `$SHELL -l` in its folder. `$ROOST_SHELL` overrides the shell, which the tests use. The session's environment is v1's `child_env` plus:

| Variable | Value |
|---|---|
| `PATH` | `<support>/bin` first, then v1's PATH |
| `ROOST_BIN` | absolute path of the running `roost` binary |
| `ROOST_REAL_CLAUDE` | the `claude` Roost resolved at startup |
| `ROOST_HOOKS` | `<support>/hooks.json` |
| `ROOST_BLURB` | v1's system-prompt note |
| `ROOST_SHIM_DIR` | `<support>/bin` |
| `ZDOTDIR` (zsh only) | `<support>/zsh` |
| `ROOST_ORIG_ZDOTDIR` (zsh only) | the user's own `ZDOTDIR`, if it was set |

`<support>` is `~/Library/Application Support/Roost`. Roost rewrites `bin/claude` and `zsh/.zshenv` on every launch, as it already does for `hooks.json`.

### The `claude` shim (`<support>/bin/claude`, `sh`)

```sh
#!/bin/sh
# Roost: run the real claude with Roost's hooks, as this same process.
real=$ROOST_REAL_CLAUDE
if [ -z "$real" ] || [ ! -x "$real" ]; then
  # Roost found no claude at startup: run the next one on PATH, untracked.
  shim_dir=$(cd "$(dirname "$0")" && pwd)
  PATH=$(printf '%s
' "$PATH" | tr ':' '
' | grep -vxF "$shim_dir" | paste -sd: -)
  export PATH
  exec claude "$@"
fi
# A claude started by Claude itself (its Bash tool) must not touch this row.
if [ -n "$CLAUDECODE" ] || [ -z "$ROOST_BIN" ] || [ -z "$ROOST_HOOKS" ]; then
  exec "$real" "$@"
fi
case "$1" in
  -v|--version|-h|--help) exec "$real" "$@" ;;
esac
if printf '%s' "$1" | grep -Eq '^[a-z][a-z-]*$'; then
  exec "$real" "$@"
fi
printf '{"pid":%d}' $$ | "$ROOST_BIN" hook ClaudeStart
exec "$real" --settings "$ROOST_HOOKS" --append-system-prompt "$ROOST_BLURB" "$@"
```

- A single lowercase first word is treated as a subcommand and passed through untouched. This keeps `claude mcp list` and future subcommands working. Roost's own typed commands always use `--`, so this only affects `claude hi` typed by hand.
- `exec` keeps the pid, so the pid reported in `ClaudeStart` is Claude's own.
- A `claude` started by Claude itself (`CLAUDECODE` set, e.g. from its Bash tool) runs untracked, so it can't overwrite its parent row.
- If Roost found no `claude` at startup, `ROOST_REAL_CLAUDE` is empty and the shim runs the next `claude` on PATH (its own directory removed), untracked.

### zsh integration (`<support>/zsh/.zshenv`)

```zsh
# Roost: hand zsh back to the user's own startup files, keep the claude shim first on PATH,
# and tell Roost once the first prompt is up.
if [[ -n $ROOST_ORIG_ZDOTDIR ]]; then ZDOTDIR=$ROOST_ORIG_ZDOTDIR; else unset ZDOTDIR; fi
[[ -f ${ZDOTDIR:-$HOME}/.zshenv ]] && source ${ZDOTDIR:-$HOME}/.zshenv
_roost_shim_first() { path=("$ROOST_SHIM_DIR" ${path:#$ROOST_SHIM_DIR}); }
_roost_ready() {
  precmd_functions=(${precmd_functions:#_roost_ready})
  "$ROOST_BIN" hook ShellReady </dev/null >/dev/null 2>&1
}
typeset -ga precmd_functions preexec_functions
precmd_functions+=(_roost_shim_first _roost_ready)
# preexec runs after every precmd hook (mise, direnv…) and before the command is looked up.
preexec_functions+=(_roost_shim_first)
```

After this file, zsh reads `.zprofile`, `.zshrc` and `.zlogin` from the user's own directory as normal. The hook puts the shim back first before every prompt and again in `preexec`, which runs after every `precmd` hook (mise, direnv and the like) and before the command is looked up. So PATH changes made in `.zshrc`, by those tools, or by hand can't hide it.

Other shells only get the PATH prepend.

### Status

A new status, `shell`, is shown as a square dim dot: the terminal is running but Claude isn't.

| Event | Status |
|---|---|
| shell spawned | `shell` |
| `ClaudeStart` (shim, `{"pid":N}`) | `starting`; remembers the pid |
| `SessionStart` (source ≠ `compact`) | `idle`; `claudeRunning = true`; updates `claudeId` and `workdir` |
| `UserPromptSubmit`, `PostToolUse` | `working` |
| `Notification` (permission / question) | `needs_you` |
| `Stop` | `idle` |
| `SessionEnd`, or the remembered pid is gone | `shell`; `claudeRunning = false` |
| the shell exits | `exited` |

- Roost checks remembered pids every 2 s with `kill(pid, 0)`. A Claude killed with Ctrl-C or a crash (no `SessionEnd`) therefore returns its row to `shell` within 2 s.
- The v1 trust-dialog rule still applies while a row is `starting`.
- Attention notifications are unchanged. The `idle` → `shell` change never notifies.

### Typing commands into a terminal

Roost queues the command and types it, plus `\r`, once the shell is ready: zsh reports its first prompt through the `ShellReady` hook installed by Roost's `.zshenv`, and other shells get it after 2 s. The prompt itself is never typed: Roost writes it to `<support>/prompts/<row>.txt` and types a short command that reads it. A terminal line holds only about 1 KB, and control characters typed into a shell act as keystrokes. The file is deleted when Claude reports in (`ClaudeStart`) or when the row is closed. Prompts always follow `--`, so a one-word or dash-leading prompt still goes through the shim's tracked path. Arguments are single-quoted (`'it'\''s'`).

| Trigger | Typed |
|---|---|
| ⌘N or `roost new` with a prompt | `claude -- "$(cat '<support>/prompts/<row>.txt')"` |
| ⌥↵ in the sheet, or `roost new --worktree` | `claude -w '<name>' -- "$(cat '<support>/prompts/<row>.txt')"` |
| restore when `claudeRunning` | `cd '<workdir>' && claude --resume '<claudeId>'` |
| ⌘O on a past conversation | the same `cd … && claude --resume …`, in a new terminal |

An empty prompt types nothing; you get a plain shell.

### State changes (`state.json`, `version: 2`)

- Each row drops `worktree` and gains `claudeRunning: bool`.
- `repo` stays as the folder the terminal starts in.
- New top-level fields: `layout` (the split tree, below) and `focus` (the id of the focused pane's row).
- A v1 file loads with defaults: `claudeRunning` is true for rows that weren't exited, so their conversations resume, and `layout` is a single pane showing the most recent row.

### Restore

- Rows not marked `exited` come back `asleep`.
- Every row shown in the restored layout spawns at once; the others spawn when first selected. Spawning means the shell, plus the resume command when `claudeRunning` is set.
- Rows the layout references that no longer exist are dropped from the layout.

### CLI changes

- **`roost new "task" [--worktree NAME] [--cwd DIR] [--name NAME]`** creates a terminal and types the Claude command. It does not change the window layout; the row appears in the sidebar.
- **`roost send`** works only while Claude has the terminal, meaning Claude's pid leads the PTY's foreground process group. Status alone can be stale, for example after Ctrl-Z, and the paste would then run in the shell. While a claude command is queued, or was typed moments ago, `send` waits up to 15 s for it. For an `asleep` row with `claudeRunning`, Roost restores it first and waits up to 15 s for `SessionStart`, as in v1. For a `shell` row it errors with `X isn't running Claude`.
- **`roost stop`** kills the terminal: shell and Claude together. Closing sends SIGHUP to the terminal's process group, as a closing terminal window does; interactive shells ignore SIGTERM, and HUP lets zsh save its history and end its jobs. SIGKILL follows after 3 s.
- While Roost is quitting it ignores hooks and the pid watcher, so the shells' deaths don't clear `claudeRunning` before the relaunch.
- `SessionEnd` with reason `clear` (from `/clear`) is ignored: the same Claude process keeps running.

### Removed

- The "Couldn't resume" card and `start_fresh`: a failed resume just leaves you at the shell prompt.
- `select_session`'s resume-on-select is replaced by spawn-on-select, as described under Restore.

## Layout

### Model

`layout` is a binary tree:

```json
{ "split": "row", "ratio": 0.5,
  "a": { "pane": "<rowId>" },
  "b": { "split": "col", "ratio": 0.6, "a": { "pane": "<rowId>" }, "b": { "pane": "<rowId>" } } }
```

- `row` means side by side; `col` means stacked.
- `ratio` is the share of space `a` gets, kept within 0.1–0.9.
- A row appears in at most one pane. An empty layout is `null`, and the main area then says "⌘N to start a session".
- The UI saves `layout` and `focus` through a new command, `save_layout { layout, focus }`, after every change (debounced 300 ms). The engine stores both as-is and returns them from `boot`.

### Behaviour

- **Sidebar.**
  - It lists every terminal. Rows that are on screen in a pane show their name at full brightness; rows that aren't are dimmed.
  - Clicking a row, ⌘1–9 or ⌘J focuses the row's pane if it's on screen. Otherwise the row replaces what the focused pane shows; the replaced terminal keeps running.
- **Pane header** (22 px): status dot, name, and what's running (`claude` or the shell's name). It is the drag handle, and double-clicking it renames.
- **Splitting.** ⌘D splits the focused pane to the right and ⌘⇧D splits it downward, each with a new terminal in the focused row's folder (no sheet, no prompt). The new pane gets focus.
- **New session.** ⌘N opens the sheet; the new terminal goes into the focused pane, or becomes the only pane when the layout is empty.
- **Drag & drop.**
  - Sources: a sidebar row or a pane header.
  - Over a pane, the cursor position picks a zone: the outer 25% on each side is left, right, top or bottom; the middle is centre.
  - An edge zone splits the target on that side with the dragged terminal. Centre puts the dragged terminal in the target pane, and if the dragged terminal came from another pane the two swap.
  - A highlight shows the zone. Dropping a pane onto itself does nothing.
- **Resizing.** Dividers are 1 px lines with a 6 px grab area. Dragging changes `ratio`, and double-clicking a divider resets it to 0.5. Terminals refit when resizing ends, and at most every 50 ms during a drag.
- **Closing.**
  - ⌘W kills the focused terminal (asking first if Claude is `working`) and removes its pane; its sibling takes the space.
  - ⌘⇧W removes the pane only; the terminal keeps running in the sidebar.
- **Focus.** ⌘⌥←→↑↓ moves focus to the nearest pane in that direction, measured between pane centres. This replaces v1's ⌘⌥↑↓ previous/next session. The focused pane gets a 1 px `#3a3a44` outline, and its terminal has keyboard focus.

### Keys

These change or add to v1's table:

| Key | Action |
|---|---|
| ⌘D / ⌘⇧D | Split the focused pane right / down with a new terminal |
| ⌘W | Close the focused terminal and its pane |
| ⌘⇧W | Remove the focused pane, keep its terminal |
| ⌘⌥←→↑↓ | Move focus between panes |

## Code layout

- **`src/layout.ts`** (new): pure functions over the tree.
  - `panes(tree) -> ids` and `prune(tree, ids)`.
  - `split(tree, targetId, newId, side)` and `remove(tree, id)`.
  - `replace(tree, oldId, newId)`, `swap(tree, a, b)` and `move(tree, id, targetId, side)`.
  - `zoneAt(rect, x, y)` and `setRatio(tree, path, ratio)`.
  - `neighbor(rects, fromId, dir)`, which takes pane rectangles and returns an id or null.
- **`src/panes.ts`** (new): renders the tree as nested flex containers with dividers and headers, moves xterm elements into pane bodies (reparenting, then fit), and handles drag & drop and divider dragging.
- **`terminals.ts`** shows any number of terminals at once instead of one, and gets `mount(id, el)` in place of `show`.
- **`main.ts`, `sidebar.ts` and `sheet.ts`** are updated for the behaviour above.
- **Rust:**
  - `engine.rs`: terminal spawn, the typed commands, the shell status, pid watching, layout storage and the v2 state.
  - `shellenv.rs`: the session environment additions.
  - New `integration.rs`: writes `bin/claude` and `zsh/.zshenv`.
  - `state.rs`: v2 schema, `Status::Shell`, and the `ClaudeStart`/`SessionEnd` mapping.
  - `cli.rs`: `send` refuses shell rows.

## Testing

**Rust unit tests:**
- Shim: with a fake `claude` that prints its args, `claude 'fix it'` gets `--settings` and `--append-system-prompt` added; `claude mcp list` and `claude --version` don't; and a `ClaudeStart` carrying the shim's pid reaches a test socket.
- zsh integration: run `zsh -i -c 'command -v claude'` with `ZDOTDIR=<support>/zsh` and a fake `HOME` whose `.zshrc` prepends a directory holding another `claude`. After a prompt cycle (`precmd`), the shim must still win. Skipped when zsh isn't installed.
- State: v1 → v2 migration and the `Status::Shell` mapping.

**Flow tests** (fake shell `/bin/sh`, fake `claude` reached through the shim):
- A new session is `shell`; typing `claude 'x'` moves it shell → starting → idle → working → … → idle, and `SessionEnd` returns it to `shell`.
- A killed fake Claude returns the row to `shell` within 3 s.
- Restore with `claudeRunning` types `cd … && claude --resume …`.
- `send` to a `shell` row errors.
- `roost new` with a prompt types the Claude command.

**UI:**
- `bun test src/layout.test.ts` covers split, remove, move, swap, prune, `zoneAt` and `neighbor`, including a move onto its own pane being a no-op and removing the last pane giving `null`.
- Drag, drop, splitting and resizing are checked in the Chrome harness, with a manual checklist for the native window.
