use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Starting,
    Shell,
    Asleep,
    Working,
    NeedsYou,
    Idle,
    Exited,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    pub id: String,
    pub claude_id: String,
    pub name: String,
    pub repo: String,
    pub workdir: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub created_at: u64,
    #[serde(default)]
    pub exited: bool,
    #[serde(default)]
    pub claude_running: bool,
    /// One line about where the conversation is: the prompt, the reply, a question or an error.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The folder as a git tree when the last prompt was sent: the base of the turn's diff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_base: Option<String>,
    /// What Claude was last doing (working, needs you, idle): a row the helper kept running
    /// comes back showing it, since hooks that fire while the app is closed are lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_status: Option<Status>,
}

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

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Reads state; a missing file is empty, a corrupt one is moved to `state.json.bak`.
pub fn load(path: &Path) -> State {
    let Ok(text) = fs::read_to_string(path) else {
        return State::default();
    };
    match serde_json::from_str(&text) {
        Ok(state) => migrate(state),
        Err(_) => {
            let _ = fs::rename(path, path.with_extension("json.bak"));
            State::default()
        }
    }
}

/// Atomic write: temp file, then rename.
pub fn save(path: &Path, state: &State) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(state)?)?;
    fs::rename(tmp, path)
}

/// First four words, lowercase `a-z0-9-`, at most 24 chars.
pub fn slug(prompt: &str) -> String {
    let joined = prompt
        .split_whitespace()
        .take(4)
        .map(|w| w.to_lowercase())
        .collect::<Vec<_>>()
        .join("-");
    let mut out = String::new();
    for c in joined.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.chars().take(24).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "session".into()
    } else {
        out
    }
}

/// A name as the user typed it: trimmed, at most 60 characters; None if blank.
pub fn display_name(input: &str) -> Option<String> {
    let name: String = input.trim().chars().take(60).collect();
    let name = name.trim_end().to_string();
    (!name.is_empty()).then_some(name)
}

