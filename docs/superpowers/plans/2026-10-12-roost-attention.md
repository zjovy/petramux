# Attention, Diffs and Sidebar Rail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Roost shows which session needs you and why (status fixes, notes, unread markers), shows what Claude changed in its last turn (a diff pane), and gets a collapsible sidebar plus macOS line-editing keys.

**Architecture:**
- Roost's hooks move into a Claude Code plugin loaded with `--plugin-dir`, which adds `StopFailure` and `PreToolUse` (AskUserQuestion / ExitPlanMode).
- The engine keeps a cleaned one-line `note` per session. The `roost hook UserPromptSubmit` process records a git tree snapshot (`turn_base`) before Claude acts.
- A `diff` command compares the snapshot with a fresh one. The frontend renders it in a layout leaf with id `diff:<session>`.
- Unread state, the ⌘J order, the ⌘B rail and the editing keys live in the frontend.

**Tech Stack:** Rust (Tauri 2, portable-pty, serde_json), TypeScript (xterm.js, Vite), `bun test`, git CLI.

**Spec:** `docs/specs/2026-10-12-roost-attention-design.md`

## Global Constraints

- The note is the first non-empty line, with markdown markers stripped and whitespace collapsed, at most 120 characters ending in `…`.
- Every git call runs with `GIT_OPTIONAL_LOCKS=0`. A snapshot leaves the real index and working tree untouched, and its `git add -A` is limited to 3 s.
- Diff output is cut at 1 MB with a final `… diff truncated` line.
- Diff errors are exactly:
  - `Not a git repository.`
  - `No turn yet. Send Claude a prompt first.`
  - `Couldn't read the diff: …`
- The rail is 44px wide, and its state lives in `localStorage` key `roost.rail` (wrapped in try/catch).
- Editing keys are Ghostty's defaults:
  - ⌘⌫ `\x15`, ⌘⌦ `\x0b`, ⌘← `\x01`, ⌘→ `\x05`;
  - ⌥⌫ `\x1b\x7f`, ⌥⌦ `\x1bd`, ⌥← `\x1bb`, ⌥→ `\x1bf`.
- Never kill or restart the user's running `/Applications/Roost.app`, and never edit their `state.json`.
- Run cargo with `source "$HOME/.cargo/env"` first. Rust tests run from `src-tauri/`; bun runs from the repo root.

## Review Focus

1. **A repo with no commits, a non-repo folder, or a deleted worktree.** The diff pane shows a plain sentence, never a crash or a hang. Task 3 has `all_changes_work_without_commits` and `outside_a_repo_there_is_no_snapshot_and_a_plain_error`.
2. **Notes made of long, emoji-heavy or multi-line markdown text.** They're cut on a character boundary, as one clean line. Task 2 has `notes_are_one_clean_short_line`.
3. **Diff text with `+++` inside a hunk, spaces in paths, binary files, deletions or renames.** Each file is parsed with the right path and counts. Task 6 has the `parse` tests.
4. **The window loses focus while the focused session finishes.** The session is still marked unread, and coming back clears it. Task 5's harness check covers this.
5. **A saved layout holds a diff leaf for a session that has since closed.** The leaf is dropped at boot with no error. Task 6 has the `keep` test and a harness check.

---

### Task 1: Hooks as a plugin, StopFailure and PreToolUse

**Files:**
- Modify: `src-tauri/src/paths.rs`, `src-tauri/src/integration.rs`, `src-tauri/src/state.rs` (`status_for`), `src-tauri/tests/flow.rs` (ARGS assertion)

**Interfaces:**
- Produces:
  - `Paths::plugin() -> PathBuf` (`<support>/plugin`);
  - the env var `ROOST_PLUGIN`;
  - `hooks_json` with 8 events;
  - `status_for("StopFailure", _) == Some(Idle)`;
  - `status_for("PreToolUse", {tool_name: AskUserQuestion|ExitPlanMode}) == Some(NeedsYou)`.

- [ ] **Step 1: Failing tests.**
  - In `integration.rs` tests:
    - `hooks_cover_six_events_including_session_end` becomes `hooks_cover_eight_events`. It asserts `hooks.len() == 8`, `hooks["PreToolUse"][0]["matcher"] == "AskUserQuestion|ExitPlanMode"` and `hooks["StopFailure"][0]["hooks"][0]["command"] == "'/Apps/It'\\''s Roost/roost' hook StopFailure"`.
    - `install_writes_executable_shim_and_zshenv` asserts that `paths.plugin().join(".claude-plugin/plugin.json")` parses with `name == "roost"`, that `paths.plugin().join("hooks/hooks.json")` contains `'/x/roost' hook SessionEnd`, and that a pre-existing `paths.dir.join("hooks.json")` (written before `install`) is gone afterwards.
    - In `run_shim_with`, replace `.env("ROOST_HOOKS", "/h.json")` with `.env("ROOST_PLUGIN", "/plug")`.
    - `shim_adds_hooks_and_reports_its_pid` expects `"--plugin-dir|/plug|--append-system-prompt|B|--|fix it|"`.
    - `session_env_sets_roost_vars…` expects `z["ROOST_PLUGIN"] == paths.plugin().display().to_string()` and `!z.contains_key("ROOST_HOOKS")`.
  - In `state.rs` tests:
    ```rust
    #[test]
    fn failures_end_the_turn_and_questions_need_you() {
        let j = |v: &str| serde_json::from_str::<serde_json::Value>(v).unwrap();
        assert_eq!(status_for("StopFailure", &j(r#"{"error":"rate_limit"}"#)), Some(Status::Idle));
        assert_eq!(status_for("PreToolUse", &j(r#"{"tool_name":"AskUserQuestion"}"#)), Some(Status::NeedsYou));
        assert_eq!(status_for("PreToolUse", &j(r#"{"tool_name":"ExitPlanMode"}"#)), Some(Status::NeedsYou));
        assert_eq!(status_for("PreToolUse", &j(r#"{"tool_name":"Bash"}"#)), None);
    }
    ```
  - In `flow.rs` `a_terminal_tracks_claude_started_in_it`, the ARGS assertion becomes `out.contains("ARGS: --plugin-dir ")`.

- [ ] **Step 2: Run the tests and see them fail.** `cargo test --lib integration state 2>&1 | tail -30`. Expected: compile errors (no `plugin()`) or assertion failures.

