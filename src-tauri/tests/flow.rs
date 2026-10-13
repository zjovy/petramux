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

fn note_of(core: &Core, id: &str) -> Option<String> {
    core.views().into_iter().find(|v| v.id == id).unwrap().note
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
    assert!(out.contains("ARGS: --plugin-dir ") && out.contains("--append-system-prompt") && out.contains("-- fix it"), "{out}");

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
fn a_session_without_a_folder_opens_in_home() {
    let tmp = tempfile::tempdir().unwrap();
    let (core, _, _) = setup(tmp.path());
    let v = new(&core, "", "");
    assert_eq!(v.repo, tmp.path().join("home").display().to_string());
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
fn relaunch_starts_claude_fresh_when_its_conversation_was_never_saved() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "a");
    wait("SessionStart", || statuses(&rec, &v.id).contains(&Status::Idle));
    core.shutdown();
    drop(core);
    std::fs::remove_dir_all(tmp.path().join("home/.claude/projects")).unwrap(); // as if never prompted

    let (core2, rec2, _) = setup(tmp.path());
    core2.select(&v.id).unwrap();
    wait("claude", || rec2.output.lock().unwrap().contains("ARGS: "));
    assert!(!rec2.output.lock().unwrap().contains("--resume"));
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
    assert_eq!(core.rename(&b.id, "Same").unwrap().name, "Same 2", "clashes ignore case, spaces and dashes");
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

#[test]
fn quitting_keeps_claude_marked_running_for_the_relaunch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, paths) = setup(tmp.path());
    let v = new(&core, &repo, "a");
    wait("idle", || statuses(&rec, &v.id).contains(&Status::Idle));
    core.shutdown();
    std::thread::sleep(Duration::from_millis(2600));
    assert!(state_json(&paths).contains("\"claudeRunning\": true"), "{}", state_json(&paths));
}

#[test]
fn clear_keeps_watching_claude() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, paths) = setup(tmp.path());
    let v = new(&core, &repo, "x");
    wait("idle", || statuses(&rec, &v.id).len() >= 6);
    core.send(&v.id, "/clear", None).unwrap();
    wait("new claude id after /clear", || state_json(&paths).contains("fake-clear-id"));
    assert_ne!(status_of(&core, &v.id), Status::Shell);
    let pid = std::fs::read_to_string(Path::new(&repo).join(".fake-claude-pid")).unwrap();
    unsafe { libc::kill(pid.trim().parse().unwrap(), libc::SIGKILL) };
    let start = Instant::now();
    wait("shell after kill", || status_of(&core, &v.id) == Status::Shell);
    assert!(start.elapsed() < Duration::from_millis(3500));
    core.shutdown();
}

#[test]
fn long_prompts_with_control_characters_reach_claude_intact() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let prompt = format!("{}\u{15}\u{3}echo INJECTED-$((6*7))\r END-MARKER", "x".repeat(4000));
    let v = new(&core, &repo, &prompt);
    wait("claude started with the whole prompt", || rec.output.lock().unwrap().contains("END-MARKER"));
    wait("idle", || statuses(&rec, &v.id).contains(&Status::Idle));
    assert!(!rec.output.lock().unwrap().contains("INJECTED-42"));
    core.shutdown();
}

#[test]
fn send_refuses_when_claude_is_suspended_and_waits_for_a_starting_one() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    // Right after `new`, the claude command is still queued: send waits for it.
    let v = new(&core, &repo, "x");
    core.send(&v.id, "early-hello", None).unwrap();
    wait("early message", || rec.output.lock().unwrap().contains("GOT: ") && rec.output.lock().unwrap().contains("early-hello"));
    wait("idle", || statuses(&rec, &v.id).len() >= 6);
    // Ctrl-Z: Claude's pid is alive and the row still says idle, but the shell has the terminal.
    let pid: i32 = std::fs::read_to_string(Path::new(&repo).join(".fake-claude-pid")).unwrap().trim().parse().unwrap();
    unsafe { libc::kill(pid, libc::SIGTSTP) };
    std::thread::sleep(Duration::from_millis(500));
    let err = core.send(&v.id, "echo SENT-$((6*7))", None).unwrap_err();
    assert!(err.contains("isn't running Claude"), "{err}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!rec.output.lock().unwrap().contains("SENT-42"));
    unsafe { libc::kill(pid, libc::SIGKILL) };
    core.shutdown();
}

#[test]
fn renamed_sessions_keep_their_text_and_the_cli_finds_them_loosely() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, _rec, paths) = setup(tmp.path());
    let v = new(&core, &repo, "");
    assert_eq!(core.rename(&v.id, "  My Dev Server 🚀  ").unwrap().name, "My Dev Server 🚀");
    assert!(core.rename(&v.id, "   ").is_err());
    let long = "x".repeat(80);
    assert_eq!(core.rename(&v.id, &long).unwrap().name.chars().count(), 60);
    core.rename(&v.id, "My Dev Server").unwrap();
    let t = Duration::from_secs(5);
    assert!(!sock::call(&paths.sock(), &Request::Stop { target: "nope".into() }, t).unwrap().ok);
    assert!(sock::call(&paths.sock(), &Request::Stop { target: "my-dev-server".into() }, t).unwrap().ok);
    wait("exited", || status_of(&core, &v.id) == Status::Exited);
}


