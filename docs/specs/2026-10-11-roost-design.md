# Roost: design

Date: 2026-10-11
Status: approved in brainstorming, pending written-spec review

## Goal

A macOS desktop app for running many Claude Code sessions side by side. It is modelled on Xirp (Spotify's internal agent manager), but it does only one thing: it manages terminals for one agent, Claude Code. It should look calmer than Xirp and add as little friction as possible. You open it, press ⌘N, type a prompt and press ↵, and from then on you only switch to a session when it needs you.

Xirp was used only as a reference for behaviour and architecture. Its source is private, and Roost uses none of it.

### Success criteria

- A new session starts from ⌘N with one line of typing and ↵. There is no sign-in, no project registration and no form.
- Every session shows a live status (working, needs you, idle) without being opened.
- ⌘J always lands on a session that is waiting for you.
- Quitting and reopening Roost brings back every session with its conversation intact.
- Agents inside Roost can spawn, list, message and stop sibling sessions through a `roost` CLI.

### Out of scope for v1

Other agents (Codex, Aider, etc.), a background daemon, sign-in, PR monitoring, transcript upload, a web or phone dashboard, a database, light theme, settings UI, split panes, `roost read` (screen scraping another session), code signing, notarization and auto-update. Builds are local (`cargo tauri build`, copied to /Applications).

## Decisions

| Topic | Decision |
|---|---|
| Form factor | Desktop app with its own window |
| Stack | Tauri 2 (Rust) + Vite + vanilla TypeScript + xterm.js; PTYs via `portable-pty` |
| Agents | Claude Code only |
| Persistence | No daemon. Sessions die when Roost quits and come back via `claude --resume` |
| Worktrees | Claude Code's own `-w <name>` flag; Roost runs no git commands |
| Status | Per-session Claude Code hooks passed with `--settings` |
| State | One JSON file; no database |
| Look | Direction A, "quiet sidebar" (see `mockups/look-options.html`, option A) |
| Package manager | bun (installed); Rust via rustup (not yet installed) |
| Bundle id | `dev.roost.app` |

## Architecture

```
┌───────────── Roost.app (one binary: Contents/MacOS/roost) ─────────────┐
│ UI (Vite + vanilla TS + xterm.js)                                      │
│   sidebar · one xterm per session, kept alive while hidden             │
│        ▲ PTY bytes (tauri::ipc::Channel)    │ invoke: write / resize   │
│ Rust core                                                              │
│   portable-pty: one `claude` child per live session                    │
│   state.json   ~/Library/Application Support/Roost/                    │
│   roost.sock   newline-delimited JSON  ◀── `roost hook …` and CLI      │
└────────────────────────────────────────────────────────────────────────┘
```

### One binary, two modes

`main.rs` checks `argv[1]` before starting Tauri. If it is a CLI command (`new`, `ls`, `send`, `stop`, `hook`), it runs as a socket client and exits without loading any window. Otherwise it starts the app.

On every launch the app makes sure `~/.local/bin/roost` is a symlink to its own binary. It creates the link if it is missing and repoints it if it points at another Roost binary. It never overwrites a file that isn't a Roost symlink; in that case it logs a warning.

### Files

All files live under `~/Library/Application Support/Roost/`:

- `state.json`: sessions and recent repos (schema below). Writes are atomic: write `state.json.tmp`, then rename.
- `roost.sock`: Unix socket. On start, an existing socket file is removed and rebound. A single-instance plugin guarantees only one app owns it.
- `hooks.json`: the Claude Code settings file passed with `--settings`. It is regenerated on every launch so it always holds the current absolute binary path.

### State schema

```json
{
  "version": 1,
  "recentRepos": ["/Users/me/Dev/petra-app"],
  "sessions": [
    {
      "id": "roost uuid, stable for the life of the row",
      "claudeId": "uuid Claude uses; updated on every SessionStart",
      "name": "fix-flaky-auth-test",
      "repo": "/Users/me/Dev/petra-app",
      "workdir": "/Users/me/Dev/petra-app/.claude/worktrees/fix-flaky-auth-test",
      "worktree": true,
      "parent": "roost uuid or null",
      "createdAt": "2026-10-11T17:00:00Z",
      "exited": false
    }
  ]
}
```

- `repo` is the folder the session was launched from. It is used for grouping and for `⌘O`.
- `workdir` starts equal to `repo` and is replaced by the `cwd` from each `SessionStart` hook. That is how Roost learns the worktree path.
- Live status is held in memory only and never persisted.

If `state.json` fails to parse, Roost renames it to `state.json.bak` and starts empty. Nothing is lost, because every conversation can still be reopened from `⌘O`.

## Sessions

### Environment

Apps started from Finder don't get the user's shell PATH. At startup Roost runs `$SHELL -lic 'printf __ROOST_ENV__; env -0'` with a 5 s timeout and parses everything after the marker. That makes the parse robust to shell startup noise. If the command fails or times out, Roost falls back to its own environment, with `/opt/homebrew/bin:/usr/local/bin:$HOME/.local/bin` prepended to PATH.

The `claude` binary is `$ROOST_CLAUDE_BIN` if set, otherwise `command -v claude` in the captured environment. If it is not found, the main pane shows: "Claude Code not found. Install it, or set ROOST_CLAUDE_BIN." and ⌘N is disabled.

Every child gets the captured env plus:

```
ROOST_SESSION=<roost id>
ROOST_SOCK=<socket path>
TERM=xterm-256color
COLORTERM=truecolor
```

### Launch

The child is spawned in a PTY sized to the visible terminal pane:

```
new:     claude --session-id <claudeId> -n <name> --settings <hooks.json>
                --append-system-prompt <blurb> [-w <name>] [<prompt>]      (cwd = repo)
resume:  claude --resume <claudeId> -n <name> --settings <hooks.json>
                --append-system-prompt <blurb>                             (cwd = workdir)
```

For a new session, `claudeId` is a fresh UUID.

The blurb is added to every session's system prompt, about 80 tokens:

> You are running inside Roost, a session manager. The `roost` CLI is on PATH: `roost new "<task>" [--worktree NAME] [--cwd DIR]` starts a sibling Claude session (you become its parent), `roost ls` lists sessions with status, `roost send <name> "<text>"` messages a session, `roost stop <name>` stops one. Use them only when the user asks for parallel work or delegation.

### Names

A session's name is a slug of the first four words of its prompt: lowercase `a-z0-9-`, at most 24 characters. An empty prompt gives `session`. Names are unique among live rows; a collision gets a `-2`, `-3`, ... suffix. A worktree session uses its name as the worktree name. ⌘⇧R and `roost` never rename a worktree, only the display name.

### Lifecycle

- **New**: ⌘N sheet or `roost new`. The row appears at once with status `starting`.
- **Quit Roost**: if any session is `working`, Roost shows one native confirm: "N agents are working. Quit anyway?" Then all children are killed. Rows stay in `state.json`.
- **Relaunch**: every non-exited row comes back as `asleep` (a hollow dim dot). It is resumed lazily, the first time it is selected, so starting Roost doesn't spawn a dozen Node processes at once. `roost send` to an asleep session resumes it first and waits for its `SessionStart` hook (up to 15 s) before writing.
- **Child exits** (Claude quits, crashes, or `roost stop`): the row becomes `exited` (struck-through name). If it exits within 3 s of a resume (missing worktree or transcript), the pane shows **Start fresh here** (a new session in the same `workdir`) and **Remove**.
- **Close (⌘W)**: kills the child if it is running and removes the row. It asks first only if the session is `working`. Roost never deletes worktrees or transcripts.
- **Stop**: `roost stop` sends SIGTERM to the child's process group and SIGKILL after 3 s. The row stays as `exited` until it is closed.

## Status

### Hooks

`hooks.json` registers one command per event. Each command is `<abs binary path> hook <Event>`:

| Event (matcher) | Status set |
|---|---|
| `SessionStart` | `idle` (also updates `claudeId` and `workdir` from the payload) |
| `UserPromptSubmit` | `working` |
| `PostToolUse` | `working` (this is what clears "needs you" after a permission is granted) |
| `Notification` (`permission_prompt\|elicitation_dialog`) | `needs_you` |
| `Stop` | `idle` |

The PTY exiting sets `exited`; before the first hook arrives, a row is `starting`; after a relaunch, rows are `asleep`. The Notification matcher values must be checked against the Claude Code hook docs during implementation. If they differ, every Notification maps to `needs_you`, except the idle-reminder type, which is ignored.

`roost hook <Event>` reads the hook JSON from stdin and identifies the row by `$ROOST_SESSION`. It sends `{"op":"hook","session":…,"event":…,"payload":{…}}` to `$ROOST_SOCK` with a 500 ms total timeout. It **always exits 0 with no output**, whether or not the socket answers. A hook can never block or alter Claude.

This was verified on 2026-10-11 against Claude Code 2.1.296: hooks passed with `--settings` fire, `ROOST_SESSION` reaches them, `--session-id` is honoured, and the payload includes `session_id`, `cwd` and `transcript_path`.

### Attention

When a session changes to `needs_you` or `idle` while the window is unfocused or that session isn't selected, Roost posts a macOS notification ("auth-fix needs you" or "auth-fix is done") through `tauri-plugin-notification`. Clicking it brings Roost to the front, and ⌘J then lands on the session. Routing a notification click straight to its session is deferred until the desktop plugin supports click callbacks.

The Dock badge (`Window::set_badge_count`) shows the number of `needs_you` sessions and clears at 0.

## CLI and socket protocol

### Commands

```
roost new "<prompt>" [--worktree NAME] [--cwd DIR] [--name NAME]   → prints name
roost ls [--json]                                                  → name, status, repo, parent
roost send <name|id> "<text>"
roost stop <name|id>
roost hook <Event>                                                 (internal)
```

- The socket path is `$ROOST_SOCK` if set, otherwise the default path.
- `new` defaults `--cwd` to the caller's working directory. If run inside a session (`$ROOST_SESSION` is set), that session becomes the parent. If the app isn't running, `new` launches it (`open -b dev.roost.app`) and polls for the socket for up to 5 s.
- `ls`, `send` and `stop` with no app running print `Roost isn't running` and exit 1.
- Error responses print `error` to stderr and exit 1.

### Protocol

Each request and each response is one JSON object per line:

```
→ {"op":"new","prompt":"…","cwd":"…","worktree":"name"|null,"name":"…"|null,"parent":"id"|null}
← {"ok":true,"id":"…","name":"…"}
→ {"op":"ls"}
← {"ok":true,"sessions":[{"id","name","status","repo","parent"}]}
→ {"op":"send","target":"…","text":"…","from":"id"|null}
→ {"op":"stop","target":"…"}
→ {"op":"hook","session":"…","event":"…","payload":{…}}
← {"ok":true} | {"ok":false,"error":"…"}
```

### Send

`send` writes the text into the target PTY:

1. `ESC[200~`, then the text, then `ESC[201~` (bracketed paste), so multi-line text is not submitted line by line.
2. After 50 ms, a `\r`.

When `from` is set, the text is prefixed with `[from <sender name>] `. If the target is mid-turn, Claude Code queues the message. A name or id that doesn't match returns `{"ok":false,"error":"no session named X"}`.

## UI

The look is direction A from `mockups/look-options.html`. The new-session sheet and keyboard map are in `mockups/new-session.html`.

### Window

- Dark only.
- Tokens: `bg #0f0f11`, `line #1e1e23`, `sel #1b1b20`, `text #d6d6dc`, `mute #6b6b76`, `amber #f5a524` (needs you), `blue #6ea8fe` (working), `green #4ade80`.
- Overlay title bar with the traffic lights inset and no title text.
- UI font: system sans. Terminal font: `"JetBrains Mono", "SF Mono", Menlo, monospace` at 12.5 px.

### Sidebar

The sidebar is 210 px wide and hairline-separated.

- Sessions are grouped by `repo` (full path); the label is its basename in small caps.
- Groups are ordered by the creation time of their first session; sessions within a group by creation time. A child is nested (indented, `└`) under its parent, and the group is decided by the root ancestor.
- Each row shows a status dot, the name, and either the age since the last status change or a `needs you` pill.
- Dots: `starting` and `asleep` are a hollow dim ring; `working` is a blue spinner ring; `needs_you` is a solid amber dot with a soft halo; `idle` is solid grey; `exited` is a grey dot with the name struck through.
- `+ New session ⌘N` is pinned to the bottom. Double-clicking a name renames it.

### Terminal pane

- Each session owns one xterm.js instance, in its own div, kept in the DOM and hidden with `display:none` when not selected. It uses the fit and webgl addons and has 10,000 lines of scrollback.
- On show: fit, send the resize to the PTY, focus.
- PTY output is coalesced in Rust and flushed to the UI at most every 16 ms per session.

### New-session sheet (⌘N)

- A centred floating sheet over the dimmed window with a multi-line prompt field.
- A repo chip defaults to the selected session's repo, else the most recent repo. ⌘P switches it to a recent repo or "Open folder…" through the native dialog.
- The derived name is shown live.
- ↵ starts the session; ⌥↵ starts it in a worktree (disabled, and greyed, when the repo isn't a git repo); ⇧↵ inserts a newline; esc cancels.

### Resume picker (⌘O)

- Lists `~/.claude/projects/<encoded path>/*.jsonl` for the current repo and its worktrees. Claude Code encodes a path by replacing `/` and `.` with `-`, and the match is any encoded directory that starts with the encoded repo path.
- Rows are newest first by mtime. Sessions already open in Roost are excluded.
- A row's title is the transcript's custom title entry if present, otherwise the first user message, truncated to 80 characters.
- A substring filter field sits at the top. ↵ adds the row as a session and resumes it.

### Keys

| Key | Action |
|---|---|
| ⌘N | New session |
| ⌘J | Select the next `needs_you` session after the current one, wrapping; no-op if none |
| ⌘1–9 | Select session by visual position |
| ⌘⌥↑ / ⌘⌥↓ | Previous / next session |
| ⌘O | Resume picker |
| ⌘P | Switch repo (in the sheet) |
| ⌘⇧R | Rename the selected session |
| ⌘W | Close the selected session |

Every other key goes to the terminal.

## Project layout

```
roost/
  index.html, package.json, vite.config.ts, tsconfig.json
  src/        main.ts (boot, keys), sidebar.ts, terminals.ts, sheet.ts, picker.ts, style.css
  src-tauri/  Cargo.toml, tauri.conf.json, capabilities/
    src/
      main.rs      argv dispatch: CLI or app
      app.rs       Tauri setup, commands, events, quit guard, badge, notifications
      state.rs     sessions, status transitions, names, state.json I/O
      pty.rs       spawn / resume / write / resize / kill, output coalescing
      sock.rs      socket server (app side) and client (CLI side), protocol types
      cli.rs       arg parsing and the five commands
      shellenv.rs  login-env capture, claude binary resolution
      past.rs      transcript discovery and titles for ⌘O
  tests/fake-claude.sh
  docs/specs/
```

### Tauri commands (UI → Rust)

`list_sessions`, `new_session {prompt, repo, worktree}`, `select_session {id, output: Channel}` (resumes it if asleep; the UI creates the Channel once per session and Rust streams PTY bytes into it), `write {id, data}`, `resize {id, cols, rows}`, `rename {id, name}`, `close {id}`, `list_past {repo}`, `resume_past {repo, claudeId}`, `pick_folder`, `is_git_repo {path}`.

### Events (Rust → UI)

`session_changed` (one row) and `sessions_changed` (the full list after an add or remove).

### Plugins

`tauri-plugin-single-instance`, `tauri-plugin-notification`, `tauri-plugin-dialog`.

## Testing

Rust unit tests:

- the hook event → status mapping, including the Notification matcher fallback;
- the launch-args builder for new, new with worktree and resume;
- name slugging and collision suffixes;
- the `state.json` round trip, atomic write and corrupt-file recovery;
- protocol encode/decode, plus a round trip over a socket in a temp dir;
- transcript title extraction from a fixture `.jsonl`;
- login-env parsing with noise before the marker.

Integration test (Rust, `#[ignore]` by default, run with `cargo test -- --ignored` before each build):

- It sets `ROOST_CLAUDE_BIN=tests/fake-claude.sh`. The script prints its args, calls `"$ROOST_BIN" hook` (the test sets `ROOST_BIN` to the built binary) for `SessionStart`, `UserPromptSubmit`, `Notification`, `PostToolUse` and `Stop` in order with realistic payloads, then waits on stdin.
- It drives the core without a window and asserts the sequence: `starting` → `idle` → `working` → `needs_you` → `working` → `idle`. It then asserts that `send` delivers bracketed-paste bytes to the fake's stdin, and that `stop` produces `exited`.

UI testing is manual, because Tauri's WebDriver doesn't support macOS. The UI holds no logic beyond rendering and key routing. The checklist:

1. ⌘N then ↵ starts a session with a live status.
2. ⌥↵ creates a worktree, and the row's workdir updates.
3. ⌘J jumps to a permission prompt.
4. A notification appears when the window is unfocused.
5. Quit and relaunch, then select a row: the conversation resumes.
6. ⌘O lists and resumes an old transcript.
7. `roost new` from inside a session nests the child.
8. `roost send` delivers multi-line text.

## Setup prerequisites

- Rust toolchain via rustup (not installed on this machine yet).
- Xcode Command Line Tools.
- bun 1.4.2 (installed).
- Claude Code 2.1.296 or later (installed).
