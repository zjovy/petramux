use petramux_lib::engine::{Attention, Core, NewSession, Setup, Sink, View};
use petramux_lib::paths::Paths;
use petramux_lib::shellenv::Env;
use petramux_lib::sock::{self, Request};
use petramux_lib::state::{Job, Status};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Rec {
    snapshots: Mutex<Vec<Vec<View>>>,
    changes: Mutex<Vec<(String, Status)>>,
    output: Mutex<String>,
    attention: Mutex<Vec<&'static str>>,
    jobs: Mutex<Vec<Vec<Job>>>,
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
    fn jobs_changed(&self, jobs: Vec<Job>) {
        self.jobs.lock().unwrap().push(jobs);
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
    setup_in(dir, script, Paths::new(dir.join("support")))
}

fn setup_in(dir: &Path, script: &str, paths: Paths) -> (Arc<Core>, Arc<Rec>, Paths) {
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
            roost_bin: PathBuf::from(env!("CARGO_BIN_EXE_petramux")),
            shell: PathBuf::from("/bin/zsh"),
        },
        rec.clone(),
    );
    sock::serve(&paths.sock(), core.handler(), Some(PathBuf::from(env!("CARGO_BIN_EXE_petramux")))).unwrap();
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

/// The fake claude's `ARGS:` line, once it has arrived whole (output can come in pieces).
fn args_line(rec: &Rec) -> Option<String> {
    let out = rec.output.lock().unwrap();
    let rest = &out[out.find("ARGS: ")?..];
    rest.find('\n').map(|end| rest[..end].to_string())
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
fn fork_copies_the_conversation_into_a_row_of_its_own() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let shell = new(&core, &repo, "");
    assert!(core.fork(&shell.id).is_err(), "a plain shell has no conversation to fork");

    let v = new(&core, &repo, "fix it");
    wait("SessionStart", || statuses(&rec, &v.id).contains(&Status::Idle));
    wait("the first claude's args", || args_line(&rec).is_some()); // output can trail the hook
    rec.output.lock().unwrap().clear();
    let f = core.fork(&v.id).unwrap();
    assert_eq!(f.parent.as_deref(), Some(v.id.as_str()));
    assert_eq!(f.name, "fix-it-fork");
    wait("the fork's claude", || args_line(&rec).is_some());
    assert!(args_line(&rec).unwrap().contains("--resume fake-claude-id --fork-session"), "{:?}", args_line(&rec));
    core.shutdown();
}

