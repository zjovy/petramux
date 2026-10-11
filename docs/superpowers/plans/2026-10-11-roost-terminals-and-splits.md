# Roost: Terminals and Splits Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make each Roost session a terminal (login shell) in which `claude` is still tracked, and replace the single main pane with a free split layout you can split, resize and rearrange by drag & drop, all restored after a relaunch.

**Architecture:**
- The engine spawns `$SHELL -l` per row instead of `claude`. A generated `claude` shim (first on PATH, kept first in zsh by a `ZDOTDIR` `.zshenv` with a `precmd` hook) adds Roost's hooks and reports Claude's pid. The pid is watched so a killed Claude returns the row to `shell`.
- Commands (`claude -- '<prompt>'`, `cd … && claude --resume …`) are typed into the shell once zsh reports its first prompt (a `ShellReady` hook), with a 2 s fallback for other shells.
- The UI keeps a pure split tree (`src/layout.ts`), renders it with `src/panes.ts`, and persists it through the engine.

**Tech Stack:** Rust (Tauri 2, portable-pty, serde, libc), TypeScript (Vite, xterm.js 6), `bun test` for the layout logic. No new dependencies.

**Spec:** `docs/specs/2026-10-11-roost-terminals-and-splits-design.md` (addendum), on top of `docs/specs/2026-10-11-roost-design.md` (v1).

## Global Constraints

- **Dependencies.** No new dependencies on either side. TS tests use bun's built-in `bun test`.
- **Support files.** These live under `~/Library/Application Support/Roost/`: `state.json` (now `version: 2`), `roost.sock`, `hooks.json`, `bin/claude` (mode 0755) and `zsh/.zshenv`. All three generated files are rewritten on every launch.
- **Session environment.** v1's `child_env` plus `PATH=<support>/bin:…`, `ROOST_BIN`, `ROOST_REAL_CLAUDE`, `ROOST_HOOKS`, `ROOST_BLURB` and `ROOST_SHIM_DIR`. For zsh only, also `ZDOTDIR=<support>/zsh` and `ROOST_ORIG_ZDOTDIR` (only when the user had a `ZDOTDIR`).
- **Status.** `shell` is new: a square dim dot. `SessionEnd`, or the remembered Claude pid disappearing (checked every 2 s), sets `shell`. Exited rows never change status again.
- **Typed commands.** Arguments are always single-quoted with `'\''` escaping, and prompts are always preceded by `--`:
  - `claude -- '<prompt>'`, or `claude -w '<name>' -- '<prompt>'` for a worktree;
  - `cd '<workdir>' && claude --resume '<claudeId>'` to resume.
