# petramux

A macOS terminal manager built for working with Claude Code.

Every session is a real terminal (a login shell). Run `claude` in it, or anything else.
petramux follows Claude in each one and tells you which sessions are working, which are
waiting on you, and what changed, while sessions keep running across quits, restarts and
updates.

## What it does

- **Sessions that keep running.** A small background helper owns every terminal, so quitting
  or updating the app never stops a session. Reopen and everything is where you left it.
- **Status at a glance.** A dot per session: working, needs you, done. A one-line note says
  what Claude is doing or asking. A notification when a session needs you or finishes, and
  ⌘J jumps to the next one that needs you.
- **One place per session.** Claude Code's background sessions are off here: no `/background`,
  and `/exit` ends Claude instead of handing the session to Claude's background service.
- **Workspaces and splits.** Split panes freely, drag panes and sidebar rows to rearrange (drop a
  row on another session's row to put them in one workspace),
  keep several workspaces, zoom a pane to fill the window, or drag a session out into a
  window of its own.
- **Diffs.** ⌘G opens what Claude changed in its last turn (or since the last commit) beside
  the session, in the folder Claude is actually working in, worktrees included.
- **Fork.** ⌘⇧F copies a conversation into a new pane beside it.
- **Restart Claude in place.** A session's **restart** button stops Claude and resumes the same
  conversation in its terminal. When Claude Code updates itself, sessions still on the old
  version show **update** instead, and restart on their own once they've sat idle a minute with
  nothing typed since their last turn.
- **Terminal comforts.** Find (⌘F), ⌘-click to open links and file paths, ⇧↩ for a new line
  in Claude's prompt, macOS line editing (⌘⌫, ⌥←, …), a command palette (⌘⇧P).
- **Themes.** 460+ terminal themes (⌥⌘T), your Ghostty config's theme and fonts, and the
  Petra, Petra Light, Linear Dark and Linear Light themes. Claude's own colors follow the theme.
- **Updates in place.** When a newer build is ready, a "Restart to update" button appears;
  sessions keep running through it.

## Install

You need macOS (Apple silicon or Intel) and [Claude Code](https://claude.com/claude-code)
2.1.296 or later.

1. Download the latest `petramux-….dmg` from
   [Releases](https://github.com/zjovy/petramux/releases/latest).
2. Open it and drag **petramux** to Applications.
3. Open petramux. The first time, macOS says it can't check the app, because it isn't signed
   with an Apple developer account: open **System Settings → Privacy & Security** and click
   **Open Anyway** (once).

To update, quit petramux (your sessions keep running), install the new download the same way,
and open it again.

### From source

For working on petramux, or to always run the latest code. You also need
[bun](https://bun.sh), the Rust toolchain ([rustup](https://rustup.rs)) and Xcode's command
line tools (`xcode-select --install`).

```sh
git clone git@github.com:zjovy/petramux.git
cd petramux
bun install
bun run tauri build
scripts/install.sh
```

`scripts/install.sh` copies the build to `/Applications/petramux.app` and opens it; your
sessions keep running if petramux was already open. After that, rebuilding is enough: the app
notices the new build and shows **Restart to update**.

`scripts/release.sh` builds the download (one app for Apple silicon and Intel) and publishes
it as a GitHub release; bump the version in `src-tauri/tauri.conf.json` first.

## Keyboard shortcuts

| Keys | |
|---|---|
| ⌘N | New terminal |
| ⌥⌘N | New terminal under the focused session (its child) |
| ⌘O | Resume a past conversation from this folder |
| ⌘⇧F | Fork the conversation into a pane beside it |
| ⌘G | What Claude changed (diff pane) |
| ⌘J | Next session that needs you, then the next unread |
| ⌘⇧R | Rename the session |
| ⌘W | Close the pane (ends its session) |
| ⌘⇧T | Bring back the last closed session (its conversation too) |
| ⌘F | Find in the terminal |
| ⌘K | Clear the terminal and its scrollback |
| ⌘D / ⌘⇧D | Split right / down with a new terminal |
| ⌘⌥ arrows | Focus the pane that way |
| ⌘⇧↩ | Zoom the pane to fill the window, and back |
| ⌘⇧W | Move the pane into a workspace of its own |
| ⌘⇧O | Open the pane in a window of its own (or drag it out of the window) |
| ⌘1…9 | Go to workspace 1 to 9 |
| ⌘B | Collapse or expand the sidebar |
| ⌥⌘T | Choose a theme |
| ⌘= / ⌘- / ⌘0 | Bigger, smaller, default text |
| ⌘⇧P | All commands and sessions |
| ⌘, | Settings |
| ⌘/ | Keyboard shortcuts |
| ⌘Q | Quit (sessions keep running, unless Settings says otherwise) |

In a terminal: ⇧↩ new line, ⌘⌫ / ⌘⌦ delete to the start / end of the line, ⌥⌫ / ⌥⌦ delete a
word, ⌘← / ⌘→ start / end of the line, ⌥← / ⌥→ back / forward a word (⌃ works as ⌥ for
these, as on Windows), ⌘-click opens a link or
file path.

## The `petramux` command

The app links `~/.local/bin/petramux`. Claude sessions inside petramux can use it to start and
talk to sibling sessions:

```sh
petramux new "<prompt>" [--worktree NAME] [--cwd DIR] [--name NAME]
petramux ls [--json]
petramux send <name|id> "<text>"
petramux stop <name|id>
```

## Settings and themes

- **Settings (⌘,):** the folder new terminals open in, whether Quit keeps sessions running,
  notifications, the terminal font and size, and the keyboard shortcut list.
- **Themes:** petramux reads your Ghostty config (`~/.config/ghostty/config`) for its theme,
  fonts and colors; a theme picked with ⌥⌘T takes precedence. "Petra (light/dark, follows
  macOS)" switches with your Mac's appearance.

## How it works

- **App:** [Tauri 2](https://tauri.app). The Rust side (`src-tauri/src`) runs sessions and
  tracks Claude; the window (`src`) is TypeScript with [xterm.js](https://xtermjs.org).
- **Terminal helper:** `petramux pty-host`, a background process that owns every terminal so
  they outlive the app. The app connects to it over a Unix socket that serves only petramux.
- **Following Claude:** in petramux's terminals, `claude` runs through a small wrapper that
  adds petramux's Claude Code plugin; its hooks report each turn, question and finish.
  Background jobs report through their records in `~/.claude/jobs`.
- **Data:** `~/Library/Application Support/petramux` (sessions, layout, the helper's socket,
  the wrapper and plugin). petramux doesn't change your Claude Code or Ghostty settings. It
  adds one Claude theme, `~/.claude/themes/petramux.json`, which its sessions use so Claude's
  colors follow the terminal theme between light and dark, even in sessions already running.

Design notes are in `docs/specs`.

## Development

```sh
bun run tauri dev                 # run the app with live reload
bun test                          # frontend tests
cd src-tauri && cargo test        # Rust tests (a fake claude stands in for the real one)
```

One test runs the real Claude Code (two short turns, so it uses your login):

```sh
cd src-tauri
ROOST_TEST_CLAUDE=$(command -v claude) cargo test --test flow real_claude -- --ignored --nocapture
```

## Credits

Terminal themes from [iTerm2-Color-Schemes](https://github.com/mbadolato/iTerm2-Color-Schemes)
(MIT, see `src-tauri/themes/LICENSE`). JetBrains Mono (SIL Open Font License) is bundled as the
default font.
