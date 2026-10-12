# Roost: attention, diffs and a quieter sidebar

Date: 2026-10-12. This builds on `2026-10-11-roost-design.md` and `2026-10-11-roost-terminals-and-splits-design.md`.

## Why

When you run four or five Claude sessions, you should be able to see which one needs you, and why, without clicking through them. You should also be able to see what Claude changed in its last turn. The ideas come from studying cmux, which was used only as a behaviour reference.

## Scope

1. A failed turn (an API error or overload) no longer stays on "working".
2. A question from Claude (`AskUserQuestion`) or a plan waiting for approval (`ExitPlanMode`) shows as "needs you", even when permissions are bypassed.
3. A user's own `claude --settings x.json` no longer drops Roost's hooks.
4. **Unread:** a session that finished or started waiting while you weren't looking gets a marker. ⌘J jumps to "needs you" first, then to unread.
5. **Note:** each row has a one-line preview under its name: your prompt, Claude's last reply, what Claude is asking, or the error.
6. **Diff pane:** what Claude changed since your last prompt (or every uncommitted change), shown in a pane beside the session. It refreshes by itself.
7. **Collapsible sidebar:** ⌘B toggles it between the full sidebar and a thin rail of status dots.
8. **Editing keys:** ⌘⌫, ⌥⌫, ⌘←/→, ⌥←/→, ⌘⌦ and ⌥⌦ edit the line the way macOS terminals do (Ghostty's defaults).

Out of scope: approving permissions from the notification, an inbox-based `roost send`, auto-naming, hibernation, PR badges.

## 1. Claude integration

**Hooks load as a plugin.** On every launch, `install` writes `<support>/plugin/.claude-plugin/plugin.json` (`{"name":"roost",...}`) and `<support>/plugin/hooks/hooks.json`. It also deletes the old `<support>/hooks.json`. The shim passes `--plugin-dir "$ROOST_PLUGIN"` instead of `--settings "$ROOST_HOOKS"`, and `ROOST_PLUGIN` replaces `ROOST_HOOKS` in the session env.

A spike with Claude Code 2.1.296 showed that plugin hooks fire alongside the hooks of a user-supplied `--settings` file. The `Stop` payload carries `last_assistant_message`.

**Hooked events:**
- the six existing ones;
- `StopFailure` (no matcher);
- `PreToolUse` with matcher `AskUserQuestion|ExitPlanMode`.

**Status changes:**
- `StopFailure` → idle.
- `PreToolUse` → needs you, but only when `tool_name` is one of those two tools. Answering fires `PostToolUse`, which sets the session back to working.

**Notes.** `Session` gains `note: Option<String>`, saved with the session and included in `View`. `state::note_for(event, payload)` returns one of three things: set the note to a text, clear it, or leave it alone.

| Event | Note |
|---|---|
| `UserPromptSubmit` | the prompt |
| `Stop` | `last_assistant_message` (if missing, the note is left alone) |
| `StopFailure` | `⚠ ` + `error` with underscores turned into spaces (e.g. `⚠ rate limit`) |
| `Notification` | `message` |
| `PreToolUse` AskUserQuestion | `tool_input.questions[0].question` |
| `PreToolUse` ExitPlanMode | `Plan ready for review` |
| `SessionStart` with source `startup` or `clear` | cleared |
| `SessionEnd` (not `/clear`) | cleared |

A note's text is cleaned like this:
- take the first non-empty line;
- strip leading markdown markers (`#`, `>`, `-`, `*`, digits followed by `.`);
- remove `**`, `__` and backticks;
- collapse whitespace;
- cut at 120 characters, ending in `…`.

**Notifications** carry the session's name as the title and its note as the body. Without a note, the body falls back to "Needs you" or "Done".

**Turn snapshot.** Before forwarding `UserPromptSubmit`, the `roost hook` process snapshots the payload's `cwd`. Claude waits for this hook, so the snapshot always comes before any edit.

`git::snapshot(dir) -> Option<String>`:
1. Find the index with `git rev-parse --git-path index`.
2. Copy it to a temp file, so git's stat cache keeps the snapshot fast.
3. Run `GIT_INDEX_FILE=<tmp> git add -A`, then `git write-tree`.
4. Delete the temp file.

Every git call runs with `GIT_OPTIONAL_LOCKS=0` and a 3-second limit. Any failure (not a repo, a timeout) returns `None`.

This leaves the real index and the working tree untouched, and it captures untracked files and deletions. The only side effect is loose blob objects in `.git/objects`, which `git gc` prunes later.

The tree id travels in the forwarded payload as `roost_base`, and the engine stores it as `Session.turn_base`, which is saved with the session.

## 2. Diff pane

**Diff ids.** A diff pane is a layout leaf whose id is `diff:<session id>`. The layout functions don't care about id contents.
- `panes.render` builds a diff body instead of mounting a terminal.
- Boot and `onSessions` keep `diff:<id>` leaves only for live sessions.
- `owner(id)` maps a diff id back to its session id, for the repo default and for the sidebar highlight.

**Opening and closing:**
- ⌘G toggles the diff pane for the focused session: open it to the right, or remove it if it's already in the tree.
- A small `diff` button in a session pane's header does the same.
- ⌘W or ⌘⇧W on a focused diff pane removes the pane and never closes the session.

**Backend.** The command `diff(id, scope)` takes `scope` as `"turn"` or `"all"`. It takes a fresh `snapshot(workdir)` and returns `git diff --no-color --no-ext-diff -M <base> <now>`, cut at 1 MB with a final `… diff truncated` line.
- For `"turn"`, `<base>` is `turn_base`.
- For `"all"`, `<base>` is the `HEAD^{tree}`, or git's empty tree when there are no commits.

Errors come back as plain sentences:
- "Not a git repository."
- "No turn yet. Send Claude a prompt first."
- "Couldn't read the diff: …"

**Frontend** (`src/diff.ts`):
- `parse(text)` turns the unified diff into files: path, added and removed counts, a binary flag, and hunks of lines.
- `render` draws the diff:
  - a header reading `diff · <name>`, with a `last turn | all` toggle;
  - for each file, its path with `+N −M`;
  - hunk headers dimmed;
  - added and removed lines tinted green and red, in the mono font.
- An empty diff reads "No changes."
- The pane re-fetches when it mounts, when you toggle the scope, and whenever its session's status changes.
- The scope lives in memory and starts at "last turn".

## 3. Sidebar

**Rows** get a second line with the note, muted at 11px, cut with an ellipsis, and indented under the name.

**Unread** is a frontend set:
- A session is marked when its status moves into `idle` from `working`, or into `needs_you`, unless the window has focus and the session is the focused pane.
- It's cleared when the session becomes the focused pane while the window has focus, or when the window gains focus while it's the focused pane.
- An unread row shows its name in bright 600 weight with a small blue dot after it.
- In the rail, an unread dot gets a blue ring.
- The Dock badge still counts "needs you" only.

**⌘J** goes to the next "needs you" session after the focused one, in sidebar order. If there is none, it goes to the next unread one.

**Rail.**
- ⌘B toggles `body.rail`, and the choice is kept in `localStorage` (inside try/catch).
- The sidebar is 44px wide, with group headers, names, notes and tree marks hidden.
- Rows become centred dots, with name and note shown on hover via `title`.
- `+ New session` becomes `+`.
- Double-click rename is off in the rail.
- The macOS traffic lights sit in the 30px drag strip above the panes, so they never cover a pane.

## 4. Editing keys

`src/keys.ts` exports `editKey(e)`, which returns the bytes to send or `undefined`:

| Key | Bytes | Effect in zsh, readline and Claude Code |
|---|---|---|
| ⌘⌫ | `\x15` | delete to line start / whole line |
| ⌥⌫ | `\x1b\x7f` | delete word back |
| ⌘⌦ | `\x0b` | delete to line end |
| ⌥⌦ | `\x1bd` | delete word forward |
| ⌘← / ⌘→ | `\x01` / `\x05` | line start / end |
| ⌥← / ⌥→ | `\x1bb` / `\x1bf` | word back / forward |

The xterm key handler writes those bytes to the PTY and swallows the event. All other ⌘ keys still go to the app.

## Testing

- **Rust:**
  - `status_for` and `note_for` cases, and note cleaning;
  - the plugin files and the shim's `--plugin-dir`;
  - `git::snapshot` in a temp repo: it captures an untracked file, leaves the real index unchanged, and returns `None` outside a repo;
  - flow tests with fake-claude: `StopFailure` → idle with a `⚠` note; `PreToolUse` AskUserQuestion → needs you with the question as note; a turn that edits a file shows up in `diff(id,"turn")`, and an unchanged tree reads as empty.
- **Bun:** `diff.parse`, `keys.editKey`, and keeping `diff:` leaves in `prune`.
- **Chrome harness:** rail toggle, the note line, the unread marker and ⌘J, and a diff pane rendering a mocked diff.
- **Real Claude smoke test:** a prompt in a git repo produces a turn diff, and a hand-typed `claude --settings` still tracks status.

## Deviations found while building

- **`PostToolUseFailure` is hooked too** (matcher `*`). With `is_interrupt` set, the turn is over and the session goes idle. Any other tool failure means the turn goes on, so it stays working.
- **Esc ends a turn that hooks never close.** Claude Code 2.1.296 fires no hook when you decline a question with Esc or interrupt a turn. A real-Claude run confirmed this: there was no `PostToolUse`, `PostToolUseFailure` or `Stop`. So when a lone `\x1b` is written to a session that is working or needs you, with Claude in the foreground, the session is set to idle. Arrow keys send `\x1b[…` and are left alone.
- **No unread dot on rows that already say "needs you".** The pill already says so, and the extra dot squeezed the name. These rows still count for ⌘J.
- **The real-Claude smoke test** (`real_claude_reports_notes_questions_and_turn_snapshots`, ignored by default) needs `src-tauri/target/smoke-repo` to be trusted by Claude first. A headless PTY doesn't answer Claude's terminal queries, so it can't reliably get through the trust dialog.
- **Untracked files over 5 MB stay out of snapshots**, so they never show in a turn diff. Hashing them would copy each one into `.git` on every prompt, and a `git add` killed at the time limit leaves a partial pack behind that `gc --auto` never removes. With more than 500 such files there is no snapshot at all. Snapshots run from the repo's top-level folder.
- **A turn with no snapshot explains why:** "Not a git repository.", "Couldn't read the diff: the folder is gone.", or "No turn yet. Send Claude a prompt first." A failed `git diff` says either "the turn's snapshot is gone" or "git took too long".
- **A diff cut at 1 MB ends with a visible notice** in the pane.