- **Layout tree.** `{ "pane": id }` or `{ "split": "row"|"col", "ratio": 0.1–0.9, "a": …, "b": … }`, or `null`. It is saved through `save_layout`, debounced 300 ms.
- **Drop zones.** Whichever edge of the pane is nearest the cursor, if it's within 25% of that edge; otherwise the centre.
- **Keys.** ⌘D splits right and ⌘⇧D splits down. ⌘W closes the terminal and its pane, ⌘⇧W removes the pane only, and ⌘⌥←→↑↓ moves focus between panes. ⌘N, ⌘J, ⌘1–9, ⌘O and ⌘⇧R keep their v1 meaning, with "select" meaning "focus its pane, or show it in the focused pane".
- **Deviations from the addendum, decided here:**
  - Prompts are typed with `--`, so one-word and dash-leading prompts are still tracked. Verified on 2026-10-11 that `claude -- '<prompt>'` works.
  - Typed commands wait for a `ShellReady` hook from the zsh `precmd`, with a 2 s fallback, instead of being written immediately. Startup scripts such as p10k can swallow typeahead.
  - `Core::new` takes a `Setup` struct and installs the generated files itself, so tests and the app share one path.
  - `hooks_json` moves to `integration.rs` and gains `SessionEnd`.
  - A shell whose folder no longer exists starts in `$HOME` (portable-pty's fallback). Only `new_session` refuses missing folders.

## Review Focus

1. **A `.zshrc` that rebuilds PATH** (for example `path=(~/bin $path)`, or `export PATH=…` that drops entries) must not hide the shim once the first prompt has run. Tested in Task 2.
2. **Prompts that contain shell syntax** (`'; rm -rf ~; '`, `$(whoami)`, backticks, newlines) must be typed as one inert argument. Tested in Task 2 (`quote` / `claude_cmd`) and end-to-end in Task 3.
3. **Claude killed without `SessionEnd`** (Ctrl-C, crash, `kill -9`) must return the row to `shell` within 3 s and clear `claudeRunning`, so a relaunch doesn't resume it. Tested in Task 3.
4. **A restored layout naming rows that are gone, or the same row twice,** must render without holes or duplicate terminals. Tested in Task 4 (`prune` and dedupe) and Task 5 (boot).
5. **Dropping a pane onto itself, or onto its only sibling's centre,** must not lose the pane. Tested in Task 4 (`move`).

---

## File map

```
src-tauri/src/
  state.rs        MODIFY  v2 schema (claudeRunning, layout, focus), Status::Shell, ClaudeStart/SessionEnd mapping, v1 migration
  integration.rs  NEW     hooks_json (+SessionEnd), shim + .zshenv text, install(), quote/claude_cmd/resume_cmd, session_env()
  engine.rs       REWRITE terminals instead of claude; Setup; pending typed line + ShellReady; pid watcher; send rules; layout storage
  pty.rs          MODIFY  drop claude_args/Launch (+ their tests); keep BLURB
  cli.rs          MODIFY  status word for Shell
  app.rs          MODIFY  Setup, boot (layout/focus/shell), save_layout + visible set for notifications, drop start_fresh
  lib.rs          MODIFY  add integration
src-tauri/tests/
  fake-claude.sh  MODIFY  writes its pid, reads lines, `/exit` fires SessionEnd
  flow.rs         REWRITE terminal-model flow tests (zsh with a temp HOME)
src/
  layout.ts       NEW     pure split-tree functions
  layout.test.ts  NEW     bun tests
  panes.ts        NEW     renders the tree: panes, headers, dividers, drag & drop
  terminals.ts    REWRITE mount/park/fit/focus many terminals
  api.ts          MODIFY  types + saveLayout, drop startFresh
  sidebar.ts      MODIFY  shell dot, offscreen dimming, draggable rows
  main.ts         REWRITE layout-aware selection, splits, keys, persistence
  sheet.ts        MODIFY  placeholder + empty-prompt name preview
  style.css       MODIFY  pane/split/divider/drop styles, shell dot, offscreen rows
.superpowers/harness.html  UPDATE (scratch, untracked) mocks for the new commands
```

---

### Task 1: State v2 and the shell status

**Files:**
- Modify: `src-tauri/src/state.rs`, `src-tauri/src/cli.rs` (status word)

**Interfaces:**
- Produces:
  - `Status::Shell`, serialised `"shell"`.
  - `Session { id, claude_id, name, repo, workdir: String, parent: Option<String>, created_at: u64, exited: bool, claude_running: bool }`. `worktree` is gone; serde ignores it in old files.
  - `State { version: u32, recent_repos, sessions, layout: Option<serde_json::Value>, focus: Option<String> }`; `State::default().version == 2`.
  - `status_for("ClaudeStart", _) == Some(Starting)` and `status_for("SessionEnd", _) == Some(Shell)`.
  - `load` migrates v1: `claude_running = !exited`; layout is `{"pane": <last non-exited id>}` with focus on it; version becomes 2.

- [ ] **Step 1: Write the failing tests**

In `state.rs`, update the test helper and add tests to the `tests` module. Replace `sess`:
```rust
    pub fn sess(name: &str) -> Session {
        Session {
            id: format!("id-{name}"),
            claude_id: format!("c-{name}"),
            name: name.into(),
            repo: "/r".into(),
            workdir: "/r".into(),
            parent: None,
            created_at: 1,
            exited: false,
            claude_running: false,
        }
    }
```
Replace the body of `missing_fields_get_defaults` (`{"sessions":[]}` is a v1 file and now migrates):
```rust
    #[test]
    fn missing_fields_get_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, r#"{"sessions":[]}"#).unwrap();
        let st = load(&p);
        assert_eq!(st.version, 2);
        assert_eq!(st.layout, None);
    }
```
Add:
```rust
    #[test]
    fn v1_files_migrate_to_terminals_with_one_pane() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, r#"{"version":1,"recentRepos":[],"sessions":[
            {"id":"a","claudeId":"ca","name":"a","repo":"/r","workdir":"/r","worktree":true,"parent":null,"createdAt":1,"exited":false},
            {"id":"b","claudeId":"cb","name":"b","repo":"/r","workdir":"/r","worktree":false,"parent":null,"createdAt":2,"exited":true}
        ]}"#).unwrap();
        let st = load(&p);
        assert_eq!(st.version, 2);
        assert!(st.sessions[0].claude_running);
        assert!(!st.sessions[1].claude_running);
        assert_eq!(st.layout, Some(serde_json::json!({"pane": "a"})));
        assert_eq!(st.focus.as_deref(), Some("a"));
    }

    #[test]
    fn v2_files_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        let st = State { sessions: vec![sess("a")], ..State::default() };
        save(&p, &st).unwrap();
        assert_eq!(load(&p), st);
    }

    #[test]
    fn claude_start_and_end_map_to_starting_and_shell() {
        let none = serde_json::json!({});
        assert_eq!(status_for("ClaudeStart", &none), Some(Status::Starting));
        assert_eq!(status_for("SessionEnd", &none), Some(Status::Shell));
        assert_eq!(serde_json::to_value(Status::Shell).unwrap(), "shell");
    }
```
Change `hook_events_map_to_status` so its last assertion is `assert_eq!(status_for("PreCompact", &none), None);`, replacing the old `SessionEnd → None` line.

- [ ] **Step 2: Run to see them fail**

Run: `cd src-tauri && cargo test --lib state::`
Expected: compile errors (`Status::Shell`, `claude_running`, `layout`, `focus` don't exist yet). That's the RED for this task.

- [ ] **Step 3: Implement**

In `state.rs`:
- Add `Shell,` to `Status` after `Starting,`.
- In `Session`, delete the `worktree` field and add after `exited`:
```rust
    #[serde(default)]
    pub claude_running: bool,
```
- Replace `State`, its `Default` and `version_one`:
```rust
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    #[serde(default = "version_one")]
    pub version: u32,
    #[serde(default)]
    pub recent_repos: Vec<String>,
    #[serde(default)]
    pub sessions: Vec<Session>,
    #[serde(default)]
    pub layout: Option<serde_json::Value>,
    #[serde(default)]
    pub focus: Option<String>,
}

fn version_one() -> u32 {
    1
}

impl Default for State {
    fn default() -> Self {
        Self { version: 2, recent_repos: vec![], sessions: vec![], layout: None, focus: None }
    }
}

/// v1 rows were Claude sessions: resume each live one in its terminal, and show the newest.
fn migrate(mut st: State) -> State {
    if st.version < 2 {
        for s in &mut st.sessions {
            s.claude_running = !s.exited;
        }
        if let Some(last) = st.sessions.iter().rev().find(|s| !s.exited) {
            st.layout = Some(serde_json::json!({ "pane": last.id }));
            st.focus = Some(last.id.clone());
        }
        st.version = 2;
    }
    st
}
```
- In `load`, change `Ok(state) => state,` to `Ok(state) => migrate(state),`.
- In `status_for`, add before the `_ => None` arm:
```rust
        "ClaudeStart" => Some(Status::Starting),
        "SessionEnd" => Some(Status::Shell),
```

In `cli.rs` `status_word`, add `Status::Shell => "shell",` after the `Starting` arm.

- [ ] **Step 4: Run to see them pass**

Run: `cargo test --lib state:: && cargo test --lib cli::`
Expected: all `state::` tests pass (14), and `cli::` passes (6).

The library won't compile at first, because `engine.rs` still uses the removed `worktree` field. Make three stop-gap edits; Task 3 replaces the file anyway:
- In `new_session`'s `Session { … }`, change `worktree: req.worktree,` to `claude_running: false,`.
- In `resume_past`'s `Session { … }`, change `worktree: false,` to `claude_running: false,`. Leave `start_fresh`'s `NewSession { … worktree: false … }` alone.
- In `fn view`, change `worktree: s.worktree,` to `worktree: false,`.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src
git commit -m "feat: state v2 with shell status and v1 migration"
```

---

### Task 2: Shell integration: shim, zsh hook, typed commands

**Files:**
- Create: `src-tauri/src/integration.rs`
- Modify: `src-tauri/src/lib.rs` (add `pub mod integration;`), `src-tauri/src/engine.rs` (remove `hooks_json`/`write_hooks` and their test; Task 3 rewrites the rest)

**Interfaces:**
- Consumes: `paths::Paths`, `shellenv::{Env, child_env}`, `pty::BLURB`.
- Produces:
  - `integration::SHIM: &str` and `integration::ZSHENV: &str`, the file contents.
  - `hooks_json(roost_bin: &Path) -> Value`, with six events (v1's five plus `SessionEnd`).
  - `install(paths: &Paths, roost_bin: &Path) -> io::Result<()>` writes `hooks.json`, `bin/claude` (0755) and `zsh/.zshenv`.
  - `shim_dir(paths) -> PathBuf`, which is `<dir>/bin`.
  - `quote(&str) -> String`.
  - `claude_cmd(prompt: &str, worktree: Option<&str>) -> String`.
  - `resume_cmd(workdir: &str, claude_id: &str) -> String`.
  - `session_env(base: &Env, paths: &Paths, roost_bin: &Path, claude: &Path, shell: &Path, session_id: &str) -> Env`.

- [ ] **Step 1: Write the failing tests**

`src-tauri/src/integration.rs` starts as stubs plus tests:
```rust
use crate::paths::Paths;
use crate::pty::BLURB;
use crate::shellenv::{child_env, Env};
use serde_json::{json, Value};
use std::io;
use std::path::{Path, PathBuf};

pub const SHIM: &str = "";
pub const ZSHENV: &str = "";

pub fn hooks_json(_roost_bin: &Path) -> Value {
    unimplemented!()
}
pub fn shim_dir(paths: &Paths) -> PathBuf {
    paths.dir.join("bin")
}
pub fn install(_paths: &Paths, _roost_bin: &Path) -> io::Result<()> {
    unimplemented!()
}
pub fn quote(_s: &str) -> String {
    unimplemented!()
}
pub fn claude_cmd(_prompt: &str, _worktree: Option<&str>) -> String {
    unimplemented!()
}
pub fn resume_cmd(_workdir: &str, _claude_id: &str) -> String {
    unimplemented!()
}
pub fn session_env(_base: &Env, _paths: &Paths, _roost_bin: &Path, _claude: &Path, _shell: &Path, _id: &str) -> Env {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;

    fn exe(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn hooks_cover_six_events_including_session_end() {
        let v = hooks_json(Path::new("/Apps/It's Roost/roost"));
        let hooks = v["hooks"].as_object().unwrap();
        assert_eq!(hooks.len(), 6);
        assert_eq!(hooks["SessionEnd"][0]["hooks"][0]["command"], r"'/Apps/It'\''s Roost/roost' hook SessionEnd");
        assert_eq!(hooks["Notification"][0]["matcher"], "permission_prompt|elicitation_dialog");
    }

    #[test]
    fn quoting_makes_shell_syntax_inert() {
        assert_eq!(quote("it's"), r"'it'\''s'");
        let nasty = "'; rm -rf ~; ' $(whoami) `id` \n next";
        let out = Command::new("/bin/sh").args(["-c", &format!("printf %s {}", quote(nasty))]).output().unwrap();
        assert_eq!(String::from_utf8(out.stdout).unwrap(), nasty);
    }

    #[test]
    fn typed_commands() {
        assert_eq!(claude_cmd("fix it", None), "claude -- 'fix it'");
        assert_eq!(claude_cmd("-v broken", Some("wt")), "claude -w 'wt' -- '-v broken'");
        assert_eq!(resume_cmd("/r/a b", "c1"), "cd '/r/a b' && claude --resume 'c1'");
    }

    #[test]
    fn install_writes_executable_shim_and_zshenv() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        install(&paths, Path::new("/x/roost")).unwrap();
        let shim = paths.dir.join("bin/claude");
        assert_eq!(std::fs::metadata(&shim).unwrap().permissions().mode() & 0o777, 0o755);
        assert_eq!(std::fs::read_to_string(shim).unwrap(), SHIM);
        assert_eq!(std::fs::read_to_string(paths.dir.join("zsh/.zshenv")).unwrap(), ZSHENV);
        assert!(std::fs::read_to_string(paths.hooks()).unwrap().contains("'/x/roost' hook SessionEnd"));
    }

    /// Runs the real shim with a fake claude (prints args) and a fake roost (records the hook).
    fn run_shim(args: &[&str]) -> (String, String) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        install(&paths, Path::new("/unused")).unwrap();
        let fake_claude = dir.path().join("real-claude");
        exe(&fake_claude, "#!/bin/sh\nprintf '%s|' \"$@\"\n");
        let fake_roost = dir.path().join("fake-roost");
        let log = dir.path().join("hook.log");
        exe(&fake_roost, &format!("#!/bin/sh\necho \"$1 $2 $(cat)\" >> '{}'\n", log.display()));
        let out = Command::new("/bin/sh")
            .arg(paths.dir.join("bin/claude"))
            .args(args)
            .env("ROOST_REAL_CLAUDE", &fake_claude)
            .env("ROOST_BIN", &fake_roost)
            .env("ROOST_HOOKS", "/h.json")
            .env("ROOST_BLURB", "B")
            .output()
            .unwrap();
        (String::from_utf8(out.stdout).unwrap(), std::fs::read_to_string(log).unwrap_or_default())
    }

    #[test]
    fn shim_adds_hooks_and_reports_its_pid() {
        let (args, hook) = run_shim(&["--", "fix it"]);
        assert_eq!(args, "--settings|/h.json|--append-system-prompt|B|--|fix it|");
        assert!(hook.starts_with("hook ClaudeStart {\"pid\":"), "{hook}");
    }

    #[test]
    fn shim_passes_subcommands_and_version_through() {
        assert_eq!(run_shim(&["mcp", "list"]), ("mcp|list|".into(), String::new()));
        assert_eq!(run_shim(&["--version"]), ("--version|".into(), String::new()));
    }

    #[test]
    fn zsh_keeps_the_shim_first_after_zshrc_reorders_path() {
        if !Path::new("/bin/zsh").exists() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path().join("support"));
        install(&paths, Path::new("/usr/bin/true")).unwrap();
        let home = dir.path().join("home");
        let other = dir.path().join("other");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        exe(&other.join("claude"), "#!/bin/sh\n");
        std::fs::write(home.join(".zshrc"), format!("path=('{}' $path)\n", other.display())).unwrap();
        let env = session_env(
            &[("PATH".to_string(), "/usr/bin:/bin".to_string()), ("HOME".to_string(), home.display().to_string())]
                .into_iter()
                .collect(),
            &paths,
            Path::new("/usr/bin/true"),
            Path::new("/real/claude"),
            Path::new("/bin/zsh"),
            "s1",
        );
        let out = Command::new("/bin/zsh")
            .args(["-l", "-i", "-c", "for f in $precmd_functions; do $f; done; command -v claude"])
            .env_clear()
            .envs(&env)
            .output()
            .unwrap();
        let got = String::from_utf8(out.stdout).unwrap();
        assert_eq!(got.trim(), shim_dir(&paths).join("claude").display().to_string(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn session_env_sets_roost_vars_and_zdotdir_for_zsh_only() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths::new(dir.path());
        let mut base = Env::new();
        base.insert("PATH".into(), "/bin".into());
        base.insert("ZDOTDIR".into(), "/u/zdot".into());
        let z = session_env(&base, &paths, Path::new("/rb"), Path::new("/rc"), Path::new("/bin/zsh"), "s1");
        assert!(z["PATH"].starts_with(&format!("{}:", shim_dir(&paths).display())));
        assert_eq!(z["ROOST_BIN"], "/rb");
        assert_eq!(z["ROOST_REAL_CLAUDE"], "/rc");
        assert_eq!(z["ROOST_HOOKS"], paths.hooks().display().to_string());
        assert_eq!(z["ROOST_BLURB"], BLURB);
        assert_eq!(z["ROOST_SESSION"], "s1");
        assert_eq!(z["ZDOTDIR"], paths.dir.join("zsh").display().to_string());
        assert_eq!(z["ROOST_ORIG_ZDOTDIR"], "/u/zdot");
        let b = session_env(&base, &paths, Path::new("/rb"), Path::new("/rc"), Path::new("/bin/bash"), "s1");
        assert_eq!(b["ZDOTDIR"], "/u/zdot");
        assert!(!b.contains_key("ROOST_ORIG_ZDOTDIR"));
    }
}
```
In `engine.rs`, delete `hooks_json`, `write_hooks` and the test `hooks_file_registers_every_event_with_quoted_binary`. In `app.rs`, change `engine::write_hooks(&paths, &exe)?;` to `crate::integration::install(&paths, &exe)?;` so the crate still builds until Task 4.

- [ ] **Step 2: Run to see them fail**

Run: `cargo test --lib integration::`
Expected: every test FAILs with `not implemented`, or on an empty file comparison.

- [ ] **Step 3: Implement**

Replace the two constants and the stubs:
```rust
/// `<support>/bin/claude`: runs the real claude with Roost's hooks, as this same process.
pub const SHIM: &str = r#"#!/bin/sh
# Roost: run the real claude with Roost's hooks, as this same process.
case "$1" in
  -v|--version|-h|--help) exec "$ROOST_REAL_CLAUDE" "$@" ;;
esac
if printf '%s' "$1" | grep -Eq '^[a-z][a-z-]*$'; then
  exec "$ROOST_REAL_CLAUDE" "$@"
fi
printf '{"pid":%d}' $$ | "$ROOST_BIN" hook ClaudeStart
exec "$ROOST_REAL_CLAUDE" --settings "$ROOST_HOOKS" --append-system-prompt "$ROOST_BLURB" "$@"
"#;

/// `<support>/zsh/.zshenv`: hands zsh back to the user's files, keeps the shim first,
/// and reports the first prompt so typed commands aren't eaten by startup scripts.
pub const ZSHENV: &str = r#"# Roost: hand zsh back to the user's own startup files, keep the claude shim first on PATH,
# and tell Roost once the first prompt is up.
if [[ -n $ROOST_ORIG_ZDOTDIR ]]; then ZDOTDIR=$ROOST_ORIG_ZDOTDIR; else unset ZDOTDIR; fi
[[ -f ${ZDOTDIR:-$HOME}/.zshenv ]] && source ${ZDOTDIR:-$HOME}/.zshenv
_roost_shim_first() { path=("$ROOST_SHIM_DIR" ${path:#$ROOST_SHIM_DIR}); }
_roost_ready() {
  precmd_functions=(${precmd_functions:#_roost_ready})
  "$ROOST_BIN" hook ShellReady </dev/null >/dev/null 2>&1
}
typeset -ga precmd_functions
precmd_functions+=(_roost_shim_first _roost_ready)
"#;

/// The per-session Claude Code settings: one `roost hook <Event>` command per event.
pub fn hooks_json(roost_bin: &Path) -> Value {
    let bin = quote(&roost_bin.display().to_string());
    let entry = |matcher: Option<&str>, event: &str| {
        let mut e = json!({"hooks": [{"type": "command", "command": format!("{bin} hook {event}")}]});
        if let Some(m) = matcher {
            e["matcher"] = json!(m);
        }
        json!([e])
    };
    json!({"hooks": {
        "SessionStart": entry(None, "SessionStart"),
        "UserPromptSubmit": entry(None, "UserPromptSubmit"),
        "PostToolUse": entry(Some("*"), "PostToolUse"),
        "Notification": entry(Some("permission_prompt|elicitation_dialog"), "Notification"),
        "Stop": entry(None, "Stop"),
        "SessionEnd": entry(None, "SessionEnd"),
    }})
}

pub fn shim_dir(paths: &Paths) -> PathBuf {
    paths.dir.join("bin")
}

fn zdotdir(paths: &Paths) -> PathBuf {
    paths.dir.join("zsh")
}

/// Writes hooks.json, the shim and the zsh integration (on every launch).
pub fn install(paths: &Paths, roost_bin: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(shim_dir(paths))?;
    std::fs::create_dir_all(zdotdir(paths))?;
    std::fs::write(paths.hooks(), serde_json::to_vec_pretty(&hooks_json(roost_bin))?)?;
    let shim = shim_dir(paths).join("claude");
    std::fs::write(&shim, SHIM)?;
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755))?;
    std::fs::write(zdotdir(paths).join(".zshenv"), ZSHENV)
}

/// One shell word, whatever it contains.
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// `--` keeps one-word and dash-leading prompts a prompt (and tracked by the shim).
pub fn claude_cmd(prompt: &str, worktree: Option<&str>) -> String {
    match worktree {
        Some(name) => format!("claude -w {} -- {}", quote(name), quote(prompt)),
        None => format!("claude -- {}", quote(prompt)),
    }
}

pub fn resume_cmd(workdir: &str, claude_id: &str) -> String {
    format!("cd {} && claude --resume {}", quote(workdir), quote(claude_id))
}

/// Env for a session's shell: v1's child env plus the shim and Roost's variables.
pub fn session_env(base: &Env, paths: &Paths, roost_bin: &Path, claude: &Path, shell: &Path, id: &str) -> Env {
    let mut env = child_env(base, id, &paths.sock());
    let shim = shim_dir(paths).display().to_string();
    let path = env.get("PATH").map(|p| format!("{shim}:{p}")).unwrap_or(shim.clone());
    env.insert("PATH".into(), path);
    env.insert("ROOST_SHIM_DIR".into(), shim);
    env.insert("ROOST_BIN".into(), roost_bin.display().to_string());
    env.insert("ROOST_REAL_CLAUDE".into(), claude.display().to_string());
    env.insert("ROOST_HOOKS".into(), paths.hooks().display().to_string());
    env.insert("ROOST_BLURB".into(), BLURB.into());
    if shell.file_name().is_some_and(|n| n == "zsh") {
        if let Some(orig) = env.get("ZDOTDIR").cloned() {
            env.insert("ROOST_ORIG_ZDOTDIR".into(), orig);
        }
        env.insert("ZDOTDIR".into(), zdotdir(paths).display().to_string());
    }
    env
}
```
`session_env` takes the shell path, so `install` and `session_env` together decide zsh-only behaviour.

- [ ] **Step 4: Run to see them pass**

Run: `cargo test --lib integration::`
Expected: 8 passed. The zsh test proves that `.zshrc`'s `path=(other $path)` is undone by the `precmd` hook.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src
git commit -m "feat: claude shim, zsh integration and typed commands"
```

---

### Task 3: Engine: terminals, typed commands, Claude tracking

**Files:**
- Rewrite: `src-tauri/src/engine.rs`
- Modify: `src-tauri/src/pty.rs` (delete `Launch`, `claude_args` and the three tests that use them; keep `BLURB`)
- Modify: `src-tauri/tests/fake-claude.sh`
- Rewrite: `src-tauri/tests/flow.rs`
- Modify: `src-tauri/src/app.rs` (Setup, boot, save_layout, visible set; drop start_fresh)

**Interfaces:**
- Consumes: Tasks 1–2 (`Status::Shell`, `claude_running`, `integration::*`), and `pty::{PtyManager, Spawn}`.
- Produces:
  - `engine::Setup { paths: Paths, env: Env, claude: Option<PathBuf>, roost_bin: PathBuf, shell: PathBuf }`.
  - `engine::View { id, name, repo, workdir: String, parent: Option<String>, status: Status, since: u64, created_at: u64 }` (camelCase).
  - `engine::NewSession` is unchanged.
  - `Core::new(setup: Setup, sink: Arc<dyn Sink>) -> Arc<Core>` installs the integration files and starts the pid watcher.
  - Other `Core` methods: `handler`, `handle`, `views`, `recent_repos`, `claude_found`, `shell_name() -> String`, `new_session`, `select`, `write`, `resize`, `rename`, `close`, `stop`, `send`, `list_past`, `resume_past`, `layout() -> (Option<Value>, Option<String>)`, `save_layout(Option<Value>, Option<String>)`, `shutdown`.
  - `start_fresh` is removed.
  - `engine::is_git_repo` is unchanged.

Rules this task pins down:
- A new row is `shell`. `ClaudeStart` records the pid; `SessionStart` sets `claudeRunning`; `SessionEnd` or a dead pid clears it and sets `shell`.
- The typed line waits for `ShellReady`, or 2 s, whichever comes first, and is typed once.
- `send` needs Claude running. An asleep row with `claudeRunning` is restored first, and `send` waits up to 15 s while it's `shell` or `starting`.
- Exited rows ignore every later status.

- [ ] **Step 1: Update the fake Claude and write the failing flow tests**

`src-tauri/tests/fake-claude.sh`:
```sh
#!/bin/sh
# Stand-in for `claude` in tests, reached through Roost's shim: prints its args,
# records its pid, fires hooks the way Claude would, then echoes lines until /exit.
echo "ARGS: $*"
echo $$ > .fake-claude-pid
hook() { printf '%s' "$2" | "$ROOST_BIN" hook "$1"; }
hook SessionStart "{\"session_id\":\"fake-claude-id\",\"cwd\":\"$(pwd -P)\"}"
sleep 0.3
hook UserPromptSubmit '{}'
sleep 0.3
hook Notification '{"notification_type":"permission_prompt"}'
sleep 0.3
hook PostToolUse '{}'
sleep 0.3
hook Stop '{}'
while IFS= read -r line; do
  echo "GOT: $line"
  case "$line" in *"/exit"*) hook SessionEnd '{}'; exit 0 ;; esac
done
```

`src-tauri/tests/flow.rs` (full replacement):
```rust
use roost_lib::engine::{Attention, Core, NewSession, Setup, Sink, View};
use roost_lib::paths::Paths;
use roost_lib::shellenv::Env;
use roost_lib::sock::{self, Request};
use roost_lib::state::Status;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Rec {
    snapshots: Mutex<Vec<Vec<View>>>,
    changes: Mutex<Vec<(String, Status)>>,
    output: Mutex<String>,
    attention: Mutex<Vec<&'static str>>,
}

impl Sink for Rec {
    fn sessions_changed(&self, views: Vec<View>) {
        self.snapshots.lock().unwrap().push(views);
    }
    fn session_changed(&self, v: View) {
        self.changes.lock().unwrap().push((v.id, v.status));
    }
    fn output(&self, _id: &str, text: String) {
        self.output.lock().unwrap().push_str(&text);
    }
    fn attention(&self, _v: &View, kind: Attention) {
        self.attention.lock().unwrap().push(match kind {
            Attention::NeedsYou => "needs_you",
            Attention::Done => "done",
        });
    }
}

fn wait(what: &str, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while !ok() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn setup(dir: &Path) -> (Arc<Core>, Arc<Rec>, Paths) {
    setup_with(dir, "fake-claude.sh")
}

/// Real zsh with a throwaway HOME, the real `roost` binary for hooks, a fake claude.
fn setup_with(dir: &Path, script: &str) -> (Arc<Core>, Arc<Rec>, Paths) {
    let paths = Paths::new(dir.join("support"));
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let mut env = Env::new();
    env.insert("PATH".into(), "/usr/bin:/bin".into());
    env.insert("HOME".into(), home.display().to_string());
    let rec = Arc::new(Rec::default());
    let core = Core::new(
        Setup {
            paths: paths.clone(),
            env,
            claude: Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests").join(script)),
            roost_bin: PathBuf::from(env!("CARGO_BIN_EXE_roost")),
            shell: PathBuf::from("/bin/zsh"),
        },
        rec.clone(),
    );
    sock::serve(&paths.sock(), core.handler()).unwrap();
    (core, rec, paths)
}

fn repo(tmp: &tempfile::TempDir) -> String {
    let r = tmp.path().canonicalize().unwrap().join("repo");
    std::fs::create_dir_all(&r).unwrap();
    r.display().to_string()
}

fn new(core: &Core, repo: &str, prompt: &str) -> View {
    core.new_session(NewSession { prompt: prompt.into(), repo: repo.into(), worktree: false, name: None, parent: None })
        .unwrap()
}

fn statuses(rec: &Rec, id: &str) -> Vec<Status> {
    rec.changes.lock().unwrap().iter().filter(|(i, _)| i == id).map(|(_, s)| *s).collect()
}

fn status_of(core: &Core, id: &str) -> Status {
    core.views().into_iter().find(|v| v.id == id).unwrap().status
}

fn state_json(paths: &Paths) -> String {
    std::fs::read_to_string(paths.state()).unwrap()
}

#[test]
fn a_terminal_tracks_claude_started_in_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, paths) = setup(tmp.path());
    let t = Duration::from_secs(5);

    let stray = Request::Hook { session: "nope".into(), event: "Stop".into(), payload: serde_json::json!({}) };
    assert!(sock::call(&paths.sock(), &stray, t).unwrap().ok);
    assert!(core.views().is_empty());

    let req = Request::New { prompt: "fix it".into(), cwd: repo.clone(), worktree: None, name: None, parent: None };
    let resp = sock::call(&paths.sock(), &req, t).unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    assert_eq!(resp.name.as_deref(), Some("fix-it"));
    let id = resp.id.unwrap();
    assert_eq!(rec.snapshots.lock().unwrap()[0][0].status, Status::Shell);

    wait("claude to run through its statuses", || statuses(&rec, &id).len() >= 6);
    assert_eq!(
        statuses(&rec, &id)[..6],
        [Status::Starting, Status::Idle, Status::Working, Status::NeedsYou, Status::Working, Status::Idle]
    );
    assert_eq!(*rec.attention.lock().unwrap(), vec!["needs_you", "done"]);
    assert_eq!(core.views()[0].workdir, repo);
    assert!(state_json(&paths).contains("fake-claude-id"));
    assert!(state_json(&paths).contains("\"claudeRunning\": true"));
    let out = rec.output.lock().unwrap().clone();
    assert!(out.contains("ARGS: --settings ") && out.contains("--append-system-prompt") && out.contains("-- fix it"), "{out}");

    let send = |text: &str| Request::Send { target: "fix-it".into(), text: text.into(), from: Some(id.clone()) };
    assert!(sock::call(&paths.sock(), &send("hello"), t).unwrap().ok);
    wait("sent text", || rec.output.lock().unwrap().contains("[from fix-it] hello"));

    assert!(sock::call(&paths.sock(), &send("/exit"), t).unwrap().ok);
    wait("back to the shell", || status_of(&core, &id) == Status::Shell);
    assert!(state_json(&paths).contains("\"claudeRunning\": false"));
    let refused = sock::call(&paths.sock(), &send("hi"), t).unwrap();
    assert!(!refused.ok && refused.error.unwrap().contains("isn't running Claude"));

    assert!(sock::call(&paths.sock(), &Request::Stop { target: "fix-it".into() }, t).unwrap().ok);
    wait("exited", || status_of(&core, &id) == Status::Exited);
}

#[test]
fn an_empty_prompt_is_just_a_shell_named_after_the_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "");
    assert_eq!((v.name.as_str(), v.status), ("repo", Status::Shell));
    std::thread::sleep(Duration::from_millis(2500));
    assert!(!rec.output.lock().unwrap().contains("ARGS:"));
    core.shutdown();
}

#[test]
fn a_killed_claude_returns_the_row_to_the_shell() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, paths) = setup(tmp.path());
    let v = new(&core, &repo, "x");
    wait("idle", || statuses(&rec, &v.id).len() >= 6);
    let pid = std::fs::read_to_string(Path::new(&repo).join(".fake-claude-pid")).unwrap();
    unsafe { libc::kill(pid.trim().parse().unwrap(), libc::SIGKILL) };
    let start = Instant::now();
    wait("shell after kill", || status_of(&core, &v.id) == Status::Shell);
    assert!(start.elapsed() < Duration::from_millis(3500));
    assert!(state_json(&paths).contains("\"claudeRunning\": false"));
    core.shutdown();
}

#[test]
fn relaunch_reopens_the_terminal_and_resumes_claude() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "a");
    wait("SessionStart", || statuses(&rec, &v.id).contains(&Status::Idle));
    core.shutdown();
    drop(core);

    let (core2, rec2, _) = setup(tmp.path());
    assert_eq!(status_of(&core2, &v.id), Status::Asleep);
    core2.select(&v.id).unwrap();
    wait("resume", || rec2.output.lock().unwrap().contains("ARGS: ") && rec2.output.lock().unwrap().contains("--resume fake-claude-id"));
    wait("idle again", || statuses(&rec2, &v.id).contains(&Status::Idle));
    core2.shutdown();
}

#[test]
fn send_to_an_asleep_claude_restores_it_first() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "a");
    wait("SessionStart", || statuses(&rec, &v.id).contains(&Status::Idle));
    core.shutdown();
    drop(core);

    let (core2, rec2, paths2) = setup(tmp.path());
    let req = Request::Send { target: v.name.clone(), text: "ping".into(), from: None };
    let resp = sock::call(&paths2.sock(), &req, Duration::from_secs(20)).unwrap();
    assert!(resp.ok, "{:?}", resp.error);
    wait("delivered", || rec2.output.lock().unwrap().contains("GOT: ") && rec2.output.lock().unwrap().contains("ping"));
    core2.shutdown();
}

#[test]
fn concurrent_selects_of_an_asleep_row_open_it_once() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "a");
    wait("SessionStart", || statuses(&rec, &v.id).contains(&Status::Idle));
    core.shutdown();
    drop(core);

    let (core2, rec2, _) = setup(tmp.path());
    let gate = Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let (core, gate, id) = (core2.clone(), gate.clone(), v.id.clone());
            std::thread::spawn(move || {
                gate.wait();
                let _ = core.select(&id);
            })
        })
        .collect();
    threads.into_iter().for_each(|t| t.join().unwrap());
    wait("resumed", || rec2.output.lock().unwrap().contains("ARGS: "));
    std::thread::sleep(Duration::from_millis(800));
    assert_eq!(rec2.output.lock().unwrap().matches("ARGS: ").count(), 1);
    core2.shutdown();
}

#[test]
fn rows_get_unique_names_and_children_record_their_parent() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, _rec, _) = setup(tmp.path());
    let mk = |parent: Option<String>| NewSession { prompt: "same".into(), repo: repo.clone(), worktree: false, name: None, parent };
    let a = core.new_session(mk(None)).unwrap();
    let b = core.new_session(mk(Some(a.id.clone()))).unwrap();
    assert_eq!((a.name.as_str(), b.name.as_str()), ("same", "same-2"));
    assert_eq!(b.parent.as_deref(), Some(a.id.as_str()));
    assert_eq!(core.rename(&b.id, "Same").unwrap().name, "same-2");
    core.close(&a.id);
    assert_eq!(core.views().len(), 1);
    core.shutdown();
}

#[test]
fn trust_dialog_marks_a_starting_claude_as_needing_you() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup_with(tmp.path(), "fake-claude-untrusted.sh");
    let v = new(&core, &repo, "x");
    wait("needs you", || status_of(&core, &v.id) == Status::NeedsYou);
    assert_eq!(*rec.attention.lock().unwrap(), vec!["needs_you"]);
    core.shutdown();
}

#[test]
fn a_missing_folder_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let (core, _rec, _) = setup(tmp.path());
    let err = core
        .new_session(NewSession { prompt: "x".into(), repo: "/definitely/not/here".into(), worktree: false, name: None, parent: None })
        .unwrap_err();
    assert!(err.contains("no such folder"), "{err}");
    assert!(core.views().is_empty());
    core.shutdown();
}

#[test]
fn layout_is_stored_and_returned() {
    let tmp = tempfile::tempdir().unwrap();
    let (core, _rec, paths) = setup(tmp.path());
    let tree = serde_json::json!({"split": "row", "ratio": 0.5, "a": {"pane": "x"}, "b": {"pane": "y"}});
    core.save_layout(Some(tree.clone()), Some("y".into()));
    assert_eq!(core.layout(), (Some(tree), Some("y".into())));
    assert!(state_json(&paths).contains("\"focus\": \"y\""));
    core.shutdown();
}
```

`fake-claude-untrusted.sh` stays as it is; it is now reached through the shim.

- [ ] **Step 2: Run to see it fail**

Run: `cargo test --test flow`
Expected: compile errors (`Setup`, `save_layout`, `layout` …). That is the RED.

- [ ] **Step 3: Write the new `engine.rs`**

Replace the whole file:
```rust
use crate::integration;
use crate::past::{self, Past};
use crate::paths::Paths;
use crate::pty::{PtyManager, Spawn};
use crate::shellenv::Env;
use crate::sock::{Handler, LsRow, Request, Response};
use crate::state::{self, Session, Status};
use serde::Serialize;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    pub name: String,
    pub repo: String,
    pub workdir: String,
    pub parent: Option<String>,
    pub status: Status,
    pub since: u64,
    pub created_at: u64,
}

pub enum Attention {
    NeedsYou,
    Done,
}

pub trait Sink: Send + Sync + 'static {
    fn sessions_changed(&self, views: Vec<View>);
    fn session_changed(&self, view: View);
    fn output(&self, id: &str, text: String);
    fn attention(&self, view: &View, kind: Attention);
}

pub struct NewSession {
    pub prompt: String,
    pub repo: String,
    pub worktree: bool,
    pub name: Option<String>,
    pub parent: Option<String>,
}

/// Everything the engine needs from the outside world (the app or a test).
pub struct Setup {
    pub paths: Paths,
    pub env: Env,
    pub claude: Option<PathBuf>,
    pub roost_bin: PathBuf,
    pub shell: PathBuf,
}

struct Live {
    status: Status,
    since: u64,
    claude_pid: Option<u32>,
    pending: Option<String>,
}

struct Inner {
    state: state::State,
    live: HashMap<String, Live>,
}

pub struct Core {
    inner: Mutex<Inner>,
    setup: Setup,
    ptys: PtyManager,
    sink: Arc<dyn Sink>,
    size: Mutex<(u16, u16)>,
    quitting: AtomicBool,
    me: Weak<Core>,
}

fn live(status: Status) -> Live {
    Live { status, since: state::now_ms(), claude_pid: None, pending: None }
}

fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as i32, 0) == 0 } || io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

impl Core {
    pub fn new(setup: Setup, sink: Arc<dyn Sink>) -> Arc<Core> {
        if let Err(e) = integration::install(&setup.paths, &setup.roost_bin) {
            eprintln!("roost: could not install shell integration: {e}");
        }
        let state = state::load(&setup.paths.state());
        let live = state
            .sessions
            .iter()
            .map(|s| (s.id.clone(), live(if s.exited { Status::Exited } else { Status::Asleep })))
            .collect();
        let core = Arc::new_cyclic(|me| Core {
            inner: Mutex::new(Inner { state, live }),
            setup,
            ptys: PtyManager::default(),
            sink,
            size: Mutex::new((120, 36)),
            quitting: AtomicBool::new(false),
            me: me.clone(),
        });
        // Claude killed without SessionEnd (Ctrl-C, crash) still returns its row to the shell.
        let weak = Arc::downgrade(&core);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(2));
            let Some(core) = weak.upgrade() else { break };
            core.reap_claude();
        });
        core
    }

    pub fn handler(self: &Arc<Self>) -> Handler {
        let me = self.clone();
        Arc::new(move |req| me.handle(req))
    }

    pub fn claude_found(&self) -> bool {
        self.setup.claude.is_some()
    }

    pub fn shell_name(&self) -> String {
        self.setup.shell.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "shell".into())
    }

    pub fn recent_repos(&self) -> Vec<String> {
        self.inner.lock().unwrap().state.recent_repos.clone()
    }

    pub fn views(&self) -> Vec<View> {
        let inner = self.inner.lock().unwrap();
        inner.state.sessions.iter().filter_map(|s| view(&inner, &s.id)).collect()
    }

    pub fn layout(&self) -> (Option<Value>, Option<String>) {
        let inner = self.inner.lock().unwrap();
        (inner.state.layout.clone(), inner.state.focus.clone())
    }

    pub fn save_layout(&self, layout: Option<Value>, focus: Option<String>) {
        let mut inner = self.inner.lock().unwrap();
        inner.state.layout = layout;
        inner.state.focus = focus;
        self.save(&inner);
    }

    fn save(&self, inner: &Inner) {
        if let Err(e) = state::save(&self.setup.paths.state(), &inner.state) {
            eprintln!("roost: could not save state: {e}");
        }
    }

    fn emit_all(&self) {
        self.sink.sessions_changed(self.views());
    }

    fn set_status(&self, id: &str, status: Status) {
        let (v, attention) = {
            let mut inner = self.inner.lock().unwrap();
            let Some(l) = inner.live.get_mut(id) else { return };
            if l.status == status || l.status == Status::Exited {
                return;
            }
            let prev = l.status;
            l.status = status;
            l.since = state::now_ms();
            let attention = match (prev, status) {
                (_, Status::NeedsYou) => Some(Attention::NeedsYou),
                (Status::Working, Status::Idle) => Some(Attention::Done),
                _ => None,
            };
            (view(&inner, id), attention)
        };
        if let Some(v) = v {
            if let Some(a) = attention {
                self.sink.attention(&v, a);
            }
            self.sink.session_changed(v);
        }
    }

    fn resolve(&self, target: &str) -> Option<String> {
        let inner = self.inner.lock().unwrap();
        inner.state.sessions.iter().find(|s| s.id == target || s.name == target).map(|s| s.id.clone())
    }

    fn session(&self, id: &str) -> Option<Session> {
        self.inner.lock().unwrap().state.sessions.iter().find(|s| s.id == id).cloned()
    }

    fn status(&self, id: &str) -> Option<Status> {
        self.inner.lock().unwrap().live.get(id).map(|l| l.status)
    }

    fn view(&self, id: &str) -> Option<View> {
        view(&self.inner.lock().unwrap(), id)
    }

    pub fn new_session(&self, req: NewSession) -> Result<View, String> {
        if !Path::new(&req.repo).is_dir() {
            return Err(format!("no such folder: {}", req.repo));
        }
        let prompt = req.prompt.trim();
        if !prompt.is_empty() && self.setup.claude.is_none() {
            return Err("Claude Code not found. Install it, or set ROOST_CLAUDE_BIN.".into());
        }
        let folder = Path::new(&req.repo).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let session = {
            let mut inner = self.inner.lock().unwrap();
            let taken: Vec<&str> = inner.state.sessions.iter().map(|s| s.name.as_str()).collect();
            let label = req.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| {
                if prompt.is_empty() { folder.clone() } else { prompt.to_string() }
            });
            let s = Session {
                id: uuid::Uuid::new_v4().to_string(),
                claude_id: String::new(),
                name: state::unique_name(&state::slug(&label), &taken),
                repo: req.repo.clone(),
                workdir: req.repo.clone(),
                parent: req.parent,
                created_at: state::now_ms(),
                exited: false,
                claude_running: false,
            };
            inner.state.sessions.push(s.clone());
            inner.live.insert(s.id.clone(), live(Status::Shell));
            let recent = &mut inner.state.recent_repos;
            recent.retain(|r| r != &req.repo);
            recent.insert(0, req.repo.clone());
            recent.truncate(10);
            self.save(&inner);
            s
        };
        self.emit_all();
        let line = (!prompt.is_empty())
            .then(|| integration::claude_cmd(prompt, req.worktree.then_some(session.name.as_str())));
        self.spawn(&session, line)?;
        self.view(&session.id).ok_or_else(|| "session vanished".into())
    }

    /// Starts the row's shell; `line` is typed once the shell is ready.
    fn spawn(&self, s: &Session, line: Option<String>) -> Result<(), String> {
        let (cols, rows) = *self.size.lock().unwrap();
        let claude = self.setup.claude.clone().unwrap_or_else(|| PathBuf::from("claude"));
        let spec = Spawn {
            program: self.setup.shell.clone(),
            args: vec!["-l".into()],
            cwd: PathBuf::from(&s.repo),
            env: integration::session_env(&self.setup.env, &self.setup.paths, &self.setup.roost_bin, &claude, &self.setup.shell, &s.id),
            cols,
            rows,
        };
        if let Some(l) = self.inner.lock().unwrap().live.get_mut(&s.id) {
            l.pending = line;
            l.claude_pid = None;
        }
        let sink = self.sink.clone();
        let (out_id, exit_id) = (s.id.clone(), s.id.clone());
        let (out_me, exit_me) = (self.me.clone(), self.me.clone());
        let result = self.ptys.spawn(
            &s.id,
            spec,
            move |text| {
                if let Some(core) = out_me.upgrade() {
                    core.on_output(&out_id, &text);
                }
                sink.output(&out_id, text)
            },
            move || {
                if let Some(core) = exit_me.upgrade() {
                    core.on_exit(&exit_id);
                }
            },
        );
        if let Err(e) = result {
            if e.kind() == io::ErrorKind::AlreadyExists {
                return Ok(()); // someone else already started it
            }
            self.on_exit(&s.id);
            return Err(format!("could not start {}: {e}", self.shell_name()));
        }
        if self.session(&s.id).is_none() {
            self.ptys.kill(&s.id); // the row was closed while we were spawning
            return Ok(());
        }
        // zsh reports its first prompt (ShellReady); other shells get the line after 2 s.
        let (me, id) = (self.me.clone(), s.id.clone());
        thread::spawn(move || {
            thread::sleep(Duration::from_secs(2));
            if let Some(core) = me.upgrade() {
                core.flush_pending(&id);
            }
        });
        Ok(())
    }

    fn flush_pending(&self, id: &str) {
        let line = self.inner.lock().unwrap().live.get_mut(id).and_then(|l| l.pending.take());
        if let Some(line) = line {
            let _ = self.write(id, format!("{line}\r").as_bytes());
        }
    }

    /// Claude asks before trusting a new folder and fires no hooks until it is answered.
    fn on_output(&self, id: &str, text: &str) {
        if self.status(id) == Some(Status::Starting) && plain(text).contains("trust this folder") {
            self.set_status(id, Status::NeedsYou);
        }
    }

    /// The shell exited.
    fn on_exit(&self, id: &str) {
        if self.quitting.load(Ordering::SeqCst) {
            return; // quitting: keep rows so they come back asleep
        }
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if let Some(l) = inner.live.get_mut(id) {
                l.claude_pid = None;
            }
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                s.exited = true;
                s.claude_running = false;
            }
            self.save(inner);
        }
        self.set_status(id, Status::Exited);
    }

    /// Claude ended (SessionEnd) or its pid vanished: back to the plain shell.
    fn claude_gone(&self, id: &str) {
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if let Some(l) = inner.live.get_mut(id) {
                l.claude_pid = None;
            }
            if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                s.claude_running = false;
            }
            self.save(inner);
        }
        self.set_status(id, Status::Shell);
    }

    fn reap_claude(&self) {
        let dead: Vec<String> = {
            let inner = self.inner.lock().unwrap();
            inner
                .live
                .iter()
                .filter(|(_, l)| l.claude_pid.is_some_and(|p| !alive(p)))
                .map(|(id, _)| id.clone())
                .collect()
        };
        for id in dead {
            self.claude_gone(&id);
        }
    }

    /// Opens an asleep row (shell, plus `claude --resume` if Claude was running in it).
    pub fn select(&self, id: &str) -> Result<View, String> {
        if self.claim_asleep(id) {
            let s = self.session(id).ok_or("no such session")?;
            let line = (s.claude_running && !s.claude_id.is_empty())
                .then(|| integration::resume_cmd(&s.workdir, &s.claude_id));
            self.spawn(&s, line)?;
        }
        self.view(id).ok_or_else(|| "no such session".into())
    }

    /// Asleep → Shell in one critical section; only the caller that wins opens it.
    fn claim_asleep(&self, id: &str) -> bool {
        let v = {
            let mut inner = self.inner.lock().unwrap();
            match inner.live.get_mut(id) {
                Some(l) if l.status == Status::Asleep => {
                    l.status = Status::Shell;
                    l.since = state::now_ms();
                }
                _ => return false,
            }
            view(&inner, id)
        };
        if let Some(v) = v {
            self.sink.session_changed(v);
        }
        true
    }

    pub fn write(&self, id: &str, data: &[u8]) -> Result<(), String> {
        self.ptys.write(id, data).map_err(|e| e.to_string())
    }

    pub fn resize(&self, id: &str, cols: u16, rows: u16) {
        *self.size.lock().unwrap() = (cols, rows);
        self.ptys.resize(id, cols, rows);
    }

    pub fn rename(&self, id: &str, name: &str) -> Result<View, String> {
        {
            let mut inner = self.inner.lock().unwrap();
            let taken: Vec<String> =
                inner.state.sessions.iter().filter(|s| s.id != id).map(|s| s.name.clone()).collect();
            let taken: Vec<&str> = taken.iter().map(String::as_str).collect();
            let unique = state::unique_name(&state::slug(name), &taken);
            let s = inner.state.sessions.iter_mut().find(|s| s.id == id).ok_or("no such session")?;
            s.name = unique;
            self.save(&inner);
        }
        let v = self.view(id).ok_or("no such session")?;
        self.sink.session_changed(v.clone());
        Ok(v)
    }

    pub fn close(&self, id: &str) {
        self.ptys.kill(id);
        {
            let mut inner = self.inner.lock().unwrap();
            inner.state.sessions.retain(|s| s.id != id);
            inner.live.remove(id);
            self.save(&inner);
        }
        self.emit_all();
    }

    pub fn stop(&self, target: &str) -> Result<(), String> {
        let id = self.resolve(target).ok_or_else(|| format!("no session named {target}"))?;
        self.ptys.kill(&id);
        Ok(())
    }

    pub fn send(&self, target: &str, text: &str, from: Option<String>) -> Result<(), String> {
        let id = self.resolve(target).ok_or_else(|| format!("no session named {target}"))?;
        let resumable = self.session(&id).is_some_and(|s| s.claude_running);
        if self.status(&id) == Some(Status::Asleep) && resumable {
            self.select(&id)?;
            let end = Instant::now() + Duration::from_secs(15);
            while matches!(self.status(&id), Some(Status::Shell | Status::Starting)) && Instant::now() < end {
                thread::sleep(Duration::from_millis(100));
            }
        }
        if !matches!(self.status(&id), Some(Status::Starting | Status::Idle | Status::Working | Status::NeedsYou)) {
            return Err(format!("{target} isn't running Claude"));
        }
        let prefix = from
            .and_then(|f| self.session(&f))
            .map(|s| format!("[from {}] ", s.name))
            .unwrap_or_default();
        let mut bytes = b"\x1b[200~".to_vec();
        bytes.extend(prefix.as_bytes());
        bytes.extend(text.as_bytes());
        bytes.extend(b"\x1b[201~");
        self.write(&id, &bytes)?;
        thread::sleep(Duration::from_millis(50));
        self.write(&id, b"\r")
    }

    pub fn list_past(&self, repo: &str) -> Vec<Past> {
        let open: HashSet<String> =
            self.inner.lock().unwrap().state.sessions.iter().map(|s| s.claude_id.clone()).collect();
        let home = self.setup.env.get("HOME").cloned().unwrap_or_default();
        past::list_past(&Path::new(&home).join(".claude/projects"), repo, &open)
    }

    pub fn resume_past(&self, repo: &str, p: Past) -> Result<View, String> {
        let id = {
            let mut inner = self.inner.lock().unwrap();
            let taken: Vec<&str> = inner.state.sessions.iter().map(|s| s.name.as_str()).collect();
            let s = Session {
                id: uuid::Uuid::new_v4().to_string(),
                claude_id: p.claude_id,
                name: state::unique_name(&state::slug(&p.title), &taken),
                repo: repo.into(),
                workdir: p.workdir,
                parent: None,
                created_at: state::now_ms(),
                exited: false,
                claude_running: true,
            };
            inner.live.insert(s.id.clone(), live(Status::Asleep));
            inner.state.sessions.push(s.clone());
            self.save(&inner);
            s.id
        };
        self.emit_all();
        self.select(&id)
    }

    pub fn shutdown(&self) {
        self.quitting.store(true, Ordering::SeqCst);
        self.ptys.kill_all();
    }

    pub fn handle(&self, req: Request) -> Response {
        let done = |r: Result<(), String>| r.map_or_else(Response::err, |_| Response::ok());
        match req {
            Request::New { prompt, cwd, worktree, name, parent } => {
                let req = NewSession { prompt, repo: cwd, worktree: worktree.is_some(), name: name.or(worktree), parent };
                match self.new_session(req) {
                    Ok(v) => Response { id: Some(v.id), name: Some(v.name), ..Response::ok() },
                    Err(e) => Response::err(e),
                }
            }
            Request::Ls => {
                let rows = self
                    .views()
                    .into_iter()
                    .map(|v| LsRow { id: v.id, name: v.name, status: v.status, repo: v.repo, parent: v.parent })
                    .collect();
                Response { sessions: Some(rows), ..Response::ok() }
            }
            Request::Send { target, text, from } => done(self.send(&target, &text, from)),
            Request::Stop { target } => done(self.stop(&target)),
            Request::Hook { session, event, payload } => {
                self.hook(&session, &event, &payload);
                Response::ok()
            }
        }
    }

    fn hook(&self, id: &str, event: &str, payload: &Value) {
        match event {
            "ShellReady" => return self.flush_pending(id),
            "SessionEnd" => return self.claude_gone(id),
            _ => {}
        }
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard;
            if !inner.state.sessions.iter().any(|s| s.id == id) {
                return;
            }
            match event {
                "ClaudeStart" => {
                    if let Some(l) = inner.live.get_mut(id) {
                        l.claude_pid = payload["pid"].as_u64().map(|p| p as u32);
                    }
                }
                "SessionStart" => {
                    if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
                        if let Some(c) = payload["session_id"].as_str() {
                            s.claude_id = c.into();
                        }
                        if let Some(c) = payload["cwd"].as_str() {
                            s.workdir = c.into();
                        }
                        s.claude_running = true;
                    }
                    self.save(inner);
                }
                _ => {}
            }
        }
        if let Some(st) = state::status_for(event, payload) {
            self.set_status(id, st);
        }
    }
}

/// Terminal text with CSI escapes (Claude positions words with them) turned into
/// spaces and whitespace collapsed, so phrases can be matched.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() || n == '~' {
                    break;
                }
            }
            out.push(' ');
        } else {
            out.push(c);
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn view(inner: &Inner, id: &str) -> Option<View> {
    let s = inner.state.sessions.iter().find(|s| s.id == id)?;
    let l = inner.live.get(id)?;
    Some(View {
        id: s.id.clone(),
        name: s.name.clone(),
        repo: s.repo.clone(),
        workdir: s.workdir.clone(),
        parent: s.parent.clone(),
        status: l.status,
        since: l.since,
        created_at: s.created_at,
    })
}

pub fn is_git_repo(path: &str) -> bool {
    std::process::Command::new("/usr/bin/git")
        .args(["-C", path, "rev-parse", "--is-inside-work-tree"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_drops_cursor_escapes() {
        assert_eq!(plain("\x1b[4GYes,\x1b[9GI\x1b[11Gtrust\x1b[17Gthis\r\n"), "Yes, I trust this");
    }

    #[test]
    fn detects_git_repos() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_git_repo(&dir.path().display().to_string()));
        std::process::Command::new("/usr/bin/git").args(["init", "-q"]).current_dir(dir.path()).status().unwrap();
        assert!(is_git_repo(&dir.path().display().to_string()));
    }
}
```

In `pty.rs`, delete `pub enum Launch`, `pub fn claude_args`, and the tests `new_session_args`, `worktree_and_dash_prompt_args` and `empty_prompt_and_resume_args`. Remove `Path` from the `std::path` import if it becomes unused.

- [ ] **Step 3b: Update `app.rs` for the new engine**

`app.rs` is part of the library, so it must change in this same task.
- Imports: `use crate::engine::{self, Attention, Core, NewSession, Setup, Sink, View};`; add `use serde_json::Value;`, `use std::collections::HashSet;` and `use std::path::PathBuf;`.
- `UiSink`: replace `selected: Mutex<Option<String>>` with `visible: Mutex<HashSet<String>>`, and in `attention` replace the `selected` lines with:
```rust
        let shown = self.visible.lock().unwrap().contains(&view.id);
        if focused && shown {
            return;
        }
```
- `Boot` and `boot`:
```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Boot {
    sessions: Vec<View>,
    recent_repos: Vec<String>,
    claude_found: bool,
    layout: Option<Value>,
    focus: Option<String>,
    shell: String,
}

#[tauri::command]
fn boot(core: CoreState) -> Boot {
    let (layout, focus) = core.layout();
    Boot {
        sessions: core.views(),
        recent_repos: core.recent_repos(),
        claude_found: core.claude_found(),
        layout,
        focus,
        shell: core.shell_name(),
    }
}
```
- `select_session` no longer tracks the selection:
```rust
#[tauri::command]
fn select_session(id: String, core: CoreState) -> Result<View, String> {
    core.select(&id)
}
```
- Delete the `start_fresh` command. Add:
```rust
/// Pane ids in a saved layout tree.
fn pane_ids(node: Option<&Value>) -> HashSet<String> {
    fn walk(n: &Value, out: &mut HashSet<String>) {
        if let Some(id) = n["pane"].as_str() {
            out.insert(id.to_string());
        }
        for k in ["a", "b"] {
            if let Some(child) = n.get(k) {
                walk(child, out);
            }
        }
    }
    let mut out = HashSet::new();
    if let Some(n) = node {
        walk(n, &mut out);
    }
    out
}

#[tauri::command]
fn save_layout(layout: Option<Value>, focus: Option<String>, core: CoreState, sink: SinkState) {
    *sink.visible.lock().unwrap() = pane_ids(layout.as_ref());
    core.save_layout(layout, focus);
}
```
- In `setup`, remove the `install(...)` / `write_hooks(...)` line and build the engine from a `Setup`, seeding the visible set:
```rust
            let shell = std::env::var_os("ROOST_SHELL")
                .map(PathBuf::from)
                .or_else(|| env.get("SHELL").map(PathBuf::from))
                .unwrap_or_else(|| PathBuf::from("/bin/zsh"));
            let sink = Arc::new(UiSink {
                app: handle,
                pending: Default::default(),
                channels: Default::default(),
                visible: Default::default(),
            });
            let core = Core::new(Setup { paths: paths.clone(), env, claude, roost_bin: exe.clone(), shell }, sink.clone());
            *sink.visible.lock().unwrap() = pane_ids(core.layout().0.as_ref());
```
  (keep `let exe = std::env::current_exe()?;`, `install_cli_link(&exe);`, the env capture and `sock::serve` as they are).
- `generate_handler!`: remove `start_fresh` and add `save_layout`.
- Add at the end of `app.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_ids_walks_the_tree() {
        let tree = serde_json::json!({"split": "row", "ratio": 0.5, "a": {"pane": "x"},
            "b": {"split": "col", "ratio": 0.5, "a": {"pane": "y"}, "b": {"pane": "z"}}});
        let ids = pane_ids(Some(&tree));
        assert_eq!(ids, ["x", "y", "z"].into_iter().map(String::from).collect());
        assert!(pane_ids(None).is_empty());
    }
}
```

- [ ] **Step 4: Run to see them pass**

Run: `cargo build && cargo test`
Expected: no warnings; flow 10 passed; every lib test passes (including `app::tests::pane_ids_walks_the_tree`); cli 2 passed.

- [ ] **Step 5: Commit**

```bash
git add src-tauri
git commit -m "feat: sessions are terminals that track the claude started in them"
```

---

### Task 4: Split-tree logic (`src/layout.ts`)

**Files:**
- Create: `src/layout.ts`, `src/layout.test.ts`

**Interfaces:**
- Produces:
  - Types: `Side = "left" | "right" | "top" | "bottom"`, `Zone = Side | "center"`, `Node = { pane: string } | { split: "row" | "col"; ratio: number; a: Node; b: Node }`, `Tree = Node | null`, `Rect = { left, top, width, height }`.
  - `panes(t) -> string[]` lists the ids in left-to-right / top-to-bottom order.
  - `prune(t, keep: Set<string>) -> Tree` drops ids not in `keep`, plus repeats.
  - `remove(t, id)`, `split(t, target, id, side)`, `replace(t, oldId, newId)` and `swap(t, x, y)`.
  - `move(t, id, target, zone)`. Onto itself is a no-op. On an edge, it removes `id` first and then splits. In the centre, it swaps when `id` is already in the tree and replaces otherwise.
  - `setRatio(t, path: ("a"|"b")[], ratio)` clamps to 0.1–0.9.
  - `zoneAt(rect, x, y)` and `neighbor(rects, from, dir) -> string | null`.

- [ ] **Step 1: Write the failing tests**

`src/layout.test.ts`:
```ts
import { expect, test } from "bun:test";
import { move, neighbor, panes, prune, remove, replace, setRatio, split, swap, zoneAt, type Node } from "./layout";

const p = (id: string): Node => ({ pane: id });
const row = (a: Node, b: Node, ratio = 0.5): Node => ({ split: "row", ratio, a, b });
const col = (a: Node, b: Node, ratio = 0.5): Node => ({ split: "col", ratio, a, b });

test("split puts the new pane on the requested side", () => {
  expect(split(null, "x", "a", "right")).toEqual(p("a"));
  expect(split(p("a"), "a", "b", "right")).toEqual(row(p("a"), p("b")));
  expect(split(p("a"), "a", "b", "left")).toEqual(row(p("b"), p("a")));
  expect(split(p("a"), "a", "b", "top")).toEqual(col(p("b"), p("a")));
  expect(split(p("a"), "a", "b", "bottom")).toEqual(col(p("a"), p("b")));
  expect(split(row(p("a"), p("b")), "b", "c", "bottom")).toEqual(row(p("a"), col(p("b"), p("c"))));
});

test("remove collapses the parent, and removing the last pane leaves null", () => {
  expect(remove(row(p("a"), col(p("b"), p("c"))), "b")).toEqual(row(p("a"), p("c")));
  expect(remove(p("a"), "a")).toBeNull();
});

test("prune drops unknown ids and repeats", () => {
  expect(prune(row(p("a"), row(p("gone"), p("a"))), new Set(["a"]))).toEqual(p("a"));
  expect(prune(null, new Set())).toBeNull();
});

test("panes lists ids in reading order", () => {
  expect(panes(row(p("a"), col(p("b"), p("c"))))).toEqual(["a", "b", "c"]);
});

test("replace and swap", () => {
  expect(replace(row(p("a"), p("b")), "b", "c")).toEqual(row(p("a"), p("c")));
  expect(swap(row(p("a"), col(p("b"), p("c"))), "a", "c")).toEqual(row(p("c"), col(p("b"), p("a"))));
});

test("move: edges split, centre swaps or replaces, onto itself is a no-op", () => {
  const t = row(p("a"), p("b"));
  expect(move(t, "a", "a", "left")).toBe(t);
  expect(move(t, "a", "b", "center")).toEqual(row(p("b"), p("a")));
  expect(move(t, "c", "b", "center")).toEqual(row(p("a"), p("c")));
  expect(move(t, "a", "b", "bottom")).toEqual(col(p("b"), p("a")));
  expect(move(t, "c", "a", "top")).toEqual(row(col(p("c"), p("a")), p("b")));
  expect(panes(move(t, "a", "b", "left")).sort()).toEqual(["a", "b"]);
});

test("setRatio follows the path and clamps", () => {
  const t = row(p("a"), col(p("b"), p("c")));
  expect(setRatio(t, [], 0.3)).toEqual(row(p("a"), col(p("b"), p("c")), 0.3));
  expect(setRatio(t, ["b"], 0.95)).toEqual(row(p("a"), col(p("b"), p("c"), 0.9)));
  expect(setRatio(t, ["b"], 0.01)).toEqual(row(p("a"), col(p("b"), p("c"), 0.1)));
});

test("zoneAt picks the nearest edge within 25%, else the centre", () => {
  const r = { left: 0, top: 0, width: 100, height: 100 };
  expect(zoneAt(r, 5, 50)).toBe("left");
  expect(zoneAt(r, 95, 50)).toBe("right");
  expect(zoneAt(r, 50, 5)).toBe("top");
  expect(zoneAt(r, 50, 95)).toBe("bottom");
  expect(zoneAt(r, 50, 50)).toBe("center");
  expect(zoneAt(r, 10, 4)).toBe("top");
});

test("neighbor finds the nearest pane in a direction", () => {
  const rects = {
    a: { left: 0, top: 0, width: 50, height: 100 },
    b: { left: 51, top: 0, width: 49, height: 50 },
    c: { left: 51, top: 51, width: 49, height: 49 },
  };
  expect(neighbor(rects, "a", "right")).toBe("b");
  expect(neighbor(rects, "b", "bottom")).toBe("c");
  expect(neighbor(rects, "c", "left")).toBe("a");
  expect(neighbor(rects, "a", "left")).toBeNull();
  expect(neighbor(rects, "nope", "left")).toBeNull();
});
```

`src/layout.ts` starts as type definitions plus stubs that throw:
```ts
export type Side = "left" | "right" | "top" | "bottom";
export type Zone = Side | "center";
export type Node = { pane: string } | { split: "row" | "col"; ratio: number; a: Node; b: Node };
export type Tree = Node | null;
export interface Rect {
  left: number;
  top: number;
  width: number;
  height: number;
}

const todo = (): never => {
  throw new Error("not implemented");
};
export const panes = (_t: Tree): string[] => todo();
export const prune = (_t: Tree, _keep: Set<string>): Tree => todo();
export const remove = (_t: Tree, _id: string): Tree => todo();
export const split = (_t: Tree, _target: string, _id: string, _side: Side): Tree => todo();
export const replace = (_t: Tree, _o: string, _n: string): Tree => todo();
export const swap = (_t: Tree, _x: string, _y: string): Tree => todo();
export const move = (_t: Tree, _id: string, _target: string, _zone: Zone): Tree => todo();
export const setRatio = (_t: Tree, _path: ("a" | "b")[], _r: number): Tree => todo();
export const zoneAt = (_r: Rect, _x: number, _y: number): Zone => todo();
export const neighbor = (_rects: Record<string, Rect>, _from: string, _dir: Side): string | null => todo();
```

- [ ] **Step 2: Run to see them fail**

Run: `bun test src/layout.test.ts`
Expected: 9 FAIL with `not implemented`.

- [ ] **Step 3: Implement**

Replace everything after the `Rect` interface:
```ts
const isPane = (n: Node): n is { pane: string } => "pane" in n;
const clamp = (r: number) => Math.min(0.9, Math.max(0.1, r));

export function panes(t: Tree): string[] {
  if (!t) return [];
  return isPane(t) ? [t.pane] : [...panes(t.a), ...panes(t.b)];
}

/** Drops panes whose id isn't in `keep`, and any repeat of an id already shown. */
export function prune(t: Tree, keep: Set<string>): Tree {
  const seen = new Set<string>();
  const walk = (n: Node): Tree => {
    if (isPane(n)) {
      if (!keep.has(n.pane) || seen.has(n.pane)) return null;
      seen.add(n.pane);
      return n;
    }
    const a = walk(n.a);
    const b = walk(n.b);
    if (!a) return b;
    if (!b) return a;
    return { ...n, a, b };
  };
  return t ? walk(t) : null;
}

export function remove(t: Tree, id: string): Tree {
  return prune(t, new Set(panes(t).filter((p) => p !== id)));
}

function mapPanes(t: Tree, f: (id: string) => Node): Tree {
  if (!t) return null;
  if (isPane(t)) return f(t.pane);
  return { ...t, a: mapPanes(t.a, f)!, b: mapPanes(t.b, f)! };
}

/** Puts `id` beside `target` on `side`; an empty tree becomes just `id`. */
export function split(t: Tree, target: string, id: string, side: Side): Tree {
  if (!t) return { pane: id };
  return mapPanes(t, (p) => {
    if (p !== target) return { pane: p };
    const dir = side === "left" || side === "right" ? "row" : "col";
    const first = side === "left" || side === "top";
    const fresh: Node = { pane: id };
    const old: Node = { pane: p };
    return { split: dir, ratio: 0.5, a: first ? fresh : old, b: first ? old : fresh };
  });
}

export function replace(t: Tree, oldId: string, newId: string): Tree {
  return mapPanes(t, (p) => ({ pane: p === oldId ? newId : p }));
}

export function swap(t: Tree, x: string, y: string): Tree {
  return mapPanes(t, (p) => ({ pane: p === x ? y : p === y ? x : p }));
}

/** Drag `id` (already in the tree, or from the sidebar) onto `target`'s `zone`. */
export function move(t: Tree, id: string, target: string, zone: Zone): Tree {
  if (id === target) return t;
  const inTree = panes(t).includes(id);
  if (zone === "center") return inTree ? swap(t, id, target) : replace(t, target, id);
  return split(inTree ? remove(t, id) : t, target, id, zone);
}

export function setRatio(t: Tree, path: ("a" | "b")[], ratio: number): Tree {
  if (!t || isPane(t)) return t;
  if (!path.length) return { ...t, ratio: clamp(ratio) };
  const [head, ...rest] = path;
  return { ...t, [head]: setRatio(t[head], rest, ratio) } as Node;
}

/** The nearest edge if the cursor is within 25% of it, otherwise the centre. */
export function zoneAt(r: Rect, x: number, y: number): Zone {
  const fx = (x - r.left) / r.width;
  const fy = (y - r.top) / r.height;
  const edges: [Side, number][] = [["left", fx], ["right", 1 - fx], ["top", fy], ["bottom", 1 - fy]];
  const [side, dist] = edges.reduce((m, c) => (c[1] < m[1] ? c : m));
  return dist < 0.25 ? side : "center";
}

/** Nearest pane in `dir` that overlaps `from` on the other axis: smallest gap, then centre offset. */
export function neighbor(rects: Record<string, Rect>, from: string, dir: Side): string | null {
  const f = rects[from];
  if (!f) return null;
  const cx = (r: Rect) => r.left + r.width / 2;
  const cy = (r: Rect) => r.top + r.height / 2;
  let best: [string, number] | null = null;
  for (const [id, r] of Object.entries(rects)) {
    if (id === from) continue;
    const horizontal = dir === "left" || dir === "right";
    const gap = horizontal
      ? dir === "right" ? r.left - (f.left + f.width) : f.left - (r.left + r.width)
      : dir === "bottom" ? r.top - (f.top + f.height) : f.top - (r.top + r.height);
    const overlap = horizontal
      ? r.top < f.top + f.height && f.top < r.top + r.height
      : r.left < f.left + f.width && f.left < r.left + r.width;
    if (gap < -1 || !overlap) continue;
    const score = gap * 1000 + (horizontal ? Math.abs(cy(r) - cy(f)) : Math.abs(cx(r) - cx(f)));
    if (!best || score < best[1]) best = [id, score];
  }
  return best ? best[0] : null;
}
```

- [ ] **Step 4: Run to see them pass**

Run: `bun test src/layout.test.ts && npx tsc --noEmit`
Expected: 9 pass; `tsc` reports nothing for `layout.ts`. It may still report `recentRepos`-style issues in files Task 5 rewrites; none exist at this point.

- [ ] **Step 5: Commit**

```bash
git add src/layout.ts src/layout.test.ts
git commit -m "feat: split-tree layout logic with tests"
```

---

### Task 5: UI: terminals in panes, splits, focus, restore

**Files:**
- Rewrite: `src/terminals.ts`, `src/main.ts`
- Create: `src/panes.ts`
- Modify: `src/api.ts`, `src/sidebar.ts`, `src/sheet.ts`, `src/style.css`, `index.html` (no change needed: `#terms` stays the host)
- Update (scratch, untracked): `.superpowers/harness.html`

**Interfaces:**
- Consumes: `layout.ts` (Task 4), and the Tauri commands from Task 3 (`boot` now returns `layout`, `focus` and `shell`; `save_layout`; no `start_fresh`).
- Produces:
  - `terminals.ts`: `mount(id, parent)`, `park(id)`, `refit(id)`, `focus(id)`, `dispose(id)` and `ids()`.
  - `panes.ts`: `interface Hooks { focus(id); rename(id); drop(id, target, zone); ratio(path, r) }`, `render(host, tree, focus, views, shell, hooks)`, `updateHeaders(views, shell)`, `setFocus(id)` and `rects() -> Record<string, Rect>`. Task 6 adds drag & drop and divider dragging inside `render`.
  - `sidebar.render(el, views, focusId, visible: Set<string>, handlers)`.

- [ ] **Step 1: `src/api.ts` changes**

- Change `Status` to `"starting" | "shell" | "asleep" | "working" | "needs_you" | "idle" | "exited"`.
- In `View`, delete `worktree` and `resumeFailed`.
- `Boot` becomes:
```ts
export interface Boot {
  sessions: View[];
  recentRepos: string[];
  claudeFound: boolean;
  layout: Tree;
  focus: string | null;
  shell: string;
}
```
  with `import type { Tree } from "./layout";` at the top.
- In `api`, delete `startFresh` and add:
```ts
  saveLayout: (layout: Tree, focus: string | null) => invoke<void>("save_layout", { layout, focus }),
```

- [ ] **Step 2: `src/terminals.ts` (full replacement)**

```ts
import { Terminal } from "@xterm/xterm";
import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import "@xterm/xterm/css/xterm.css";
import { api } from "./api";

interface Entry {
  term: Terminal;
  fit: FitAddon;
  el: HTMLDivElement;
}

const terms = new Map<string, Entry>();
// Terminals not in any pane live here, still running and still buffering output.
const parking = document.createElement("div");
parking.hidden = true;
document.body.append(parking);

const theme = {
  background: "#0f0f11",
  foreground: "#d6d6dc",
  cursor: "#f5a524",
  cursorAccent: "#0f0f11",
  selectionBackground: "#2a2a31",
};

function create(id: string, parent: HTMLElement): Entry {
  const el = document.createElement("div");
  el.className = "term-host";
  parent.append(el); // open in a visible box so xterm can measure
  const term = new Terminal({
    fontFamily: '"JetBrains Mono", "SF Mono", Menlo, monospace',
    fontSize: 12.5,
    scrollback: 10000,
    cursorBlink: true,
    macOptionIsMeta: true,
    theme,
  });
  const fit = new FitAddon();
  term.loadAddon(fit);
  term.open(el);
  try {
    const gl = new WebglAddon();
    // WebKit keeps ~16 live WebGL contexts and drops the oldest; fall back to the DOM renderer.
    gl.onContextLoss(() => gl.dispose());
    term.loadAddon(gl);
  } catch {
    // no WebGL: xterm keeps its DOM renderer
  }
  term.onData((data) => void api.write(id, data).catch(() => {}));
  // ⌘-keys belong to the app (and to the Edit menu for copy/paste), never to the PTY.
  term.attachCustomKeyEventHandler((ev) => !ev.metaKey);
  void api.attach(id, (text) => term.write(text));
  let timer: number | undefined;
  new ResizeObserver(() => {
    clearTimeout(timer);
    timer = window.setTimeout(() => refit(id), 50);
  }).observe(el);
  return { term, fit, el };
}

export function mount(id: string, parent: HTMLElement) {
  const e = terms.get(id);
  if (!e) terms.set(id, create(id, parent));
  else if (e.el.parentElement !== parent) parent.append(e.el);
}

export function park(id: string) {
  const e = terms.get(id);
  if (e && e.el.parentElement !== parking) parking.append(e.el);
}

export function refit(id: string) {
  const e = terms.get(id);
  if (!e || e.el.parentElement === parking || e.el.clientWidth === 0) return;
  e.fit.fit();
  void api.resize(id, e.term.cols, e.term.rows);
}

export function focus(id: string) {
  terms.get(id)?.term.focus();
}

export function dispose(id: string) {
  const e = terms.get(id);
  if (!e) return;
  e.term.dispose();
  e.el.remove();
  terms.delete(id);
}

export function ids(): string[] {
  return [...terms.keys()];
}
```

- [ ] **Step 3: `src/panes.ts` (new)**

```ts
import type { View } from "./api";
import { panes as paneIds, type Node, type Rect, type Tree, type Zone } from "./layout";
import * as terms from "./terminals";

export interface Hooks {
  focus(id: string): void;
  rename(id: string): void;
  drop(id: string, target: string, zone: Zone): void;
  ratio(path: ("a" | "b")[], ratio: number): void;
}

let host: HTMLElement | null = null;

const div = (cls: string) => {
  const d = document.createElement("div");
  d.className = cls;
  return d;
};

const running = (v: View) => !["shell", "asleep", "exited"].includes(v.status);

function header(el: HTMLElement, v: View | undefined, shell: string) {
  el.className = v ? `ph s-${v.status}` : "ph";
  const name = document.createElement("span");
  name.className = "name";
  name.textContent = v?.name ?? "";
  const what = document.createElement("span");
  what.className = "what";
  what.textContent = v ? (running(v) ? "claude" : shell) : "";
  el.replaceChildren(div("dot"), name, what);
}

/** Rebuilds the pane tree; terminals move into their pane bodies, the rest are parked. */
export function render(el: HTMLElement, tree: Tree, focus: string | null, views: View[], shell: string, hooks: Hooks) {
  host = el;
  const ids = paneIds(tree);
  for (const id of terms.ids()) if (!ids.includes(id)) terms.park(id);
  el.replaceChildren();
  if (!tree) return;
  const bodies = new Map<string, HTMLElement>();
  const build = (n: Node, path: ("a" | "b")[]): HTMLElement => {
    if ("pane" in n) {
      const pane = div(n.pane === focus ? "pane focus" : "pane");
      pane.dataset.id = n.pane;
      const head = div("ph");
      const body = div("pb");
      pane.append(head, body);
      header(head, views.find((v) => v.id === n.pane), shell);
      pane.addEventListener("mousedown", () => hooks.focus(n.pane), true);
      head.ondblclick = () => hooks.rename(n.pane);
      bodies.set(n.pane, body);
      return pane;
    }
    const box = div(`split ${n.split}`);
    const a = build(n.a, [...path, "a"]);
    const b = build(n.b, [...path, "b"]);
    a.style.flex = `${n.ratio} 1 0`;
    b.style.flex = `${1 - n.ratio} 1 0`;
    const divider = div("divider");
    divider.dataset.path = path.join("");
    box.append(a, divider, b);
    return box;
  };
  el.append(build(tree, []));
  for (const [id, body] of bodies) terms.mount(id, body);
  requestAnimationFrame(() => ids.forEach(terms.refit));
}

const allPanes = () => [...(host?.querySelectorAll<HTMLElement>(".pane") ?? [])];

export function updateHeaders(views: View[], shell: string) {
  for (const pane of allPanes()) header(pane.querySelector(".ph")!, views.find((v) => v.id === pane.dataset.id), shell);
}

export function setFocus(id: string) {
  for (const pane of allPanes()) pane.classList.toggle("focus", pane.dataset.id === id);
}

export function rects(): Record<string, Rect> {
  const out: Record<string, Rect> = {};
  for (const pane of allPanes()) {
    const r = pane.getBoundingClientRect();
    out[pane.dataset.id!] = { left: r.left, top: r.top, width: r.width, height: r.height };
  }
  return out;
}
```

- [ ] **Step 4: `src/sidebar.ts` changes**

Change `render`'s signature and row classes; nothing else in the file changes:
```ts
export function render(el: HTMLElement, views: View[], focusId: string | null, visible: Set<string>, on: Handlers) {
```
and in the row loop:
```ts
    row.className = `row s-${v.status}${v.id === focusId ? " on" : ""}${visible.has(v.id) ? "" : " off"}`;
```

- [ ] **Step 5: `src/sheet.ts` changes**

- Placeholder: `What should Claude do? (empty opens a shell)`.
- The name preview falls back to the folder. Add `const preview = () => { $(".nm").textContent = ta.value.trim() ? slug(ta.value) : slug(basename(repo)); };`. Call it from `ta.oninput` (replacing the current body) and at the end of `setRepo`.

- [ ] **Step 6: `src/style.css` changes**

Delete the `.term { … }` and `.term[hidden] { … }` rules, change `#terms` to `#terms { flex: 1; min-height: 0; display: flex; position: relative; }`, and append:
```css
.split { display: flex; flex: 1; min-width: 0; min-height: 0; }
.split.row { flex-direction: row; }
.split.col { flex-direction: column; }
.divider { flex: none; background: var(--line); position: relative; z-index: 2; }
.split.row > .divider { width: 1px; cursor: col-resize; }
.split.col > .divider { height: 1px; cursor: row-resize; }
.divider::after { content: ""; position: absolute; inset: -3px; }
.pane { display: flex; flex-direction: column; flex: 1; min-width: 0; min-height: 0; position: relative; }
.pane.focus { box-shadow: inset 0 0 0 1px #3a3a44; }
.ph { height: 22px; flex: none; display: flex; align-items: center; gap: 6px; padding: 0 10px; font-size: 11px; color: var(--mute); border-bottom: 1px solid var(--line); cursor: grab; }
.pane.focus .ph { color: var(--text); }
.ph .what { margin-left: auto; }
.pb { flex: 1; min-height: 0; position: relative; }
.term-host { position: absolute; inset: 4px 4px 4px 10px; }
.drop { position: absolute; z-index: 5; pointer-events: none; background: rgba(245, 165, 36, .10); border: 1.5px dashed rgba(245, 165, 36, .6); border-radius: 4px; }
.s-shell .dot { border: 1.5px solid #4a4a52; border-radius: 2px; }
.row.off .name { color: var(--mute); }
```

- [ ] **Step 7: `src/main.ts` (full replacement)**

```ts
import { api, type View } from "./api";
import { move, neighbor, panes as paneIds, prune, remove, replace, setRatio, split, type Side, type Tree } from "./layout";
import * as panes from "./panes";
import { openPicker } from "./picker";
import { openSheet } from "./sheet";
import * as sidebar from "./sidebar";
import * as terms from "./terminals";

const list = document.getElementById("list")!;
const host = document.getElementById("terms")!;
const empty = document.getElementById("empty")!;

let views: View[] = [];
let tree: Tree = null;
let focusId: string | null = null;
let recentRepos: string[] = [];
let claudeFound = true;
let shell = "zsh";

const byId = (id: string | null) => views.find((v) => v.id === id);
const ARROWS: Record<string, Side> = { ArrowLeft: "left", ArrowRight: "right", ArrowUp: "top", ArrowDown: "bottom" };

function report(err: unknown) {
  console.error(err);
  const t = document.createElement("div");
  t.className = "toast";
  t.textContent = String(err);
  document.body.append(t);
  setTimeout(() => t.remove(), 4000);
}

let saveTimer: number | undefined;
function persist() {
  clearTimeout(saveTimer);
  saveTimer = window.setTimeout(() => void api.saveLayout(tree, focusId).catch(report), 300);
}

const hooks: panes.Hooks = {
  focus: (id) => focusPane(id),
  rename: (id) => renameRow(id),
  drop: (id, target, zone) => {
    setTree(move(tree, id, target, zone), id);
    void api.select(id).catch(report);
  },
  ratio: (path, r) => {
    tree = setRatio(tree, path, r);
    persist();
  },
};

function drawSidebar() {
  if (list.querySelector(".rename")) return; // never wipe a rename in progress
  sidebar.render(list, views, focusId, new Set(paneIds(tree)), {
    select: (id) => void show(id),
    rename: (id, name) => void api.rename(id, name).catch(report),
  });
}

function drawEmpty() {
  empty.hidden = tree !== null;
  empty.textContent = claudeFound
    ? "Press ⌘N to start a session."
    : "Claude Code not found: terminals work, `claude` won't. Install it, or set ROOST_CLAUDE_BIN.";
}

/** Every layout change goes through here: render, redraw the sidebar, focus, save. */
function setTree(t: Tree, focus: string | null) {
  tree = t;
  const ids = paneIds(tree);
  focusId = focus && ids.includes(focus) ? focus : (ids[0] ?? null);
  panes.render(host, tree, focusId, views, shell, hooks);
  drawEmpty();
  drawSidebar();
  if (focusId) terms.focus(focusId);
  persist();
}

function focusPane(id: string) {
  if (focusId === id) return;
  focusId = id;
  panes.setFocus(id);
  terms.focus(id);
  drawSidebar();
  persist();
}

/** Focus the row's pane if it's on screen, else show it in the focused pane. */
async function show(id: string) {
  if (paneIds(tree).includes(id)) focusPane(id);
  else setTree(tree && focusId ? replace(tree, focusId, id) : { pane: id }, id);
  try {
    await api.select(id);
  } catch (e) {
    report(e);
  }
}

function defaultRepo(): string | undefined {
  return byId(focusId)?.repo ?? recentRepos[0];
}

async function newSession() {
  const repo = defaultRepo() ?? (await api.pickFolder());
  if (!repo) return;
  openSheet({
    repo,
    recentRepos,
    start: async (prompt, repo, worktree) => {
      try {
        const v = await api.create(prompt, repo, worktree);
        recentRepos = [repo, ...recentRepos.filter((r) => r !== repo)].slice(0, 10);
        await show(v.id);
        return true;
      } catch (e) {
        report(e);
        return false;
      }
    },
    closed: () => focusId && terms.focus(focusId),
  });
}

async function resumePast() {
  const repo = defaultRepo() ?? (await api.pickFolder());
  if (!repo) return;
  void openPicker({
    repo,
    pick: async (p) => {
      try {
        const v = await api.resumePast(repo, p);
        await show(v.id);
        return true;
      } catch (e) {
        report(e);
        return false;
      }
    },
    closed: () => focusId && terms.focus(focusId),
  });
}

async function splitFocused(side: Side) {
  const src = byId(focusId);
  if (!src || !focusId) return void newSession();
  try {
    const v = await api.create("", src.repo, false);
    setTree(split(tree, focusId, v.id, side), v.id);
  } catch (e) {
    report(e);
  }
}

function neighborOf(id: string): string | null {
  const r = panes.rects();
  for (const d of ["right", "left", "bottom", "top"] as Side[]) {
    const n = neighbor(r, id, d);
    if (n) return n;
  }
  return null;
}

async function closeFocused() {
  const v = byId(focusId);
  if (!v) return;
  if (v.status === "working" && !(await api.ask(`${v.name} is working. Close it?`, "Close"))) return;
  const next = neighborOf(v.id);
  await api.close(v.id).catch(report);
  terms.dispose(v.id);
  setTree(remove(tree, v.id), next);
}

function removePane() {
  if (!focusId) return;
  setTree(remove(tree, focusId), neighborOf(focusId));
}

function moveFocus(dir: Side) {
  if (!focusId) return;
  const n = neighbor(panes.rects(), focusId, dir);
  if (n) focusPane(n);
}

function jumpNeedsYou(order: string[]) {
  const needs = new Set(views.filter((v) => v.status === "needs_you").map((v) => v.id));
  const start = focusId ? order.indexOf(focusId) : -1;
  for (let i = 1; i <= order.length; i++) {
    const id = order[(start + i + order.length) % order.length];
    if (needs.has(id)) return void show(id);
  }
}

function renameRow(id: string | null) {
  const v = byId(id);
  const nameEl = id ? list.querySelector<HTMLElement>(`.row[data-id="${CSS.escape(id)}"] .name`) : null;
  if (v && nameEl) sidebar.startRename(nameEl, v, (rid, name) => void api.rename(rid, name).catch(report));
}

const overlayOpen = () => document.querySelector(".overlay") !== null;

window.addEventListener("keydown", (e) => {
  if (!e.metaKey || overlayOpen()) return;
  const order = sidebar.ordered(views).map((x) => x.view.id);
  const key = e.key.toLowerCase();
  let handled = true;
  if (key === "n" && !e.shiftKey) void newSession();
  else if (key === "o") void resumePast();
  else if (key === "j") jumpNeedsYou(order);
  else if (key === "d") void splitFocused(e.shiftKey ? "bottom" : "right");
  else if (key === "w" && e.shiftKey) removePane();
  else if (key === "w") void closeFocused();
  else if (key === "r" && e.shiftKey) renameRow(focusId);
  else if (e.altKey && e.key in ARROWS) moveFocus(ARROWS[e.key]);
  else if (/^Digit[1-9]$/.test(e.code)) {
    const id = order[Number(e.code.slice(5)) - 1];
    if (id) void show(id);
  } else handled = false;
  if (handled) e.preventDefault();
});

api.onSessions((v) => {
  views = v;
  const pruned = prune(tree, new Set(views.map((x) => x.id)));
  if (JSON.stringify(pruned) !== JSON.stringify(tree)) setTree(pruned, focusId);
  else {
    panes.updateHeaders(views, shell);
    drawSidebar();
  }
});
api.onSession((v) => {
  views = views.map((x) => (x.id === v.id ? v : x));
  panes.updateHeaders(views, shell);
  drawSidebar();
});
setInterval(drawSidebar, 10_000);
document.getElementById("new")!.onclick = () => void newSession();

const boot = await api.boot();
views = boot.sessions;
recentRepos = boot.recentRepos;
claudeFound = boot.claudeFound;
shell = boot.shell;
setTree(prune(boot.layout, new Set(views.filter((v) => v.status !== "exited").map((v) => v.id))), boot.focus);
for (const id of paneIds(tree)) void api.select(id).catch(report);
```

- [ ] **Step 8: Update the browser harness (scratch)**

In `.superpowers/harness.html`, update the mock so the UI can boot:
- Give mocked views statuses that include `"shell"`, and drop `worktree` and `resumeFailed` from `v(...)`.
- `boot` returns `{ sessions, recentRepos, claudeFound: true, layout: { split: "row", ratio: 0.5, a: { pane: "a" }, b: { split: "col", ratio: 0.5, a: { pane: "b" }, b: { pane: "d" } } }, focus: "b", shell: "zsh" }`.
- Add `case "save_layout": window.__saved = args; return null;`.
- `new_session` returns a view with status `"shell"` and, as now, emits `sessions_changed`.

- [ ] **Step 9: Check it**

Run: `npx tsc --noEmit && bun test && bun run build`
Expected: clean.

Then `bun run dev` and open `http://localhost:1420/.superpowers/harness.html` in Chrome (chrome-devtools MCP). Check, with an evaluate script:
- Three panes render (one left, two stacked on the right); the focused one has the outline.
- Each pane header shows a dot, the name and `claude` or `zsh`.
- ⌘D on the focused pane gives 4 panes with the new one focused; ⌘⇧W gives 3 again, with the terminal still in the sidebar and dimmed.
- ⌘⌥← moves focus to the left pane; ⌘W closes it and the right column takes the space.
- Clicking a dimmed sidebar row shows it in the focused pane.
- `window.__saved` holds the current tree.

Take one screenshot to compare against the approved option A mockup.

- [ ] **Step 10: Commit**

```bash
git add src index.html
git commit -m "feat: terminals in a split layout with focus, splits and restore"
```

---

### Task 6: Drag & drop and resizing

**Files:**
- Modify: `src/panes.ts`, `src/sidebar.ts`

**Interfaces:**
- Consumes: `Hooks.drop` and `Hooks.ratio` (already wired in `main.ts` by Task 5), and `zoneAt` from Task 4.
- Produces: `panes.DRAG_TYPE = "text/roost-id"`; sidebar rows and pane headers become drag sources; panes become drop targets; dividers can be dragged and double-clicked.

Rules: the drop overlay covers the half of the pane on the zone's side, or the whole body for the centre. Divider dragging clamps to 0.1–0.9 and changes the flex live. On mouse-up it calls `hooks.ratio`, and the terminals refit through their ResizeObservers. Double-clicking a divider resets it to 0.5.

- [ ] **Step 1: Write the failing check (harness)**

With `bun run dev` running, reload the harness and run this in the page (chrome-devtools `evaluate_script`). Save it as `.superpowers/dnd-check.js` so it can be re-run:
```js
async () => {
  const wait = (ms) => new Promise((r) => setTimeout(r, ms));
  const out = {};
  const dt = new DataTransfer();
  const row = [...document.querySelectorAll(".row")].find((r) => r.classList.contains("off"));
  const target = document.querySelector(".pane");
  const r = target.getBoundingClientRect();
  const at = { clientX: r.right - 5, clientY: r.top + r.height / 2, bubbles: true, cancelable: true, dataTransfer: dt };
  row.dispatchEvent(new DragEvent("dragstart", { dataTransfer: dt, bubbles: true }));
  target.dispatchEvent(new DragEvent("dragover", at));
  out.overlayZone = target.querySelector(".drop")?.dataset.zone ?? null;
  target.dispatchEvent(new DragEvent("drop", at));
  await wait(400);
  out.panesAfterDrop = document.querySelectorAll(".pane").length;
  out.droppedVisible = !!document.querySelector(`.pane[data-id="${row.dataset.id}"]`);
  const divider = document.querySelector(".divider");
  const box = divider.parentElement.getBoundingClientRect();
  divider.dispatchEvent(new MouseEvent("mousedown", { bubbles: true, clientX: box.left + box.width / 2, clientY: box.top + 10 }));
  window.dispatchEvent(new MouseEvent("mousemove", { clientX: box.left + box.width * 0.3, clientY: box.top + 10 }));
  window.dispatchEvent(new MouseEvent("mouseup", {}));
  await wait(400);
  out.savedRatio = window.__saved?.layout?.ratio;
  return out;
}
```
Expected before the change: `overlayZone: null`, `panesAfterDrop` unchanged and `droppedVisible: false`. That is the RED.

- [ ] **Step 2: Implement in `src/panes.ts`**

Add `zoneAt` to the `layout` import (`import { panes as paneIds, zoneAt, type Node, type Rect, type Tree, type Zone } from "./layout";`), then add above `render`:
```ts
export const DRAG_TYPE = "text/roost-id";

const ZONE_BOX: Record<Zone, string> = {
  left: "left:0;top:22px;bottom:0;width:50%",
  right: "right:0;top:22px;bottom:0;width:50%",
  top: "left:0;right:0;top:22px;height:calc(50% - 11px)",
  bottom: "left:0;right:0;bottom:0;height:calc(50% - 11px)",
  center: "left:0;right:0;top:22px;bottom:0",
};

function dropTarget(pane: HTMLElement, id: string, hooks: Hooks) {
  let overlay: HTMLElement | null = null;
  const clear = () => {
    overlay?.remove();
    overlay = null;
  };
  pane.addEventListener("dragover", (e) => {
    if (!e.dataTransfer?.types.includes(DRAG_TYPE)) return;
    e.preventDefault();
    const r = pane.getBoundingClientRect();
    const zone = zoneAt({ left: r.left, top: r.top, width: r.width, height: r.height }, e.clientX, e.clientY);
    overlay ??= pane.appendChild(div("drop"));
    overlay.setAttribute("style", ZONE_BOX[zone]);
    overlay.dataset.zone = zone;
  });
  pane.addEventListener("dragleave", (e) => {
    if (!pane.contains(e.relatedTarget as HTMLElement | null)) clear();
  });
  pane.addEventListener("drop", (e) => {
    e.preventDefault();
    const dragged = e.dataTransfer?.getData(DRAG_TYPE);
    const zone = overlay?.dataset.zone as Zone | undefined;
    clear();
    if (dragged && zone) hooks.drop(dragged, id, zone);
  });
}

function resizable(box: HTMLElement, divider: HTMLElement, dir: "row" | "col", path: ("a" | "b")[], hooks: Hooks) {
  const sides = () => [divider.previousElementSibling as HTMLElement, divider.nextElementSibling as HTMLElement];
  const apply = (ratio: number) => {
    const [a, b] = sides();
    a.style.flex = `${ratio} 1 0`;
    b.style.flex = `${1 - ratio} 1 0`;
  };
  divider.addEventListener("mousedown", (e) => {
    e.preventDefault();
    const r = box.getBoundingClientRect();
    let ratio = 0.5;
    const onMove = (ev: MouseEvent) => {
      const raw = dir === "row" ? (ev.clientX - r.left) / r.width : (ev.clientY - r.top) / r.height;
      ratio = Math.min(0.9, Math.max(0.1, raw));
      apply(ratio);
    };
    const onUp = () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      hooks.ratio(path, ratio); // terminals refit through their ResizeObservers
    };
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  });
  divider.addEventListener("dblclick", () => {
    apply(0.5);
    hooks.ratio(path, 0.5);
  });
}
```
In `render`'s `build`, wire them up. In the pane branch, after `head.ondblclick = …`:
```ts
      head.draggable = true;
      head.addEventListener("dragstart", (e) => {
        e.dataTransfer?.setData(DRAG_TYPE, n.pane);
        if (e.dataTransfer) e.dataTransfer.effectAllowed = "move";
      });
      dropTarget(pane, n.pane, hooks);
```
In the split branch, after `divider.dataset.path = …`:
```ts
    resizable(box, divider, n.split, path, hooks);
```

- [ ] **Step 3: Make sidebar rows draggable in `src/sidebar.ts`**

Add `import { DRAG_TYPE } from "./panes";` and, in the row loop after `row.ondblclick = …`:
```ts
    row.draggable = true;
    row.addEventListener("dragstart", (e) => e.dataTransfer?.setData(DRAG_TYPE, v.id));
```

- [ ] **Step 4: Run the check again**

Run: `npx tsc --noEmit && bun test && bun run build`, then reload the harness and re-run `.superpowers/dnd-check.js`.
Expected:
- `overlayZone: "right"`;
- `panesAfterDrop` is one more than before;
- `droppedVisible: true`;
- `savedRatio` is about `0.3` (between 0.25 and 0.35).

Also drag a pane header onto another pane's centre with the same script shape (source `.pane .ph`, zone centre at the middle of the target), and check that the two ids swapped positions in `window.__saved.layout`.

- [ ] **Step 5: Commit**

```bash
git add src
git commit -m "feat: drag panes and sessions to dock, drag dividers to resize"
```

---

### Task 7: Release check

**Files:**
- Modify: `docs/specs/2026-10-11-roost-terminals-and-splits-design.md` (record this plan's deviations)

- [ ] **Step 1: Everything green**

Run: `cd src-tauri && cargo build && cargo test && cd .. && bun test && bun run build`
Expected: no warnings; every Rust test passes (lib, cli, flow 10); the bun tests pass (9); the build succeeds.

- [ ] **Step 2: Record the deviations in the spec addendum**

In "Typing commands into a terminal", replace the first sentence with:

"Roost queues the command and types it, plus `\r`, once the shell is ready: zsh reports its first prompt through the `ShellReady` hook installed by Roost's `.zshenv`, and other shells get it after 2 s. Prompts always follow `--`, so a one-word or dash-leading prompt still goes through the shim's tracked path."

In the typed-commands table, change the first two rows to `claude -- '<prompt>'` and `claude -w '<name>' -- '<prompt>'`.

In "The `claude` shim", replace the sentence about `claude hi` running untracked with: "Roost's own typed commands always use `--`, so this only affects `claude hi` typed by hand."

- [ ] **Step 3: Release build and real-Claude smoke test**

```bash
pkill -f "/Applications/Roost.app/Contents/MacOS/roost"
bun run tauri build
ditto src-tauri/target/release/bundle/macos/Roost.app /Applications/Roost.app
open /Applications/Roost.app
```
Then, once `roost ls` answers:
```bash
roost new "reply with exactly one word: hi" --cwd ~/Dev
roost ls          # repeat: shell -> starting -> idle
roost send reply-with-exactly-one '/exit'
roost ls          # shell
roost send reply-with-exactly-one 'hi'   # refused: isn't running Claude
roost stop reply-with-exactly-one
```
Expected: the statuses as commented; the session's transcript exists under `~/.claude/projects/-Users-zjov-Dev/`. Afterwards, quit Roost and remove the test row from `state.json`.

- [ ] **Step 4: Native-window checklist (for the user)**

1. ⌘N with an empty prompt gives a zsh prompt in the repo. Typing `claude` lights the row up; `/exit` returns it to the square dot.
2. ⌘N with a prompt opens a shell that runs `claude -- '<prompt>'` by itself.
3. ⌘D and ⌘⇧D split. Drag a sidebar row onto the right edge of a pane, and a pane header onto another pane's centre. Drag and double-click a divider.
4. ⌘⌥←→↑↓ move focus between panes. ⌘⇧W hides a pane and keeps its terminal; clicking the dimmed row brings it back.
5. Quit with Claude running in two split panes and reopen: the same split comes back, and both resume their conversations.

- [ ] **Step 4b: Commit**

```bash
git add docs/specs
git commit -m "docs: record typed-command readiness and -- in the addendum"
```