- [ ] **Step 3: Implement.**
  - `paths.rs`: replace `hooks()` with
    ```rust
    pub fn plugin(&self) -> PathBuf {
        self.dir.join("plugin")
    }
    ```
  - `integration.rs`:
    - In SHIM, replace `[ -z "$ROOST_HOOKS" ]` with `[ -z "$ROOST_PLUGIN" ]`, and the last line with `exec "$real" --plugin-dir "$ROOST_PLUGIN" --append-system-prompt "$ROOST_BLURB" "$@"`.
    - In `hooks_json`, add `"PreToolUse": entry(Some("AskUserQuestion|ExitPlanMode"), "PreToolUse"),` and `"StopFailure": entry(None, "StopFailure"),`, and change its doc comment to "The hooks of Roost's Claude Code plugin".
    - Add the plugin manifest:
      ```rust
      /// `<support>/plugin/.claude-plugin/plugin.json`: hooks load with `--plugin-dir`, so a user's own `--settings` keeps them.
      const PLUGIN_JSON: &str = r#"{"name":"roost","version":"1.0.0","description":"Reports this Claude session's status to Roost."}"#;
      ```
    - `install` writes `.claude-plugin/plugin.json` and `hooks/hooks.json` under `paths.plugin()` (`create_dir_all` both), then runs `let _ = std::fs::remove_file(paths.dir.join("hooks.json")); // written by Roost before the plugin`.
    - `session_env` sets `env.insert("ROOST_PLUGIN".into(), paths.plugin().display().to_string());` in place of `ROOST_HOOKS`.
  - `state.rs` `status_for`:
    ```rust
    "SessionStart" | "Stop" | "StopFailure" => Some(Status::Idle),
    "PreToolUse" => match payload["tool_name"].as_str() {
        Some("AskUserQuestion" | "ExitPlanMode") => Some(Status::NeedsYou),
        _ => None,
    },
    ```

- [ ] **Step 4: Run the tests and see them pass.** `cargo test 2>&1 | tail -15`. Expected: everything passes.

- [ ] **Step 5: Commit.** `git commit -am "feat: load hooks as a Claude plugin; failed turns end, questions need you"`

### Task 2: Notes

**Files:**
- Modify:
  - `src-tauri/src/state.rs` (`Session.note`, `Session.turn_base`, `note_for`, `clean_note`, test helper `sess`);
  - `src-tauri/src/engine.rs` (`View.note`, hook applies the note, `claude_gone` clears it, `set_status -> bool`);
  - `src-tauri/src/app.rs` (notification title and body);
  - `src-tauri/tests/fake-claude.sh` (`/fail`, `/ask`, `/turn`);
  - `src-tauri/tests/flow.rs`.

**Interfaces:**
- Consumes the Task 1 statuses.
- Produces:
  - `Session { note: Option<String>, turn_base: Option<String>, .. }`, both `#[serde(default, skip_serializing_if = "Option::is_none")]`;
  - `View.note: Option<String>`, serialized as `note`;
  - `state::note_for(event, &Value) -> Option<Option<String>>`;
  - `state::clean_note(&str) -> Option<String>`.

- [ ] **Step 1: Failing tests.**
  - In `state.rs` tests:
    ```rust
    #[test]
    fn notes_are_one_clean_short_line() {
        assert_eq!(clean_note("\n\n## **Done**: fixed `auth.ts`\nmore"), Some("Done: fixed auth.ts".into()));
        assert_eq!(clean_note("  1. first   step "), Some("first step".into()));
        assert_eq!(clean_note(" \n "), None);
        let n = clean_note(&"é🙂".repeat(100)).unwrap();
        assert_eq!(n.chars().count(), 120);
        assert!(n.ends_with('…'));
    }

    #[test]
    fn events_set_clear_or_keep_the_note() {
        let j = |v: &str| serde_json::from_str::<serde_json::Value>(v).unwrap();
        let set = |s: &str| Some(Some(s.to_string()));
        assert_eq!(note_for("UserPromptSubmit", &j(r#"{"prompt":"fix the login bug\nplease"}"#)), set("fix the login bug"));
        assert_eq!(note_for("Stop", &j(r#"{"last_assistant_message":"All tests pass."}"#)), set("All tests pass."));
        assert_eq!(note_for("Stop", &j("{}")), None);
        assert_eq!(note_for("StopFailure", &j(r#"{"error":"rate_limit"}"#)), set("⚠ rate limit"));
        assert_eq!(note_for("Notification", &j(r#"{"message":"Claude needs your permission to use Bash"}"#)), set("Claude needs your permission to use Bash"));
        assert_eq!(note_for("Notification", &j(r#"{"notification_type":"idle_prompt","message":"waiting"}"#)), None);
        assert_eq!(note_for("PreToolUse", &j(r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which DB?"}]}}"#)), set("Which DB?"));
        assert_eq!(note_for("PreToolUse", &j(r#"{"tool_name":"ExitPlanMode"}"#)), set("Plan ready for review"));
        assert_eq!(note_for("PreToolUse", &j(r#"{"tool_name":"Bash"}"#)), None);
        assert_eq!(note_for("SessionStart", &j(r#"{"source":"clear"}"#)), Some(None));
        assert_eq!(note_for("SessionStart", &j(r#"{"source":"resume"}"#)), None);
        assert_eq!(note_for("PostToolUse", &j("{}")), None);
    }
    ```
  - In `fake-claude.sh`, add these to the `case`:
    ```sh
    *"/fail"*) hook StopFailure '{"error":"rate_limit"}' ;;
    *"/ask"*) hook PreToolUse '{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which database?"}]}}' ;;
    *"/turn"*) hook UserPromptSubmit "{\"prompt\":\"edit it\",\"cwd\":\"$(pwd -P)\"}"
               echo changed >> edited.txt
               hook Stop '{"last_assistant_message":"Edited edited.txt."}' ;;
    ```
  - In `flow.rs`:
    ```rust
    fn note_of(core: &Core, id: &str) -> Option<String> {
        core.views().into_iter().find(|v| v.id == id).unwrap().note
    }

    #[test]
    fn notes_follow_the_conversation_and_failures_end_the_turn() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = repo(&tmp);
        let (core, rec, paths) = setup(tmp.path());
        let v = new(&core, &repo, "x");
        wait("idle", || statuses(&rec, &v.id).len() >= 6);
        core.send(&v.id, "/ask", None).unwrap();
        wait("question", || status_of(&core, &v.id) == Status::NeedsYou);
        assert_eq!(note_of(&core, &v.id).as_deref(), Some("Which database?"));
        core.send(&v.id, "/turn", None).unwrap();
        wait("reply note", || note_of(&core, &v.id).as_deref() == Some("Edited edited.txt."));
        assert!(state_json(&paths).contains("Edited edited.txt."));
        core.send(&v.id, "/fail", None).unwrap();
        wait("failure note", || note_of(&core, &v.id).as_deref() == Some("⚠ rate limit"));
        assert_eq!(status_of(&core, &v.id), Status::Idle);
        core.send(&v.id, "/exit", None).unwrap();
        wait("note cleared with claude gone", || status_of(&core, &v.id) == Status::Shell && note_of(&core, &v.id).is_none());
        core.shutdown();
    }
    ```