#[test]
fn claude_starts_in_the_ansi_theme_petramux_asks_for() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, paths) = setup(tmp.path());
    std::fs::write(paths.dir.join("claude-theme"), "light-ansi\n").unwrap();
    let a = new(&core, &repo, "one");
    wait("claude", || args_line(&rec).is_some());
    assert!(args_line(&rec).unwrap().contains(r#"--settings {"theme":"light-ansi"}"#), "{:?}", args_line(&rec));

    // no theme, or anything unexpected in the file: claude starts as the user set it up
    for content in ["", "dark; rm -rf ~"] {
        std::fs::write(paths.dir.join("claude-theme"), content).unwrap();
        rec.output.lock().unwrap().clear();
        let b = new(&core, &repo, "two");
        wait("claude", || args_line(&rec).is_some());
        assert!(!args_line(&rec).unwrap().contains("--settings"), "{:?}", args_line(&rec));
        core.close(&b.id);
    }
    core.close(&a.id);
    core.shutdown();
}

#[test]
fn settings_are_saved_and_survive_a_relaunch() {
    let tmp = tempfile::tempdir().unwrap();
    let (core, _, _) = setup(tmp.path());
    let mut s = core.settings();
    s.notify_done = false;
    s.new_session_folder = Some("/tmp".into());
    core.set_settings(s.clone());
    core.shutdown();
    drop(core);
    let (core2, _, _) = setup(tmp.path());
    assert_eq!(core2.settings(), s);
    core2.shutdown();
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
fn a_pane_showing_a_background_job_takes_the_job_s_status() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "fix it");
    wait("claude to finish its turn", || statuses(&rec, &v.id).len() >= 6);

    // a spinning title alone isn't work: Claude spins it for background commands too
    core.write(&v.id, b"/spin\r").unwrap(); // title "◐ working", no job of that name
    std::thread::sleep(std::time::Duration::from_millis(2500));
    assert_eq!(status_of(&core, &v.id), Status::Idle);

    // the title names a job Claude runs in its background service: its record is the truth
    let jobs = tmp.path().join("home/.claude/jobs/j1");
    std::fs::create_dir_all(&jobs).unwrap();
    let record = |state: &str, detail: &str| {
        let json = format!(r#"{{"name":"working","state":"{state}","detail":"{detail}","updatedAt":"2026-10-13T18:00:00Z"}}"#);
        std::fs::write(jobs.join("state.json"), json).unwrap();
    };
    record("blocked", "Four open items await your answers");
    wait("needs you, from the job", || status_of(&core, &v.id) == Status::NeedsYou);
    assert_eq!(note_of(&core, &v.id).as_deref(), Some("Four open items await your answers"));
    record("working", "Showing the indexes");
    wait("working, from the job", || status_of(&core, &v.id) == Status::Working);
    record("done", "PR safe to merge");
    wait("idle, from the job", || status_of(&core, &v.id) == Status::Idle);
    assert_eq!(note_of(&core, &v.id).as_deref(), Some("PR safe to merge"));
    core.shutdown();
}

#[test]
fn a_background_job_is_listed_until_a_pane_shows_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let dir = tmp.path().join("home/.claude/jobs/j9");
    std::fs::create_dir_all(&dir).unwrap();
    let record = |state: &str, needs: &str| {
        let json = format!(r#"{{"name":"attached","state":"{state}","needs":{needs},"cwd":"{repo}"}}"#);
        std::fs::write(dir.join("state.json"), json).unwrap();
    };
    record("working", "null");
    wait("the job listed", || core.jobs().iter().any(|j| j.id == "j9" && j.status == Status::Working));
    record("blocked", r#""ok to merge?""#);
    wait("the job needs you", || core.jobs().iter().any(|j| j.status == Status::NeedsYou));
    assert_eq!(*rec.attention.lock().unwrap(), vec!["needs_you"], "notified like a session");
    assert!(rec.jobs.lock().unwrap().last().is_some_and(|l| l[0].note.as_deref() == Some("ok to merge?")));

    // opened: `claude attach` in its folder; the pane takes its status and it leaves the list
    let v = core.open_job("j9").unwrap();
    assert_eq!((v.name.as_str(), v.workdir.as_str()), ("attached", repo.as_str()));
    assert!(core.jobs().is_empty());
    wait("claude attach to run", || args_line(&rec).is_some_and(|l| l.trim_end() == "ARGS: attach j9"));
    wait("the pane takes the job's status", || status_of(&core, &v.id) == Status::NeedsYou);
    assert!(core.jobs().is_empty(), "still shown in its pane");

    // leaving Claude's view puts it back on the list, and the pane is a shell again
    core.write(&v.id, b"/exit\r").unwrap();
    wait("the job back on the list", || core.jobs().iter().any(|j| j.id == "j9"));
    assert_eq!(status_of(&core, &v.id), Status::Shell);

    // done: off the list (nothing to do there), with the notification that it finished
    record("working", "null");
    wait("working again", || core.jobs().iter().any(|j| j.status == Status::Working));
    record("done", "null");
    wait("off the list once done", || core.jobs().is_empty());
    wait("told it's done", || rec.attention.lock().unwrap().last() == Some(&"done"));
    assert!(rec.jobs.lock().unwrap().last().is_some_and(|l| l.is_empty()), "the window's list too");
    core.shutdown();
}

#[test]
fn a_pane_showing_a_job_reattaches_after_a_relaunch_and_never_hides_it_meanwhile() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, paths) = setup(tmp.path());
    let dir = tmp.path().join("home/.claude/jobs/j9");
    std::fs::create_dir_all(&dir).unwrap();
    let json = format!(r#"{{"name":"attached","state":"working","cwd":"{repo}"}}"#);
    std::fs::write(dir.join("state.json"), json).unwrap();
    wait("the job listed", || core.jobs().iter().any(|j| j.id == "j9"));
    let v = core.open_job("j9").unwrap();
    wait("claude attach to run", || args_line(&rec).is_some_and(|l| l.trim_end() == "ARGS: attach j9"));
    wait("the pane takes the job's status", || status_of(&core, &v.id) == Status::Working);

    // the helper goes (a crash, a reboot): nothing shows the job now, so it's listed again
    let helper = format!("pty-host {}", paths.pty_sock().display());
    assert!(std::process::Command::new("/usr/bin/pkill").args(["-9", "-f", &helper]).status().unwrap().success());
    wait("the row to go asleep", || status_of(&core, &v.id) == Status::Asleep);
    wait("the job listed again", || core.jobs().iter().any(|j| j.id == "j9"));
    core.detach();
    drop(core);

    // relaunched, the row opens the job again
    let (core2, rec2, _) = setup(tmp.path());
    wait("the job listed after the relaunch", || core2.jobs().iter().any(|j| j.id == "j9"));
    core2.select(&v.id).unwrap();
    wait("reattached", || args_line(&rec2).is_some_and(|l| l.trim_end() == "ARGS: attach attached"));
    wait("shown in its pane again", || core2.jobs().is_empty());
    core2.shutdown();
}

#[test]
fn a_helper_that_dies_leaves_rows_to_resume_not_ended() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, paths) = setup(tmp.path());
    let v = new(&core, &repo, "fix it");
    wait("claude to finish its turn", || statuses(&rec, &v.id).len() >= 6);
    // the helper goes away (a crash, a kill in Activity Monitor) while the app is open
    let helper = format!("pty-host {}", paths.pty_sock().display());
    assert!(std::process::Command::new("/usr/bin/pkill").args(["-9", "-f", &helper]).status().unwrap().success());
    wait("the row to go asleep", || status_of(&core, &v.id) == Status::Asleep);
    std::thread::sleep(std::time::Duration::from_millis(500)); // Claude's SessionEnd arrives too
    assert_eq!(status_of(&core, &v.id), Status::Asleep);
    let saved = state_json(&paths);
    assert!(saved.contains("\"claudeRunning\": true") && !saved.contains("\"exited\": true"), "{saved}");
    core.detach();
    drop(core);

    let (core2, rec2, _) = setup(tmp.path());
    core2.select(&v.id).unwrap();
    wait("resumed", || args_line(&rec2).is_some_and(|l| l.contains("--resume fake-claude-id")));
    core2.shutdown();
}