/// How names are compared and looked up: case, spaces, dashes and symbols ignored,
/// so `roost send my-dev-server` finds "My Dev Server".
pub fn name_key(name: &str) -> String {
    name.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// `base`, or `base 2`, `base 3`… if another row's name already has the same key.
pub fn unique_display(base: &str, taken: &[&str]) -> String {
    let keys: Vec<String> = taken.iter().map(|t| name_key(t)).collect();
    let free = |c: &str| !keys.contains(&name_key(c));
    if free(base) {
        return base.into();
    }
    (2..).map(|n| format!("{base} {n}")).find(|c| free(c)).unwrap()
}

/// Slug-style unique name (`fix-it`, `fix-it-2`), compared loosely like `unique_display`.
pub fn unique_name(base: &str, taken: &[&str]) -> String {
    let keys: Vec<String> = taken.iter().map(|t| name_key(t)).collect();
    let free = |c: &str| !keys.contains(&name_key(c));
    if free(base) {
        return base.into();
    }
    (2..).map(|n| format!("{base}-{n}")).find(|c| free(c)).unwrap()
}

/// Status implied by a Claude Code hook event. `None` = leave status alone.
pub fn status_for(event: &str, payload: &serde_json::Value) -> Option<Status> {
    match event {
        // Claude re-fires SessionStart after auto-compaction, mid-turn.
        "SessionStart" if payload["source"] == "compact" => None,
        "SessionStart" | "Stop" | "StopFailure" => Some(Status::Idle),
        "PreToolUse" => match payload["tool_name"].as_str() {
            Some("AskUserQuestion" | "ExitPlanMode") => Some(Status::NeedsYou),
            _ => None,
        },
        "UserPromptSubmit" | "PostToolUse" => Some(Status::Working),
        // Esc on a running tool or an open question ends the turn; no Stop follows.
        "PostToolUseFailure" if payload["is_interrupt"] == true => Some(Status::Idle),
        "PostToolUseFailure" => Some(Status::Working),
        "Notification" => match payload.get("notification_type").and_then(|v| v.as_str()) {
            Some("idle_prompt") => None,
            _ => Some(Status::NeedsYou),
        },
        "ClaudeStart" => Some(Status::Starting),
        "SessionEnd" => Some(Status::Shell),
        _ => None,
    }
}

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
/// Claude animates the terminal title while it works (◐ ◑, about once a second) and shows ✳
/// when it's done. Some(true): working, Some(false): done; from the last title set in `text`.
pub fn title_working(text: &str) -> Option<bool> {
    let title = text.rsplit("\x1b]").find_map(|t| t.strip_prefix("0;").or_else(|| t.strip_prefix("2;")))?;
    match title.chars().next()? {
        '◐' | '◑' => Some(true),
        '✳' => Some(false),
        _ => None,
    }
}

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

#[cfg(test)]
mod title_tests {
    use super::*;

    #[test]
    fn claude_s_terminal_title_says_whether_it_is_working() {
        assert_eq!(title_working("\x1b]0;◐ fix the bug\x07"), Some(true));
        assert_eq!(title_working("out\x1b]2;◑ fix the bug\x1b\\more"), Some(true));
        assert_eq!(title_working("\x1b]2;✳ fix the bug\x07"), Some(false));
        // the last title in a chunk wins
        assert_eq!(title_working("\x1b]0;◐ a\x07 text \x1b]0;✳ a\x07"), Some(false));
        assert_eq!(title_working("\x1b]0;zsh\x07"), None);
        assert_eq!(title_working("plain output ◐ not a title"), None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_end_the_turn_and_questions_need_you() {
        let j = |v: &str| serde_json::from_str::<serde_json::Value>(v).unwrap();
        assert_eq!(status_for("StopFailure", &j(r#"{"error":"rate_limit"}"#)), Some(Status::Idle));
        assert_eq!(status_for("PreToolUse", &j(r#"{"tool_name":"AskUserQuestion"}"#)), Some(Status::NeedsYou));
        assert_eq!(status_for("PreToolUse", &j(r#"{"tool_name":"ExitPlanMode"}"#)), Some(Status::NeedsYou));
        assert_eq!(status_for("PreToolUse", &j(r#"{"tool_name":"Bash"}"#)), None);
    }

    #[test]
    fn esc_on_a_tool_or_question_ends_the_turn() {
        let j = |v: &str| serde_json::from_str::<serde_json::Value>(v).unwrap();
        assert_eq!(status_for("PostToolUseFailure", &j(r#"{"tool_name":"AskUserQuestion","is_interrupt":true}"#)), Some(Status::Idle));
        assert_eq!(status_for("PostToolUseFailure", &j(r#"{"tool_name":"Bash","is_interrupt":false}"#)), Some(Status::Working));
    }

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
        assert_eq!(
            note_for("Notification", &j(r#"{"message":"Claude needs your permission to use Bash"}"#)),
            set("Claude needs your permission to use Bash")
        );
        assert_eq!(note_for("Notification", &j(r#"{"notification_type":"idle_prompt","message":"waiting"}"#)), None);
        assert_eq!(
            note_for("PreToolUse", &j(r#"{"tool_name":"AskUserQuestion","tool_input":{"questions":[{"question":"Which DB?"}]}}"#)),
            set("Which DB?")
        );
        assert_eq!(note_for("PreToolUse", &j(r#"{"tool_name":"ExitPlanMode"}"#)), set("Plan ready for review"));
        assert_eq!(note_for("PreToolUse", &j(r#"{"tool_name":"Bash"}"#)), None);
        assert_eq!(note_for("SessionStart", &j(r#"{"source":"clear"}"#)), Some(None));
        assert_eq!(note_for("SessionStart", &j(r#"{"source":"resume"}"#)), None);
        assert_eq!(note_for("PostToolUse", &j("{}")), None);
    }

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
            note: None,
            turn_base: None,
            claude_status: None,
        }
    }

    #[test]
    fn slug_takes_first_four_words() {
        assert_eq!(slug("Fix the flaky auth test now"), "fix-the-flaky-auth");
    }

    #[test]
    fn slug_strips_punctuation_and_caps_at_24() {
        assert_eq!(slug("Render LaTeX: fractions!!! everywhere"), "render-latex-fractions-e");
    }

    #[test]
    fn slug_falls_back_to_session() {
        assert_eq!(slug(""), "session");
        assert_eq!(slug("   "), "session");
        assert_eq!(slug("🚀 🚀"), "session");
    }

    #[test]
    fn unique_name_suffixes() {
        assert_eq!(unique_name("b", &["a"]), "b");
        assert_eq!(unique_name("a", &["a", "a-2"]), "a-3");
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nested/state.json");
        let st = State { recent_repos: vec!["/r".into()], sessions: vec![sess("a")], ..State::default() };
        save(&p, &st).unwrap();
        assert_eq!(load(&p), st);
        assert!(!dir.path().join("nested/state.json.tmp").exists());
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(&dir.path().join("state.json")), State::default());
    }

    #[test]
    fn corrupt_file_moves_to_bak_and_loads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, "{nope").unwrap();
        assert_eq!(load(&p), State::default());
        assert_eq!(fs::read_to_string(dir.path().join("state.json.bak")).unwrap(), "{nope");
    }

    #[test]
    fn missing_fields_get_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("state.json");
        fs::write(&p, r#"{"sessions":[]}"#).unwrap();
        let st = load(&p);
        assert_eq!(st.version, 2);
        assert_eq!(st.layout, None);
    }

    #[test]
    fn hook_events_map_to_status() {
        let none = serde_json::json!({});
        assert_eq!(status_for("SessionStart", &none), Some(Status::Idle));
        assert_eq!(status_for("UserPromptSubmit", &none), Some(Status::Working));
        assert_eq!(status_for("PostToolUse", &none), Some(Status::Working));
        assert_eq!(status_for("Stop", &none), Some(Status::Idle));
        assert_eq!(status_for("PreCompact", &none), None);
    }

    #[test]
    fn compaction_mid_turn_is_not_a_finished_turn() {
        let compact = serde_json::json!({"source": "compact"});
        let resume = serde_json::json!({"source": "resume"});
        assert_eq!(status_for("SessionStart", &compact), None);
        assert_eq!(status_for("SessionStart", &resume), Some(Status::Idle));
    }

    #[test]
    fn notifications_need_you_except_idle_reminders() {
        let perm = serde_json::json!({"notification_type": "permission_prompt"});
        let idle = serde_json::json!({"notification_type": "idle_prompt"});
        let bare = serde_json::json!({"message": "Claude needs your permission"});
        assert_eq!(status_for("Notification", &perm), Some(Status::NeedsYou));
        assert_eq!(status_for("Notification", &idle), None);
        assert_eq!(status_for("Notification", &bare), Some(Status::NeedsYou));
    }

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
}