- [ ] **Step 2: Run and see it fail.** `cargo test 2>&1 | grep -E "error|FAILED|panicked" | head`. Expected: no `clean_note`, `note_for` or `View.note` yet.

- [ ] **Step 3: Implement.**
  - `state.rs`: add both fields to `Session` and `note: None, turn_base: None` to the `sess` helper. Then add:
    ```rust
    /// What an event says about a session's note: `Some(Some(text))` sets it, `Some(None)`
    /// clears it, `None` leaves it. Claude's exit clears it in the engine.
    pub fn note_for(event: &str, p: &serde_json::Value) -> Option<Option<String>> {
        let text = |v: &serde_json::Value| v.as_str().and_then(clean_note);
        match event {
            "UserPromptSubmit" => Some(text(&p["prompt"])),
            "Stop" => text(&p["last_assistant_message"]).map(Some),
            "StopFailure" => {
                let err = p["error"].as_str().unwrap_or("unknown error").replace('_', " ");
                Some(clean_note(&format!("⚠ {err}")))
            }
            "Notification" if p["notification_type"] != "idle_prompt" => text(&p["message"]).map(Some),
            "PreToolUse" => match p["tool_name"].as_str() {
                Some("AskUserQuestion") => {
                    Some(text(&p["tool_input"]["questions"][0]["question"]).or_else(|| Some("Claude has a question".into())))
                }
                Some("ExitPlanMode") => Some(Some("Plan ready for review".into())),
                _ => None,
            },
            "SessionStart" if matches!(p["source"].as_str(), Some("startup" | "clear")) => Some(None),
            _ => None,
        }
    }

    /// One short line for the sidebar: the first non-empty line, without markdown markers,
    /// whitespace collapsed, at most 120 characters.
    pub fn clean_note(text: &str) -> Option<String> {
        let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
        let line = line.trim_start_matches(|c: char| matches!(c, '#' | '>' | '-' | '*' | '+') || c.is_whitespace());
        let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        let line = match line[digits..].strip_prefix(". ") {
            Some(rest) if digits > 0 => rest,
            _ => line,
        };
        let line = line.replace("**", "").replace("__", "").replace('`', "");
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() {
            return None;
        }
        Some(if line.chars().count() > 120 { format!("{}…", line.chars().take(119).collect::<String>()) } else { line })
    }
    ```
  - `engine.rs`:
    - Add `pub note: Option<String>` to `View` and `note: s.note.clone()` in `view()`.
    - Add `note: None, turn_base: None` to the two `Session { .. }` literals.
    - `set_status` returns `bool`: true when it emitted.
    - In `hook`, inside the locked block after the event match, add:
      ```rust
      let note = state::note_for(event, payload);
      if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
          if let Some(n) = note.filter(|n| *n != s.note) {
              s.note = n;
              note_changed = true;
          }
      }
      if note_changed {
          self.save(inner);
      }
      ```
      `let mut note_changed = false;` goes before the block. After it:
      ```rust
      let emitted = state::status_for(event, payload).is_some_and(|st| self.set_status(id, st));
      if note_changed && !emitted {
          if let Some(v) = self.view(id) {
              self.sink.session_changed(v);
          }
      }
      ```
    - In `claude_gone`, set `s.note = None;` next to `claude_running = false`.
  - `app.rs` `attention`: the title is the session name, and the body is the note or "Needs you" / "Done":
    ```rust
    let body = view.note.clone().unwrap_or_else(|| match kind {
        Attention::NeedsYou => "Needs you".into(),
        Attention::Done => "Done".into(),
    });
    let _ = self.app.notification().builder().title(&view.name).body(body).show();
    ```

- [ ] **Step 4: Run.** `cargo test 2>&1 | tail -15`. Expected: all pass, including the 6-status sequence test.

- [ ] **Step 5: Commit.** `git commit -am "feat: a one-line note per session from prompts, replies, questions and errors"`

### Task 3: Turn snapshots and the diff command

**Files:**
- Create: `src-tauri/src/git.rs`
- Modify:
  - `src-tauri/src/lib.rs` (`pub mod git;`);
  - `src-tauri/src/shellenv.rs` (`run_checked`);
  - `src-tauri/src/cli.rs` (`run_hook` adds `roost_base`);
  - `src-tauri/src/engine.rs` (store `turn_base`, `Core::diff`);
  - `src-tauri/src/app.rs` (`diff` command, registered);
  - `src-tauri/tests/flow.rs`.

**Interfaces:**
- Consumes `Session.turn_base` from Task 2.
- Produces:
  - `git::snapshot(&Path) -> Option<String>`;
  - `git::diff(&Path, base: Option<&str>) -> Result<String, String>`;
  - `Core::diff(&self, id: &str, scope: &str) -> Result<String, String>`;
  - the Tauri command `diff { id, scope }` returning a `String`.

- [ ] **Step 1: Failing tests.** `git.rs` tests:
  ```rust
  #[cfg(test)]
  mod tests {
      use super::*;
      use std::fs;

      fn sh(dir: &Path, args: &[&str]) -> String {
          let out = Command::new(GIT).arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
          String::from_utf8(out.stdout).unwrap()
      }

      fn repo() -> tempfile::TempDir {
          let d = tempfile::tempdir().unwrap();
          sh(d.path(), &["init", "-q"]);
          fs::write(d.path().join("a.txt"), "one\n").unwrap();
          sh(d.path(), &["add", "a.txt"]);
          sh(d.path(), &["commit", "-qm", "x"]);
          d
      }

      #[test]
      fn snapshot_captures_untracked_files_and_leaves_the_index_alone() {
          let d = repo();
          let index = fs::read(d.path().join(".git/index")).unwrap();
          let base = snapshot(d.path()).unwrap();
          fs::write(d.path().join("new file.txt"), "hi\n").unwrap();
          fs::write(d.path().join("a.txt"), "two\n").unwrap();
          let out = diff(d.path(), Some(&base)).unwrap();
          assert!(out.contains("+++ b/new file.txt") && out.contains("+two"), "{out}");
          assert_eq!(fs::read(d.path().join(".git/index")).unwrap(), index);
          assert_eq!(sh(d.path(), &["status", "--porcelain"]), " M a.txt\n?? new file.txt\n");
      }

      #[test]
      fn an_unchanged_folder_has_an_empty_diff() {
          let d = repo();
          let base = snapshot(d.path()).unwrap();
          assert_eq!(diff(d.path(), Some(&base)).unwrap(), "");
      }

      #[test]
      fn all_changes_work_without_commits() {
          let d = tempfile::tempdir().unwrap();
          sh(d.path(), &["init", "-q"]);
          fs::write(d.path().join("b.txt"), "x\n").unwrap();
          assert!(diff(d.path(), None).unwrap().contains("+++ b/b.txt"));
      }

      #[test]
      fn outside_a_repo_there_is_no_snapshot_and_a_plain_error() {
          let d = tempfile::tempdir().unwrap();
          assert_eq!(snapshot(d.path()), None);
          assert_eq!(diff(d.path(), None), Err("Not a git repository.".into()));
          assert!(diff(Path::new("/no/such/dir"), None).unwrap_err().starts_with("Couldn't read the diff"));
      }

      #[test]
      fn big_diffs_are_cut() {
          let cut = truncate("é".repeat(MAX_DIFF));
          assert!(cut.len() < MAX_DIFF + 40 && cut.ends_with("… diff truncated\n"));
      }
  }
  ```
  `flow.rs`:
  ```rust
  #[test]
  fn a_turn_that_edits_a_file_shows_in_the_turn_diff() {
      let tmp = tempfile::tempdir().unwrap();
      let repo = repo(&tmp);
      std::process::Command::new("/usr/bin/git").args(["init", "-q"]).current_dir(&repo).status().unwrap();
      let (core, rec, _) = setup(tmp.path());
      let v = new(&core, &repo, "x");
      wait("idle", || statuses(&rec, &v.id).len() >= 6);
      assert_eq!(core.diff(&v.id, "turn"), Err("No turn yet. Send Claude a prompt first.".into()));
      core.send(&v.id, "/turn", None).unwrap();
      wait("turn done", || note_of(&core, &v.id).as_deref() == Some("Edited edited.txt."));
      let d = core.diff(&v.id, "turn").unwrap();
      assert!(d.contains("+++ b/edited.txt") && d.contains("+changed") && !d.contains(".fake-claude-pid"), "{d}");
      assert!(core.diff(&v.id, "all").unwrap().contains(".fake-claude-pid"));
      core.shutdown();
  }
  ```

- [ ] **Step 2: Run and see it fail.** `cargo test 2>&1 | grep -E "^error|FAILED" | head`. Expected: no module `git`, no `Core::diff`.

- [ ] **Step 3: Implement.**
  - `shellenv.rs`: split `run_with_timeout` into a private `run(cmd, timeout) -> Option<(ExitStatus, Vec<u8>)>` that keeps the exit status from `try_wait`. Then:
    ```rust
    pub fn run_with_timeout(cmd: Command, timeout: Duration) -> Option<Vec<u8>> {
        run(cmd, timeout).map(|(_, out)| out)
    }

    /// Like `run_with_timeout`, but a failed exit is `None` too.
    pub fn run_checked(cmd: Command, timeout: Duration) -> Option<Vec<u8>> {
        run(cmd, timeout).filter(|(status, _)| status.success()).map(|(_, out)| out)
    }
    ```
  - `git.rs`:
    ```rust
    //! Snapshots of a working tree as git tree objects, for "what changed this turn".
    use crate::shellenv::run_checked;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::time::Duration;

    const GIT: &str = "/usr/bin/git";
    const SHORT: Duration = Duration::from_secs(1);
    const LIMIT: Duration = Duration::from_secs(3);
    /// git's empty tree: the base of "all changes" before the first commit.
    const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";
    const MAX_DIFF: usize = 1 << 20;

    fn git(dir: &Path, args: &[&str], index: Option<&Path>, limit: Duration) -> Option<String> {
        let mut cmd = Command::new(GIT);
        cmd.arg("-C").arg(dir).args(args).env("GIT_OPTIONAL_LOCKS", "0");
        if let Some(i) = index {
            cmd.env("GIT_INDEX_FILE", i);
        }
        run_checked(cmd, limit).map(|out| String::from_utf8_lossy(&out).into_owned())
    }

    /// Removes the temporary index (and a lock git may leave) however the snapshot ends.
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(self.0.with_extension("lock"));
        }
    }

    /// The folder's whole repo as a tree id: tracked changes, untracked files and deletions.
    /// Works on a copy of the index, so the user's staging and files are untouched.
    pub fn snapshot(dir: &Path) -> Option<String> {
        let index = git(dir, &["rev-parse", "--path-format=absolute", "--git-path", "index"], None, SHORT)?;
        let tmp = Scratch(std::env::temp_dir().join(format!("roost-index-{}", uuid::Uuid::new_v4())));
        let index = Path::new(index.trim());
        if index.exists() {
            std::fs::copy(index, &tmp.0).ok()?; // keeps git's stat cache: only changed files are hashed
        }
        git(dir, &["add", "-A"], Some(&tmp.0), LIMIT)?;
        let tree = git(dir, &["write-tree"], Some(&tmp.0), SHORT)?;
        Some(tree.trim().to_string()).filter(|t| !t.is_empty())
    }

    /// `git diff` from `base` (a tree id; HEAD's tree when None) to the folder as it is now.
    pub fn diff(dir: &Path, base: Option<&str>) -> Result<String, String> {
        if !dir.is_dir() {
            return Err("Couldn't read the diff: the folder is gone.".into());
        }
        if git(dir, &["rev-parse", "--is-inside-work-tree"], None, SHORT).is_none() {
            return Err("Not a git repository.".into());
        }
        let base = match base {
            Some(b) => b.to_string(),
            None => git(dir, &["rev-parse", "--verify", "-q", "HEAD^{tree}"], None, SHORT)
                .map(|t| t.trim().to_string())
                .unwrap_or_else(|| EMPTY_TREE.into()),
        };
        let now = snapshot(dir).ok_or("Couldn't read the diff: git took too long or failed.")?;
        let out = git(dir, &["diff", "--no-color", "--no-ext-diff", "-M", &base, &now], None, LIMIT)
            .ok_or("Couldn't read the diff: the turn's snapshot is gone.")?;
        Ok(truncate(out))
    }

    fn truncate(mut s: String) -> String {
        if s.len() > MAX_DIFF {
            let mut cut = MAX_DIFF;
            while !s.is_char_boundary(cut) {
                cut -= 1;
            }
            s.truncate(cut);
            s.push_str("\n… diff truncated\n");
        }
        s
    }
    ```
  - `cli.rs` `run_hook`: make the payload `mut`, and before building the request:
    ```rust
    // Claude waits for this hook, so the snapshot is taken before it edits anything.
    if event == "UserPromptSubmit" {
        if let Some(base) = payload["cwd"].as_str().and_then(|d| crate::git::snapshot(Path::new(d))) {
            payload["roost_base"] = base.into();
        }
    }
    ```
  - `engine.rs`:
    - In `hook`'s locked match, add an arm:
      ```rust
      "UserPromptSubmit" => {
          if let Some(s) = inner.state.sessions.iter_mut().find(|s| s.id == id) {
              s.turn_base = payload["roost_base"].as_str().map(String::from);
          }
      }
      ```
      The note save that follows persists it. Make the note block save when `note_changed || event == "UserPromptSubmit"`.
    - Add:
      ```rust
      /// What changed in the session's folder: since the last prompt ("turn"), or since HEAD ("all").
      pub fn diff(&self, id: &str, scope: &str) -> Result<String, String> {
          let (dir, base) = {
              let inner = self.inner.lock().unwrap();
              let s = inner.state.sessions.iter().find(|s| s.id == id).ok_or("no such session")?;
              (s.workdir.clone(), s.turn_base.clone())
          };
          match scope {
              "all" => crate::git::diff(Path::new(&dir), None),
              _ => match base {
                  Some(b) => crate::git::diff(Path::new(&dir), Some(&b)),
                  None => Err("No turn yet. Send Claude a prompt first.".into()),
              },
          }
      }
      ```
  - `app.rs`: add the command and register `diff` in `generate_handler!`:
    ```rust
    // async: git can take a moment on a big repo; never on the main thread.
    #[tauri::command]
    async fn diff(id: String, scope: String, core: CoreState<'_>) -> Result<String, String> {
        core.diff(&id, &scope)
    }
    ```

- [ ] **Step 4: Run.** `cargo test 2>&1 | tail -15`. Expected: all pass.

- [ ] **Step 5: Commit.** `git add -A src-tauri && git commit -m "feat: snapshot each turn and serve the diff since the last prompt"`

### Task 4: Line-editing keys

**Files:**
- Create: `src/keys.ts`, `src/keys.test.ts`
- Modify: `src/terminals.ts`

**Interfaces:** Produces `editKey(e: Pick<KeyboardEvent, "key" | "metaKey" | "altKey" | "ctrlKey" | "shiftKey">): string | undefined`.

- [ ] **Step 1: Failing test** `src/keys.test.ts`:
  ```ts
  import { expect, test } from "bun:test";
  import { editKey } from "./keys";

  const k = (key: string, mods: Partial<Record<"metaKey" | "altKey" | "ctrlKey" | "shiftKey", boolean>> = {}) =>
    editKey({ key, metaKey: false, altKey: false, ctrlKey: false, shiftKey: false, ...mods });

  test("⌘ keys edit and move by line", () => {
    expect(k("Backspace", { metaKey: true })).toBe("\x15");
    expect(k("Delete", { metaKey: true })).toBe("\x0b");
    expect(k("ArrowLeft", { metaKey: true })).toBe("\x01");
    expect(k("ArrowRight", { metaKey: true })).toBe("\x05");
  });

  test("⌥ keys edit and move by word", () => {
    expect(k("Backspace", { altKey: true })).toBe("\x1b\x7f");
    expect(k("Delete", { altKey: true })).toBe("\x1bd");
    expect(k("ArrowLeft", { altKey: true })).toBe("\x1bb");
    expect(k("ArrowRight", { altKey: true })).toBe("\x1bf");
  });

  test("other keys and chords are left alone", () => {
    expect(k("Backspace")).toBeUndefined();
    expect(k("ArrowLeft", { metaKey: true, altKey: true })).toBeUndefined(); // ⌘⌥← moves pane focus
    expect(k("ArrowLeft", { metaKey: true, shiftKey: true })).toBeUndefined();
    expect(k("a", { metaKey: true })).toBeUndefined();
  });
  ```
- [ ] **Step 2: Run.** `bun test src/keys.test.ts`. Expected: FAIL, module not found.
- [ ] **Step 3: Implement** `src/keys.ts`:
  ```ts
  /** macOS line editing in a terminal (Ghostty's defaults), as bytes for the shell or Claude. */
  const META: Record<string, string> = { Backspace: "\x15", Delete: "\x0b", ArrowLeft: "\x01", ArrowRight: "\x05" };
  const ALT: Record<string, string> = { Backspace: "\x1b\x7f", Delete: "\x1bd", ArrowLeft: "\x1bb", ArrowRight: "\x1bf" };

  export function editKey(e: Pick<KeyboardEvent, "key" | "metaKey" | "altKey" | "ctrlKey" | "shiftKey">): string | undefined {
    if (e.ctrlKey || e.shiftKey || e.metaKey === e.altKey) return undefined; // exactly one of ⌘ or ⌥
    return (e.metaKey ? META : ALT)[e.key];
  }
  ```
  In `terminals.ts`, import it and replace the key handler:
  ```ts
  // ⌘/⌥ editing keys become line edits; other ⌘-keys belong to the app (and the Edit menu), never the PTY.
  term.attachCustomKeyEventHandler((ev) => {
    const bytes = ev.type === "keydown" ? editKey(ev) : undefined;
    if (bytes) {
      ev.preventDefault();
      void api.write(id, bytes).catch(() => {});
      return false;
    }
    return !ev.metaKey;
  });
  ```
- [ ] **Step 4: Run.** `bun test && npx tsc --noEmit`. Expected: all pass.
- [ ] **Step 5: Commit.** `git add src && git commit -m "feat: ⌘/⌥ backspace, delete and arrows edit the line like a Mac terminal"`

### Task 5: Note line, unread, ⌘J and the ⌘B rail

**Files:**
- Modify: `src/api.ts` (`View.note`), `src/sidebar.ts`, `src/main.ts`, `src/style.css`, `index.html`, `.superpowers/harness.html` (views get `note`).

**Interfaces:**
- Consumes `View.note` from Task 2.
- Produces `sidebar.render(el, views, focusId, visible, unread: Set<string>, on)`.

- [ ] **Step 1: Failing harness check.** Save as `.superpowers/attention-check.js`, then run it via chrome-devtools `evaluate_script` after reloading `.superpowers/harness.html` (with `bun run dev` on :1420):
  ```js
  async () => {
    const wait = (ms) => new Promise((r) => setTimeout(r, ms));
    const out = {};
    const row = (id) => document.querySelector(`.row[data-id="${id}"]`);
    out.noteShown = row("a")?.querySelector(".note")?.textContent ?? null;
    const a = window.__views.find((v) => v.id === "a");
    window.__emit("session_changed", { ...a, status: "working" });
    await wait(20);
    window.__emit("session_changed", { ...a, status: "idle", note: "All done." });
    await wait(50);
    out.unreadAfterFinish = row("a").classList.contains("unread");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "j", metaKey: true, bubbles: true }));
    await wait(100);
    out.jumpedTo = document.querySelector(".pane.focus")?.dataset.id;
    out.unreadCleared = !row("a").classList.contains("unread");
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "b", metaKey: true, bubbles: true }));
    await wait(250);
    out.railWidth = document.getElementById("sidebar").getBoundingClientRect().width;
    out.namesHidden = getComputedStyle(row("a").querySelector(".name")).display === "none";
    window.dispatchEvent(new KeyboardEvent("keydown", { key: "b", metaKey: true, bubbles: true }));
    await wait(250);
    out.backWidth = document.getElementById("sidebar").getBoundingClientRect().width;
    return out;
  }
  ```
  Before this, give view `a` the note `"fix the login bug"` in the harness and make sure `a` isn't the focused pane. Expected now: `noteShown: null`, `unreadAfterFinish: false`, rail width unchanged.
  The window loses focus whenever DevTools takes focus, so `document.hasFocus()` is false during the run. That's exactly Review Focus 4: a finished session counts as unread while you aren't looking.
- [ ] **Step 2: Implement.**
  - `api.ts`: `note: string | null;` on `View`.
  - `sidebar.ts`:
    - `render` takes `unread: Set<string>` after `visible`.
    - Each row becomes a column holding `.line` (tree mark, dot, name, unread dot `.ud` when unread, meta), then `.note` when `v.note` is set. The note gets `style.paddingLeft = ${15 + (depth ? 14 : 0)}px`.
    - Add the `unread` class, and set `row.title = v.note ? `${v.name}\n${v.note}` : v.name`.
    - `row.ondblclick` does nothing when `document.body.classList.contains("rail")`.
  - `main.ts`:
    - Add `const unread = new Set<string>();`.
    - Add `const looking = (id: string) => document.hasFocus() && owner(focusId) === id;`. Until Task 6 adds `owner`, define a local `const owner = (id: string | null) => id ?? "";`.
    - Add the helpers:
      ```ts
      /** Finished or waiting while you weren't looking: marked until you focus it. */
      function track(prev: View | undefined, v: View) {
        if (!prev || prev.status === v.status) return;
        const finished = v.status === "idle" && prev.status === "working";
        if ((finished || v.status === "needs_you") && !looking(v.id)) unread.add(v.id);
      }
      function markRead(id: string | null) {
        if (id && document.hasFocus() && unread.delete(owner(id))) drawSidebar();
      }
      ```
    - Call `track` in `onSession` before replacing the view, and for each view in `onSessions`, where ids that are gone are also dropped from `unread`.
    - Call `markRead(focusId)` at the end of `setTree` and `focusPane`, and on `window.addEventListener("focus", () => markRead(focusId))`.
    - Pass `unread` to `sidebar.render`.
    - ⌘J:
      ```ts
      function jump(order: string[]) {
        const start = focusId ? order.indexOf(owner(focusId)) : -1;
        const next = (want: (id: string) => boolean) => {
          for (let i = 1; i <= order.length; i++) {
            const id = order[(start + i + order.length) % order.length];
            if (want(id)) return id;
          }
        };
        const needs = new Set(views.filter((v) => v.status === "needs_you").map((v) => v.id));
        const id = next((id) => needs.has(id)) ?? next((id) => unread.has(id));
        if (id) void show(id);
      }
      ```
    - Rail:
      ```ts
      function setRail(on: boolean) {
        document.body.classList.toggle("rail", on);
        try {
          localStorage.setItem("roost.rail", on ? "1" : "0");
        } catch {}
      }
      ```
      At boot: `try { setRail(localStorage.getItem("roost.rail") === "1"); } catch {}`. Add the key `else if (key === "b") setRail(!document.body.classList.contains("rail"));`.
  - `index.html`: `<button id="new"><span>+</span><span class="label">New session</span><kbd>⌘N</kbd></button>`.
  - `style.css`:
    ```css
    #sidebar { transition: width .15s ease; }
    .row { flex-direction: column; align-items: stretch; gap: 1px; }
    .row .line { display: flex; align-items: center; gap: 8px; min-width: 0; }
    .row .note { color: var(--mute); font-size: 11px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
    .row .ud { width: 6px; height: 6px; border-radius: 50%; background: var(--blue); flex: none; }
    .row.unread .name { color: var(--text); font-weight: 600; }
    #new { justify-content: flex-start; gap: 6px; }
    #new kbd { margin-left: auto; }
    body.rail #sidebar { width: 44px; padding: 0 4px 8px; }
    body.rail .grp, body.rail .row .name, body.rail .row .meta, body.rail .row .note, body.rail .row .tree,
    body.rail .row .ud, body.rail #new .label, body.rail #new kbd { display: none; }
    body.rail .row { padding: 8px 0; }
    body.rail .row .line { justify-content: center; }
    body.rail .row.unread .dot { box-shadow: 0 0 0 2px var(--bg), 0 0 0 3.5px var(--blue); }
    body.rail #new { justify-content: center; }
    ```
    Change `.row`'s existing `display: flex; align-items: center` to work with the new column layout: keep `display: flex` and let the rules above override the direction.
  - `harness.html`: the `v()` helper sets `note: extra.note ?? null`, and view `a` gets `{ note: "fix the login bug" }`.
- [ ] **Step 3: Check.** Run `npx tsc --noEmit && bun test && bun run build`, then rerun the check. Expected:
  - `noteShown: "fix the login bug"`;
  - `unreadAfterFinish: true`;
  - `jumpedTo: "a"`;
  - `unreadCleared` stays `false` while DevTools holds focus. Then call `window.focus()` in the page and `markRead` clears it; `window.dispatchEvent(new Event("focus"))` simulates this.
  - `railWidth: 44`, `namesHidden: true`, `backWidth: 210`.

  Take a screenshot of both sidebar states.
- [ ] **Step 4: Commit.** `git add src index.html && git commit -m "feat: note line, unread marker, ⌘J to what needs you, ⌘B dot rail"`

### Task 6: The diff pane

**Files:**
- Create: `src/diff.ts` (pure), `src/diff.test.ts`, `src/diffpane.ts` (DOM and api)
- Modify: `src/api.ts` (`api.diff`), `src/panes.ts`, `src/main.ts`, `src/style.css`, `.superpowers/harness.html` (a mock `diff` command)

**Interfaces:**
- Consumes the `diff` command from Task 3.
- Produces:
  - `diffId(id)`, `isDiff(id)`, `owner(id)`, `keep(ids): Set<string>`, `parse(text): DiffFile[]`;
  - `Hooks.diff(id)`;
  - ⌘G.

- [ ] **Step 1: Failing test** `src/diff.test.ts`:
  ```ts
  import { expect, test } from "bun:test";
  import { diffId, isDiff, keep, owner, parse } from "./diff";

  const sample = [
    "diff --git a/src/a b.ts b/src/a b.ts", "index 1..2 100644", "--- a/src/a b.ts", "+++ b/src/a b.ts",
    "@@ -1,2 +1,3 @@", " keep", "-old", "+new", "+++ looks like a header", "\\ No newline at end of file",
    "diff --git a/img.png b/img.png", "new file mode 100644", "index 0000000..1", "Binary files /dev/null and b/img.png differ",
    "diff --git a/gone.txt b/gone.txt", "deleted file mode 100644", "--- a/gone.txt", "+++ /dev/null", "@@ -1 +0,0 @@", "-bye",
    "diff --git a/old.ts b/new.ts", "similarity index 100%", "rename from old.ts", "rename to new.ts", "",
  ].join("\n");

  test("files get their paths and counts", () => {
    const files = parse(sample);
    expect(files.map((f) => f.path)).toEqual(["src/a b.ts", "img.png", "gone.txt", "new.ts"]);
    expect([files[0].add, files[0].del]).toEqual([2, 1]);
    expect(files[1].binary).toBe(true);
    expect([files[2].add, files[2].del]).toEqual([0, 1]);
  });

  test("hunk lines keep their kind and text, and +++ inside a hunk is an added line", () => {
    expect(parse(sample)[0].lines.map((l) => l.kind)).toEqual(["hunk", "ctx", "del", "add", "add"]);
    expect(parse(sample)[0].lines[4].text).toBe("+++ looks like a header");
  });

  test("an empty diff has no files", () => expect(parse("")).toEqual([]));

  test("diff panes are tied to their session", () => {
    expect(diffId("s1")).toBe("diff:s1");
    expect(isDiff("diff:s1") && !isDiff("s1")).toBe(true);
    expect([owner("diff:s1"), owner("s1")]).toEqual(["s1", "s1"]);
    expect([...keep(["s1"])]).toEqual(["s1", "diff:s1"]);
  });
  ```
- [ ] **Step 2: Run.** `bun test src/diff.test.ts`. Expected: FAIL, no module.
- [ ] **Step 3: Implement.**
  - `src/diff.ts`:
    ```ts
    /** Pure helpers for the diff pane: layout ids and `git diff` parsing. */
    export const diffId = (id: string) => `diff:${id}`;
    export const isDiff = (id: string) => id.startsWith("diff:");
    export const owner = (id: string) => (isDiff(id) ? id.slice(5) : id);
    /** Pane ids that may stay in the layout: each live session and its diff pane. */
    export const keep = (ids: string[]) => new Set(ids.flatMap((id) => [id, diffId(id)]));

    export interface DiffLine { kind: "hunk" | "ctx" | "add" | "del"; text: string }
    export interface DiffFile { path: string; add: number; del: number; binary: boolean; lines: DiffLine[] }

    /** `a/P b/P` names one unchanged path, even with spaces in it. */
    function gitPath(header: string): string {
      const s = header.slice("diff --git ".length);
      const len = (s.length - 5) / 2;
      return Number.isInteger(len) && s.startsWith("a/") ? s.slice(2, 2 + len) : s;
    }

    export function parse(text: string): DiffFile[] {
      const files: DiffFile[] = [];
      let f: DiffFile | null = null;
      let inHunks = false;
      for (const raw of text.split("\n")) {
        if (raw.startsWith("diff --git ")) {
          f = { path: gitPath(raw), add: 0, del: 0, binary: false, lines: [] };
          files.push(f);
          inHunks = false;
        } else if (!f) continue;
        else if (raw.startsWith("@@")) {
          inHunks = true;
          f.lines.push({ kind: "hunk", text: raw });
        } else if (!inHunks) {
          if (raw.startsWith("--- a/")) f.path = raw.slice(6);
          else if (raw.startsWith("+++ b/")) f.path = raw.slice(6);
          else if (raw.startsWith("rename to ")) f.path = raw.slice(10);
          else if (raw.startsWith("Binary files ")) f.binary = true;
        } else if (raw.startsWith("+")) {
          f.add++;
          f.lines.push({ kind: "add", text: raw });
        } else if (raw.startsWith("-")) {
          f.del++;
          f.lines.push({ kind: "del", text: raw });
        } else if (raw.startsWith(" ")) f.lines.push({ kind: "ctx", text: raw });
      }
      return files;
    }
    ```
  - `src/api.ts`: `diff: (id: string, scope: "turn" | "all") => invoke<string>("diff", { id, scope }),`.
  - `src/diffpane.ts`:
    ```ts
    import { api, type View } from "./api";
    import { owner, parse } from "./diff";

    type Scope = "turn" | "all";
    const scopes = new Map<string, Scope>(); // per diff pane, for this run of the app
    const seen = new Map<string, string>(); // last status seen per diff pane: a change re-reads the diff
    let req = 0;

    const el = (tag: string, cls: string, text = "") => {
      const e = document.createElement(tag);
      e.className = cls;
      e.textContent = text;
      return e;
    };

    /** A rebuilt pane has an empty body: make its next `update` load. */
    export const forget = (paneId: string) => seen.delete(paneId);

    function message(body: HTMLElement, text: string) {
      body.replaceChildren(el("div", "diff-msg", text));
    }

    export async function load(body: HTMLElement, paneId: string) {
      const mine = String(++req);
      body.dataset.req = mine;
      let text: string;
      try {
        text = await api.diff(owner(paneId), scopes.get(paneId) ?? "turn");
      } catch (e) {
        if (body.dataset.req === mine) message(body, String(e));
        return;
      }
      if (body.dataset.req !== mine) return; // a newer load is on its way
      const files = parse(text);
      if (!files.length) return message(body, "No changes.");
      const top = body.querySelector(".diff")?.scrollTop ?? 0;
      const root = el("div", "diff");
      for (const f of files) {
        const head = el("div", "df");
        head.append(el("span", "p", f.path), el("span", "n", f.binary ? "binary" : `+${f.add} −${f.del}`));
        root.append(head);
        for (const l of f.lines) root.append(el("div", `dl ${l.kind}`, l.text));
      }
      body.replaceChildren(root);
      root.scrollTop = top;
    }

    /** Header: which session, and the last-turn / all toggle. Re-reads the diff when the session's status moves. */
    export function update(pane: HTMLElement, paneId: string, v: View | undefined) {
      const head = pane.querySelector<HTMLElement>(".ph")!;
      const body = pane.querySelector<HTMLElement>(".pb")!;
      head.className = "ph diffh";
      const tabs = el("span", "scope");
      for (const [s, label] of [["turn", "last turn"], ["all", "all"]] as [Scope, string][]) {
        const b = el("button", (scopes.get(paneId) ?? "turn") === s ? "on" : "", label);
        b.onmousedown = (e) => e.stopPropagation();
        b.onclick = () => {
          scopes.set(paneId, s);
          update(pane, paneId, v);
          void load(body, paneId);
        };
        tabs.append(b);
      }
      head.replaceChildren(el("span", "name", `diff · ${v?.name ?? ""}`), tabs);
      const status = v?.status ?? "";
      if (seen.get(paneId) !== status) {
        seen.set(paneId, status);
        void load(body, paneId);
      }
    }
    ```
  - `panes.ts`:
    - Import `isDiff` and `* as diffpane`.
    - Add `diff(id: string): void` to `Hooks`.
    - `header(el, v, shell)` gains a `diff` button after `what` for session panes. Its `onmousedown` stops propagation and its `onclick` calls `hooks.diff(v.id)`, so `header` takes `hooks`.
    - In `build`, for `isDiff(n.pane)`: call `diffpane.update(pane, n.pane, views.find((v) => v.id === owner(n.pane)))` after appending head and body, `seen` is cleared so it loads, and nothing goes into `bodies`.
    - `updateHeaders` calls `diffpane.update` for diff panes, otherwise `header`.
    - Since `render` rebuilds fresh DOM, call `diffpane.forget(paneId)` (`seen.delete`) before `update` in `build`, so a rebuilt pane always loads.
  - `main.ts`:
    - Import `diffId, isDiff, keep, owner` from `./diff`, and remove the Task 5 local `owner` stub. Calls that pass `focusId` use `owner(focusId ?? "")`.
    - Add:
      ```ts
      function toggleDiff(id = focusId) {
        if (!id) return;
        const d = diffId(owner(id));
        if (paneIds(tree).includes(d)) setTree(remove(tree, d), owner(id));
        else if (paneIds(tree).includes(owner(id))) setTree(split(tree, owner(id), d, "right"), owner(id));
      }
      ```
    - Add `hooks.diff = (id) => toggleDiff(id)` and the key `else if (key === "g") toggleDiff();`.
    - `closeFocused` starts with `if (focusId && isDiff(focusId)) return removePane();`.
    - `defaultRepo` and `splitFocused` use `byId(owner(focusId ?? ""))`.
    - `drawSidebar` passes `owner(focusId ?? "")` as the focus.
    - `hooks.drop` calls `api.select` only when `!isDiff(id)`.
    - Both `prune` calls use `keep(...)`, and boot `select` skips diff ids.
  - `style.css`:
    ```css
    .hbtn { all: unset; font-size: 10px; color: var(--mute); padding: 0 6px; border: 1px solid var(--edge); border-radius: 4px; cursor: default; }
    .hbtn:hover { color: var(--text); }
    .ph .scope { margin-left: auto; display: flex; gap: 2px; }
    .ph .scope button { all: unset; padding: 0 6px; border-radius: 4px; cursor: default; }
    .ph .scope button.on { background: var(--sel); color: var(--text); }
    .diff { position: absolute; inset: 0; overflow: auto; font: 12px/1.5 var(--mono); padding-bottom: 24px; user-select: text; -webkit-user-select: text; }
    .diff .df { position: sticky; top: 0; display: flex; justify-content: space-between; gap: 12px; padding: 6px 12px; color: var(--text); background: var(--bg); border-bottom: 1px solid var(--line); }
    .diff .df .n { color: var(--mute); flex: none; }
    .diff .dl { padding: 0 12px; white-space: pre; min-height: 18px; }
    .diff .add { background: rgba(74, 222, 128, .10); color: #bff5d0; }
    .diff .del { background: rgba(248, 113, 113, .10); color: #f8c4c4; }
    .diff .hunk { color: var(--mute); padding-top: 6px; }
    .diff-msg { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; color: var(--mute); }
    ```
  - `harness.html`: `case "diff": return args.scope === "all" ? window.__diffAll ?? "" : window.__diffTurn ?? "";`. Seed `__diffTurn` with a two-file sample.
- [ ] **Step 4: Check.**
  - Run `bun test && npx tsc --noEmit && bun run build`. Expected: all pass.
  - In the harness, check:
    - ⌘G on the focused pane adds a `.pane[data-id="diff:<id>"]` with `.df` rows, and `__saved.layout` contains `diff:`;
    - the `all` tab shows "No changes.";
    - ⌘G again removes the pane;
    - ⌘W on a focused diff pane keeps the session row;
    - a layout with a `diff:x` leaf for a missing session boots without it.

    Take a screenshot.
- [ ] **Step 5: Commit.** `git add src && git commit -m "feat: diff pane with what Claude changed since your last prompt (⌘G)"`

### Task 7: Release check

**Files:** Modify `docs/specs/2026-10-12-roost-attention-design.md` (record deviations).

- [ ] **Step 1: Everything green.** Run `cd src-tauri && cargo build && cargo test && cd .. && bun test && npx tsc --noEmit && bun run build`. Expected: no warnings, everything passes. Run the full cargo suite 3 times to catch flakes.
- [ ] **Step 2: Real-Claude smoke test.**
  - Add an `#[ignore]`d flow test, `real_claude_reports_notes_questions_and_turn_snapshots`, that is built like `setup` but differs in three ways:
    - `claude: Some(<real claude path from ROOST_TEST_CLAUDE>)`;
    - a real `HOME` (for auth);
    - a git-initialised repo inside `src-tauri/target/smoke-repo`, which is under the trusted `~/Dev`.
  - Steps:
    1. Prompt "reply with exactly: hi". Wait for `idle` with note `hi`. Check that `diff(turn)` is `Ok("")`, then write a file and check that `diff(turn)` shows it.
    2. Send "Use the AskUserQuestion tool to ask me whether I prefer red or blue." Wait for `needs_you` with a note that mentions red. Send Escape (`\x1b`), then `/exit`.
  - Run it: `ROOST_TEST_CLAUDE=$HOME/.local/share/claude/versions/<v> cargo test --test flow real_claude -- --ignored --nocapture`.
- [ ] **Step 3: Record deviations** in the spec under a closing "Deviations" heading (only if any).
- [ ] **Step 4: Release build without touching the running app.** Run `bun run tauri build`. Do NOT install it or kill `/Applications/Roost.app`; the user installs with ⌘Q, then `ditto`.
- [ ] **Step 5: Commit.** `git commit -am "docs: record attention-batch deviations"`