#[test]
fn a_session_waiting_on_you_still_says_so_after_a_relaunch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "fix it");
    wait("claude to finish its turn", || statuses(&rec, &v.id).len() >= 6);
    core.write(&v.id, b"/ask\r").unwrap();
    wait("needs you", || status_of(&core, &v.id) == Status::NeedsYou);
    core.detach();
    drop(core);

    let (core2, _, _) = setup(tmp.path());
    assert_eq!(status_of(&core2, &v.id), Status::NeedsYou);
    core2.shutdown();
}

#[test]
fn moving_the_roost_folder_keeps_sessions_running() {
    // the real folder layout, under a throwaway home (in /tmp: socket paths max out at 104 bytes)
    let tmp = tempfile::Builder::new().tempdir_in("/tmp").unwrap();
    let repo = repo(&tmp);
    let home = tmp.path().join("home");
    let old = Paths::new(home.join("Library/Application Support/Roost"));
    let (core, rec, _) = setup_in(tmp.path(), "fake-claude.sh", old.clone());
    let v = new(&core, &repo, "fix it");
    wait("claude to finish its turn", || statuses(&rec, &v.id).len() >= 6);
    core.detach(); // the old app quits; its helper keeps the session
    drop(core);

    petramux_lib::paths::migrate(&home).unwrap();
    // the helper ends its sessions if its socket vanishes: give it time to look
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let new_dir = Paths::new(home.join("Library/Application Support/petramux"));
    assert!(new_dir.state().exists());
    let (core2, rec2, _) = setup_in(tmp.path(), "fake-claude.sh", new_dir);
    assert_eq!(status_of(&core2, &v.id), Status::Idle, "the same session, still running");
    core2.write(&v.id, b"still here\r").unwrap();
    wait("the same claude answering", || rec2.output.lock().unwrap().contains("GOT: still here"));
    core2.shutdown();
}

#[test]
fn a_pane_s_job_is_remembered_across_a_relaunch() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "fix it");
    wait("claude to finish its turn", || statuses(&rec, &v.id).len() >= 6);
    let jobs = tmp.path().join("home/.claude/jobs/j1");
    std::fs::create_dir_all(&jobs).unwrap();
    let record = |state: &str, detail: &str| {
        let json = format!(r#"{{"name":"working","state":"{state}","detail":"{detail}","updatedAt":"2026-10-13T18:00:00Z"}}"#);
        std::fs::write(jobs.join("state.json"), json).unwrap();
    };
    record("working", "Busy");
    core.write(&v.id, b"/spin\r").unwrap(); // the title names job "working"
    wait("working, from the job", || status_of(&core, &v.id) == Status::Working);
    // the replay won't hold the job's title (cut away, or a later plain title)
    core.write(&v.id, b"/plaintitle\r").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    core.detach();
    drop(core);

    // the job now waits on the user, and Claude sets no new title: the saved name still finds it
    record("blocked", "Go?");
    let (core2, _, _) = setup(tmp.path());
    wait("needs you, from the remembered job", || status_of(&core2, &v.id) == Status::NeedsYou);
    assert_eq!(note_of(&core2, &v.id).as_deref(), Some("Go?"));
    core2.shutdown();
}