#[test]
fn new_names_never_clash_loosely_with_existing_ones() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, _rec, _) = setup(tmp.path());
    let a = new(&core, &repo, "");
    core.rename(&a.id, "Fix It").unwrap();
    let b = core
        .new_session(NewSession { prompt: String::new(), repo: repo.clone(), worktree: false, name: Some("fix it".into()), parent: None })
        .unwrap();
    assert_eq!(b.name, "fix-it-2");
    core.shutdown();
}

#[test]
fn send_after_claude_exits_is_refused_at_once() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "x");
    wait("idle", || statuses(&rec, &v.id).len() >= 6);
    core.send(&v.id, "/exit", None).unwrap();
    wait("shell", || status_of(&core, &v.id) == Status::Shell);
    let start = Instant::now();
    assert!(core.send(&v.id, "hi", None).is_err());
    assert!(start.elapsed() < Duration::from_secs(1), "waited {:?}", start.elapsed());
    core.shutdown();
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
    assert_eq!(core.diff(&v.id, "turn"), Err("Not a git repository.".into())); // even after a turn
    core.send(&v.id, "/fail", None).unwrap();
    wait("failure note", || note_of(&core, &v.id).as_deref() == Some("⚠ rate limit"));
    assert_eq!(status_of(&core, &v.id), Status::Idle);
    core.send(&v.id, "/exit", None).unwrap();
    wait("note cleared with claude gone", || status_of(&core, &v.id) == Status::Shell && note_of(&core, &v.id).is_none());
    core.shutdown();
}

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

#[test]
fn esc_ends_a_turn_that_hooks_never_close() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "x");
    wait("idle", || statuses(&rec, &v.id).len() >= 6);
    core.send(&v.id, "/ask", None).unwrap();
    wait("question", || status_of(&core, &v.id) == Status::NeedsYou);
    core.write(&v.id, b"\x1b[A").unwrap(); // an arrow key is not Esc
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(status_of(&core, &v.id), Status::NeedsYou);
    core.write(&v.id, b"\x1b").unwrap();
    wait("idle after esc", || status_of(&core, &v.id) == Status::Idle);
    core.shutdown();
}

fn wait_long(what: &str, secs: u64, mut ok: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !ok() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(200));
    }
}

/// Real Claude with the real login (two short turns). Trust `src-tauri/target/smoke-repo` in Claude once first.
/// `ROOST_TEST_CLAUDE=<path to claude> cargo test --test flow real_claude -- --ignored --nocapture`
#[test]
#[ignore]
fn real_claude_reports_notes_questions_and_turn_snapshots() {
    let claude = PathBuf::from(std::env::var("ROOST_TEST_CLAUDE").expect("set ROOST_TEST_CLAUDE"));
    let tmp = tempfile::tempdir().unwrap();
    // The folder must already be trusted by Claude: a headless PTY can't answer its trust dialog.
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target").join("smoke-repo");
    let _ = std::fs::remove_dir_all(&repo);
    std::fs::create_dir_all(&repo).unwrap();
    std::process::Command::new("/usr/bin/git").args(["init", "-q"]).current_dir(&repo).status().unwrap();
    let paths = Paths::new(tmp.path().join("support"));
    let mut env = Env::new();
    for k in ["PATH", "HOME", "USER", "LANG"] {
        if let Ok(v) = std::env::var(k) {
            env.insert(k.into(), v);
        }
    }
    let rec = Arc::new(Rec::default());
    let core = Core::new(
        Setup {
            paths: paths.clone(),
            env,
            claude: Some(claude),
            roost_bin: PathBuf::from(env!("CARGO_BIN_EXE_roost")),
            shell: PathBuf::from("/bin/zsh"),
        },
        rec.clone(),
    );
    sock::serve(&paths.sock(), core.handler()).unwrap();
    let repo_s = repo.display().to_string();
    let v = new(&core, &repo_s, "reply with exactly: hi");
    wait_long("the reply as the note", 120, || {
        note_of(&core, &v.id).is_some_and(|n| n.to_lowercase().starts_with("hi")) && status_of(&core, &v.id) == Status::Idle
    });
    assert_eq!(core.diff(&v.id, "turn"), Ok(String::new()));
    std::fs::write(repo.join("later.txt"), "x\n").unwrap();
    assert!(core.diff(&v.id, "turn").unwrap().contains("+++ b/later.txt"));

    core.send(&v.id, "Use the AskUserQuestion tool to ask me whether I prefer red or blue.", None).unwrap();
    wait_long("the question", 120, || status_of(&core, &v.id) == Status::NeedsYou);
    let note = note_of(&core, &v.id).unwrap_or_default();
    assert!(note.ends_with('?') && note != "Claude has a question", "the question itself: {note}");
    core.write(&v.id, b"\x1b").unwrap();
    wait_long("the turn to end", 60, || status_of(&core, &v.id) == Status::Idle);
    core.send(&v.id, "/exit", None).unwrap();
    wait_long("back to the shell", 30, || status_of(&core, &v.id) == Status::Shell);
    core.shutdown();
    let _ = std::fs::remove_dir_all(&repo);
}
