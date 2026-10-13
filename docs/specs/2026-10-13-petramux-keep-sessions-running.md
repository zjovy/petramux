# petramux: sessions keep running when the app quits

Date: 2026-10-13. Asked for by the user ("like Xirp: after an update I'm right back on the same sessions, still working").

## What changes

Today the app owns every terminal, so quitting or updating stops every Claude mid-task. On reopen, each conversation is resumed with `claude --resume`, but the work in progress is lost.

From now on, a small helper process owns the terminals and outlives the app. The app is a viewer that reconnects:

- **Quit (⌘Q) and updates** leave every session running. Reopening shows the same workspaces and sessions, with their recent output replayed, and Claude still working.
- **Closing a session (⌘W)** still ends it.
- A new menu item, **Stop All Sessions and Quit**, ends everything.
- When the helper isn't running, a reopen resumes conversations with `claude --resume`, as today. This covers a reboot, a crash, or a session that ended while the app was closed.

## The helper: `petramux pty-host <socket>`

- It's the same binary, run with a subcommand.
- The app starts it when it can't connect to `<support>/pty.sock`. It's started detached (`setsid`, null stdio), so it survives the app quitting. Because the app launched it, macOS credits its permission prompts to petramux.
- It holds each terminal: the PTY master, its child, and the last 1 MiB of output in a ring buffer.
- It exits after 10 s with no terminals and no clients.

**Protocol (version 1).** Newline-delimited JSON over the Unix socket.

Control requests, one response line each:
- `hello` returns `{"ok":true,"protocol":1}`.
- `spawn {id, program, args, cwd, env, cols, rows}` refuses an id that's already running.
- `write {id, data}`, where `data` is UTF-8 text.
- `resize {id, cols, rows}`.
- `kill {id}`: SIGHUP to the process group, then SIGKILL after 3 s.
- `list` returns `[{id, pid, fg}]`, where `fg` is the foreground process group leader.
- `quit`: kill everything and exit.

**Streams.** `attach {id}` replies `{"ok":true}`, then sends raw output bytes until the terminal ends, and then EOF.
- The stream starts with the buffered output. If the buffer was cut, it starts at the first line break so it doesn't open mid escape sequence.
- Replay and live output are joined under one lock, so nothing is lost or repeated.

**Compatibility.** An updated app may meet an older helper that is still running sessions. The protocol only grows: new fields are optional, and unknown requests answer `{"ok":false}`.

## The app side

`PtyManager` keeps its API (spawn, write, resize, kill, foreground, is_running) but talks to the helper. It gains:
- `attach(id, on_output, on_exit)`, for terminals that were already running;
- `running()`, the ids the helper holds;
- `stop_all()`, which kills everything and stops the helper.

At startup, `Core` asks the helper which terminals are alive, and handles each row:
- **The helper holds its terminal:** attach the stream, and its replayed output reaches the pane when it mounts. The row is live:
  - If the foreground process isn't the shell and the row had Claude running, Claude is still there. The row shows idle until its next hook, and `claude_pid` is the foreground pid.
  - Otherwise the row is a shell.
- **The helper doesn't hold it:** the row is asleep, as today, and resumes on select.
- **A terminal whose row is gone:** killed.

Shutdown:
- On quit, `Core::detach()` stops listening and kills nothing.
- `Core::shutdown()` kills everything and stops the helper. It's used by "Stop All Sessions and Quit" and by tests.
- Hooks that fire while the app is closed are lost, so status catches up at the next hook.

The "N agents are working. Quit anyway?" dialog goes away, because quitting no longer stops them.

## Testing

- **Helper:** spawn and echo; a second client attaches and gets the replay plus live output, with nothing missed or repeated; kill closes the stream; a cut buffer replays from a line break; `list` reports the foreground pid.
- **Flow:** a session survives a relaunch. `detach`, then a new `Core` sees the row as live, not asleep, and Claude isn't started again: there's only one `ARGS:` line, the replay contains the old output, and input still reaches it.
- **Flow:** after `shutdown`, a relaunch resumes with `--resume`, as today.
- **Real app:** start a long Claude turn, quit, reopen, and the turn is still working.