#[test]
fn a_session_kept_running_redraws_once_the_new_window_sizes_it() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "fix it");
    wait("claude to finish its turn", || statuses(&rec, &v.id).len() >= 6);
    core.resize(&v.id, 100, 30);
    wait("drawn at its size", || rec.output.lock().unwrap().contains("REDRAWN"));
    core.detach();
    drop(core);

    // the new window's pane is the same size: no size change, yet its replay was drawn before
    // the pane had a size, so Claude must draw again
    let (core2, rec2, _) = setup(tmp.path());
    wait("its earlier output, replayed", || args_line(&rec2).is_some());
    let before = rec2.output.lock().unwrap().matches("REDRAWN").count();
    core2.resize(&v.id, 100, 30);
    wait("claude drawing again", || rec2.output.lock().unwrap().matches("REDRAWN").count() > before);
    // only once: later sizes are the user's
    std::thread::sleep(Duration::from_millis(500));
    let once = rec2.output.lock().unwrap().matches("REDRAWN").count();
    core2.resize(&v.id, 100, 30);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(rec2.output.lock().unwrap().matches("REDRAWN").count(), once);
    core2.shutdown();
}

#[test]
fn a_running_session_survives_the_app_quitting() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = repo(&tmp);
    let (core, rec, _) = setup(tmp.path());
    let v = new(&core, &repo, "fix it");
    wait("claude to finish its turn", || statuses(&rec, &v.id).len() >= 6);
    core.detach(); // quit: the helper keeps the terminal
    drop(core);

    let (core2, rec2, _) = setup(tmp.path());
    assert_eq!(status_of(&core2, &v.id), Status::Idle, "live with Claude, not asleep");
    wait("its earlier output, replayed", || args_line(&rec2).is_some());
    core2.write(&v.id, b"hello again\r").unwrap();
    wait("the same claude answering", || rec2.output.lock().unwrap().contains("GOT: hello again"));
    assert_eq!(rec2.output.lock().unwrap().matches("ARGS: ").count(), 1, "claude was not started again");
    core2.shutdown();
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
    wait("resume", || args_line(&rec2).is_some_and(|l| l.contains("--resume fake-claude-id")));
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
    wait("claude", || args_line(&rec2).is_some());
    assert!(!args_line(&rec2).unwrap().contains("--resume"), "{:?}", args_line(&rec2));
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
    wait("resumed", || args_line(&rec2).is_some());
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

/// Real Claude with the real login (two short turns), in a throwaway `src-tauri/target/smoke-repo`.
/// `ROOST_TEST_CLAUDE=<path to claude> cargo test --test flow real_claude -- --ignored --nocapture`
#[test]
#[ignore]
fn real_claude_reports_notes_questions_and_turn_snapshots() {
    let claude = PathBuf::from(std::env::var("ROOST_TEST_CLAUDE").expect("set ROOST_TEST_CLAUDE"));
    let tmp = tempfile::tempdir().unwrap();
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
            roost_bin: PathBuf::from(env!("CARGO_BIN_EXE_petramux")),
            shell: PathBuf::from("/bin/zsh"),
        },
        rec.clone(),
    );
    sock::serve(&paths.sock(), core.handler(), Some(PathBuf::from(env!("CARGO_BIN_EXE_petramux")))).unwrap();
    let repo_s = repo.display().to_string();
    let v = new(&core, &repo_s, "reply with exactly: hi");
    // A folder Claude hasn't seen gets its trust question first ("No, exit" is chosen): answered
    // as you would, ↓ to "Yes, I trust this folder" and Enter.
    wait_long("claude to start", 60, || {
        status_of(&core, &v.id) == Status::NeedsYou || statuses(&rec, &v.id).contains(&Status::Working)
    });
    if status_of(&core, &v.id) == Status::NeedsYou {
        std::thread::sleep(Duration::from_secs(1)); // the question takes keys once it's drawn
        core.write(&v.id, b"\x1b[B").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        core.write(&v.id, b"\r").unwrap();
    }
    wait_long("the reply as the note", 120, || {
        note_of(&core, &v.id).is_some_and(|n| n.to_lowercase().starts_with("hi")) && status_of(&core, &v.id) == Status::Idle
    });
    assert!(statuses(&rec, &v.id).contains(&Status::Working), "the turn never showed as working");
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
